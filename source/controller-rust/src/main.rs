#![forbid(unsafe_code)]
mod auth;
mod backup;
mod billing;
mod core;
mod deploy;
mod diagnostics;
mod enrollment;
mod file_sessions;
mod files;
mod http;
mod legacy_terminals;
mod migration;
mod model;
mod nodes;
mod offsite;
mod operations;
mod realtime;
mod retained;
mod server_management;
mod setup;
mod ssh;
mod ssh_address;
mod telegram;
mod terminal;
mod theme;
mod tmux_stream;
mod updates;
mod web;
use crate::{core::*, model::*};
use std::{
    fs::{File, OpenOptions},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    process::{Command, Stdio},
    time::Duration,
};
#[derive(Clone)]
struct Options {
    dir: PathBuf,
    listen: std::net::SocketAddr,
    origin: String,
    gateway: bool,
    daemon: bool,
    supervise: bool,
    worker: bool,
    init: bool,
    reset_mfa: bool,
    web: bool,
    install: bool,
}
impl Options {
    fn args(&self) -> Vec<String> {
        let mut a = vec![
            "-state".into(),
            self.dir.to_string_lossy().into(),
            "-listen".into(),
            self.listen.to_string(),
            "-origin".into(),
            self.origin.clone(),
        ];
        if self.web {
            a.push("-web".into());
        }
        if self.gateway {
            a.push("-php-gateway".into())
        }
        a
    }
}
fn options() -> Result<Options, &'static str> {
    let mut o = Options {
        dir: "/srv/vistart-probe/state".into(),
        listen: "127.0.0.1:19281".parse().unwrap(),
        origin: String::new(),
        gateway: false,
        daemon: false,
        supervise: false,
        worker: false,
        init: false,
        reset_mfa: false,
        web: false,
        install: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-state" => o.dir = args.next().ok_or("state path missing")?.into(),
            "-listen" => {
                o.listen = args
                    .next()
                    .ok_or("listen missing")?
                    .parse()
                    .map_err(|_| "invalid listen address")?
            }
            "-origin" => o.origin = args.next().ok_or("origin missing")?,
            "-php-gateway" => o.gateway = true,
            "-web" => o.web = true,
            "--install" => o.install = true,
            "-daemon" => o.daemon = true,
            "-supervise" => o.supervise = true,
            "-worker" => o.worker = true,
            "-init" => o.init = true,
            "--reset-mfa" => o.reset_mfa = true,
            "--version" | "-version" => {
                println!("vistart-probe-controller {VERSION} rust");
                std::process::exit(0)
            }
            _ => return Err("unsupported argument"),
        }
    }
    if o.listen.port() == 0 || (!o.web && !o.listen.ip().is_loopback()) {
        return Err("public listen requires native -web mode");
    }
    if o.reset_mfa && o.origin.is_empty() {
        o.origin = "https://localhost".into();
    }
    let url = reqwest::Url::parse(&o.origin).map_err(|_| "invalid HTTPS origin")?;
    if (url.scheme() != "https" && !(o.web && url.scheme() == "http"))
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err("origin must be a canonical HTTP(S) origin");
    }
    if url.scheme() == "https" && !o.listen.ip().is_loopback() {
        return Err("HTTPS origin requires a loopback listener behind the local reverse proxy");
    }
    o.origin = url.as_str().trim_end_matches('/').into();
    if !o.dir.is_absolute() {
        o.dir = std::env::current_dir()
            .map_err(|_| "working directory unavailable")?
            .join(o.dir)
    }
    if o.web && o.gateway {
        return Err("web and PHP gateway modes are mutually exclusive");
    }
    if [
        o.daemon,
        o.supervise,
        o.worker,
        o.init,
        o.reset_mfa,
        o.install,
    ]
    .iter()
    .filter(|v| **v)
    .count()
        > 1
    {
        return Err("conflicting process mode");
    }
    Ok(o)
}
fn lock(dir: &std::path::Path) -> Result<File, &'static str> {
    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(dir.join("service.lock"))
        .map_err(|_| "service lock unavailable")?;
    rustix::fs::flock(&f, rustix::fs::FlockOperation::NonBlockingLockExclusive)
        .map_err(|_| "control service already running")?;
    Ok(f)
}
fn init(o: &Options) -> Result<(), &'static str> {
    std::fs::create_dir_all(&o.dir).map_err(|_| "state directory unavailable")?;
    std::fs::set_permissions(&o.dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|_| "state permissions unavailable")?;
    let _lock = lock(&o.dir)?;
    if o.dir.join("auth.json").exists() {
        return Err("existing authentication state must not be overwritten");
    }
    let password = format!("{}Aa7!", &token()[..24]);
    let auth = Auth {
        username: "admin".into(),
        hash: bcrypt::hash(&password, 12).map_err(|_| "password initialization failed")?,
        version: token(),
        ..Default::default()
    };
    let data = Data {
        schema: 2,
        site: Site {
            name: "羽迹探针".into(),
            public: true,
            refresh_seconds: 3,
        },
        ..Default::default()
    };
    atomic_json(&o.dir.join("auth.json"), &auth).map_err(|_| "state initialization failed")?;
    atomic_json(&o.dir.join("nodes.json"), &data).map_err(|_| "state initialization failed")?;
    atomic_json(
        &o.dir.join("admin-initial.json"),
        &serde_json::json!({"username":"admin","password":password,"url":o.origin}),
    )
    .map_err(|_| "state initialization failed")?;
    println!("Administrator initialized; credentials saved to private admin-initial.json");
    Ok(())
}
fn daemon(o: &Options) -> Result<(), &'static str> {
    if !o.dir.join("auth.json").is_file() {
        return Err("authentication state unavailable");
    }
    let exe = std::env::current_exe().map_err(|_| "executable unavailable")?;
    let log = OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(o.dir.join("service.log"))
        .map_err(|_| "service log unavailable")?;
    Command::new(&exe)
        .args(o.args())
        .arg("-supervise")
        .current_dir(exe.parent().unwrap())
        .stdin(Stdio::null())
        .stdout(log.try_clone().map_err(|_| "service log unavailable")?)
        .stderr(log)
        .spawn()
        .map_err(|_| "background startup failed")?;
    Ok(())
}
async fn signal() {
    let Ok(mut term) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
    else {
        return;
    };
    tokio::select! {_=term.recv()=>{},_=tokio::signal::ctrl_c()=>{}}
}
async fn supervise(o: &Options) -> Result<(), &'static str> {
    // A gateway wake can race the previous supervisor's graceful shutdown.
    // Retry the exclusive lock briefly; never run two workers for one state directory.
    let mut attempts = 0;
    let _lock = loop {
        match lock(&o.dir) {
            Ok(file) => break file,
            Err("control service already running") if attempts < 20 => {
                attempts += 1;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(error) => return Err(error),
        }
    };
    let pid = o.dir.join("supervisor.pid");
    atomic_bytes(&pid, std::process::id().to_string().as_bytes())
        .map_err(|_| "supervisor state unavailable")?;
    let exe = std::env::current_exe().map_err(|_| "executable unavailable")?;
    let mut delay = 1;
    let signal = signal();
    tokio::pin!(signal);
    let result=async{loop{let start=std::time::Instant::now();let mut child=tokio::process::Command::new(&exe).args(o.args()).arg("-worker").stdin(Stdio::null()).kill_on_drop(true).spawn().map_err(|_|"worker startup failed")?;tokio::select!{_=&mut signal=>{if let Some(id)=child.id().and_then(|v|rustix::process::Pid::from_raw(v as i32)){let _=rustix::process::kill_process(id,rustix::process::Signal::TERM);}
if tokio::time::timeout(Duration::from_secs(5),child.wait()).await.is_err(){let _=child.kill().await;}return Ok(())},_=child.wait()=>{}}if start.elapsed()>Duration::from_secs(60){delay=1;}tokio::select!{_=&mut signal=>return Ok(()),_=tokio::time::sleep(Duration::from_secs(delay))=>{}}delay=(delay*2).min(32);}}.await;
    let _ = std::fs::remove_file(pid);
    result
}
fn run() -> Result<(), &'static str> {
    let o = options()?;
    if o.web || o.install {
        std::fs::create_dir_all(&o.dir).map_err(|_| "state directory unavailable")?;
        std::fs::set_permissions(&o.dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| "state directory permissions unavailable")?;
    }
    if o.install {
        use std::io::Read;
        let _lock = lock(&o.dir)?;
        let mut bytes = zeroize::Zeroizing::new(Vec::new());
        std::io::stdin()
            .take(4097)
            .read_to_end(&mut bytes)
            .map_err(|_| "installation input unavailable")?;
        if bytes.len() > 4096 {
            return Err("installation input too large");
        }
        let input = serde_json::from_slice(&bytes).map_err(|_| "installation input invalid")?;
        setup::initialize(&o.dir, input)?;
        println!("Installation complete. Open the configured panel address to sign in.");
        return Ok(());
    }
    if o.reset_mfa {
        return reset_mfa(&o);
    }
    if o.init {
        return init(&o);
    }
    if o.daemon {
        return daemon(&o);
    }
    if o.supervise {
        rustix::process::setsid().map_err(|_| "cannot detach supervisor")?;
    }
    // Raise only this process's soft descriptor limit, within the administrator's hard limit.
    let limit = rustix::process::getrlimit(rustix::process::Resource::Nofile);
    let wanted = limit.maximum.unwrap_or(8192).min(8192);
    if limit.current.is_some_and(|current| current < wanted) {
        let _ = rustix::process::setrlimit(
            rustix::process::Resource::Nofile,
            rustix::process::Rlimit {
                current: Some(wanted),
                maximum: limit.maximum,
            },
        );
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(4)
        .enable_all()
        .build()
        .map_err(|_| "runtime unavailable")?;
    runtime.block_on(async {
        if o.supervise {
            return supervise(&o).await;
        }
        let _lock = if o.worker { None } else { Some(lock(&o.dir)?) };
        if o.web {
            setup::recover(&o.dir)?;
            if !o.dir.join("auth.json").exists() && o.origin.starts_with("http://") {
                return Err("HTTP installation requires private --install initialization");
            }
            if !o.dir.join("auth.json").exists() {
                setup::serve(&o.dir, &o.origin, o.listen).await?;
            }
            if !o.dir.join("auth.json").exists() {
                return Ok(());
            }
        }
        let app = App::new(o.dir, o.origin, o.gateway)?;
        let stop = app.0.stop.clone();
        tokio::spawn(async move {
            signal().await;
            stop.cancel();
        });
        let result = http::serve(app.clone(), o.listen).await;
        app.0.stop.cancel();
        tokio::time::sleep(Duration::from_millis(200)).await;
        result
    })
}
fn main() {
    if let Err(message) = run() {
        eprintln!("{message}");
        std::process::exit(1)
    }
}

