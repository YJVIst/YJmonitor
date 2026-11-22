use crate::{core::*, deploy, ssh};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;
#[derive(Clone)]
pub struct Enrollment {
    pub node: String,
    pub version: String,
    pub owner: String,
    pub expires: i64,
    pub claimed: bool,
}
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Input {
    id: String,
    revoke: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Claim {
    token: String,
    arch: String,
    host_key: String,
}
pub fn issue(app: &App, i: &mut Inner, c: &Context, body: &[u8]) -> ApiResult<ApiReply> {
    let s = app.guard(i, c, true, true)?;
    let v: Input = decode(body)?;
    let n = i
        .data
        .nodes
        .iter()
        .find(|n| n.public.id == v.id && !n.demo && !n.removing)
        .ok_or_else(|| ApiError::new(404, "服务器不存在"))?;
    let name = n.public.name.clone();
    i.request(&format!("enrollment:{}", s.id), 12)?;
    i.enrollments
        .retain(|_, e| e.node != v.id && e.expires >= now());
    if v.revoke {
        return Ok(ApiReply::ok(json!({"ok":true})));
    }
    if i.enrollments.len() >= 32 {
        return Err(ApiError::rate("有效接入命令过多，请稍后重试", 60));
    }
    let code = Zeroizing::new(token());
    let digest = hex::encode(Sha256::digest(code.as_bytes()));
    i.enrollments.insert(
        digest,
        Enrollment {
            node: v.id,
            version: s.version,
            owner: s.handle,
            expires: now() + 600,
            claimed: false,
        },
    );
    app.record(i, "enrollment_issued", &name);
    let command = format!(
        r#"curl -fsS --proto '=https' --tlsv1.2 {} | (if [ "$(id -u)" = 0 ]; then sh -s -- {}; else sudo sh -s -- {}; fi)"#,
        ssh::quote(&format!("{}/api/enroll/script", app.0.origin)),
        ssh::quote(&code),
        ssh::quote(&code)
    );
    Ok(ApiReply::ok(json!({"command":command,"expires":now()+600})))
}
pub fn script(app: &App) -> ApiReply {
    let body = include_str!("../assets/enroll-agent.sh")
        .replace("__ORIGIN_JSON__", &json!(app.0.origin).to_string());
    let mut reply = ApiReply::ok(json!({"script":body}));
    reply.raw = Some(std::sync::Arc::from(body.into_bytes()));
    reply
}
pub async fn claim(app: App, c: Context, body: Vec<u8>) -> ApiResult<ApiReply> {
    if c.method != "POST" {
        return Err(ApiError::new(405, "请求方法不正确"));
    }
    let v: Claim = decode(&body)?;
    if !crate::file_sessions::valid(&v.token)
        || !["amd64", "arm64"].contains(&v.arch.as_str())
        || v.host_key.len() > 4096
    {
        return Err(ApiError::new(400, "接入信息不正确"));
    }
    let host = russh::keys::PublicKey::from_openssh(&v.host_key)
        .map_err(|_| ApiError::new(400, "SSH 主机公钥不正确"))?;
    let raw = host
        .to_bytes()
        .map_err(|_| ApiError::new(400, "SSH 主机公钥不正确"))?;
    let digest = hex::encode(Sha256::digest(v.token.as_bytes()));
    let (node, mut secret, version) = {
        let mut i = app.lock();
        i.request(&format!("enroll-claim:{}", c.ip), 6)?;
        let e = i
            .enrollments
            .get(&digest)
            .cloned()
            .filter(|e| {
                !e.claimed
                    && e.expires >= now()
                    && e.version == i.auth.version
                    && i.sessions.values().any(|s| {
                        s.auth
                            && s.handle == e.owner
                            && s.version == e.version
                            && now() - s.created < 28800
                            && now() - s.seen < 1800
                    })
            })
            .ok_or_else(|| ApiError::new(401, "接入命令已使用、过期或撤销"))?;
        let node = i
            .data
            .nodes
            .iter()
            .find(|n| n.public.id == e.node && !n.demo && !n.removing)
            .cloned()
            .ok_or_else(|| ApiError::new(404, "服务器不存在"))?;
        if i.jobs
            .values()
            .any(|j| j.node_id == node.public.id && j.state == "running")
        {
            return Err(ApiError::new(409, "服务器正在部署"));
        }
        if i.agents.contains_key(&node.public.id) {
            return Err(ApiError::new(
                409,
                "该节点已在线，请先停止原 Agent 后重新生成命令",
            ));
        }
        i.enrollments.get_mut(&digest).unwrap().claimed = true;
        let secret = deploy::ensure_secret(&app, &mut i, &node.public.id)?;
        (node, secret, e.version)
    };
    let _slot = app
        .0
        .deploy_slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::rate("其他安装正在进行，请重新生成接入命令", 10))?;
    let binary = deploy::fetch(&app, &v.arch)
        .await
        .map_err(|_| ApiError::new(502, "Agent 下载或签名校验失败，请重新生成命令"))?;
    let manifest = deploy::get(&app, "stable.json", 16384)
        .await
        .map_err(|_| ApiError::new(502, "发布清单不可用"))?;
    let plain = Zeroizing::new(token());
    secret.token_hash = hex::encode(Sha256::digest(plain.as_bytes()));
    secret.token = app
        .seal(&format!("{}:token", node.public.id), plain.as_bytes())
        .map_err(|_| ApiError::internal())?;
    ssh::bind_recovery(&app, &node.public.id, &mut secret, &raw)
        .map_err(|_| ApiError::internal())?;
    let config=Zeroizing::new(serde_json::to_vec(&json!({"node_id":node.public.id,"token":plain.as_str(),"controller_url":app.0.origin.replacen("https://","wss://",1)+"/api/agent","ssh_port":node.port})).map_err(|_|ApiError::internal())?);
    let archive = Zeroizing::new(
        deploy::installation(
            &binary,
            &config,
            &secret.public_key,
            &secret.recovery_public,
            &manifest,
            &node.username,
        )
        .map_err(|_| ApiError::internal())?,
    );
    {
        let mut i = app.lock();
        let valid_claim = i.enrollments.get(&digest).is_some_and(|e| {
            e.claimed
                && e.expires >= now()
                && i.sessions.values().any(|s| {
                    s.auth
                        && s.handle == e.owner
                        && s.version == version
                        && now() - s.created < 28800
                        && now() - s.seen < 1800
                })
        });
        if !valid_claim
            || i.auth.version != version
            || !i.data.nodes.iter().any(|n| {
                n.public.id == node.public.id
                    && !n.removing
                    && n.username == node.username
                    && n.ip == node.ip
                    && n.port == node.port
            })
        {
            return Err(ApiError::new(409, "接入授权已失效"));
        }
        let mut data = i.data.clone();
        data.secrets.insert(node.public.id.clone(), secret);
        if let Some(n) = data
            .nodes
            .iter_mut()
            .find(|n| n.public.id == node.public.id)
        {
            n.deploy_state = "enrolling".into();
            n.deploy_message = "接入命令已领取，等待 Agent 上线".into();
        }
        app.save_data(&mut i, data)?;
        i.enrollments.remove(&digest);
        app.record(&mut i, "enrollment_claimed", &node.public.name);
    }
    Ok(ApiReply::ok(
        json!({"archive":STANDARD.encode(archive.as_slice()),"sha256":hex::encode(Sha256::digest(archive.as_slice()))}),
    ))
}
