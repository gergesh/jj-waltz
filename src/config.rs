use anyhow::{Context, Result};
use serde::Deserialize;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const CONFIG_DIR: &str = "jj-waltz";
const CONFIG_FILE: &str = "config.toml";
const DEFAULT_BOOKMARK_TEMPLATE: &str = "{workspace}";
/// Sibling of the default workspace: `<parent>/<repo>.<workspace>`.
pub const DEFAULT_DIRECTORY_TEMPLATE: &str = "{parent}/{repo}.{workspace}";
const DEFAULT_TRUNK_REVSET: &str = "trunk()";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub default_command: DefaultCommand,
    pub workspace: WorkspaceConfig,
    pub trunk: TrunkConfig,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DefaultCommand {
    #[default]
    Ui,
    List,
    Help,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceConfig {
    pub create_bookmark: bool,
    /// None inherits the verified primary workspace; Some(false) explicitly opts out.
    pub colocate: Option<bool>,
    pub bookmark_template: String,
    pub copy_on_write: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrunkConfig {
    pub revset: String,
}

#[derive(Debug, Deserialize, Default)]
struct RawConfig {
    default_command: Option<DefaultCommand>,
    workspace: Option<RawWorkspaceConfig>,
    trunk: Option<RawTrunkConfig>,
}

#[derive(Debug, Deserialize, Default)]
struct RawWorkspaceConfig {
    #[serde(default)]
    create_bookmark: bool,
    colocate: Option<bool>,
    bookmark_template: Option<String>,
    #[serde(default)]
    copy_on_write: bool,
}

#[derive(Debug, Deserialize, Default)]
struct RawTrunkConfig {
    revset: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_command: DefaultCommand::Ui,
            workspace: WorkspaceConfig {
                create_bookmark: false,
                colocate: None,
                bookmark_template: DEFAULT_BOOKMARK_TEMPLATE.to_owned(),
                copy_on_write: false,
            },
            trunk: TrunkConfig {
                revset: DEFAULT_TRUNK_REVSET.to_owned(),
            },
        }
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        let Some(path) = config_path()? else {
            return Ok(Self::default());
        };
        if !path.exists() {
            return Ok(Self::default());
        }

        let contents = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let raw: RawConfig = toml::from_str(&contents)
            .with_context(|| format!("failed to parse {}", path.display()))?;

        Ok(raw.into())
    }
}

impl From<RawConfig> for Config {
    fn from(raw: RawConfig) -> Self {
        let defaults = Config::default();
        let workspace = raw.workspace.unwrap_or_default();
        let trunk = raw.trunk.unwrap_or_default();

        Self {
            default_command: raw.default_command.unwrap_or_default(),
            workspace: WorkspaceConfig {
                create_bookmark: workspace.create_bookmark,
                colocate: workspace.colocate,
                bookmark_template: workspace
                    .bookmark_template
                    .unwrap_or(defaults.workspace.bookmark_template),
                copy_on_write: workspace.copy_on_write,
            },
            trunk: TrunkConfig {
                revset: trunk.revset.unwrap_or(defaults.trunk.revset),
            },
        }
    }
}

pub fn bookmark_from_template(template: &str, workspace: &str) -> String {
    template.replace("{workspace}", workspace)
}

/// Expands a `--directory` template for one new workspace.
///
/// `{parent}` and `{repo}` are the parent directory and basename of the default
/// workspace, and `{workspace}` is the new workspace's name. A leading `~` is the home
/// directory, and a relative result is resolved against `{parent}`.
pub fn workspace_directory(
    template: &str,
    default_root: &Path,
    workspace: &str,
) -> Result<PathBuf> {
    let parent = default_root
        .parent()
        .context("workspace root has no parent directory")?;
    let repo = default_root
        .file_name()
        .context("workspace root has no valid basename")?;
    let expanded = template
        .replace("{parent}", &parent.to_string_lossy())
        .replace("{repo}", &repo.to_string_lossy())
        .replace("{workspace}", workspace);
    if expanded.contains(['{', '}']) {
        anyhow::bail!(
            "--directory {template:?} has an unknown token; \
             supported tokens are {{parent}}, {{repo}} and {{workspace}}"
        )
    }

    let path = if expanded == "~" || expanded.starts_with("~/") {
        let home =
            env::var_os("HOME").context("--directory starts with `~` but HOME is not set")?;
        PathBuf::from(home).join(expanded.trim_start_matches('~').trim_start_matches('/'))
    } else {
        PathBuf::from(&expanded)
    };
    let path = if path.is_absolute() {
        path
    } else {
        parent.join(path)
    };
    if path.parent().is_none() {
        anyhow::bail!("--directory {template:?} resolves to the filesystem root")
    }
    Ok(path)
}

