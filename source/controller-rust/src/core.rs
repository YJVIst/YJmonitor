use crate::model::*;
use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use axum::{
    Json,
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    net::{IpAddr, SocketAddr},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, OnceLock},
};
use subtle::ConstantTimeEq;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

pub const COOKIE: &str = "__Host-vistart_probe";
pub const MAX_NODES: usize = 1000;
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub type ApiResult<T> = Result<T, ApiError>;
#[derive(Debug)]
pub struct ApiError {
    pub status: u16,
    pub message: String,
    pub retry: Option<u16>,
}
impl ApiError {
    pub fn new(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            retry: None,
        }
    }
    pub fn internal() -> Self {
        Self::new(500, "状态保存失败，请稍后重试")
    }
    pub fn rate(message: &str, seconds: u16) -> Self {
        Self {
            status: 429,
            message: message.into(),
            retry: Some(seconds),
        }
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut r = (
            StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(json!({"error":self.message})),
        )
            .into_response();
        if let Some(n) = self.retry {
            r.headers_mut().insert(
                "Retry-After",
                HeaderValue::from_str(&n.to_string()).unwrap(),
            );
        }
        secure_headers(&mut r);
        r
    }
}
pub struct ApiReply {
    pub value: Value,
    pub status: u16,
    pub cookie: Option<String>,
    pub raw: Option<std::sync::Arc<[u8]>>,
}
impl ApiReply {
    pub fn ok(value: Value) -> Self {
        Self {
            value,
            status: 200,
            cookie: None,
            raw: None,
        }
    }
    pub fn accepted(value: Value) -> Self {
        Self {
            value,
            status: 202,
            cookie: None,
            raw: None,
        }
    }
    pub fn session(value: Value, id: &str) -> Self {
        Self {
            value,
            status: 200,
            raw: None,
            cookie: Some(format!(
                "{COOKIE}={id}; Path=/; Max-Age=28800; Secure; HttpOnly; SameSite=Strict"
            )),
        }
    }
}
impl IntoResponse for ApiReply {
    fn into_response(self) -> Response {
        let mut r = if let Some(raw) = self.raw {
            (
                StatusCode::from_u16(self.status).unwrap(),
                [("Content-Type", "application/json")],
                axum::body::Bytes::from_owner(raw),
            )
                .into_response()
        } else {
            (StatusCode::from_u16(self.status).unwrap(), Json(self.value)).into_response()
        };
        if let Some(c) = self.cookie {
            r.headers_mut()
                .insert("Set-Cookie", HeaderValue::from_str(&c).unwrap());
        }
        secure_headers(&mut r);
        r
    }
}
pub fn secure_headers(r: &mut Response) {
    r.headers_mut()
        .insert("Cache-Control", HeaderValue::from_static("no-store"));
    r.headers_mut().insert(
        "X-Content-Type-Options",
        HeaderValue::from_static("nosniff"),
    );
}
pub fn token() -> String {
    let mut b = [0; 32];
    getrandom::fill(&mut b).expect("secure random source unavailable");
    hex::encode(b)
}
pub fn constant(a: &str, b: &str) -> bool {
    a.as_bytes().ct_eq(b.as_bytes()).into()
}
pub fn now() -> i64 {
    Utc::now().timestamp()
}
pub fn valid_text(s: &str, max: usize) -> bool {
    !s.trim().is_empty() && s.chars().count() <= max && !s.chars().any(|c| c < ' ' || c == '\x7f')
}
pub fn valid_password(s: &str) -> bool {
    (12..=72).contains(&s.len()) && !s.contains('\0')
}
pub fn username(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 32
        && (b[0].is_ascii_alphabetic() || b[0] == b'_')
        && b.iter()
            .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(c))
}
pub fn acceptable_ip(s: &str) -> bool {
    match s.parse::<IpAddr>() {
        Ok(IpAddr::V4(v)) => {
            !v.is_loopback()
                && !v.is_unspecified()
                && !v.is_multicast()
                && !v.is_link_local()
                && !v.is_broadcast()
        }
        Ok(IpAddr::V6(v)) => match v.to_ipv4_mapped() {
            Some(v) => acceptable_ip(&v.to_string()),
            None => {
                !v.is_loopback()
                    && !v.is_unspecified()
                    && !v.is_multicast()
                    && !v.is_unicast_link_local()
            }
        },
        _ => false,
    }
}
fn json_size_limit(path: &Path) -> usize {
    const MIB: usize = 1024 * 1024;
    match path.file_name().and_then(|s| s.to_str()) {
        Some("nodes.json") => 64 * MIB,
        Some("restore-journal.json") => 128 * MIB,
        Some(name) if name.ends_with(".backup") => 24 * MIB,
        _ => 16 * MIB,
    }
}
pub fn encode_state_json<T: Serialize>(path: &Path, value: &T) -> std::io::Result<Vec<u8>> {
    let raw = serde_json::to_vec(value)?;
    if raw.len() > json_size_limit(path) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::FileTooLarge,
            "state too large",
        ));
    }
    Ok(raw)
}
pub fn atomic_json<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    atomic_bytes(path, &encode_state_json(path, value)?)
}
pub fn atomic_bytes(path: &Path, raw: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_file_name(format!(
        "{}.{}",
        path.file_name().unwrap().to_string_lossy(),
        &token()[..12]
    ));
    let result = (|| {
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(raw)?;
        f.sync_all()?;
        drop(f);
        fs::rename(&tmp, path)?;
        File::open(path.parent().unwrap())?.sync_all()
    })();
    let _ = fs::remove_file(tmp);
    result
}
pub fn read_json<T: DeserializeOwned>(path: &Path) -> std::io::Result<T> {
    let mut b = Vec::new();
    let limit = json_size_limit(path);
    File::open(path)?
        .take((limit + 1) as u64)
        .read_to_end(&mut b)?;
    if b.len() > limit {
        return Err(std::io::Error::other("state too large"));
    }
    serde_json::from_slice(&b).map_err(std::io::Error::other)
}
pub fn decode<T: DeserializeOwned>(body: &[u8]) -> ApiResult<T> {
    serde_json::from_slice(body).map_err(|_| ApiError::new(400, "请求内容不正确"))
}
pub fn countries() -> &'static HashMap<String, Value> {
    static C: OnceLock<HashMap<String, Value>> = OnceLock::new();
    C.get_or_init(|| {
        serde_json::from_str(include_str!("../assets/countries.json"))
            .expect("country dataset invalid")
    })
}
pub fn country_code(s: &str) -> String {
    let mut v = s.trim().to_uppercase();
    if v == "UK" {
        v = "GB".into()
    }
    if countries().contains_key(&v) {
        v
    } else {
        "OTHER".into()
    }
}
pub fn country_name(s: &str) -> String {
    countries()
        .get(&country_code(s))
        .and_then(|v| v["name"].as_str())
        .unwrap_or("未知地区")
        .into()
}

