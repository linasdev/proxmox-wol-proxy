use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct PwpTargetSettings {
    pub name: String,
    pub vm_id: u32,
    pub proxy_url: Option<String>,
}
