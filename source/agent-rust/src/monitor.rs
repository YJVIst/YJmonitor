use crate::{
    Result, VERSION,
    wire::{Metrics, NetworkInterface, Volume},
};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File},
    io::Read,
    os::unix::fs::MetadataExt,
    path::Path,
    time::Instant,
};

pub fn read_text(path: impl AsRef<Path>, limit: u64) -> Result<String> {
    let mut out = String::new();
    File::open(path)
        .map_err(|_| "metric unavailable")?
        .take(limit + 1)
        .read_to_string(&mut out)
        .map_err(|_| "metric unavailable")?;
    if out.len() as u64 > limit {
        return Err("metric too large");
    }
    Ok(out)
}
fn net_text(path: impl AsRef<Path>) -> String {
    read_text(path, 1024 * 1024)
        .unwrap_or_default()
        .trim()
        .into()
}
#[derive(Clone, Copy, Default, Debug)]
struct Cpu {
    total: u64,
    idle: u64,
}
fn parse_cpu(raw: &str) -> Result<Cpu> {
    let fields: Vec<_> = raw
        .lines()
        .next()
        .unwrap_or("")
        .split_whitespace()
        .collect();
    if fields.len() < 9 || fields[0] != "cpu" {
        return Err("cpu unavailable");
    }
    let mut sample = Cpu::default();
    for (i, f) in fields.iter().enumerate().take(9).skip(1) {
        let value = f.parse::<u64>().map_err(|_| "cpu invalid")?;
        sample.total = sample.total.checked_add(value).ok_or("cpu overflow")?;
        if i == 4 || i == 5 {
            sample.idle = sample.idle.checked_add(value).ok_or("cpu overflow")?;
        }
    }
    Ok(sample)
}
fn memory(raw: &str) -> Result<(u64, u64, u64, u64)> {
    let mut values = HashMap::new();
    for line in raw.lines() {
        let mut f = line.split_whitespace();
        if let (Some(k), Some(v)) = (f.next(), f.next()) {
            if let Some(n) = v.parse::<u64>().ok().and_then(|x| x.checked_mul(1024)) {
                values.insert(k.trim_end_matches(':'), n);
            }
        }
    }
    let get = |k: &str| values.get(k).copied().unwrap_or(0);
    let total = get("MemTotal");
    if total == 0 {
        return Err("memory unavailable");
    }
    let available = values
        .get("MemAvailable")
        .copied()
        .unwrap_or_else(|| {
            get("MemFree")
                .saturating_add(get("Buffers"))
                .saturating_add(get("Cached"))
        })
        .min(total);
    let swap = get("SwapTotal");
    Ok((
        total,
        total - available,
        swap,
        swap - get("SwapFree").min(swap),
    ))
}
fn volumes() -> Vec<Volume> {
    let mut mounts = vec!["/".to_string()];
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for line in read_text("/proc/mounts", 4 * 1024 * 1024)
        .unwrap_or_default()
        .lines()
    {
        let f: Vec<_> = line.split_whitespace().collect();
        if f.len() < 3 || !["ext2", "ext3", "ext4", "xfs", "btrfs", "zfs", "vfat"].contains(&f[2]) {
            continue;
        }
        let p = f[1]
            .replace("\\040", " ")
            .replace("\\011", "\t")
            .replace("\\134", "\\");
        if p != "/" {
            mounts.push(p);
        }
    }
    for mount in mounts {
        if out.len() >= 16 {
            break;
        }
        let Ok(info) = fs::metadata(&mount) else {
            continue;
        };
        if seen.contains(&info.dev()) {
            continue;
        }
        let Ok(s) = rustix::fs::statfs(mount.as_str()) else {
            continue;
        };
        let Ok(size) = u64::try_from(s.f_bsize) else {
            continue;
        };
        if s.f_blocks == 0 || size == 0 {
            continue;
        }
        let Some(total) = s.f_blocks.checked_mul(size) else {
            continue;
        };
        let free = s.f_bfree.saturating_mul(size).min(total);
        seen.insert(info.dev());
        out.push(Volume {
            name: if out.is_empty() {
                "系统盘".into()
            } else {
                format!("数据盘 {}", out.len())
            },
            total,
            used: total - free,
        });
    }
    out
}
#[derive(Clone, Debug)]
struct Counters {
    rx: u64,
    tx: u64,
    index: String,
}
struct NetSample {
    counts: Counters,
    at: Instant,
}
fn valid_interface(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 15
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.:@-".contains(&b))
}
fn parse_network(raw: &str) -> HashMap<String, Counters> {
    let mut result = HashMap::new();
    for line in raw.lines() {
        let Some((name, rest)) = line.rsplit_once(':') else {
            continue;
        };
        let name = name.trim();
        if name == "lo" || !valid_interface(name) {
            continue;
        }
        let f: Vec<_> = rest.split_whitespace().collect();
        if f.len() != 16 {
            continue;
        }
        if let (Ok(rx), Ok(tx)) = (f[0].parse::<u64>(), f[8].parse::<u64>()) {
            result.insert(
                name.into(),
                Counters {
                    rx,
                    tx,
                    index: String::new(),
                },
            );
        }
    }
    result
}
fn default_interfaces(v4: &str, v6: &str) -> HashSet<String> {
    let mut result = HashSet::new();
    let mut best = u64::MAX;
    let mut selected = "";
    for line in v4.lines() {
        let f: Vec<_> = line.split_whitespace().collect();
        if f.len() < 8 || f[1] != "00000000" || f[7] != "00000000" || f[0] == "lo" {
            continue;
        }
        if let (Ok(flags), Ok(metric)) = (u64::from_str_radix(f[3], 16), f[6].parse::<u64>()) {
            if flags & 1 != 0 && flags & 0x200 == 0 && metric < best {
                best = metric;
                selected = f[0];
            }
        }
    }
    if !selected.is_empty() {
        result.insert(selected.into());
    }
    best = u64::MAX;
    selected = "";
    for line in v6.lines() {
        let f: Vec<_> = line.split_whitespace().collect();
        if f.len() != 10
            || f[0] != "00000000000000000000000000000000"
            || f[1] != "00"
            || f[9] == "lo"
        {
            continue;
        }
        if let (Ok(flags), Ok(metric)) =
            (u64::from_str_radix(f[8], 16), u64::from_str_radix(f[5], 16))
        {
            if flags & 1 != 0 && flags & 0x200 == 0 && metric < best {
                best = metric;
                selected = f[9];
            }
        }
    }
    if !selected.is_empty() {
        result.insert(selected.into());
    }
    result
}
fn rate(now: u64, old: u64, seconds: f64, same: bool) -> Option<f64> {
    if !same || seconds <= 0.0 || now < old {
        return None;
    }
    let value = (now - old) as f64 / seconds;
    if !value.is_finite() || value > 1e14 {
        None
    } else {
        Some(value)
    }
}
pub struct Collector {
    previous: Cpu,
    network_previous: HashMap<String, NetSample>,
    model: String,
    system: String,
    cores: usize,
}
impl Default for Collector {
    fn default() -> Self {
        Self::new()
    }
}
impl Collector {
    pub fn new() -> Self {
        let stat = read_text("/proc/stat", 4 * 1024 * 1024).unwrap_or_default();
        let mut model = format!("{} CPU", architecture());
        let mut system = "Linux".into();
        for line in read_text("/proc/cpuinfo", 8 * 1024 * 1024)
            .unwrap_or_default()
            .lines()
        {
            if let Some((k, v)) = line.split_once(':') {
                if k.trim() == "model name" {
                    model = v.trim().into();
                    break;
                }
            }
        }
        for line in read_text("/etc/os-release", 65536)
            .unwrap_or_default()
            .lines()
        {
            if let Some(v) = line.strip_prefix("PRETTY_NAME=") {
                system = v.trim_matches(['\'', '"']).into();
                break;
            }
        }
        let cores = stat
            .lines()
            .filter(|x| {
                x.strip_prefix("cpu")
                    .is_some_and(|v| v.as_bytes().first().is_some_and(u8::is_ascii_digit))
            })
            .count()
            .clamp(1, 8192);
        Self {
            previous: parse_cpu(&stat).unwrap_or_default(),
            network_previous: HashMap::new(),
            model,
            system,
            cores,
        }
    }
    fn network(&mut self, now: Instant) -> (Vec<NetworkInterface>, bool) {
        let Ok(raw) = read_text("/proc/net/dev", 4 * 1024 * 1024) else {
            self.network_previous.clear();
            return (vec![], false);
        };
        let defaults = default_interfaces(
            &net_text("/proc/net/route"),
            &net_text("/proc/net/ipv6_route"),
        );
        let mut out = Vec::new();
        let mut next = HashMap::new();
        for (name, mut value) in parse_network(&raw) {
            let root = Path::new("/sys/class/net").join(&name);
            value.index = net_text(root.join("ifindex"));
            let mut state = net_text(root.join("operstate"));
            if ![
                "up",
                "down",
                "unknown",
                "dormant",
                "lowerlayerdown",
                "notpresent",
                "testing",
            ]
            .contains(&state.as_str())
            {
                state = "unknown".into();
            }
            let old = self.network_previous.get(&name);
            let same =
                old.is_some_and(|p| p.counts.index == value.index && !value.index.is_empty());
            let seconds = old
                .map(|p| now.duration_since(p.at).as_secs_f64())
                .unwrap_or(0.0);
            out.push(NetworkInterface {
                name: name.clone(),
                state,
                default: defaults.contains(&name),
                r#virtual: fs::metadata(Path::new("/sys/devices/virtual/net").join(&name)).is_ok(),
                rx_bytes: value.rx,
                tx_bytes: value.tx,
                rx_rate: old.and_then(|p| rate(value.rx, p.counts.rx, seconds, same)),
                tx_rate: old.and_then(|p| rate(value.tx, p.counts.tx, seconds, same)),
            });
            next.insert(
                name,
                NetSample {
                    counts: value,
                    at: now,
                },
            );
        }
        self.network_previous = next;
        out.sort_by(|a, b| {
            b.default
                .cmp(&a.default)
                .then(a.r#virtual.cmp(&b.r#virtual))
                .then(a.name.cmp(&b.name))
        });
        out.truncate(64);
        (out, true)
    }
    pub fn sample(&mut self, country: &crate::country::Country) -> Result<Metrics> {
        let now = parse_cpu(&read_text("/proc/stat", 4 * 1024 * 1024)?)?;
        let mut usage = 0.0;
        if now.total > self.previous.total && now.idle >= self.previous.idle {
            let total = now.total - self.previous.total;
            let idle = now.idle - self.previous.idle;
            if idle <= total {
                usage = 100.0 * (total - idle) as f64 / total as f64;
            }
        }
        self.previous = now;
        let (memory_total, memory_used, swap_total, swap_used) =
            memory(&read_text("/proc/meminfo", 1024 * 1024)?)?;
        let uptime = net_text("/proc/uptime")
            .split_whitespace()
            .next()
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|v| v.is_finite() && *v >= 0.0)
            .unwrap_or(0.0) as u64;
        let (network, network_available) = self.network(Instant::now());
        Ok(Metrics {
            boot_id: std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
                .unwrap_or_default()
                .trim()
                .to_string(),
            network_available,
            network,
            latency_probe: true,
            cpu: (usage * 10.0).round() / 10.0,
            cpu_model: self.model.clone(),
            cores: self.cores,
            arch: architecture().into(),
            system: self.system.clone(),
            memory_total,
            memory_used,
            swap_total,
            swap_used,
            volumes: volumes(),
            uptime,
            country: country.code.clone(),
            public_ip: country.addresses.unwrap_or_default().primary(),
            public_ips: country.addresses,
            version: VERSION.into(),
        })
    }
}
fn architecture() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        x => x,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cpu_accounting_excludes_double_counted_guest() {
        let c = parse_cpu("cpu 100 20 30 400 50 6 7 8 90 10\ncpu0 0").unwrap();
        assert_eq!(c.total, 621);
        assert_eq!(c.idle, 450);
        assert!(parse_cpu("cpu -1 2 3 4 5 6 7 8").is_err());
    }
    #[test]
    fn memory_available_fallback_and_swap() {
        assert_eq!(
            memory("MemTotal: 1000 kB\nMemAvailable: 400 kB\nSwapTotal: 200 kB\nSwapFree: 100 kB")
                .unwrap(),
            (1024000, 614400, 204800, 102400)
        );
        assert_eq!(
            memory("MemTotal: 100 kB\nMemFree: 20 kB\nBuffers: 10 kB\nCached: 30 kB").unwrap(),
            (102400, 40960, 0, 0)
        );
    }
    #[test]
    fn invalid_memory_does_not_wrap() {
        assert!(memory("MemTotal: 18446744073709551615 kB").is_err());
        assert_eq!(
            memory("MemTotal: 1 kB\nMemAvailable: 5 kB\nSwapTotal: 2 kB\nSwapFree: 3 kB").unwrap(),
            (1024, 0, 2048, 0)
        );
    }
    #[test]
    fn network_names_and_counters() {
        let n = parse_network(
            " lo: 42 0 0 0 0 0 0 0 42 0 0 0 0 0 0 0\n ens3: 4096 1 0 0 0 0 0 0 8192 1 0 0 0 0 0 0\n ../../bad: 1 0 0 0 0 0 0 0 2 0 0 0 0 0 0 0\n invalid: -1 0 0 0 0 0 0 0 5 0 0 0 0 0 0 0\n",
        );
        assert_eq!(n.len(), 1);
        assert_eq!(n["ens3"].rx, 4096);
        assert_eq!(n["ens3"].tx, 8192);
    }
    #[test]
    fn network_reset_and_replacement_never_spike() {
        assert_eq!(rate(4096, 2048, 2.0, true), Some(1024.0));
        for r in [
            rate(4, 5, 1.0, true),
            rate(5, 4, 0.0, true),
            rate(5, 4, 1.0, false),
            rate(u64::MAX, 0, 0.001, true),
        ] {
            assert_eq!(r, None);
        }
    }
    #[test]
    fn ipv4_ipv6_default_routes() {
        let d = default_interfaces(
            "eth0 00000000 0100000A 0003 0 0 100 00000000\neth1 00000000 0100000A 0003 0 0 20 00000000\n",
            "00000000000000000000000000000000 00 00000000000000000000000000000000 00 fe800000000000000000000000000001 00000010 00000000 00000000 00000003 ens6\n",
        );
        assert_eq!(d, HashSet::from(["eth1".into(), "ens6".into()]));
    }
    #[test]
    fn real_metrics_valid_and_network_first_sample_unknown() {
        let mut c = Collector::new();
        let m = c.sample(&Default::default()).unwrap();
        assert!(m.memory_total > 0 && m.memory_used <= m.memory_total);
        assert!(m.cores > 0);
        assert!(m.network.len() <= 64);
        for n in m.network {
            assert_eq!(n.rx_rate, None);
            assert_eq!(n.tx_rate, None);
        }
    }
}
