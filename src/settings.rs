use crate::proxmox::settings::PwpProxmoxSettings;
use crate::target::settings::PwpTargetSettings;
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct PwpSettings {
    pub proxmox: PwpProxmoxSettings,
    pub targets: Vec<PwpTargetSettings>,
    pub mac_address: String,
    pub broadcast_address: String,

    #[serde(default)]
    pub trusted_proxy_addresses: Option<Vec<String>>,

    #[serde(default)]
    pub tls_root_certificate_path: Option<PathBuf>,

    #[serde(default)]
    pub proxy_client_timeout_ms: Option<u64>,

    #[serde(default)]
    pub proxy_client_connect_timeout_ms: Option<u64>,

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
