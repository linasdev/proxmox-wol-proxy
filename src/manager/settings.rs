use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct PwpManagerSettings {
    #[serde(default)]
    pub trusted_proxy_addresses: Option<Vec<String>>,

    #[serde(default)]
    pub mutually_exclusive_vm_ids: Vec<u32>,

    #[serde(default = "default_reachability_check_delay_ms")]
    pub reachability_check_delay_ms: u64,

    #[serde(default = "default_guest_agent_ping_delay_ms")]
    pub guest_agent_ping_delay_ms: u64,

    #[serde(default = "default_vm_start_timeout_secs")]
    pub vm_start_timeout_secs: u64,

    #[serde(default = "default_wake_on_lan_every_secs")]
    pub wake_on_lan_every_secs: u64,

    #[serde(default = "default_wake_on_lan_attempts")]
    pub wake_on_lan_attempts: u32,
}

fn default_reachability_check_delay_ms() -> u64 {
    500
}

fn default_guest_agent_ping_delay_ms() -> u64 {
    1000
}

fn default_vm_start_timeout_secs() -> u64 {
    30
}

fn default_wake_on_lan_every_secs() -> u64 {
    30
}

fn default_wake_on_lan_attempts() -> u32 {
    3
}
