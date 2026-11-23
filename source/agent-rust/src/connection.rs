use crate::{
    Result,
    config::Config,
    wire::{MAX_CHUNK, MAX_MESSAGE, Message, Metrics, WINDOW},
};
use futures_util::{SinkExt, StreamExt};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::{Semaphore, mpsc, watch},
    task::JoinSet,
    time::{Instant, timeout},
};
use tokio_tungstenite::{
    Connector, connect_async_tls_with_config,
    tungstenite::{
        client::IntoClientRequest,
        http::HeaderValue,
        protocol::{Message as WsMessage, WebSocketConfig},
    },
};
use tokio_util::sync::CancellationToken;
const IO_TIMEOUT: Duration = Duration::from_secs(5);
type Outgoing = crate::transport_queue::Sender<WsMessage>;
async fn send(out: &Outgoing, msg: Message) -> Result<()> {
    let bulk = crate::transport_queue::bulk(&msg.kind);
    let frame = WsMessage::Text(msg.encode()?.into());
    if bulk {
        timeout(IO_TIMEOUT, out.send_bulk(frame))
            .await
            .map_err(|_| "tunnel output stalled")?
            .map_err(|_| "tunnel output closed")
    } else {
        out.try_control(frame)
            .map_err(|_| "control queue unavailable")
    }
}
struct Tunnel {
    generation: u64,
    input: mpsc::Sender<Vec<u8>>,
    credits: Arc<Semaphore>,
    cancel: CancellationToken,
}
impl Drop for Tunnel {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
enum TaskEnd {
    Writer,
    Tunnel(String, u64),
}
async fn tunnel(
    id: &str,
    port: u16,
    mut input: mpsc::Receiver<Vec<u8>>,
    credits: Arc<Semaphore>,
    out: Outgoing,
    cancel: CancellationToken,
) {
    let connect = timeout(
        IO_TIMEOUT,
        TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)),
    );
    let stream = tokio::select! {_=cancel.cancelled()=>return,result=connect=>match result{
        Ok(Ok(stream))=>stream,
        _=>{let mut reply=Message::new("ssh_ready",id);reply.error="local SSH service unavailable".into();let _=send(&out,reply).await;return;}
    }};
    let _ = stream.set_nodelay(true);
    if send(&out, Message::new("ssh_ready", id)).await.is_err() {
        return;
    }
    let (mut reader, mut writer) = stream.into_split();
    let upload = async {
        let mut buf = vec![0; MAX_CHUNK];
        loop {
            let n = reader
                .read(&mut buf)
                .await
                .map_err(|_| "local SSH read failed")?;
            if n == 0 {
                return Ok::<(), &'static str>(());
            }
            credits
                .acquire()
                .await
                .map_err(|_| "tunnel closed")?
                .forget();
            send(&out, Message::bytes("ssh_data", id, &buf[..n])).await?;
        }
    };
    let download = async {
        while let Some(data) = input.recv().await {
            timeout(IO_TIMEOUT, writer.write_all(&data))
                .await
                .map_err(|_| "local SSH write stalled")?
                .map_err(|_| "local SSH write failed")?;
            send(&out, Message::new("ssh_ack", id)).await?;
        }
        Ok::<(), &'static str>(())
    };
    tokio::select! {_=cancel.cancelled()=>{},_=upload=>{},_=download=>{}}
    let _ = send(&out, Message::new("ssh_close", id)).await;
}
pub fn tls_config() -> Result<Arc<rustls::ClientConfig>> {
    let native = rustls_native_certs::load_native_certs();
    let mut roots = rustls::RootCertStore::empty();
    for cert in native.certs {
        roots
            .add(cert)
            .map_err(|_| "trusted certificate store invalid")?;
    }
    if roots.is_empty() {
        return Err("trusted certificate store unavailable");
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|_| "TLS configuration invalid")?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Arc::new(config))
}
pub async fn connect(
    config: &Config,
    health: &watch::Sender<Option<crate::management::HealthAck>>,
    generation_id: u64,
    tls: Arc<rustls::ClientConfig>,
    mut metrics: watch::Receiver<Option<Metrics>>,
    stop: CancellationToken,
) -> Result<()> {
    config.validate()?;
    let mut request = config
        .controller_url
        .as_str()
        .into_client_request()
        .map_err(|_| "control request invalid")?;
    request.headers_mut().insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {}", config.token))
            .map_err(|_| "node credential invalid")?,
    );
    request.headers_mut().insert(
        "X-Probe-Node",
        HeaderValue::from_str(&config.node_id).map_err(|_| "node identifier invalid")?,
    );
    let ws_config = WebSocketConfig::default()
        .read_buffer_size(16 * 1024)
        .write_buffer_size(0)
        .max_write_buffer_size(2 * MAX_MESSAGE)
        .max_message_size(Some(MAX_MESSAGE))
        .max_frame_size(Some(MAX_MESSAGE));
    // The connector verifies the peer certificate and the configured hostname. No insecure TLS switch.
    let (ws, _) = timeout(
        Duration::from_secs(10),
        connect_async_tls_with_config(request, Some(ws_config), true, Some(Connector::Rustls(tls))),
    )
    .await
    .map_err(|_| "control connection timed out")?
    .map_err(|_| "control connection unavailable")?;
    let (mut writer, mut reader) = ws.split();
    let (out, mut outgoing) =
        crate::transport_queue::channel::<WsMessage>(crate::wire::MAX_TUNNELS * WINDOW + 16, 32);
    let mut tasks = JoinSet::new();
    tasks.spawn(async move {
        while let Some(msg) = outgoing.recv().await {
            if !matches!(timeout(IO_TIMEOUT, writer.send(msg)).await, Ok(Ok(()))) {
                break;
            }
        }
        TaskEnd::Writer
    });
    // JoinSet aborts every child on return, including a connection future dropped by shutdown.
    let mut tunnels: HashMap<String, Tunnel> = HashMap::new();
    let mut generation = 0u64;
    let mut sequence = 0u64;
    let mut last = Instant::now();
    let mut window = last;
    let (mut frames, mut traffic) = (0usize, 0usize);
    metrics.borrow_and_update();
    eprintln!("WSS control channel connected");
    loop {
        tokio::select! {
            _=stop.cancelled()=>return Ok(()),
            _=tokio::time::sleep_until(last+Duration::from_secs(20))=>return Err("control channel timed out"),
            ended=tasks.join_next()=>match ended{
                Some(Ok(TaskEnd::Tunnel(id,gen_id)))=>{if tunnels.get(&id).is_some_and(|t|t.generation==gen_id){tunnels.remove(&id);}},
                _=>return Err("control writer stopped"),
            },
            changed=metrics.changed()=>{
                if changed.is_err(){return Err("metric sampler stopped");}
                let sample=metrics.borrow_and_update().clone();
                if let Some(sample)=sample{
                    sequence=sequence.checked_add(1).ok_or("metric sequence exhausted")?;
                    let mut msg=Message::new("metrics","");msg.sequence=sequence;msg.metrics=Some(sample);send(&out,msg).await?;
                }
            },
            next=reader.next()=>{
                let raw=match next{
                    Some(Ok(WsMessage::Text(raw)))=>raw,
                    Some(Ok(WsMessage::Ping(raw)))=>{out.try_control(WsMessage::Pong(raw)).map_err(|_|"control queue unavailable")?;continue;},
                    Some(Ok(WsMessage::Pong(_)))=>continue,
                    _=>return Err("control channel closed"),
                };
                let msg=Message::parse(raw.as_bytes())?;last=Instant::now();
                if last.duration_since(window)>=Duration::from_secs(1){window=last;frames=0;traffic=0;}
                frames+=1;traffic=traffic.saturating_add(msg.data.len());
                if frames>2000||traffic>12*1024*1024{return Err("control traffic limit reached");}
                match msg.kind.as_str(){
                    "ping"=>{if msg.session.len()!=32{return Err("invalid latency probe");}send(&out,Message::new("pong",&msg.session)).await?;},
                    "ack"=>{health.send_replace(Some(crate::management::HealthAck::new(generation_id)));},
                    "ssh_open"=>{
                        if msg.session.len()!=32{return Err("invalid terminal session");}
                        if tunnels.contains_key(&msg.session)||tunnels.len()>=crate::wire::MAX_TUNNELS{
                            let mut reply=Message::new("ssh_ready",&msg.session);reply.error="terminal capacity reached".into();send(&out,reply).await?;continue;
                        }
                        generation=generation.checked_add(1).ok_or("session generation exhausted")?;
                        let(input,rx)=mpsc::channel(WINDOW);let credits=Arc::new(Semaphore::new(WINDOW));let cancel=stop.child_token();
                        tunnels.insert(msg.session.clone(),Tunnel{generation,input,credits:credits.clone(),cancel:cancel.clone()});
                        let output=out.clone();let port=config.ssh_port;let id=msg.session;
                        tasks.spawn(async move{tunnel(&id,port,rx,credits,output,cancel).await;TaskEnd::Tunnel(id,generation)});
                    },
                    "ssh_ack"=>{if let Some(t)=tunnels.get(&msg.session){if t.credits.available_permits()<WINDOW{t.credits.add_permits(1);}}},
                    "ssh_data"=>{
                        let data=msg.decode_data()?;
                        if tunnels.get(&msg.session).is_some_and(|t|t.input.try_send(data).is_err()){tunnels.remove(&msg.session);}
                    },
                    "ssh_close"=>{tunnels.remove(&msg.session);},
                    _=>return Err("unsupported control message"),
                }
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn trusted_roots_and_tls_versions_available() {
        assert!(tls_config().is_ok());
    }
    #[tokio::test]
    async fn tunnel_window_is_bounded() {
        let s = Semaphore::new(WINDOW);
        for _ in 0..WINDOW {
            s.acquire().await.unwrap().forget();
        }
        assert!(
            timeout(Duration::from_millis(10), s.acquire())
                .await
                .is_err()
        );
        s.add_permits(1);
        assert!(s.try_acquire().is_ok());
    }
}
