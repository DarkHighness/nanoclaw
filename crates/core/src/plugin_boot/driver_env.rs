#![cfg(feature = "memory-embed")]

use anyhow::{Result, anyhow};
use types::PluginId;

pub(super) fn materialize_api_key_envs(
    table: &mut toml::map::Map<String, toml::Value>,
    env_map: &agent_env::EnvMap,
    plugin_id: &PluginId,
) -> Result<()> {
    if let Some(api_key_env) = table
        .remove("api_key_env")
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
    {
        let api_key = env_map.get_non_empty(&api_key_env).ok_or_else(|| {
            anyhow!("missing API key env `{api_key_env}` for plugin `{plugin_id}` service config")
        })?;
        table.insert("api_key".to_string(), toml::Value::String(api_key));
    }

    for (_, value) in table.iter_mut() {
        if let toml::Value::Table(child) = value {
            materialize_api_key_envs(child, env_map, plugin_id)?;
        }
    }

    Ok(())
}
