use crate::{
    auth,
    core::*,
    migration::Transfer,
    model::*,
    operations::{self, State},
};
use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Sha256;
use std::{
    collections::HashMap,
    io::{Read, Write},
};
use zeroize::Zeroizing;
const AAD: &[u8] = b"yuji-probe-backup-v1:pbkdf2-sha256:600000:aes256gcm";
const PACK_LIMIT: usize = 16 * 1024 * 1024;
const RAW_LIMIT: usize = 128 * 1024 * 1024;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    format: String,
    salt: String,
    nonce: String,
    data: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Bundle {
    schema: u32,
    origin: String,
    created: i64,
    master: String,
    auth: Auth,
    data: Data,
    telegram: TelegramState,
    operations: State,
    // v0.5.0 backups can contain retired history data. Consume it without restoring it.
    #[serde(
        default,
        rename = "tracks",
        skip_serializing,
        deserialize_with = "discard_legacy"
    )]
    _legacy_tracks: (),
    #[serde(
        default,
        rename = "pins",
        skip_serializing,
        deserialize_with = "discard_legacy"
    )]
    _legacy_pins: (),
}
fn discard_legacy<'de, D: serde::Deserializer<'de>>(d: D) -> Result<(), D::Error> {
    serde::de::IgnoredAny::deserialize(d)?;
    Ok(())
}
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
struct Input {
    passphrase: String,
    backup: Value,
    every_hours: u32,
    keep: usize,
    name: String,
    confirm: bool,
}
fn error(message: &str) -> ApiError {
    ApiError::new(400, message)
}
fn password_ok(p: &str) -> bool {
    (12..=128).contains(&p.len()) && !p.contains('\0')
}
fn pack(bundle: Bundle, passphrase: &str) -> ApiResult<Value> {
    if !password_ok(passphrase) {
        return Err(error("备份口令需为 12–128 字节"));
    }
    let raw = Zeroizing::new(serde_json::to_vec(&bundle).map_err(|_| ApiError::internal())?);
    if raw.len() > RAW_LIMIT {
        return Err(error("备份数据超过 128 MiB，请请减少备份内容或联系管理员"));
    }
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gz.write_all(&raw).map_err(|_| ApiError::internal())?;
    let compressed = Zeroizing::new(gz.finish().map_err(|_| ApiError::internal())?);
    if compressed.len() > PACK_LIMIT - 16 {
        return Err(error("压缩备份超过 16 MiB"));
    }
    let mut salt = [0; 16];
    let mut nonce = [0; 12];
    getrandom::fill(&mut salt).map_err(|_| ApiError::internal())?;
    getrandom::fill(&mut nonce).map_err(|_| ApiError::internal())?;
    let mut key = Zeroizing::new([0u8; 32]);
    pbkdf2::pbkdf2_hmac::<Sha256>(passphrase.as_bytes(), &salt, 600_000, key.as_mut());
    let cipher = Aes256Gcm::new_from_slice(key.as_ref()).map_err(|_| ApiError::internal())?;
    let encrypted = cipher
        .encrypt(
            &Nonce::from(nonce),
            Payload {
                msg: &compressed,
                aad: AAD,
            },
        )
        .map_err(|_| ApiError::internal())?;
    serde_json::to_value(Envelope {
        format: "yuji-probe-backup-v1".into(),
        salt: STANDARD.encode(salt),
        nonce: STANDARD.encode(nonce),
        data: STANDARD.encode(encrypted),
    })
    .map_err(|_| ApiError::internal())
}
fn unpack(value: Value, passphrase: &str) -> ApiResult<Bundle> {
    if !password_ok(passphrase) {
        return Err(error("备份口令不正确"));
    }
    let e: Envelope = serde_json::from_value(value).map_err(|_| error("备份格式不正确"))?;
    if e.format != "yuji-probe-backup-v1" || e.data.len() > PACK_LIMIT.div_ceil(3) * 4 {
        return Err(error("备份格式或大小不正确"));
    }
    let salt = STANDARD
        .decode(e.salt)
        .map_err(|_| error("备份格式不正确"))?;
    let nonce: [u8; 12] = STANDARD
        .decode(e.nonce)
        .map_err(|_| error("备份格式不正确"))?
        .try_into()
        .map_err(|_| error("备份格式不正确"))?;
    if salt.len() != 16 {
        return Err(error("备份格式不正确"));
    }
    let bytes = STANDARD
        .decode(e.data)
        .map_err(|_| error("备份格式不正确"))?;
    let mut key = Zeroizing::new([0u8; 32]);
    pbkdf2::pbkdf2_hmac::<Sha256>(passphrase.as_bytes(), &salt, 600_000, key.as_mut());
    let cipher = Aes256Gcm::new_from_slice(key.as_ref()).map_err(|_| ApiError::internal())?;
    let compressed = Zeroizing::new(
        cipher
            .decrypt(
                &Nonce::from(nonce),
                Payload {
                    msg: &bytes,
                    aad: AAD,
                },
            )
            .map_err(|_| error("口令错误或备份已损坏"))?,
    );
    let mut raw = Zeroizing::new(Vec::new());
    flate2::read::GzDecoder::new(compressed.as_slice())
        .take((RAW_LIMIT + 1) as u64)
        .read_to_end(&mut raw)
        .map_err(|_| error("备份解压失败"))?;
    if raw.len() > RAW_LIMIT {
        return Err(error("备份解压大小超过限制"));
    }
    let b: Bundle = serde_json::from_slice(&raw).map_err(|_| error("备份内容不正确"))?;
    if b.schema != 1
        || b.data.nodes.len() > MAX_NODES
        || b.data.commands.len() > 50
        || !valid_text(&b.data.site.name, 60)
        || !username(&b.auth.username)
        || b.auth.hash.len() > 128
    {
        return Err(error("备份内容超出限制"));
    }
    if b.data.nodes.iter().any(|n| {
        !operations::valid_id(&n.public.id)
            || crate::ssh_address::normalize(&n.ip).is_none()
            || !username(&n.username)
            || !(1..=65535).contains(&n.port)
            || n.public.group.chars().count() > 40
            || n.notes.chars().count() > 2000
    }) {
        return Err(error("节点配置不正确"));
    }
    let ids: std::collections::HashSet<_> =
        b.data.nodes.iter().map(|n| n.public.id.as_str()).collect();
    if ids.len() != b.data.nodes.len()
        || b.data.secrets.len() > MAX_NODES
        || !b.data.secrets.keys().all(|id| ids.contains(id.as_str()))
        || b.auth.recovery.len() > 10
        || b.auth
            .recovery
            .iter()
            .any(|r| r.len() != 64 || !r.bytes().all(|c| c.is_ascii_hexdigit()))
        || b.operations.transfers.len() > MAX_NODES
        || b.operations.retired.len() > MAX_NODES
        || !crate::offsite::validate_restore(&b.operations.offsite)
    {
        return Err(error("备份身份或状态不完整"));
    }
    if b.auth.hash.parse::<bcrypt::HashParts>().is_err() {
        return Err(error("管理员密码哈希无效"));
    }
    Ok(b)
}
fn snapshot(app: &App, i: &Inner) -> Bundle {
    let mut operations = i.ops.clone();
    operations::prune_removed_nodes(&mut operations, &i.data.nodes);
    Bundle {
        schema: 1,
        origin: app.0.origin.clone(),
        created: now(),
        master: STANDARD.encode(app.0.master.as_slice()),
        auth: i.auth.clone(),
        data: i.data.clone(),
        telegram: i.telegram.clone(),
        operations,
        _legacy_tracks: (),
        _legacy_pins: (),
    }
}
pub async fn api(app: App, c: Context, body: Vec<u8>) -> ApiResult<ApiReply> {
    let input: Input = if body.is_empty() {
        Input::default()
    } else {
        decode(&body)?
    };
    {
        let mut i = app.lock();
        app.guard(&mut i, &c, true, c.method != "GET")?;
        auth::require_elevated(&i, &c)?;
    }
    let action = c.path.strip_prefix("/api/admin/ops/").unwrap_or("");
    if action == "backup" && c.method == "GET" {
        let i = app.lock();
        let mut files = Vec::new();
        if let Ok(entries) = std::fs::read_dir(app.0.dir.join("backups")) {
            for e in entries.flatten().take(32) {
                let name = e.file_name().to_string_lossy().to_string();
                if valid_backup(&name)
                    && let Ok(m) = e.metadata()
                {
                    files.push(json!({"name":name,"size":m.len()}));
                }
            }
        }
        return Ok(ApiReply::ok(
            json!({"files":files,"everyHours":i.ops.schedule.every_hours,"keep":i.ops.schedule.keep,"hasPassphrase":!i.ops.schedule.sealed_password.is_empty(),"last":i.ops.schedule.last,"error":i.ops.schedule.error}),
        ));
    }
    if action == "backup/download" && c.method == "POST" {
        if !valid_backup(&input.name) {
            return Err(error("文件名不正确"));
        }
        let path = app.0.dir.join("backups").join(&input.name);
        let mut raw = Vec::new();
        std::fs::File::open(path)
            .map_err(|_| error("备份文件不可用"))?
            .take(24 * 1024 * 1024 + 1)
            .read_to_end(&mut raw)
            .map_err(|_| error("备份读取失败"))?;
        if raw.len() > 24 * 1024 * 1024 {
            return Err(error("备份文件过大"));
        }
        let value: Value = serde_json::from_slice(&raw).map_err(|_| error("备份文件不可用"))?;
        return Ok(ApiReply::ok(json!({"backup":value,"name":input.name})));
    }
    if action == "backup/schedule" && c.method == "POST" {
        if ![0, 6, 12, 24, 168].contains(&input.every_hours) || !(1..=30).contains(&input.keep) {
            return Err(error("备份周期或保留数量不正确"));
        }
        let mut i = app.lock();
        let mut schedule = i.ops.schedule.clone();
        if !input.passphrase.is_empty() {
            if !password_ok(&input.passphrase) {
                return Err(error("备份口令需为 12–128 字节"));
            }
            schedule.sealed_password = app
                .seal("backup:passphrase", input.passphrase.as_bytes())
                .map_err(|_| ApiError::internal())?;
        }
        if input.every_hours > 0 && schedule.sealed_password.is_empty() {
            return Err(error("请设置备份口令"));
        }
        schedule.every_hours = input.every_hours;
        schedule.keep = input.keep;
        i.ops.schedule = schedule;
        operations::save(&app, &i)?;
        app.record(&mut i, "backup_schedule_updated", &c.ip);
        return Ok(ApiReply::ok(json!({"ok":true})));
    }
    if action == "backup/create" && c.method == "POST" {
        let _slot = app
            .0
            .login_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| ApiError::rate("已有加密任务", 5))?;
        let b = {
            let mut i = app.lock();
            i.request("backup:create", 3)?;
            snapshot(&app, &i)
        };
        let pass = Zeroizing::new(input.passphrase);
        let result = tokio::task::spawn_blocking(move || pack(b, &pass))
            .await
            .map_err(|_| ApiError::internal())??;
        let mut i = app.lock();
        app.guard(&mut i, &c, true, true)?;
        app.record(&mut i, "backup_exported", &c.ip);
        return Ok(ApiReply::ok(
            json!({"backup":result,"name":format!("yuji-{}-{}.backup",now(),&token()[..8])}),
        ));
    }
    if action == "restore" && c.method == "POST" {
        app.lock().request("backup:restore", 3)?;
        if !input.confirm {
            return Err(error("请确认替换当前节点、管理员和设置"));
        }
        let _slot = app
            .0
            .login_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| ApiError::rate("已有恢复任务", 5))?;
        let pass = Zeroizing::new(input.passphrase);
        let bundle = tokio::task::spawn_blocking(move || unpack(input.backup, &pass))
            .await
            .map_err(|_| ApiError::internal())??;
        let mut i = app.lock();
        app.guard(&mut i, &c, true, true)?;
        auth::require_elevated(&i, &c)?;
        if i.ops.busy
            || i.ops.transfers.values().any(|t| t.state == "running")
            || i.jobs.values().any(|j| j.state == "running")
        {
            return Err(ApiError::new(409, "请等待节点管理任务完成后恢复"));
        }
        let migrated = restore(&app, &mut i, bundle)?;
        app.record(&mut i, "backup_restored", &c.ip);
        return Ok(ApiReply::ok(
            json!({"ok":true,"migration":migrated,"message":"恢复完成，请使用备份中的管理员账户重新登录"}),
        ));
    }
    Err(ApiError::new(404, "接口不存在"))
}
fn rewrap(app: &App, master: &[u8], label: &str, value: &mut String) -> ApiResult<()> {
    if value.is_empty() {
        return Ok(());
    }
    let raw = STANDARD
        .decode(value.as_bytes())
        .map_err(|_| error("备份密钥损坏"))?;
    if raw.len() < 28 {
        return Err(error("备份密钥损坏"));
    }
    let nonce: [u8; 12] = raw[..12].try_into().unwrap();
    let cipher = Aes256Gcm::new_from_slice(master).map_err(|_| error("备份主密钥不正确"))?;
    let plain = Zeroizing::new(
        cipher
            .decrypt(
                &Nonce::from(nonce),
                Payload {
                    msg: &raw[12..],
                    aad: label.as_bytes(),
                },
            )
            .map_err(|_| error("备份密钥校验失败"))?,
    );
    *value = app.seal(label, &plain).map_err(|_| ApiError::internal())?;
    Ok(())
}
fn secret_rewrap(app: &App, key: &[u8], id: &str, s: &mut NodeSecret) -> ApiResult<()> {
    rewrap(app, key, &format!("{id}:token"), &mut s.token)?;
    rewrap(app, key, &format!("{id}:ssh"), &mut s.ssh_key)?;
    rewrap(app, key, &format!("{id}:recovery"), &mut s.recovery_key)
}
fn restore(app: &App, i: &mut Inner, mut b: Bundle) -> ApiResult<bool> {
    b.data.theme.migrate_removed();
    b.data.theme.validate()?;
    let master = Zeroizing::new(
        STANDARD
            .decode(&b.master)
            .map_err(|_| error("备份主密钥不正确"))?,
    );
    if master.len() != 32 {
        return Err(error("备份主密钥不正确"));
    }
    rewrap(app, &master, "admin:mfa", &mut b.auth.mfa)?;
    rewrap(
        app,
        &master,
        "telegram-bot-token-v1",
        &mut b.telegram.config.token,
    )?;
    rewrap(
        app,
        &master,
        "backup:passphrase",
        &mut b.operations.schedule.sealed_password,
    )?;
    rewrap(
        app,
        &master,
        "backup:offsite-secret",
        &mut b.operations.offsite.secret,
    )?;
    for (id, s) in &mut b.data.secrets {
        secret_rewrap(app, &master, id, s)?;
    }
    for (id, t) in &mut b.operations.transfers {
        secret_rewrap(app, &master, id, &mut t.old)?;
    }
    for (id, s) in &mut b.operations.retired {
        secret_rewrap(app, &master, id, s)?;
    }
    let migrated = b.origin != app.0.origin;
    if migrated {
        b.operations.transfers.clear();
        let nodes = b.data.nodes.clone();
        for n in nodes {
            let id = &n.public.id;
            if n.demo || n.removing {
                continue;
            }
            if let Some(old) = b.data.secrets.get(id).cloned() {
                // Fresh identity on the new controller; the old controller cannot reclaim migrated nodes.
                let (key, public) = crate::ssh::create_key("vistart-probe-managed")
                    .map_err(|_| ApiError::internal())?;
                let (recovery, recovery_public) = crate::ssh::create_key("vistart-probe-recovery")
                    .map_err(|_| ApiError::internal())?;
                use sha2::Digest;
                let token = Zeroizing::new(token());
                let next = NodeSecret {
                    token_hash: hex::encode(Sha256::digest(token.as_bytes())),
                    token: app
                        .seal(&format!("{id}:token"), token.as_bytes())
                        .map_err(|_| ApiError::internal())?,
                    ssh_key: app
                        .seal(&format!("{id}:ssh"), &key)
                        .map_err(|_| ApiError::internal())?,
                    public_key: public,
                    host_key: old.host_key.clone(),
                    derived_recovery: false,
                    recovery_key: app
                        .seal(&format!("{id}:recovery"), &recovery)
                        .map_err(|_| ApiError::internal())?,
                    recovery_public,
                };
                b.operations.transfers.insert(
                    id.clone(),
                    Transfer {
                        action: "reconfigure".into(),
                        target: app.0.origin.clone(),
                        state: "pending".into(),
                        message: "等待新主控发起 SSH 恢复".into(),
                        old,
                        ..Default::default()
                    },
                );
                b.data.secrets.insert(id.clone(), next);
            }
        }
    }
    for n in &mut b.data.nodes {
        n.public.online = false;
        n.public.latency_ms = None;
        n.last_seen = 0;
    }
    b.auth.version = token();
    b.telegram.queue.clear();
    b.telegram.test = TelegramTestResult::default();
    b.telegram.group.test = TelegramTestResult::default();
    b.operations.busy = false;
    let mut files = HashMap::new();
    for (name, value) in [
        ("auth.json", serde_json::to_value(&b.auth)),
        ("nodes.json", serde_json::to_value(&b.data)),
        ("telegram.json", serde_json::to_value(&b.telegram)),
        ("operations.json", serde_json::to_value(&b.operations)),
    ] {
        let value = value.map_err(|_| ApiError::internal())?;
        encode_state_json(&app.0.dir.join(name), &value)
            .map_err(|_| ApiError::new(413, "备份状态超过容量限制"))?;
        files.insert(name.to_string(), value);
    }
    atomic_json(&app.0.dir.join("restore-journal.json"), &files)
        .map_err(|_| ApiError::internal())?;
    if complete_restore(&app.0.dir).is_err() {
        // A committed journal is replayed before accepting traffic after restart.
        app.0.stop.cancel();
        return Err(ApiError::new(503, "恢复提交中断，服务重启后将继续恢复"));
    }
    auth::invalidate(i);
    for link in i.agents.values() {
        link.stop.cancel();
    }
    i.agents.clear();
    i.tickets.clear();
    i.ssh_dns.clear();
    i.jobs.clear();
    i.cache = None;
    i.theme_cache = None;
    i.auth = b.auth;
    i.data = b.data;
    i.telegram = b.telegram;
    i.ops = b.operations;
    // Clear old delivery before allowing imported notification credentials.
    if let Some((_, cancel)) = &i.delivery {
        cancel.cancel();
    }
    i.delivery = None;
    i.telegram_revision += 1;
    i.telegram_ready = now() + 30;
    Ok(migrated)
}
pub fn complete_restore(dir: &std::path::Path) -> Result<(), &'static str> {
    let path = dir.join("restore-journal.json");
    if !path.exists() {
        return Ok(());
    }
    let mut files: HashMap<String, Value> =
        read_json(&path).map_err(|_| "restore journal invalid or too large")?;
    let allowed = [
        "auth.json",
        "nodes.json",
        "telegram.json",
        "operations.json",
    ];
    // Resume a v0.5.0 restore without reintroducing removed history files.
    files.remove("history");
    files.remove("ssh-pins.json");
    if files.len() != allowed.len() || !files.keys().all(|v| allowed.contains(&v.as_str())) {
        return Err("restore journal invalid");
    }
    let prepared = files
        .into_iter()
        .map(|(name, value)| {
            let path = dir.join(name);
            let bytes = encode_state_json(&path, &value).map_err(|_| "restore state too large")?;
            Ok((path, bytes))
        })
        .collect::<Result<Vec<_>, &'static str>>()?;
    for (path, bytes) in prepared {
        atomic_bytes(&path, &bytes).map_err(|_| "restore commit failed")?;
    }
    std::fs::remove_file(path).map_err(|_| "restore commit failed")?;
    Ok(())
}
fn valid_backup(s: &str) -> bool {
    s.starts_with("yuji-")
        && s.ends_with(".backup")
        && s.len() < 100
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b))
}
pub async fn scheduled(app: &App) {
    let _ = start_scheduled(app, false);
}
pub fn start_scheduled(app: &App, force: bool) -> ApiResult<()> {
    let work = {
        let mut i = app.lock();
        let s = &i.ops.schedule;
        if i.ops.busy {
            return if force {
                Err(ApiError::new(409, "备份任务正在进行"))
            } else {
                Ok(())
            };
        }
        if s.sealed_password.is_empty() {
            return if force {
                Err(error("请先设置自动备份口令"))
            } else {
                Ok(())
            };
        }
        if !force && (s.every_hours == 0 || now() - s.last < i64::from(s.every_hours) * 3600) {
            return Ok(());
        }
        let pass = app
            .unseal("backup:passphrase", &s.sealed_password)
            .map_err(|_| error("备份口令无法解密"))?;
        let keep = s.keep.clamp(1, 30);
        let b = snapshot(app, &i);
        i.ops.busy = true;
        (b, pass, keep)
    };
    let app = app.clone();
    tokio::spawn(async move {
        let (b, pass, keep) = work;
        let result = tokio::task::spawn_blocking(move || {
            let pass = std::str::from_utf8(&pass).map_err(|_| error("备份口令损坏"))?;
            pack(b, pass)
        })
        .await
        .unwrap_or_else(|_| Err(ApiError::internal()));
        let result = result.and_then(|value| {
            let dir = app.0.dir.join("backups");
            std::fs::create_dir_all(&dir).map_err(|_| ApiError::internal())?;
            let name = format!("yuji-{}-{}.backup", now(), &token()[..8]);
            atomic_json(&dir.join(&name), &value).map_err(|_| ApiError::internal())?;
            let mut entries = std::fs::read_dir(&dir)
                .map_err(|_| ApiError::internal())?
                .flatten()
                .filter(|e| valid_backup(&e.file_name().to_string_lossy()))
                .map(|e| e.path())
                .collect::<Vec<_>>();
            entries.sort();
            let remove = entries.len().saturating_sub(keep);
            for path in entries.into_iter().take(remove) {
                let _ = std::fs::remove_file(path);
            }
            Ok(name)
        });
        let mut i = app.lock();
        i.ops.busy = false;
        i.ops.schedule.last = now();
        match result {
            Ok(name) => {
                i.ops.schedule.error.clear();
                crate::offsite::enqueue(&mut i, &name);
            }
            Err(e) => i.ops.schedule.error = e.message,
        }
        let _ = operations::save(&app, &i);
    });
    Ok(())
}
pub fn valid_version(s: &str) -> bool {
    let pieces: Vec<_> = s.split('.').collect();
    pieces.len() == 3
        && pieces
            .iter()
            .all(|v| !v.is_empty() && v.len() <= 5 && v.bytes().all(|b| b.is_ascii_digit()))
}
pub fn newer_release(candidate: &str, current: &str) -> bool {
    if !valid_version(candidate) || !valid_version(current) {
        return false;
    }
    let parts = |v: &str| {
        v.split('.')
            .map(|n| n.parse::<u32>().unwrap())
            .collect::<Vec<_>>()
    };
    parts(candidate) > parts(current)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn updates_never_offer_equal_or_older_release() {
        assert!(!newer_release("0.4.1", "0.5.0"));
        assert!(!newer_release("0.5.0", "0.5.0"));
        assert!(newer_release("0.10.0", "0.9.9"));
        assert!(newer_release("1.0.0", "0.99.0"));
        assert!(!newer_release("invalid", "0.5.0"));
    }
    #[test]
    fn package_tamper_and_wrong_password() {
        let mut b = Bundle {
            schema: 1,
            origin: "https://old.example".into(),
            created: 1,
            master: STANDARD.encode([1; 32]),
            auth: Auth {
                username: "admin".into(),
                hash: bcrypt::hash("example-test-passphrase", 4).unwrap(),
                ..Default::default()
            },
            data: Data {
                site: Site {
                    name: "QA".into(),
                    ..Default::default()
                },
                ..Default::default()
            },
            telegram: TelegramState::default(),
            operations: State::default(),
            _legacy_tracks: (),
            _legacy_pins: (),
        };
        for num in 0..MAX_NODES {
            let id = format!("backup-{num}");
            b.data.nodes.push(Node {
                public: PublicNode {
                    id: id.clone(),
                    name: format!("Node {num}"),
                    ..Default::default()
                },
                ip: format!("10.1.{}.{}", num / 250, num % 250 + 1),
                port: 22,
                username: "root".into(),
                ..Default::default()
            });
            b.data.secrets.insert(id.clone(), NodeSecret::default());
            b.operations.transfers.insert(
                id.clone(),
                Transfer {
                    state: "done".into(),
                    ..Default::default()
                },
            );
            b.operations.retired.insert(id, NodeSecret::default());
        }
        let packed = pack(b, "test-backup-passphrase").unwrap();
        assert!(unpack(packed.clone(), "wrong-backup-passphrase").is_err());
        assert_eq!(
            unpack(packed.clone(), "test-backup-passphrase")
                .unwrap()
                .origin,
            "https://old.example"
        );
        assert_eq!(
            unpack(packed.clone(), "test-backup-passphrase")
                .unwrap()
                .data
                .nodes
                .len(),
            MAX_NODES
        );
        let mut bad = packed;
        bad["format"] = json!("other");
        assert!(unpack(bad, "test-backup-passphrase").is_err());
    }
    #[test]
    fn traversal_and_versions() {
        assert!(!valid_backup("../../auth.json"));
        assert!(!valid_version("0.5.0/../../"));
        assert!(valid_version("0.5.0"));
    }
}

