#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderKind {
    OpenAi,
    Anthropic,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderDescriptor {
    pub kind: ProviderKind,
    pub model: String,
}

impl ProviderDescriptor {
    #[must_use]
    pub fn openai(model: impl Into<String>) -> Self {
        Self {
            kind: ProviderKind::OpenAi,
            model: model.into(),
        }
    }

    #[must_use]
    pub fn anthropic(model: impl Into<String>) -> Self {
        Self {
            kind: ProviderKind::Anthropic,
            model: model.into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderCapabilities {
    pub streaming: bool,
    pub tool_calls: bool,
    pub multimodal_messages: bool,
    pub provider_managed_history: bool,
    pub provider_native_compaction: bool,
}

impl Default for ProviderCapabilities {
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
