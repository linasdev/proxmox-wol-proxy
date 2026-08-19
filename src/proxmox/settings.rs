use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct PwpProxmoxSettings {
    pub mac_address: String,
    pub broadcast_address: String,
    pub node_name: String,
    pub base_url: String,
    pub token_id: String,
    pub token_secret: String,

    #[serde(default)]
    pub certificate_fingerprint: Option<String>,

    #[serde(default = "default_connect_timeout_ms")]
    pub connect_timeout_ms: u64,

    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
}

fn default_connect_timeout_ms() -> u64 {
    500
}

fn default_timeout_ms() -> u64 {
    500
}
