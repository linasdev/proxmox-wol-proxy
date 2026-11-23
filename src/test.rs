use crate::error::PwpError;
use crate::manager::PwpManager;
use crate::proxmox::PwpProxmoxNode;
use crate::settings::PwpSettings;
use config::{Config, FileFormat};
use mockito::{Mock, Server, ServerGuard};
use serde_json::json;
use std::sync::Arc;

pub struct TestContext {
    pub server: ServerGuard,
    pub settings: PwpSettings,
    running_vms_mock: Option<Mock>,
}

impl TestContext {
    pub async fn mock_running_vms(&mut self, running: &[u32]) {
        self.mock_running_vms_response(200, running_vms_body(running))
            .await;
    }

    pub async fn mock_running_vms_response(&mut self, status: usize, body: String) {
        if let Some(old) = self.running_vms_mock.take() {
            old.remove_async().await;
        }

        self.running_vms_mock = Some(
            self.server
                .mock("GET", "/api2/json/cluster/resources?type=vm")
                .match_header("authorization", "PVEAPIToken=root@pam!id=secret")
                .with_status(status)
                .with_body(body)
                .create_async()
                .await,
        );
    }

    pub fn node(&self) -> Result<PwpProxmoxNode, PwpError> {
        PwpProxmoxNode::new(self.settings.proxmox.clone())
    }

    pub async fn manager(&self) -> Result<Arc<PwpManager>, PwpError> {
        PwpManager::new(self.settings.clone()).await
    }
}

pub async fn initialize_test_context(
    manager_toml: &str,
    proxmox_toml: &str,
    targets: &[(&str, u32, &str)],
) -> Result<TestContext, PwpError> {
    let server = Server::new_async().await;

    let targets = targets
        .iter()
        .map(|(name, vm_id, extra)| {
            format!("[[targets]]\nname = \"{name}\"\nvm-id = {vm_id}\n{extra}\n")
        })
        .collect::<Vec<_>>()
        .join("\n");

    let toml = format!(
        r#"
        [manager]
        reachability-check-delay-ms = 10
        guest-agent-ping-delay-ms = 10
        vm-start-timeout-secs = 1
        wake-on-lan-every-secs = 1
        wake-on-lan-attempts = 1

        [proxmox]
        mac-address = "10:ff:e0:8c:69:3c"
        broadcast-address = "127.0.0.1"
        node-name = "node"
        base-url = "{}"
        token-id = "root@pam!id"
        token-secret = "secret"
        connect-timeout-ms = 500
        timeout-ms = 2000

        {targets}
        "#,
        server.url()
    );

    let overrides = format!("[manager]\n{manager_toml}\n[proxmox]\n{proxmox_toml}");

    let settings: PwpSettings = Config::builder()
        .add_source(config::File::from_str(&toml, FileFormat::Toml))
        .add_source(config::File::from_str(&overrides, FileFormat::Toml))
        .build()
        .and_then(Config::try_deserialize)
        .map_err(Arc::new)
        .map_err(PwpError::Config)?;

    Ok(TestContext {
        server,
        settings,
        running_vms_mock: None,
    })
}

fn running_vms_body(running: &[u32]) -> String {
    let data = running
        .iter()
        .map(|vm_id| json!({"id": format!("qemu/{vm_id}"), "status": "running"}))
        .collect::<Vec<_>>();

    json!({ "data": data }).to_string()
}
