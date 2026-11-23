use crate::{core::*, deploy, model::*, operations, ssh};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Transfer {
    pub task_id: String,
    pub batch_id: String,
    pub updated_at: i64,
    pub action: String,
    pub target: String,
    pub state: String,
    pub message: String,
    pub next: i64,
    pub old: NodeSecret,
}
pub fn start_manage(app: &App, i: &mut Inner, id: &str, action: &str) -> ApiResult<()> {
    let n = i
        .data
        .nodes
        .iter()
        .find(|n| n.public.id == id)
        .ok_or_else(|| ApiError::new(404, "节点不存在"))?;
    if action == "upgrade"
        && let Some(reason) = crate::server_management::upgrade_blocked(i, n)
    {
        return Err(ApiError::new(409, reason));
    }
    if i.jobs
        .values()
        .any(|j| j.node_id == id && j.state == "running")
    {
        return Err(ApiError::new(409, "该服务器正在部署"));
    }
    if i.ops
        .transfers
        .get(id)
        .is_some_and(crate::server_management::busy)
    {
        return Err(ApiError::new(409, "该节点已有管理任务"));
    }
    let old = i
        .data
        .secrets
        .get(id)
        .cloned()
        .ok_or_else(|| ApiError::new(409, "节点尚未部署"))?;
    let previous = i.ops.transfers.get(id).cloned();
    i.ops.transfers.insert(
        id.into(),
        Transfer {
            task_id: token(),
            updated_at: now(),
            action: action.into(),
            target: app.0.origin.clone(),
            state: "pending".into(),
            message: "等待连接节点".into(),
            old,
            ..Default::default()
        },
    );
    let name = n.public.name.clone();
    if let Err(e) = operations::save(app, i) {
        if let Some(t) = previous {
            i.ops.transfers.insert(id.into(), t);
        } else {
            i.ops.transfers.remove(id);
        }
        return Err(e);
    }
    app.record(i, "node_management_requested", &format!("{name}: {action}"));
    Ok(())
}
pub async fn receive(_app: App, _c: Context, _body: Vec<u8>) -> ApiResult<ApiReply> {
    Err(ApiError::new(404, "恢复通过受限 SSH 通道执行"))
}
pub async fn tick(app: &App) {
    // Shared semaphore also bounds password deployments; fill at most two slots.
    for _ in 0..2 {
        start_next(app);
    }
}
fn start_next(app: &App) {
    let work = {
        let mut i = app.lock();
        let at = now();
        let id = i
            .ops
            .transfers
            .iter()
            .filter(|(id, t)| {
                ["pending", "retry"].contains(&t.state.as_str())
                    && t.next <= at
                    && i.data.nodes.iter().any(|n| &n.public.id == *id)
                    && !i
                        .jobs
                        .values()
                        .any(|j| &j.node_id == *id && j.state == "running")
                    && !i
                        .enrollments
                        .values()
                        .any(|e| &e.node == *id && e.claimed && e.expires >= at)
            })
            .min_by_key(|(id, t)| (t.next, *id))
            .map(|(id, _)| id.clone());
        let Some(id) = id else { return };
        let Some(node) = i.data.nodes.iter().find(|n| n.public.id == id).cloned() else {
            return;
        };
        let Ok(slot) = app.0.deploy_slots.clone().try_acquire_owned() else {
            return;
        };
        let t = i.ops.transfers.get_mut(&id).unwrap();
        let previous = t.clone();
        t.state = "running".into();
        t.updated_at = now();
        t.message = "正在连接被控服务器".into();
        let task = t.clone();
        if operations::save(app, &i).is_err() {
            i.ops.transfers.insert(id.clone(), previous);
            return;
        }
        (id, node, task, slot)
    };
    let app = app.clone();
    tokio::spawn(async move {
        let (id, node, task, _slot) = work;
        let result = tokio::time::timeout(Duration::from_secs(180), manage(&app, &node, &task))
            .await
            .unwrap_or(Err("管理操作超时"));
        let mut i = app.lock();
        // A removed/replaced task must never receive a stale completion.
        if !i
            .ops
            .transfers
            .get(&id)
            .is_some_and(|t| t.task_id == task.task_id && t.state == "running")
        {
            return;
        }
        if let Some(t) = i.ops.transfers.get_mut(&id) {
            t.updated_at = now();
            match &result {
                Ok(()) => {
                    t.state = "done".into();
                    t.message = match task.action.as_str() {
                        "remove" => "远端配置与专用密钥已清除",
                        "reconfigure" => "Agent 已连接新主控",
                        "rollback" => "Agent 已回退并通过连接检查",
                        _ => "Agent 已升级并通过连接检查",
                    }
                    .into();
                    t.old = NodeSecret::default();
                }
                Err(e) => {
                    t.state = if task.batch_id.is_empty() && task.action != "remove" {
                        "retry"
                    } else {
                        "failed"
                    }
                    .into();
                    t.message = (*e).into();
                    t.next = now() + 300;
                }
            }
        }
        if result.is_ok()
            && task.action == "remove"
            && crate::nodes::finish_removal(&app, &mut i, &id).is_err()
            && let Some(t) = i.ops.transfers.get_mut(&id)
        {
            t.message = "远端清理已完成，请再次移除以完成本地保存".into();
        }
        app.record(
            &mut i,
            if result.is_ok() {
                "node_management_done"
            } else {
                "node_management_failed"
            },
            &node.public.name,
        );
        let _ = operations::save(&app, &i);
    });
}
async fn manage(app: &App, node: &Node, task: &Transfer) -> Result<(), &'static str> {
    if !task.batch_id.is_empty() {
        let i = app.lock();
        if !i
            .data
            .nodes
            .iter()
            .any(|n| n.public.id == node.public.id && n.public.online && !n.removing)
        {
            return Err("服务器已离线或移除，请在恢复在线后重试");
        }
    }
    // Old Agents reject releases signed after the 0.2.4 key rotation.
    // Use the node's existing authenticated SSH tunnel for this one transition.
    if task.action == "upgrade" && crate::backup::newer_release("0.2.4", &node.agent_version) {
        return bootstrap(app, node).await;
    }
    if task.old.recovery_key.is_empty() {
        if task.action == "upgrade" {
            return bootstrap(app, node).await;
        }
        return Err("旧 Agent 尚未安装恢复密钥，请在原主控升级 Agent 或填写 SSH 密码重新部署");
    }
    let (client, host_key) = ssh::recovery_client(app, node, &task.old)
        .await
        .map_err(|_| "SSH 无法连接或恢复密钥验证失败")?;
    let proof = app.unseal(&format!("{}:token", node.public.id), &task.old.token)?;
    let proof = std::str::from_utf8(&proof).map_err(|_| "恢复凭据不可用")?;
    let mut request = json!({"action":task.action,"proof":proof});
    if task.action == "reconfigure" {
        let secret = {
            let mut i = app.lock();
            let mut data = i.data.clone();
            let next = data
                .secrets
                .get_mut(&node.public.id)
                .ok_or("新节点凭据不可用")?;
            ssh::bind_recovery(app, &node.public.id, next, &host_key)?;
            let secret = next.clone();
            app.save_data(&mut i, data)
                .map_err(|_| "保存恢复凭据失败")?;
            secret
        };
        let token = app.unseal(&format!("{}:token", node.public.id), &secret.token)?;
        request["config"] = json!({"node_id":node.public.id,"token":std::str::from_utf8(&token).map_err(|_|"节点凭据不可用")?,"ssh_port":node.port,"controller_url":task.target.replacen("https://","wss://",1)+"/api/agent"});
        request["public_key"] = json!(secret.public_key);
        request["recovery_public"] = json!(secret.recovery_public);
    } else if task.action == "upgrade" {
        let arch = match node.public.arch.as_str() {
            "amd64" | "x86_64" => "amd64",
            "arm64" | "aarch64" => "arm64",
            _ => return Err("无法确定节点架构"),
        };
        let raw = deploy::fetch(app, arch).await?;
        request["binary"] = json!(STANDARD.encode(raw));
        request["manifest"] =
            serde_json::from_slice(&deploy::get(app, "stable.json", 16384).await?)
                .map_err(|_| "发布清单不可用")?;
    }
    let payload =
        zeroize::Zeroizing::new(serde_json::to_vec(&request).map_err(|_| "管理请求不可用")?);
    let result = ssh::exec(&client, "yuji-manage", &payload, 32768).await;
    let _ = client
        .disconnect(russh::Disconnect::ByApplication, "management finished", "")
        .await;
    match result {
        Ok(reply) if reply.lines().any(|v| v.trim() == r#"{"ok":true}"#) => {
            if task.action == "upgrade" {
                let (client, _) = ssh::recovery_client(app, node, &task.old).await?;
                let input = zeroize::Zeroizing::new(
                    serde_json::to_vec(&json!({"action":"terminal-prepare","proof":proof}))
                        .map_err(|_| "terminal dependency request unavailable")?,
                );
                let result = ssh::exec(&client, "yuji-manage", &input, 4096).await;
                let _ = client
                    .disconnect(
                        russh::Disconnect::ByApplication,
                        "terminal dependency ready",
                        "",
                    )
                    .await;
                if !result.is_ok_and(|r| r.lines().any(|v| v.trim() == r#"{"ok":true}"#)) {
                    return Err("Agent 已更新，tmux 未就绪；请重新部署 Agent 或在服务器安装 tmux");
                }
            }
            Ok(())
        }
        _ => Err("远端未确认完成；失败配置会回退，请检查 SSH、证书和 Agent 状态"),
    }
}
pub async fn bootstrap(app: &App, node: &Node) -> Result<(), &'static str> {
    let (secret, link) = {
        let mut i = app.lock();
        let secret =
            deploy::ensure_secret(app, &mut i, &node.public.id).map_err(|_| "无法准备节点凭据")?;
        let link = i
            .agents
            .get(&node.public.id)
            .cloned()
            .ok_or("旧 Agent 不在线，请使用 SSH 密码重新部署")?;
        let mut data = i.data.clone();
        data.secrets.insert(node.public.id.clone(), secret.clone());
        app.save_data(&mut i, data)
            .map_err(|_| "节点凭据保存失败")?;
        (secret, link)
    };
    let client = ssh::agent_client(app, link, &node.public.id, &node.username, &secret).await?;
    let arch = match node.public.arch.as_str() {
        "amd64" | "x86_64" => "amd64",
        "arm64" | "aarch64" => "arm64",
        _ => return Err("节点架构未知"),
    };
    let bytes = deploy::fetch(app, arch).await?;
    let manifest = deploy::get(app, "stable.json", 16384).await?;
    let token = app.unseal(&format!("{}:token", node.public.id), &secret.token)?;
    let config=serde_json::to_vec(&json!({"node_id":node.public.id,"token":std::str::from_utf8(&token).map_err(|_|"节点凭据不可用")?,"ssh_port":node.port,"controller_url":app.0.origin.replacen("https://","wss://",1)+"/api/agent"})).map_err(|_|"配置生成失败")?;
    let archive = deploy::installation(
        &bytes,
        &config,
        &secret.public_key,
        &secret.recovery_public,
        &manifest,
        &node.username,
    )?;
    // Detach installer: restarting the old Agent disconnects this very SSH tunnel.
    // The fixed script only consumes a root-owned temporary archive, not browser input.
    let prefix = if node.username == "root" {
        ""
    } else {
        "sudo -n "
    };
    let command = format!(
        "{prefix}sh -c 'set -eu; umask 077; d=$(mktemp -d /var/tmp/vistart-upgrade.XXXXXXXX); tar -xzf - -C \"$d\"; chmod 700 \"$d\"; nohup sh -c '\\''cd \"$1\"; sh ./install.sh >install.log 2>&1; status=$?; rm -rf -- \"$1\"; exit \"$status\"'\\'' sh \"$d\" </dev/null >/dev/null 2>&1 & echo prepared'"
    );
    ssh::exec(&client, &command, &archive, 4096)
        .await
        .map_err(|_| "SSH 安装失败，请确认已有 SSH 账户具备 root 或免密 sudo 安装权限")?;
    let started = now();
    for _ in 0..65 {
        tokio::time::sleep(Duration::from_secs(1)).await;
        if app.lock().data.nodes.iter().any(|n| {
            n.public.id == node.public.id
                && n.agent_version == deploy::AGENT_VERSION
                && n.last_seen > started
        }) {
            return Ok(());
        }
    }
    Err("Agent 未确认升级，请检查系统安装日志")
}
