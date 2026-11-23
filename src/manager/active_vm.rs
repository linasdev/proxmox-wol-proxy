use log::info;
use std::fmt::{Debug, Formatter};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::Duration;
use tokio::sync::Notify;
use tokio::time::Instant;

static BASE_TIME: LazyLock<Instant> = LazyLock::new(Instant::now);

#[derive(Clone)]
pub struct PwpActiveVm {
    inner: Arc<PwpActiveVmInner>,
}

pub struct PwpActiveVmInner {
    vm_id: u32,
    ongoing_request_count: AtomicU32,
    idle_since_secs: AtomicU64,
    idle_notify: Notify,
    busy_notify: Notify,
    max_idle_duration: Duration,
}

impl PwpActiveVm {
    pub fn new(vm_id: u32, is_idle: bool, max_idle_duration: Duration) -> Self {
        let ongoing_request_count = AtomicU32::new(0);
        let idle_since_secs = AtomicU64::new(Instant::now().duration_since(*BASE_TIME).as_secs());
        let idle_notify = Notify::new();
        let busy_notify = Notify::new();

        if is_idle {
            idle_notify.notify_one();
        }

        let inner = Arc::new(PwpActiveVmInner {
            vm_id,
            ongoing_request_count,
            idle_since_secs,
            idle_notify,
            busy_notify,
            max_idle_duration,
        });

        Self { inner }
    }

    pub fn idle_notify(&self) -> &Notify {
        &self.inner.idle_notify
    }

    pub fn busy_notify(&self) -> &Notify {
        &self.inner.busy_notify
    }

    pub fn max_idle_duration(&self) -> Duration {
        self.inner.max_idle_duration
    }

    pub fn handle_request(&self) -> Arc<PwpOngoingRequestGuard> {
        self.inner
            .ongoing_request_count
            .fetch_add(1, Ordering::Relaxed);
        self.inner.busy_notify.notify_waiters();
        let ongoing_request_guard = PwpOngoingRequestGuard {
            inner: self.inner.clone(),
        };

        Arc::new(ongoing_request_guard)
    }

    pub fn shutdown_at(&self) -> Instant {
        *BASE_TIME
            + Duration::from_secs(self.inner.idle_since_secs.load(Ordering::Acquire))
            + self.inner.max_idle_duration
    }
}

impl Debug for PwpActiveVm {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> std::fmt::Result {
        fmt.debug_struct("PwpActiveVm")
            .field("vm_id", &self.inner.vm_id)
            .finish_non_exhaustive()
    }
}

pub struct PwpOngoingRequestGuard {
    inner: Arc<PwpActiveVmInner>,
}

impl Drop for PwpOngoingRequestGuard {
    fn drop(&mut self) {
        self.inner
            .ongoing_request_count
            .fetch_sub(1, Ordering::Relaxed);
        if self.inner.ongoing_request_count.load(Ordering::Acquire) == 0 {
            info!("VM '{}' became idle", self.inner.vm_id);

            self.inner.idle_since_secs.store(
                Instant::now().duration_since(*BASE_TIME).as_secs(),
                Ordering::Release,
            );
            self.inner.idle_notify.notify_one();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use googletest::prelude::*;
    use tokio::time::timeout;

    const SHORT: Duration = Duration::from_millis(50);

    #[tokio::test]
    async fn new_idle_vm_notifies_idle() {
        let vm = PwpActiveVm::new(1, true, Duration::from_secs(10));

        assert_that!(
            timeout(SHORT, vm.idle_notify().notified()).await,
            ok(anything())
        );
    }

    #[tokio::test]
    async fn new_busy_vm_does_not_notify_idle() {
        let vm = PwpActiveVm::new(1, false, Duration::from_secs(10));

        assert_that!(
            timeout(SHORT, vm.idle_notify().notified()).await,
            err(anything())
        );
    }

    #[tokio::test]
    async fn dropping_last_guard_notifies_idle() {
        let vm = PwpActiveVm::new(1, false, Duration::from_secs(10));
        let first = vm.handle_request();
        let second = vm.handle_request();

        drop(first);
        assert_that!(
            timeout(SHORT, vm.idle_notify().notified()).await,
            err(anything())
        );

        drop(second);
        assert_that!(
            timeout(SHORT, vm.idle_notify().notified()).await,
            ok(anything())
        );
    }

    #[tokio::test]
    async fn handle_request_notifies_busy_waiters() {
        let vm = PwpActiveVm::new(1, true, Duration::from_secs(10));
        let waiter = {
            let vm = vm.clone();
            tokio::spawn(async move { vm.busy_notify().notified().await })
        };
        tokio::task::yield_now().await;

        let _guard = vm.handle_request();

        assert_that!(timeout(SHORT, waiter).await, ok(anything()));
    }

    #[tokio::test]
    async fn shutdown_at_is_idle_since_plus_max_idle() {
        let max_idle = Duration::from_secs(120);
        let vm = PwpActiveVm::new(1, true, max_idle);
        let shutdown_at = vm.shutdown_at();
        let now = Instant::now();

        assert_that!(vm.max_idle_duration(), eq(max_idle));
        assert_that!(
            shutdown_at + Duration::from_secs(1) >= now + max_idle,
            is_true()
        );
        assert_that!(shutdown_at <= now + max_idle, is_true());
    }

    #[tokio::test]
    async fn debug_output_contains_vm_id() {
        let vm = PwpActiveVm::new(77, true, Duration::ZERO);

        assert_that!(format!("{vm:?}").contains("77"), is_true());
    }
}