fn reset_mfa(o: &Options) -> Result<(), &'static str> {
    use std::{
        io::{BufRead, Read, Write},
        os::unix::fs::MetadataExt,
    };
    if !rustix::process::geteuid().is_root() {
        return Err("Run this recovery command as root");
    }
    let _lock = lock(&o.dir)?;
    let path = o.dir.join("auth.json");
    let meta = std::fs::metadata(&path).map_err(|_| "authentication state unavailable")?;
    let mut auth: Auth = read_json(&path).map_err(|_| "authentication state unavailable")?;
    eprint!("Type RESET MFA to remove the authenticator and all recovery codes: ");
    std::io::stderr()
        .flush()
        .map_err(|_| "terminal unavailable")?;
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .take(64)
        .read_line(&mut line)
        .map_err(|_| "confirmation unavailable")?;
    if line.trim() != "RESET MFA" {
        return Err("Recovery cancelled");
    }
    auth.mfa.clear();
    auth.mfa_last = 0;
    auth.recovery.clear();
    auth.version = token();
    atomic_json(&path, &auth).map_err(|_| "authentication update failed")?;
    rustix::fs::chown(
        &path,
        Some(rustix::process::Uid::from_raw(meta.uid())),
        Some(rustix::process::Gid::from_raw(meta.gid())),
    )
    .map_err(|_| "authentication ownership update failed")?;
    println!(
        "Two-factor authentication reset. Start the controller and sign in with the existing password."
    );
    Ok(())
}
