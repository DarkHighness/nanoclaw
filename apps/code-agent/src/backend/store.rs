use agent::{FileRunStore, InMemoryRunStore, RunStore};
use anyhow::Result;
use nanoclaw_config::CoreConfig;
use std::path::Path;
use std::sync::Arc;
use tracing::warn;

pub(crate) async fn build_store(
    core: &CoreConfig,
    workspace_root: &Path,
) -> Result<Arc<dyn RunStore>> {
    let store_dir = core.resolved_store_dir(workspace_root);
    match FileRunStore::open(&store_dir).await {
        Ok(store) => Ok(Arc::new(store)),
        Err(error) => {
            let warning = format!(
                "failed to initialize file run store at {}: {error}",
                store_dir.display()
            );
            warn!("{warning}; falling back to in-memory store");
            Ok(Arc::new(InMemoryRunStore::new()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::build_store;
    use nanoclaw_config::CoreConfig;
    use tempfile::tempdir;

    #[tokio::test]
    async fn build_store_prefers_file_backed_runs() {
        let dir = tempdir().unwrap();
        let store = build_store(&CoreConfig::default(), dir.path())
            .await
            .unwrap();

        assert!(store.list_runs().await.unwrap().is_empty());
    }
}
