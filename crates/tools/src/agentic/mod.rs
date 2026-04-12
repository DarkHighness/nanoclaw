mod plan;
mod request_permissions;
mod request_user_input;
mod skill;
mod task;
mod tool_discovery;

/// Host-coordinated prompt tools stay hidden when the active session cannot
/// service interactive prompts.
pub const HOST_FEATURE_REQUEST_USER_INPUT: &str = "host-user-input";
pub const HOST_FEATURE_REQUEST_PERMISSIONS: &str = "host-permission-request";

pub use plan::*;
pub use request_permissions::*;
pub use request_user_input::*;
pub use skill::*;
pub use task::*;
pub use tool_discovery::*;
