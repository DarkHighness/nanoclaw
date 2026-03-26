//! TOML-native plugin manifest discovery and activation planning.
//!
//! This crate is intentionally control-plane only. It resolves plugin metadata
//! into deterministic activation inputs (skills, hooks, MCP, and driver
//! requests) without directly mutating runtime behavior.

mod config;
mod discovery;
mod error;
mod manifest;
mod resolution;

pub use config::*;
pub use discovery::*;
pub use error::*;
pub use manifest::*;
pub use resolution::*;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::tempdir;
    use toml::map::Map;

    #[test]
    fn discovery_loads_plugin_manifest_and_component_files() {
        let dir = tempdir().unwrap();
        let plugin_root = dir.path().join("demo");
        fs::create_dir_all(plugin_root.join(".nanoclaw-plugin")).unwrap();
        fs::create_dir_all(plugin_root.join("skills").join("review")).unwrap();
        fs::write(
            plugin_root.join(".nanoclaw-plugin/plugin.toml"),
            r#"
id = "demo"
kind = "bundle"
enabled_by_default = true

[components]
skill_roots = ["skills"]
hook_files = [".nanoclaw-plugin/hooks.toml"]
mcp_files = [".nanoclaw-plugin/mcp.toml"]
"#,
        )
        .unwrap();
        fs::write(
            plugin_root.join(".nanoclaw-plugin/hooks.toml"),
            r#"
[[hooks]]
name = "review-reminder"
event = "UserPromptSubmit"

[hooks.handler]
type = "prompt"
prompt = "review first"
"#,
        )
        .unwrap();
        fs::write(
            plugin_root.join(".nanoclaw-plugin/mcp.toml"),
            r#"
[[mcp_servers]]
name = "docs"

[mcp_servers.transport]
transport = "stdio"
command = "uvx"
args = ["demo-mcp"]
cwd = "."
"#,
        )
        .unwrap();

        let discovered = discover_plugins(&[dir.path().to_path_buf()]).unwrap();
        assert_eq!(discovered.plugins.len(), 1);
        let plugin = &discovered.plugins[0];
        assert_eq!(plugin.manifest.id, "demo");
        assert_eq!(plugin.skill_roots, vec![plugin_root.join("skills")]);
        assert_eq!(plugin.hooks.len(), 1);
        assert_eq!(plugin.mcp_servers.len(), 1);
    }

    #[test]
    fn discovery_prefers_first_plugin_for_duplicate_id() {
        let dir = tempdir().unwrap();
        let first = dir.path().join("a");
        let second = dir.path().join("b");
        fs::create_dir_all(first.join(".nanoclaw-plugin")).unwrap();
        fs::create_dir_all(second.join(".nanoclaw-plugin")).unwrap();
        fs::write(first.join(".nanoclaw-plugin/plugin.toml"), r#"id = "dup""#).unwrap();
        fs::write(second.join(".nanoclaw-plugin/plugin.toml"), r#"id = "dup""#).unwrap();

        let discovered = discover_plugins(&[dir.path().to_path_buf()]).unwrap();
        assert_eq!(discovered.plugins.len(), 1);
        assert_eq!(discovered.plugins[0].root_dir, first);
        assert!(
            discovered
                .diagnostics
                .iter()
                .any(|diag| diag.code == "plugin_duplicate_id")
        );
    }

    #[test]
    fn activation_plan_resolves_enablement_and_driver_config() {
        let mut plugin = sample_plugin(
            "memory-core",
            PluginKind::Memory,
            true,
            Some("builtin.memory-core"),
        );
        plugin.manifest.defaults = Map::from_iter([(
            "search".to_string(),
            toml::Value::Table(Map::from_iter([
                ("limit".to_string(), toml::Value::Integer(6)),
                (
                    "mode".to_string(),
                    toml::Value::String("lexical".to_string()),
                ),
            ])),
        )]);
        let discovery = PluginDiscovery {
            plugins: vec![plugin],
            diagnostics: Vec::new(),
        };
        let mut resolver = PluginResolverConfig::default();
        resolver.entries.insert(
            "memory-core".to_string(),
            PluginEntryConfig {
                enabled: Some(true),
                config: Map::from_iter([
                    (
                        "index_path".to_string(),
                        toml::Value::String(".agent-core/memory.db".to_string()),
                    ),
                    (
                        "search".to_string(),
                        toml::Value::Table(Map::from_iter([(
                            "limit".to_string(),
                            toml::Value::Integer(9),
                        )])),
                    ),
                ]),
            },
        );
        resolver.slots.memory = Some("memory-core".to_string());

        let plan = build_activation_plan(discovery, &resolver);
        assert_eq!(plan.driver_activations.len(), 1);
        assert_eq!(plan.driver_activations[0].driver_id, "builtin.memory-core");
        assert_eq!(
            plan.driver_activations[0]
                .config
                .get("index_path")
                .and_then(toml::Value::as_str),
            Some(".agent-core/memory.db")
        );
        let limit = plan.driver_activations[0]
            .config
            .get("search")
            .and_then(toml::Value::as_table)
            .and_then(|table| table.get("limit"))
            .and_then(toml::Value::as_integer);
        let mode = plan.driver_activations[0]
            .config
            .get("search")
            .and_then(toml::Value::as_table)
            .and_then(|table| table.get("mode"))
            .and_then(toml::Value::as_str);
        assert_eq!(limit, Some(9));
        assert_eq!(mode, Some("lexical"));
        assert_eq!(plan.slots.memory.as_deref(), Some("memory-core"));
    }

    #[test]
    fn activation_plan_marks_memory_slot_kind_mismatch() {
        let discovery = PluginDiscovery {
            plugins: vec![sample_plugin("plain", PluginKind::Bundle, true, None)],
            diagnostics: Vec::new(),
        };
        let mut resolver = PluginResolverConfig::default();
        resolver.slots.memory = Some("plain".to_string());

        let plan = build_activation_plan(discovery, &resolver);
        assert!(
            plan.diagnostics
                .iter()
                .any(|diag| diag.code == "memory_slot_kind_mismatch")
        );
        assert_eq!(plan.slots.memory, None);
    }

    #[test]
    fn memory_slot_force_enables_selected_plugin() {
        let discovery = PluginDiscovery {
            plugins: vec![sample_plugin(
                "memory-embed",
                PluginKind::Memory,
                false,
                Some("builtin.memory-embed"),
            )],
            diagnostics: Vec::new(),
        };
        let mut resolver = PluginResolverConfig::default();
        resolver.slots.memory = Some("memory-embed".to_string());

        let plan = build_activation_plan(discovery, &resolver);
        assert_eq!(plan.slots.memory.as_deref(), Some("memory-embed"));
        assert_eq!(plan.driver_activations.len(), 1);
        assert!(
            plan.plugin_states
                .iter()
                .any(|state| state.plugin_id == "memory-embed" && state.enabled)
        );
    }

    #[test]
    fn activation_plan_respects_allow_and_deny_rules() {
        let discovery = PluginDiscovery {
            plugins: vec![
                sample_plugin("allowed", PluginKind::Bundle, true, None),
                sample_plugin("denied", PluginKind::Bundle, true, None),
            ],
            diagnostics: Vec::new(),
        };
        let mut resolver = PluginResolverConfig::default();
        resolver.allow = vec!["allowed".to_string(), "denied".to_string()];
        resolver.deny = vec!["denied".to_string()];

        let plan = build_activation_plan(discovery, &resolver);
        let states = plan
            .plugin_states
            .iter()
            .map(|state| (state.plugin_id.as_str(), state.enabled))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(states.get("allowed"), Some(&true));
        assert_eq!(states.get("denied"), Some(&false));
    }

    fn sample_plugin(
        id: &str,
        kind: PluginKind,
        enabled_by_default: bool,
        driver: Option<&str>,
    ) -> DiscoveredPlugin {
        DiscoveredPlugin {
            manifest: PluginManifest {
                id: id.to_string(),
                version: None,
                name: None,
                description: None,
                kind,
                enabled_by_default,
                driver: driver.map(ToOwned::to_owned),
                components: PluginComponents::default(),
                instructions: Vec::new(),
                defaults: Map::new(),
            },
            root_dir: PathBuf::from(format!("/tmp/{id}")),
            manifest_path: PathBuf::from(format!("/tmp/{id}/.nanoclaw-plugin/plugin.toml")),
            skill_roots: Vec::new(),
            hooks: Vec::new(),
            mcp_servers: Vec::new(),
        }
    }
}
