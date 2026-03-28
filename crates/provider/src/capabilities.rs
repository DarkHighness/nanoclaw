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
