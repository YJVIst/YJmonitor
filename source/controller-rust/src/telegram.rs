mod group;
use crate::{
    core::*,
    model::*,
    nodes::{renewal_day, renewal_due, zone},
};
use chrono::{DateTime, TimeZone};
pub use group::api as group_api;
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;
const LIMIT: usize = 256;
const LABEL: &str = "telegram-bot-token-v1";
pub fn valid_token(s: &str) -> bool {
    let Some((id, key)) = s.split_once(':') else {
        return false;
    };
    (5..=16).contains(&id.len())
        && id.bytes().all(|b| b.is_ascii_digit())
        && (20..=128).contains(&key.len())
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
}
fn valid_chat(s: &str) -> bool {
    let raw = s.strip_prefix('-').unwrap_or(s);
    !raw.starts_with('0')
        && raw.len() <= 16
        && !raw.is_empty()
        && raw.bytes().all(|b| b.is_ascii_digit())
        && s.parse::<i64>()
            .is_ok_and(|v| v != 0 && v > -(1i64 << 52) && v < 1i64 << 52)
}
fn save(app: &App, i: &mut Inner, next: TelegramState) -> bool {
    if atomic_json(&app.0.dir.join("telegram.json"), &next).is_err() {
        i.telegram_error = "通知状态保存失败，正在重试".into();
        return false;
    }
    i.telegram = next;
    i.telegram_error.clear();
    true
}
pub fn view(i: &Inner) -> Value {
    let s = &i.telegram;
    json!({"group":s.group,"enabled":s.config.enabled,"hasToken":!s.config.token.is_empty(),"chatId":s.config.chat_id,"notifyOnline":s.config.online,"notifyOffline":s.config.offline,"notifyRenewal":s.config.renewal,"notifyLogin":s.config.login,"offlineDelaySeconds":20,"pending":s.queue.len(),"lastSuccess":s.last_success,"lastError":if i.telegram_error.is_empty(){&s.last_error}else{&i.telegram_error},"test":s.test,"dropped":s.dropped})
}
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
struct Input {
    enabled: bool,
    token: String,
    clear_token: bool,
    chat_id: String,
    #[serde(rename = "notifyOnline")]
    online: bool,
    #[serde(rename = "notifyOffline")]
    offline: bool,
    #[serde(rename = "notifyRenewal")]
    renewal: Option<bool>,
    #[serde(rename = "notifyLogin")]
    login: Option<bool>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}
