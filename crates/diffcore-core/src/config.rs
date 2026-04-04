//! Configuration file support for `.diffcore.toml`.
//!
//! Parses the optional project configuration file and provides a unified
//! `DiffcoreConfig` that merges defaults with file-provided overrides.
//!
//! See spec §6.2 for the full config file format.

use std::collections::HashMap;
use std::env;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::types::RankWeights;

/// Complete diffcore configuration, merging defaults with `.diffcore.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiffcoreConfig {
    /// Project metadata.
    #[serde(default)]
    pub project: ProjectConfig,
    /// Declared entrypoint glob patterns.
    #[serde(default)]
    pub entrypoints: EntrypointConfig,
    /// Named architectural layers mapped to glob patterns.
    #[serde(default)]
    pub layers: HashMap<String, String>,
    /// File ignore patterns.
    #[serde(default)]
    pub ignore: IgnoreConfig,
    /// LLM provider configuration.
    #[serde(default)]
    pub llm: LlmConfig,
    /// Ranking weight overrides.
    #[serde(default)]
    pub ranking: RankWeights,
    /// Diff behavior configuration.
    #[serde(default)]
    pub diff: DiffConfig,
}

impl Default for DiffcoreConfig {
    fn default() -> Self {
        Self {
            project: ProjectConfig::default(),
            entrypoints: EntrypointConfig::default(),
            layers: HashMap::new(),
            ignore: IgnoreConfig::default(),
            llm: LlmConfig::default(),
            ranking: RankWeights::default(),
            diff: DiffConfig::default(),
        }
    }
}

/// Project metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectConfig {
    /// Project name (used in output metadata).
    #[serde(default)]
    pub name: Option<String>,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        Self { name: None }
    }
}

/// Declared entrypoint glob patterns by type.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct EntrypointConfig {
    /// HTTP route handler globs.
    #[serde(default)]
    pub http: Vec<String>,
    /// Background worker / queue consumer globs.
    #[serde(default)]
    pub workers: Vec<String>,
    /// CLI command globs.
    #[serde(default)]
    pub cli: Vec<String>,
    /// Cron job globs.
    #[serde(default)]
    pub cron: Vec<String>,
    /// Event handler globs.
    #[serde(default)]
    pub events: Vec<String>,
}

/// File ignore configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct IgnoreConfig {
    /// Glob patterns for files to exclude from analysis.
    #[serde(default)]
    pub paths: Vec<String>,
}

/// Diff behavior configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiffConfig {
    /// When true, branch comparisons include uncommitted working tree changes.
    /// Equivalent to `git diff <base>` instead of `git diff <base>..HEAD`.
    #[serde(default = "default_include_uncommitted")]
    pub include_uncommitted: bool,
}

fn default_include_uncommitted() -> bool {
    true
}

impl Default for DiffConfig {
    fn default() -> Self {
        Self {
            include_uncommitted: true,
        }
    }
}

/// LLM provider configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LlmConfig {
    /// Provider name: "anthropic" or "openai".
    #[serde(default)]
    pub provider: Option<String>,
    /// Model identifier.
    #[serde(default)]
    pub model: Option<String>,
    /// Shell command to retrieve the API key (e.g. `op read op://vault/item/field`).
    #[serde(default)]
    pub key_cmd: Option<String>,
    /// API key stored directly in the config file.
    /// Precedence: key_cmd > key > env vars.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Optional LLM refinement pass configuration.
    #[serde(default)]
    pub refinement: RefinementConfig,
}

/// Configuration for the optional LLM refinement pass.
///
/// The refinement pass takes deterministic groups (v1) and asks an LLM to improve them:
/// split coincidental groupings, merge scattered refactors, re-rank by semantic review
/// order, reclassify misplaced files. Uses an evaluator-optimizer loop: refine → score →
/// refine again if score improved, up to `max_iterations`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RefinementConfig {
    /// Whether refinement is enabled (default: false).
    #[serde(default)]
    pub enabled: bool,
    /// Provider for refinement (can differ from annotation provider).
    #[serde(default)]
    pub provider: Option<String>,
    /// Model for refinement (user-selectable).
    #[serde(default)]
    pub model: Option<String>,
    /// Shell command to retrieve the refinement API key.
    #[serde(default)]
    pub key_cmd: Option<String>,
    /// Maximum evaluator-optimizer loop iterations (default: 1).
    /// 1 = single refinement pass, 2+ = iterative improvement.
    #[serde(default = "default_max_iterations")]
    pub max_iterations: u32,
}

