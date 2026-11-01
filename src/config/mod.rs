mod init;
mod merge;
mod schema;

pub use init::run_init_wizard;
pub use merge::{display_paths, LoadedConfig, QueryOrigin};
pub use schema::{Config, QueryConfig};

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// Get the config directory path (~/.config/pr-bro/)
pub fn get_config_dir() -> PathBuf {
    let home = dirs::home_dir().expect("Could not determine home directory");
    home.join(".config").join("pr-bro")
}

/// Get the default config file path (~/.config/pr-bro/config.yaml)
pub fn get_config_path() -> PathBuf {
    get_config_dir().join("config.yaml")
}

/// Ensure the config directory exists
pub fn ensure_config_dir() -> Result<()> {
    let config_dir = get_config_dir();
    if !config_dir.exists() {
        fs::create_dir_all(&config_dir).with_context(|| {
            format!(
                "Failed to create config directory at {}",
                config_dir.display()
            )
        })?;
    }
    Ok(())
}

/// Load and merge one or more config files, in order. Later files override earlier ones.
///
/// Each file is parsed on its own (errors name the file); built-in defaults are
/// applied once, after merging.
///
/// # Arguments
///
/// * `paths` - Config files to load, in command-line order
///
/// # Errors
///
/// Returns an error if:
/// - A config file does not exist or cannot be read
/// - The YAML cannot be parsed
/// - The merged config has no queries, or a query has no `query`
pub fn load_configs(paths: &[PathBuf]) -> Result<LoadedConfig> {
    let mut acc = merge::MergedLayers::default();
    for path in paths {
        acc = merge::merge(acc, read_layer(path)?, path);
    }
    merge::finish(acc, paths)
}

fn read_layer(path: &Path) -> Result<merge::ConfigLayer> {
    let content = match fs::read_to_string(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            anyhow::bail!("Config file not found at {}", path.display())
        }
        result => {
            result.with_context(|| format!("Failed to read config file at {}", path.display()))?
        }
    };

    serde_saphyr::from_str(&content)
        .map_err(|e| anyhow::anyhow!("Failed to parse config {}: {}", path.display(), e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scoring::ScoringConfig;

    fn unique_config_path(test_name: &str) -> PathBuf {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "pr-bro-test-config-{}-{}.yaml",
            test_name, timestamp
        ))
    }

    /// Write `yaml` to a unique temp file and load it on its own.
    fn load_yaml(test_name: &str, yaml: &str) -> Result<Config> {
        let path = unique_config_path(test_name);
        fs::write(&path, yaml).unwrap();
        let result = load_configs(std::slice::from_ref(&path)).map(|loaded| loaded.config);
        let _ = fs::remove_file(&path);
        result
    }

    #[test]
    fn test_load_configs_missing_file_among_several_names_it() {
        let first = unique_config_path("missing-first");
        let missing = unique_config_path("missing-second");
        fs::write(&first, "queries: [{query: q}]").unwrap();
        let err = load_configs(&[first.clone(), missing.clone()]).unwrap_err();
        let _ = fs::remove_file(&first);
        assert!(
            format!("{:#}", err).contains(&missing.display().to_string()),
            "{:#}",
            err
        );
    }

    #[test]
    fn test_load_configs_unknown_key_in_later_file_names_it() {
        let first = unique_config_path("unknown-first");
        let second = unique_config_path("unknown-second");
        fs::write(&first, "queries: [{query: q}]").unwrap();
        fs::write(&second, "queries: [{name: a, querry: q}]").unwrap();
        let err = format!(
            "{:#}",
            load_configs(&[first.clone(), second.clone()]).unwrap_err()
        );
        let _ = fs::remove_file(&first);
        let _ = fs::remove_file(&second);
        assert!(err.contains(&second.display().to_string()), "{}", err);
        assert!(err.contains("unknown field `querry`"), "{}", err);
    }

    #[test]
    fn test_load_configs_merges_files_in_order() {
        let team = unique_config_path("order-team");
        let local = unique_config_path("order-local");
        fs::write(&team, include_str!("testdata/team.yaml")).unwrap();
        fs::write(&local, include_str!("testdata/local.yaml")).unwrap();
        let loaded = load_configs(&[team.clone(), local.clone()]).unwrap();
        let _ = fs::remove_file(&team);
        let _ = fs::remove_file(&local);
        let names: Vec<_> = loaded
            .config
            .queries
            .iter()
            .map(|q| q.name.as_deref())
            .collect();
        assert_eq!(names, [Some("backend"), Some("frontend"), Some("infra")]);
        assert_eq!(loaded.origins[0].overridden_by, vec![local.clone()]);
        assert_eq!(loaded.origins[2].source, local);
    }

    // Characterization tests: pin the Debug output of loading a single file.

    #[test]
    fn test_characterize_docs_full_example() {
        let config = load_yaml(
            "docs-example",
            include_str!("testdata/docs_full_example.yaml"),
        )
        .unwrap();
        assert_eq!(
            format!("{:#?}", config),
            include_str!("testdata/docs_full_example.debug").trim_end()
        );
    }

    #[test]
    fn test_characterize_init_wizard_defaults() {
        // Same Config the init wizard writes when every prompt is accepted with its default
        let wizard_config = Config {
            scoring: Some(ScoringConfig::default()),
            queries: vec![QueryConfig {
                name: Some("Query 1".to_string()),
                query: "review-requested:@me review:required is:open".to_string(),
                scoring: None,
            }],
            auto_refresh_interval: 300,
            theme: "auto".to_string(),
        };
        let yaml = serde_saphyr::to_string(&wizard_config).unwrap();
        let config = load_yaml("init-defaults", &yaml).unwrap();
        assert_eq!(
            format!("{:#?}", config),
            include_str!("testdata/init_wizard_defaults.debug").trim_end()
        );
    }

    #[test]
    fn test_characterize_no_scoring_block() {
        let config = load_yaml("no-scoring", include_str!("testdata/no_scoring.yaml")).unwrap();
        assert_eq!(
            format!("{:#?}", config),
            include_str!("testdata/no_scoring.debug").trim_end()
        );
    }
}
