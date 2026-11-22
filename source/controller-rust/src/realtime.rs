use crate::{core::*, model::*};
use axum::{
    extract::{
        ConnectInfo, State,
        ws::{Message as WS, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, Method},
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context as TaskContext, Poll},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream, ReadBuf},
    sync::{Semaphore, mpsc, oneshot},
    time::timeout,
};
use tokio_util::sync::CancellationToken;
use vistart_probe_agent::wire::{MAX_CHUNK, MAX_MESSAGE, Metrics, WINDOW};
#[derive(Default, Serialize, Deserialize)]
pub struct Message {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session: String,
    #[serde(default)]
    pub sequence: u64,
    #[serde(
        default,
        deserialize_with = "compatible_metrics",
        skip_serializing_if = "Option::is_none"
    )]
    pub metrics: Option<Metrics>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub data: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
}
fn compatible_metrics<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Metrics>, D::Error> {
    let Some(mut v) = Option::<Value>::deserialize(d)? else {
        return Ok(None);
    };
    let Some(object) = v.as_object_mut() else {
        return Err(serde::de::Error::custom("metrics must be an object"));
    };
    for (k, value) in [
        ("network_available", json!(false)),
        ("latency_probe", json!(false)),
        ("network", json!([])),
    ] {
        if object.get(k).is_none_or(Value::is_null) {
            object.insert(k.into(), value);
        }
    }
    serde_json::from_value(v)
        .map(Some)
        .map_err(serde::de::Error::custom)
}
impl Message {
    pub fn new(kind: &str, session: &str) -> Self {
        Self {
            kind: kind.into(),
            session: session.into(),
            ..Default::default()
        }
    }
    pub fn parse(raw: &[u8]) -> Result<Self, &'static str> {
        if raw.len() > MAX_MESSAGE {
            return Err("frame too large");
        }
        let m: Self = serde_json::from_slice(raw).map_err(|_| "invalid control JSON")?;
        if m.session.len() > 64 || m.kind.len() > 32 || m.data.len() > MAX_CHUNK.div_ceil(3) * 4 {
            return Err("invalid frame");
        };
        if !m.data.is_empty() {
            m.bytes()?;
        }
        Ok(m)
    }
    pub fn bytes(&self) -> Result<Vec<u8>, &'static str> {
        let v = STANDARD
            .decode(&self.data)
            .map_err(|_| "invalid tunnel encoding")?;
        if v.len() > MAX_CHUNK {
            return Err("chunk too large");
        }
        Ok(v)
    }
}
pub type Output = vistart_probe_agent::transport_queue::Sender<WS>;
pub async fn send(out: &Output, message: Message) -> Result<(), &'static str> {
    let bulk = vistart_probe_agent::transport_queue::bulk(&message.kind);
    let raw = serde_json::to_string(&message).map_err(|_| "encoding failed")?;
    if raw.len() > MAX_MESSAGE {
        return Err("frame too large");
    }
    let frame = WS::Text(raw.into());
    if bulk {
        timeout(Duration::from_secs(5), out.send_bulk(frame))
            .await
            .map_err(|_| "tunnel writer timeout")?
            .map_err(|_| "tunnel writer closed")
    } else {
        out.try_control(frame)
            .map_err(|_| "control writer unavailable")
    }
}
struct TunnelSlot {
    input: mpsc::Sender<Vec<u8>>,
    credits: Arc<Semaphore>,
    ready: Option<oneshot::Sender<bool>>,
    stop: CancellationToken,
}
pub struct AgentLink {
    pub stop: CancellationToken,
    pub output: Output,
    tunnels: Mutex<HashMap<String, TunnelSlot>>,
}
struct PendingTunnel {
    link: Arc<AgentLink>,
    id: String,
    armed: bool,
}
impl Drop for PendingTunnel {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        if let Some(slot) = self.link.tunnels.lock().unwrap().remove(&self.id) {
            slot.stop.cancel();
        }
        let raw = serde_json::to_string(&Message::new("ssh_close", &self.id)).unwrap();
        if self.link.output.try_bulk(WS::Text(raw.into())).is_err() {
            self.link.stop.cancel();
        }
    }
}
pub struct Tunnel {
    io: DuplexStream,
    stop: CancellationToken,
}
impl Drop for Tunnel {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}
impl AsyncRead for Tunnel {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        b: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_read(cx, b)
    }
}
impl AsyncWrite for Tunnel {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        b: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.io).poll_write(cx, b)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.io).poll_shutdown(cx)
    }
}
impl AgentLink {
    pub async fn open(self: &Arc<Self>) -> Result<Tunnel, &'static str> {
        let id = token()[..32].to_string();
        let (local, other) = tokio::io::duplex(MAX_CHUNK * WINDOW);
        let (input, mut incoming) = mpsc::channel(WINDOW);
        let (ready, wait) = oneshot::channel();
        let credits = Arc::new(Semaphore::new(WINDOW));
        let stop = self.stop.child_token();
        {
            let mut all = self.tunnels.lock().unwrap();
            if all.len() >= vistart_probe_agent::wire::MAX_TUNNELS {
                return Err("node tunnel capacity reached");
            }
            all.insert(
                id.clone(),
                TunnelSlot {
                    input,
                    credits: credits.clone(),
                    ready: Some(ready),
                    stop: stop.clone(),
                },
            );
        }
        let mut pending = PendingTunnel {
            link: self.clone(),
            id: id.clone(),
            armed: true,
        };
        if send(&self.output, Message::new("ssh_open", &id))
            .await
            .is_err()
            || !matches!(timeout(Duration::from_secs(7), wait).await, Ok(Ok(true)))
        {
            self.tunnels.lock().unwrap().remove(&id);
            let _ = send(&self.output, Message::new("ssh_close", &id)).await;
            return Err("agent SSH tunnel unavailable");
        }
        pending.armed = false;
        let link = self.clone();
        let cancel = stop.clone();
        tokio::spawn(async move {
            let (mut reader, mut writer) = tokio::io::split(other);
            let upload = async {
                let mut buf = vec![0; MAX_CHUNK];
                loop {
                    let n = reader
                        .read(&mut buf)
                        .await
                        .map_err(|_| "tunnel read failed")?;
                    if n == 0 {
                        return Ok::<(), &'static str>(());
                    }
                    credits
                        .acquire()
                        .await
                        .map_err(|_| "tunnel closed")?
                        .forget();
                    let mut msg = Message::new("ssh_data", &id);
                    msg.data = STANDARD.encode(&buf[..n]);
                    send(&link.output, msg).await?;
                }
            };
            let download = async {
                while let Some(data) = incoming.recv().await {
                    timeout(Duration::from_secs(30), writer.write_all(&data))
                        .await
                        .map_err(|_| "tunnel write timeout")?
                        .map_err(|_| "tunnel write failed")?;
                    send(&link.output, Message::new("ssh_ack", &id)).await?;
                }
                Ok::<(), &'static str>(())
            };
            tokio::select! {_=cancel.cancelled()=>{},_=upload=>{},_=download=>{}}
            link.tunnels.lock().unwrap().remove(&id);
            let _ = send(&link.output, Message::new("ssh_close", &id)).await;
        });
        Ok(Tunnel { io: local, stop })
    }
}
// A delayed latency measurement must not tear down an authenticated agent
// and all of its SSH tunnels. Only a matching, timely reply updates the gauge;
// valid metrics still enforce the separate connection liveness deadline.
fn latency_reply(pending: &mut Option<(String, Instant)>, nonce: &str, at: Instant) -> Option<f64> {
    let (expected, sent) = pending.as_ref()?;
    if !constant(expected, nonce) {
        return None;
    }
    let elapsed = at.duration_since(*sent);
    *pending = None;
    (elapsed <= Duration::from_secs(10)).then(|| (elapsed.as_secs_f64() * 10000.0).round() / 10.0)
}

