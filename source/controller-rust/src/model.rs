use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;
use vistart_probe_agent::wire::NetworkInterface;

pub fn null_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Resource {
    #[serde(rename = "used")]
    pub used: Option<f64>,
    #[serde(rename = "total")]
    pub total: f64,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Disk {
    #[serde(rename = "name")]
    pub name: String,
    #[serde(rename = "used")]
    pub used: Option<f64>,
    #[serde(rename = "total")]
    pub total: f64,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PublicNode {
    pub group: String,
    pub pinned: bool,
    pub order: u32,
    #[serde(rename = "networkAvailable")]
    pub network_available: bool,
    #[serde(rename = "network", deserialize_with = "null_default")]
    pub network: Vec<NetworkInterface>,
    #[serde(rename = "latencyMs")]
    pub latency_ms: Option<f64>,
    #[serde(rename = "id")]
    pub id: String,
    #[serde(rename = "name")]
    pub name: String,
    #[serde(rename = "country")]
    pub country: String,
    #[serde(rename = "city")]
    pub city: String,
    #[serde(rename = "code")]
    pub code: String,
    #[serde(rename = "online")]
    pub online: bool,
    #[serde(rename = "pending")]
    pub pending: bool,
    #[serde(rename = "cpuModel")]
    pub cpu_model: String,
    #[serde(rename = "cores")]
    pub cores: i64,
    #[serde(rename = "arch")]
    pub arch: String,
    #[serde(rename = "system")]
    pub system: String,
    #[serde(rename = "cpu")]
    pub cpu: Option<f64>,
    #[serde(rename = "memory")]
    pub memory: Resource,
    #[serde(rename = "swap")]
    pub swap: Option<Resource>,
    #[serde(rename = "disks", deserialize_with = "null_default")]
    pub disks: Vec<Disk>,
    #[serde(rename = "uptime")]
    pub uptime: Option<String>,
    #[serde(rename = "lastSeenMinutes")]
    pub last_seen_minutes: i64,
    #[serde(rename = "history", deserialize_with = "null_default")]
    pub history: Vec<f64>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Node {
    #[serde(rename = "renewalCycle")]
    pub renewal_cycle: crate::billing::RenewalCycle,
    #[serde(rename = "renewalAnchorDay")]
    pub renewal_anchor_day: u32,
    #[serde(rename = "renewalAmount")]
    pub renewal_amount: String,
    #[serde(rename = "renewalCurrency")]
    pub renewal_currency: String,
    pub notes: String,
    pub removing: bool,
    #[serde(rename = "providerName")]
    pub provider_name: String,
    #[serde(rename = "providerURL")]
    pub provider_url: String,
    #[serde(rename = "expiresAt")]
    pub expires_at: String,
    #[serde(rename = "notifyRenewal")]
    pub notify_renewal: bool,
    #[serde(rename = "renewalVersion")]
    pub renewal_version: String,
    #[serde(rename = "agentVersion")]
    pub agent_version: String,
    #[serde(rename = "lastSeen")]
    pub last_seen: i64,
    #[serde(rename = "countryAuto")]
    pub country_auto: bool,
    #[serde(rename = "detectedIP")]
    pub detected_ip: String,
    #[serde(rename = "agentPeerIP", skip_serializing_if = "String::is_empty")]
    pub agent_peer_ip: String,
    #[serde(rename = "deployState")]
    pub deploy_state: String,
    #[serde(rename = "deployMessage")]
    pub deploy_message: String,
    #[serde(rename = "public")]
    pub public: PublicNode,
    #[serde(rename = "ip")]
    pub ip: String,
    #[serde(rename = "port")]
    pub port: i64,
    #[serde(rename = "username")]
    pub username: String,
    #[serde(rename = "visible")]
    pub visible: bool,
    #[serde(rename = "demo")]
    pub demo: bool,
    #[serde(skip)]
    pub latency_at: i64,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Site {
    #[serde(rename = "name")]
    pub name: String,
    #[serde(rename = "public")]
    pub public: bool,
    #[serde(rename = "refreshSeconds")]
    pub refresh_seconds: i64,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Data {
    pub theme: crate::theme::Theme,
    #[serde(rename = "commands", deserialize_with = "null_default")]
    pub commands: Vec<SavedCommand>,
    #[serde(rename = "preview")]
    pub preview: bool,
    #[serde(rename = "secrets", deserialize_with = "null_default")]
    pub secrets: HashMap<String, NodeSecret>,
    #[serde(rename = "schema")]
    pub schema: i64,
    #[serde(rename = "site")]
    pub site: Site,
    #[serde(rename = "nodes", deserialize_with = "null_default")]
    pub nodes: Vec<Node>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Auth {
    pub recovery: Vec<String>,
    #[serde(rename = "mfa")]
    pub mfa: String,
    #[serde(rename = "mfa_last")]
    pub mfa_last: i64,
    #[serde(rename = "username")]
    pub username: String,
    #[serde(rename = "hash")]
    pub hash: String,
    #[serde(rename = "version")]
    pub version: String,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Audit {
    pub source: String,
    pub result: String,
    #[serde(rename = "at")]
    pub at: String,
    #[serde(rename = "action")]
    pub action: String,
    #[serde(rename = "subject")]
    pub subject: String,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct NodeSecret {
    pub recovery_key: String,
    pub derived_recovery: bool,
    pub recovery_public: String,
    #[serde(rename = "token_hash")]
    pub token_hash: String,
    #[serde(rename = "token")]
    pub token: String,
    #[serde(rename = "ssh_key")]
    pub ssh_key: String,
    #[serde(rename = "public_key")]
    pub public_key: String,
    #[serde(rename = "host_key", skip_serializing_if = "String::is_empty")]
    pub host_key: String,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedCommand {
    #[serde(rename = "id")]
    pub id: String,
    #[serde(rename = "name")]
    pub name: String,
    #[serde(rename = "script")]
    pub script: String,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DeployJob {
    #[serde(rename = "id")]
    pub id: String,
    #[serde(rename = "nodeId")]
    pub node_id: String,
    #[serde(rename = "state")]
    pub state: String,
    #[serde(rename = "message")]
    pub message: String,
    #[serde(rename = "started")]
    pub started: i64,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ReleaseFile {
    #[serde(rename = "name")]
    pub name: String,
    #[serde(rename = "sha256")]
    pub sha256: String,
    #[serde(rename = "size")]
    pub size: i64,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Release {
    #[serde(rename = "version")]
    pub version: String,
    #[serde(rename = "files", deserialize_with = "null_default")]
    pub files: HashMap<String, ReleaseFile>,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SignedRelease {
    #[serde(rename = "payload")]
    pub payload: String,
    #[serde(rename = "signature")]
    pub signature: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TelegramConfig {
    #[serde(rename = "notifyLogin")]
    pub login: bool,
    #[serde(rename = "notifyRenewal")]
    pub renewal: bool,
    #[serde(rename = "enabled")]
    pub enabled: bool,
    #[serde(rename = "tokenEncrypted")]
    pub token: String,
    #[serde(rename = "chatId")]
    pub chat_id: String,
    #[serde(rename = "notifyOnline")]
    pub online: bool,
    #[serde(rename = "notifyOffline")]
    pub offline: bool,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TelegramNodeState {
    #[serde(default)]
    pub was_offline: bool,
    #[serde(rename = "online")]
    pub online: bool,
    #[serde(rename = "offlineSince")]
    pub offline_since: i64,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RenewalMember {
    #[serde(rename = "renewalAmount")]
    pub renewal_amount: String,
    #[serde(rename = "renewalCurrency")]
    pub renewal_currency: String,
    #[serde(rename = "nodeId")]
    pub node_id: String,
    #[serde(rename = "name")]
    pub name: String,
    #[serde(rename = "expiresAt")]
    pub expires_at: String,
    #[serde(rename = "renewalVersion")]
    pub renewal_version: String,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TelegramEvent {
    #[serde(default)]
    pub group: bool,
    #[serde(default)]
    pub recovery: bool,
    #[serde(rename = "renewalDay")]
    pub renewal_day: String,
    #[serde(rename = "renewalNodes", deserialize_with = "null_default")]
    pub renewal_nodes: Vec<RenewalMember>,
    #[serde(rename = "expiresAt")]
    pub expires_at: String,
    #[serde(rename = "renewalVersion")]
    pub renewal_version: String,
    #[serde(rename = "id")]
    pub id: String,
    #[serde(rename = "nodeId")]
    pub node_id: String,
    #[serde(rename = "name")]
    pub name: String,
    #[serde(rename = "site")]
    pub site: String,
    #[serde(rename = "kind")]
    pub kind: String,
    #[serde(rename = "at")]
    pub at: i64,
    #[serde(rename = "attempts")]
    pub attempts: i64,
    #[serde(rename = "nextAttempt")]
    pub next_attempt: i64,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TelegramTestResult {
    #[serde(rename = "id")]
    pub id: String,
    #[serde(rename = "status")]
    pub status: String,
    #[serde(rename = "error")]
    pub error: String,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TelegramGroup {
    pub enabled: bool,
    pub chat_id: String,
    pub title: String,
    pub verified_at: i64,
    pub last_success: i64,
    pub last_error: String,
    pub last_test_at: i64,
    pub test: TelegramTestResult,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TelegramState {
    pub group: TelegramGroup,
    #[serde(rename = "renewals", deserialize_with = "null_default")]
    pub renewals: HashMap<String, String>,
    #[serde(rename = "config")]
    pub config: TelegramConfig,
    #[serde(rename = "nodes", deserialize_with = "null_default")]
    pub nodes: HashMap<String, TelegramNodeState>,
    #[serde(rename = "queue", deserialize_with = "null_default")]
    pub queue: Vec<TelegramEvent>,
    #[serde(rename = "lastSuccess")]
    pub last_success: i64,
    #[serde(rename = "lastError")]
    pub last_error: String,
    #[serde(rename = "lastTestAt")]
    pub last_test_at: i64,
    #[serde(rename = "test")]
    pub test: TelegramTestResult,
    #[serde(rename = "dropped")]
    pub dropped: i64,
}

impl Default for TelegramConfig {
    fn default() -> Self {
        Self {
            login: false,
            enabled: false,
            token: String::new(),
            chat_id: String::new(),
            online: true,
            offline: true,
            renewal: true,
        }
    }
}