fn default_max_iterations() -> u32 {
    1
}

impl Default for RefinementConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: None,
            model: None,
            key_cmd: None,
            max_iterations: 1,
        }
    }
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: None,
            model: None,
            key_cmd: None,
            key: None,
            refinement: RefinementConfig::default(),
        }
    }
}

/// Errors that can occur when loading configuration.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("Failed to read config file: {0}")]
    Io(#[from] std::io::Error),
    #[error("Failed to parse config file: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("Invalid config: {0}")]
    Validation(String),
}

impl DiffcoreConfig {
    /// Return the global diffcore config path in the user's home directory.
    ///
    /// Resolution order:
    /// 1. `DIFFCORE_CONFIG_HOME`
    /// 2. `HOME/.diffcore`
    pub fn global_config_path() -> Option<PathBuf> {
        diffcore_config_home().map(|dir| dir.join("config.toml"))
    }

    /// Load configuration from the user's global config file.
    ///
    /// Returns defaults when the file is missing or the home directory cannot be resolved.
    pub fn load_global() -> Result<Self, ConfigError> {
        if let Some(path) = Self::global_config_path() {
            if path.exists() {
                Self::from_file(&path)
            } else {
                Ok(Self::default())
            }
        } else {
            Ok(Self::default())
        }
    }

    /// Load configuration from a `.diffcore.toml` file at the given path.
    ///
    /// Returns `Ok(config)` with the parsed config, or `Err` on I/O or parse errors.
    pub fn from_file(path: &Path) -> Result<Self, ConfigError> {
        let contents = std::fs::read_to_string(path)?;
        Self::from_str(&contents)
    }

    /// Parse configuration from a TOML string.
    pub fn from_str(toml_str: &str) -> Result<Self, ConfigError> {
        let config: DiffcoreConfig = toml::from_str(toml_str)?;
        config.validate()?;
        Ok(config)
    }

    /// Look for `.diffcore.toml` in the given directory (typically the repo root).
    ///
    /// Returns `Ok(default)` if the file doesn't exist, `Err` if the file exists but is invalid.
    pub fn load_from_dir(dir: &Path) -> Result<Self, ConfigError> {
        let config_path = dir.join(".diffcore.toml");
        if config_path.exists() {
            Self::from_file(&config_path)
        } else {
            Ok(Self::default())
        }
    }

    /// Load repo-local config and merge in global LLM defaults from `~/.diffcore/config.toml`.
    ///
    /// Repo-local project settings remain authoritative; only the `[llm]` section falls back
    /// to the global config when values are not set locally.
    pub fn load_with_global_llm_from_dir(dir: &Path) -> Result<Self, ConfigError> {
        let mut local = Self::load_from_dir(dir)?;
        let global = Self::load_global()?;
        local.apply_global_llm_defaults(&global);
        Ok(local)
    }

    /// Save configuration to the global diffcore config in the user's home directory.
    pub fn save_global(&self) -> Result<(), ConfigError> {
        let Some(config_path) = Self::global_config_path() else {
            return Err(ConfigError::Validation(
                "Could not resolve a home directory for global config".to_string(),
            ));
        };

        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let toml_str = toml::to_string_pretty(self)
            .map_err(|e| ConfigError::Validation(format!("Failed to serialize config: {}", e)))?;
        std::fs::write(&config_path, toml_str)?;
        Ok(())
    }

    /// Validate the configuration for consistency.
    fn validate(&self) -> Result<(), ConfigError> {
        // Validate ranking weights are non-negative
        if self.ranking.risk < 0.0
            || self.ranking.centrality < 0.0
            || self.ranking.surface_area < 0.0
            || self.ranking.uncertainty < 0.0
        {
            return Err(ConfigError::Validation(
                "Ranking weights must be non-negative".to_string(),
            ));
        }

        // Validate LLM provider if specified
        if let Some(ref provider) = self.llm.provider {
            let valid = ["anthropic", "openai", "gemini", "codex", "claude"];
            if !valid.contains(&provider.as_str()) {
                return Err(ConfigError::Validation(format!(
                    "Unknown LLM provider '{}'. Valid providers: {}",
                    provider,
                    valid.join(", ")
                )));
            }
        }

        // Validate refinement provider if specified
        if let Some(ref provider) = self.llm.refinement.provider {
            let valid = ["anthropic", "openai", "gemini", "codex", "claude"];
            if !valid.contains(&provider.as_str()) {
                return Err(ConfigError::Validation(format!(
                    "Unknown refinement provider '{}'. Valid providers: {}",
                    provider,
                    valid.join(", ")
                )));
            }
        }

        // Validate max_iterations is at least 1
        if self.llm.refinement.max_iterations == 0 {
            return Err(ConfigError::Validation(
                "Refinement max_iterations must be at least 1".to_string(),
            ));
        }

        Ok(())
    }

