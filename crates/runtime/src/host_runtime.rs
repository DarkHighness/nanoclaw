use std::io;

/// Shared host-level Tokio runtime limits.
///
/// Host apps must build their Tokio runtime before the agent runtime exists, so
/// this helper centralizes validation and blocking-pool policy instead of
/// duplicating it in each binary entrypoint.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HostRuntimeLimits {
    pub worker_threads: Option<usize>,
    pub max_blocking_threads: Option<usize>,
}

pub fn build_host_tokio_runtime(limits: HostRuntimeLimits) -> io::Result<tokio::runtime::Runtime> {
    let mut builder = tokio::runtime::Builder::new_multi_thread();
    builder.enable_all();
    if let Some(worker_threads) = limits.worker_threads {
        if worker_threads == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "runtime.tokio_worker_threads must be greater than zero",
            ));
        }
        builder.worker_threads(worker_threads);
    }
    if let Some(max_blocking_threads) = limits.max_blocking_threads {
        if max_blocking_threads == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "runtime.tokio_max_blocking_threads must be greater than zero",
            ));
        }
        // Memory indexing and other filesystem-backed helpers use the blocking
        // pool for host-only work. Keeping the cap here makes those limits
        // consistent across app entrypoints.
        builder.max_blocking_threads(max_blocking_threads);
    }
    builder.build()
}

#[cfg(test)]
mod tests {
    use super::{HostRuntimeLimits, build_host_tokio_runtime};

    #[test]
    fn rejects_zero_worker_threads() {
        let error = build_host_tokio_runtime(HostRuntimeLimits {
            worker_threads: Some(0),
            max_blocking_threads: None,
        })
        .unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("tokio_worker_threads"));
    }

    #[test]
    fn rejects_zero_max_blocking_threads() {
        let error = build_host_tokio_runtime(HostRuntimeLimits {
            worker_threads: None,
            max_blocking_threads: Some(0),
        })
        .unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(error.to_string().contains("tokio_max_blocking_threads"));
    }

    #[test]
    fn builds_runtime_with_explicit_limits() {
        let runtime = build_host_tokio_runtime(HostRuntimeLimits {
            worker_threads: Some(1),
            max_blocking_threads: Some(1),
        })
        .unwrap();

        runtime.block_on(async {});
    }
}
