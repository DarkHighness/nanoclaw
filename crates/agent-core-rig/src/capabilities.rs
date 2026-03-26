#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RigProviderKind {
    OpenAi,
    Anthropic,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RigProviderDescriptor {
    pub provider: RigProviderKind,
    pub model: String,
}

impl RigProviderDescriptor {
    #[must_use]
    pub fn openai(model: impl Into<String>) -> Self {
        Self {
            provider: RigProviderKind::OpenAi,
            model: model.into(),
        }
    }

    #[must_use]
    pub fn anthropic(model: impl Into<String>) -> Self {
        Self {
            provider: RigProviderKind::Anthropic,
            model: model.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RigProviderCapabilities {
    pub streaming: bool,
    pub tool_calls: bool,
    pub multimodal_messages: bool,
    pub provider_managed_history: bool,
    pub provider_native_compaction: bool,
}

impl Default for RigProviderCapabilities {
    fn default() -> Self {
        Self {
            streaming: true,
            tool_calls: true,
            multimodal_messages: true,
            provider_managed_history: false,
            provider_native_compaction: false,
        }
    }
}
