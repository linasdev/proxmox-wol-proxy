use crate::error::PwpError;
use crate::proxmox::node::PwpProxmoxNode;
use crate::settings::PwpSettings;
use crate::target::proxy::PwpProxyTarget;
use actix_web::HttpRequest;
use actix_web::http::Uri;
use futures_util::future;
use futures_util::future::{BoxFuture, FutureExt};
use log::{debug, info};
use std::net::{AddrParseError, IpAddr, SocketAddr};
use std::str::FromStr;
use std::sync::Arc;
use tokio::select;
use tokio::sync::Mutex;
use tokio::time::{Duration, interval, sleep};
use url::Url;

#[derive(Clone)]
pub struct PwpRequestedVmStateFuture<'f> {
    pub vm_id: u32,
    pub future: future::Shared<BoxFuture<'f, Result<(), PwpError>>>,
}

pub struct PwpManager {
    proxmox_node: PwpProxmoxNode,
    proxy_targets: Vec<Arc<PwpProxyTarget>>,
    requested_vm_state_future: Arc<Mutex<Option<PwpRequestedVmStateFuture<'static>>>>,

    trusted_proxy_addresses: Option<Vec<IpAddr>>,
    mutually_exclusive_vm_ids: Vec<u32>,
    reachability_check_delay_duration: Duration,
    guest_agent_ping_delay_duration: Duration,
    vm_start_timeout_duration: Duration,
    wake_on_lan_every_duration: Duration,
    wake_on_lan_attempts: u32,
}

impl PwpManager {
    pub fn new(settings: PwpSettings) -> Result<Arc<Self>, PwpError> {
        info!("Starting Proxmox Wake-on-LAN proxy manager");

        let proxmox_node = PwpProxmoxNode::new(settings.clone())?;
        let proxy_targets = settings
            .targets
            .clone()
            .into_iter()
            .map(PwpProxyTarget::new)
            .map(Arc::new)
            .collect::<Vec<_>>();
        let requested_vm_state_future = Arc::new(Mutex::new(None));

        let trusted_proxy_addresses = settings
            .trusted_proxy_addresses
            .map(|trusted_proxy_addresses| {
                trusted_proxy_addresses
                    .iter()
                    .map(String::as_str)
                    .map(IpAddr::from_str)
                    .collect::<Result<Vec<_>, AddrParseError>>()
            })
            .transpose()
            .map_err(PwpError::InvalidTrustedProxyAddress)?;

        let mutually_exclusive_vm_ids = settings.mutually_exclusive_vm_ids;
        let reachability_check_delay_duration =
            Duration::from_millis(settings.reachability_check_delay_ms);
        let guest_agent_ping_delay_duration =
            Duration::from_millis(settings.guest_agent_ping_delay_ms);
        let vm_start_timeout_duration = Duration::from_secs(settings.vm_start_timeout_secs);
        let wake_on_lan_every_duration = Duration::from_secs(settings.wake_on_lan_every_secs);
        let wake_on_lan_attempts = settings.wake_on_lan_attempts;

        info!(
            "Will sleep {:.2}s after performing each reachability check when starting the Proxmox node",
            reachability_check_delay_duration.as_secs_f32()
        );
        info!(
            "Will sleep {:.2}s after performing each guest agent ping when starting a VM on the Proxmox node",
            guest_agent_ping_delay_duration.as_secs_f32()
        );
        info!(
            "Will send Wake-on-LAN packets every {:.2}s when starting the Proxmox node",
            wake_on_lan_every_duration.as_secs_f32()
        );
        info!(
            "Will send a maximum of {wake_on_lan_attempts} Wake-on-LAN packet(s) when starting the Proxmox node"
        );

        if !mutually_exclusive_vm_ids.is_empty() {
            info!(
                "Will start these VMs one at a time (mutually exclusive): {mutually_exclusive_vm_ids:?}"
            );
        }

        Ok(Arc::new(Self {
            proxmox_node,
            proxy_targets,
            requested_vm_state_future,
            trusted_proxy_addresses,
            mutually_exclusive_vm_ids,
            reachability_check_delay_duration,
            guest_agent_ping_delay_duration,
            vm_start_timeout_duration,
            wake_on_lan_every_duration,
            wake_on_lan_attempts,
        }))
    }

    pub fn authenticate(self: Arc<Self>, peer_address: Option<SocketAddr>) -> Result<(), PwpError> {
        if let Some(trusted_proxy_addresses) = self.trusted_proxy_addresses.as_ref() {
            if let Some(peer_address) = peer_address
                && trusted_proxy_addresses.contains(&peer_address.ip())
            {
                Ok(())
            } else {
                Err(PwpError::AccessDenied)
            }
        } else {
            Ok(())
        }
    }

