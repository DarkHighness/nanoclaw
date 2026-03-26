use std::path::PathBuf;

/// Best-effort file activity hook for background services such as LSP runtimes.
///
/// Local file tools should stay user-visible and deterministic even when optional
/// background integrations are unavailable, so implementations must not block the
/// caller on long startup or network work.
pub trait FileActivityObserver: Send + Sync {
    fn did_open(&self, path: PathBuf);

    fn did_change(&self, path: PathBuf);

    fn did_save(&self, path: PathBuf);

    fn did_remove(&self, path: PathBuf);
}