    /// Resolve entrypoint glob patterns against a base directory.
    ///
    /// Returns a flat list of file paths that match any declared entrypoint pattern.
    pub fn resolve_entrypoint_globs(&self, base_dir: &Path) -> Vec<PathBuf> {
        let all_patterns: Vec<&str> = self
            .entrypoints
            .http
            .iter()
            .chain(self.entrypoints.workers.iter())
            .chain(self.entrypoints.cli.iter())
            .chain(self.entrypoints.cron.iter())
            .chain(self.entrypoints.events.iter())
            .map(|s| s.as_str())
            .collect();

        let mut result = Vec::new();
        for pattern in all_patterns {
            let full_pattern = base_dir.join(pattern).to_string_lossy().to_string();
            if let Ok(paths) = glob::glob(&full_pattern) {
                for entry in paths.flatten() {
                    result.push(entry);
                }
            }
        }
        result.sort();
        result.dedup();
        result
    }

    /// Save the configuration to `.diffcore.toml` in the given directory.
    ///
    /// If a `.diffcore.toml` already exists, it is overwritten.
    pub fn save_to_dir(&self, dir: &Path) -> Result<(), ConfigError> {
        let config_path = dir.join(".diffcore.toml");
        let toml_str = toml::to_string_pretty(self)
            .map_err(|e| ConfigError::Validation(format!("Failed to serialize config: {}", e)))?;
        std::fs::write(&config_path, toml_str)?;
        Ok(())
    }

    /// Check if a file path should be ignored based on configured ignore patterns.
    ///
    /// Patterns are matched against the relative path from the repo root.
    pub fn is_ignored(&self, relative_path: &str) -> bool {
        // Always exclude .diffcore/ directory (cache, comments, config)
        if relative_path.starts_with(".diffcore/") || relative_path == ".diffcore" {
            return true;
        }
        for pattern in &self.ignore.paths {
            if let Ok(glob_pattern) = glob::Pattern::new(pattern) {
                if glob_pattern.matches(relative_path) {
                    return true;
                }
            }
        }
        false
    }

    /// Get the layer name for a file path, if it matches any configured layer pattern.
    pub fn layer_for_path(&self, relative_path: &str) -> Option<String> {
        for (layer_name, pattern) in &self.layers {
            if let Ok(glob_pattern) = glob::Pattern::new(pattern) {
                if glob_pattern.matches(relative_path) {
                    return Some(layer_name.clone());
                }
            }
        }
        None
    }

    fn apply_global_llm_defaults(&mut self, global: &Self) {
        self.llm.provider = self
            .llm
            .provider
            .clone()
            .or_else(|| global.llm.provider.clone());
        self.llm.model = self.llm.model.clone().or_else(|| global.llm.model.clone());
        self.llm.key_cmd = self
            .llm
            .key_cmd
            .clone()
            .or_else(|| global.llm.key_cmd.clone());
        self.llm.key = self.llm.key.clone().or_else(|| global.llm.key.clone());

        self.llm.refinement.enabled = self.llm.refinement.enabled || global.llm.refinement.enabled;
        self.llm.refinement.provider = self
            .llm
            .refinement
            .provider
            .clone()
            .or_else(|| global.llm.refinement.provider.clone());
        self.llm.refinement.model = self
            .llm
            .refinement
            .model
            .clone()
            .or_else(|| global.llm.refinement.model.clone());
        self.llm.refinement.key_cmd = self
            .llm
            .refinement
            .key_cmd
            .clone()
            .or_else(|| global.llm.refinement.key_cmd.clone());
        if self.llm.refinement.max_iterations == default_max_iterations() {
            self.llm.refinement.max_iterations = global.llm.refinement.max_iterations;
        }
    }
}

fn diffcore_config_home() -> Option<PathBuf> {
    env::var_os("DIFFCORE_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".diffcore")))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stdout,
    clippy::print_stderr
)]
mod tests {
    use super::*;

    fn diffcore_config_home_for_testing(
        override_home: Option<&Path>,
        fallback_home: Option<&Path>,
    ) -> Option<PathBuf> {
        override_home
            .map(PathBuf::from)
            .or_else(|| fallback_home.map(|path| path.join(".diffcore")))
    }

