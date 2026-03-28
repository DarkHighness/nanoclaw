//! Shared helpers for bounded async test runtimes.
//!
//! Some substrate tests indirectly hit Tokio's blocking pool through SQLite,
//! LanceDB, or filesystem wrappers. These helpers make those limits explicit so
//! slow or runaway tests do not silently fan out across all host CPUs.

use std::future::Future;

pub const TEST_MAX_BLOCKING_THREADS_ENV: &str = "NANOCLAW_TEST_MAX_BLOCKING_THREADS";
pub const DEFAULT_TEST_MAX_BLOCKING_THREADS: usize = 1;

#[must_use]
pub fn current_thread_max_blocking_threads_from_env() -> usize {
    std::env::var(TEST_MAX_BLOCKING_THREADS_ENV)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_TEST_MAX_BLOCKING_THREADS)
}

pub fn run_current_thread_test<F>(future: F) -> F::Output
where
    F: Future,
{
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(current_thread_max_blocking_threads_from_env())
        .build()
        .expect("failed to build bounded test runtime");
    runtime.block_on(future)
}

#[cfg(test)]
mod tests {
    use super::{
        DEFAULT_TEST_MAX_BLOCKING_THREADS, TEST_MAX_BLOCKING_THREADS_ENV,
        current_thread_max_blocking_threads_from_env, run_current_thread_test,
    };
    use std::sync::{Mutex, OnceLock};

    fn env_test_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn uses_default_limit_when_env_is_absent() {
        let _guard = env_test_lock().lock().unwrap();
        unsafe {
            std::env::remove_var(TEST_MAX_BLOCKING_THREADS_ENV);
        }

        assert_eq!(
            current_thread_max_blocking_threads_from_env(),
            DEFAULT_TEST_MAX_BLOCKING_THREADS
        );
    }

    #[test]
    fn ignores_invalid_env_values() {
        let _guard = env_test_lock().lock().unwrap();
        unsafe {
            std::env::set_var(TEST_MAX_BLOCKING_THREADS_ENV, "0");
        }
        assert_eq!(
            current_thread_max_blocking_threads_from_env(),
            DEFAULT_TEST_MAX_BLOCKING_THREADS
        );

        unsafe {
            std::env::set_var(TEST_MAX_BLOCKING_THREADS_ENV, "invalid");
        }
        assert_eq!(
            current_thread_max_blocking_threads_from_env(),
            DEFAULT_TEST_MAX_BLOCKING_THREADS
        );
    }

    #[test]
    fn respects_env_override() {
        let _guard = env_test_lock().lock().unwrap();
        unsafe {
            std::env::set_var(TEST_MAX_BLOCKING_THREADS_ENV, "3");
        }

        assert_eq!(current_thread_max_blocking_threads_from_env(), 3);
    }

    #[test]
    fn runs_future_on_bounded_runtime() {
        run_current_thread_test(async {
            let value = tokio::task::spawn_blocking(|| 7usize).await.unwrap();
            assert_eq!(value, 7);
        });
    }
}