#[derive(Clone)]
pub struct Context {
    pub method: String,
    pub path: String,
    pub headers: HeaderMap,
    pub sid: String,
    pub ip: String,
    pub remote: SocketAddr,
}
impl Context {
    pub fn new(
        method: &str,
        path: &str,
        headers: HeaderMap,
        remote: SocketAddr,
        origin: &str,
    ) -> Self {
        let cookie = if origin.starts_with("http://") {
            "vistart_probe_http"
        } else {
            COOKIE
        };
        let sid = headers
            .get("Cookie")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| {
                s.split(';')
                    .map(str::trim)
                    .find_map(|s| s.strip_prefix(&format!("{cookie}=")))
            })
            .filter(|v| v.len() == 64)
            .unwrap_or("")
            .to_string();
        let ip = if remote.ip().is_loopback() {
            headers
                .get("X-Real-IP")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<IpAddr>().ok())
                .unwrap_or(remote.ip())
        } else {
            remote.ip()
        };
        Self {
            method: method.into(),
            path: path.into(),
            headers,
            sid,
            ip: ip.to_string(),
            remote,
        }
    }
    pub fn header(&self, k: &str) -> &str {
        self.headers
            .get(k)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
    }
}
#[derive(Clone)]
pub struct Session {
    pub id: String,
    pub csrf: String,
    pub auth: bool,
    pub version: String,
    pub created: i64,
    pub seen: i64,
    pub source: String,
    pub device: String,
    pub handle: String,
    pub elevated: i64,
    pub mfa_pending: String,
    pub mfa_expires: i64,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Attempt {
    #[serde(rename = "Count")]
    pub count: u32,
    #[serde(rename = "Until")]
    pub until: DateTime<Utc>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LoginLimits {
    #[serde(deserialize_with = "null_default")]
    pub sources: HashMap<String, Attempt>,
    pub global: Attempt,
}
#[derive(Clone)]
pub struct Ticket {
    pub session_id: String,
    pub node_id: String,
    pub expires: i64,
}
#[derive(Clone)]
pub struct App(pub Arc<Shared>);
pub struct Shared {
    pub dir: PathBuf,
    pub origin: String,
    pub php_gateway: bool,
    pub master: Zeroizing<Vec<u8>>,
    pub inner: Mutex<Inner>,
    pub login_slots: Arc<Semaphore>,
    pub deploy_slots: Arc<Semaphore>,
    pub pending_terminals: Arc<Semaphore>,
    pub terminal_slots: Arc<Semaphore>,
    pub stop: CancellationToken,
    pub http: reqwest::Client,
}
pub struct Inner {
    pub theme_cache: Option<(String, std::sync::Arc<[u8]>)>,
    pub auth: Auth,
    pub data: Data,
    pub admin_cache: Option<(i64, Arc<[u8]>)>,
    pub cache: Option<(i64, std::sync::Arc<[u8]>)>,
    pub ops: crate::operations::State,
    pub updates: crate::updates::State,
    pub sessions: HashMap<String, Session>,
    pub limits: LoginLimits,
    pub requests: HashMap<String, Attempt>,
    pub audit: Vec<Audit>,
    pub agents: HashMap<String, Arc<crate::realtime::AgentLink>>,
    pub agent_reserved: HashSet<String>,
    pub tickets: HashMap<String, Ticket>,
    pub jobs: HashMap<String, DeployJob>,
    pub enrollments: HashMap<String, crate::enrollment::Enrollment>,
    pub retained: HashMap<String, crate::retained::Retained>,
    pub file_sessions: HashMap<String, crate::file_sessions::TransferSession>,
    pub terminals: HashMap<String, HashMap<String, CancellationToken>>,
    pub ssh_dns: HashMap<String, Vec<IpAddr>>,
    pub routes: HashMap<String, crate::diagnostics::State>,
    pub route_owners: crate::diagnostics::Owners,
    pub file_edits: HashSet<String>,
    pub telegram: TelegramState,
    pub telegram_ready: i64,
    pub telegram_revision: u64,
    pub telegram_error: String,
    pub telegram_next: i64,
    pub delivery: Option<(TelegramEvent, CancellationToken)>,
}
impl App {
    pub fn lock(&self) -> MutexGuard<'_, Inner> {
        self.0.inner.lock().expect("state lock poisoned")
    }
    pub fn new(dir: PathBuf, origin: String, php_gateway: bool) -> Result<Self, &'static str> {
        crate::backup::complete_restore(&dir)?;
        match fs::remove_file(dir.join("ssh-pins.json")) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err("cannot retire obsolete SSH trust file"),
        }
        let auth: Auth =
            read_json(&dir.join("auth.json")).map_err(|_| "authentication state unavailable")?;
        let mut data: Data =
            read_json(&dir.join("nodes.json")).map_err(|_| "node state unavailable")?;
        let theme_migrated = data.theme.migrate_removed();
        data.theme.validate().map_err(|_| "theme state invalid")?;
        if theme_migrated {
            atomic_json(&dir.join("nodes.json"), &data).map_err(|_| "theme migration failed")?;
        }
        if data.nodes.len() > MAX_NODES || data.commands.len() > 50 {
            return Err("state limits exceeded");
        }
        if data.schema < 2 {
            data.schema = 2;
            data.preview = true;
            atomic_json(&dir.join("nodes.json"), &data).map_err(|_| "state migration failed")?;
        }
        for n in &mut data.nodes {
            if !n.demo {
                n.public.online = false;
                n.public.latency_ms = None;
            }
        }
        let path = dir.join("app.key");
        let master = if path.exists() {
            fs::read(&path).map_err(|_| "application key unavailable")?
        } else {
            if !data.secrets.is_empty() || !auth.mfa.is_empty() {
                return Err("application key missing for existing credentials");
            }
            let mut b = vec![0; 32];
            getrandom::fill(&mut b).map_err(|_| "secure random unavailable")?;
            let mut f = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
                .map_err(|_| "application key unavailable")?;
            f.write_all(&b)
                .and_then(|_| f.sync_all())
                .map_err(|_| "application key unavailable")?;
            b
        };
        if master.len() != 32 {
            return Err("application key invalid");
        }
        let limits = match read_json(&dir.join("login-limits.json")) {
            Ok(v) => v,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => LoginLimits::default(),
            Err(_) => return Err("security state unavailable"),
        };
        let telegram = match read_json(&dir.join("telegram.json")) {
            Ok(v) => v,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => TelegramState {
                config: TelegramConfig {
                    online: true,
                    offline: true,
                    renewal: true,
                    ..Default::default()
                },
                ..Default::default()
            },
            Err(_) => return Err("notification state unavailable"),
        };
        let tls = vistart_probe_agent::connection::tls_config()?;
        let http = reqwest::Client::builder()
            .tls_backend_preconfigured((*tls).clone())
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(std::time::Duration::from_secs(5))
            .timeout(std::time::Duration::from_secs(50))
            .user_agent(format!("Vistart-Probe/{VERSION}"))
            .build()
            .map_err(|_| "HTTPS client unavailable")?;
        let mut ops = crate::operations::load(&dir)?;
        crate::operations::prune_removed_nodes(&mut ops, &data.nodes);
        let inner = Inner {
            theme_cache: None,
            cache: None,
            admin_cache: None,
            ops,
            updates: crate::updates::State::default(),
            auth,
            data,
            sessions: HashMap::new(),
            limits,
            requests: HashMap::new(),
            audit: read_json(&dir.join("audit.json")).unwrap_or_default(),
            agents: HashMap::new(),
            agent_reserved: HashSet::new(),
            tickets: HashMap::new(),
            jobs: HashMap::new(),
            enrollments: HashMap::new(),
            retained: crate::retained::load(&dir)?,
            file_sessions: crate::file_sessions::load(&dir)?,
            terminals: HashMap::new(),
            ssh_dns: HashMap::new(),
            routes: HashMap::new(),
            route_owners: crate::diagnostics::Owners::default(),
            file_edits: HashSet::new(),
            telegram,
            telegram_ready: now() + 30,
            telegram_revision: 0,
            telegram_error: String::new(),
            telegram_next: 0,
            delivery: None,
        };
        let app = Self(Arc::new(Shared {
            dir,
            origin,
            php_gateway,
            master: Zeroizing::new(master),
            inner: Mutex::new(inner),
            login_slots: Arc::new(Semaphore::new(2)),
            deploy_slots: Arc::new(Semaphore::new(2)),
            pending_terminals: Arc::new(Semaphore::new(8)),
            terminal_slots: Arc::new(Semaphore::new(8)),
            stop: CancellationToken::new(),
            http,
        }));
        {
            let i = app.lock();
            if !i.telegram.config.token.is_empty() {
                let value = app.unseal("telegram-bot-token-v1", &i.telegram.config.token)?;
                if !crate::telegram::valid_token(
                    std::str::from_utf8(&value).map_err(|_| "notification token invalid")?,
                ) {
                    return Err("notification token invalid");
                }
            }
        }
        Ok(app)
    }
    pub fn seal(&self, label: &str, raw: &[u8]) -> Result<String, &'static str> {
        let cipher = Aes256Gcm::new_from_slice(&self.0.master).map_err(|_| "cipher unavailable")?;
        let mut nonce = [0; 12];
        getrandom::fill(&mut nonce).map_err(|_| "random unavailable")?;
        let encrypted = cipher
            .encrypt(
                &Nonce::from(nonce),
                Payload {
                    msg: raw,
                    aad: label.as_bytes(),
                },
            )
            .map_err(|_| "encryption failed")?;
        let mut out = nonce.to_vec();
        out.extend(encrypted);
        Ok(STANDARD.encode(out))
    }
    pub fn unseal(&self, label: &str, value: &str) -> Result<Zeroizing<Vec<u8>>, &'static str> {
        let raw = STANDARD
            .decode(value)
            .map_err(|_| "invalid encrypted secret")?;
        if raw.len() < 28 {
            return Err("invalid encrypted secret");
        };
        let cipher = Aes256Gcm::new_from_slice(&self.0.master).map_err(|_| "cipher unavailable")?;
        cipher
            .decrypt(
                &Nonce::from(<[u8; 12]>::try_from(&raw[..12]).map_err(|_| "invalid nonce")?),
                Payload {
                    msg: &raw[12..],
                    aad: label.as_bytes(),
                },
            )
            .map(Zeroizing::new)
            .map_err(|_| "secret authentication failed")
    }
    pub fn gateway(&self, c: &Context) -> bool {
        let mut h = Sha256::new();
        h.update(b"vistart-probe-php-gateway-v1:");
        h.update(self.0.master.as_slice());
        c.remote.ip().is_loopback()
            && constant(c.header("X-Probe-Gateway"), &hex::encode(h.finalize()))
    }
    pub fn record(&self, i: &mut Inner, action: &str, subject: &str) {
        i.admin_cache = None;
        i.audit.push(Audit {
            source: String::new(),
            result: if action.ends_with("failed") {
                "failed"
            } else {
                "ok"
            }
            .into(),
            at: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            action: action.into(),
            subject: subject.into(),
        });
        if i.audit.len() > 2000 {
            i.audit.drain(..i.audit.len() - 2000);
        }
        if atomic_json(&self.0.dir.join("audit.json"), &i.audit).is_err() {
            eprintln!("Audit persistence unavailable");
        }
    }
    pub fn save_data(&self, i: &mut Inner, data: Data) -> ApiResult<()> {
        atomic_json(&self.0.dir.join("nodes.json"), &data).map_err(|e| {
            if e.kind() == std::io::ErrorKind::FileTooLarge {
                ApiError::new(413, "节点状态超过 64 MiB，未保存更改")
            } else {
                ApiError::internal()
            }
        })?;
        i.data = data;
        i.cache = None;
        i.admin_cache = None;
        Ok(())
    }
    pub fn persist_limits(&self, i: &mut Inner) -> ApiResult<()> {
        i.limits.sources.retain(|_, v| v.until.timestamp() >= now());
        atomic_json(&self.0.dir.join("login-limits.json"), &i.limits)
            .map_err(|_| ApiError::new(503, "安全记录不可用"))
    }
    pub fn guard(
        &self,
        i: &mut Inner,
        c: &Context,
        admin: bool,
        write: bool,
    ) -> ApiResult<Session> {
        let x = i
            .session(&c.sid)
            .filter(|x| !admin || x.auth)
            .ok_or_else(|| ApiError::new(401, "请先登录管理员账户"))?;
        if write
            && (c.header("Origin") != self.0.origin || !constant(c.header("X-CSRF-Token"), &x.csrf))
        {
            return Err(ApiError::new(403, "请求校验失败，请刷新页面"));
        }
        Ok(x)
    }
}
impl Inner {
    pub fn revoke(&mut self, id: &str) {
        if let Some(s) = self.sessions.get(id) {
            self.enrollments.retain(|_, e| e.owner != s.handle);
            for r in self.retained.values_mut() {
                if r.owner == s.handle {
                    r.closing = true;
                    if let Some(stop) = &r.stop {
                        stop.cancel();
                    }
                }
            }
            for r in self.file_sessions.values_mut() {
                if r.owner == s.handle {
                    r.closing = true;
                }
            }
        }
        if let Some(all) = self.terminals.remove(id) {
            for t in all.values() {
                t.cancel();
            }
        }
        self.tickets.retain(|_, t| t.session_id != id);
        self.sessions.remove(id);
    }
    pub fn session(&mut self, id: &str) -> Option<Session> {
        let x = self.sessions.get(id)?;
        let t = now();
        if t - x.seen > if x.auth { 1800 } else { 1200 }
            || t - x.created > 28800
            || (x.auth && x.version != self.auth.version)
        {
            self.revoke(id);
            return None;
        }
        let x = self.sessions.get_mut(id)?;
        x.seen = t;
        Some(x.clone())
    }
    pub fn new_session(&mut self, old: &str, authenticated: bool) -> ApiResult<Session> {
        let previous = self.sessions.get(old).cloned();
        self.revoke(old);
        let t = now();
        let stale: Vec<_> = self
            .sessions
            .iter()
            .filter(|(_, s)| t - s.seen > 1800 || t - s.created > 28800)
            .map(|(k, _)| k.clone())
            .collect();
        for k in stale {
            self.revoke(&k);
        }
        if self.sessions.len() >= 2048 {
            let victim = self
                .sessions
                .values()
                .filter(|s| !s.auth)
                .min_by_key(|s| s.seen)
                .map(|s| s.id.clone());
            if let Some(id) = victim {
                self.revoke(&id)
            } else {
                return Err(ApiError::new(503, "会话已满，请稍后重试"));
            }
        }
        let x = Session {
            id: token(),
            csrf: token(),
            auth: authenticated,
            version: self.auth.version.clone(),
            created: t,
            seen: t,
            source: previous
                .as_ref()
                .map(|s| s.source.clone())
                .unwrap_or_default(),
            device: previous
                .as_ref()
                .map(|s| s.device.clone())
                .unwrap_or_default(),
            handle: token(),
            elevated: 0,
            mfa_pending: String::new(),
            mfa_expires: 0,
        };
        self.sessions.insert(x.id.clone(), x.clone());
        Ok(x)
    }
    pub fn info(&self, x: &Session) -> Value {
        let mut v =
            json!({"authenticated":x.auth,"csrf":x.csrf,"mfaRequired":!self.auth.mfa.is_empty()});
        if x.auth {
            v["username"] = json!(self.auth.username);
            v["role"] = json!("admin");
            v["mfaEnabled"] = json!(!self.auth.mfa.is_empty());
            v["recoveryRemaining"] = json!(self.auth.recovery.len());
        }
        v
    }
    pub fn request(&mut self, key: &str, limit: u32) -> ApiResult<()> {
        let t = now();
        if self.requests.len() > 4096 {
            self.requests.retain(|_, v| v.until.timestamp() > t);
        }
        if self.requests.len() > 8192 {
            return Err(ApiError::new(503, "服务繁忙"));
        }
        let a = self.requests.entry(key.into()).or_default();
        if a.until.timestamp() <= t {
            *a = Attempt {
                count: 0,
                until: DateTime::from_timestamp(t + 60, 0).unwrap(),
            };
        }
        if a.count >= limit {
            return Err(ApiError::rate("请求过于频繁，请稍后重试", 60));
        }
        a.count += 1;
        Ok(())
    }
    pub fn terminal_valid(
        &self,
        sid: &str,
        version: &str,
        node: &str,
        link: &Arc<crate::realtime::AgentLink>,
    ) -> bool {
        self.sessions.get(sid).is_some_and(|s| {
            s.auth
                && s.version == version
                && self.auth.version == version
                && now() - s.created < 28800
                && now() - s.seen < 1800
        }) && self.agents.get(node).is_some_and(|a| Arc::ptr_eq(a, link))
    }
    pub fn expire_nodes(&mut self) {
        let t = now();
        for n in &mut self.data.nodes {
            if n.demo {
                continue;
            }
            n.public.last_seen_minutes = if n.last_seen > 0 {
                ((t - n.last_seen) / 60).max(0)
            } else {
                0
            };
            if n.last_seen == 0 || t - n.last_seen > 15 {
                n.public.online = false;
                n.public.latency_ms = None;
            }
            if n.latency_at == 0 || t - n.latency_at > 15 {
                n.public.latency_ms = None;
            }
        }
    }
}
