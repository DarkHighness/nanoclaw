use serde::Deserialize;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub(crate) struct StatusLineConfig {
    pub(crate) status: bool,
    pub(crate) model: bool,
    pub(crate) cwd: bool,
    pub(crate) repo: bool,
    pub(crate) branch: bool,
    pub(crate) context_window: bool,
    pub(crate) input_tokens: bool,
    pub(crate) output_tokens: bool,
    pub(crate) queue: bool,
    pub(crate) clock: bool,
    pub(crate) session: bool,
}

impl Default for StatusLineConfig {
    fn default() -> Self {
        Self {
            status: true,
            model: true,
            cwd: true,
            repo: true,
            branch: true,
            context_window: true,
            input_tokens: true,
            output_tokens: true,
            queue: true,
            clock: true,
            session: false,
        }
    }
}
