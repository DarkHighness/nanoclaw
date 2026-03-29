use crate::config::AgentCoreConfig;
use anyhow::Result;
use std::path::Path;
use std::sync::Arc;
use store::{FileSessionStore, InMemorySessionStore, SessionStore};
use tracing::warn;

pub(super) struct StoreHandle {
    pub(super) store: Arc<dyn SessionStore>,
    pub(super) label: String,
    pub(super) warning: Option<String>,
}

pub(super) async fn build_store(
    config: &AgentCoreConfig,
    workspace_root: &Path,
) -> Result<StoreHandle> {
    let store_dir = config.resolved_store_dir(workspace_root);
    match FileSessionStore::open(&store_dir).await {
        Ok(store) => Ok(StoreHandle {
            store: Arc::new(store),
            label: format!("file {}", store_dir.display()),
            warning: None,
        }),
        Err(error) => {
            let warning = format!(
                "failed to initialize file run store at {}: {error}",
                store_dir.display()
            );
            warn!("{warning}; falling back to in-memory store");
            Ok(StoreHandle {
                store: Arc::new(InMemorySessionStore::new()),
                label: "memory fallback".to_string(),
                warning: Some(warning),
            })
        }
    }
}
