use crate::error::PwpError;
use crate::proxmox::certificate_verifier::PwpProxmoxCertificateVerifier;
use crate::proxmox::settings::PwpProxmoxSettings;
use axum::http::{HeaderMap, HeaderValue, header};
use log::info;
use macaddr::MacAddr;
use reqwest::Client;
use rustls::ClientConfig;
use serde_json::Value;
use std::net::IpAddr;
use std::str::FromStr;
use std::sync::Arc;
use tokio::net::UdpSocket;
use tokio::time::Duration;

pub mod certificate_verifier;
pub mod settings;

pub struct PwpProxmoxNode {
    node_name: String,
    mac_address: MacAddr,
    broadcast_address: IpAddr,
    base_url: String,
    proxmox_client: Client,
}

impl PwpProxmoxNode {
    pub fn new(settings: PwpProxmoxSettings) -> Result<Self, PwpError> {
        let node_name = settings.node_name;

        info!("Creating Proxmox node with name: {node_name}");

        let mac_address = MacAddr::from_str(settings.mac_address.as_str())
            .map_err(PwpError::InvalidMacAddress)?;
        let broadcast_address = IpAddr::from_str(settings.broadcast_address.as_str())
            .map_err(PwpError::InvalidBroadcastAddress)?;

        info!(
            "Will send Wake-on-Lan(s) to MAC address '{mac_address}' on broadcast address '{broadcast_address}'"
        );

        let base_url = settings.base_url;
        let authorization_header_value = HeaderValue::from_str(
            format!(
                "PVEAPIToken={}={}",
                settings.token_id, settings.token_secret
            )
            .as_str(),
        )
        .map_err(|_| PwpError::InvalidProxmoxAuthorizationHeader)?;

        let proxy_target_client_config =
            if let Some(certificate_fingerprint) = settings.certificate_fingerprint.as_ref() {
                let client_config = ClientConfig::builder()
                    .dangerous()
                    .with_custom_certificate_verifier(PwpProxmoxCertificateVerifier::new(
                        certificate_fingerprint,
                    )?)
                    .with_no_client_auth();

                Some(Arc::new(client_config))
            } else {
                None
            };

        let mut proxmox_client_builder = Client::builder()
            .default_headers(HeaderMap::from_iter([(
                header::AUTHORIZATION,
                authorization_header_value,
            )]))
            .connect_timeout(Duration::from_millis(settings.connect_timeout_ms))
            .timeout(Duration::from_millis(settings.timeout_ms));

        if let Some(proxy_target_client_config) = proxy_target_client_config {
            proxmox_client_builder =
                proxmox_client_builder.use_preconfigured_tls(proxy_target_client_config);
        }

        let proxmox_client = proxmox_client_builder
            .build()
            .map_err(Arc::new)
            .map_err(PwpError::ProxmoxClientError)?;

        info!("Created Proxmox client with base URL: {base_url}");

        Ok(Self {
            node_name,
            mac_address,
            broadcast_address,
            base_url,
            proxmox_client,
        })
    }

    pub async fn get_running_vm_ids(&self) -> Result<Option<Vec<u32>>, PwpError> {
        info!("Querying Proxmox node for running VMs");

        let request = self.proxmox_client.get(format!(
            "{}/api2/json/cluster/resources?type=vm",
            self.base_url
        ));

        match request.send().await {
            Ok(response) if response.status().is_success() => {
                let body = response
                    .text()
                    .await
                    .map_err(Arc::new)
                    .map_err(PwpError::ProxmoxClientError)?;
                let body_value: Value = serde_json::from_str(body.as_str())?;

                if let Some(data) = body_value.get("data")
                    && let Some(vms) = data.as_array()
                {
                    let running_vm_ids = vms
                        .iter()
                        .filter_map(|vm| {
                            vm.get("status").and_then(Value::as_str).and_then(|status| {
                                if status == "running" {
                                    vm.get("id")
                                        .and_then(Value::as_u64)
                                        .map(|running_vm_id| running_vm_id as u32)
                                } else {
                                    None
                                }
                            })
                        })
                        .collect();

                    Ok(Some(running_vm_ids))
                } else {
                    Err(PwpError::InvalidProxmoxResponse(body))
                }
            }
            Ok(response) => {
                let response = response
                    .error_for_status()
                    .map_err(Arc::new)
                    .map_err(PwpError::ProxmoxClientError)?;
                Err(PwpError::NonSuccessProxmoxResponse(
                    response.status(),
                    response.text().await.ok(),
                ))
            }
            Err(error) if error.is_connect() || error.is_timeout() => Ok(None),
            Err(error) => Err(PwpError::ProxmoxClientError(Arc::new(error))),
        }
    }

