use crate::{Result, VERSION, wire::PublicIps};
use std::{
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
#[derive(Clone, Default)]
pub struct Country {
    pub code: String,
    pub addresses: Option<PublicIps>,
}
#[derive(Default)]
struct Observation {
    value: Option<(String, IpAddr, Instant)>,
}
impl Observation {
    fn update(&mut self, result: Result<(String, IpAddr)>, now: Instant) {
        if let Ok((country, ip)) = result {
            self.value = Some((country, ip, now));
        } else if self
            .value
            .as_ref()
            .is_some_and(|v| now.duration_since(v.2) >= Duration::from_secs(1800))
        {
            self.value = None;
        }
    }
}
pub fn normalize(code: &str) -> Option<String> {
    let code = code.trim().to_ascii_uppercase();
    let code = if code == "UK" { "GB" } else { code.as_str() };
    if include_str!("country_codes.txt")
        .split_whitespace()
        .any(|v| v == code)
    {
        Some(code.into())
    } else {
        None
    }
}
pub fn parse_response(raw: &[u8], trace: bool) -> Result<(String, IpAddr)> {
    if raw.len() > 8192 {
        return Err("country response too large");
    }
    let (mut code, mut ip) = (String::new(), String::new());
    if trace {
        for line in std::str::from_utf8(raw)
            .map_err(|_| "country response invalid")?
            .lines()
        {
            if let Some((k, v)) = line.split_once('=') {
                match k {
                    "loc" => code = v.into(),
                    "ip" => ip = v.into(),
                    _ => {}
                }
            }
        }
    } else {
        #[derive(serde::Deserialize)]
        struct Response {
            country: String,
            ip: String,
        }
        let v: Response = serde_json::from_slice(raw).map_err(|_| "country response invalid")?;
        code = v.country;
        ip = v.ip;
    }
    let country = normalize(&code).ok_or("country unavailable")?;
    let ip = ip
        .trim()
        .parse::<IpAddr>()
        .map_err(|_| "public address invalid")?;
    Ok((country, ip))
}
async fn lookup(client: &reqwest::Client, bind: IpAddr) -> Result<(String, IpAddr)> {
    for (endpoint, trace) in [
        ("https://api.country.is/", false),
        ("https://www.cloudflare.com/cdn-cgi/trace", true),
    ] {
        let response = async {
            let mut res = client
                .get(endpoint)
                .send()
                .await
                .map_err(|_| "country lookup unavailable")?;
            if res.status() != reqwest::StatusCode::OK
                || res.content_length().is_some_and(|n| n > 8192)
            {
                return Err("country response invalid");
            }
            let mut raw = Vec::new();
            while let Some(chunk) = res
                .chunk()
                .await
                .map_err(|_| "country lookup unavailable")?
            {
                if raw.len() + chunk.len() > 8192 {
                    return Err("country response too large");
                }
                raw.extend_from_slice(&chunk);
            }
            parse_response(&raw, trace)
        };
        if let Ok(Ok(value)) = tokio::time::timeout(Duration::from_secs(6), response).await {
            if value.1.is_ipv4() == bind.is_ipv4()
                && !matches!(value.1, IpAddr::V6(ip) if ip.to_ipv4_mapped().is_some())
            {
                return Ok(value);
            }
        }
    }
    Err("country lookup unavailable")
}
pub async fn run(
    tls: Arc<rustls::ClientConfig>,
    sender: watch::Sender<Country>,
    cancel: CancellationToken,
) {
    let build = |bind| {
        reqwest::Client::builder()
            .tls_backend_preconfigured((*tls).clone())
            .local_address(bind)
            .https_only(true)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(4))
            .timeout(Duration::from_secs(6))
            .pool_max_idle_per_host(0)
            .user_agent(format!("Vistart-Probe/{VERSION}"))
            .build()
    };
    let v4 = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
    let v6 = IpAddr::V6(Ipv6Addr::UNSPECIFIED);
    let (Ok(client4), Ok(client6)) = (build(v4), build(v6)) else {
        return;
    };
    let (mut last4, mut last6) = (Observation::default(), Observation::default());
    loop {
        let (r4, r6) = tokio::select! {
            _ = cancel.cancelled() => return,
            both = async { tokio::join!(lookup(&client4,v4), lookup(&client6,v6)) } => both,
        };
        let now = Instant::now();
        last4.update(r4, now);
        last6.update(r6, now);
        let mut addresses = PublicIps::default();
        let mut code = String::new();
        for (country, ip, _) in [&last4.value, &last6.value].into_iter().flatten() {
            if code.is_empty() {
                code.clone_from(country);
            }
            match ip {
                IpAddr::V4(v) => addresses.ipv4 = Some(*v),
                IpAddr::V6(v) => addresses.ipv6 = Some(*v),
            }
        }
        sender.send_replace(Country {
            code,
            addresses: Some(addresses),
        });
        tokio::select! {_=cancel.cancelled()=>return,_=tokio::time::sleep(Duration::from_secs(600))=>{}}
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn all_countries_and_alias() {
        assert_eq!(
            include_str!("country_codes.txt").split_whitespace().count(),
            250
        );
        assert_eq!(normalize(" uk "), Some("GB".into()));
        for v in ["SG", "IS", "CW", "XK", "AQ"] {
            assert_eq!(normalize(v), Some(v.into()));
        }
        assert_eq!(normalize("ZZ"), None);
    }
    #[test]
    fn failed_lookup_expires_only_its_family() {
        let start = Instant::now();
        let mut v4 = Observation::default();
        let mut v6 = Observation::default();
        let ip4: IpAddr = "192.0.2.5".parse().unwrap();
        let ip6: IpAddr = "2001:db8::7".parse().unwrap();
        v4.update(Ok(("GB".into(), ip4)), start);
        v6.update(Ok(("GB".into(), ip6)), start);
        v6.update(Err("timeout"), start + Duration::from_secs(600));
        assert_eq!(v6.value.as_ref().unwrap().1, ip6);
        v6.update(Err("timeout"), start + Duration::from_secs(1800));
        assert!(v6.value.is_none());
        assert_eq!(v4.value.unwrap().1, ip4);
        let replacement = "2001:db8::8".parse().unwrap();
        v6.update(
            Ok(("GB".into(), replacement)),
            start + Duration::from_secs(2400),
        );
        assert_eq!(v6.value.unwrap().1, replacement);
    }

    #[test]
    fn validates_country_and_public_address() {
        assert_eq!(
            parse_response(br#"{"country":"sg","ip":"203.0.113.8"}"#, false).unwrap(),
            ("SG".into(), "203.0.113.8".parse().unwrap())
        );
        assert!(parse_response(br#"{"country":"SG","ip":"bad"}"#, false).is_err());
        assert!(parse_response(br#"{"country":"ZZ","ip":"203.0.113.8"}"#, false).is_err());
        assert_eq!(
            parse_response(b"loc=SG\nip=2001:db8::1\n", true).unwrap().1,
            "2001:db8::1".parse::<IpAddr>().unwrap()
        );
        assert!(parse_response(&vec![0; 8193], false).is_err());
    }
}
