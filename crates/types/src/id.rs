use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::borrow::Borrow;
use std::fmt;
use uuid::Uuid;

#[must_use]
pub fn new_opaque_id() -> String {
    Uuid::new_v4().to_string()
}

macro_rules! define_id {
    ($name:ident) => {
        #[derive(
            Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
        )]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            #[must_use]
            pub fn new() -> Self {
                Self(new_opaque_id())
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }

            #[must_use]
            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_string())
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

macro_rules! define_string_identifier {
    ($name:ident) => {
        #[derive(
            Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
        )]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }

            #[must_use]
            pub fn into_inner(self) -> String {
                self.0
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_string())
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl Borrow<str> for $name {
            fn borrow(&self) -> &str {
                self.as_str()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }
    };
}

define_id!(EventId);
define_id!(EnvelopeId);
define_id!(MessageId);
define_id!(SessionId);
define_id!(AgentSessionId);
define_id!(TurnId);
define_id!(ToolCallId);
define_id!(CallId);
define_id!(ResponseId);
define_id!(ReasoningId);
define_id!(AgentId);
define_string_identifier!(TaskId);
define_string_identifier!(CronId);
define_string_identifier!(MonitorId);
define_string_identifier!(BrowserId);
define_string_identifier!(WorktreeId);
define_string_identifier!(CheckpointId);
define_string_identifier!(PluginId);
define_string_identifier!(PluginDriverId);
define_string_identifier!(HookName);
define_string_identifier!(McpServerName);

impl From<ToolCallId> for CallId {
    fn from(value: ToolCallId) -> Self {
        Self::from(value.into_inner())
    }
}

impl From<&ToolCallId> for CallId {
    fn from(value: &ToolCallId) -> Self {
        Self::from(value.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::{CallId, ToolCallId};

    #[test]
    fn tool_call_id_converts_to_call_id_without_stringly_call_site_code() {
        let tool_call_id = ToolCallId::from("tool-call-1");
        let call_id = CallId::from(&tool_call_id);
        assert_eq!(call_id.as_str(), tool_call_id.as_str());
    }
}