    pub async fn start_vm(&self, vm_id: u32) -> Result<(), PwpError> {
        info!("Starting VM: {vm_id}");

        self.proxmox_client
            .post(format!(
                "{}/api2/json/nodes/{}/qemu/{vm_id}/status/start",
                self.base_url, self.node_name
            ))
            .form(&())
            .send()
            .await
            .map_err(Arc::new)
            .map_err(PwpError::ProxmoxClientError)?
            .error_for_status()
            .map_err(Arc::new)
            .map_err(PwpError::ProxmoxClientError)?;

        Ok(())
    }

    pub async fn shutdown_vm(&self, vm_id: u32) -> Result<(), PwpError> {
        info!("Shutting down VM: {vm_id}");

        self.proxmox_client
            .post(format!(
                "{}/api2/json/nodes/{}/qemu/{vm_id}/status/shutdown",
                self.base_url, self.node_name
            ))
            .form(&())
            .send()
            .await
            .map_err(Arc::new)
            .map_err(PwpError::ProxmoxClientError)?
            .error_for_status()
            .map_err(Arc::new)
            .map_err(PwpError::ProxmoxClientError)?;

        Ok(())
    }

    pub async fn shutdown(&self) -> Result<(), PwpError> {
        info!("Shutting down the Proxmox node");

        self.proxmox_client
            .post(format!(
                "{}/api2/json/nodes/{}/status",
                self.base_url, self.node_name
            ))
            .form(&[("command", "shutdown")])
            .send()
            .await
            .map_err(Arc::new)
            .map_err(PwpError::ProxmoxClientError)?;

        Ok(())
    }

    pub async fn ping_vm_guest_agent(&self, vm_id: u32) -> Result<bool, PwpError> {
        info!("Pinging VM '{vm_id}' guest agent");

        let request = self
            .proxmox_client
            .post(format!(
                "{}/api2/json/nodes/{}/qemu/{vm_id}/agent/ping",
                self.base_url, self.node_name
            ))
            .form(&());

        match request.send().await {
            Ok(response) if response.status().is_success() => Ok(true),
            Ok(response) if response.status().as_u16() == 500 => {
                let body = response
                    .text()
                    .await
                    .map_err(Arc::new)
                    .map_err(PwpError::ProxmoxClientError)?;
                let body_value: Value = serde_json::from_str(body.as_str())?;

                body_value
                    .get("message")
                    .and_then(Value::as_str)
                    .and_then(|message| match message.trim() {
                        message if message == format!("VM {vm_id} is not running") => Some(false),
                        "QEMU guest agent is not running" => Some(false),
                        _ => None,
                    })
                    .ok_or(PwpError::InvalidProxmoxResponse(body))
            }
            Ok(response) => {
                let response = response
                    .error_for_status()
                    .map_err(Arc::new)
                    .map_err(PwpError::ProxmoxClientError)?;
                Err(PwpError::NonSuccessProxmoxResponse(
                    response.status(),
                    response.text().await.ok(),
                ))
            }
            Err(error) => Err(PwpError::ProxmoxClientError(Arc::new(error))),
        }
    }

    pub async fn send_wake_on_lan(&self) -> Result<(), PwpError> {
        info!("Sending Wake-on-LAN packet");

        let wol_packet = {
            let mut wol_packet = [0u8; 102];
            wol_packet[0..6].copy_from_slice(&[0xff; 6]);

            for i in 0..16 {
                let start_index = 6 + (6 * i);
                wol_packet[start_index..start_index + 6]
                    .copy_from_slice(self.mac_address.as_bytes());
            }

            wol_packet
        };

        let udp_socket = UdpSocket::bind("0.0.0.0:0").await?;
        udp_socket.set_broadcast(true)?;
        udp_socket
            .send_to(&wol_packet, (self.broadcast_address, 9))
            .await?;

        Ok(())
    }
}
