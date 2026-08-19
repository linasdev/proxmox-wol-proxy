use crate::manager::settings::PwpManagerSettings;
use crate::proxmox::settings::PwpProxmoxSettings;
use crate::target::settings::PwpTargetSettings;
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct PwpSettings {
    #[serde(default)]
    pub server: PwpServerSettings,

    #[serde(default)]
    pub client: PwpClientSettings,

    pub manager: PwpManagerSettings,
    pub proxmox: PwpProxmoxSettings,
    pub targets: Vec<PwpTargetSettings>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct PwpServerSettings {
    #[serde(default = "default_listen_address")]
    pub listen_address: String,

    #[serde(default = "default_listen_port")]
    pub listen_port: u16,

    #[serde(default)]
    pub tls_certificate_chain_path: Option<PathBuf>,

    #[serde(default)]
    pub tls_private_key_path: Option<PathBuf>,
}

impl Default for PwpServerSettings {
    fn default() -> Self {
        Self {
            listen_address: default_listen_address(),
            listen_port: default_listen_port(),
            tls_certificate_chain_path: None,
            tls_private_key_path: None,
        }
    }
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

fn default_listen_address() -> String {
    "0.0.0.0".to_string()
}

fn default_listen_port() -> u16 {
    8080
}
