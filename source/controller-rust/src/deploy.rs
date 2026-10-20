use crate::{core::*, model::*, ssh};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD, STANDARD_NO_PAD},
};
use ed25519_dalek::{Signature, VerifyingKey};
use flate2::{Compression, write::GzEncoder};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{net::SocketAddr, time::Duration};
use tokio::time::timeout;
use zeroize::Zeroizing;
const RELEASE_VERSION: &str = "0.11.3";
pub const AGENT_VERSION: &str = "0.2.3";
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Inspect {
    ip: String,
    port: u16,
}
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct TrustInput {
    inspection: String,
    replace: bool,
}
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Input {
    id: String,
    password: String,
}
fn address(n: &Node) -> ApiResult<SocketAddr> {
    Ok(SocketAddr::new(
        n.ip.parse()
            .map_err(|_| ApiError::new(400, "SSH IP 不正确"))?,
        u16::try_from(n.port)
            .ok()
            .filter(|v| *v > 0)
            .ok_or_else(|| ApiError::new(400, "SSH 端口不正确"))?,
    ))
}
pub async fn inspect(app: App, c: Context, body: Vec<u8>) -> ApiResult<ApiReply> {
    let v: Inspect = decode(&body)?;
    if !acceptable_ip(&v.ip) || v.port == 0 {
        return Err(ApiError::new(400, "请填写有效的服务器 IP 与端口"));
    }
    let sid = {
        let mut i = app.lock();
        let x = app.guard(&mut i, &c, true, true)?;
        i.request(&format!("inspect:{}", c.ip), 12)?;
        x.id
    };
    let _slot = app
        .0
        .deploy_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::rate("正在检查其他服务器，请稍后重试", 5))?;
    let address = SocketAddr::new(v.ip.parse().unwrap(), v.port);
    let key = ssh::inspect(address)
        .await
        .map_err(|_| ApiError::new(502, "无法获取 SSH 主机密钥，请检查 IP、端口和防火墙"))?;
    let raw = key.to_bytes().map_err(|_| ApiError::internal())?;
    let encoded = STANDARD.encode(&raw);
    let fingerprint = host_fingerprint(&raw);
    let mut i = app.lock();
    let current = app.guard(&mut i, &c, true, true)?;
    if sid != current.id {
        return Err(ApiError::new(401, "登录已过期"));
    }
    let pinned = i.pins.get(&address.to_string()).cloned();
    let changed = pinned.as_ref().is_some_and(|old| old != &encoded);
    let previous_fingerprint = pinned
        .as_ref()
        .map(|old| STANDARD.decode(old).map(|raw| host_fingerprint(&raw)))
        .transpose()
        .map_err(|_| ApiError::new(409, "保存的 SSH 指纹无效，请检查信任记录"))?;
    i.trust.retain(|_, v| v.expires >= now());
    if i.trust.len() >= 128 {
        return Err(ApiError::rate("请稍后重试", 60));
    }
    let id = token();
    i.trust.insert(
        id.clone(),
        Trust {
            session_id: sid,
            address: address.to_string(),
            key: encoded.clone(),
            previous_key: pinned.clone(),
            expires: now() + 300,
        },
    );
    Ok(ApiReply::ok(
        json!({"trusted":pinned.as_ref()==Some(&encoded),"changed":changed,"address":address.to_string(),"fingerprint":fingerprint,"previousFingerprint":previous_fingerprint,"inspection":id}),
    ))
}
fn host_fingerprint(raw: &[u8]) -> String {
    format!("SHA256:{}", STANDARD_NO_PAD.encode(Sha256::digest(raw)))
}
pub fn trust(app: &App, i: &mut Inner, c: &Context, body: &[u8]) -> ApiResult<ApiReply> {
    let x = app.guard(i, c, true, true)?;
    let v: TrustInput = decode(body)?;
    let found = i
        .trust
        .get(&v.inspection)
        .filter(|v| v.session_id == x.id && v.expires >= now())
        .cloned()
        .ok_or_else(|| ApiError::new(400, "主机检查已过期，请重新检查"))?;
    // The administrator confirms exactly the old/new pair from this session.
    // A concurrent trust change invalidates the inspection, even if the new key matches.
    if i.pins.get(&found.address) != found.previous_key.as_ref() {
        i.trust.remove(&v.inspection);
        return Err(ApiError::new(
            409,
            "SSH 信任记录已更新，请重新检查并核对指纹",
        ));
    }
    let changed = found
        .previous_key
        .as_ref()
        .is_some_and(|old| old != &found.key);
    if changed && !v.replace {
        return Err(ApiError::new(
            409,
            "服务器 SSH 指纹已变更，请核对新旧指纹并明确确认更新",
        ));
    }
    if changed
        && i.jobs.values().any(|job| {
            job.state == "running"
                && i.data.nodes.iter().any(|node| {
                    node.public.id == job.node_id
                        && address(node).is_ok_and(|value| value.to_string() == found.address)
                })
        })
    {
        return Err(ApiError::new(409, "此服务器正在部署，请等待完成后重新检查"));
    }
    let subject = if changed {
        let old = STANDARD
            .decode(found.previous_key.as_deref().unwrap_or_default())
            .map_err(|_| ApiError::internal())?;
        let new = STANDARD
            .decode(&found.key)
            .map_err(|_| ApiError::internal())?;
        format!(
            "{} · {} → {}",
            found.address,
            host_fingerprint(&old),
            host_fingerprint(&new)
        )
    } else {
        found.address.clone()
    };
    let mut pins = i.pins.clone();
    pins.insert(found.address.clone(), found.key.clone());
    atomic_json(&app.0.dir.join("ssh-pins.json"), &pins).map_err(|_| ApiError::internal())?;
    i.pins = pins;
    i.trust.remove(&v.inspection);
    app.record(
        i,
        if changed {
            "ssh_host_replaced"
        } else {
            "ssh_host_trusted"
        },
        &subject,
    );
    Ok(ApiReply::ok(json!({"ok":true})))
}
pub fn ensure_secret(app: &App, i: &mut Inner, id: &str) -> ApiResult<NodeSecret> {
    if let Some(s) = i.data.secrets.get(id)
        && !s.ssh_key.is_empty()
        && !s.token.is_empty()
    {
        let mut s = s.clone();
        if s.recovery_key.is_empty() {
            let (key, public) =
                ssh::create_key("vistart-probe-recovery").map_err(|_| ApiError::internal())?;
            s.recovery_key = app
                .seal(&format!("{id}:recovery"), &key)
                .map_err(|_| ApiError::internal())?;
            s.recovery_public = public;
        }
        return Ok(s);
    }
    let plain = Zeroizing::new(token());
    let (key, public) =
        ssh::create_key("vistart-probe-managed").map_err(|_| ApiError::internal())?;
    let (recovery, recovery_public) =
        ssh::create_key("vistart-probe-recovery").map_err(|_| ApiError::internal())?;
    let secret = NodeSecret {
        recovery_key: app
            .seal(&format!("{id}:recovery"), &recovery)
            .map_err(|_| ApiError::internal())?,
        recovery_public,
        token_hash: hex::encode(Sha256::digest(plain.as_bytes())),
        token: app
            .seal(&format!("{id}:token"), plain.as_bytes())
            .map_err(|_| ApiError::internal())?,
        ssh_key: app
            .seal(&format!("{id}:ssh"), &key)
            .map_err(|_| ApiError::internal())?,
        public_key: public,
        host_key: String::new(),
    };
    Ok(secret)
}
pub fn begin(app: &App, i: &mut Inner, c: &Context, body: &[u8]) -> ApiResult<ApiReply> {
    app.guard(i, c, true, true)?;
    let v: Input = decode(body)?;
    let node = i
        .data
        .nodes
        .iter()
        .find(|n| n.public.id == v.id && !n.demo)
        .cloned()
        .ok_or_else(|| ApiError::new(404, "请添加真实服务器"))?;
    if node.removing
        || i.ops
            .transfers
            .get(&v.id)
            .is_some_and(|t| matches!(t.state.as_str(), "pending" | "running"))
    {
        return Err(ApiError::new(409, "该服务器已有管理任务，请等待完成"));
    }
    if !acceptable_ip(&node.ip)
        || !username(&node.username)
        || v.password.is_empty()
        || v.password.len() > 256
        || v.password.contains(['\r', '\n', '\0'])
    {
        return Err(ApiError::new(400, "请填写有效的 SSH 用户名与密码"));
    }
    let pin = i
        .pins
        .get(&address(&node)?.to_string())
        .cloned()
        .ok_or_else(|| ApiError::new(409, "请先核实 SSH 主机指纹"))?;
    if i.jobs
        .values()
        .any(|j| j.node_id == v.id && j.state == "running")
    {
        return Err(ApiError::new(409, "此服务器正在部署"));
    }
    let slot = app
        .0
        .deploy_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::rate("当前有其他部署任务，请稍后再试", 5))?;
    if i.enrollments
        .values()
        .any(|e| e.node == v.id && e.claimed && e.expires >= now())
    {
        return Err(ApiError::new(409, "一次性接入正在处理，请稍后重试"));
    }
    let mut secret = ensure_secret(app, i, &v.id)?;
    let agent_token = app
        .unseal(&format!("{}:token", v.id), &secret.token)
        .map_err(|_| ApiError::internal())?;
    secret.host_key = pin;
    let mut data = i.data.clone();
    data.secrets.insert(v.id.clone(), secret.clone());
    let job = DeployJob {
        id: token()[..24].into(),
        node_id: v.id,
        state: "running".into(),
        message: "正在连接服务器".into(),
        started: now(),
    };
    if let Some(n) = data.nodes.iter_mut().find(|n| n.public.id == job.node_id) {
        n.deploy_state = job.state.clone();
        n.deploy_message = job.message.clone();
    }
    app.save_data(i, data)?;
    i.jobs
        .retain(|_, j| j.state == "running" || now() - j.started < 3600);
    i.jobs.insert(job.id.clone(), job.clone());
    app.record(i, "deploy_started", &node.public.name);
    let app = app.clone();
    let job_id = job.id.clone();
    let password = Zeroizing::new(v.password);
    tokio::spawn(async move {
        let _slot = slot;
        let result = tokio::select! {_=app.0.stop.cancelled()=>Err("服务正在关闭，部署已停止"),r=timeout(Duration::from_secs(240),run(&app,&job_id,&node,&secret,&agent_token,&password))=>r.unwrap_or(Err("部署超时，请检查节点连接"))};
        match result {
            Ok(()) => progress(&app, &job_id, "done", "Agent 已连接，监控数据已就绪"),
            Err(message) => progress(&app, &job_id, "failed", message),
        }
    });
    Ok(ApiReply::accepted(json!({"job":job})))
}
pub fn status(app: &App, i: &mut Inner, c: &Context, id: &str) -> ApiResult<ApiReply> {
    app.guard(i, c, true, false)?;
    let j = i
        .jobs
        .get(id)
        .ok_or_else(|| ApiError::new(404, "部署任务不存在"))?;
    Ok(ApiReply::ok(json!({"job":j})))
}
fn progress(app: &App, id: &str, state: &str, message: &str) {
    let mut i = app.lock();
    let Some(j) = i.jobs.get_mut(id) else { return };
    j.state = state.into();
    j.message = message.into();
    let node = j.node_id.clone();
    for n in &mut i.data.nodes {
        if n.public.id == node {
            n.deploy_state = state.into();
            n.deploy_message = message.into();
        }
    }
    if state != "running" {
        if atomic_json(&app.0.dir.join("nodes.json"), &i.data).is_err() {
            eprintln!("Deploy state persistence unavailable");
        }
        app.record(&mut i, &format!("deploy_{state}"), &node);
    }
}
// Keep the shared client redirect-free: only release downloads may follow
// GitHub's HTTPS asset redirects. Never forward panel/Agent credentials.
fn allowed_release_url(url: &reqwest::Url) -> bool {
    if url.scheme() != "https"
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return false;
    }
    match url.host_str() {
        Some("github.com") => {
            url.path()
                .strip_prefix("/YJVIst/YJmonitor/releases/download/v")
                .and_then(|p| p.split_once('/'))
                .is_some_and(|(version, name)| {
                    crate::backup::valid_version(version) && valid_release_name(name)
                })
                && url.query().is_none()
        }
        Some("release-assets.githubusercontent.com") => {
            url.path().starts_with("/github-production-release-asset/")
        }
        Some("objects.githubusercontent.com") => url
            .path()
            .starts_with("/github-production-release-asset-2e65be/"),
        _ => false,
    }
}
pub async fn get(app: &App, name: &str, limit: usize) -> Result<Vec<u8>, &'static str> {
    if !valid_release_name(name) {
        return Err("invalid release name");
    }
    // Offline cache is only consumed by callers which verify the signed manifest and digest.
    let cached = app.0.dir.join("releases").join(name);
    if let Ok(meta) = std::fs::symlink_metadata(&cached) {
        if meta.is_file() && meta.len() <= limit as u64 {
            let raw = std::fs::read(cached).map_err(|_| "release cache unavailable")?;
            // A later controller may require a newer Agent than a repair cache contains.
            if name == "stable.json" && manifest(&raw)?.version != AGENT_VERSION {
                return fetch_asset(app, RELEASE_VERSION, name, limit).await;
            }
            return Ok(raw);
        }
        return Err("release cache invalid");
    }
    fetch_asset(app, RELEASE_VERSION, name, limit).await
}
fn valid_release_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 160
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}
// Online metadata checks bypass the deployment cache and pin one validated release tag.
pub async fn fetch_asset(
    app: &App,
    version: &str,
    name: &str,
    limit: usize,
) -> Result<Vec<u8>, &'static str> {
    if !crate::backup::valid_version(version) || !valid_release_name(name) {
        return Err("invalid release asset");
    }
    timeout(Duration::from_secs(50), async {
        let mut url = reqwest::Url::parse(&format!(
            "https://github.com/YJVIst/YJmonitor/releases/download/v{version}/{name}"
        ))
        .map_err(|_| "invalid release URL")?;
        for hop in 0..=4 {
            if !allowed_release_url(&url) {
                return Err("untrusted release redirect");
            }
            let mut response = app
                .0
                .http
                .get(url.clone())
                .send()
                .await
                .map_err(|_| "release unavailable")?;
            if matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
                if hop == 4 {
                    return Err("too many release redirects");
                }
                let location = response
                    .headers()
                    .get(reqwest::header::LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .ok_or("invalid release redirect")?;
                url = url.join(location).map_err(|_| "invalid release redirect")?;
                continue;
            }
            if response.status() != 200
                || response.content_length().is_some_and(|n| n > limit as u64)
            {
                return Err("release unavailable");
            }
            let mut out = Vec::new();
            while let Some(b) = response
                .chunk()
                .await
                .map_err(|_| "release download failed")?
            {
                if b.len() > limit.saturating_sub(out.len()) {
                    return Err("release too large");
                }
                out.extend(b);
            }
            return Ok(out);
        }
        Err("release unavailable")
    })
    .await
    .map_err(|_| "release download timeout")?
}

