use crate::Result;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
pub const MAX_MESSAGE: usize = 96 * 1024;
pub const MAX_CHUNK: usize = 16 * 1024;
pub const WINDOW: usize = 16;
// Four UI tabs with separate shell, browsing and transfer streams, plus cleanup/inspection.
pub const MAX_TUNNELS: usize = 16;
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Volume {
    pub name: String,
    pub total: u64,
    pub used: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct NetworkInterface {
    pub name: String,
    pub state: String,
    pub default: bool,
    pub r#virtual: bool,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
    pub rx_rate: Option<f64>,
    pub tx_rate: Option<f64>,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicIps {
    pub ipv4: Option<std::net::Ipv4Addr>,
    pub ipv6: Option<std::net::Ipv6Addr>,
}
impl PublicIps {
    pub fn primary(self) -> String {
        self.ipv4
            .map(|a| a.to_string())
            .or_else(|| self.ipv6.map(|a| a.to_string()))
            .unwrap_or_default()
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Metrics {
    #[serde(default)]
    pub boot_id: String,
    pub network_available: bool,
    pub network: Vec<NetworkInterface>,
    pub latency_probe: bool,
    pub cpu: f64,
    pub cpu_model: String,
    pub cores: usize,
    pub arch: String,
    pub system: String,
    pub memory_total: u64,
    pub memory_used: u64,
    pub swap_total: u64,
    pub swap_used: u64,
    pub volumes: Vec<Volume>,
    pub uptime: u64,
    pub country: String,
    pub public_ip: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub public_ips: Option<PublicIps>,
    pub version: String,
}
fn zero(v: &u64) -> bool {
    *v == 0
}
#[derive(Default, Debug, Serialize, Deserialize)]
pub struct Message {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub session: String,
    #[serde(default, skip_serializing_if = "zero")]
    pub sequence: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metrics: Option<Metrics>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub data: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
}
impl Message {
    pub fn new(kind: &str, session: &str) -> Self {
        Self {
            kind: kind.into(),
            session: session.into(),
            ..Self::default()
        }
    }
    pub fn parse(raw: &[u8]) -> Result<Self> {
        if raw.len() > MAX_MESSAGE {
            return Err("control frame too large");
        }
        let msg: Self = serde_json::from_slice(raw).map_err(|_| "invalid control JSON")?;
        if msg.session.len() > 64
            || msg.kind.len() > 32
            || msg.data.len() > MAX_CHUNK.div_ceil(3) * 4
        {
            return Err("invalid control message");
        }
        if !msg.data.is_empty() {
            msg.decode_data()?;
        }
        Ok(msg)
    }
    pub fn bytes(kind: &str, session: &str, raw: &[u8]) -> Self {
        Self {
            data: STANDARD.encode(raw),
            ..Self::new(kind, session)
        }
    }
    pub fn decode_data(&self) -> Result<Vec<u8>> {
        let bytes = STANDARD
            .decode(&self.data)
            .map_err(|_| "invalid tunnel encoding")?;
        if bytes.len() > MAX_CHUNK {
            return Err("tunnel chunk too large");
        }
        Ok(bytes)
    }
    pub fn encode(&self) -> Result<String> {
        let raw = serde_json::to_string(self).map_err(|_| "control encode failed")?;
        if raw.len() > MAX_MESSAGE {
            return Err("control frame too large");
        }
        Ok(raw)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn go_byte_slice_base64_compatibility() {
        let m=Message::parse(br#"{"type":"ssh_data","session":"0123456789abcdef0123456789abcdef","data":"AAEC//4="}"#).unwrap();
        assert_eq!(m.decode_data().unwrap(), [0, 1, 2, 255, 254]);
        assert_eq!(
            Message::bytes("ssh_data", &m.session, &[0, 1, 2, 255, 254]).data,
            m.data
        );
    }
    #[test]
    fn control_message_limits() {
        assert!(Message::parse(&vec![b' '; MAX_MESSAGE + 1]).is_err());
        assert!(Message::parse(br#"{"type":"ssh_data","data":"invalid!"}"#).is_err());
        let mut m = Message::new("ssh_data", &"s".repeat(65));
        assert!(Message::parse(m.encode().unwrap().as_bytes()).is_err());
        m = Message::bytes("ssh_data", "x", &vec![0; MAX_CHUNK + 1]);
        assert!(Message::parse(m.encode().unwrap().as_bytes()).is_err());
    }
    #[test]
    fn zero_fields_omitted_and_unknown_fields_compatible() {
        assert_eq!(
            Message::new("ack", "").encode().unwrap(),
            r#"{"type":"ack"}"#
        );
        assert_eq!(
            Message::parse(br#"{"type":"ack","sequence":1,"future":true}"#)
                .unwrap()
                .sequence,
            1
        );
    }
}
