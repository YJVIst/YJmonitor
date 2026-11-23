//! First installation uses a private ownership link, CSRF and a single writer.
use crate::{core::*, model::*, web};
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::State,
    http::{Request, header},
    response::{IntoResponse, Response},
    routing::any,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
const COOKIE: &str = "__Host-yuji_setup";
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    name: String,
    username: String,
    password: String,
}
#[derive(Serialize, Deserialize)]
struct Pending {
    auth: Auth,
    data: Data,
    key: Vec<u8>,
}
fn validate(v: &Input) -> Result<(), &'static str> {
    if v.name.trim().is_empty() || v.name.len() > 180 || v.name.chars().any(char::is_control) {
        return Err("站点名称不正确");
    }
    let mut chars = v.username.bytes();
    if v.username.len() > 32
        || !chars
            .next()
            .is_some_and(|v| v.is_ascii_alphabetic() || v == b'_')
        || !chars.all(|v| v.is_ascii_alphanumeric() || b"_.-".contains(&v))
    {
        return Err("管理员用户名格式不正确");
    }
    if !(12..=72).contains(&v.password.len()) || v.password.contains('\0') {
        return Err("密码长度需为 12 至 72 字节");
    }
    Ok(())
}
fn commit(dir: &Path, pending: Pending) -> Result<(), &'static str> {
    atomic_bytes(&dir.join("app.key"), &pending.key).map_err(|_| "安装密钥保存失败")?;
    atomic_json(&dir.join("nodes.json"), &pending.data).map_err(|_| "站点配置保存失败")?;
    // Authentication is the commit marker. Never reopen installation once it exists.
    atomic_json(&dir.join("auth.json"), &pending.auth).map_err(|_| "管理员配置保存失败")?;
    for name in ["install.pending.json", "setup.json", "setup-link.txt"] {
        let _ = std::fs::remove_file(dir.join(name));
    }
    Ok(())
}
pub fn recover(dir: &Path) -> Result<(), &'static str> {
    if !dir.join("auth.json").exists() && dir.join("install.pending.json").exists() {
        let pending: Pending =
            read_json(&dir.join("install.pending.json")).map_err(|_| "安装恢复记录不可用")?;
        if pending.key.len() != 32
            || pending.data.schema != 2
            || !pending.data.secrets.is_empty()
            || !pending.data.nodes.is_empty()
        {
            return Err("安装恢复记录无效");
        }
        commit(dir, pending)?;
    }
    Ok(())
}
pub fn initialize(dir: &Path, v: Input) -> Result<(), &'static str> {
    validate(&v)?;
    if dir.join("auth.json").exists()
        || dir.join("nodes.json").exists()
        || dir.join("app.key").exists()
    {
        return Err("已有配置，禁止覆盖安装");
    }
    let password = zeroize::Zeroizing::new(v.password);
    let auth = Auth {
        username: v.username,
        hash: bcrypt::hash(password.as_str(), 12).map_err(|_| "密码处理失败")?,
        version: token(),
        ..Default::default()
    };
    let data = Data {
        schema: 2,
        site: Site {
            name: v.name.trim().into(),
            public: true,
        },
        ..Default::default()
    };
    let mut key = vec![0u8; 32];
    getrandom::fill(&mut key).map_err(|_| "安全随机数不可用")?;
    let pending = Pending { auth, data, key };
    atomic_json(&dir.join("install.pending.json"), &pending).map_err(|_| "安装记录不可用")?;
    commit(dir, pending)
}
#[derive(Clone)]
struct Setup {
    dir: PathBuf,
    origin: String,
    hash: String,
    csrf: String,
    slot: Arc<Semaphore>,
    done: CancellationToken,
}
fn cookie(req: &Request<Body>) -> &str {
    req.headers()
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .split(';')
        .filter_map(|s| s.trim().split_once('='))
        .find(|(k, _)| *k == COOKIE)
        .map(|(_, v)| v)
        .unwrap_or("")
}
fn owns(s: &Setup, raw: &str) -> bool {
    crate::retained::valid(raw) && constant(&hex::encode(Sha256::digest(raw.as_bytes())), &s.hash)
}
async fn handler(State(s): State<Setup>, req: Request<Body>) -> Response {
    let result = async {
        if !web::host_matches(&req, &s.origin) {
            return Err(ApiError::new(421, "请使用配置的站点域名"));
        }
        if s.dir.join("auth.json").exists() {
            return Err(ApiError::new(409, "站点已安装，安装入口已锁定"));
        }
        let path = req.uri().path().to_owned();
        if req.method() == "GET" && path == "/" {
            if let Some(key) = req
                .uri()
                .query()
                .and_then(|q| q.strip_prefix("setup_key="))
                .filter(|key| owns(&s, key))
            {
                let mut r = axum::http::StatusCode::SEE_OTHER.into_response();
                r.headers_mut()
                    .insert(header::LOCATION, header::HeaderValue::from_static("/"));
                r.headers_mut().insert(
                    header::SET_COOKIE,
                    format!(
                        "{COOKIE}={key}; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age=1800"
                    )
                    .parse()
                    .unwrap(),
                );
                return Ok(r);
            }
            return Ok((
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                include_str!("../assets/setup-rust.html"),
            )
                .into_response());
        }
        if req.method() == "GET" && path == "/setup.js" {
            return Ok((
                [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
                include_str!("../assets/setup-rust.js"),
            )
                .into_response());
        }
        let owned = owns(&s, cookie(&req));
        if req.method() == "GET" && path == "/install/status" {
            return Ok(ApiReply::ok(if owned {
                json!({"owner":true,"csrf":s.csrf})
            } else {
                json!({"owner":false})
            })
            .into_response());
        }
        if req.method() != "POST" || path != "/install" {
            return Err(ApiError::new(404, "请先完成安装"));
        }
        if !owned
            || req.headers().get("Origin").and_then(|v| v.to_str().ok()) != Some(s.origin.as_str())
            || !req
                .headers()
                .get("X-CSRF-Token")
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| constant(v, &s.csrf))
        {
            return Err(ApiError::new(403, "请使用一次性安装链接并刷新页面"));
        }
        if req
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            != Some("application/json")
        {
            return Err(ApiError::new(415, "请使用 JSON 请求"));
        }
        let permit = s
            .slot
            .clone()
            .try_acquire_owned()
            .map_err(|_| ApiError::new(409, "安装正在进行"))?;
        let bytes = tokio::time::timeout(Duration::from_secs(5), to_bytes(req.into_body(), 4096))
            .await
            .map_err(|_| ApiError::new(408, "请求超时"))?
            .map_err(|_| ApiError::new(413, "请求内容过大"))?;
        let input: Input =
            serde_json::from_slice(&bytes).map_err(|_| ApiError::new(400, "安装信息不正确"))?;
        validate(&input).map_err(|e| ApiError::new(400, e))?;
        let dir = s.dir.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            initialize(&dir, input)
        })
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|e| ApiError::new(409, e))?;
        let done = s.done.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            done.cancel();
        });
        let mut r = ApiReply::ok(json!({"ok":true,"redirect":"/?login=1"})).into_response();
        r.headers_mut().insert(
            header::SET_COOKIE,
            header::HeaderValue::from_static(
                "__Host-yuji_setup=; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age=0",
            ),
        );
        Ok(r)
    }
    .await;
    web::secure(match result {
        Ok(r) => r,
        Err(e) => e.into_response(),
    })
}
pub async fn serve(
    dir: &Path,
    origin: &str,
    listen: std::net::SocketAddr,
) -> Result<(), &'static str> {
    let state = dir.join("setup.json");
    let hash = if state.exists() {
        let v: serde_json::Value = read_json(&state).map_err(|_| "安装所有权记录不可用")?;
        v["hash"]
            .as_str()
            .filter(|v| crate::retained::valid(v))
            .ok_or("安装所有权记录无效")?
            .to_owned()
    } else {
        let key = token();
        let hash = hex::encode(Sha256::digest(key.as_bytes()));
        atomic_json(&state, &json!({"hash":hash})).map_err(|_| "安装所有权记录不可用")?;
        atomic_bytes(
            &dir.join("setup-link.txt"),
            format!("{origin}/?setup_key={key}\n").as_bytes(),
        )
        .map_err(|_| "安装链接不可用")?;
        hash
    };
    let done = CancellationToken::new();
    let s = Setup {
        dir: dir.into(),
        origin: origin.into(),
        hash,
        csrf: token(),
        slot: Arc::new(Semaphore::new(1)),
        done: done.clone(),
    };
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .map_err(|_| "loopback port unavailable")?;
    eprintln!("Installation pending; open the private setup-link.txt in the state directory.");
    axum::serve(listener, Router::new().fallback(any(handler)).with_state(s))
        .with_graceful_shutdown(async move {
            tokio::select! {_=done.cancelled()=>{},_=crate::signal()=>{}}
        })
        .await
        .map_err(|_| "安装服务失败")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn installation_input_is_validated() {
        let v = Input {
            name: "羽迹探针".into(),
            username: "admin".into(),
            password: "long-test-password".into(),
        };
        assert!(validate(&v).is_ok());
        assert!(
            validate(&Input {
                username: "admin;id".into(),
                ..v
            })
            .is_err()
        );
    }
}
