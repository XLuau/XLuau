use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use crate::compiler::{CompilerError, Result};

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct XluauConfig {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default = "default_include")]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default = "default_out_dir")]
    pub out_dir: PathBuf,
    #[serde(default = "default_target")]
    pub target: String,
    #[serde(default)]
    pub custom_target_function: Option<String>,
    #[serde(default = "default_luau_target")]
    pub luau_target: String,
    #[serde(default = "default_base_dir")]
    pub base_dir: PathBuf,
    #[serde(default)]
    pub paths: BTreeMap<String, String>,
    #[serde(default)]
    pub packages: BTreeMap<String, String>,
    #[serde(default = "default_extensions")]
    pub extensions: Vec<String>,
    #[serde(default = "default_index_files")]
    pub index_files: Vec<String>,
    #[serde(default = "default_true")]
    pub source_maps: bool,
    #[serde(default)]
    pub line_pragmas: bool,
    #[serde(default = "default_true")]
    pub strict: bool,
    #[serde(default = "default_true")]
    pub no_implicit_any: bool,
    #[serde(default = "default_true")]
    pub no_unchecked_optional: bool,
    #[serde(default = "default_task_adapter")]
    pub task_adapter: String,
    #[serde(default = "default_package_dir")]
    pub package_dir: PathBuf,
    #[serde(default = "default_bundle_file")]
    pub bundle_file: PathBuf,
    #[serde(default = "default_bundle_path")]
    pub bundle_path: String,
    #[serde(default = "default_registry")]
    pub registry: String,
    #[serde(default = "default_true")]
    pub minify: bool,
    #[serde(default = "default_true")]
    pub deduplicate_deps: bool,
    #[serde(default)]
    pub comptime_http: ComptimeHttpConfig,
    #[serde(default)]
    pub roblox_output: RobloxOutputConfig,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ComptimeHttpConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub allow: Vec<String>,
    #[serde(default = "default_comptime_http_timeout_ms")]
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RobloxOutputConfig {
    #[serde(default = "default_true")]
    pub emit_rbxmx: bool,
    #[serde(default)]
    pub suffixes: RobloxSuffixConfig,
    #[serde(default)]
    pub project_rbxmx: RobloxProjectRbxmxConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RobloxSuffixConfig {
    #[serde(default = "default_roblox_server_suffix")]
    pub server: String,
    #[serde(default = "default_roblox_legacy_suffix")]
    pub legacy: String,
    #[serde(default = "default_roblox_client_suffix")]
    pub client: String,
    #[serde(default = "default_roblox_local_suffix")]
    pub local: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RobloxProjectRbxmxConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub path: Option<PathBuf>,
    #[serde(default)]
    pub root_name: Option<String>,
    #[serde(default = "default_roblox_project_root_class")]
    pub root_class_name: String,
}

impl Default for XluauConfig {
    fn default() -> Self {
        Self {
            version: default_version(),
            include: default_include(),
            exclude: Vec::new(),
            out_dir: default_out_dir(),
            target: default_target(),
            custom_target_function: None,
            luau_target: default_luau_target(),
            base_dir: default_base_dir(),
            paths: BTreeMap::new(),
            packages: BTreeMap::new(),
            extensions: default_extensions(),
            index_files: default_index_files(),
            source_maps: default_true(),
            line_pragmas: false,
            strict: default_true(),
            no_implicit_any: default_true(),
            no_unchecked_optional: default_true(),
            task_adapter: default_task_adapter(),
            package_dir: default_package_dir(),
            bundle_file: default_bundle_file(),
            bundle_path: default_bundle_path(),
            registry: default_registry(),
            minify: default_true(),
            deduplicate_deps: default_true(),
            comptime_http: ComptimeHttpConfig::default(),
            roblox_output: RobloxOutputConfig::default(),
        }
    }
}

impl Default for RobloxOutputConfig {
    fn default() -> Self {
        Self {
            emit_rbxmx: default_true(),
            suffixes: RobloxSuffixConfig::default(),
            project_rbxmx: RobloxProjectRbxmxConfig::default(),
        }
    }
}

impl Default for RobloxSuffixConfig {
    fn default() -> Self {
        Self {
            server: default_roblox_server_suffix(),
            legacy: default_roblox_legacy_suffix(),
            client: default_roblox_client_suffix(),
            local: default_roblox_local_suffix(),
        }
    }
}

impl XluauConfig {
    pub fn load_from(root: &Path) -> Result<Self> {
        let config_path = root.join("xluau.config.json");
        if !config_path.exists() {
            return Ok(Self::default());
        }

        let contents = fs::read_to_string(&config_path).map_err(|source| CompilerError::Io {
            path: config_path.clone(),
            source,
        })?;

        serde_json::from_str(&contents).map_err(|source| CompilerError::Config {
            path: config_path,
            source,
        })
    }
}

fn default_version() -> u32 {
    1
}

fn default_include() -> Vec<String> {
    vec!["src/**/*.xl".to_string()]
}

fn default_out_dir() -> PathBuf {
    PathBuf::from("out")
}

fn default_target() -> String {
    "filesystem".to_string()
}

fn default_luau_target() -> String {
    "new-solver".to_string()
}

fn default_base_dir() -> PathBuf {
    PathBuf::from("src")
}

fn default_extensions() -> Vec<String> {
    vec![".xl".to_string(), ".luau".to_string(), ".lua".to_string()]
}

fn default_index_files() -> Vec<String> {
    vec!["init".to_string()]
}

fn default_true() -> bool {
    true
}

fn default_task_adapter() -> String {
    "coroutine".to_string()
}

fn default_package_dir() -> PathBuf {
    PathBuf::from("xluau_packages")
}

fn default_bundle_file() -> PathBuf {
    PathBuf::from("packages.luau")
}

fn default_bundle_path() -> String {
    "./packages.luau".to_string()
}

fn default_registry() -> String {
    "https://raw.githubusercontent.com/XLuau/XLpkg/master/index.json".to_string()
}

fn default_comptime_http_timeout_ms() -> u64 {
    5_000
}

fn default_roblox_server_suffix() -> String {
    ".server".to_string()
}

fn default_roblox_legacy_suffix() -> String {
    ".legacy".to_string()
}

fn default_roblox_client_suffix() -> String {
    ".client".to_string()
}

fn default_roblox_local_suffix() -> String {
    ".local".to_string()
}

fn default_roblox_project_root_class() -> String {
    "Folder".to_string()
}
