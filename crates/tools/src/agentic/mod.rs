mod checkpoint;
#[cfg(feature = "automation-tools")]
mod cron;
mod request_permissions;
mod request_user_input;
mod review;
mod skill;
mod task;
mod tool_discovery;
mod worktree;

/// Host-coordinated prompt tools stay hidden when the active session cannot
/// service interactive prompts.
pub const HOST_FEATURE_REQUEST_USER_INPUT: &str = "host-user-input";
pub const HOST_FEATURE_REQUEST_PERMISSIONS: &str = "host-permission-request";

pub use checkpoint::*;
#[cfg(feature = "automation-tools")]
pub use cron::*;
pub use request_permissions::*;
pub use request_user_input::*;
pub use review::*;
pub use skill::*;
pub use task::*;
pub use tool_discovery::*;
pub use worktree::*;
