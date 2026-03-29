use agent::{FileSessionStore, InMemorySessionStore, SessionStore};
use anyhow::Result;
use nanoclaw_config::CoreConfig;
use std::path::Path;
use std::sync::Arc;
use tracing::warn;

pub(crate) struct SessionStoreHandle {
    pub(crate) store: Arc<dyn SessionStore>,
    pub(crate) label: String,
    pub(crate) warning: Option<String>,
}

pub(crate) async fn build_store(
    core: &CoreConfig,
    workspace_root: &Path,
) -> Result<SessionStoreHandle> {
    let store_dir = core.resolved_store_dir(workspace_root);
    match FileSessionStore::open(&store_dir).await {
        Ok(store) => Ok(SessionStoreHandle {
            store: Arc::new(store),
            label: format!("file {}", store_dir.display()),
            warning: None,
        }),
        Err(error) => {
            let warning = format!(
                "failed to initialize file session store at {}: {error}",
                store_dir.display()
            );
            warn!("{warning}; falling back to in-memory store");
            Ok(SessionStoreHandle {
                store: Arc::new(InMemorySessionStore::new()),
                label: "memory fallback".to_string(),
                warning: Some(warning),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::build_store;
    use nanoclaw_config::CoreConfig;
    use tempfile::tempdir;

    #[tokio::test]
    async fn build_store_prefers_file_backed_sessions() {
        let dir = tempdir().unwrap();
        let core = CoreConfig::default();
        let handle = build_store(&core, dir.path()).await.unwrap();

        assert_eq!(
            handle.label,
            format!("file {}", core.resolved_store_dir(dir.path()).display())
        );
        assert!(handle.warning.is_none());
        assert!(handle.store.list_sessions().await.unwrap().is_empty());
    }
}
