use crate::error::PwpError;
use crate::manager::active_vm::{PwpActiveVm, PwpOngoingRequestGuard};
use crate::proxmox::PwpProxmoxNode;
use crate::settings::PwpSettings;
use crate::target::PwpProxyTarget;
use axum::body::Body;
use axum::http::Request;
use chrono::Utc;
use futures_util::future;
use futures_util::future::{BoxFuture, FutureExt, OptionFuture, select_all};
use log::{debug, info, warn};
use std::collections::BTreeMap;
use std::net::{AddrParseError, IpAddr, SocketAddr};
use std::ops::Add;
use std::str::FromStr;
use std::sync::Arc;
use tokio::select;
use tokio::sync::{Mutex, Notify};
use tokio::time::{Duration, Instant, interval, sleep, sleep_until};
use url::Url;

pub mod active_vm;
pub mod settings;

#[derive(Clone)]
pub struct PwpActiveVmFuture<'f> {
    pub vm_id: u32,
    pub future: future::Shared<BoxFuture<'f, Result<Arc<PwpOngoingRequestGuard>, PwpError>>>,
}

pub struct PwpManager {
    proxmox_node: PwpProxmoxNode,
    proxy_targets: Vec<Arc<PwpProxyTarget>>,
    active_vm_future: Arc<Mutex<Option<PwpActiveVmFuture<'static>>>>,
    active_vms: Arc<Mutex<BTreeMap<u32, PwpActiveVm>>>,
    active_vm_change_notify: Notify,

    trusted_proxy_addresses: Option<Vec<IpAddr>>,
    mutually_exclusive_vm_ids: Vec<u32>,
    max_idle_duration: Duration,
    reachability_check_delay_duration: Duration,
    guest_agent_ping_delay_duration: Duration,
    vm_start_timeout_duration: Duration,
    wake_on_lan_every_duration: Duration,
    wake_on_lan_attempts: u32,
}

