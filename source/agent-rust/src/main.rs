#![forbid(unsafe_code)]
use std::{path::PathBuf, time::Duration};
use tokio::{
    sync::watch,
    time::{Instant, MissedTickBehavior},
};
use tokio_util::sync::CancellationToken;
use vistart_probe_agent::{
    Result, VERSION, config::Config, connection, country, monitor::Collector, wire::Metrics,
};
fn config_path() -> Result<Option<PathBuf>> {
    let mut args = std::env::args_os().skip(1);
    let mut path = PathBuf::from("/var/lib/vistart-probe-agent/config.json");
    while let Some(arg) = args.next() {
        if arg == "--version" || arg == "-version" {
            println!("vistart-probe-agent {VERSION} (Rust)");
            return Ok(None);
        }
        if arg == "-config" || arg == "--config" {
            path = args.next().ok_or("configuration path required")?.into();
        } else {
            return Err("usage: vistart-probe-agent [-config PATH] [--version]");
        }
    }
    Ok(Some(path))
}
fn main() {
    let code = run_main();
    if let Err(error) = code {
        eprintln!("Agent stopped: {error}");
        std::process::exit(1);
    }
}
fn run_main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).is_some_and(|s| s == "--manage") {
        if args.len() != 4 || args[2] != "-config" {
            return Err("usage: --manage -config PATH");
        }
        return vistart_probe_agent::management::run(std::path::Path::new(&args[3]));
    }
    let Some(path) = config_path()? else {
        return Ok(());
    };
    let config = Config::load(&path)?;
    let tls = connection::tls_config()?;
    tokio::runtime::Builder::new_current_thread().enable_all().max_blocking_threads(2).thread_stack_size(512*1024)
        .build().map_err(|_|"runtime unavailable")?.block_on(async move{
        let stop=CancellationToken::new();let shutdown=stop.clone();
        let mut term=tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).map_err(|_|"signal handler unavailable")?;
        tokio::spawn(async move{tokio::select!{_=tokio::signal::ctrl_c()=>{},_=term.recv()=>{}}shutdown.cancel();});
        let(country_tx,country_rx)=watch::channel(country::Country::default());
        let country_task=tokio::spawn(country::run(tls.clone(),country_tx,stop.clone()));
        let(metrics_tx,metrics_rx)=watch::channel::<Option<Metrics>>(None);
        let sample_stop=stop.clone();
        let sample_task=tokio::spawn(async move{
            let mut collector=Collector::new();
            let mut ticker=tokio::time::interval_at(Instant::now()+Duration::from_secs(3),Duration::from_secs(3));ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop{
                tokio::select!{_=sample_stop.cancelled()=>return,_=ticker.tick()=>{}}
                let country=country_rx.borrow().clone();
                let job=tokio::task::spawn_blocking(move||{let value=collector.sample(&country);(collector,value)});
                let Ok((next,value))=job.await else{sample_stop.cancel();return;};collector=next;
                if let Ok(value)=value{metrics_tx.send_replace(Some(value));}
            }
        });
        let (health_tx,health_rx)=watch::channel(None);
        let health_config=config.clone();
        let health_task=tokio::spawn(async move {
            vistart_probe_agent::management::report_health(path,&health_config,health_rx).await;
        });
        let mut generation=0;
        let mut delay=Duration::from_secs(1);
        loop{
            generation+=1;
            let started=Instant::now();
            tokio::select!{_=stop.cancelled()=>break,_=connection::connect(&config,&health_tx,generation,tls.clone(),metrics_rx.clone(),stop.clone())=>{}}
            if stop.is_cancelled(){break;}
            if started.elapsed()>Duration::from_secs(60){delay=Duration::from_secs(1);}
            let mut random=[0u8;2];
            // This random value only spreads reconnect attempts; it never authenticates anything.
            let jitter=if rustix::rand::getrandom(&mut random,rustix::rand::GetRandomFlags::NONBLOCK).is_ok(){u16::from_ne_bytes(random)%1000}else{0};
            eprintln!("Control channel disconnected; retry scheduled");
            tokio::select!{_=stop.cancelled()=>break,_=tokio::time::sleep(delay+Duration::from_millis(u64::from(jitter)))=>{}}
            delay=(delay*2).min(Duration::from_secs(32));
        }
        drop(health_tx);let _=health_task.await;
        stop.cancel();country_task.abort();sample_task.abort();let _=country_task.await;let _=sample_task.await;Ok(())
    })
}
