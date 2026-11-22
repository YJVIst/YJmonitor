// Executed by a forced SSH command. Never accepts a shell command, path or service name from the peer.
use crate::{Result, VERSION, config::Config};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Installed {
    pub node_id: String,
    pub ssh_home: String,
    pub ssh_uid: u32,
    pub ssh_gid: u32,
    pub public_key: String,
    pub recovery_public: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    action: String,
    proof: String,
    #[serde(default)]
    config: Option<Config>,
    #[serde(default)]
    public_key: String,
    #[serde(default)]
    recovery_public: String,
    #[serde(default)]
    manifest: serde_json::Value,
    #[serde(default)]
    binary: String,
}
pub fn stamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn relabel(path: &Path) -> Result<()> {
    // Only root-owned managed paths reach this helper. Restore distro policy
    // after atomic replacement; never disable SELinux or change global booleans.
    if Path::new("/sys/fs/selinux/enforce").exists() {
        let status = Command::new("/usr/sbin/restorecon")
            .arg("-F")
            .arg("--")
            .arg(path)
            .status()
            .map_err(|_| "SELinux relabel unavailable")?;
        if !status.success() {
            return Err("SELinux relabel failed");
        }
    }
    Ok(())
}
fn atomic_owned(path: &Path, bytes: &[u8], mode: u32, owner: Option<(u32, u32)>) -> Result<()> {
    let random = rustls::crypto::ring::default_provider().secure_random;
    let mut nonce = [0u8; 16];
    random
        .fill(&mut nonce)
        .map_err(|_| "random source unavailable")?;
    let next = path.with_file_name(format!(".yuji-{}", hex_digest(&nonce)));
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&next)
        .map_err(|_| "staged file unavailable")?;
    let result = (|| {
        if let Some((uid, gid)) = owner {
            rustix::fs::fchown(
                &f,
                Some(rustix::process::Uid::from_raw(uid)),
                Some(rustix::process::Gid::from_raw(gid)),
            )
            .map_err(|_| "ownership restore failed")?;
        }
        f.write_all(bytes)
            .and_then(|_| f.sync_all())
            .map_err(|_| "write failed")?;
        fs::rename(&next, path).map_err(|_| "commit failed")?;
        relabel(path)?;
        fs::File::open(path.parent().ok_or("path invalid")?)
            .and_then(|d| d.sync_all())
            .map_err(|_| "sync failed")
    })();
    if result.is_err() {
        let _ = fs::remove_file(next);
    }
    result
}
fn atomic(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    atomic_owned(path, bytes, mode, None)
}
fn key(s: &str) -> Result<String> {
    let parts: Vec<_> = s.split_whitespace().collect();
    if !(2..=3).contains(&parts.len())
        || parts[0] != "ssh-ed25519"
        || s.contains(['\n', '\r', '\0'])
    {
        return Err("invalid managed public key");
    }
    let raw = STANDARD
        .decode(parts[1])
        .map_err(|_| "invalid managed public key")?;
    if raw.len() != 51 || &raw[..19] != b"\0\0\0\x0bssh-ed25519\0\0\0\x20" {
        return Err("invalid managed public key");
    }
    Ok(format!("{} {}", parts[0], parts[1]))
}
fn layout(path: &Path) -> Result<(PathBuf, String)> {
    match path.to_str() {
        Some("/var/lib/vistart-probe-agent/config.json") => Ok((
            "/opt/vistart-probe-agent".into(),
            "vistart-probe-agent.service".into(),
        )),
        Some("/var/lib/vistart-probe-test-agent/config.json") => Ok((
            "/opt/vistart-probe-test-agent".into(),
            "vistart-probe-test-agent.service".into(),
        )),
        _ => Err("unrecognized managed installation"),
    }
}
fn command(service: &str, action: &str) -> Result<()> {
    if !Command::new("/usr/bin/systemctl")
        .args([action, service])
        .status()
        .map_err(|_| "service operation unavailable")?
        .success()
    {
        return Err("service operation failed");
    }
    Ok(())
}
fn restore_file(path: &Path, raw: &[u8], mode: u32, uid: u32, gid: u32) -> Result<()> {
    atomic_owned(path, raw, mode, Some((uid, gid)))
}
fn key_directory(meta: &Installed) -> Result<rustix::fd::OwnedFd> {
    use rustix::fs::{Mode, OFlags, open, openat};
    let home = Path::new(&meta.ssh_home);
    if !home.is_absolute() {
        return Err("invalid SSH directory");
    }
    let dir = open(
        home,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|_| "SSH home unavailable")?;
    openat(
        &dir,
        ".ssh",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(|_| "SSH directory unavailable")
}
fn write_keys(meta: &Installed, raw: &[u8]) -> Result<()> {
    use rustix::fs::{AtFlags, Mode, OFlags, openat, renameat, unlinkat};
    let dir = key_directory(meta)?;
    let mut nonce = [0u8; 16];
    rustls::crypto::ring::default_provider()
        .secure_random
        .fill(&mut nonce)
        .map_err(|_| "random source unavailable")?;
    let name = format!(".yuji-{}", hex_digest(&nonce));
    let fd = openat(
        &dir,
        &name,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW,
        Mode::from_raw_mode(0o600),
    )
    .map_err(|_| "SSH keys staging failed")?;
    let mut f = fs::File::from(fd);
    let result = (|| {
        rustix::fs::fchown(
            &f,
            Some(rustix::process::Uid::from_raw(meta.ssh_uid)),
            Some(rustix::process::Gid::from_raw(meta.ssh_gid)),
        )
        .map_err(|_| "SSH key ownership failed")?;
        f.write_all(raw)
            .and_then(|_| f.sync_all())
            .map_err(|_| "SSH keys write failed")?;
        renameat(&dir, &name, &dir, "authorized_keys").map_err(|_| "SSH keys commit failed")?;
        rustix::fs::fsync(&dir).map_err(|_| "SSH keys sync failed")
    })();
    if result.is_err() {
        let _ = unlinkat(&dir, &name, AtFlags::empty());
    }
    result
}
fn edit_keys(
    meta: &Installed,
    install: &Path,
    new_public: &str,
    new_recovery: &str,
    remove: bool,
) -> Result<Vec<u8>> {
    let dir = key_directory(meta)?;
    let fd = rustix::fs::openat(
        &dir,
        "authorized_keys",
        rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NOFOLLOW,
        rustix::fs::Mode::empty(),
    )
    .map_err(|_| "SSH keys unavailable")?;
    let mut file = fs::File::from(fd);
    let stat = file.metadata().map_err(|_| "SSH keys unavailable")?;
    if !stat.is_file() || stat.nlink() != 1 || stat.len() > 1024 * 1024 {
        return Err("unsafe SSH keys file");
    }
    let mut raw = Vec::new();
    file.read_to_end(&mut raw)
        .map_err(|_| "SSH keys unavailable")?;
    let content = std::str::from_utf8(&raw).map_err(|_| "SSH keys encoding")?;
    let old1 = key(&meta.public_key)?;
    let old2 = key(&meta.recovery_public)?;
    let mut lines: Vec<String> = content
        .lines()
        .filter(|line| !line.contains(&old1) && !line.contains(&old2))
        .map(String::from)
        .collect();
    if !remove {
        let normal = key(new_public)?;
        let recovery = key(new_recovery)?;
        lines.push(format!("from=\"127.0.0.1,::1\",no-agent-forwarding,no-port-forwarding,no-X11-forwarding {normal} vistart-probe-managed"));
        let sudo = if meta.ssh_uid == 0 {
            ""
        } else {
            "/usr/bin/sudo -n "
        };
        let config = if install.ends_with("vistart-probe-test-agent") {
            "/var/lib/vistart-probe-test-agent/config.json"
        } else {
            "/var/lib/vistart-probe-agent/config.json"
        };
        lines.push(format!("restrict,command=\"{sudo}{}/agent --manage -config {config}\" {recovery} vistart-probe-recovery",install.display()));
    }
    write_keys(meta, format!("{}\n", lines.join("\n")).as_bytes())?;
    Ok(raw)
}
fn verify_binary(envelope: &serde_json::Value, bytes: &[u8]) -> Result<String> {
    use ed25519_dalek::{Signature, VerifyingKey};
    let payload = STANDARD
        .decode(envelope["payload"].as_str().ok_or("manifest invalid")?)
        .map_err(|_| "manifest invalid")?;
    if payload.len() > 16384 {
        return Err("manifest too large");
    }
    let signature = STANDARD
        .decode(envelope["signature"].as_str().ok_or("signature invalid")?)
        .map_err(|_| "signature invalid")?;
    let raw = STANDARD
        .decode(include_str!("../assets/release-public.txt").trim())
        .map_err(|_| "trust anchor invalid")?;
    let k = VerifyingKey::from_bytes(&raw.try_into().map_err(|_| "trust anchor invalid")?)
        .map_err(|_| "trust anchor invalid")?;
    k.verify_strict(
        &payload,
        &Signature::from_slice(&signature).map_err(|_| "signature invalid")?,
    )
    .map_err(|_| "signature mismatch")?;
    let m: serde_json::Value = serde_json::from_slice(&payload).map_err(|_| "manifest invalid")?;
    let v = m["version"].as_str().ok_or("version missing")?;
    if v.len() > 40
        || !v
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b))
    {
        return Err("version invalid");
    }
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        _ => return Err("unsupported architecture"),
    };
    let f = &m["files"][arch];
    if f["name"].as_str() != Some(&format!("vistart-probe-agent-{v}-linux-{arch}"))
        || f["size"].as_u64() != Some(bytes.len() as u64)
        || f["sha256"].as_str() != Some(&hex_digest(bytes))
    {
        return Err("binary integrity failure");
    }
    if bytes.len() < 1024 || bytes.len() > 32 * 1024 * 1024 {
        return Err("binary size invalid");
    }
    Ok(v.into())
}
fn hex_digest(raw: &[u8]) -> String {
    Sha256::digest(raw)
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect()
}
pub fn run(path: &Path) -> Result<()> {
    if !rustix::process::geteuid().is_root() {
        return Err("root privileges required");
    }
    let (install, unit) = layout(path)?;
    let meta_path = install.join("management.json");
    let stat = fs::symlink_metadata(&meta_path).map_err(|_| "managed recovery is not installed")?;
    if stat.uid() != 0 || stat.mode() & 0o022 != 0 || stat.len() > 16384 || !stat.is_file() {
        return Err("invalid recovery metadata");
    }
    let raw_meta = fs::read(&meta_path).map_err(|_| "recovery metadata unavailable")?;
    let meta: Installed =
        serde_json::from_slice(&raw_meta).map_err(|_| "recovery metadata invalid")?;
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(install.join("manage.lock"))
        .map_err(|_| "management lock unavailable")?;
    rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive)
        .map_err(|_| "management already in progress")?;
    let mut input = Vec::new();
    std::io::stdin()
        .take(48 * 1024 * 1024 + 1)
        .read_to_end(&mut input)
        .map_err(|_| "input unavailable")?;
    if input.len() > 48 * 1024 * 1024 {
        return Err("request too large");
    }
    let request: Request =
        serde_json::from_slice(&input).map_err(|_| "management request invalid")?;
    let st = fs::symlink_metadata(path).map_err(|_| "configuration unavailable")?;
    if !st.is_file() || st.nlink() != 1 || st.len() > 16384 {
        return Err("unsafe configuration");
    }
    let old_config = fs::read(path).map_err(|_| "configuration unavailable")?;
    let old = Config::load(path)?;
    if meta.node_id != old.node_id
        || request.proof.len() != 64
        || !constant(request.proof.as_bytes(), old.token.as_bytes())
    {
        return Err("ownership proof invalid");
    }
    match request.action.as_str() {
        "reconfigure" => {
            let next = request.config.ok_or("configuration missing")?;
            next.validate()?;
            if next.node_id != old.node_id || next.ssh_port != old.ssh_port {
                return Err("node identity mismatch");
            }
            let public = key(&request.public_key)?;
            let recovery = key(&request.recovery_public)?;
            let attrs = fs::metadata(path).map_err(|_| "configuration unavailable")?;
            let before = stamp();
            let raw = serde_json::to_vec(&next).map_err(|_| "configuration invalid")?;
            let previous_keys = edit_keys(&meta, &install, &public, &recovery, false)?;
            let result = (|| {
                restore_file(path, &raw, 0o600, attrs.uid(), attrs.gid())?;
                let newmeta = Installed {
                    public_key: public,
                    recovery_public: recovery,
                    ..meta.clone()
                };
                atomic(
                    &meta_path,
                    &serde_json::to_vec(&newmeta).map_err(|_| "metadata invalid")?,
                    0o600,
                )?;
                command(&unit, "restart")?;
                wait_health(path, &next, before, None)
            })();
            if result.is_err() {
                let _ = restore_file(path, &old_config, 0o600, attrs.uid(), attrs.gid());
                let _ = write_keys(&meta, &previous_keys);
                let _ = atomic(&meta_path, &raw_meta, 0o600);
                let _ = command(&unit, "restart");
                return Err("new controller unavailable; previous connection restored");
            }
        }
        "terminal-prepare" => {
            let script = "set -eu; export PATH=/usr/sbin:/usr/bin:/sbin:/bin; command -v tmux >/dev/null && exit 0; if command -v apt-get >/dev/null; then export DEBIAN_FRONTEND=noninteractive; apt-get update -qq >/dev/null 2>&1 && apt-get install -y -qq --no-install-recommends tmux >/dev/null 2>&1; elif command -v dnf >/dev/null; then dnf install -y -q tmux >/dev/null 2>&1; else exit 73; fi; command -v tmux >/dev/null";
            if !Command::new("/usr/bin/timeout")
                .args(["150", "/bin/sh", "-c", script])
                .status()
                .map_err(|_| "terminal dependency preparation unavailable")?
                .success()
            {
                return Err("tmux installation failed; retry deployment");
            }
        }
        "remove" => {
            // The SSH connection may travel through this Agent: acknowledge before delayed stop.
            let status = Command::new("/usr/bin/systemd-run")
                .args([
                    "--quiet",
                    "--collect",
                    "--on-active=3s",
                    "/usr/bin/systemctl",
                    "stop",
                    &unit,
                ])
                .status()
                .map_err(|_| "delayed stop unavailable")?;
            if !status.success() {
                return Err("delayed stop failed");
            }
            edit_keys(&meta, &install, "", "", true)?;
            command(&unit, "disable")?;
            fs::remove_file(path).map_err(|_| "configuration removal failed")?;
            for name in ["agent.health", "config.next"] {
                let _ = fs::remove_file(path.with_file_name(name));
            }
            fs::remove_file(&meta_path).map_err(|_| "recovery metadata removal failed")?;
        }
        "upgrade" | "rollback" => {
            let binary = install.join("agent");
            let previous = install.join("agent.previous");
            let previous_manifest = install.join("agent.previous.manifest");
            let current_manifest = install.join("agent.manifest");
            let (old_raw, old_manifest) = (
                fs::read(&binary).map_err(|_| "current binary unavailable")?,
                fs::read(&current_manifest).map_err(|_| "current manifest unavailable")?,
            );
            let (bytes, manifest) = if request.action == "upgrade" {
                (
                    STANDARD
                        .decode(request.binary)
                        .map_err(|_| "binary encoding invalid")?,
                    serde_json::to_vec(&request.manifest).map_err(|_| "manifest invalid")?,
                )
            } else {
                (
                    fs::read(&previous).map_err(|_| "no previous version")?,
                    fs::read(&previous_manifest).map_err(|_| "no previous manifest")?,
                )
            };
            let envelope = serde_json::from_slice(&manifest).map_err(|_| "manifest invalid")?;
            let version = verify_binary(&envelope, &bytes)?;
            let old_env =
                serde_json::from_slice(&old_manifest).map_err(|_| "current manifest invalid")?;
            verify_binary(&old_env, &old_raw)?;
            let before = stamp();
            atomic(&previous, &old_raw, 0o755)?;
            atomic(&previous_manifest, &old_manifest, 0o600)?;
            atomic(&binary, &bytes, 0o755)?;
            atomic(&current_manifest, &manifest, 0o600)?;
            let result = command(&unit, "restart")
                .and_then(|_| wait_health(path, &old, before, Some(&version)));
            if result.is_err() {
                let _ = atomic(&binary, &old_raw, 0o755);
                let _ = atomic(&current_manifest, &old_manifest, 0o600);
                let _ = command(&unit, "restart");
                return Err("new Agent unhealthy; previous binary restored");
            }
        }
        _ => return Err("unsupported management operation"),
    }
    println!("{{\"ok\":true}}");
    Ok(())
}
fn constant(a: &[u8], b: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    a.ct_eq(b).into()
}

