use zed_extension_api::{self as zed, Result, serde_json, settings::LspSettings};

/// The language server id in `extension.toml`, and the binary's name.
const BRIDGE: &str = "agda-bridge";

struct AgdaExtension;

impl zed::Extension for AgdaExtension {
    fn new() -> Self {
        Self
    }

    fn language_server_command(
        &mut self,
        _language_server_id: &zed::LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<zed::Command> {
        let binary = LspSettings::for_worktree(BRIDGE, worktree)?.binary;

        // A path in `lsp.agda-bridge.binary.path` wins over `PATH`.
        let command = binary
            .as_ref()
            .and_then(|binary| binary.path.clone())
            .or_else(|| worktree.which(BRIDGE))
            .ok_or_else(|| {
                "agda-bridge was not found. Build it with `cargo install --path bridge` \
                 from the extension's repository, or set `lsp.agda-bridge.binary.path`."
                    .to_string()
            })?;
        let args = binary
            .as_ref()
            .and_then(|binary| binary.arguments.clone())
            .unwrap_or_default();

        // The shell environment lets the bridge find `agda` the same way a
        // terminal would, for example in `~/.cabal/bin` or a Nix profile.
        let mut env = worktree.shell_env();
        if let Some(extra) = binary.and_then(|binary| binary.env) {
            env.extend(extra);
        }

        Ok(zed::Command { command, args, env })
    }

    fn language_server_initialization_options(
        &mut self,
        _language_server_id: &zed::LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<Option<serde_json::Value>> {
        // The settings at startup, so Agda starts with the right program.
        bridge_settings(worktree).map(Some)
    }

    fn language_server_workspace_configuration(
        &mut self,
        _language_server_id: &zed::LanguageServerId,
        worktree: &zed::Worktree,
    ) -> Result<Option<serde_json::Value>> {
        // Zed sends these after startup and whenever they change.
        bridge_settings(worktree).map(Some)
    }
}

/// The bridge's settings: `lsp.agda-bridge.settings` in Zed's settings, on top
/// of `lsp.agda-bridge.initialization_options`, where earlier versions read
/// them, so both places work.
fn bridge_settings(worktree: &zed::Worktree) -> Result<serde_json::Value> {
    let lsp = LspSettings::for_worktree(BRIDGE, worktree)?;
    let mut merged = serde_json::Map::new();
    for value in [lsp.initialization_options, lsp.settings].into_iter().flatten() {
        if let serde_json::Value::Object(object) = value {
            merged.extend(object);
        }
    }
    Ok(serde_json::Value::Object(merged))
}

zed::register_extension!(AgdaExtension);