impl PwpManager {
    pub fn new(settings: PwpSettings) -> Result<Arc<Self>, PwpError> {
        info!("Starting Proxmox Wake-on-LAN proxy manager");

        let proxmox_node = PwpProxmoxNode::new(settings.proxmox)?;
        let proxy_targets = settings
            .targets
            .into_iter()
            .map(PwpProxyTarget::new)
            .map(Arc::new)
            .collect::<Vec<_>>();
        let active_vm_future = Arc::new(Mutex::new(None));
        let active_vms = Arc::new(Mutex::new(BTreeMap::new()));
        let active_vm_change_notify = Notify::new();

        let settings = settings.manager;

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
        let max_idle_duration = Duration::from_secs(settings.max_idle_secs);
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
            active_vm_future,
            active_vms,
            active_vm_change_notify,

            trusted_proxy_addresses,
            mutually_exclusive_vm_ids,
            max_idle_duration,
            reachability_check_delay_duration,
            guest_agent_ping_delay_duration,
            vm_start_timeout_duration,
            wake_on_lan_every_duration,
            wake_on_lan_attempts,
        }))
    }

    pub fn authenticate(self: Arc<Self>, peer_address: &SocketAddr) -> Result<(), PwpError> {
        if let Some(trusted_proxy_addresses) = self.trusted_proxy_addresses.as_ref() {
            if !trusted_proxy_addresses.contains(&peer_address.ip()) {
                Err(PwpError::AccessDenied)
            } else {
                Ok(())
            }
        } else {
            Ok(())
        }
    }

    pub fn choose_proxy_target(
        self: Arc<Self>,
        request: &Request<Body>,
    ) -> Result<Arc<PwpProxyTarget>, PwpError> {
        let target_headers = request
            .headers()
            .get_all("X-Proxy-Target")
            .into_iter()
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

    pub fn choose_proxy_url(
        self: Arc<Self>,
        request: &Request<Body>,
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
                let mut proxy_url = Url::from_str(request.uri().to_string().as_str())
                    .map_err(PwpError::InvalidRequestUrl)?;
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

    pub async fn handle_vm_shutdowns(self: Arc<Self>) -> ! {
        info!("Starting VM shutdown handler");

        loop {
            let active_vm_idle_notify_futures = self
                .active_vms
                .lock()
                .await
                .iter()
                .map(|(active_vm_id, active_vm)| {
                    let active_vm_id = *active_vm_id;
                    let active_vm = active_vm.clone();
                    async move {
                        active_vm.idle_notify.notified().await;
                        (active_vm_id, active_vm)
                    }
                    .boxed()
                })
                .collect::<Vec<_>>();

            let next_active_vm_idle_notify_future: OptionFuture<_> =
                if active_vm_idle_notify_futures.is_empty() {
                    None.into()
                } else {
                    Some(select_all(active_vm_idle_notify_futures)).into()
                };

            let (active_vm_id, active_vm) = select! {
                Some((active_vm_id_and_vm, _, _)) = next_active_vm_idle_notify_future => active_vm_id_and_vm,
                _ = self.active_vm_change_notify.notified() => continue,
            };

            let after_max_idle_duration = active_vm.idle_since().add(self.max_idle_duration);

            info!(
                "Waiting until '{}' before shutting down VM: {active_vm_id}",
                Self::format_instant(after_max_idle_duration)
            );

            select! {
                _ = sleep_until(after_max_idle_duration) => {
                    if let Err(error) = self.proxmox_node.shutdown_vm(active_vm_id).await {
                        warn!("Failed to shutdown VM '{active_vm_id}': {error}");
                    }

                    let mut active_vm_guard = self.active_vms.lock().await;
                    active_vm_guard.remove(&active_vm_id);

                    if active_vm_guard.is_empty() && let Err(error) = self.proxmox_node.shutdown().await {
                        warn!("Failed to shutdown the Proxmox node: {error}");
                    }
                },
                _ = active_vm.busy_notify.notified() => {
                    info!("Request received, cancelling VM '{active_vm_id}' shutdown");
                    continue;
                }
            }
        }
    }

    pub async fn ensure_active_vm_for_target(
        self: Arc<Self>,
        proxy_target: Arc<PwpProxyTarget>,
    ) -> Result<Arc<PwpOngoingRequestGuard>, PwpError> {
        let target_vm_id = proxy_target.vm_id();

        let active_vm_future = {
            let mut active_vm_future_guard = self.active_vm_future.lock().await;

            active_vm_future_guard
                .get_or_insert_with(|| self.clone().get_active_vm_future(proxy_target.clone()))
                .clone()
        };

        if active_vm_future.vm_id == target_vm_id {
            let result = active_vm_future.future.await;
            self.active_vm_future.lock().await.take();
            result
        } else {
            info!(
                "VM '{target_vm_id}' is required for request but manager is currently starting VM '{}'",
                active_vm_future.vm_id
            );
            Err(PwpError::ProxmoxNodeBusy)
        }
    }

    fn get_active_vm_future(
        self: Arc<Self>,
        proxy_target: Arc<PwpProxyTarget>,
    ) -> PwpActiveVmFuture<'static> {
        let target_vm_id = proxy_target.vm_id();

        let future = async move {
            let (ongoing_request_guard, active_vms_changed) = {
                let mut active_vm_guard = self.active_vms.lock().await;
                if let Some(active_vm) = active_vm_guard.get(&target_vm_id) {
                    (active_vm.handle_request(), false)
                } else {
                    let active_vm = PwpActiveVm::new(target_vm_id, false);
                    active_vm_guard.insert(target_vm_id, active_vm.clone());
                    (active_vm.handle_request(), true)
                }
            };

            if active_vms_changed {
                self.active_vm_change_notify.notify_waiters();
            }

            info!(
                "Ensuring VM '{target_vm_id}' is active for proxy target: {}",
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

            Ok(ongoing_request_guard)
        }.boxed().shared();

        PwpActiveVmFuture {
            vm_id: target_vm_id,
            future,
        }
    }

    fn format_instant(instant: Instant) -> String {
        let now_instant = Instant::now();
        let now_utc = Utc::now();

        let date_time = if instant >= now_instant {
            now_utc + chrono::Duration::from_std(instant - now_instant).unwrap()
        } else {
            now_utc - chrono::Duration::from_std(now_instant - instant).unwrap()
        };

        date_time.to_rfc3339()
    }
}
