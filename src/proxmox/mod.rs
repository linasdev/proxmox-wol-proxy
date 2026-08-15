use crate::error::PwpError;
use crate::proxmox::settings::PwpProxmoxSettings;
use log::info;
use macaddr::MacAddr;
use proxmox_client::ProxmoxClient;
use serde_json::Value;
use std::net::IpAddr;
use std::str::FromStr;
use tokio::net::UdpSocket;
use tokio::time::Duration;

pub mod settings;

pub struct PwpProxmoxNode {
    node_name: String,
    mac_address: MacAddr,
    broadcast_address: IpAddr,
    proxmox_client: ProxmoxClient,
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

        let proxmox_client = ProxmoxClient::builder(settings.base_url.as_str())
            .api_token(settings.token_id.as_str(), settings.token_secret.as_str())
            .connect_timeout(Duration::from_millis(settings.connect_timeout_ms))
            .accept_invalid_certs(settings.accept_invalid_certs)
            .allow_insecure_http(settings.allow_insecure_http)
            .build()?;

        info!(
            "Created Proxmox client with base URL: {}",
            settings.base_url
        );

        Ok(Self {
            node_name,
            mac_address,
            broadcast_address,
            proxmox_client,
        })
    }

    pub async fn get_running_vm_ids(&self) -> Result<Option<Vec<u32>>, PwpError> {
        info!("Querying Proxmox node for running VMs");

        match self.proxmox_client.list_cluster_resources().await {
            Ok(cluster_resources) => {
                let vms = cluster_resources
                    .iter()
                    .filter_map(|resource| {
                        if let Some(status) = resource.status.as_ref() {
                            if status.as_str() == "running" {
                                resource.vmid.map(|vm_id| vm_id as u32)
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    })
                    .collect();
                Ok(Some(vms))
            }
            Err(proxmox_client::Error::Request(error)) if error.is_connect() => Ok(None),
            Err(proxmox_client::Error::Request(error)) if error.is_timeout() => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub async fn start_vm(&self, vm_id: u32) -> Result<(), PwpError> {
        info!("Starting VM: {vm_id}");
        self.proxmox_client
            .start_vm(self.node_name.as_str(), vm_id, None)
            .await?;
        Ok(())
    }

    pub async fn shutdown_vm(&self, vm_id: u32) -> Result<(), PwpError> {
        info!("Shutting down VM: {vm_id}");
        self.proxmox_client
            .shutdown_vm(self.node_name.as_str(), vm_id, None)
            .await?;
        Ok(())
    }

    pub async fn ping_vm_guest_agent(&self, vm_id: u32) -> Result<bool, PwpError> {
        info!("Pinging VM '{vm_id}' guest agent");

        match self
            .proxmox_client
            .agent_ping(self.node_name.as_str(), vm_id)
            .await
        {
            Ok(_) => Ok(true),
            Err(error) => {
                if let proxmox_client::Error::InternalServerError { body } = &error {
                    let body_value: Value = serde_json::from_str(body.as_str())?;

                    if let Some(message) = body_value.get("message")
                        && let Some(message) = message.as_str()
                    {
                        match message.trim() {
                            message if message == format!("VM {vm_id} is not running") => Ok(false),
                            "QEMU guest agent is not running" => Ok(false),
                            _ => Err(error.into()),
                        }
                    } else {
                        Err(error.into())
                    }
                } else {
                    Err(error.into())
                }
            }
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
