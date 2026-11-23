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
use std::collections::{BTreeMap, BTreeSet};
use std::net::{AddrParseError, IpAddr, SocketAddr};
use std::str::FromStr;
use std::sync::Arc;
use tokio::select;
use tokio::sync::{Mutex, Notify};
use tokio::time::{Duration, Instant, interval, sleep, sleep_until};
use url::Url;

pub mod active_vm;
pub mod settings;

#[derive(Clone)]
pub struct PwpActiveNodeFuture<'f> {
    pub future: future::Shared<BoxFuture<'f, Result<(), PwpError>>>,
}

#[derive(Clone)]
pub struct PwpActiveVmFuture<'f> {
    pub future: future::Shared<BoxFuture<'f, Result<Arc<PwpOngoingRequestGuard>, PwpError>>>,
}

pub struct PwpManager {
    proxmox_node: PwpProxmoxNode,
    proxy_targets: Vec<Arc<PwpProxyTarget>>,
    active_node_future: Arc<Mutex<Option<PwpActiveNodeFuture<'static>>>>,
    active_vm_futures: Arc<Mutex<BTreeMap<u32, PwpActiveVmFuture<'static>>>>,
    active_vms: Arc<Mutex<BTreeMap<u32, PwpActiveVm>>>,
    active_vm_change_notify: Notify,

    trusted_proxy_addresses: Option<Vec<IpAddr>>,
    mutually_exclusive_vm_ids: Vec<Vec<u32>>,
    treat_non_managed_vms_as_mutually_exclusive: bool,
    prevent_shutdown_when_non_managed_vms_active: bool,
    max_idle_duration: Duration,
    reachability_check_delay_duration: Duration,
    guest_agent_ping_delay_duration: Duration,
    vm_start_timeout_duration: Duration,
    wake_on_lan_every_duration: Duration,
    wake_on_lan_attempts: u32,
}