fn config_path() -> Result<Option<PathBuf>> {
    let config_home = if let Some(value) = env::var_os("XDG_CONFIG_HOME") {
        PathBuf::from(value)
    } else if let Some(value) = env::var_os("HOME") {
        PathBuf::from(value).join(".config")
    } else {
        return Ok(None);
    };

    Ok(Some(config_home.join(CONFIG_DIR).join(CONFIG_FILE)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_do_not_create_bookmarks() {
        let config = Config::default();
        assert_eq!(config.default_command, DefaultCommand::Ui);
        assert!(!config.workspace.create_bookmark);
        assert_eq!(config.workspace.bookmark_template, "{workspace}");
        assert!(!config.workspace.copy_on_write);
        assert_eq!(config.trunk.revset, "trunk()");
    }

    #[test]
    fn bare_command_is_explicitly_configurable() {
        for (value, expected) in [
            ("ui", DefaultCommand::Ui),
            ("list", DefaultCommand::List),
            ("help", DefaultCommand::Help),
        ] {
            let raw: RawConfig = toml::from_str(&format!("default_command = {value:?}")).unwrap();
            assert_eq!(Config::from(raw).default_command, expected);
        }
        assert!(toml::from_str::<RawConfig>("default_command = 'remove'").is_err());
    }

    #[test]
    fn parses_copy_on_write_opt_in() {
        let raw: RawConfig = toml::from_str(
            r#"
                [workspace]
                copy_on_write = true
            "#,
        )
        .unwrap();

        let config = Config::from(raw);
        assert!(config.workspace.copy_on_write);
        assert!(!config.workspace.create_bookmark);
    }

    #[test]
    fn colocation_preserves_unset_and_explicit_false() {
        for (text, expected) in [
            ("", None),
            ("[workspace]", None),
            ("[workspace]\ncolocate = false", Some(false)),
            ("[workspace]\ncolocate = true", Some(true)),
        ] {
            let raw: RawConfig = toml::from_str(text).unwrap();
            assert_eq!(Config::from(raw).workspace.colocate, expected);
        }
    }

    #[test]
    fn parses_trunk_revset_without_workspace_section() {
        let raw: RawConfig = toml::from_str(
            r#"
                [trunk]
                revset = "main@origin"
            "#,
        )
        .unwrap();

        let config = Config::from(raw);
        assert_eq!(config.trunk.revset, "main@origin");
        assert!(!config.workspace.create_bookmark);
        assert_eq!(config.workspace.bookmark_template, "{workspace}");
    }

    #[test]
    fn empty_trunk_section_uses_default_revset() {
        let raw: RawConfig = toml::from_str("[trunk]").unwrap();

        assert_eq!(Config::from(raw).trunk.revset, "trunk()");
    }

    #[test]
    fn default_directory_is_a_sibling_of_the_default_workspace() {
        assert_eq!(
            workspace_directory(
                DEFAULT_DIRECTORY_TEMPLATE,
                Path::new("/work/app"),
                "feature-a"
            )
            .unwrap(),
            Path::new("/work/app.feature-a")
        );
    }

    #[test]
    fn directory_template_supports_tokens_relative_and_home_paths() {
        let root = Path::new("/work/app");
        let expand = |template| workspace_directory(template, root, "ws").unwrap();

        assert_eq!(
            expand("{parent}/{repo}-workspaces/{workspace}"),
            Path::new("/work/app-workspaces/ws")
        );
        assert_eq!(
            expand("{repo}-trees/{workspace}"),
            Path::new("/work/app-trees/ws")
        );
        assert_eq!(expand("/srv/{repo}/{workspace}"), Path::new("/srv/app/ws"));
        let home = PathBuf::from(env::var_os("HOME").unwrap());
        assert_eq!(
            expand("~/trees/{repo}/{workspace}"),
            home.join("trees/app/ws")
        );
    }

    #[test]
    fn directory_template_rejects_unknown_tokens() {
        let error = workspace_directory("{parent}/{nope}", Path::new("/work/app"), "ws")
            .unwrap_err()
            .to_string();
        assert!(error.contains("unknown token"), "{error}");
    }

    #[test]
    fn template_replaces_workspace_token() {
        assert_eq!(
            bookmark_from_template("wip/{workspace}", "feature-a"),
            "wip/feature-a"
        );
    }
}
