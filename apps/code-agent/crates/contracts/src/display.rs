use serde::Deserialize;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct TuiDisplayConfig {
    pub welcome_ascii_logo: bool,
    pub top_turn_title: bool,
}

impl Default for TuiDisplayConfig {
    fn default() -> Self {
        Self {
            welcome_ascii_logo: true,
            top_turn_title: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TuiDisplayConfig;

    #[test]
    fn display_defaults_keep_logo_and_turn_title_enabled() {
        let config = TuiDisplayConfig::default();
        assert!(config.welcome_ascii_logo);
        assert!(config.top_turn_title);
    }
}
