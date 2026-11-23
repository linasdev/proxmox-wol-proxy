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

#[cfg(test)]
mod tests {
    use super::*;
    use config::{Config, FileFormat};
    use googletest::prelude::*;

    const MINIMAL: &str = r#"
        [manager]

        [proxmox]
        mac-address = "10:ff:e0:8c:69:3c"
        broadcast-address = "10.10.255.255"
        node-name = "node"
        base-url = "https://node:8006"
        token-id = "id"
        token-secret = "secret"

        [[targets]]
        name = "vm1"
        vm-id = 101
    "#;

    fn parse(toml: &str) -> Result<PwpSettings, config::ConfigError> {
        Config::builder()
            .add_source(config::File::from_str(toml, FileFormat::Toml))
            .build()?
            .try_deserialize()
    }

    #[gtest]
    fn minimal_config_uses_defaults() {
        let settings = parse(MINIMAL).unwrap();

        assert_that!(settings.server.listen_address, eq("0.0.0.0"));
        assert_that!(settings.server.listen_port, eq(8080));
        assert_that!(settings.client.tls_root_certificate_path, none());

        let manager = &settings.manager;
        assert_that!(manager.trusted_proxy_addresses, none());
        assert_that!(manager.mutually_exclusive_vm_ids, is_empty());
        assert_that!(
            manager.treat_non_managed_vms_as_mutually_exclusive,
            is_true()
        );
        assert_that!(
            manager.prevent_shutdown_when_non_managed_vms_active,
            is_true()
        );
        assert_that!(manager.max_idle_secs, eq(300));
        assert_that!(manager.reachability_check_delay_ms, eq(500));
        assert_that!(manager.guest_agent_ping_delay_ms, eq(1000));
        assert_that!(manager.vm_start_timeout_secs, eq(30));
        assert_that!(manager.wake_on_lan_every_secs, eq(30));
        assert_that!(manager.wake_on_lan_attempts, eq(3));

        assert_that!(settings.proxmox.connect_timeout_ms, eq(1000));
        assert_that!(settings.proxmox.timeout_ms, eq(5000));
        assert_that!(settings.proxmox.certificate_fingerprint, none());

        let target = &settings.targets[0];
        assert_that!(target.name, eq("vm1"));
        assert_that!(target.vm_id, eq(101));
        assert_that!(target.default_url, none());
        assert_that!(target.max_idle_secs, none());
        assert_that!(target.should_proxy, is_true());
        assert_that!(target.preserve_host_header, is_false());
    }

    #[gtest]
    fn parses_nested_mutually_exclusive_vm_ids() {
        let toml = MINIMAL.replace(
            "[manager]",
            "[manager]\nmutually-exclusive-vm-ids = [[101, 102], [103, 104, 105]]",
        );
        let settings = parse(&toml).unwrap();

        assert_that!(
            settings.manager.mutually_exclusive_vm_ids,
            eq(&vec![vec![101, 102], vec![103, 104, 105]])
        );
    }

    #[gtest]
    fn rejects_flat_mutually_exclusive_vm_ids() {
        let toml = MINIMAL.replace(
            "[manager]",
            "[manager]\nmutually-exclusive-vm-ids = [101, 102]",
        );

        assert_that!(parse(&toml), err(anything()));
    }

    #[gtest]
    fn parses_target_overrides() {
        let toml = MINIMAL.replace(
            "vm-id = 101",
            "vm-id = 101\nmax-idle-secs = 42\nshould-proxy = false\npreserve-host-header = true\ndefault-url = \"https://vm1\"",
        );
        let settings = parse(&toml).unwrap();
        let target = &settings.targets[0];

        assert_that!(target.max_idle_secs, eq(Some(42)));
        assert_that!(target.should_proxy, is_false());
        assert_that!(target.preserve_host_header, is_true());
        assert_that!(target.default_url.as_deref(), eq(Some("https://vm1")));
    }

    #[gtest]
    fn missing_required_fields_are_rejected() {
        assert_that!(parse("[manager]\n"), err(anything()));
        assert_that!(parse(&MINIMAL.replace("vm-id = 101", "")), err(anything()));
    }
}