#[cfg(test)]
mod legacy_tests {
    use super::*;
    #[test]
    fn old_backup_discards_history_policy_and_alarms() {
        let raw = json!({
            "schema":1, "origin":"https://old.example", "created":1, "master":"",
            "auth":{}, "data":{"nodes":[{"public":{"id":"one","name":"Node"},"policy":{"files":"off","cpu":90,"maintenanceUntil":123}}]},
            "telegram":{}, "operations":{"alarms":{"one:CPU":{"active":true}},"schedule":{"every_hours":24,"keep":7}},
            "tracks":{"one":{"minute":[{"at":123,"cpu":25}],"rx":5000}}, "pins":{}
        });
        let b: Bundle = serde_json::from_value(raw).unwrap();
        assert_eq!(b.operations.schedule.every_hours, 24);
        assert_eq!(b.data.nodes[0].public.id, "one");
        let clean = serde_json::to_value(b).unwrap();
        assert!(clean.get("tracks").is_none());
        assert!(clean["operations"].get("alarms").is_none());
        assert!(clean["data"]["nodes"][0].get("policy").is_none());
    }
    #[test]
    fn oversized_journal_member_leaves_all_live_files_intact() {
        let dir = std::env::temp_dir().join(format!("yuji-oversize-{}", token()));
        std::fs::create_dir(&dir).unwrap();
        let files = json!({"auth.json":{}, "nodes.json":{}, "telegram.json":"x".repeat(16*1024*1024), "operations.json":{}});
        atomic_json(&dir.join("restore-journal.json"), &files).unwrap();
        assert!(complete_restore(&dir).is_err());
        for name in files.as_object().unwrap().keys() {
            assert!(!dir.join(name).exists());
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn nodes_legacy_size_and_failed_overwrite() {
        let dir = std::env::temp_dir().join(format!("yuji-capacity-{}", token()));
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("nodes.json");
        let legacy = " ".repeat(17 * 1024 * 1024) + r#"{"v":1}"#;
        std::fs::write(&path, legacy).unwrap();
        assert_eq!(read_json::<Value>(&path).unwrap(), json!({"v":1}));
        let large = "x".repeat(17 * 1024 * 1024);
        atomic_json(&path, &large).unwrap();
        assert_eq!(read_json::<String>(&path).unwrap(), large);
        assert!(atomic_json(&path, &"x".repeat(64 * 1024 * 1024)).is_err());
        assert_eq!(read_json::<String>(&path).unwrap(), large);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn old_restore_journal_ignores_retired_history_and_rejects_paths() {
        let dir = std::env::temp_dir().join(format!("yuji-legacy-{}", token()));
        std::fs::create_dir(&dir).unwrap();
        let mut files = json!({"auth.json":{}, "nodes.json":{}, "telegram.json":{}, "operations.json":{}, "ssh-pins.json":{}, "history":{"../../escape":{}}});
        atomic_json(&dir.join("restore-journal.json"), &files).unwrap();
        complete_restore(&dir).unwrap();
        assert!(!dir.join("history").exists());
        assert!(!dir.join("restore-journal.json").exists());
        files
            .as_object_mut()
            .unwrap()
            .insert("../escape.json".into(), json!({}));
        atomic_json(&dir.join("restore-journal.json"), &files).unwrap();
        assert!(complete_restore(&dir).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
