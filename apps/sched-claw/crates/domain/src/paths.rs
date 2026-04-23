use std::path::{Path, PathBuf};

pub const APP_NAME: &str = "sched-claw";

#[must_use]
pub fn app_state_dir(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".nanoclaw/apps").join(APP_NAME)
}