    fn global_config_path_for_testing(
        override_home: Option<&Path>,
        fallback_home: Option<&Path>,
    ) -> Option<PathBuf> {
        diffcore_config_home_for_testing(override_home, fallback_home)
            .map(|dir| dir.join("config.toml"))
    }

    // ── Spec §12.2 Config Layer Tests ──

    #[test]
    fn test_parse_valid_config() {
        let toml_str = r#"
[project]
name = "my-app"

[entrypoints]
http = ["src/routes/**/*.ts"]
workers = ["src/jobs/**/*.ts"]
cli = ["src/cli/main.rs"]

[layers]
api = "src/handlers/**"
domain = "src/services/**"
persistence = "src/repositories/**"
ui = "src/components/**"

[ignore]
paths = ["**/*.test.ts", "**/*.spec.ts", "migrations/**"]

[llm]
provider = "anthropic"
model = "claude-sonnet-4-6"

[ranking]
risk = 0.4
centrality = 0.3
surface_area = 0.15
uncertainty = 0.15
"#;
        let config = DiffcoreConfig::from_str(toml_str).unwrap();

        assert_eq!(config.project.name, Some("my-app".to_string()));
        assert_eq!(config.entrypoints.http, vec!["src/routes/**/*.ts"]);
        assert_eq!(config.entrypoints.workers, vec!["src/jobs/**/*.ts"]);
        assert_eq!(config.entrypoints.cli, vec!["src/cli/main.rs"]);
        assert_eq!(config.layers.len(), 4);
        assert_eq!(config.layers.get("api").unwrap(), "src/handlers/**");
        assert_eq!(config.layers.get("domain").unwrap(), "src/services/**");
        assert_eq!(
            config.layers.get("persistence").unwrap(),
            "src/repositories/**"
        );
        assert_eq!(config.layers.get("ui").unwrap(), "src/components/**");
        assert_eq!(config.ignore.paths.len(), 3);
        assert_eq!(config.llm.provider, Some("anthropic".to_string()));
        assert_eq!(config.llm.model, Some("claude-sonnet-4-6".to_string()));
        assert!((config.ranking.risk - 0.4).abs() < f64::EPSILON);
        assert!((config.ranking.centrality - 0.3).abs() < f64::EPSILON);
        assert!((config.ranking.surface_area - 0.15).abs() < f64::EPSILON);
        assert!((config.ranking.uncertainty - 0.15).abs() < f64::EPSILON);
    }

    #[test]
    fn test_missing_config() {
        let dir = tempfile::tempdir().unwrap();
        let config = DiffcoreConfig::load_from_dir(dir.path()).unwrap();
        assert_eq!(config, DiffcoreConfig::default());
    }

    #[test]
    fn test_global_config_path_prefers_env_override() {
        let home = tempfile::tempdir().unwrap();
        let path = home.path().join("config.toml");
        assert_eq!(
            diffcore_config_home_for_testing(Some(home.path()), None),
            Some(home.path().to_path_buf())
        );
        assert_eq!(
            global_config_path_for_testing(Some(home.path()), None),
            Some(path)
        );
    }

    #[test]
    fn test_partial_config() {
        // Config with only some sections — rest should use defaults
        let toml_str = r#"
[ranking]
risk = 0.5
centrality = 0.5
surface_area = 0.0
uncertainty = 0.0
"#;
        let config = DiffcoreConfig::from_str(toml_str).unwrap();

        // Ranking overridden
        assert!((config.ranking.risk - 0.5).abs() < f64::EPSILON);
        assert!((config.ranking.centrality - 0.5).abs() < f64::EPSILON);
        assert!((config.ranking.surface_area - 0.0).abs() < f64::EPSILON);
        assert!((config.ranking.uncertainty - 0.0).abs() < f64::EPSILON);

        // Rest is default
        assert_eq!(config.project.name, None);
        assert!(config.entrypoints.http.is_empty());
        assert!(config.layers.is_empty());
        assert!(config.ignore.paths.is_empty());
        assert_eq!(config.llm.provider, None);
    }

