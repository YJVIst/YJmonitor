use crate::core::{App, acceptable_ip};
use futures_util::{StreamExt, stream};
use std::{
    collections::HashSet,
    net::{IpAddr, SocketAddr},
    time::Duration,
};
use tokio::{
    net::{TcpStream, lookup_host},
    time::timeout,
};

pub fn normalize(input: &str) -> Option<String> {
    let value = input.trim();
    if let Ok(ip) = value.parse::<IpAddr>() {
        return acceptable_ip(value).then(|| ip.to_string());
    }
    let host = value.strip_suffix('.').unwrap_or(value);
    let valid_label = |s: &str| {
        !s.is_empty()
            && s.len() <= 63
            && !s.starts_with('-')
            && !s.ends_with('-')
            && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
    };
    let (_, suffix) = host.rsplit_once('.')?;
    if host.len() > 253
        || !host.split('.').all(valid_label)
        || !suffix.bytes().any(|b| b.is_ascii_alphabetic())
        || suffix.eq_ignore_ascii_case("localhost")
    {
        return None;
    }
    Some(host.to_ascii_lowercase())
}

async fn resolve(host: &str, port: u16) -> Result<Vec<SocketAddr>, &'static str> {
    let host = normalize(host).ok_or("SSH 地址无效")?;
    if let Ok(ip) = host.parse() {
        return Ok(vec![SocketAddr::new(ip, port)]);
    }
    let fqdn = format!("{host}.");
    let addresses = timeout(Duration::from_secs(4), lookup_host((fqdn.as_str(), port)))
        .await
        .map_err(|_| "DNS 解析超时")?
        .map_err(|_| "DNS 解析失败")?;
    let mut result = Vec::new();
    for addr in addresses {
        if acceptable_ip(&addr.ip().to_string()) && !result.contains(&addr) {
            result.push(addr);
        }
    }
    if result.is_empty() {
        return Err("域名没有可用的 SSH 地址");
    }
    Ok(result)
}

pub async fn connect(host: &str, port: u16) -> Result<TcpStream, &'static str> {
    let addresses = resolve(host, port).await?;
    TcpStream::connect(&addresses[..])
        .await
        .map_err(|_| "SSH connection failed")
}

pub async fn monitor(app: App) {
    loop {
        let hosts: HashSet<_> = app
            .lock()
            .data
            .nodes
            .iter()
            .filter(|n| !n.demo && n.ip.parse::<IpAddr>().is_err())
            .map(|n| n.ip.clone())
            .collect();
        app.lock().ssh_dns.retain(|host, _| hosts.contains(host));
        let mut lookups = stream::iter(hosts)
            .map(|host| async move {
                let ips = resolve(&host, 22)
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .map(|addr| addr.ip())
                    .collect();
                (host, ips)
            })
            .buffer_unordered(8);
        while let Some((host, ips)) = lookups.next().await {
            let mut i = app.lock();
            i.ssh_dns.insert(host, ips);
            i.admin_cache = None;
        }
        tokio::select! {
            _ = app.0.stop.cancelled() => return,
            _ = tokio::time::sleep(Duration::from_secs(30)) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_domain_and_literal_addresses() {
        let host = normalize(" My-Router.Example.NET. ").unwrap();
        assert_eq!(host, "my-router.example.net");
        assert_eq!(normalize("2001:0db8::1").as_deref(), Some("2001:db8::1"));
    }

    #[test]
    fn rejects_invalid_names_and_forbidden_addresses() {
        for s in [
            "127.0.0.1",
            "::1",
            "::ffff:127.0.0.1",
            "https://a.example",
            "a.example:22",
            "root@a.example",
            "bad..example",
            "-a.example",
            "a-.example",
            "a.localhost",
            "127.1",
            "999.0.0.1",
        ] {
            assert!(normalize(s).is_none(), "{s}");
        }
        assert!(normalize(&format!("{}.example", "a".repeat(64))).is_none());
    }
}
