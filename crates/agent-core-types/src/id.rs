use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[must_use]
pub fn new_opaque_id() -> String {
    Uuid::new_v4().to_string()
}

macro_rules! define_id {
    ($name:ident) => {
        #[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub struct $name(pub String);

        impl $name {
            #[must_use]
            pub fn new() -> Self {
                Self(new_opaque_id())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }
    };
}

define_id!(RunId);
define_id!(SessionId);
define_id!(TurnId);
define_id!(ToolCallId);