pub fn manifest(raw: &[u8]) -> Result<Release, &'static str> {
    let e: SignedRelease = serde_json::from_slice(raw).map_err(|_| "invalid manifest")?;
    let payload = STANDARD.decode(e.payload).map_err(|_| "invalid manifest")?;
    let sig = STANDARD
        .decode(e.signature)
        .map_err(|_| "invalid signature")?;
    let pubkey = STANDARD
        .decode(include_str!("../assets/release-public.txt").trim())
        .map_err(|_| "invalid trust anchor")?;
    let key = VerifyingKey::from_bytes(&pubkey.try_into().map_err(|_| "invalid trust anchor")?)
        .map_err(|_| "invalid trust anchor")?;
    key.verify_strict(
        &payload,
        &Signature::from_slice(&sig).map_err(|_| "invalid signature")?,
    )
    .map_err(|_| "signature mismatch")?;
    let m: Release = serde_json::from_slice(&payload).map_err(|_| "invalid manifest")?;
    if !crate::backup::valid_version(&m.version) {
        return Err("invalid release version");
    }
    for arch in ["amd64", "arm64"] {
        let f = m.files.get(arch).ok_or("architecture unavailable")?;
        if f.name != format!("vistart-probe-agent-{}-linux-{arch}", m.version)
            || !(1024..=32 * 1024 * 1024).contains(&f.size)
            || f.sha256.len() != 64
            || !f.sha256.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err("invalid release entry");
        }
    }
    Ok(m)
}
pub fn release(raw: &[u8], arch: &str) -> Result<ReleaseFile, &'static str> {
    let m = manifest(raw)?;
    if m.version != AGENT_VERSION {
        return Err("unsupported release version");
    }
    m.files.get(arch).cloned().ok_or("architecture unavailable")
}
pub async fn fetch(app: &App, arch: &str) -> Result<Vec<u8>, &'static str> {
    let f = release(&get(app, "stable.json", 16384).await?, arch)?;
    let binary = get(app, &f.name, 32 * 1024 * 1024).await?;
    if binary.len() as i64 != f.size || hex::encode(Sha256::digest(&binary)) != f.sha256 {
        return Err("release checksum mismatch");
    }
    Ok(binary)
}
pub fn installation(
    binary: &[u8],
    config: &[u8],
    public: &str,
    recovery_public: &str,
    manifest: &[u8],
    user: &str,
) -> Result<Vec<u8>, &'static str> {
    let mut script =
        include_str!("../assets/install-agent.sh").replace("__SSH_USER__", &ssh::quote(user));
    if cfg!(feature = "test-deployment") {
        script = script
            .replace("vistart-probe-agent", "PROBE_AGENT_NAMESPACE")
            .replace("vistart-probe", "vistart-probe-test")
            .replace("PROBE_AGENT_NAMESPACE", "vistart-probe-test-agent");
    }
    let authorized = format!(
        "from=\"127.0.0.1,::1\",no-agent-forwarding,no-port-forwarding,no-X11-forwarding {public} vistart-probe-managed\n"
    );
    let gz = GzEncoder::new(Vec::new(), Compression::fast());
    let mut tar = tar::Builder::new(gz);
    for (name, bytes, mode) in [
        ("agent", binary, 0o755),
        ("config.json", config, 0o600),
        ("install.sh", script.as_bytes(), 0o600),
        (
            "agent-platform.sh",
            include_bytes!("../assets/agent-platform.sh").as_slice(),
            0o600,
        ),
        ("authorized-key", authorized.as_bytes(), 0o600),
        ("recovery-public", recovery_public.as_bytes(), 0o600),
        ("agent.manifest", manifest, 0o600),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(mode);
        header.set_cksum();
        tar.append_data(&mut header, name, bytes)
            .map_err(|_| "installation archive failed")?;
    }
    tar.into_inner()
        .map_err(|_| "installation archive failed")?
        .finish()
        .map_err(|_| "installation archive failed")
}
struct RemoteDir {
    client: std::sync::Arc<ssh::Client>,
    user: String,
    password: Zeroizing<String>,
    path: String,
}
impl Drop for RemoteDir {
    fn drop(&mut self) {
        let c = self.client.clone();
        let u = self.user.clone();
        let p = Zeroizing::new(self.password.to_string());
        let path = self.path.clone();
        tokio::spawn(async move {
            let _ = timeout(
                Duration::from_secs(5),
                ssh::remote(&c, &u, &p, &format!("rm -rf -- {}", ssh::quote(&path)), &[]),
            )
            .await;
            let _ = timeout(
                Duration::from_secs(1),
                c.disconnect(russh::Disconnect::ByApplication, "deployment finished", ""),
            )
            .await;
        });
    }
}
async fn run(
    app: &App,
    job: &str,
    node: &Node,
    secret: &NodeSecret,
    agent_token: &[u8],
    password: &str,
) -> Result<(), &'static str> {
    let address = address(node).map_err(|_| "SSH 地址不正确")?;
    let client = ssh::password_client(address, &node.username, password, &secret.host_key)
        .await
        .map_err(|_| "SSH 验证失败，请核实用户名、密码与服务器指纹")?;
    progress(app, job, "running", "正在检查系统与架构");
    let check = ssh::remote(
        &client,
        &node.username,
        password,
        "sh -s -- prepare",
        include_bytes!("../assets/agent-platform.sh"),
    )
    .await
    .map_err(|_| "系统检查失败，请核实受支持的 Linux/systemd 版本、架构与软件源")?;
    let arch = check.lines().last().unwrap_or("").trim();
    if !["amd64", "arm64"].contains(&arch) {
        return Err("服务器架构检查失败");
    }
    progress(app, job, "running", "正在拉取并校验 Agent");
    let binary = fetch(app, arch)
        .await
        .map_err(|_| "Agent 下载或签名校验失败，请检查发布源")?;
    let token = std::str::from_utf8(agent_token).map_err(|_| "节点凭证无效")?;
    let config=Zeroizing::new(serde_json::to_vec(&json!({"node_id":node.public.id,"token":token,"controller_url":app.0.origin.replacen("https://","wss://",1)+"/api/agent","ssh_port":node.port})).map_err(|_|"节点配置无效")?);
    let payload = Zeroizing::new(
        installation(
            &binary,
            &config,
            &secret.public_key,
            &secret.recovery_public,
            &get(app, "stable.json", 16384).await?,
            &node.username,
        )
        .map_err(|_| "安装文件准备失败")?,
    );
    let temp = ssh::remote(
        &client,
        &node.username,
        password,
        "umask 077; mktemp -d /var/tmp/vistart-probe-install.XXXXXXXX",
        &[],
    )
    .await
    .map_err(|_| "安装目录创建失败")?;
    let temp = temp.trim();
    let suffix = temp
        .strip_prefix("/var/tmp/vistart-probe-install.")
        .ok_or("安装目录创建失败")?;
    if suffix.len() != 8 || !suffix.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err("安装目录创建失败");
    }
    let _cleanup = RemoteDir {
        client: client.clone(),
        user: node.username.clone(),
        password: Zeroizing::new(password.into()),
        path: temp.into(),
    };
    ssh::remote(
        &client,
        &node.username,
        password,
        &format!("tar -xzf - -C {}", ssh::quote(temp)),
        &payload,
    )
    .await
    .map_err(|_| "Agent 上传失败")?;
    progress(app, job, "running", "正在安装独立探针服务");
    let installed_at = now();
    ssh::remote(
        &client,
        &node.username,
        password,
        &format!("cd {} && sh ./install.sh", ssh::quote(temp)),
        &[],
    )
    .await
    .map_err(|_| "Agent 安装失败，请检查系统权限、依赖和磁盘空间")?;
    progress(app, job, "running", "安装完成，等待 WSS 首次上报");
    for _ in 0..45 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        if app.lock().data.nodes.iter().any(|n| {
            n.public.id == node.public.id
                && n.last_seen >= installed_at
                && now() - n.last_seen < 10
                && n.agent_version == AGENT_VERSION
        }) {
            return Ok(());
        }
    }
    Err("Agent 已安装，但尚未收到上报，请检查 WSS 网络连接")
}
#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        app: App,
        context: Context,
        inspection: String,
        old: String,
        new: String,
    }
    impl Fixture {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("probe-host-trust-{}", token()));
            std::fs::create_dir(&dir).unwrap();
            atomic_json(
                &dir.join("auth.json"),
                &Auth {
                    version: token(),
                    ..Default::default()
                },
            )
            .unwrap();
            atomic_json(
                &dir.join("nodes.json"),
                &Data {
                    schema: 2,
                    ..Default::default()
                },
            )
            .unwrap();
            let app = App::new(dir, "https://trust.test".into(), false).unwrap();
            let session = app.lock().new_session("", true).unwrap();
            let mut headers = axum::http::HeaderMap::new();
            headers.insert(
                "Cookie",
                format!("{COOKIE}={}", session.id).parse().unwrap(),
            );
            headers.insert("Origin", "https://trust.test".parse().unwrap());
            headers.insert("X-CSRF-Token", session.csrf.parse().unwrap());
            let context = Context::new(
                "POST",
                "/api/admin/trust-ssh",
                headers,
                "127.0.0.1:9999".parse().unwrap(),
                &app.0.origin,
            );
            let inspection = token();
            let old = STANDARD.encode(b"old observed host key");
            let new = STANDARD.encode(b"new observed host key");
            {
                let mut i = app.lock();
                i.pins.insert("192.0.2.1:22".into(), old.clone());
                i.pins.insert("192.0.2.1:2222".into(), old.clone());
                i.trust.insert(
                    inspection.clone(),
                    Trust {
                        session_id: session.id,
                        address: "192.0.2.1:22".into(),
                        key: new.clone(),
                        previous_key: Some(old.clone()),
                        expires: now() + 300,
                    },
                );
                atomic_json(&app.0.dir.join("ssh-pins.json"), &i.pins).unwrap();
            }
            Self {
                app,
                context,
                inspection,
                old,
                new,
            }
        }
        fn confirm(&self, replace: bool) -> ApiResult<ApiReply> {
            trust(
                &self.app,
                &mut self.app.lock(),
                &self.context,
                &serde_json::to_vec(&json!({"inspection":self.inspection,"replace":replace}))
                    .unwrap(),
            )
        }
        fn stored(&self) -> std::collections::HashMap<String, String> {
            read_json(&self.app.0.dir.join("ssh-pins.json")).unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.app.0.dir);
        }
    }
    #[test]
    fn changed_key_requires_explicit_confirmation_and_is_one_use() {
        let f = Fixture::new();
        assert_eq!(f.confirm(false).err().unwrap().status, 409);
        assert_eq!(f.stored()["192.0.2.1:22"], f.old);
        assert!(f.confirm(true).is_ok());
        assert_eq!(f.stored()["192.0.2.1:22"], f.new);
        assert_eq!(f.stored()["192.0.2.1:2222"], f.old);
        assert_eq!(f.confirm(true).err().unwrap().status, 400);
        let i = f.app.lock();
        assert_eq!(i.audit.last().unwrap().action, "ssh_host_replaced");
        assert!(
            i.audit
                .last()
                .unwrap()
                .subject
                .contains(&host_fingerprint(b"old observed host key"))
        );
        assert!(
            i.audit
                .last()
                .unwrap()
                .subject
                .contains(&host_fingerprint(b"new observed host key"))
        );
    }
    #[test]
    fn first_connection_and_unchanged_key_keep_original_flow() {
        for first in [true, false] {
            let f = Fixture::new();
            let mut i = f.app.lock();
            if first {
                i.pins.remove("192.0.2.1:22");
                i.trust.get_mut(&f.inspection).unwrap().previous_key = None;
            } else {
                i.trust.get_mut(&f.inspection).unwrap().key = f.old.clone();
            }
            drop(i);
            assert!(f.confirm(false).is_ok());
            assert_eq!(
                f.app.lock().audit.last().unwrap().action,
                "ssh_host_trusted"
            );
        }
    }
    #[test]
    fn concurrent_trust_change_invalidates_old_confirmation() {
        for concurrent in [
            STANDARD.encode(b"third key"),
            STANDARD.encode(b"new observed host key"),
        ] {
            let f = Fixture::new();
            f.app
                .lock()
                .pins
                .insert("192.0.2.1:22".into(), concurrent.clone());
            assert_eq!(f.confirm(true).err().unwrap().status, 409);
            assert_eq!(f.app.lock().pins["192.0.2.1:22"], concurrent);
        }
    }
    #[test]
    fn inspection_is_bound_to_login_and_expiry() {
        let f = Fixture::new();
        f.app
            .lock()
            .trust
            .get_mut(&f.inspection)
            .unwrap()
            .session_id = token();
        assert_eq!(f.confirm(true).err().unwrap().status, 400);
        let mut i = f.app.lock();
        let found = i.trust.get_mut(&f.inspection).unwrap();
        found.session_id = f.context.sid.clone();
        found.expires = now() - 1;
        drop(i);
        assert_eq!(f.confirm(true).err().unwrap().status, 400);
        assert_eq!(f.stored()["192.0.2.1:22"], f.old);
    }
    #[test]
    fn host_rotation_requires_admin_and_csrf() {
        let mut f = Fixture::new();
        f.context.headers.remove("X-CSRF-Token");
        assert_eq!(f.confirm(true).err().unwrap().status, 403);
        f.app.lock().sessions.get_mut(&f.context.sid).unwrap().auth = false;
        assert_eq!(f.confirm(true).err().unwrap().status, 401);
        assert_eq!(f.stored()["192.0.2.1:22"], f.old);
    }
    #[test]
    fn failed_persistence_does_not_replace_in_memory_key() {
        let f = Fixture::new();
        let path = f.app.0.dir.join("ssh-pins.json");
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert_eq!(f.confirm(true).err().unwrap().status, 500);
        let i = f.app.lock();
        assert_eq!(i.pins["192.0.2.1:22"], f.old);
        assert!(i.trust.contains_key(&f.inspection));
        assert!(i.audit.is_empty());
    }
    #[test]
    fn active_deployment_prevents_rotation() {
        let f = Fixture::new();
        let mut i = f.app.lock();
        i.data.nodes.push(Node {
            ip: "192.0.2.1".into(),
            port: 22,
            public: PublicNode {
                id: "node".into(),
                ..Default::default()
            },
            ..Default::default()
        });
        i.jobs.insert(
            "job".into(),
            DeployJob {
                id: "job".into(),
                node_id: "node".into(),
                state: "running".into(),
                message: String::new(),
                started: now(),
            },
        );
        drop(i);
        assert_eq!(f.confirm(true).err().unwrap().status, 409);
        assert_eq!(f.stored()["192.0.2.1:22"], f.old);
    }
    #[test]
    fn confirmation_cannot_supply_its_own_key_or_target() {
        for extra in [json!({"key":"injected"}), json!({"address":"192.0.2.3:22"})] {
            let f = Fixture::new();
            let mut body = json!({"inspection":f.inspection,"replace":true});
            body.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            assert_eq!(
                trust(
                    &f.app,
                    &mut f.app.lock(),
                    &f.context,
                    &serde_json::to_vec(&body).unwrap()
                )
                .err()
                .unwrap()
                .status,
                400
            );
            assert_eq!(f.stored()["192.0.2.1:22"], f.old);
        }
    }
    #[test]
    fn release_redirect_boundaries() {
        for url in [
            "https://github.com/YJVIst/YJmonitor/releases/download/v0.11.3/stable.json",
            "https://release-assets.githubusercontent.com/github-production-release-asset/1/abc?sig=example",
            "https://objects.githubusercontent.com/github-production-release-asset-2e65be/1/abc",
        ] {
            assert!(
                allowed_release_url(&reqwest::Url::parse(url).unwrap()),
                "{url}"
            );
        }
        for url in [
            "http://github.com/YJVIst/YJmonitor/releases/download/v0.11.3/stable.json",
            "https://github.com:444/YJVIst/YJmonitor/releases/download/v0.11.3/stable.json",
            "https://user:password@github.com/YJVIst/YJmonitor/releases/download/v0.11.3/stable.json",
            "https://github.com/coexacx/other/releases/download/v0.11.3/stable.json",
            "https://github.com/coexacx/yuji-probe/releases/download/v0.11.3/stable.json",
            "https://github.com/YJVIst/YJmonitor/releases/download/rust-v0.11.0/stable.json",
            "https://github.com/YJVIst/YJmonitor/releases/download/v0.11.3/../../../login",
            "https://github.com/YJVIst/YJmonitor/releases/download/v0.11.3/stable.json#fragment",
            "https://github.com/YJVIst/YJmonitor/releases/download/v0.11.3/stable.json?redirect=1",
            "https://release-assets.githubusercontent.com.evil.example/github-production-release-asset/1",
            "https://release-assets.githubusercontent.com/elsewhere/1",
            "https://127.0.0.1/github-production-release-asset/1",
            "file:///etc/passwd",
        ] {
            assert!(
                !allowed_release_url(&reqwest::Url::parse(url).unwrap()),
                "{url}"
            );
        }
    }
    #[test]
    fn invalid_signed_release() {
        assert!(release(br#"{"payload":"e30=","signature":""}"#, "amd64").is_err());
    }
    #[test]
    fn archive_names_and_namespace() {
        let b = installation(
            b"test",
            b"{}",
            "ssh-ed25519 test",
            "ssh-ed25519 recovery",
            b"{}",
            "root",
        )
        .unwrap();
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(&b[..]));
        let paths: Vec<_> = tar
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().into_owned())
            .collect();
        assert_eq!(paths.len(), 7);
        assert!(paths.iter().any(|p| p == "agent.manifest"));
        assert!(paths.iter().any(|p| p == "recovery-public"));
        assert!(paths.iter().all(|p| p.components().count() == 1));
    }
    #[test]
    fn signed_agent_manifest_is_required_even_for_version_checks() {
        let raw = include_bytes!("../assets/agent-stable-test.json");
        assert_eq!(manifest(raw).unwrap().version, AGENT_VERSION);
        let mut envelope: serde_json::Value = serde_json::from_slice(raw).unwrap();
        envelope["payload"] = json!(STANDARD.encode(br#"{"version":"99.0.0","files":{}}"#));
        assert!(manifest(&serde_json::to_vec(&envelope).unwrap()).is_err());
        for name in [
            "",
            ".",
            "..",
            "../stable.json",
            "stable.json?x=1",
            "stable.json/",
        ] {
            assert!(!valid_release_name(name));
        }
        assert!(allowed_release_url(
            &reqwest::Url::parse(
                "https://github.com/YJVIst/YJmonitor/releases/download/v0.99.0/stable.json"
            )
            .unwrap()
        ));
    }
}
