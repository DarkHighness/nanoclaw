use memory::MemoryBackend;
use std::sync::Arc;
use std::time::Duration;
use types::PluginId;

pub(super) fn maybe_spawn_memory_background_sync(
    backend: Arc<dyn MemoryBackend>,
    plugin_id: &PluginId,
    enabled: bool,
    run_on_start: bool,
    interval_ms: u64,
    warnings: &mut Vec<String>,
) {
    if !enabled {
        return;
    }
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        warnings.push(format!(
            "plugin `{plugin_id}` requested background sync but no tokio runtime was active during boot"
        ));
        return;
    };
    let plugin_id = plugin_id.to_string();
    handle.spawn(async move {
        if run_on_start {
            if let Err(error) = backend.sync().await {
                tracing::warn!(plugin_id, error = %error, "memory background sync failed during startup");
            }
        }

        let mut interval =
            tokio::time::interval(Duration::from_millis(interval_ms.max(1_000)));
        // Tokio intervals tick immediately on first poll. Consume that eager
        // tick so `run_on_start = false` really means “wait one full interval”.
        interval.tick().await;
        loop {
            interval.tick().await;
            if let Err(error) = backend.sync().await {
                tracing::warn!(plugin_id, error = %error, "memory background sync failed");
            }
        }
    });
}
