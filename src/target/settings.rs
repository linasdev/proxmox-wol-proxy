use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct PwpTargetSettings {
    pub name: String,
    pub vm_id: u32,
    pub default_url: Option<String>,

    #[serde(default)]
    pub max_idle_secs: Option<u64>,

    #[serde(default = "default_should_proxy")]
    pub should_proxy: bool,

    #[serde(default)]
    pub preserve_host_header: bool,
}

fn default_should_proxy() -> bool {
    true
}