pub fn valid_metrics(m: &Metrics) -> bool {
    if !m.cpu.is_finite()
        || !(0.0..=100.0).contains(&m.cpu)
        || !(1..=8192).contains(&m.cores)
        || !valid_text(&m.cpu_model, 256)
        || !valid_text(&m.system, 128)
        || !valid_text(&m.arch, 16)
        || !valid_text(&m.version, 32)
        || m.memory_total == 0
        || m.memory_total > 1 << 60
        || m.memory_used > m.memory_total
        || m.swap_total > 1 << 60
        || m.swap_used > m.swap_total
        || m.volumes.len() > 16
    {
        return false;
    }
    for v in &m.volumes {
        if !valid_text(&v.name, 40) || v.total == 0 || v.total > 1 << 60 || v.used > v.total {
            return false;
        }
    }
    if !m.country.is_empty()
        && (m.country.len() != 2 || !m.country.bytes().all(|b| b.is_ascii_uppercase()))
    {
        return false;
    }
    if !m.public_ip.is_empty() && m.public_ip.parse::<std::net::IpAddr>().is_err() {
        return false;
    }
    if m.network.len() > 64 || (!m.network_available && !m.network.is_empty()) {
        return false;
    }
    let mut seen = HashSet::new();
    for n in &m.network {
        if n.name.is_empty()
            || n.name.len() > 15
            || [".", "..", "lo"].contains(&n.name.as_str())
            || !n
                .name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_.:@-".contains(&c))
            || !seen.insert(&n.name)
            || n.rx_bytes > 1 << 60
            || n.tx_bytes > 1 << 60
            || ![
                "up",
                "down",
                "unknown",
                "dormant",
                "lowerlayerdown",
                "notpresent",
                "testing",
            ]
            .contains(&n.state.as_str())
        {
            return false;
        }
        for rate in [n.rx_rate, n.tx_rate].into_iter().flatten() {
            if !rate.is_finite() || !(0.0..=1e14).contains(&rate) {
                return false;
            }
        }
    }
    true
}
fn apply(i: &mut Inner, id: &str, m: Metrics) -> bool {
    let Some(n) = i.data.nodes.iter_mut().find(|n| n.public.id == id) else {
        return false;
    };
    let gib = |v: u64| v as f64 / (1024.0 * 1024.0 * 1024.0);
    n.public.network_available = m.network_available;
    n.public.network = m.network;
    n.last_seen = now();
    n.agent_version = m.version;
    if n.deploy_state == "enrolling" {
        n.deploy_state = "done".into();
        n.deploy_message = "Agent 已连接，监控数据已就绪".into();
    }
    n.demo = false;
    n.public.online = true;
    n.public.pending = false;
    n.public.cpu = Some(m.cpu);
    n.public.cpu_model = m.cpu_model;
    n.public.cores = m.cores as i64;
    n.public.arch = m.arch;
    n.public.system = m.system;
    n.public.memory = Resource {
        used: Some(gib(m.memory_used)),
        total: gib(m.memory_total),
    };
    n.public.swap = if m.swap_total > 0 {
        Some(Resource {
            used: Some(gib(m.swap_used)),
            total: gib(m.swap_total),
        })
    } else {
        None
    };
    n.public.disks = m
        .volumes
        .into_iter()
        .map(|v| Disk {
            name: v.name,
            used: Some(gib(v.used)),
            total: gib(v.total),
        })
        .collect();
    n.public.uptime = Some(format!(
        "{} 天 {} 小时",
        m.uptime / 86400,
        m.uptime % 86400 / 3600
    ));
    n.public.history.push(m.cpu);
    if n.public.history.len() > 40 {
        n.public.history.drain(..n.public.history.len() - 40);
    }
    n.public.last_seen_minutes = 0;
    if n.country_auto && !m.country.is_empty() {
        n.public.code = country_code(&m.country);
        n.public.country = country_name(&m.country);
        if n.public.city == "待识别" {
            n.public.city.clear();
        }
    }
    if !m.public_ip.is_empty() {
        n.detected_ip = m.public_ip;
    }
    true
}

