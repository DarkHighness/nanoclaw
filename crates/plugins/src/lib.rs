//! TOML-native plugin manifest discovery and activation planning.
//!
//! This crate is intentionally control-plane only. It resolves plugin metadata
//! into deterministic activation inputs (skills, hooks, MCP, and runtime
//! activations) without directly mutating runtime behavior.

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
    use std::fs;
    use tempfile::tempdir;
    use toml::map::Map;
    use types::HookMutationPermission;

    #[test]
    fn discovery_loads_new_manifest_and_component_files() {
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

[runtime]
driver = "builtin.wasm-hook-runtime"
module = "wasm/plugin.wasm"
abi = "nanoclaw.plugin.v1"

[permissions]
exec = [".nanoclaw/plugins-cache/demo"]
host_api = ["get_hook_context", "emit_hook_effect"]
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
type = "wasm"
module = "wasm/prompt-filter.wasm"
entrypoint = "on_user_prompt"
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
        match &plugin.hooks[0].handler {
            types::HookHandler::Wasm(wasm) => {
                assert_eq!(
                    wasm.module,
                    plugin_root
                        .join("wasm/prompt-filter.wasm")
                        .to_string_lossy()
                        .to_string()
                );
            }
            other => panic!("unexpected hook handler: {other:?}"),
        }
        assert_eq!(plugin.mcp_servers.len(), 1);
        assert_eq!(
            plugin
                .manifest
                .runtime
                .as_ref()
                .and_then(|runtime| runtime.abi.as_deref()),
            Some("nanoclaw.plugin.v1")
        );
    }

    #[test]
    fn activation_plan_resolves_runtime_permissions_and_config() {
        let dir = tempdir().unwrap();
        let mut plugin = sample_plugin(
            dir.path(),
            "team-policy",
            PluginKind::Bundle,
            true,
            Some(PluginRuntimeSpec {
                driver: "builtin.wasm-hook-runtime".to_string(),
                module: Some("wasm/plugin.wasm".to_string()),
                abi: Some("nanoclaw.plugin.v1".to_string()),
            }),
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
        plugin.manifest.permissions = PluginPermissionRequest {
            read: vec!["docs".to_string()],
            write: vec![".nanoclaw/plugin-state/team-policy".to_string()],
            exec: vec![".nanoclaw/plugins-cache/team-policy".to_string()],
            network: PluginNetworkAccess::Deny,
            message_mutation: HookMutationPermission::Allow,
            host_api: vec![
                types::HookHostApiGrant::GetHookContext,
                types::HookHostApiGrant::EmitHookEffect,
            ],
            hook_events: vec![types::HookEvent::UserPromptSubmit],
        };
        let discovery = PluginDiscovery {
            plugins: vec![plugin],
            diagnostics: Vec::new(),
        };
        let mut resolver = PluginResolverConfig::default();
        resolver.entries.insert(
            "team-policy".to_string(),
            PluginEntryConfig {
                enabled: Some(true),
                permissions: PluginPermissionGrant {
                    read: vec!["docs".to_string()],
                    write: vec![".nanoclaw/plugin-state/team-policy".to_string()],
                    exec: vec![".nanoclaw/plugins-cache/team-policy".to_string()],
                    network: PluginNetworkAccess::Deny,
                    message_mutation: HookMutationPermission::Allow,
                    host_api: vec![
                        types::HookHostApiGrant::GetHookContext,
                        types::HookHostApiGrant::EmitHookEffect,
                    ],
                },
                config: Map::from_iter([
                    (
                        "index_path".to_string(),
                        toml::Value::String(".nanoclaw/memory.db".to_string()),
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

        let plan = build_activation_plan(discovery, &resolver, dir.path());
        assert_eq!(plan.runtime_activations.len(), 1);
        assert_eq!(
            plan.runtime_activations[0].runtime.driver,
            "builtin.wasm-hook-runtime"
        );
        assert_eq!(
            plan.runtime_activations[0]
                .config
                .get("index_path")
                .and_then(toml::Value::as_str),
            Some(".nanoclaw/memory.db")
        );
        let limit = plan.runtime_activations[0]
            .config
            .get("search")
            .and_then(toml::Value::as_table)
            .and_then(|table| table.get("limit"))
            .and_then(toml::Value::as_integer);
        assert_eq!(limit, Some(9));
        assert_eq!(
            plan.runtime_activations[0].granted_permissions.read_roots,
            vec![dir.path().join("docs")]
        );
        assert_eq!(
            plan.runtime_activations[0].granted_permissions.exec_roots,
            vec![dir.path().join(".nanoclaw/plugins-cache/team-policy")]
        );
    }

    #[test]
    fn activation_plan_rejects_permission_grant_overreach() {
        let dir = tempdir().unwrap();
        let mut plugin = sample_plugin(
            dir.path(),
            "team-policy",
            PluginKind::Bundle,
            true,
            Some(PluginRuntimeSpec {
                driver: "builtin.wasm-hook-runtime".to_string(),
                module: Some("wasm/plugin.wasm".to_string()),
                abi: None,
            }),
        );
        plugin.manifest.permissions = PluginPermissionRequest {
            exec: vec![".nanoclaw/plugins-cache/team-policy".to_string()],
            ..PluginPermissionRequest::default()
        };
        let discovery = PluginDiscovery {
            plugins: vec![plugin],
            diagnostics: Vec::new(),
        };
        let mut resolver = PluginResolverConfig::default();
        resolver.entries.insert(
            "team-policy".to_string(),
            PluginEntryConfig {
                enabled: Some(true),
                permissions: PluginPermissionGrant {
                    exec: vec![".nanoclaw/plugins-cache/other".to_string()],
                    ..PluginPermissionGrant::default()
                },
                config: Map::new(),
            },
        );

        let plan = build_activation_plan(discovery, &resolver, dir.path());
        assert!(plan.runtime_activations.is_empty());
        assert!(
            plan.diagnostics
                .iter()
                .any(|diag| diag.code == "plugin_permission_grant_exceeds_request")
        );
        assert!(
            plan.plugin_states
                .iter()
                .any(|state| state.plugin_id == "team-policy" && !state.enabled)
        );
    }

    #[test]
    fn message_mutation_manifest_values_parse() {
        let manifest: PluginManifest = toml::from_str(
            r#"
id = "mutator"

[permissions]
message_mutation = "allow"
"#,
        )
        .unwrap();
        assert_eq!(
            manifest.permissions.message_mutation,
            HookMutationPermission::Allow
        );
    }

    fn sample_plugin(
        workspace_root: &std::path::Path,
        id: &str,
        kind: PluginKind,
        enabled_by_default: bool,
        runtime: Option<PluginRuntimeSpec>,
    ) -> DiscoveredPlugin {
        DiscoveredPlugin {
            manifest: PluginManifest {
                id: id.to_string(),
                version: None,
                name: None,
                description: None,
                kind,
                enabled_by_default,
                components: PluginComponents::default(),
                runtime,
                capabilities: PluginCapabilitySet {
                    hook_handlers: vec![types::HookHandlerKind::Wasm],
                    message_mutations: vec![PluginMessageMutationCapability::Append],
                    tool_policies: vec![PluginToolPolicyCapability::RewriteArgs],
                    host_api: vec![
                        types::HookHostApiGrant::GetHookContext,
                        types::HookHostApiGrant::EmitHookEffect,
                    ],
                    mcp_exports: false,
                    skill_exports: false,
                },
                permissions: PluginPermissionRequest::default(),
                instructions: Vec::new(),
                defaults: Map::new(),
            },
            root_dir: workspace_root.join(id),
            manifest_path: workspace_root.join(format!("{id}/.nanoclaw-plugin/plugin.toml")),
            skill_roots: Vec::new(),
            hooks: Vec::new(),
            mcp_servers: Vec::new(),
        }
    }
}