#[derive(Clone, Copy)]
pub struct HealthAck {
    generation: u64,
    received: std::time::Instant,
    at: u64,
}
impl HealthAck {
    pub fn new(generation: u64) -> Self {
        Self {
            generation,
            received: std::time::Instant::now(),
            at: stamp(),
        }
    }
    fn due(self, previous: Option<Self>) -> bool {
        previous.is_none_or(|last| {
            last.generation != self.generation
                || self.received.duration_since(last.received) >= Duration::from_secs(30)
        })
    }
}
pub async fn report_health(
    path: PathBuf,
    config: &Config,
    mut acks: tokio::sync::watch::Receiver<Option<HealthAck>>,
) {
    let path = std::sync::Arc::new(path.with_file_name("agent.health"));
    let identity = std::sync::Arc::new(
        serde_json::json!({"node_id":config.node_id,"controller_url":config.controller_url,"version":VERSION,"credential_digest":hex_digest(config.token.as_bytes())}),
    );
    let mut previous = None;
    while acks.changed().await.is_ok() {
        let Some(ack) = *acks.borrow_and_update() else {
            continue;
        };
        if !ack.due(previous) {
            continue;
        }
        let path = path.clone();
        let identity = identity.clone();
        let result = tokio::task::spawn_blocking(move || {
            let mut value = (*identity).clone();
            value["at"] = ack.at.into();
            atomic(
                &path,
                &serde_json::to_vec(&value).map_err(|_| "health encoding failed")?,
                0o600,
            )
        })
        .await;
        if matches!(result, Ok(Ok(()))) {
            previous = Some(ack);
        } else {
            eprintln!("Agent health persistence unavailable");
        }
    }
}
fn wait_health(path: &Path, config: &Config, after: u64, version: Option<&str>) -> Result<()> {
    for _ in 0..40 {
        let current = fs::read(path.with_file_name("agent.health"))
            .ok()
            .and_then(|v| serde_json::from_slice::<serde_json::Value>(&v).ok());
        if let Some(v) = current {
            if v["at"].as_u64().is_some_and(|at| at > after)
                && v["node_id"].as_str() == Some(&config.node_id)
                && v["controller_url"].as_str() == Some(&config.controller_url)
                && v["credential_digest"].as_str() == Some(&hex_digest(config.token.as_bytes()))
                && version.is_none_or(|ver| v["version"].as_str() == Some(ver))
            {
                return Ok(());
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    Err("Agent did not confirm WSS connection")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn first_ack_reconnect_and_upgrade_proof() {
        let dir = std::env::temp_dir().join(format!("yuji-health-{}", std::process::id()));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("config.json");
        let config = Config {
            node_id: "testnode".into(),
            token: "a".repeat(64),
            controller_url: "wss://test.invalid/api/agent".into(),
            ssh_port: 22,
        };
        let (tx, rx) = tokio::sync::watch::channel(None);
        let cfg = config.clone();
        let location = path.clone();
        let worker = tokio::spawn(async move {
            report_health(location, &cfg, rx).await;
        });
        let start = HealthAck::new(1);
        for (seconds, generation, expected) in [
            (0, 1, true),
            (3, 1, false),
            (27, 1, false),
            (30, 1, true),
            (3, 2, true),
        ] {
            let ack = HealthAck {
                generation,
                received: start.received + Duration::from_secs(seconds),
                at: start.at + seconds,
            };
            assert_eq!(
                ack.due(if seconds == 0 { None } else { Some(start) }),
                expected
            );
        }
        tx.send_replace(Some(start));
        drop(tx);
        worker.await.unwrap();
        wait_health(&path, &config, start.at - 1, Some(VERSION)).unwrap();
        let data: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.join("agent.health")).unwrap()).unwrap();
        assert_eq!(data["at"], start.at);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn immutable_layout() {
        assert!(layout(Path::new("/etc/passwd")).is_err());
        assert!(!constant(b"a", b"b"));
        assert!(key("ssh-rsa bad").is_err());
    }
}