pub(crate) fn authorize_agent(i: &mut Inner, c: &Context) -> ApiResult<(String, String)> {
    let denied = |i: &mut Inner, status, message| -> ApiResult<(String, String)> {
        i.request(&format!("agent:{}", c.ip), 120)?;
        Err(ApiError::new(status, message))
    };
    if c.method != "GET" || !c.header("Origin").is_empty() {
        return denied(i, 403, "agent origin rejected");
    }
    let id = c.header("X-Probe-Node");
    let value = c
        .header("Authorization")
        .strip_prefix("Bearer ")
        .unwrap_or("");
    if id.len() > 64 || value.len() != 64 {
        return denied(i, 401, "agent authentication required");
    }
    let digest = hex::encode(Sha256::digest(value.as_bytes()));
    if !i
        .data
        .secrets
        .get(id)
        .is_some_and(|s| constant(&s.token_hash, &digest))
    {
        return denied(i, 401, "agent authentication failed");
    }
    // Valid nodes behind the same NAT do not consume each other's failure budget.
    i.request(&format!("agent-node:{id}"), 30)?;
    if i.agents.contains_key(id) || i.agent_reserved.contains(id) {
        return Err(ApiError::new(409, "agent already connected"));
    }
    Ok((id.to_string(), digest))
}
pub async fn agent_handler(
    State(app): State<App>,
    ConnectInfo(remote): ConnectInfo<SocketAddr>,
    method: Method,
    headers: HeaderMap,
    upgrade: Result<WebSocketUpgrade, axum::extract::ws::rejection::WebSocketUpgradeRejection>,
) -> Response {
    if !app.0.origin.starts_with("https://") || !remote.ip().is_loopback() {
        return ApiError::new(403, "HTTPS reverse proxy is required").into_response();
    }
    let c = Context::new(
        method.as_str(),
        "/api/agent",
        headers,
        remote,
        &app.0.origin,
    );
    let validation = authorize_agent(&mut app.lock(), &c);
    let (id, digest) = match validation {
        Ok(v) => v,
        Err(e) => return e.into_response(),
    };
    let ws = match upgrade {
        Ok(v) => v,
        Err(_) => return ApiError::new(400, "WebSocket upgrade required").into_response(),
    };
    {
        let mut i = app.lock();
        if i.agents.contains_key(&id) || !i.agent_reserved.insert(id.clone()) {
            return ApiError::new(409, "agent already connected").into_response();
        }
    }
    let failed_app = app.clone();
    let failed_id = id.clone();
    ws.read_buffer_size(16 * 1024)
        .write_buffer_size(0)
        .max_write_buffer_size(MAX_MESSAGE * 2)
        .max_message_size(MAX_MESSAGE)
        .max_frame_size(MAX_MESSAGE)
        .on_failed_upgrade(move |_| {
            failed_app.lock().agent_reserved.remove(&failed_id);
        })
        .on_upgrade(move |socket| agent_loop(app, id, digest, c.ip, socket))
}
async fn agent_loop(app: App, id: String, digest: String, peer_ip: String, ws: WebSocket) {
    let (mut writer, mut reader) = ws.split();
    let (out, mut rx) = vistart_probe_agent::transport_queue::channel::<WS>(
        vistart_probe_agent::wire::MAX_TUNNELS * WINDOW + 16,
        32,
    );
    let link = Arc::new(AgentLink {
        stop: app.0.stop.child_token(),
        output: out,
        tunnels: Mutex::new(HashMap::new()),
    });
    {
        let mut i = app.lock();
        i.agent_reserved.remove(&id);
        if !i
            .data
            .secrets
            .get(&id)
            .is_some_and(|s| constant(&s.token_hash, &digest))
        {
            return;
        }
        if acceptable_ip(&peer_ip)
            && let Some(node) = i.data.nodes.iter_mut().find(|n| n.public.id == id)
        {
            node.agent_peer_ip = peer_ip;
        }
        i.agents.insert(id.clone(), link.clone());
        app.record(&mut i, "agent_connected", &id);
    }
    let stop = link.stop.clone();
    let writer_task = tokio::spawn(async move {
        loop {
            tokio::select! {_=stop.cancelled()=>break,m=rx.recv()=>{let Some(m)=m else{break};if !matches!(timeout(Duration::from_secs(5),writer.send(m)).await,Ok(Ok(()))){break}}}
        }
        stop.cancel();
    });
    let result=async{let(mut sequence,mut frames,mut traffic)=(0u64,0usize,0usize);let mut window=Instant::now();let mut last=Instant::now();let mut rate_time=last;let mut credits=4.0f64;let mut ping:Option<(String,Instant)>=None;
 loop{let next=tokio::select!{_=link.stop.cancelled()=>return Ok::<(),&'static str>(()),_=tokio::time::sleep_until((last+Duration::from_secs(15)).into())=>return Err("metrics timeout"),v=reader.next()=>v};let raw=match next{Some(Ok(WS::Text(s)))=>s,Some(Ok(WS::Ping(b)))=>{link.output.try_control(WS::Pong(b)).map_err(|_|"control writer unavailable")?;continue},Some(Ok(WS::Pong(_)))=>continue,_=>return Err("agent connection closed")};let m=Message::parse(raw.as_bytes())?;let t=Instant::now();if t.duration_since(window)>=Duration::from_secs(1){window=t;frames=0;traffic=0}frames+=1;let bytes=m.bytes()?;traffic=traffic.saturating_add(bytes.len());if frames>2000||traffic>8*1024*1024{return Err("agent traffic limit")}
 match m.kind.as_str(){"metrics"=>{let metrics=m.metrics.ok_or("metrics missing")?;credits=(credits+t.duration_since(rate_time).as_secs_f64()*4.0).min(4.0);rate_time=t;if m.sequence<=sequence||credits<1.0||!valid_metrics(&metrics){return Err("invalid metrics")};credits-=1.0;sequence=m.sequence;let latency=metrics.latency_probe;{let mut i=app.lock();if !apply(&mut i,&id,metrics){return Err("node removed")}
}last=t;let mut ack=Message::new("ack","");ack.sequence=sequence;send(&link.output,ack).await?;if latency&&ping.as_ref().is_none_or(|(_,at)|at.elapsed()>Duration::from_secs(10)){let nonce=token()[..32].to_string();send(&link.output,Message::new("ping",&nonce)).await?;ping=Some((nonce,Instant::now()));}},"pong"=>{if let Some(ms)=latency_reply(&mut ping,&m.session,Instant::now()){let mut i=app.lock();if let Some(n)=i.data.nodes.iter_mut().find(|n|n.public.id==id){n.public.latency_ms=Some(ms);n.latency_at=now();}}},"ssh_ready"=>{let mut all=link.tunnels.lock().unwrap();if let Some(slot)=all.get_mut(&m.session)&& let Some(ready)=slot.ready.take(){let _=ready.send(m.error.is_empty());}},"ssh_ack"=>{if let Some(slot)=link.tunnels.lock().unwrap().get(&m.session)&& slot.credits.available_permits()<WINDOW{slot.credits.add_permits(1);}},"ssh_data"=>{if let Some(slot)=link.tunnels.lock().unwrap().get(&m.session)&& slot.input.try_send(bytes).is_err(){slot.stop.cancel();}},"ssh_close"=>{if let Some(slot)=link.tunnels.lock().unwrap().get(&m.session){slot.stop.cancel();}},_=>return Err("unsupported agent message")}
 }}.await;
    let disconnect_reason = result.err();
    link.stop.cancel();
    writer_task.abort();
    let _ = writer_task.await;
    let mut i = app.lock();
    if i.agents.get(&id).is_some_and(|a| Arc::ptr_eq(a, &link)) {
        i.agents.remove(&id);
        if let Some(n) = i.data.nodes.iter_mut().find(|n| n.public.id == id) {
            n.public.online = false;
            n.public.latency_ms = None;
        }
        if let Some(reason) = disconnect_reason {
            app.record(
                &mut i,
                "agent_connection_error",
                &format!("{id} · {reason}"),
            );
        }
        app.record(&mut i, "agent_disconnected", &id);
    }
}
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct TicketInput {
    id: String,
}
pub fn ticket(app: &App, i: &mut Inner, c: &Context, body: &[u8]) -> ApiResult<ApiReply> {
    let session = app.guard(i, c, true, true)?;
    let v: TicketInput = decode(body)?;
    if !i
        .data
        .nodes
        .iter()
        .any(|n| n.public.id == v.id && !n.removing)
    {
        return Err(ApiError::new(403, "该节点仅允许监控"));
    }
    if !i.agents.contains_key(&v.id)
        || !i
            .data
            .secrets
            .get(&v.id)
            .is_some_and(|s| !s.ssh_key.is_empty())
    {
        return Err(ApiError::new(409, "Agent 尚未就绪，暂时无法连接终端"));
    }
    i.tickets.retain(|_, t| t.expires >= now());
    if i.tickets.len() >= 64 {
        return Err(ApiError::rate("终端请求过多，请稍后重试", 30));
    }
    let ticket = token();
    i.tickets.insert(
        ticket.clone(),
        Ticket {
            session_id: session.id,
            node_id: v.id,
            expires: now() + 30,
        },
    );
    Ok(ApiReply::ok(json!({"ticket":ticket,"expiresIn":30})))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale_latency_reply_does_not_consume_current_challenge() {
        let at = Instant::now();
        let mut pending = Some(("current".into(), at));
        assert_eq!(
            latency_reply(&mut pending, "previous", at + Duration::from_secs(1)),
            None
        );
        assert!(pending.is_some());
        assert_eq!(
            latency_reply(&mut pending, "current", at + Duration::from_millis(123)),
            Some(123.0)
        );
        assert!(pending.is_none());
        assert_eq!(
            latency_reply(&mut pending, "current", at + Duration::from_secs(2)),
            None
        );
    }
    #[test]
    fn late_latency_measurement_is_ignored() {
        let at = Instant::now();
        let mut pending = Some(("late".into(), at));
        assert_eq!(
            latency_reply(&mut pending, "late", at + Duration::from_secs(11)),
            None
        );
        assert!(pending.is_none());
    }

    #[test]
    fn tunnel_framing_bounds() {
        assert!(Message::parse(br#"{"type":"ssh_data","data":"!"}"#).is_err());
        let m = Message {
            kind: "ssh_data".into(),
            data: STANDARD.encode(vec![0; MAX_CHUNK + 1]),
            ..Default::default()
        };
        assert!(Message::parse(&serde_json::to_vec(&m).unwrap()).is_err());
    }
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;
    #[tokio::test]
    async fn cancelled_handshake_releases_slot_and_closes_remote() {
        let (output, mut rx) = vistart_probe_agent::transport_queue::channel(4, 4);
        let link = Arc::new(AgentLink {
            stop: CancellationToken::new(),
            output,
            tunnels: Mutex::new(HashMap::new()),
        });
        let pending = link.clone();
        let task = tokio::spawn(async move { pending.open().await });
        let WS::Text(open) = rx.recv().await.unwrap() else {
            panic!("expected control message")
        };
        let open = Message::parse(open.as_bytes()).unwrap();
        assert_eq!(open.kind, "ssh_open");
        assert_eq!(link.tunnels.lock().unwrap().len(), 1);
        task.abort();
        assert!(task.await.err().unwrap().is_cancelled());
        assert!(link.tunnels.lock().unwrap().is_empty());
        let WS::Text(close) = rx.recv().await.unwrap() else {
            panic!("expected close message")
        };
        let close = Message::parse(close.as_bytes()).unwrap();
        assert_eq!(close.kind, "ssh_close");
        assert_eq!(open.session, close.session);
    }
}