    #[test]
    fn test_load_with_global_llm_from_dir_uses_global_llm_defaults() {
        let home = tempfile::tempdir().unwrap();
        let repo = tempfile::tempdir().unwrap();

        let global_path = global_config_path_for_testing(Some(home.path()), None).unwrap();
        std::fs::create_dir_all(global_path.parent().unwrap()).unwrap();
        std::fs::write(
            &global_path,
            r#"
[llm]
provider = "codex"
model = "default"

[llm.refinement]
enabled = true
provider = "claude"
model = "default"
"#,
        )
        .unwrap();

        std::fs::write(
            repo.path().join(".diffcore.toml"),
            r#"
[ignore]
paths = ["dist/**"]
"#,
        )
        .unwrap();

        let local = DiffcoreConfig::load_from_dir(repo.path()).unwrap();
        let global = DiffcoreConfig::from_file(&global_path).unwrap();
        let mut merged = local.clone();
        merged.apply_global_llm_defaults(&global);

        assert_eq!(merged.ignore.paths, vec!["dist/**"]);
        assert_eq!(merged.llm.provider, Some("codex".to_string()));
        assert_eq!(merged.llm.model, Some("default".to_string()));
        assert!(merged.llm.refinement.enabled);
        assert_eq!(merged.llm.refinement.provider, Some("claude".to_string()));
    }

    #[test]
    fn test_save_global_roundtrip_via_custom_home() {
        let home = tempfile::tempdir().unwrap();
        let path = global_config_path_for_testing(Some(home.path()), None).unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();

        let mut config = DiffcoreConfig::default();
        config.llm.provider = Some("claude".to_string());
        config.llm.model = Some("default".to_string());

        let toml_str = toml::to_string_pretty(&config).unwrap();
        std::fs::write(&path, toml_str).unwrap();

        let loaded = DiffcoreConfig::from_file(&path).unwrap();
        assert_eq!(loaded.llm.provider, Some("claude".to_string()));
        assert_eq!(loaded.llm.model, Some("default".to_string()));
    }

    #[test]
    fn test_invalid_config() {
        let toml_str = r#"
[ranking
risk = "not a number"
"#;
        let result = DiffcoreConfig::from_str(toml_str);
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            ConfigError::Parse(_) => {} // Expected
            _ => panic!("Expected parse error, got: {:?}", err),
        }
    }

    #[test]
    fn test_entrypoint_globs() {
        let dir = tempfile::tempdir().unwrap();

        // Create directory structure with matching files
        let routes_dir = dir.path().join("src").join("routes");
        std::fs::create_dir_all(&routes_dir).unwrap();
        std::fs::write(routes_dir.join("users.ts"), "export default {}").unwrap();
        std::fs::write(routes_dir.join("posts.ts"), "export default {}").unwrap();

        let jobs_dir = dir.path().join("src").join("jobs");
        std::fs::create_dir_all(&jobs_dir).unwrap();
        std::fs::write(jobs_dir.join("sync.ts"), "export default {}").unwrap();

        // Also create a non-matching file
        let utils_dir = dir.path().join("src").join("utils");
        std::fs::create_dir_all(&utils_dir).unwrap();
        std::fs::write(utils_dir.join("format.ts"), "export default {}").unwrap();

        let config = DiffcoreConfig::from_str(
            r#"
[entrypoints]
http = ["src/routes/*.ts"]
workers = ["src/jobs/*.ts"]
"#,
        )
        .unwrap();

        let resolved = config.resolve_entrypoint_globs(dir.path());
        assert_eq!(resolved.len(), 3);

        // All resolved paths should be under src/routes or src/jobs
        let route_count = resolved
            .iter()
            .filter(|p| p.to_string_lossy().contains("routes"))
            .count();
        let job_count = resolved
            .iter()
            .filter(|p| p.to_string_lossy().contains("jobs"))
            .count();
        assert_eq!(route_count, 2);
        assert_eq!(job_count, 1);

        // utils/format.ts should NOT be included
        assert!(!resolved
            .iter()
            .any(|p| p.to_string_lossy().contains("utils")));
    }

    #[test]
    fn test_ignore_patterns() {
        let config = DiffcoreConfig::from_str(
            r#"
[ignore]
paths = ["**/*.test.ts", "**/*.spec.ts", "migrations/**"]
"#,
        )
        .unwrap();

        // These should be ignored
        assert!(config.is_ignored("src/services/user.test.ts"));
        assert!(config.is_ignored("src/handlers/auth.spec.ts"));
        assert!(config.is_ignored("migrations/001_create_users.sql"));

        // These should NOT be ignored
        assert!(!config.is_ignored("src/services/user.ts"));
        assert!(!config.is_ignored("src/handlers/auth.ts"));
        assert!(!config.is_ignored("src/utils/format.ts"));
    }

