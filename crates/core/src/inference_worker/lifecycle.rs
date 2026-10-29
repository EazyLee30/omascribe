//! Process-local, text-free worker lifecycle evidence. Counters survive newer
//! successful releases; in particular an unconfirmed exit stays adverse.
use serde::Serialize;
use std::time::Duration;

pub(super) fn millis(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

#[derive(Clone, Serialize)]
pub struct WorkerLifecycle {
    pub schema: u32,
    pub releases: u64,
    pub ttl_releases: u64,
    pub batch_releases: u64,
    pub lease_releases: u64,
    pub other_releases: u64,
    pub resource_stops: u64,
    pub admission_refusals: u64,
    pub retention_refusals: u64,
    pub last_release: Option<Release>,
}
impl Default for WorkerLifecycle {
    fn default() -> Self {
        Self {
            schema: 1,
            releases: 0,
            ttl_releases: 0,
            batch_releases: 0,
            lease_releases: 0,
            other_releases: 0,
            resource_stops: 0,
            admission_refusals: 0,
            retention_refusals: 0,
            last_release: None,
        }
    }
}

#[derive(Clone, Serialize)]
pub struct Release {
    pub sequence: u64,
    pub pid: u32,
    pub reason: &'static str,
    pub ttl_ms: u64,
    pub idle_ms: Option<u64>,
    pub exit_confirmed: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ReleaseReason {
    TtlExpired,
    BatchEnd,
    BatchGraceExpired,
    LeaseEnd,
    Invalidated,
    ScopeChanged,
    ModelChanged,
    WorkerUnhealthy,
    WorkerExited,
    RequestFailed,
    LowMemory,
    MemoryUnknown,
    ProcessBudget,
    SystemPressure,
    Timeout,
    Cancelled,
    AsrBacklog,
    Maintenance,
    StartupFailed,
    ExitUnconfirmed,
}
impl ReleaseReason {
    pub(super) fn policy(self) -> bool {
        matches!(
            self,
            Self::TtlExpired | Self::BatchEnd | Self::BatchGraceExpired | Self::LeaseEnd
        )
    }
    fn name(self) -> &'static str {
        match self {
            Self::TtlExpired => "ttl_expired",
            Self::BatchEnd => "batch_end",
            Self::BatchGraceExpired => "batch_grace_expired",
            Self::LeaseEnd => "lease_end",
            Self::Invalidated => "invalidated",
            Self::ScopeChanged => "scope_changed",
            Self::ModelChanged => "model_changed",
            Self::WorkerUnhealthy => "worker_unhealthy",
            Self::WorkerExited => "worker_exited",
            Self::RequestFailed => "request_failed",
            Self::LowMemory => "low_memory",
            Self::MemoryUnknown => "memory_unknown",
            Self::ProcessBudget => "process_budget",
            Self::SystemPressure => "system_pressure",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
            Self::AsrBacklog => "asr_backlog",
            Self::Maintenance => "maintenance",
            Self::StartupFailed => "startup_failed",
            Self::ExitUnconfirmed => "exit_unconfirmed",
        }
    }
}
impl WorkerLifecycle {
    /// Called after cleanup finishes, under the pool lock. Completion-order
    /// sequence allocation prevents an older slow waiter overwriting a newer
    /// release with a smaller sequence or exposing an unconfirmed vacant slot.
    pub(super) fn released(
        &mut self,
        pid: u32,
        reason: ReleaseReason,
        ttl: Duration,
        idle: Option<Duration>,
        exit_confirmed: bool,
    ) {
        let reason = if exit_confirmed {
            reason
        } else {
            ReleaseReason::ExitUnconfirmed
        };
        match reason {
            ReleaseReason::TtlExpired => self.ttl_releases += 1,
            ReleaseReason::BatchEnd | ReleaseReason::BatchGraceExpired => self.batch_releases += 1,
            ReleaseReason::LeaseEnd => self.lease_releases += 1,
            _ => self.other_releases += 1,
        }
        self.releases += 1;
        self.last_release = Some(Release {
            sequence: self.releases,
            pid,
            reason: reason.name(),
            ttl_ms: millis(ttl),
            idle_ms: idle.map(millis),
            exit_confirmed,
        });
    }
}
