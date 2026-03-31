//! File-backed TUI theme loading for code-agent.
//!
//! Builtin themes live in a checked-in TOML asset that is embedded with
//! `include_str!`, while user workspaces may point `tui.theme_file` at their
//! own catalog file. The runtime only deals with parsed theme catalogs and the
//! active theme id; it does not hardcode palette values in Rust source.

use anyhow::{Context, Result, bail, ensure};
use ratatui::style::Color;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

const BUILTIN_THEME_CATALOG: &str = include_str!("../themes/defaults.toml");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ThemePalette {
    pub(crate) bg: Color,
    pub(crate) main_bg: Color,
    pub(crate) footer_bg: Color,
    pub(crate) bottom_pane_bg: Color,
    pub(crate) border_active: Color,
    pub(crate) text: Color,
    pub(crate) muted: Color,
    pub(crate) subtle: Color,
    pub(crate) accent: Color,
    pub(crate) user: Color,
    pub(crate) assistant: Color,
    pub(crate) error: Color,
    pub(crate) warn: Color,
    pub(crate) header: Color,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ThemeDefinition {
    pub(crate) id: String,
    pub(crate) summary: String,
    pub(crate) palette: ThemePalette,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ThemeSummary {
    pub(crate) id: String,
    pub(crate) summary: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ThemeCatalog {
    pub(crate) active_theme: String,
    pub(crate) themes: Vec<ThemeDefinition>,
}

impl ThemeCatalog {
    pub(crate) fn summaries(&self) -> Vec<ThemeSummary> {
        self.themes
            .iter()
            .map(|theme| ThemeSummary {
                id: theme.id.clone(),
                summary: theme.summary.clone(),
            })
            .collect()
    }

    pub(crate) fn contains(&self, theme_id: &str) -> bool {
        self.themes.iter().any(|theme| theme.id == theme_id)
    }

    pub(crate) fn palette_for(&self, theme_id: &str) -> Option<ThemePalette> {
        self.themes
            .iter()
            .find(|theme| theme.id == theme_id)
            .map(|theme| theme.palette)
    }
}

#[derive(Clone, Debug)]
struct ActiveThemeState {
    catalog: ThemeCatalog,
    active_theme: String,
}

static ACTIVE_THEME_STATE: OnceLock<RwLock<ActiveThemeState>> = OnceLock::new();

fn active_theme_lock() -> &'static RwLock<ActiveThemeState> {
    ACTIVE_THEME_STATE.get_or_init(|| {
        let catalog =
            parse_theme_catalog(BUILTIN_THEME_CATALOG).expect("builtin theme catalog must parse");
        let active_theme = catalog.active_theme.clone();
        RwLock::new(ActiveThemeState {
            catalog,
            active_theme,
        })
    })
}

pub(crate) fn install_theme_catalog(catalog: ThemeCatalog) {
    let active_theme = catalog.active_theme.clone();
    *active_theme_lock()
        .write()
        .expect("active theme lock poisoned") = ActiveThemeState {
        catalog,
        active_theme,
    };
}

pub(crate) fn theme_summaries() -> Vec<ThemeSummary> {
    active_theme_lock()
        .read()
        .expect("active theme lock poisoned")
        .catalog
        .summaries()
}

pub(crate) fn active_theme_id() -> String {
    active_theme_lock()
        .read()
        .expect("active theme lock poisoned")
        .active_theme
        .clone()
}

pub(crate) fn set_active_theme(theme_id: &str) -> Result<()> {
    let mut state = active_theme_lock()
        .write()
        .expect("active theme lock poisoned");
    ensure!(
        state.catalog.contains(theme_id),
        "unknown theme `{theme_id}`"
    );
    state.active_theme = theme_id.to_string();
    Ok(())
}

pub(crate) fn active_palette() -> ThemePalette {
    let state = active_theme_lock()
        .read()
        .expect("active theme lock poisoned");
    state
        .catalog
        .palette_for(&state.active_theme)
        .expect("active theme must resolve to a palette")
}

pub(crate) fn load_theme_catalog(
    workspace_root: &Path,
    theme_file: Option<&str>,
    active_override: Option<&str>,
) -> Result<ThemeCatalog> {
    let mut catalog_file = parse_theme_catalog_file(BUILTIN_THEME_CATALOG)
        .context("failed to parse builtin code-agent themes")?;
    if let Some(theme_file) = theme_file {
        let path = resolve_path(workspace_root, theme_file);
        let raw = std::fs::read_to_string(&path).with_context(|| {
            format!("failed to read code-agent theme catalog {}", path.display())
        })?;
        let user_catalog = parse_theme_catalog_file(&raw).with_context(|| {
            format!(
                "failed to parse code-agent theme catalog {}",
                path.display()
            )
        })?;
        catalog_file = merge_theme_catalog_files(catalog_file, user_catalog);
    }
    let mut catalog = materialize_theme_catalog(catalog_file)?;
    if let Some(active_override) = active_override {
        ensure!(
            catalog.contains(active_override),
            "configured theme `{active_override}` is not present in the loaded theme catalog"
        );
        catalog.active_theme = active_override.to_string();
    }
    Ok(catalog)
}

fn parse_theme_catalog(raw: &str) -> Result<ThemeCatalog> {
    materialize_theme_catalog(parse_theme_catalog_file(raw)?)
}

fn parse_theme_catalog_file(raw: &str) -> Result<ThemeCatalogFile> {
    Ok(toml::from_str(raw)?)
}

fn materialize_theme_catalog(parsed: ThemeCatalogFile) -> Result<ThemeCatalog> {
    if parsed.themes.is_empty() {
        bail!("theme catalog must define at least one theme");
    }

    let mut themes = parsed
        .themes
        .into_iter()
        .map(|(id, theme)| {
            let palette = ThemePalette {
                bg: parse_hex_color(&theme.bg).with_context(|| format!("theme `{id}` bg"))?,
                main_bg: parse_hex_color(&theme.main_bg)
                    .with_context(|| format!("theme `{id}` main_bg"))?,
                footer_bg: parse_hex_color(&theme.footer_bg)
                    .with_context(|| format!("theme `{id}` footer_bg"))?,
                bottom_pane_bg: parse_hex_color(&theme.bottom_pane_bg)
                    .with_context(|| format!("theme `{id}` bottom_pane_bg"))?,
                border_active: parse_hex_color(&theme.border_active)
                    .with_context(|| format!("theme `{id}` border_active"))?,
                text: parse_hex_color(&theme.text).with_context(|| format!("theme `{id}` text"))?,
                muted: parse_hex_color(&theme.muted)
                    .with_context(|| format!("theme `{id}` muted"))?,
                subtle: parse_hex_color(&theme.subtle)
                    .with_context(|| format!("theme `{id}` subtle"))?,
                accent: parse_hex_color(&theme.accent)
                    .with_context(|| format!("theme `{id}` accent"))?,
                user: parse_hex_color(&theme.user).with_context(|| format!("theme `{id}` user"))?,
                assistant: parse_hex_color(&theme.assistant)
                    .with_context(|| format!("theme `{id}` assistant"))?,
                error: parse_hex_color(&theme.error)
                    .with_context(|| format!("theme `{id}` error"))?,
                warn: parse_hex_color(&theme.warn).with_context(|| format!("theme `{id}` warn"))?,
                header: parse_hex_color(&theme.header)
                    .with_context(|| format!("theme `{id}` header"))?,
            };
            Ok(ThemeDefinition {
                id,
                summary: theme.summary,
                palette,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    themes.sort_by(|left, right| left.id.cmp(&right.id));

    let active_theme = parsed.active.unwrap_or_else(|| themes[0].id.clone());
    ensure!(
        themes.iter().any(|theme| theme.id == active_theme),
        "theme catalog active theme `{active_theme}` does not exist"
    );

    Ok(ThemeCatalog {
        active_theme,
        themes,
    })
}

fn merge_theme_catalog_files(
    mut builtin: ThemeCatalogFile,
    user_catalog: ThemeCatalogFile,
) -> ThemeCatalogFile {
    // User-supplied files extend the builtin catalog and may deliberately
    // replace a builtin theme by reusing the same id.
    builtin.themes.extend(user_catalog.themes);
    if user_catalog.active.is_some() {
        builtin.active = user_catalog.active;
    }
    builtin
}

fn parse_hex_color(value: &str) -> Result<Color> {
    let raw = value.trim();
    let hex = raw
        .strip_prefix('#')
        .ok_or_else(|| anyhow::anyhow!("expected `#RRGGBB`, got `{raw}`"))?;
    ensure!(hex.len() == 6, "expected `#RRGGBB`, got `{raw}`");
    let red = u8::from_str_radix(&hex[0..2], 16)?;
    let green = u8::from_str_radix(&hex[2..4], 16)?;
    let blue = u8::from_str_radix(&hex[4..6], 16)?;
    Ok(Color::Rgb(red, green, blue))
}

fn resolve_path(workspace_root: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        workspace_root.join(path)
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeCatalogFile {
    #[serde(default)]
    active: Option<String>,
    themes: BTreeMap<String, ThemeFileEntry>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThemeFileEntry {
    summary: String,
    bg: String,
    main_bg: String,
    footer_bg: String,
    bottom_pane_bg: String,
    border_active: String,
    text: String,
    muted: String,
    subtle: String,
    accent: String,
    user: String,
    assistant: String,
    error: String,
    warn: String,
    header: String,
}

#[cfg(test)]
mod tests {
    use super::{ThemeCatalog, load_theme_catalog, parse_theme_catalog, set_active_theme};
    use tempfile::tempdir;

    #[test]
    fn parses_builtin_theme_catalog() {
        let catalog = parse_theme_catalog(super::BUILTIN_THEME_CATALOG).unwrap();
        assert!(catalog.themes.len() >= 8);
        assert!(catalog.contains("aurora"));
        assert!(catalog.contains("graphite"));
        assert!(catalog.contains("cinder"));
        assert!(catalog.contains("fjord"));
        assert!(catalog.contains("meadow"));
        assert!(catalog.contains("signal"));
        assert!(catalog.contains("paper"));
        assert!(catalog.contains("glacier"));
    }

    #[test]
    fn loads_theme_catalog_from_workspace_file() {
        let dir = tempdir().unwrap();
        let theme_path = dir.path().join("themes.toml");
        std::fs::write(
            &theme_path,
            r##"
active = "paper"

[themes.paper]
summary = "light paper"
bg = "#faf6ef"
main_bg = "#f5f0e7"
footer_bg = "#efe8de"
bottom_pane_bg = "#e7dfd2"
border_active = "#8b8175"
text = "#2b241d"
muted = "#6f665d"
subtle = "#9d9388"
accent = "#2f7c82"
user = "#9a6a2f"
assistant = "#3c7c56"
error = "#b4554f"
warn = "#b37a21"
header = "#17120d"
            "##,
        )
        .unwrap();

        let catalog = load_theme_catalog(dir.path(), Some("themes.toml"), None).unwrap();
        assert_eq!(catalog.active_theme, "paper");
        assert!(catalog.contains("paper"));
        assert!(catalog.contains("graphite"));
    }

    #[test]
    fn user_theme_catalog_extends_builtin_themes_when_active_is_omitted() {
        let dir = tempdir().unwrap();
        let theme_path = dir.path().join("themes.toml");
        std::fs::write(
            &theme_path,
            r##"
[themes.paper]
summary = "light paper"
bg = "#faf6ef"
main_bg = "#f5f0e7"
footer_bg = "#efe8de"
bottom_pane_bg = "#e7dfd2"
border_active = "#8b8175"
text = "#2b241d"
muted = "#6f665d"
subtle = "#9d9388"
accent = "#2f7c82"
user = "#9a6a2f"
assistant = "#3c7c56"
error = "#b4554f"
warn = "#b37a21"
header = "#17120d"
            "##,
        )
        .unwrap();

        let catalog = load_theme_catalog(dir.path(), Some("themes.toml"), None).unwrap();
        assert_eq!(catalog.active_theme, "graphite");
        assert!(catalog.contains("paper"));
        assert!(catalog.contains("graphite"));
    }

    #[test]
    fn rejects_unknown_active_theme_override() {
        let dir = tempdir().unwrap();
        let error = load_theme_catalog(dir.path(), None, Some("missing")).unwrap_err();
        assert!(error.to_string().contains("configured theme `missing`"));
    }

    #[test]
    fn active_theme_switch_requires_known_theme() {
        let catalog = ThemeCatalog {
            active_theme: "graphite".to_string(),
            themes: parse_theme_catalog(super::BUILTIN_THEME_CATALOG)
                .unwrap()
                .themes,
        };
        super::install_theme_catalog(catalog);
        let error = set_active_theme("missing").unwrap_err();
        assert!(error.to_string().contains("unknown theme `missing`"));
    }
}
