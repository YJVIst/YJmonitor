//! Private server inventory, observed connection health and durable bounded upgrade requests.
use crate::{core::*, migration, model::*, nodes, operations};
use chrono::DateTime;
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

pub const MAX_BATCH: usize = MAX_NODES;
pub fn busy(t: &migration::Transfer) -> bool {
    matches!(t.state.as_str(), "pending" | "running" | "retry")
}
pub fn upgrade_blocked(i: &Inner, n: &Node) -> Option<&'static str> {
    if n.removing {
        return Some("服务器正在移除");
    }
    if i.ops.transfers.get(&n.public.id).is_some_and(busy)
        || i.jobs
            .values()
            .any(|j| j.node_id == n.public.id && j.state == "running")
    {
        return Some("已有部署或管理任务");
    }
    if i.enrollments
        .values()
        .any(|e| e.node == n.public.id && e.claimed && e.expires >= now())
    {
        return Some("一次性接入正在处理");
    }
    if n.demo || n.public.pending || !n.public.online {
        return Some("请等待 Agent 在线后升级");
    }
    if !i.data.secrets.contains_key(&n.public.id) {
        return Some("节点尚未部署");
    }
    if !matches!(
        n.public.arch.as_str(),
        "amd64" | "x86_64" | "arm64" | "aarch64"
    ) {
        return Some("节点架构尚未识别或不支持");
    }
    if !crate::backup::valid_version(&n.agent_version) {
        return Some("等待 Agent 上报有效版本");
    }
    if !crate::backup::newer_release(crate::deploy::AGENT_VERSION, &n.agent_version) {
        return Some("无需升级：当前版本已达到或高于配套版本");
    }
    None
}
fn error_label(reason: &str) -> &'static str {
    match reason {
        "metrics timeout" => "监控上报超时（15 秒未收到有效数据）",
        "agent connection closed" => "Agent 的 WSS 连接已关闭",
        "control writer unavailable" => "控制消息发送通道不可用",
        "agent traffic limit" => "Agent 消息速率超出限制",
        "invalid metrics" | "metrics missing" => "监控数据格式或序列校验失败",
        "unsupported agent message" => "Agent 消息类型不受支持",
        "node removed" => "服务器记录已移除",
        _ => "Agent 通讯校验或传输中断",
    }
}
pub fn list(i: &Inner) -> Vec<Value> {
    // One scan of the bounded audit ring, rather than a scan for every node.
    let mut disconnected = HashMap::new();
    let mut errors = HashMap::new();
    for e in i.audit.iter().rev() {
        let Ok(at) = DateTime::parse_from_rfc3339(&e.at) else {
            continue;
        };
        if e.action == "agent_disconnected" {
            disconnected
                .entry(e.subject.as_str())
                .or_insert(at.timestamp());
        } else if e.action == "agent_connection_error"
            && let Some((id, reason)) = e.subject.split_once(" · ")
        {
            errors
                .entry(id)
                .or_insert((at.timestamp(), error_label(reason)));
        }
    }
    let at = now();
    i.data
        .nodes
        .iter()
        .map(|n| {
            let mut v = nodes::admin_node(n);
            if n.ip.parse::<std::net::IpAddr>().is_err() {
                v["resolvedIPs"] =
                    json!(i.ssh_dns.get(&n.ip).map(Vec::as_slice).unwrap_or_default());
            }
            let observed = disconnected
                .get(n.public.id.as_str())
                .copied()
                .filter(|t| *t >= n.last_seen);
            let since = if n.public.online || n.public.pending {
                None
            } else {
                observed.or_else(|| (n.last_seen > 0).then_some((n.last_seen + 15).min(at)))
            };
            let last_error = errors.get(n.public.id.as_str());
            v["health"] = json!({
                "checkedAt":at,"lastReportAt":n.last_seen,
                "offlineSince":since,"offlineEstimated":since.is_some() && observed.is_none(),
                "lastErrorAt":last_error.map(|e|e.0),
                "lastError":last_error.map(|e|e.1),
                "hasLink":i.agents.contains_key(&n.public.id)
            });
            v["management"] = i.ops.transfers.get(&n.public.id).map_or(Value::Null, |t| {
                json!({
                    "taskId":t.task_id,"batchId":t.batch_id,"action":t.action,"state":t.state,
                    "message":t.message,"updatedAt":t.updated_at,"nextAttempt":t.next
                })
            });
            let blocked = upgrade_blocked(i, n);
            v["canUpgrade"] = json!(blocked.is_none());
            v["upgradeBlocked"] = json!(blocked);
            v["targetAgentVersion"] = json!(crate::deploy::AGENT_VERSION);
            v
        })
        .collect()
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BatchInput {
    ids: Vec<String>,
}
fn validate_ids(ids: &[String]) -> ApiResult<()> {
    let unique: HashSet<_> = ids.iter().collect();
    if ids.is_empty()
        || ids.len() > MAX_BATCH
        || unique.len() != ids.len()
        || ids.iter().any(|id| !operations::valid_id(id))
    {
        return Err(ApiError::new(400, "请选择 1 至 1000 台服务器，且不能重复"));
    }
    Ok(())
}
pub fn batch_update(app: App, c: Context, body: Vec<u8>) -> ApiResult<ApiReply> {
    let mut i = app.lock();
    app.guard(&mut i, &c, true, c.method != "GET")?;
    if c.method != "POST" {
        return Err(ApiError::new(405, "请求方式不支持"));
    }
    let v: BatchInput = decode(&body)?;
    validate_ids(&v.ids)?;
    i.request("batch-agent-update", 30)?;
    let batch_id = token();
    let before = i.ops.transfers.clone();
    let mut accepted = Vec::new();
    let mut rejected = Vec::new();
    for id in &v.ids {
        let Some(n) = i.data.nodes.iter().find(|n| &n.public.id == id) else {
            rejected.push(json!({"id":id,"message":"服务器不存在"}));
            continue;
        };
        if let Some(message) = upgrade_blocked(&i, n) {
            rejected.push(json!({"id":id,"message":message}));
            continue;
        }
        let old = i
            .data
            .secrets
            .get(id)
            .cloned()
            .expect("eligibility checked");
        i.ops.transfers.insert(
            id.clone(),
            migration::Transfer {
                action: "upgrade".into(),
                target: app.0.origin.clone(),
                state: "pending".into(),
                message: "等待升级".into(),
                task_id: token(),
                batch_id: batch_id.clone(),
                updated_at: now(),
                next: now(),
                old,
            },
        );
        accepted.push(id.clone());
    }
    if !accepted.is_empty() {
        if let Err(e) = operations::save(&app, &i) {
            i.ops.transfers = before;
            return Err(e);
        }
        app.record(
            &mut i,
            "agents_batch_update_requested",
            &format!("{} 台 · {}", accepted.len(), batch_id),
        );
    }
    Ok(ApiReply::accepted(
        json!({"batchId":batch_id,"accepted":accepted,"rejected":rejected,
        "concurrency":2,"agentVersion":crate::deploy::AGENT_VERSION}),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture {
        app: App,
        session: Session,
    }
    impl Fixture {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("probe-batch-{}", token()));
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
            let app = App::new(dir, "https://batch.test".into(), false).unwrap();
            let session = app.lock().new_session("", true).unwrap();
            app.lock().sessions.get_mut(&session.id).unwrap().elevated = now();
            for num in 0..4 {
                let id = format!("node-{num}");
                let mut n = Node::default();
                n.public.id = id.clone();
                n.public.name = id.clone();
                n.public.online = true;
                n.public.arch = "x86_64".into();
                n.last_seen = now();
                n.agent_version = "0.2.2".into();
                app.lock().data.nodes.push(n);
                app.lock().data.secrets.insert(
                    id,
                    NodeSecret {
                        token: "private-fixture".into(),
                        ..Default::default()
                    },
                );
            }
            Self { app, session }
        }
        fn context(&self) -> Context {
            let mut h = axum::http::HeaderMap::new();
            h.insert(
                "Cookie",
                format!("{COOKIE}={}", self.session.id).parse().unwrap(),
            );
            h.insert("Origin", "https://batch.test".parse().unwrap());
            h.insert("X-CSRF-Token", self.session.csrf.parse().unwrap());
            Context::new(
                "POST",
                "/api/admin/ops/agents/batch-update",
                h,
                "127.0.0.1:9999".parse().unwrap(),
                &self.app.0.origin,
            )
        }
        fn submit(&self, ids: Value) -> ApiResult<ApiReply> {
            batch_update(
                self.app.clone(),
                self.context(),
                serde_json::to_vec(&json!({"ids":ids})).unwrap(),
            )
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.app.0.dir);
        }
    }
    #[test]
    fn batch_requires_admin_csrf_but_no_second_password() {
        let f = Fixture::new();
        let body = br#"{"ids":["node-0"]}"#.to_vec();
        let mut c = f.context();
        c.sid.clear();
        assert_eq!(
            batch_update(f.app.clone(), c, body.clone())
                .err()
                .unwrap()
                .status,
            401
        );
        let mut c = f.context();
        c.headers.remove("X-CSRF-Token");
        assert_eq!(
            batch_update(f.app.clone(), c, body.clone())
                .err()
                .unwrap()
                .status,
            403
        );
        f.app
            .lock()
            .sessions
            .get_mut(&f.session.id)
            .unwrap()
            .elevated = 0;
        assert_eq!(
            f.submit(json!(["node-0"])).unwrap().value["accepted"],
            json!(["node-0"])
        );
    }
    #[test]
    fn strict_batch_limits_and_unknown_fields() {
        let f = Fixture::new();
        for ids in [
            json!([]),
            json!(["node-0", "node-0"]),
            json!(["../node"]),
            json!(vec!["n"; MAX_BATCH + 1]),
        ] {
            assert_eq!(f.submit(ids).err().unwrap().status, 400);
        }
        assert!(
            decode::<BatchInput>(br#"{"ids":["node-0"],"url":"https://evil.example"}"#).is_err()
        );
        assert!(f.app.lock().ops.transfers.is_empty());
    }
    #[test]
    fn partial_batch_is_durable_and_duplicate_requests_do_not_restart_tasks() {
        let f = Fixture::new();
        f.app.lock().data.nodes[1].public.online = false;
        f.app.lock().data.nodes[2].removing = true;
        let r = f
            .submit(json!(["node-0", "node-1", "node-2", "missing"]))
            .unwrap();
        assert_eq!(r.value["accepted"], json!(["node-0"]));
        assert_eq!(r.value["rejected"].as_array().unwrap().len(), 3);
        let task = f.app.lock().ops.transfers["node-0"].task_id.clone();
        let r = f.submit(json!(["node-0"])).unwrap();
        assert_eq!(r.value["accepted"], json!([]));
        assert_eq!(f.app.lock().ops.transfers["node-0"].task_id, task);
        let stored = operations::load(&f.app.0.dir).unwrap();
        assert_eq!(stored.transfers["node-0"].task_id, task);
        assert!(!stored.transfers["node-0"].batch_id.is_empty());
        let private = serde_json::to_string(&list(&f.app.lock())).unwrap();
        assert!(!private.contains("private-fixture"));
        assert!(!private.contains("recovery_key"));
    }
    #[test]
    fn failed_batch_can_be_retried_but_migration_cannot_be_overwritten() {
        let f = Fixture::new();
        f.submit(json!(["node-0", "node-1"])).unwrap();
        {
            let mut i = f.app.lock();
            i.ops.transfers.get_mut("node-0").unwrap().state = "failed".into();
            let t = i.ops.transfers.get_mut("node-1").unwrap();
            t.state = "retry".into();
            t.action = "reconfigure".into();
        }
        let r = f.submit(json!(["node-0", "node-1"])).unwrap();
        assert_eq!(r.value["accepted"], json!(["node-0"]));
        assert_eq!(f.app.lock().ops.transfers["node-1"].action, "reconfigure");
    }
    #[test]
    fn persistence_failure_does_not_enqueue_memory_only_jobs() {
        let f = Fixture::new();
        std::fs::create_dir(f.app.0.dir.join("operations.json")).unwrap();
        assert_eq!(f.submit(json!(["node-0"])).err().unwrap().status, 500);
        assert!(f.app.lock().ops.transfers.is_empty());
    }
    #[test]
    fn health_uses_observed_disconnect_and_redacted_known_errors() {
        let f = Fixture::new();
        let at = now();
        {
            let mut i = f.app.lock();
            i.data.nodes[0].public.online = false;
            i.data.nodes[0].last_seen = at - 60;
            for (action, subject) in [
                ("agent_disconnected", "node-0"),
                ("agent_connection_error", "node-0 · metrics timeout"),
                ("agent_connection_error", "node-1 · credential-secret"),
            ] {
                i.audit.push(Audit {
                    at: chrono::DateTime::from_timestamp(at - 45, 0)
                        .unwrap()
                        .to_rfc3339(),
                    action: action.into(),
                    subject: subject.into(),
                    ..Default::default()
                });
            }
        }
        let rows = list(&f.app.lock());
        assert_eq!(rows[0]["health"]["offlineSince"], at - 45);
        assert_eq!(rows[0]["health"]["offlineEstimated"], false);
        assert_eq!(rows[1]["health"]["offlineSince"], Value::Null);
        assert!(
            !serde_json::to_string(&rows)
                .unwrap()
                .contains("credential-secret")
        );
        assert!(
            rows[0]["health"]["lastError"]
                .as_str()
                .unwrap()
                .contains("15 秒")
        );
    }
    #[tokio::test]
    async fn queue_shares_two_slots_and_fails_closed_when_node_goes_offline() {
        let f = Fixture::new();
        f.submit(json!(["node-0", "node-1", "node-2", "node-3"]))
            .unwrap();
        // Occupied deploy slots prevent any management operation from starting.
        let slot = f
            .app
            .0
            .deploy_slots
            .clone()
            .acquire_many_owned(2)
            .await
            .unwrap();
        migration::tick(&f.app).await;
        assert!(
            f.app
                .lock()
                .ops
                .transfers
                .values()
                .all(|t| t.state == "pending")
        );
        drop(slot);
        // No await between starts and assertion: tasks are spawned but not polled.
        migration::tick(&f.app).await;
        assert_eq!(
            f.app
                .lock()
                .ops
                .transfers
                .values()
                .filter(|t| t.state == "running")
                .count(),
            2
        );
        assert_eq!(f.app.0.deploy_slots.available_permits(), 0);
        for n in &mut f.app.lock().data.nodes {
            n.public.online = false;
        }
        for _ in 0..20 {
            tokio::task::yield_now().await;
            if f.app.0.deploy_slots.available_permits() == 2 {
                break;
            }
        }
        assert_eq!(
            f.app
                .lock()
                .ops
                .transfers
                .values()
                .filter(|t| t.state == "failed")
                .count(),
            2
        );
        assert_eq!(
            f.app
                .lock()
                .ops
                .transfers
                .values()
                .filter(|t| t.state == "pending")
                .count(),
            2
        );
        assert_eq!(f.app.0.deploy_slots.available_permits(), 2);
    }
    #[test]
    fn restart_resumes_inflight_batch_without_restarting_completed_jobs() {
        let f = Fixture::new();
        f.submit(json!(["node-0", "node-1"])).unwrap();
        {
            let mut i = f.app.lock();
            i.ops.transfers.get_mut("node-0").unwrap().state = "running".into();
            i.ops.transfers.get_mut("node-1").unwrap().state = "done".into();
            operations::save(&f.app, &i).unwrap();
        }
        let stored = operations::load(&f.app.0.dir).unwrap();
        assert_eq!(stored.transfers["node-0"].state, "pending");
        assert_eq!(stored.transfers["node-1"].state, "done");
    }
    #[test]
    fn disconnected_removal_can_finish_while_its_worker_is_still_running() {
        let f = Fixture::new();
        let c = f.context();
        let mut i = f.app.lock();
        i.data.nodes[0].public.online = false;
        i.data.nodes[0].removing = true;
        i.ops.transfers.insert(
            "node-0".into(),
            migration::Transfer {
                action: "remove".into(),
                state: "running".into(),
                ..Default::default()
            },
        );
        for id in ["node-0", "node-1", "node-0"] {
            assert!(nodes::delete_node(&f.app, &mut i, &c, id, b"").is_ok());
        }
        assert!(!i.ops.transfers.contains_key("node-0"));
        assert_eq!(i.data.nodes.len(), 2);
        assert!(
            i.data
                .nodes
                .iter()
                .all(|n| n.public.id != "node-0" && n.public.id != "node-1")
        );
    }
    #[test]
    fn deployment_and_removal_cannot_race_an_executing_upgrade() {
        let f = Fixture::new();
        f.submit(json!(["node-0"])).unwrap();
        let c = f.context();
        let result = crate::deploy::begin(
            &f.app,
            &mut f.app.lock(),
            &c,
            br#"{"id":"node-0","password":"fixture"}"#,
        );
        assert_eq!(result.err().unwrap().status, 409);
        f.app.lock().ops.transfers.get_mut("node-0").unwrap().state = "running".into();
        let result = nodes::delete_node(&f.app, &mut f.app.lock(), &c, "node-0", b"");
        assert_eq!(result.err().unwrap().status, 409);
        assert_eq!(f.app.lock().data.nodes.len(), 4);
    }
    #[tokio::test]
    async fn queued_upgrade_waits_for_an_existing_deployment() {
        let f = Fixture::new();
        f.submit(json!(["node-0"])).unwrap();
        f.app.lock().jobs.insert(
            "existing".into(),
            DeployJob {
                id: "existing".into(),
                node_id: "node-0".into(),
                state: "running".into(),
                ..Default::default()
            },
        );
        migration::tick(&f.app).await;
        assert_eq!(f.app.lock().ops.transfers["node-0"].state, "pending");
        assert_eq!(f.app.0.deploy_slots.available_permits(), 2);
    }
    #[test]
    fn thousand_node_state_loads_and_add_limit_rejects_the_next_node() {
        let f = Fixture::new();
        {
            let mut i = f.app.lock();
            i.data.nodes.clear();
            for num in 0..MAX_NODES {
                let mut n = Node::default();
                n.public.id = format!("capacity-{num}");
                n.public.name = format!("Node {num}");
                n.ip = format!("10.0.{}.{}", num / 250, num % 250 + 1);
                n.port = 22;
                n.username = "root".into();
                i.data.nodes.push(n);
            }
            atomic_json(&f.app.0.dir.join("nodes.json"), &i.data).unwrap();
        }
        {
            let i = f.app.lock();
            let mut ops = operations::State::default();
            for n in &i.data.nodes {
                ops.transfers.insert(
                    n.public.id.clone(),
                    migration::Transfer {
                        action: "upgrade".into(),
                        state: "pending".into(),
                        ..Default::default()
                    },
                );
            }
            ops.transfers.insert(
                "old-removed".into(),
                migration::Transfer {
                    action: "remove".into(),
                    state: "done".into(),
                    ..Default::default()
                },
            );
            ops.retired
                .insert("old-removed".into(), NodeSecret::default());
            ops.removal_names
                .insert("old-removed".into(), "Removed server".into());
            atomic_json(&f.app.0.dir.join("operations.json"), &ops).unwrap();
        }
        let loaded = App::new(f.app.0.dir.clone(), "https://batch.test".into(), false).unwrap();
        assert_eq!(loaded.lock().data.nodes.len(), MAX_NODES);
        {
            let i = loaded.lock();
            assert_eq!(i.ops.transfers.len(), MAX_NODES);
            assert!(i.ops.transfers.values().all(|t| t.state == "pending"));
            assert!(!i.ops.transfers.contains_key("old-removed"));
            assert!(i.ops.retired.is_empty() && i.ops.removal_names.is_empty());
        }

        let payload = serde_json::to_vec(
            &json!({"name":"Extra","ip":"192.0.2.99","port":22,"username":"root","code":"OTHER"}),
        )
        .unwrap();
        let e = nodes::save_node(&f.app, &mut f.app.lock(), &f.context(), "", &payload)
            .err()
            .unwrap();
        assert_eq!(e.status, 409);
        assert!(e.message.contains("1000"));
        let ids = (0..MAX_NODES)
            .map(|n| format!("node-{n}"))
            .collect::<Vec<_>>();
        assert!(validate_ids(&ids).is_ok());
        let mut too_many = ids;
        too_many.push("extra".into());
        assert!(validate_ids(&too_many).is_err());
    }
    #[test]
    fn thousand_authenticated_agents_share_nat_without_bypassing_bad_token_limits() {
        use sha2::{Digest, Sha256};
        let f = Fixture::new();
        let token = token();
        let digest = hex::encode(Sha256::digest(token.as_bytes()));
        let context = |id: &str, value: &str| {
            let mut h = axum::http::HeaderMap::new();
            h.insert("X-Probe-Node", id.parse().unwrap());
            h.insert("Authorization", format!("Bearer {value}").parse().unwrap());
            Context::new(
                "GET",
                "/api/agent",
                h,
                "127.0.0.1:9999".parse().unwrap(),
                &f.app.0.origin,
            )
        };
        for num in 0..MAX_NODES {
            let id = format!("nat-{num}");
            f.app.lock().data.secrets.insert(
                id.clone(),
                NodeSecret {
                    token_hash: digest.clone(),
                    ..Default::default()
                },
            );
            assert!(
                crate::realtime::authorize_agent(&mut f.app.lock(), &context(&id, &token)).is_ok()
            );
        }
        for _ in 0..120 {
            assert_eq!(
                crate::realtime::authorize_agent(&mut f.app.lock(), &context("missing", &token))
                    .err()
                    .unwrap()
                    .status,
                401
            );
        }
        assert_eq!(
            crate::realtime::authorize_agent(&mut f.app.lock(), &context("missing", &token))
                .err()
                .unwrap()
                .status,
            429
        );
        assert!(
            crate::realtime::authorize_agent(&mut f.app.lock(), &context("nat-0", &token)).is_ok()
        );
        for _ in 0..28 {
            assert!(
                crate::realtime::authorize_agent(&mut f.app.lock(), &context("nat-0", &token))
                    .is_ok()
            );
        }
        assert_eq!(
            crate::realtime::authorize_agent(&mut f.app.lock(), &context("nat-0", &token))
                .err()
                .unwrap()
                .status,
            429
        );
    }
    #[test]
    fn current_newer_unknown_agents_cannot_be_reinstalled() {
        let f = Fixture::new();
        for version in [crate::deploy::AGENT_VERSION, "9.0.0", "", "garbage"] {
            f.app.lock().data.nodes[0].agent_version = version.into();
            let result = f.submit(json!(["node-0"])).unwrap();
            assert!(result.value["accepted"].as_array().unwrap().is_empty());
            assert_eq!(result.value["rejected"].as_array().unwrap().len(), 1);
            assert!(f.app.lock().ops.transfers.is_empty());
            assert_eq!(
                migration::start_manage(&f.app, &mut f.app.lock(), "node-0", "upgrade")
                    .unwrap_err()
                    .status,
                409
            );
        }
    }
    #[tokio::test]
    async fn version_operations_keep_auth_csrf_without_second_password() {
        let f = Fixture::new();
        f.app
            .lock()
            .sessions
            .get_mut(&f.session.id)
            .unwrap()
            .elevated = 0;
        std::fs::write(f.app.0.dir.join("updater-enabled"), b"test").unwrap();
        for action in [
            "agent-update",
            "agent-rollback",
            "panel-update",
            "panel-rollback",
        ] {
            let mut c = f.context();
            c.path = format!("/api/admin/ops/{action}");
            let body = br#"{"id":"node-0","version":"99.0.0"}"#.to_vec();
            let mut missing = c.clone();
            missing.sid.clear();
            assert_eq!(
                operations::api(f.app.clone(), missing, body.clone())
                    .await
                    .err()
                    .unwrap()
                    .status,
                401
            );
            let mut bad_csrf = c.clone();
            bad_csrf.headers.remove("X-CSRF-Token");
            assert_eq!(
                operations::api(f.app.clone(), bad_csrf, body.clone())
                    .await
                    .err()
                    .unwrap()
                    .status,
                403
            );
            f.app.lock().updates.latest = "99.0.0".into();
            f.app.lock().updates.checked_at = now();
            assert_eq!(
                operations::api(f.app.clone(), c, body)
                    .await
                    .unwrap()
                    .status,
                202
            );
            f.app.lock().ops.transfers.clear();
            let _ = std::fs::remove_file(f.app.0.dir.join("update-request.json"));
        }
        for action in ["recovery", "sessions/revoke"] {
            let mut c = f.context();
            c.path = format!("/api/admin/ops/{action}");
            assert_eq!(
                operations::api(f.app.clone(), c, br#"{"id":"others"}"#.to_vec())
                    .await
                    .err()
                    .unwrap()
                    .status,
                428
            );
        }
    }
}