pub fn api(app: &App, i: &mut Inner, c: &Context, body: &[u8]) -> ApiResult<ApiReply> {
    app.guard(i, c, true, c.method != "GET")?;
    if c.path == "/api/admin/telegram" && c.method == "GET" {
        return Ok(ApiReply::ok(view(i)));
    }
    if c.path == "/api/admin/telegram/preview" && c.method == "GET" {
        return Ok(ApiReply::ok(previews(i)));
    }
    if c.path == "/api/admin/telegram" && c.method == "PUT" {
        let mut v: Input = decode(body)?;
        v.token = v.token.trim().into();
        v.chat_id = v.chat_id.trim().into();
        if (!v.token.is_empty() && !valid_token(&v.token)) || (v.clear_token && !v.token.is_empty())
        {
            return Err(ApiError::new(400, "Bot Token 格式不正确"));
        }
        if !v.chat_id.is_empty() && !valid_chat(&v.chat_id) {
            return Err(ApiError::new(
                400,
                "接收人 ID 需为有效数字，群组 ID 可为负数",
            ));
        }
        let mut next = i.telegram.clone();
        let mut encrypted = next.config.token.clone();
        if v.clear_token {
            encrypted.clear()
        }
        if !v.token.is_empty() {
            encrypted = app
                .seal(LABEL, v.token.as_bytes())
                .map_err(|_| ApiError::internal())?;
        }
        let renewal = v.renewal.unwrap_or(next.config.renewal);
        let login = v.login.unwrap_or(next.config.login);
        if v.enabled
            && (encrypted.is_empty()
                || v.chat_id.is_empty()
                || (!v.online && !v.offline && !renewal && !login))
        {
            return Err(ApiError::new(
                400,
                "请填写 Bot Token、接收人 ID，并选择通知事件",
            ));
        }
        if v.clear_token || !v.token.is_empty() {
            next.group.enabled = false;
            next.group.verified_at = 0;
            next.group.test = TelegramTestResult::default();
            next.group.last_error = "Bot Token 已修改，请重新验证并启用群组通知".into();
        }
        next.config = TelegramConfig {
            enabled: v.enabled,
            token: encrypted,
            chat_id: v.chat_id,
            online: v.online,
            offline: v.offline,
            renewal,
            login,
        };
        next.nodes = i
            .data
            .nodes
            .iter()
            .filter(|n| !n.demo)
            .map(|n| {
                (
                    n.public.id.clone(),
                    TelegramNodeState {
                        online: n.public.online,
                        offline_since: 0,
                        was_offline: !n.public.online,
                    },
                )
            })
            .collect();
        for e in &next.queue {
            if e.kind == "renewal" {
                for m in &e.renewal_nodes {
                    if next.renewals.get(&m.node_id) == Some(&m.renewal_version) {
                        next.renewals.remove(&m.node_id);
                    }
                }
            }
        }
        next.queue.clear();
        next.test = TelegramTestResult::default();
        next.group.test = TelegramTestResult::default();
        next.last_error.clear();
        if !save(app, i, next) {
            return Err(ApiError::internal());
        }
        if let Some((_, cancel)) = &i.delivery {
            cancel.cancel()
        }
        i.telegram_revision += 1;
        app.record(i, "telegram_updated", "Telegram 通知设置");
        return Ok(ApiReply::ok(view(i)));
    }
    if c.path == "/api/admin/telegram/test" && c.method == "POST" {
        let _: Empty = decode(body)?;
        if i.telegram.config.token.is_empty() || !valid_chat(&i.telegram.config.chat_id) {
            return Err(ApiError::new(400, "请先保存 Bot Token 和接收人 ID"));
        }
        if now() - i.telegram.last_test_at < 60 {
            return Err(ApiError::rate("每 60 秒可发送一次测试通知", 60));
        }
        if i.telegram.queue.len() >= LIMIT {
            return Err(ApiError::rate("通知队列已满，请稍后再试", 60));
        }
        let mut next = i.telegram.clone();
        let id = token()[..24].to_string();
        next.queue.push(TelegramEvent {
            id: id.clone(),
            site: i.data.site.name.clone(),
            kind: "test".into(),
            at: now(),
            ..Default::default()
        });
        next.last_test_at = now();
        next.test = TelegramTestResult {
            id: id.clone(),
            status: "pending".into(),
            error: String::new(),
        };
        if !save(app, i, next) {
            return Err(ApiError::internal());
        }
        app.record(i, "telegram_test", "测试通知");
        return Ok(ApiReply::accepted(json!({"id":id})));
    }
    Err(ApiError::new(405, "不支持的请求方式"))
}
fn member_current(i: &Inner, m: &RenewalMember) -> bool {
    i.telegram.config.enabled
        && i.telegram.config.renewal
        && i.data.nodes.iter().any(|n| {
            n.public.id == m.node_id
                && n.notify_renewal
                && !n.demo
                && n.renewal_version == m.renewal_version
                && n.expires_at == m.expires_at
        })
}
fn event_current(i: &Inner, e: &TelegramEvent) -> bool {
    if e.group
        && (!i.telegram.group.enabled || !matches!(e.kind.as_str(), "online" | "offline" | "test"))
    {
        return false;
    }
    if !e.group && e.kind != "test" && !i.telegram.config.enabled {
        return false;
    }
    match e.kind.as_str() {
        "online" | "offline" => i
            .data
            .nodes
            .iter()
            .any(|n| n.public.id == e.node_id && !n.removing),
        "renewal" => {
            !e.renewal_nodes.is_empty() && e.renewal_nodes.iter().all(|m| member_current(i, m))
        }
        "security" => i.telegram.config.login,
        "test" => true,
        _ => false,
    }
}
fn label(e: &mut TelegramEvent) {
    if let Some(first) = e.renewal_nodes.first() {
        e.node_id = first.node_id.clone();
        e.expires_at = first.expires_at.clone();
        e.renewal_version = first.renewal_version.clone();
        e.name = format!("{} 到期的服务器", e.renewal_day);
    }
}
pub fn invalidate(app: &App, i: &mut Inner) {
    if let Some((event, cancel)) = &i.delivery
        && !event_current(i, event)
    {
        cancel.cancel();
    }
    let mut next = i.telegram.clone();
    let mut changed = false;
    next.queue.retain_mut(|e| {
        if e.kind != "renewal" {
            let keep = event_current(i, e);
            changed |= !keep;
            return keep;
        }
        let old = e.renewal_nodes.len();
        e.renewal_nodes.retain(|m| member_current(i, m));
        if e.renewal_nodes.len() != old {
            changed = true;
            if e.renewal_nodes.is_empty() {
                return false;
            }
            e.id = token()[..24].into();
            e.attempts = 0;
            e.next_attempt = 0;
            label(e);
        }
        true
    });
    next.renewals.retain(|id, _| {
        let found = i.data.nodes.iter().any(|n| &n.public.id == id);
        changed |= !found;
        found
    });
    if changed {
        save(app, i, next);
    }
}
fn tick(app: &App, i: &mut Inner) {
    invalidate(app, i);
    i.expire_nodes();
    let t = now();
    if (!i.telegram.config.enabled && !i.telegram.group.enabled) || t < i.telegram_ready {
        return;
    }
    let mut next = i.telegram.clone();
    let mut changed = false;
    let mut present = std::collections::HashSet::new();
    for node in &i.data.nodes {
        if node.demo || node.removing {
            continue;
        }
        let id = &node.public.id;
        present.insert(id.clone());
        let known = next.nodes.contains_key(id);
        let mut state = next.nodes.get(id).cloned().unwrap_or_default();
        let before = (state.online, state.offline_since, state.was_offline);
        let recovery = state.was_offline;
        let mut kind = "";
        if node.public.online {
            if !state.online {
                kind = "online"
            }
            state = TelegramNodeState {
                online: true,
                offline_since: 0,
                was_offline: false,
            };
        } else if state.online {
            if state.offline_since == 0 {
                state.offline_since = t
            }
            if t - state.offline_since >= 20 {
                kind = "offline";
                state = TelegramNodeState {
                    was_offline: true,
                    ..Default::default()
                };
            }
        }
        changed |= !known || before != (state.online, state.offline_since, state.was_offline);
        next.nodes.insert(id.clone(), state);
        if !kind.is_empty() {
            let personal = next.config.enabled
                && ((kind == "online" && next.config.online)
                    || (kind == "offline" && next.config.offline));
            for is_group in [false, true] {
                if if is_group {
                    !next.group.enabled
                } else {
                    !personal
                } {
                    continue;
                }
                if next.queue.len() >= LIMIT {
                    next.dropped += 1;
                    if is_group {
                        next.group.last_error = "通知队列已满，部分事件未发送".into();
                    } else {
                        next.last_error = "通知队列已满，部分事件未发送".into();
                    }
                } else {
                    next.queue.push(TelegramEvent {
                        id: token()[..24].into(),
                        node_id: id.clone(),
                        name: node.public.name.clone(),
                        site: i.data.site.name.clone(),
                        kind: kind.into(),
                        group: is_group,
                        recovery,
                        next_attempt: t + 5,
                        at: t,
                        ..Default::default()
                    });
                }
                changed = true;
            }
        }
    }
    if next.config.enabled && next.config.renewal {
        for n in &i.data.nodes {
            if !renewal_due(n, t)
                || n.renewal_version.is_empty()
                || next.renewals.get(&n.public.id) == Some(&n.renewal_version)
            {
                continue;
            }
            let day = renewal_day(&n.expires_at);
            let index = next.queue.iter().position(|e| {
                e.kind == "renewal" && e.renewal_day == day && e.renewal_nodes.len() < MAX_NODES
            });
            let index = if let Some(k) = index {
                k
            } else {
                if next.queue.len() >= LIMIT {
                    continue;
                }
                next.queue.push(TelegramEvent {
                    id: token()[..24].into(),
                    site: i.data.site.name.clone(),
                    kind: "renewal".into(),
                    at: t,
                    renewal_day: day,
                    next_attempt: t + 5,
                    ..Default::default()
                });
                next.queue.len() - 1
            };
            let e = &mut next.queue[index];
            if let Some((active, cancel)) = &i.delivery
                && active.id == e.id
            {
                cancel.cancel();
                e.id = token()[..24].into();
                e.attempts = 0;
                e.next_attempt = t + 3;
            }
            e.renewal_nodes.push(RenewalMember {
                node_id: n.public.id.clone(),
                name: n.public.name.clone(),
                expires_at: n.expires_at.clone(),
                renewal_version: n.renewal_version.clone(),
                renewal_amount: n.renewal_amount.clone(),
                renewal_currency: n.renewal_currency.clone(),
            });
            label(e);
            next.renewals
                .insert(n.public.id.clone(), n.renewal_version.clone());
            changed = true;
        }
    }
    next.nodes.retain(|id, _| {
        let keep = present.contains(id);
        changed |= !keep;
        keep
    });
    next.queue.retain(|e| {
        let keep = (matches!(e.kind.as_str(), "test" | "renewal" | "security")
            || present.contains(&e.node_id))
            && event_current(i, e);
        changed |= !keep;
        keep
    });
    if changed {
        save(app, i, next);
    }
}
fn html(value: &str, limit: usize) -> String {
    // Only generated markup is trusted. Bound Unicode length before entity escaping.
    let clipped: String = value
        .chars()
        .take(limit)
        .filter(|c| !c.is_control())
        .collect();
    clipped
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
pub fn text(e: &TelegramEvent) -> String {
    let when = zone()
        .timestamp_opt(e.at, 0)
        .single()
        .map(|v| v.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_default();
    let site = html(&e.site, 60);
    let footer = format!("\n\n<i>{when} · UTC+8</i>");
    let mut message = match e.kind.as_str() {
        "test" if e.group => format!(
            "🔔 <b>群组通知测试</b>\n{site}\n\n机器人管理员权限已确认。本群仅接收服务器上线、离线和恢复通知。"
        ),
        "test" => format!("🔔 <b>通知测试</b>\n{site}\n\n通知已连接，后续提醒将发送到此会话。"),
        "security" => format!("🔐 <b>管理员登录</b>\n{site}\n\n{}", html(&e.name, 180)),
        "renewal" => {
            let mut out = format!(
                "📅 <b>服务器续费提醒</b>\n{site}\n\n到期日  <b>{}</b> · UTC+8\n待续费  <b>{} 台</b>\n",
                html(&e.renewal_day, 10),
                e.renewal_nodes.len()
            );
            for n in e.renewal_nodes.iter().take(20) {
                let at = DateTime::parse_from_rfc3339(&n.expires_at)
                    .map(|v| v.with_timezone(&zone()).format("%H:%M").to_string())
                    .unwrap_or_else(|_| "--:--".into());
                out.push_str(&format!(
                    "\n• <b>{}</b>  <code>{at}</code>",
                    html(&n.name, 45)
                ));
                if !n.renewal_amount.is_empty() {
                    out.push_str(&format!(
                        " · {}",
                        html(
                            &crate::billing::amount_label(&n.renewal_amount, &n.renewal_currency),
                            32
                        )
                    ));
                }
            }
            if e.renewal_nodes.len() > 20 {
                out.push_str(&format!(
                    "\n另有 {} 台，请在面板查看。",
                    e.renewal_nodes.len() - 20
                ));
            }
            let mut totals: std::collections::BTreeMap<&str, u64> =
                std::collections::BTreeMap::new();
            let mut unpriced = 0;
            for n in &e.renewal_nodes {
                if let Some(v) = crate::billing::minor(&n.renewal_amount, &n.renewal_currency) {
                    *totals.entry(&n.renewal_currency).or_default() += v;
                } else {
                    unpriced += 1;
                }
            }
            for (currency, total) in totals.iter().take(8) {
                out.push_str(&format!(
                    "\n{currency} 合计  <b>{}</b>",
                    crate::billing::decimal(*total, currency)
                ));
            }
            if totals.len() > 8 {
                out.push_str("\n更多币种请在面板查看。");
            }
            if unpriced > 0 {
                out.push_str(&format!("\n{unpriced} 台金额未设置"));
            }
            out.push_str("\n\n在面板确认已续费可按设置周期更新到期时间；选择不再续费可停止提醒。");
            out
        }
        "offline" => format!(
            "🔴 <b>服务器离线</b>\n{site}\n\n服务器  <b>{}</b>\n连续 20 秒未恢复连接，请检查服务器状态。",
            html(&e.name, 60)
        ),
        "online" if e.recovery => format!(
            "🟢 <b>服务器恢复</b>\n{site}\n\n服务器  <b>{}</b>\n连接已恢复，监控数据正在更新。",
            html(&e.name, 60)
        ),
        _ => format!(
            "🟢 <b>服务器上线</b>\n{site}\n\n服务器  <b>{}</b>\n已连接，监控数据正在更新。",
            html(&e.name, 60)
        ),
    };
    message.push_str(&footer);
    message
}
fn payload(chat: &str, text: &str, origin: &str) -> Value {
    json!({"chat_id":chat,"text":text,"parse_mode":"HTML","link_preview_options":{"is_disabled":true},
        "reply_markup":{"inline_keyboard":[[{"text":"打开面板","url":origin}]]}})
}
fn previews(i: &Inner) -> Value {
    let name = i
        .data
        .nodes
        .iter()
        .find(|n| !n.removing)
        .map(|n| n.public.name.as_str())
        .unwrap_or("示例服务器");
    let at = now();
    let expiry = zone().timestamp_opt(at + 3 * 86400, 0).single().unwrap();
    let rows: Vec<_> = [
        ("offline", "服务器离线"),
        ("online", "服务器上线"),
        ("renewal", "续费提醒"),
        ("security", "管理员登录"),
        ("test", "测试通知"),
    ]
    .into_iter()
    .map(|(kind, label)| {
        let e = TelegramEvent {
            kind: kind.into(),
            site: i.data.site.name.clone(),
            name: if kind == "security" {
                "登录来源  192.0.2.1".into()
            } else {
                name.into()
            },
            at,
            renewal_day: expiry.format("%Y-%m-%d").to_string(),
            renewal_nodes: vec![RenewalMember {
                name: name.into(),
                expires_at: expiry.to_rfc3339(),
                ..Default::default()
            }],
            ..Default::default()
        };
        json!({"kind":kind,"label":label,"html":text(&e)})
    })
    .collect();
    json!({"previews":rows})
}
#[derive(Default)]
struct Delivery {
    error: String,
    retry: bool,
    wait: i64,
}
async fn send(app: &App, secret: &str, chat: &str, text: &str) -> Delivery {
    let call = async {
        let mut r = app
            .0
            .http
            .post(format!("https://api.telegram.org/bot{secret}/sendMessage"))
            .timeout(Duration::from_secs(10))
            .json(&payload(chat, text, &app.0.origin))
            .send()
            .await
            .map_err(|_| ())?;
        let status = r.status().as_u16();
        let mut bytes = Vec::new();
        while let Some(c) = r.chunk().await.map_err(|_| ())? {
            if bytes.len() + c.len() > 16384 {
                return Ok::<_, ()>(Delivery {
                    error: "Telegram 服务响应异常".into(),
                    retry: true,
                    wait: 0,
                });
            }
            bytes.extend(c);
        }
        let value: Value = serde_json::from_slice(&bytes).unwrap_or_default();
        if status == 200 && value["ok"] == true {
            return Ok(Delivery::default());
        }
        let code = value["error_code"]
            .as_u64()
            .filter(|v| (400..=599).contains(v))
            .unwrap_or(status as u64);
        let (error, retry, wait) = match code {
            400 => (
                "Telegram 无法找到接收人，请核对 ID 并先向 Bot 发送 /start",
                false,
                0,
            ),
            401 | 404 => ("Bot Token 无效，请检查后重新保存", false, 0),
            403 => (
                "Bot 无权发送消息，请先发送 /start，或检查群组权限与屏蔽状态",
                false,
                0,
            ),
            429 => (
                "Telegram 暂时限流，稍后重试",
                true,
                value["parameters"]["retry_after"]
                    .as_i64()
                    .unwrap_or(3)
                    .clamp(3, 86400),
            ),
            _ => ("Telegram 服务响应异常", code >= 500 || code == 200, 0),
        };
        Ok(Delivery {
            error: error.into(),
            retry,
            wait,
        })
    };
    call.await.unwrap_or_else(|_| Delivery {
        error: "Telegram 连接失败或超时".into(),
        retry: true,
        wait: 0,
    })
}
fn due_index(queue: &[TelegramEvent], at: i64) -> Option<usize> {
    let mut seen = [false; 2];
    queue.iter().position(|e| {
        let n = usize::from(e.group);
        if seen[n] {
            return false;
        }
        seen[n] = true;
        e.next_attempt <= at
    })
}
async fn dispatch(app: &App) {
    let chosen = {
        let mut i = app.lock();
        let t = now();
        if i.telegram.queue.is_empty() || t < i.telegram_next {
            return;
        }
        invalidate(app, &mut i);
        let Some(index) = due_index(&i.telegram.queue, t) else {
            return;
        };
        let mut next = i.telegram.clone();
        next.queue[index].attempts += 1;
        next.queue[index].next_attempt = t + 30;
        if !save(app, &mut i, next) {
            return;
        }
        let event = i.telegram.queue[index].clone();
        let secret = app.unseal(LABEL, &i.telegram.config.token);
        let revision = i.telegram_revision;
        let chat = if event.group {
            i.telegram.group.chat_id.clone()
        } else {
            i.telegram.config.chat_id.clone()
        };
        i.telegram_next = t + 3;
        let cancel = app.0.stop.child_token();
        i.delivery = Some((event.clone(), cancel.clone()));
        (event, secret, revision, chat, cancel)
    };
    let (event, secret, revision, chat, cancel) = chosen;
    let result = if let Ok(raw) = secret {
        if event.attempts > 5 || now() - event.at > 86400 {
            Delivery {
                error: "通知已超过重试次数或有效期".into(),
                ..Default::default()
            }
        } else if let Ok(secret) = std::str::from_utf8(&raw) {
            let message = text(&event);
            tokio::select! {_=cancel.cancelled()=>{let mut i=app.lock();if i.delivery.as_ref().is_some_and(|(e,_)|e.id==event.id){i.delivery=None;}return},r=async { if event.group && let Err(error)=group::verify(app,secret,&chat).await { return Delivery { error:error.message, retry: error.status>=500, wait:30 }; } send(app,secret,&chat,&message).await }=>r}
        } else {
            Delivery {
                error: "通知密钥不可用，请重新保存 Bot Token".into(),
                ..Default::default()
            }
        }
    } else {
        Delivery {
            error: "通知密钥不可用，请重新保存 Bot Token".into(),
            ..Default::default()
        }
    };
    let mut i = app.lock();
    if i.delivery.as_ref().is_some_and(|(e, _)| e.id == event.id) {
        i.delivery = None;
    }
    if i.telegram_revision != revision
        || !i.telegram.queue.iter().any(|e| e.id == event.id)
        || !event_current(&i, &event)
    {
        return;
    }
    let mut next = i.telegram.clone();
    let Some(index) = next.queue.iter().position(|e| e.id == event.id) else {
        return;
    };
    if event.group {
        next.group.last_error = result.error.clone();
    } else {
        next.last_error = result.error.clone();
    }
    let finished = result.error.is_empty() || !result.retry || event.attempts >= 5;
    if result.error.is_empty() {
        if event.group {
            next.group.last_success = now();
            next.group.verified_at = now();
        } else {
            next.last_success = now();
        }
    }
    if finished {
        next.queue.remove(index);
        let test = if event.group {
            &mut next.group.test
        } else {
            &mut next.test
        };
        if event.kind == "test" && test.id == event.id {
            test.status = if result.error.is_empty() {
                "sent"
            } else {
                "failed"
            }
            .into();
            test.error = result.error.clone();
        }
    } else {
        next.queue[index].next_attempt = now() + result.wait.max(1i64 << event.attempts);
    }
    if save(app, &mut i, next) && finished {
        let name = if event.kind == "test" {
            "测试通知"
        } else {
            &event.name
        };
        let subject = if result.error.is_empty() {
            name.to_string()
        } else {
            format!("{name}：{}", result.error)
        };
        app.record(
            &mut i,
            if result.error.is_empty() {
                "telegram_sent"
            } else {
                "telegram_failed"
            },
            &subject,
        );
    }
}
pub fn start(app: &App) {
    let a = app.clone();
    tokio::spawn(async move {
        let mut clock = tokio::time::interval(Duration::from_secs(1));
        clock.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {_=a.0.stop.cancelled()=>break,_=clock.tick()=>tick(&a,&mut a.lock())}
        }
    });
    let a = app.clone();
    tokio::spawn(async move {
        loop {
            tokio::select! {_=a.0.stop.cancelled()=>break,_=tokio::time::sleep(Duration::from_secs(1))=>dispatch(&a).await}
        }
    });
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn credential_syntax() {
        assert!(valid_token("123456:abcdefghijklmnopqrstuvwxy"));
        assert!(!valid_token("123456:abc/../../path"));
        assert!(valid_chat("-100123456789"));
        assert!(!valid_chat("00123"));
        assert!(!valid_chat("4503599627370496"));
    }
    #[test]
    fn limited_renewal_summary() {
        let e = TelegramEvent {
            kind: "renewal".into(),
            site: "Test".into(),
            renewal_nodes: (0..30)
                .map(|n| RenewalMember {
                    name: format!("node {n}"),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        let s = text(&e);
        assert!(s.contains("另有 10 台"));
        assert!(!s.contains("node 20"));
    }
}

#[cfg(test)]
mod queue_tests {
    use super::*;
    #[tokio::test]
    async fn online_offline_and_grouped_renewal_persistence_without_network() {
        let dir = std::env::temp_dir().join(format!("probe-rust-notification-test-{}", token()));
        std::fs::create_dir(&dir).unwrap();
        let make_node = |id: &str| Node {
            public: PublicNode {
                id: id.into(),
                name: id.into(),
                online: true,
                ..Default::default()
            },
            last_seen: now(),
            notify_renewal: true,
            expires_at: (Utc::now() + chrono::Duration::days(1))
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            renewal_version: token(),
            ..Default::default()
        };
        let data = Data {
            schema: 2,
            nodes: vec![make_node("one"), make_node("two")],
            site: Site {
                name: "QA".into(),
                public: true,
            },
            ..Default::default()
        };
        atomic_json(&dir.join("auth.json"), &Auth::default()).unwrap();
        atomic_json(&dir.join("nodes.json"), &data).unwrap();
        let app = App::new(dir.clone(), "https://example.invalid".into(), true).unwrap();
        {
            let mut i = app.lock();
            i.telegram_ready = 0;
            i.telegram.config.enabled = true;
            i.telegram.config.renewal = false;
            i.data.nodes[0].last_seen = now() - 60;
            i.telegram.nodes.insert(
                "one".into(),
                TelegramNodeState {
                    online: true,
                    offline_since: now() - 21,
                    was_offline: false,
                },
            );
            tick(&app, &mut i);
            assert_eq!(i.telegram.queue.len(), 1);
            assert_eq!(i.telegram.queue[0].kind, "offline");
            i.data.nodes[0].last_seen = now();
            i.data.nodes[0].public.online = true;
            tick(&app, &mut i);
            assert_eq!(i.telegram.queue.len(), 2);
            assert_eq!(i.telegram.queue[1].kind, "online");
            i.telegram.queue.clear();
            i.telegram.config.token = "test-only-no-network".into();
            queue_notice(&app, &mut i, "Admin logged in", "security");
            assert!(i.telegram.queue.is_empty());
            i.telegram.config.login = true;
            queue_notice(&app, &mut i, "Admin logged in", "security");
            tick(&app, &mut i);
            assert_eq!(i.telegram.queue.len(), 1);
            assert_eq!(i.telegram.queue[0].kind, "security");
            i.telegram.queue.push(TelegramEvent {
                kind: "resource".into(),
                ..Default::default()
            });
            invalidate(&app, &mut i);
            assert_eq!(i.telegram.queue.len(), 1);
            i.telegram.config.login = false;
            invalidate(&app, &mut i);
            assert!(i.telegram.queue.is_empty());
            i.telegram.config.renewal = true;
            tick(&app, &mut i);
            assert_eq!(i.telegram.queue.len(), 1);
            assert_eq!(i.telegram.queue[0].renewal_nodes.len(), 2);
            let cancel = tokio_util::sync::CancellationToken::new();
            let event = i.telegram.queue[0].clone();
            i.delivery = Some((event.clone(), cancel.clone()));
            i.data.nodes[0].notify_renewal = false;
            invalidate(&app, &mut i);
            assert!(cancel.is_cancelled());
            assert_eq!(i.telegram.queue[0].renewal_nodes.len(), 1);
            assert_ne!(i.telegram.queue[0].id, event.id);
            i.data.nodes[1].expires_at = (Utc::now() + chrono::Duration::days(31))
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
            i.data.nodes[1].renewal_version = token();
            invalidate(&app, &mut i);
            assert!(i.telegram.queue.is_empty());
            i.data.nodes[1].expires_at = (Utc::now() + chrono::Duration::days(1))
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
            tick(&app, &mut i);
            tick(&app, &mut i);
            assert_eq!(i.telegram.queue.len(), 1);
            assert_eq!(i.telegram.queue[0].renewal_nodes.len(), 1);
            let persisted: TelegramState = read_json(&dir.join("telegram.json")).unwrap();
            assert_eq!(persisted.queue[0].id, i.telegram.queue[0].id);
            let sealed = app.seal("node:token", b"known fixture").unwrap();
            assert_eq!(
                app.unseal("node:token", &sealed).unwrap().as_slice(),
                b"known fixture"
            );
            assert!(app.unseal("other:token", &sealed).is_err());
        }
        drop(app);
        std::fs::remove_dir_all(dir).unwrap();
    }
    use chrono::Utc;
}

pub fn queue_notice(app: &App, i: &mut Inner, message: &str, kind: &str) {
    if !i.telegram.config.enabled
        || i.telegram.config.token.is_empty()
        || kind != "security"
        || !i.telegram.config.login
    {
        return;
    }
    let mut next = i.telegram.clone();
    if next.queue.len() >= LIMIT {
        next.dropped += 1;
        let _ = save(app, i, next);
        return;
    }
    next.queue.push(TelegramEvent {
        id: token()[..24].into(),
        site: i.data.site.name.clone(),
        kind: kind.into(),
        name: message.chars().take(3000).collect(),
        at: now(),
        next_attempt: now() + 5,
        ..Default::default()
    });
    let _ = save(app, i, next);
}

#[cfg(test)]
mod presentation_tests {
    use super::*;
    #[test]
    fn escapes_untrusted_names_and_uses_panel_button() {
        let e = TelegramEvent {
            kind: "offline".into(),
            site: "<script>&".into(),
            name: "<b onclick=x>bad</b>".into(),
            ..Default::default()
        };
        let message = text(&e);
        assert!(!message.contains("<script>"));
        assert!(message.contains("&lt;b onclick=x&gt;bad&lt;/b&gt;"));
        assert!(message.contains("<b>服务器离线</b>"));
        let request = payload("-100123", &message, "https://probe.example");
        assert_eq!(request["parse_mode"], "HTML");
        assert_eq!(
            request["reply_markup"]["inline_keyboard"][0][0]["url"],
            "https://probe.example"
        );
        assert_eq!(request["link_preview_options"]["is_disabled"], true);
    }
    #[test]
    fn grouped_renewals_stay_inside_telegram_size_limit() {
        let e = TelegramEvent {
            kind: "renewal".into(),
            site: "😀".repeat(200),
            renewal_day: "2026-10-03".into(),
            renewal_nodes: (0..200)
                .map(|_| RenewalMember {
                    name: "😀".repeat(200),
                    expires_at: "2026-10-03T10:00:00Z".into(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        let message = text(&e);
        // UTF-16 is a conservative count; Telegram excludes formatting tags.
        assert!(message.encode_utf16().count() < 4096);
        assert!(message.contains("另有 180 台"));
        assert_eq!(message.matches("10:00").count(), 0);
        assert_eq!(message.matches("18:00").count(), 20);
    }
    #[test]
    fn old_notification_config_preserves_choices_with_login_disabled() {
        let cfg: TelegramConfig = serde_json::from_value(
            json!({"enabled":true,"notifyOnline":false,"notifyOffline":true,"notifyRenewal":false}),
        )
        .unwrap();
        assert!(cfg.enabled && cfg.offline);
        assert!(!cfg.online && !cfg.renewal && !cfg.login);
    }
}