    #[test]
    fn test_custom_layer_names() {
        let config = DiffcoreConfig::from_str(
            r#"
[layers]
api = "src/handlers/**"
domain = "src/services/**"
persistence = "src/repositories/**"
ui = "src/components/**"
"#,
        )
        .unwrap();

        assert_eq!(
            config.layer_for_path("src/handlers/users.ts"),
            Some("api".to_string())
        );
        assert_eq!(
            config.layer_for_path("src/services/auth.ts"),
            Some("domain".to_string())
        );
        assert_eq!(
            config.layer_for_path("src/repositories/user-repo.ts"),
            Some("persistence".to_string())
        );
        assert_eq!(
            config.layer_for_path("src/components/Button.tsx"),
            Some("ui".to_string())
        );
        // File not matching any layer
        assert_eq!(config.layer_for_path("src/utils/format.ts"), None);
    }

    // ── Additional unit tests ──

    #[test]
    fn test_default_ranking_weights() {
        let config = DiffcoreConfig::default();
        assert!((config.ranking.risk - 0.35).abs() < f64::EPSILON);
        assert!((config.ranking.centrality - 0.25).abs() < f64::EPSILON);
        assert!((config.ranking.surface_area - 0.20).abs() < f64::EPSILON);
        assert!((config.ranking.uncertainty - 0.20).abs() < f64::EPSILON);
    }

    #[test]
    fn test_invalid_llm_provider() {
        let toml_str = r#"
[llm]
provider = "invalid-provider"
"#;
        let result = DiffcoreConfig::from_str(toml_str);
        assert!(result.is_err());
        match result.unwrap_err() {
            ConfigError::Validation(msg) => {
                assert!(msg.contains("Unknown LLM provider"));
                assert!(msg.contains("invalid-provider"));
            }
            err => panic!("Expected validation error, got: {:?}", err),
        }
    }

    #[test]
    fn test_negative_ranking_weight_rejected() {
        let toml_str = r#"
[ranking]
risk = -0.5
centrality = 0.25
surface_area = 0.20
uncertainty = 0.20
"#;
        let result = DiffcoreConfig::from_str(toml_str);
        assert!(result.is_err());
        match result.unwrap_err() {
            ConfigError::Validation(msg) => {
                assert!(msg.contains("non-negative"));
            }
            err => panic!("Expected validation error, got: {:?}", err),
        }
    }

