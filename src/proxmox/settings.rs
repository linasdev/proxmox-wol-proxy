use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct PwpProxmoxSettings {
    pub node_name: String,
    pub base_url: String,
    pub token_id: String,
    pub token_secret: String,
}
