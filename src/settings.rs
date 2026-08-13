use crate::manager::settings::PwpManagerSettings;
use crate::proxmox::settings::PwpProxmoxSettings;
use crate::target::settings::PwpTargetSettings;
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct PwpSettings {
    pub manager: PwpManagerSettings,
    pub proxmox: PwpProxmoxSettings,
    pub targets: Vec<PwpTargetSettings>,

    #[serde(default)]
    pub client: PwpClientSettings,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct PwpClientSettings {
    #[serde(default)]
    pub tls_root_certificate_path: Option<PathBuf>,

    #[serde(default)]
    pub proxy_client_connect_timeout_ms: Option<u64>,

    #[serde(default)]
    pub proxy_client_timeout_ms: Option<u64>,
}

impl Default for PwpClientSettings {
    fn default() -> Self {
        Self {
            tls_root_certificate_path: None,
            proxy_client_connect_timeout_ms: None,
            proxy_client_timeout_ms: None,
        }
    }
}