    pub fn choose_proxy_target(
        self: Arc<Self>,
        request: &HttpRequest,
    ) -> Result<Arc<PwpProxyTarget>, PwpError> {
        let target_headers = request
            .headers()
            .get_all("X-Proxy-Target")
            .collect::<Vec<_>>();
        if target_headers.len() != 1 {
            if self.proxy_targets.len() == 1 {
                debug!(
                    "Using the only configured proxy target and ignoring 'X-Proxy-Target' header"
                );
                return Ok(self.proxy_targets[0].clone());
            }

            return Err(PwpError::MissingOrDuplicateProxyTargetHeader);
        }

        let target_header = target_headers[0]
            .to_str()
            .map_err(Arc::new)
            .map_err(PwpError::InvalidProxyTargetHeader)?;

        if target_header.trim().is_empty() {
            if self.proxy_targets.len() == 1 {
                debug!(
                    "Using the only configured proxy target and ignoring 'X-Proxy-Target' header"
                );
                return Ok(self.proxy_targets[0].clone());
            }

            return Err(PwpError::MissingOrDuplicateProxyTargetHeader);
        }

        debug!("Choosing proxy target by name: {target_header}");

        let mut chosen_proxy_target = None;

        for current_proxy_target in self.proxy_targets.iter() {
            if current_proxy_target.name() == target_header {
                if chosen_proxy_target.is_some() {
                    return Err(PwpError::DuplicateProxyTargetName(
                        target_header.to_string(),
                    ));
                }

                chosen_proxy_target = Some(current_proxy_target.clone());
            }
        }

        chosen_proxy_target.ok_or_else(|| PwpError::MissingProxyTarget(target_header.to_string()))
    }

    pub fn choose_proxy_uri(
        self: Arc<Self>,
        request: &HttpRequest,
        proxy_target: Arc<PwpProxyTarget>,
    ) -> Result<Option<Uri>, PwpError> {
        self.choose_proxy_url(request, proxy_target.clone())?
            .map(|proxy_url| {
                if !proxy_url.has_authority() {
                    return Err(PwpError::FailedToAssembleProxyUrl);
                }

                if !proxy_url.has_host() {
                    return Err(PwpError::FailedToAssembleProxyUrl);
                }

                let scheme = proxy_url.scheme();
                let authority = proxy_url.authority();

                let authority_end = scheme.len() + "://".len() + authority.len();
                let path_and_query = &proxy_url.as_str()[authority_end..];

                Uri::builder()
                    .scheme(scheme)
                    .authority(authority)
                    .path_and_query(path_and_query)
                    .build()
                    .map_err(|_| PwpError::FailedToAssembleProxyUrl)
            })
            .transpose()
    }

    pub fn choose_proxy_url(
        self: Arc<Self>,
        request: &HttpRequest,
        proxy_target: Arc<PwpProxyTarget>,
    ) -> Result<Option<Url>, PwpError> {
        let header_url = request
            .headers()
            .get("X-Proxy-URL")
            .map(|header_value| {
                header_value
                    .to_str()
                    .map_err(Arc::new)
                    .map_err(PwpError::InvalidProxyUrlHeader)
            })
            .transpose()?;

        if !proxy_target.should_proxy() {
            debug!(
                "Proxy target '{}' is not configured to proxy, ignoring all proxy URL sources",
                proxy_target.name()
            );
            Ok(None)
        } else if let Some(header_url) = header_url {
            debug!(
                "Proxy target '{}' is configured to proxy, using URL provided in the 'X-Proxy-URL' header",
                proxy_target.name()
            );
            Ok(Some(
                Url::parse(header_url).map_err(PwpError::InvalidHeaderUrl)?,
            ))
        } else if let Some(target_url) = proxy_target.default_url() {
            debug!(
                "Proxy target '{}' is configured to proxy, using URL provided in the target configuration",
                proxy_target.name()
            );
            let target_url = Url::parse(target_url).map_err(PwpError::InvalidProxyTargetUrl)?;

            if let Some(host) = target_url.host_str() {
                let mut proxy_url = request.full_url();
                proxy_url
                    .set_scheme(target_url.scheme())
                    .map_err(|_| PwpError::FailedToAssembleProxyUrl)?;
                proxy_url
                    .set_host(Some(host))
                    .map_err(|_| PwpError::FailedToAssembleProxyUrl)?;
                proxy_url
                    .set_port(target_url.port())
                    .map_err(|_| PwpError::FailedToAssembleProxyUrl)?;
                Ok(Some(proxy_url))
            } else {
                Err(PwpError::MissingProxyTargetUrlHost)
            }
        } else {
            Err(PwpError::MissingProxyUrl)
        }
    }

