//! The bridge's settings, from `lsp.agda-bridge.settings` in Zed's
//! `settings.json`. Zed sends them right after starting the bridge and again
//! whenever they change (`workspace/didChangeConfiguration`); the extension
//! also passes them as initialization options, so they hold from the start.

use serde_json::Value;

use crate::input;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// The Agda program.
    pub agda_path: String,
    /// Extra command line arguments for Agda.
    pub extra_args: Vec<String>,
    /// The output file, relative to the worktree root, if not the default.
    pub output_file: Option<String>,
    /// How symbols are typed.
    pub symbols: input::Options,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            agda_path: "agda".into(),
            extra_args: Vec::new(),
            output_file: None,
            symbols: input::Options::default(),
        }
    }
}

impl Settings {
    /// Read the settings from `value`. Missing ones keep their default, and
    /// so do ones of the wrong kind, which are also reported.
    pub fn read(value: &Value) -> (Settings, Vec<String>) {
        let mut settings = Settings::default();
        let (symbols, mut problems) = input::Options::read(value);
        settings.symbols = symbols;
        match &value["agdaPath"] {
            Value::Null => {}
            Value::String(path) if !path.trim().is_empty() => settings.agda_path = path.clone(),
            other => problems.push(format!("`agdaPath` must be a program, not {other}.")),
        }
        match &value["extraArgs"] {
            Value::Null => {}
            Value::Array(args) if args.iter().all(Value::is_string) => {
                settings.extra_args = args
                    .iter()
                    .filter_map(|arg| arg.as_str().map(String::from))
                    .collect();
            }
            other => problems.push(format!(
                "`extraArgs` must be a list of strings, not {other}."
            )),
        }
        match &value["outputFile"] {
            Value::Null => {}
            Value::String(file) if !file.trim().is_empty() => {
                settings.output_file = Some(file.clone())
            }
            other => problems.push(format!("`outputFile` must be a path, not {other}.")),
        }
        (settings, problems)
    }

    /// Whether Agda must start again for these settings.
    pub fn restarts_agda(&self, before: &Settings) -> bool {
        self.agda_path != before.agda_path || self.extra_args != before.extra_args
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_settings_and_reports_wrong_ones() {
        assert_eq!(Settings::read(&Value::Null), (Settings::default(), vec![]));
        let (settings, problems) = Settings::read(&json!({
            "agdaPath": "/opt/agda",
            "extraArgs": ["--safe"],
            "outputFile": "out.md",
            "symbolInput": "latex",
        }));
        assert!(problems.is_empty());
        assert_eq!(settings.agda_path, "/opt/agda");
        assert_eq!(settings.extra_args, ["--safe"]);
        assert_eq!(settings.output_file.as_deref(), Some("out.md"));
        assert!(settings.symbols.latex && !settings.symbols.typst);
        assert!(settings.restarts_agda(&Settings::default()));

        let (settings, problems) =
            Settings::read(&json!({ "agdaPath": 3, "extraArgs": "--safe", "outputFile": "" }));
        assert_eq!(settings, Settings::default());
        assert_eq!(problems.len(), 3);
        assert!(!settings.restarts_agda(&Settings::default()));
    }
}