    #[test]
    fn test_load_from_dir_with_file() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".diffcore.toml");
        std::fs::write(
            &config_path,
            r#"
[project]
name = "test-project"

[ranking]
risk = 0.5
centrality = 0.5
surface_area = 0.0
uncertainty = 0.0
"#,
        )
        .unwrap();

        let config = DiffcoreConfig::load_from_dir(dir.path()).unwrap();
        assert_eq!(config.project.name, Some("test-project".to_string()));
        assert!((config.ranking.risk - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_empty_config_file() {
        // A completely empty file should produce defaults
        let config = DiffcoreConfig::from_str("").unwrap();
        assert_eq!(config, DiffcoreConfig::default());
    }

    #[test]
    fn test_key_cmd_preserved() {
        let toml_str = r#"
[llm]
provider = "anthropic"
key_cmd = "op read op://vault/diffcore/api-key"
"#;
        let config = DiffcoreConfig::from_str(toml_str).unwrap();
        assert_eq!(
            config.llm.key_cmd,
            Some("op read op://vault/diffcore/api-key".to_string())
        );
    }

    #[test]
    fn test_config_key_parsed() {
        let toml_str = r#"
[llm]
provider = "anthropic"
key = "sk-ant-test-key-12345"
"#;
        let config = DiffcoreConfig::from_str(toml_str).unwrap();
        assert_eq!(config.llm.key, Some("sk-ant-test-key-12345".to_string()));
    }

    #[test]
    fn test_config_key_not_serialized_when_none() {
        let config = DiffcoreConfig::default();
        let toml_str = toml::to_string_pretty(&config).unwrap();
        // key should not appear in output when None (skip_serializing_if)
        assert!(!toml_str.contains("key ="));
    }

    #[test]
    fn test_config_key_roundtrip_via_save() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = DiffcoreConfig::default();
        config.llm.provider = Some("anthropic".to_string());
        config.llm.key = Some("sk-roundtrip-test".to_string());
        config.save_to_dir(dir.path()).unwrap();

        let loaded = DiffcoreConfig::load_from_dir(dir.path()).unwrap();
        assert_eq!(loaded.llm.key, Some("sk-roundtrip-test".to_string()));
    }

    #[test]
    fn test_multiple_entrypoint_types() {
        let toml_str = r#"
[entrypoints]
http = ["src/routes/**/*.ts", "src/api/**/*.ts"]
workers = ["src/jobs/**/*.ts"]
cli = ["src/cli/main.rs"]
cron = ["src/cron/**/*.ts"]
events = ["src/handlers/events/**/*.ts"]
"#;
        let config = DiffcoreConfig::from_str(toml_str).unwrap();
        assert_eq!(config.entrypoints.http.len(), 2);
        assert_eq!(config.entrypoints.workers.len(), 1);
        assert_eq!(config.entrypoints.cli.len(), 1);
        assert_eq!(config.entrypoints.cron.len(), 1);
        assert_eq!(config.entrypoints.events.len(), 1);
    }

    #[test]
    fn test_serialization_roundtrip() {
        let original = DiffcoreConfig {
            project: ProjectConfig {
                name: Some("roundtrip-test".to_string()),
            },
            entrypoints: EntrypointConfig {
                http: vec!["src/routes/**/*.ts".to_string()],
                ..Default::default()
            },
            layers: {
                let mut m = HashMap::new();
                m.insert("api".to_string(), "src/handlers/**".to_string());
                m
            },
            ignore: IgnoreConfig {
                paths: vec!["**/*.test.ts".to_string()],
            },
            llm: LlmConfig {
                provider: Some("anthropic".to_string()),
                model: Some("claude-sonnet-4-6".to_string()),
                key_cmd: None,
                key: None,
                ..Default::default()
            },
            ranking: RankWeights {
                risk: 0.4,
                centrality: 0.3,
                surface_area: 0.15,
                uncertainty: 0.15,
            },
            diff: DiffConfig::default(),
        };

        // Serialize to JSON, deserialize back
        let json = serde_json::to_string(&original).unwrap();
        let deserialized: DiffcoreConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(original, deserialized);
    }

    #[test]
    fn test_ignore_empty_patterns() {
        let config = DiffcoreConfig::default();
        // With no ignore patterns, nothing should be ignored
        assert!(!config.is_ignored("any/file.ts"));
        assert!(!config.is_ignored("test/file.spec.ts"));
    }

    #[test]
    fn test_layer_for_path_no_layers() {
        let config = DiffcoreConfig::default();
        assert_eq!(config.layer_for_path("src/anything.ts"), None);
    }

    // ── Refinement Config Tests ──

    #[test]
    fn test_refinement_config_defaults() {
        let config = DiffcoreConfig::default();
        assert!(!config.llm.refinement.enabled);
        assert_eq!(config.llm.refinement.provider, None);
        assert_eq!(config.llm.refinement.model, None);
        assert_eq!(config.llm.refinement.key_cmd, None);
        assert_eq!(config.llm.refinement.max_iterations, 1);
    }

    #[test]
    fn test_parse_refinement_config() {
        let toml_str = r#"
[llm]
provider = "anthropic"

[llm.refinement]
enabled = true
provider = "openai"
model = "gpt-4.1"
max_iterations = 3
"#;
        let config = DiffcoreConfig::from_str(toml_str).unwrap();
        assert!(config.llm.refinement.enabled);
        assert_eq!(config.llm.refinement.provider, Some("openai".to_string()));
        assert_eq!(config.llm.refinement.model, Some("gpt-4.1".to_string()));
        assert_eq!(config.llm.refinement.max_iterations, 3);
    }

    #[test]
    fn test_refinement_disabled_by_default() {
        let toml_str = r#"
[llm]
provider = "anthropic"
"#;
        let config = DiffcoreConfig::from_str(toml_str).unwrap();
        assert!(!config.llm.refinement.enabled);
        assert_eq!(config.llm.refinement.max_iterations, 1);
    }

    #[test]
    fn test_refinement_invalid_provider_rejected() {
        let toml_str = r#"
[llm.refinement]
enabled = true
provider = "invalid"
"#;
        let result = DiffcoreConfig::from_str(toml_str);
        assert!(result.is_err());
        match result.unwrap_err() {
            ConfigError::Validation(msg) => {
                assert!(msg.contains("refinement provider"));
                assert!(msg.contains("invalid"));
            }
            err => panic!("Expected validation error, got: {:?}", err),
        }
    }

    #[test]
    fn test_refinement_zero_iterations_rejected() {
        let toml_str = r#"
[llm.refinement]
enabled = true
max_iterations = 0
"#;
        let result = DiffcoreConfig::from_str(toml_str);
        assert!(result.is_err());
        match result.unwrap_err() {
            ConfigError::Validation(msg) => {
                assert!(msg.contains("max_iterations"));
            }
            err => panic!("Expected validation error, got: {:?}", err),
        }
    }

    #[test]
    fn test_refinement_different_provider_from_annotation() {
        let toml_str = r#"
[llm]
provider = "anthropic"
model = "claude-sonnet-4-6"

[llm.refinement]
enabled = true
provider = "gemini"
model = "gemini-2.5-pro"
max_iterations = 2
"#;
        let config = DiffcoreConfig::from_str(toml_str).unwrap();
        assert_eq!(config.llm.provider, Some("anthropic".to_string()));
        assert_eq!(config.llm.refinement.provider, Some("gemini".to_string()));
    }

    #[test]
    fn test_refinement_key_cmd() {
        let toml_str = r#"
[llm.refinement]
enabled = true
provider = "anthropic"
key_cmd = "op read op://vault/diffcore/refinement-key"
"#;
        let config = DiffcoreConfig::from_str(toml_str).unwrap();
        assert_eq!(
            config.llm.refinement.key_cmd,
            Some("op read op://vault/diffcore/refinement-key".to_string())
        );
    }

    // ── Property-Based Tests ──

    mod prop_tests {
        use super::*;
        use proptest::prelude::*;

        proptest! {
            /// Parsing default config never panics
            #[test]
            fn prop_default_config_always_valid(_seed in 0u64..1000) {
                let config = DiffcoreConfig::default();
                config.validate().unwrap();
            }

            /// Any non-negative weights produce a valid config
            #[test]
            fn prop_non_negative_weights_valid(
                risk in 0.0f64..=10.0,
                centrality in 0.0f64..=10.0,
                surface_area in 0.0f64..=10.0,
                uncertainty in 0.0f64..=10.0,
            ) {
                let toml_str = format!(
                    "[ranking]\nrisk = {}\ncentrality = {}\nsurface_area = {}\nuncertainty = {}",
                    risk, centrality, surface_area, uncertainty
                );
                let result = DiffcoreConfig::from_str(&toml_str);
                prop_assert!(result.is_ok(), "Valid weights should parse: {:?}", result.err());
            }

            /// is_ignored never panics on arbitrary paths
            #[test]
            fn prop_is_ignored_no_panic(path in "[a-zA-Z0-9/_.-]{0,100}") {
                let config = DiffcoreConfig::from_str(
                    r#"
[ignore]
paths = ["**/*.test.ts", "**/*.spec.ts"]
"#
                ).unwrap();
                // Just assert it doesn't panic
                let _ = config.is_ignored(&path);
            }

            /// layer_for_path never panics on arbitrary paths
            #[test]
            fn prop_layer_for_path_no_panic(path in "[a-zA-Z0-9/_.-]{0,100}") {
                let config = DiffcoreConfig::from_str(
                    r#"
[layers]
api = "src/handlers/**"
domain = "src/services/**"
"#
                ).unwrap();
                let _ = config.layer_for_path(&path);
            }

            /// Empty TOML string always produces default config
            #[test]
            fn prop_empty_string_is_default(_seed in 0u64..100) {
                let config = DiffcoreConfig::from_str("").unwrap();
                prop_assert_eq!(config, DiffcoreConfig::default());
            }

            /// Serialization roundtrip preserves config (within f64 precision)
            #[test]
            fn prop_json_roundtrip(
                risk in 0.0f64..=10.0,
                centrality in 0.0f64..=10.0,
                surface_area in 0.0f64..=10.0,
                uncertainty in 0.0f64..=10.0,
            ) {
                let config = DiffcoreConfig {
                    ranking: RankWeights { risk, centrality, surface_area, uncertainty },
                    ..Default::default()
                };
                let json = serde_json::to_string(&config).unwrap();
                let roundtripped: DiffcoreConfig = serde_json::from_str(&json).unwrap();
                // Use approximate comparison for f64 fields due to JSON serialization precision
                prop_assert!((config.ranking.risk - roundtripped.ranking.risk).abs() < 1e-10);
                prop_assert!((config.ranking.centrality - roundtripped.ranking.centrality).abs() < 1e-10);
                prop_assert!((config.ranking.surface_area - roundtripped.ranking.surface_area).abs() < 1e-10);
                prop_assert!((config.ranking.uncertainty - roundtripped.ranking.uncertainty).abs() < 1e-10);
                prop_assert_eq!(config.project, roundtripped.project);
                prop_assert_eq!(config.entrypoints, roundtripped.entrypoints);
                prop_assert_eq!(config.layers, roundtripped.layers);
                prop_assert_eq!(config.ignore, roundtripped.ignore);
                prop_assert_eq!(config.llm, roundtripped.llm);
            }
        }
    }
}