    pub async fn ensure_requested_vm_state_for_target(
        self: Arc<Self>,
        proxy_target: Arc<PwpProxyTarget>,
    ) -> Result<(), PwpError> {
        let target_vm_id = proxy_target.vm_id();

        let requested_vm_state_future = {
            let mut requested_vm_state_future_guard = self.requested_vm_state_future.lock().await;

            requested_vm_state_future_guard
                .get_or_insert_with(|| {
                    self.clone()
                        .get_requested_vm_state_future(proxy_target.clone())
                })
                .clone()
        };

        if requested_vm_state_future.vm_id == target_vm_id {
            let result = requested_vm_state_future.future.await;
            self.requested_vm_state_future.lock().await.take();
            result
        } else {
            info!(
                "VM '{target_vm_id}' is required for request but manager is currently starting VM '{}'",
                requested_vm_state_future.vm_id
            );
            Err(PwpError::ProxmoxNodeBusy)
        }
    }

    fn get_requested_vm_state_future(
        self: Arc<Self>,
        proxy_target: Arc<PwpProxyTarget>,
    ) -> PwpRequestedVmStateFuture<'static> {
        let target_vm_id = proxy_target.vm_id();

        let future = async move {
            info!(
                "Ensuring requested VM state for proxy target: {}",
                proxy_target.name()
            );

            let mut running_vm_ids = self.proxmox_node.get_running_vm_ids().await?;

            if running_vm_ids.is_none() {
                info!("Proxmox node is not running, will send Wake-on-LAN packet(s) to wake it up");

                let mut wake_on_lan_sent_count = 0u32;
                let mut wake_on_lan_interval = interval(self.wake_on_lan_every_duration);

                while running_vm_ids.is_none() {
                    select! {
                        _ = wake_on_lan_interval.tick() => {
                            if wake_on_lan_sent_count >= self.wake_on_lan_attempts {
                                return Err(PwpError::ProxmoxNodeStartTimedOut);
                            }

                            self.proxmox_node.send_wake_on_lan().await?;
                            wake_on_lan_sent_count += 1;
                        }
                        _ = sleep(self.reachability_check_delay_duration) => {}
                    }

                    running_vm_ids = self.proxmox_node.get_running_vm_ids().await?;
                }
            }

            let running_vm_ids = running_vm_ids.unwrap();
            let running_mutually_exclusive_vm_ids = running_vm_ids
                .iter()
                .copied()
                .filter(|vm_id| self.mutually_exclusive_vm_ids.contains(vm_id))
                .collect::<Vec<_>>();

            if running_vm_ids.is_empty() {
                info!("Proxmox node is running with no VMs");
            } else {
                info!("Proxmox node is running with VMs: {running_vm_ids:?}");
            }

            if !running_mutually_exclusive_vm_ids.is_empty() {
                info!(
                    "Proxmox node is running mutually exclusive VMs: {running_mutually_exclusive_vm_ids:?}"
                );
            }

            let need_to_start_vm = !running_vm_ids.contains(&target_vm_id);
            let can_start_vm = !self
                .mutually_exclusive_vm_ids
                .contains(&target_vm_id)
                || running_mutually_exclusive_vm_ids.is_empty();

            if need_to_start_vm && can_start_vm {
                self.proxmox_node.start_vm(target_vm_id).await?;
            } else if need_to_start_vm {
                info!("VM '{target_vm_id}' needs to be started on the Proxmox node, which is already running (a) conflicting VM(s): {running_mutually_exclusive_vm_ids:?}");
                return Err(PwpError::ProxmoxNodeBusy);
            }

            let mut vm_running = self
                .proxmox_node
                .ping_vm_guest_agent(target_vm_id)
                .await?;

            if !vm_running {
                info!("VM guest agent ping failed, will wait for VM to start");

                let mut vm_start_timeout = sleep(self.vm_start_timeout_duration).boxed();

                while !vm_running {
                    select! {
                        _ = &mut vm_start_timeout => return Err(PwpError::ProxmoxVmStartTimedOut),
                        _ = sleep(self.guest_agent_ping_delay_duration) => {},
                    }

                    vm_running = self
                        .proxmox_node
                        .ping_vm_guest_agent(target_vm_id)
                        .await?;
                }
            }

            info!("VM guest agent ping succeeded");

            Ok(())
        }.boxed().shared();

        PwpRequestedVmStateFuture {
            vm_id: target_vm_id,
            future,
        }
    }
}
