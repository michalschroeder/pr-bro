mod init;
mod schema;

pub use init::run_init_wizard;
pub use schema::{Config, QueryConfig};

use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

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

/// Load configuration from a YAML file
///
/// # Arguments
///
/// * `path` - Optional path to config file. If None, uses default path (~/.config/pr-bro/config.yaml)
///
/// # Errors
///
/// Returns an error if:
/// - The config file does not exist
/// - The config file cannot be read
/// - The YAML cannot be parsed
pub fn load_config(path: Option<PathBuf>) -> Result<Config> {
    let config_path = path.unwrap_or_else(get_config_path);

    if !config_path.exists() {
        anyhow::bail!(
            "Config file not found at {}. Create ~/.config/pr-bro/config.yaml",
            config_path.display()
        );
    }

    let config_content = fs::read_to_string(&config_path)
        .with_context(|| format!("Failed to read config file at {}", config_path.display()))?;

    let config: Config = serde_saphyr::from_str(&config_content)
        .map_err(|e| anyhow::anyhow!("Failed to parse config {}: {}", config_path.display(), e))?;

    Ok(config)
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

    /// Write `yaml` to a unique temp file and load it with the single-file loader.
    fn load_yaml(test_name: &str, yaml: &str) -> Result<Config> {
        let path = unique_config_path(test_name);
        fs::write(&path, yaml).unwrap();
        let result = load_config(Some(path.clone()));
        let _ = fs::remove_file(&path);
        result
    }

    // Characterization tests: pin the Debug output of today's single-file loader.

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
