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

    #[serde(default = "default_connect_timeout_ms")]
    pub connect_timeout_ms: u64,

    #[serde(default)]
    pub accept_invalid_certs: bool,

    #[serde(default)]
    pub allow_insecure_http: bool,
}

fn default_connect_timeout_ms() -> u64 {
    500
}
