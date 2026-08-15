use log::info;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::Duration;
use tokio::sync::Notify;
use tokio::time::Instant;

static BASE_TIME: LazyLock<Instant> = LazyLock::new(Instant::now);

#[derive(Clone)]
pub struct PwpActiveVm {
    vm_id: u32,
    ongoing_request_count: Arc<AtomicU32>,
    idle_since_secs: Arc<AtomicU64>,
    pub idle_notify: Arc<Notify>,
    pub busy_notify: Arc<Notify>,
}

impl PwpActiveVm {
    pub fn new(vm_id: u32, is_idle: bool) -> Self {
        let ongoing_request_count = Arc::new(AtomicU32::new(0));
        let idle_since_secs = Arc::new(AtomicU64::new(
            Instant::now().duration_since(*BASE_TIME).as_secs(),
        ));
        let idle_notify = Arc::new(Notify::new());
        let busy_notify = Arc::new(Notify::new());

        if is_idle {
            idle_notify.notify_one();
        }

        Self {
            vm_id,
            ongoing_request_count,
            idle_since_secs,
            idle_notify,
            busy_notify,
        }
    }

    pub fn handle_request(&self) -> Arc<PwpOngoingRequestGuard> {
        self.ongoing_request_count.fetch_add(1, Ordering::Relaxed);
        self.busy_notify.notify_waiters();
        let ongoing_request_guard = PwpOngoingRequestGuard {
            vm_id: self.vm_id,
            ongoing_request_count: self.ongoing_request_count.clone(),
            idle_since_secs: self.idle_since_secs.clone(),
            idle_notify: self.idle_notify.clone(),
        };

        Arc::new(ongoing_request_guard)
    }

    pub fn idle_since(&self) -> Instant {
        *BASE_TIME + Duration::from_secs(self.idle_since_secs.load(Ordering::Acquire))
    }
}

pub struct PwpOngoingRequestGuard {
    vm_id: u32,
    ongoing_request_count: Arc<AtomicU32>,
    idle_since_secs: Arc<AtomicU64>,
    idle_notify: Arc<Notify>,
}

impl Drop for PwpOngoingRequestGuard {
    fn drop(&mut self) {
        self.ongoing_request_count.fetch_sub(1, Ordering::Relaxed);
        if self.ongoing_request_count.load(Ordering::Acquire) == 0 {
            info!("VM '{}' became idle", self.vm_id);

            self.idle_since_secs.store(
                Instant::now().duration_since(*BASE_TIME).as_secs(),
                Ordering::Release,
            );
            self.idle_notify.notify_one();
        }
    }
}
