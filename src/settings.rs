use crate::proxmox::settings::PwpProxmoxSettings;
use crate::target::settings::PwpTargetSettings;
use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct PwpSettings {
    pub proxmox: PwpProxmoxSettings,
    pub targets: Vec<PwpTargetSettings>,
    pub mac_address: String,
    pub broadcast_address: String,
}
