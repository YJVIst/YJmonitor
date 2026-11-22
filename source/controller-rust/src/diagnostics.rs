//! Bounded controller-to-node diagnostics, available only to an attached SSH lease.
use crate::core::*;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    net::{IpAddr, SocketAddr},
    process::Stdio,
    time::{Duration, Instant},
};
use tokio::{io::AsyncReadExt, net::TcpStream, process::Command, sync::Semaphore, time::timeout};

pub struct State {
    address: SocketAddr,
    touched: i64,
    checked: i64,
    latency: Option<f64>,
    pinging: bool,
    route_at: i64,
    tracing: bool,
    owner_loading: bool,
    hops: Vec<Value>,
    message: String,
}
impl State {
    fn new(address: SocketAddr) -> Self {
        Self {
            address,
            touched: now(),
            checked: 0,
            latency: None,
            pinging: false,
            route_at: 0,
            tracing: false,
            owner_loading: false,
            hops: vec![],
            message: String::new(),
        }
    }
    fn reply(&self) -> Value {
        json!({"checkedAt":self.checked,"reachable":self.latency.is_some(),"latencyMs":self.latency,
            "routeAt":self.route_at,"tracing":self.tracing,"ownerLoading":self.owner_loading,"hops":self.hops,"message":self.message})
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    id: String,
    session: String,
}
fn target(ip: &str, port: i64) -> Option<SocketAddr> {
    let ip: IpAddr = ip.parse().ok()?;
    if ip.is_unspecified() || ip.is_multicast() || matches!(ip,IpAddr::V4(v) if v.is_broadcast()) {
        return None;
    }
    let port = u16::try_from(port).ok().filter(|p| *p > 0)?;
    Some(SocketAddr::new(ip, port))
}
static TRACE_SLOTS: Semaphore = Semaphore::const_new(2);
pub fn api(app: App, c: Context, body: Vec<u8>) -> ApiResult<ApiReply> {
    let r: Request =
        serde_json::from_slice(&body).map_err(|_| ApiError::new(400, "诊断参数不正确"))?;
    let mut inner = app.lock();
    let auth = app.guard(&mut inner, &c, true, true)?;
    if !crate::file_sessions::valid(&r.session)
        || !inner.file_sessions.get(&r.session).is_some_and(|s| {
            s.node == r.id
                && s.owner == auth.handle
                && s.version == auth.version
                && s.attached
                && !s.closing
        })
    {
        return Err(ApiError::new(403, "请先连接该服务器的 SSH 终端"));
    }
    let node = inner
        .data
        .nodes
        .iter()
        .find(|n| n.public.id == r.id)
        .ok_or_else(|| ApiError::new(404, "服务器不存在"))?;
    let address = target(&node.ip, node.port)
        .or_else(|| {
            let ip = inner.ssh_dns.get(&node.ip)?.first()?;
            target(&ip.to_string(), node.port)
        })
        .ok_or_else(|| ApiError::new(400, "服务器地址尚未解析，请稍后重试"))?;
    inner
        .routes
        .retain(|_, s| now() - s.touched < 600 || s.pinging || s.tracing);
    if !inner.routes.contains_key(&r.id) && inner.routes.len() >= 64 {
        return Err(ApiError::rate("诊断任务较多，请稍后重试", 10));
    }
    let state = inner
        .routes
        .entry(r.id.clone())
        .or_insert_with(|| State::new(address));
    if state.address != address {
        *state = State::new(address);
    }
    state.touched = now();
    if !state.pinging && now() - state.checked >= 5 {
        state.pinging = true;
        let app = app.clone();
        let id = r.id.clone();
        tokio::spawn(async move {
            let start = Instant::now();
            let latency = tokio::select! {
                _=app.0.stop.cancelled()=>None,
                result=timeout(Duration::from_secs(2),TcpStream::connect(address))=>
                    result.ok().and_then(Result::ok).map(|_|start.elapsed().as_secs_f64()*1000.)
            };
            if let Some(state) = app
                .lock()
                .routes
                .get_mut(&id)
                .filter(|s| s.address == address)
            {
                state.checked = now();
                state.latency = latency;
                state.pinging = false;
            }
        });
    }
    if !state.tracing
        && now() - state.route_at >= 60
        && let Ok(slot) = TRACE_SLOTS.try_acquire()
    {
        state.tracing = true;
        state.message = String::new();
        let app = app.clone();
        let id = r.id;
        tokio::spawn(async move {
            let _slot = slot;
            let (hops, message) = tokio::select! {
                _=app.0.stop.cancelled()=>(vec![],"诊断已停止".into()),
                result=trace(address.ip())=>result
            };
            let route_at = now();
            {
                if let Some(state) = app
                    .lock()
                    .routes
                    .get_mut(&id)
                    .filter(|s| s.address == address)
                {
                    state.route_at = route_at;
                    state.hops = hops.clone();
                    state.message = message;
                    state.tracing = false;
                    state.owner_loading = true;
                }
            }
            // Publish hop IPs immediately; registry lookups must not delay route display.
            let _ = timeout(
                Duration::from_secs(12),
                enrich(&app, &id, address, route_at, hops),
            )
            .await;
            if let Some(state) = app
                .lock()
                .routes
                .get_mut(&id)
                .filter(|s| s.address == address && s.route_at == route_at)
            {
                state.owner_loading = false;
            }
        });
    }
    Ok(ApiReply::ok(state.reply()))
}
async fn trace(ip: IpAddr) -> (Vec<Value>, String) {
    let executable = [
        "/usr/bin/tracepath",
        "/bin/tracepath",
        "/usr/sbin/tracepath",
    ]
    .into_iter()
    .find(|p| std::path::Path::new(p).is_file());
    let Some(executable) = executable else {
        return (
            vec![],
            "主控未安装 tracepath；TCP 连接检测仍可使用。".into(),
        );
    };
    let child = Command::new(executable)
        .args(["-n", "-m", "16", "-l", "128", &ip.to_string()])
        .env_clear()
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn();
    let Ok(mut child) = child else {
        return (vec![], "路由工具无法启动，请检查主控权限。".into());
    };
    let Some(stdout) = child.stdout.take() else {
        return (vec![], "无法读取路由结果。".into());
    };
    let mut buffer = Vec::new();
    let mut reader = stdout.take(32768);
    let read = timeout(Duration::from_secs(15), reader.read_to_end(&mut buffer)).await;
    // Reap the subprocess on timeout and on output cap; never leave a tracer running.
    let timed_out = read.is_err() || buffer.len() >= 32768;
    if timed_out {
        let _ = child.kill().await;
    }
    let _ = timeout(Duration::from_secs(1), child.wait()).await;
    let hops = parse_trace(&String::from_utf8_lossy(&buffer));
    let message = if hops.is_empty() {
        "尚未收到路由响应；路由可能被防火墙过滤。"
    } else if timed_out {
        "路由探测已限时结束；* 表示该跳未回应，不代表终端中断。"
    } else {
        ""
    };
    (hops, message.into())
}
fn parse_trace(raw: &str) -> Vec<Value> {
    let mut hops = std::collections::BTreeMap::new();
    for line in raw.lines().take(512) {
        let Some((ttl, tail)) = line.trim().split_once(':') else {
            continue;
        };
        let Ok(ttl) = ttl.trim().parse::<u8>() else {
            continue;
        };
        if !(1..=16).contains(&ttl) {
            continue;
        }
        let fields: Vec<_> = tail.split_whitespace().take(12).collect();
        let address = fields.first().and_then(|s| s.parse::<IpAddr>().ok());
        if let Some(address) = address {
            let ms = fields
                .iter()
                .find_map(|s| s.strip_suffix("ms").and_then(|s| s.parse::<f64>().ok()))
                .filter(|n| n.is_finite() && *n >= 0.);
            hops.insert(ttl,json!({"ttl":ttl,"address":address.to_string(),"ms":ms,"reached":fields.contains(&"reached")}));
        } else if tail.contains("no reply") {
            hops.entry(ttl)
                .or_insert_with(|| json!({"ttl":ttl,"address":null,"ms":null,"reached":false}));
        }
    }
    hops.into_values().collect()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_registered_numeric_targets() {
        for ip in [
            "--help",
            "example.com",
            "127.0.0.1;id",
            "0.0.0.0",
            "::",
            "224.0.0.1",
            "ff02::1",
            "255.255.255.255",
        ] {
            assert!(target(ip, 22).is_none(), "{ip}");
        }
        assert!(target("192.0.2.1", 22).is_some());
        assert!(target("::1", 22).is_some());
        assert!(target("127.0.0.1", 0).is_none());
        assert!(target("127.0.0.1", 65536).is_none());
        assert!(
            serde_json::from_str::<Request>(r#"{"id":"x","session":"y","target":"1.1.1.1"}"#)
                .is_err()
        );
    }
    #[test]
    fn parse_numeric_hops_bounded_and_timeout_truthful() {
        let hops = parse_trace(
            " 1?: [LOCALHOST] pmtu 1500\n 1: 10.0.0.1 1.234ms\n 1: no reply\n 2: no reply\n 3: 2001:db8::1 20.123ms reached\n17: 8.8.8.8 1ms\n4: <script> 0ms\n",
        );
        assert_eq!(hops.len(), 3);
        assert_eq!(hops[0]["address"], "10.0.0.1");
        assert!(hops[1]["address"].is_null());
        assert_eq!(hops[2]["reached"], true);
    }
}

// Only public hop addresses go to the fixed HTTPS registry endpoint. Private
// addresses never leave the controller. Cache and budget are shared by sessions.
#[derive(Default)]
pub struct Owners {
    cache: std::collections::HashMap<IpAddr, (i64, Value)>,
    requests: std::collections::VecDeque<i64>,
}
static OWNER_SLOTS: Semaphore = Semaphore::const_new(2);
fn public_hop(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            let b = v.octets();
            !(v.is_private()
                || v.is_loopback()
                || v.is_link_local()
                || v.is_documentation()
                || v.is_broadcast()
                || v.is_unspecified()
                || v.is_multicast()
                || b[0] == 0
                || b[0] >= 224
                || (b[0] == 100 && (64..=127).contains(&b[1]))
                || (b[0] == 198 && (18..=19).contains(&b[1]))
                || (b[0] == 192 && b[1] == 0 && b[2] == 0))
        }
        IpAddr::V6(v) => {
            let s = v.segments();
            s[0] & 0xe000 == 0x2000
                && !(s[0] == 0x2001 && (s[1] < 0x0200 || s[1] == 0x0db8))
                && !(s[0] == 0x3fff && s[1] < 0x1000)
        }
    }
}
fn registry_owner(value: &Value) -> Option<Value> {
    if value["status"] != "ok" {
        return None;
    }
    let mut names = Vec::new();
    let mut asns = Vec::new();
    for entry in value["data"]["asns"].as_array()?.iter().take(3) {
        let asn = entry["asn"]
            .as_u64()
            .filter(|v| *v > 0 && *v <= u32::MAX as u64)?;
        let name = entry["holder"].as_str()?.trim();
        if name.is_empty() || name.len() > 512 || name.chars().any(char::is_control) {
            continue;
        }
        let name: String = name.chars().take(160).collect();
        if !asns.contains(&asn) {
            asns.push(asn);
        }
        if !names.contains(&name) {
            names.push(name);
        }
    }
    (!names.is_empty()).then(|| json!({"operator":names.join(" / "),"asns":asns}))
}
async fn fetch_owner(app: &App, ip: IpAddr) -> Option<Value> {
    let mut url =
        reqwest::Url::parse("https://stat.ripe.net/data/prefix-overview/data.json").ok()?;
    url.query_pairs_mut()
        .append_pair("resource", &ip.to_string())
        .append_pair("max_related", "0")
        .append_pair("sourceapp", "yuji-probe-rust");
    let mut response = app
        .0
        .http
        .get(url)
        .timeout(Duration::from_secs(4))
        .send()
        .await
        .ok()?;
    if !response.status().is_success() || response.content_length().is_some_and(|n| n > 65536) {
        return None;
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if bytes.len() + chunk.len() > 65536 {
            return None;
        }
        bytes.extend_from_slice(&chunk);
    }
    registry_owner(&serde_json::from_slice::<Value>(&bytes).ok()?)
}
async fn owner(app: &App, ip: IpAddr) -> Value {
    if !public_hop(ip) {
        return json!({"operator":"内网 / 保留地址","asns":[]});
    }
    let unknown = || json!({"operator":"未知","asns":[]});
    let Ok(_slot) = OWNER_SLOTS.acquire().await else {
        return unknown();
    };
    {
        let mut inner = app.lock();
        let cache = &mut inner.route_owners;
        cache.cache.retain(|_, (until, _)| *until > now());
        if let Some((_, value)) = cache.cache.get(&ip) {
            return value.clone();
        }
        while cache.requests.front().is_some_and(|at| now() - at >= 86400) {
            cache.requests.pop_front();
        }
        if cache.requests.len() >= 900
            || cache
                .requests
                .iter()
                .rev()
                .take_while(|at| now() - **at < 60)
                .count()
                >= 60
        {
            return unknown();
        }
        cache.requests.push_back(now());
    }
    let result = tokio::select! {
        _=app.0.stop.cancelled()=>None,
        result=fetch_owner(app,ip)=>result
    };
    let lifetime = if result.is_some() { 86400 } else { 900 };
    let value = result.unwrap_or_else(unknown);
    let mut inner = app.lock();
    let cache = &mut inner.route_owners.cache;
    if cache.len() >= 512
        && let Some(old) = cache.iter().min_by_key(|(_, v)| v.0).map(|(ip, _)| *ip)
    {
        cache.remove(&old);
    }
    cache.insert(ip, (now() + lifetime, value.clone()));
    value
}
async fn enrich(app: &App, id: &str, address: SocketAddr, route_at: i64, hops: Vec<Value>) {
    use futures_util::{StreamExt, stream};
    let mut ips = std::collections::HashSet::new();
    for hop in hops {
        if let Some(ip) = hop["address"]
            .as_str()
            .and_then(|s| s.parse::<IpAddr>().ok())
        {
            ips.insert(ip);
        }
    }
    let lookups = stream::iter(ips)
        .map(|ip| async move { (ip, owner(app, ip).await) })
        .buffer_unordered(2);
    tokio::pin!(lookups);
    while let Some((ip, info)) = lookups.next().await {
        let target = ip.to_string();
        if let Some(state) = app
            .lock()
            .routes
            .get_mut(id)
            .filter(|s| s.address == address && s.route_at == route_at)
        {
            for hop in &mut state.hops {
                if hop["address"].as_str() == Some(target.as_str()) {
                    hop["operator"] = info["operator"].clone();
                    hop["asns"] = info["asns"].clone();
                }
            }
        }
    }
}
#[cfg(test)]
mod owner_tests {
    use super::*;
    #[test]
    fn registry_never_receives_private_or_special_addresses() {
        for ip in [
            "10.0.0.1",
            "127.0.0.1",
            "172.16.1.2",
            "192.168.1.2",
            "169.254.1.2",
            "100.64.1.2",
            "198.18.0.1",
            "192.0.2.1",
            "0.0.0.1",
            "255.255.255.255",
            "240.0.0.1",
            "::",
            "::1",
            "::ffff:8.8.8.8",
            "fc00::1",
            "fe80::1",
            "2001:db8::1",
            "ff02::1",
        ] {
            assert!(!public_hop(ip.parse().unwrap()), "{ip}");
        }
        for ip in [
            "1.1.1.1",
            "8.8.8.8",
            "2606:4700:4700::1111",
            "2001:4860:4860::8888",
        ] {
            assert!(public_hop(ip.parse().unwrap()), "{ip}");
        }
    }
    #[test]
    fn registry_names_are_bounded_and_require_valid_as_numbers() {
        let value =
            json!({"status":"ok","data":{"asns":[{"asn":64500,"holder":"Example Network"}]}});
        assert_eq!(
            registry_owner(&value).unwrap()["operator"],
            "Example Network"
        );
        assert!(registry_owner(&json!({"status":"error","data":{"asns":[]}})).is_none());
        assert!(
            registry_owner(&json!({"status":"ok","data":{"asns":[{"asn":0,"holder":"Bad"}]}}))
                .is_none()
        );
        assert!(
            registry_owner(
                &json!({"status":"ok","data":{"asns":[{"asn":64500,"holder":"Bad\nName"}]}})
            )
            .is_none()
        );
    }
}