impl PwpManager {
    pub async fn new(settings: PwpSettings) -> Result<Arc<Self>, PwpError> {
        info!("Starting Proxmox Wake-on-LAN proxy manager");

        let proxmox_node = PwpProxmoxNode::new(settings.proxmox)?;
        let proxy_targets = settings
            .targets
            .into_iter()
            .map(PwpProxyTarget::new)
            .map(Arc::new)
            .collect::<Vec<_>>();

        let mut max_idle_durations_by_vm_id = BTreeMap::new();
        for proxy_target in proxy_targets.iter() {
            let existing = max_idle_durations_by_vm_id
                .entry(proxy_target.vm_id())
                .or_insert(proxy_target.max_idle_duration());

            if *existing != proxy_target.max_idle_duration() {
                return Err(PwpError::ConflictingSharedVmMaxIdleSecs(
                    proxy_target.vm_id(),
                ));
            }
        }

        let active_node_future = Arc::new(Mutex::new(None));
        let active_vm_futures = Arc::new(Mutex::new(BTreeMap::new()));
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
        let treat_non_managed_vms_as_mutually_exclusive =
            settings.treat_non_managed_vms_as_mutually_exclusive;
        let prevent_shutdown_when_non_managed_vms_active =
            settings.prevent_shutdown_when_non_managed_vms_active;
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
                "Will start VMs in these groups one at a time (mutually exclusive): {mutually_exclusive_vm_ids:?}"
            );
        }

        if treat_non_managed_vms_as_mutually_exclusive {
            info!("Will treat non-managed VMs as mutually exclusive");
            info!("Will shutdown Proxmox node when all VMs are shut down");
        } else {
            info!("Will shutdown Proxmox node when all managed VMs are shut down");
        }

        let running_vm_ids = proxmox_node.get_running_vm_ids().await?.unwrap_or_default();
        for running_vm_id in running_vm_ids.into_iter() {
            if !proxy_targets
                .iter()
                .any(|proxy_target| proxy_target.vm_id() == running_vm_id)
            {
                info!("Proxmox node is already running non-managed VM '{running_vm_id}', ignoring");
                continue;
            }

            info!(
                "Proxmox node is already running managed VM '{running_vm_id}', starting shutdown timer"
            );

            let vm_max_idle_duration = proxy_targets
                .iter()
                .find(|proxy_target| proxy_target.vm_id() == running_vm_id)
                .and_then(|proxy_target| proxy_target.max_idle_duration())
                .unwrap_or(max_idle_duration);
            let active_vm = PwpActiveVm::new(running_vm_id, true, vm_max_idle_duration);
            active_vms.lock().await.insert(running_vm_id, active_vm);
        }

        Ok(Arc::new(Self {
            proxmox_node,
            proxy_targets,
            active_node_future,
            active_vm_futures,
            active_vms,
            active_vm_change_notify,

            trusted_proxy_addresses,
            mutually_exclusive_vm_ids,
            treat_non_managed_vms_as_mutually_exclusive,
            prevent_shutdown_when_non_managed_vms_active,
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
            let mut target_url = Url::parse(target_url).map_err(PwpError::InvalidProxyTargetUrl)?;

            if let Some(_) = target_url.host_str() {
                let request_uri = request.uri();
                target_url.set_path(request_uri.path());
                target_url.set_query(request_uri.query());

                Ok(Some(target_url))
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
                        active_vm.idle_notify().notified().await;
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

            let active_vm_shutdown_at = active_vm.shutdown_at();

            info!(
                "Waiting until '{}' before shutting down VM: {active_vm_id}",
                Self::format_instant(active_vm_shutdown_at)
            );

            select! {
                _ = sleep_until(active_vm_shutdown_at) => {
                    if let Err(error) = self.proxmox_node.shutdown_vm(active_vm_id).await {
                        warn!("Failed to shutdown VM '{active_vm_id}': {error}");
                    }

                    let mut active_vm_guard = self.active_vms.lock().await;
                    active_vm_guard.remove(&active_vm_id);

                    if !active_vm_guard.is_empty() {
                        continue;
                    }

                    if self.prevent_shutdown_when_non_managed_vms_active {
                        match self.proxmox_node.get_running_vm_ids().await {
                            Ok(Some(running_vm_ids)) if running_vm_ids.is_empty() => {}
                            Err(error) => {
                                warn!("Failed to get running VM ids: {error}");
                                continue;
                            }
                            _ => {
                                info!("Non-managed VMs are running, will not shutdown the Proxmox node");
                                continue;
                            },
                        }
                    }

                    if let Err(error) = self.proxmox_node.shutdown().await {
                        warn!("Failed to shutdown the Proxmox node: {error}");
                    }
                },
                _ = active_vm.busy_notify().notified() => {
                    info!("Request received, stopping VM '{active_vm_id}' shutdown timer");
                    continue;
                }
                _ = self.active_vm_change_notify.notified() => {
                    info!("Active VMs changed, stopping VM '{active_vm_id}' shutdown timer");

                    // Need to restore the Notify status of the active VM, otherwise it will never shut down
                    active_vm.idle_notify().notify_one();

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
            let mut active_vm_futures_guard = self.active_vm_futures.lock().await;

            if let Some(active_vm_future) = active_vm_futures_guard.get(&target_vm_id) {
                active_vm_future.clone()
            } else {
                let conflicting_vm_ids = self.conflicting_vm_ids(target_vm_id);

                {
                    if let Some(starting_vm_id) = active_vm_futures_guard
                        .keys()
                        .find(|vm_id| conflicting_vm_ids.contains(vm_id))
                    {
                        info!(
                            "VM '{target_vm_id}' is required for request but manager is currently starting conflicting VM '{starting_vm_id}'"
                        );
                        return Err(PwpError::ProxmoxNodeBusy);
                    }
                }

                let active_vm_future = self.clone().get_active_vm_future(proxy_target.clone());
                active_vm_futures_guard.insert(target_vm_id, active_vm_future.clone());
                active_vm_future
            }
        };

        active_vm_future.future.await
    }

    fn get_active_vm_future(
        self: Arc<Self>,
        proxy_target: Arc<PwpProxyTarget>,
    ) -> PwpActiveVmFuture<'static> {
        let target_vm_id = proxy_target.vm_id();

        let future = async move {
            let result = self.clone().start_vm(proxy_target).await;
            self.active_vm_futures.lock().await.remove(&target_vm_id);
            result
        }
        .boxed()
        .shared();

        PwpActiveVmFuture { future }
    }

    async fn start_vm(
        self: Arc<Self>,
        proxy_target: Arc<PwpProxyTarget>,
    ) -> Result<Arc<PwpOngoingRequestGuard>, PwpError> {
        let target_vm_id = proxy_target.vm_id();

        {
            let ongoing_request_guard = self
                .active_vms
                .lock()
                .await
                .get(&target_vm_id)
                .map(|active_vm| active_vm.handle_request());

            info!(
                "Ensuring VM '{target_vm_id}' is active for proxy target: {}",
                proxy_target.name()
            );

            self.clone().ensure_active_node().await?;

            let running_vm_ids = self
                .proxmox_node
                .get_running_vm_ids()
                .await?
                .ok_or(PwpError::ProxmoxNodeStartTimedOut)?;

            let target_is_mutually_exclusive = self.is_mutually_exclusive(target_vm_id);
            let conflicting_vm_ids = self.conflicting_vm_ids(target_vm_id);
            let running_mutually_exclusive_vm_ids = running_vm_ids
                .iter()
                .copied()
                .filter(|vm_id| {
                    if *vm_id == target_vm_id {
                        return false;
                    }

                    if conflicting_vm_ids.contains(vm_id) {
                        return true;
                    }

                    if !target_is_mutually_exclusive
                        || !self.treat_non_managed_vms_as_mutually_exclusive
                    {
                        return false;
                    }

                    if self
                        .proxy_targets
                        .iter()
                        .any(|proxy_target| proxy_target.vm_id() == *vm_id)
                    {
                        return false;
                    }

                    true
                })
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
            let can_start_vm = running_mutually_exclusive_vm_ids.is_empty();

            if need_to_start_vm && can_start_vm {
                self.proxmox_node.start_vm(target_vm_id).await?;
            } else if need_to_start_vm {
                info!(
                    "VM '{target_vm_id}' needs to be started on the Proxmox node, which is already running (a) conflicting VM(s): {running_mutually_exclusive_vm_ids:?}"
                );
                return Err(PwpError::ProxmoxNodeBusy);
            }

            let mut vm_running = self.proxmox_node.ping_vm_guest_agent(target_vm_id).await?;

            if !vm_running {
                info!("VM guest agent ping failed, will wait for VM to start");

                let mut vm_start_timeout = sleep(self.vm_start_timeout_duration).boxed();

                while !vm_running {
                    select! {
                        _ = &mut vm_start_timeout => {
                            let active_vm = match self.proxmox_node.get_running_vm_ids().await? {
                                Some(running_vm_ids) if running_vm_ids.contains(&target_vm_id) => {
                                    info!("Failed to start VM '{target_vm_id}' but it reports as running, will try shutting it down");

                                    let active_vm = {
                                        let mut active_vm_guard = self.active_vms.lock().await;
                                        let active_vm = PwpActiveVm::new(target_vm_id, true, Duration::ZERO);
                                        active_vm_guard.insert(target_vm_id, active_vm.clone());
                                        active_vm
                                    };

                                    self.active_vm_change_notify.notify_waiters();

                                    Some(active_vm)
                                }
                                _ => {
                                    warn!("Failed to start VM '{target_vm_id}'");
                                    None
                                },
                            };

                            return Err(PwpError::ProxmoxVmStartTimedOut(active_vm));
                        },
                        _ = sleep(self.guest_agent_ping_delay_duration) => {},
                    }

                    vm_running = self.proxmox_node.ping_vm_guest_agent(target_vm_id).await?;
                }
            }

            info!("VM guest agent ping succeeded");

            let (ongoing_request_guard, active_vm_created) =
                if let Some(ongoing_request_guard) = ongoing_request_guard {
                    (ongoing_request_guard, false)
                } else {
                    let mut active_vm_guard = self.active_vms.lock().await;
                    let active_vm = PwpActiveVm::new(
                        target_vm_id,
                        false,
                        proxy_target
                            .max_idle_duration()
                            .unwrap_or(self.max_idle_duration),
                    );
                    active_vm_guard.insert(target_vm_id, active_vm.clone());
                    (active_vm.handle_request(), true)
                };

            if active_vm_created {
                self.active_vm_change_notify.notify_waiters();
            }

            Ok(ongoing_request_guard)
        }
    }

    async fn ensure_active_node(self: Arc<Self>) -> Result<(), PwpError> {
        let active_node_future = self
            .active_node_future
            .lock()
            .await
            .get_or_insert_with(|| self.clone().get_active_node_future())
            .clone();

        active_node_future.future.await
    }

    fn get_active_node_future(self: Arc<Self>) -> PwpActiveNodeFuture<'static> {
        let future = async move {
            let result = self.clone().start_node().await;
            self.active_node_future.lock().await.take();
            result
        }
        .boxed()
        .shared();

        PwpActiveNodeFuture { future }
    }

    async fn start_node(self: Arc<Self>) -> Result<(), PwpError> {
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

        Ok(())
    }

    fn is_mutually_exclusive(&self, vm_id: u32) -> bool {
        self.mutually_exclusive_vm_ids
            .iter()
            .any(|group| group.contains(&vm_id))
    }

    fn conflicting_vm_ids(&self, vm_id: u32) -> BTreeSet<u32> {
        self.mutually_exclusive_vm_ids
            .iter()
            .filter(|group| group.contains(&vm_id))
            .flatten()
            .copied()
            .filter(|other_vm_id| *other_vm_id != vm_id)
            .collect()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test::initialize_test_context;
    use googletest::prelude::*;

    fn discard<T>(result: Result<T, PwpError>) -> Result<(), PwpError> {
        result.map(|_| ())
    }

    fn proxy_target(manager: &PwpManager, vm_id: u32) -> Arc<PwpProxyTarget> {
        manager
            .proxy_targets
            .iter()
            .find(|proxy_target| proxy_target.vm_id() == vm_id)
            .unwrap()
            .clone()
    }

    #[tokio::test]
    async fn is_mutually_exclusive_checks_all_groups() -> Result<(), PwpError> {
        let mut ctx = initialize_test_context(
            "mutually-exclusive-vm-ids = [[1, 2], [3, 4]]",
            "",
            &[("vm1", 1, "")],
        )
        .await?;
        ctx.mock_running_vms(&[]).await;
        let manager = ctx.manager().await?;

        assert_that!(manager.is_mutually_exclusive(1), is_true());
        assert_that!(manager.is_mutually_exclusive(4), is_true());
        assert_that!(manager.is_mutually_exclusive(5), is_false());

        Ok(())
    }

    #[tokio::test]
    async fn is_mutually_exclusive_without_groups_is_false() -> Result<(), PwpError> {
        let mut ctx = initialize_test_context("", "", &[("vm1", 1, "")]).await?;
        ctx.mock_running_vms(&[]).await;
        let manager = ctx.manager().await?;

        assert_that!(manager.is_mutually_exclusive(1), is_false());

        Ok(())
    }

    #[tokio::test]
    async fn conflicting_vm_ids_excludes_self_and_unrelated_groups() -> Result<(), PwpError> {
        let mut ctx = initialize_test_context(
            "mutually-exclusive-vm-ids = [[1, 2], [3, 4]]",
            "",
            &[("vm1", 1, "")],
        )
        .await?;
        ctx.mock_running_vms(&[]).await;
        let manager = ctx.manager().await?;

        assert_that!(manager.conflicting_vm_ids(1), elements_are![eq(&2)]);
        assert_that!(manager.conflicting_vm_ids(4), elements_are![eq(&3)]);
        assert_that!(manager.conflicting_vm_ids(5), is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn conflicting_vm_ids_is_not_transitive_across_groups() -> Result<(), PwpError> {
        let mut ctx = initialize_test_context(
            "mutually-exclusive-vm-ids = [[1, 2], [2, 3]]",
            "",
            &[("vm1", 1, "")],
        )
        .await?;
        ctx.mock_running_vms(&[]).await;
        let manager = ctx.manager().await?;

        assert_that!(manager.conflicting_vm_ids(1), elements_are![eq(&2)]);
        assert_that!(manager.conflicting_vm_ids(2), elements_are![eq(&1), eq(&3)]);
        assert_that!(manager.conflicting_vm_ids(3), elements_are![eq(&2)]);

        Ok(())
    }

    #[tokio::test]
    async fn conflicting_vm_ids_handles_group_with_duplicates() -> Result<(), PwpError> {
        let mut ctx = initialize_test_context(
            "mutually-exclusive-vm-ids = [[1, 1, 2]]",
            "",
            &[("vm1", 1, "")],
        )
        .await?;
        ctx.mock_running_vms(&[]).await;
        let manager = ctx.manager().await?;

        assert_that!(manager.conflicting_vm_ids(1), elements_are![eq(&2)]);

        Ok(())
    }

    #[gtest]
    fn format_instant_returns_rfc3339_for_future_instant() {
        let offset = chrono::Duration::hours(1);
        let before = Utc::now();
        let formatted = PwpManager::format_instant(Instant::now() + Duration::from_secs(3600));
        let after = Utc::now();

        let parsed = chrono::DateTime::parse_from_rfc3339(&formatted)
            .unwrap()
            .with_timezone(&Utc);

        assert_that!(parsed, all![ge(before + offset), le(after + offset)]);
    }

    #[tokio::test]
    async fn new_rejects_shared_vm_with_differing_max_idle_secs() -> Result<(), PwpError> {
        let mut ctx = initialize_test_context(
            "",
            "",
            &[
                ("a", 1, "max-idle-secs = 10"),
                ("b", 1, "max-idle-secs = 20"),
            ],
        )
        .await?;
        ctx.mock_running_vms(&[]).await;

        let result = ctx.manager().await;

        assert_that!(
            discard(result),
            err(matches_pattern!(PwpError::ConflictingSharedVmMaxIdleSecs(
                eq(&1)
            )))
        );

        Ok(())
    }

    #[tokio::test]
    async fn new_accepts_shared_vm_with_equal_max_idle_secs() -> Result<(), PwpError> {
        let mut ctx = initialize_test_context(
            "",
            "",
            &[
                ("a", 1, "max-idle-secs = 10"),
                ("b", 1, "max-idle-secs = 10"),
            ],
        )
        .await?;
        ctx.mock_running_vms(&[]).await;

        let manager = ctx.manager().await?;

        assert_that!(manager.proxy_targets.len(), eq(2));

        Ok(())
    }

    #[tokio::test]
    async fn new_tracks_only_running_managed_vms() -> Result<(), PwpError> {
        let mut ctx = initialize_test_context("", "", &[("vm1", 1, "")]).await?;
        ctx.mock_running_vms(&[1, 99]).await;
        let manager = ctx.manager().await?;

        let active_vm_ids = manager
            .active_vms
            .lock()
            .await
            .keys()
            .copied()
            .collect::<Vec<_>>();
        assert_that!(active_vm_ids, elements_are![eq(&1)]);

        Ok(())
    }

    #[tokio::test]
    async fn new_uses_target_max_idle_secs_for_running_vm() -> Result<(), PwpError> {
        let mut ctx = initialize_test_context(
            "max-idle-secs = 100",
            "",
            &[("vm1", 1, "max-idle-secs = 7"), ("vm2", 2, "")],
        )
        .await?;
        ctx.mock_running_vms(&[1, 2]).await;
        let manager = ctx.manager().await?;

        let active_vms = manager.active_vms.lock().await;
        assert_that!(
            active_vms[&1].max_idle_duration(),
            eq(Duration::from_secs(7))
        );
        assert_that!(
            active_vms[&2].max_idle_duration(),
            eq(Duration::from_secs(100))
        );

        Ok(())
    }

    #[tokio::test]
    async fn ensure_active_vm_does_not_start_running_vm() -> Result<(), PwpError> {
        let mut ctx = initialize_test_context("", "", &[("vm1", 1, "")]).await?;
        ctx.mock_running_vms(&[1]).await;
        let manager = ctx.manager().await?;
        let start = ctx
            .server
            .mock("POST", "/api2/json/nodes/node/qemu/1/status/start")
            .expect(0)
            .create_async()
            .await;
        let ping = ctx
            .server
            .mock("POST", "/api2/json/nodes/node/qemu/1/agent/ping")
            .with_body("{}")
            .expect(1)
            .create_async()
            .await;

        let result = manager
            .clone()
            .ensure_active_vm_for_target(proxy_target(&manager, 1))
            .await;

        assert_that!(discard(result), ok(anything()));
        start.assert_async().await;
        ping.assert_async().await;

        Ok(())
    }

    #[tokio::test]
    async fn ensure_active_vm_starts_stopped_vm_and_registers_it() -> Result<(), PwpError> {
        let mut ctx = initialize_test_context("", "", &[("vm1", 1, "")]).await?;
        ctx.mock_running_vms(&[]).await;
        let manager = ctx.manager().await?;
        let start = ctx
            .server
            .mock("POST", "/api2/json/nodes/node/qemu/1/status/start")
            .with_body("{}")
            .expect(1)
            .create_async()
            .await;
        ctx.server
            .mock("POST", "/api2/json/nodes/node/qemu/1/agent/ping")
            .with_body("{}")
            .create_async()
            .await;

        let result = manager
            .clone()
            .ensure_active_vm_for_target(proxy_target(&manager, 1))
            .await;

        assert_that!(discard(result), ok(anything()));
        start.assert_async().await;
        assert_that!(manager.active_vms.lock().await.contains_key(&1), is_true());
        assert_that!(manager.active_vm_futures.lock().await.len(), eq(0));

        Ok(())
    }

    #[tokio::test]
    async fn ensure_active_vm_rejects_vm_conflicting_with_running_one() -> Result<(), PwpError> {
        let mut ctx = initialize_test_context(
            "mutually-exclusive-vm-ids = [[1, 2]]",
            "",
            &[("vm1", 1, ""), ("vm2", 2, "")],
        )
        .await?;
        ctx.mock_running_vms(&[2]).await;
        let manager = ctx.manager().await?;
        let start = ctx
            .server
            .mock("POST", "/api2/json/nodes/node/qemu/1/status/start")
            .expect(0)
            .create_async()
            .await;

        let result = manager
            .clone()
            .ensure_active_vm_for_target(proxy_target(&manager, 1))
            .await;

        assert_that!(
            discard(result),
            err(matches_pattern!(PwpError::ProxmoxNodeBusy))
        );
        start.assert_async().await;

        Ok(())
    }

    #[tokio::test]
    async fn ensure_active_vm_starts_unrelated_vms_independently() -> Result<(), PwpError> {
        let mut ctx = initialize_test_context(
            "mutually-exclusive-vm-ids = [[1, 2]]",
            "",
            &[("vm1", 1, ""), ("vm3", 3, "")],
        )
        .await?;
        ctx.mock_running_vms(&[1]).await;
        let manager = ctx.manager().await?;
        let start = ctx
            .server
            .mock("POST", "/api2/json/nodes/node/qemu/3/status/start")
            .with_body("{}")
            .expect(1)
            .create_async()
            .await;
        ctx.server
            .mock("POST", mockito::Matcher::Regex(r"agent/ping$".into()))
            .with_body("{}")
            .create_async()
            .await;

        let (first, second) = tokio::join!(
            manager
                .clone()
                .ensure_active_vm_for_target(proxy_target(&manager, 1)),
            manager
                .clone()
                .ensure_active_vm_for_target(proxy_target(&manager, 3)),
        );

        assert_that!(discard(first), ok(anything()));
        assert_that!(discard(second), ok(anything()));
        start.assert_async().await;

        Ok(())
    }
}
