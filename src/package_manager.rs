use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::{
    compiler::{Compiler, CompilerError, Result},
    config::XluauConfig,
    module::ModuleResolver,
};

const XLUAU_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct XluauLock {
    #[serde(default = "default_lock_version")]
    pub version: u32,
    #[serde(default = "default_generated_timestamp")]
    pub generated: String,
    #[serde(default = "default_xluau_version")]
    pub xluau: String,
    #[serde(default)]
    pub packages: BTreeMap<String, LockedPackage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LockedPackage {
    #[serde(default)]
    pub registry_name: Option<String>,
    pub repo: String,
    pub version: String,
    pub sha: String,
    pub integrity: String,
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
    #[serde(default = "default_entry_file")]
    pub entry: String,
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryIndex {
    pub version: u32,
    #[serde(default)]
    pub packages: BTreeMap<String, RegistryPackage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryPackage {
    pub repo: String,
    #[serde(default)]
    pub description: String,
    pub latest: String,
    #[serde(default)]
    pub versions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageManifest {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub repo: String,
    #[serde(default = "default_entry_file")]
    pub entry: String,
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
    #[serde(default)]
    pub targets: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PackageManager {
    pub root: PathBuf,
    pub config: XluauConfig,
}

#[derive(Debug, Clone)]
pub struct PublishValidation {
    pub manifest: PackageManifest,
    pub exported_types: Vec<String>,
    pub public_fields: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct BundleOptions {
    pub minify: bool,
}

#[derive(Debug, Clone)]
pub struct InstalledPackageSummary {
    pub package_id: String,
    pub version: String,
    pub repo: String,
}

#[derive(Debug, Clone)]
enum PackageSource {
    Registry {
        name: String,
        requested_version: Option<String>,
    },
    GitHub {
        repo: String,
        requested_ref: Option<String>,
    },
    LocalPath {
        path: PathBuf,
        requested_ref: Option<String>,
    },
}

#[derive(Debug, Clone)]
struct ResolvedSource {
    package_id: String,
    registry_name: Option<String>,
    repo: String,
    checkout_ref: Option<String>,
    source_hint: Option<String>,
    local_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
struct InstalledPackage {
    package_id: String,
    registry_name: Option<String>,
    repo: String,
    version: String,
    sha: String,
    integrity: String,
    manifest: PackageManifest,
    root: PathBuf,
    source_hint: Option<String>,
}

#[derive(Debug, Clone)]
struct BundledModule {
    id: String,
    luau: String,
}

#[derive(Debug, Clone)]
struct PackageBundle {
    package_id: String,
    alias_names: Vec<String>,
    type_aliases: Vec<String>,
    surface_type: String,
    module_iife: String,
}

impl PackageManager {
    pub fn discover(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        let config = XluauConfig::load_from(&root)?;
        Ok(Self { root, config })
    }

    pub fn ensure_bundle(&self) -> Result<()> {
        if self.config.packages.is_empty() {
            return Ok(());
        }
        self.bundle(BundleOptions {
            minify: self.config.minify,
        })
        .map(|_| ())
    }

    pub fn bundle(&self, options: BundleOptions) -> Result<PathBuf> {
        if self.config.packages.is_empty() {
            return Ok(self.bundle_output_path());
        }

        let lock = self.load_lock()?;
        let required = self.required_package_ids()?;
        let installed = required
            .into_iter()
            .map(|package_id| self.load_installed_package(&lock, &package_id))
            .collect::<Result<Vec<_>>>()?;

        let mut packages = Vec::new();
        for installed_package in &installed {
            packages.push(self.build_package_bundle(
                installed_package,
                &installed,
                options.minify,
            )?);
        }

        let contents = render_packages_bundle(&packages);
        let path = self.bundle_output_path();
        fs::write(&path, contents).map_err(|source| CompilerError::Io {
            path: path.clone(),
            source,
        })?;
        Ok(path)
    }

    pub fn install_all(&self) -> Result<Vec<InstalledPackageSummary>> {
        let mut lock = self.load_lock()?;
        let registry = self.load_registry()?;
        let mut installed = Vec::new();
        let mut seen = HashSet::new();
        for source in self.config.packages.values() {
            let parsed = parse_package_source(source);
            self.install_source_recursive(
                &registry,
                &mut lock,
                &parsed,
                &mut installed,
                &mut seen,
            )?;
        }
        self.write_lock(&lock)?;
        self.bundle(BundleOptions {
            minify: self.config.minify,
        })?;
        Ok(installed)
    }

    pub fn install_requests(
        &mut self,
        requests: &[String],
    ) -> Result<Vec<InstalledPackageSummary>> {
        let registry = self.load_registry()?;
        let mut lock = self.load_lock()?;
        let mut installed = Vec::new();
        let mut seen = HashSet::new();

        for request in requests {
            let source = parse_package_source(request);
            let alias = default_alias_for_source(&source, &registry)?;
            let config_value = default_config_value_for_source(&source, &registry)?;
            self.config.packages.insert(alias, config_value);
            self.install_source_recursive(
                &registry,
                &mut lock,
                &source,
                &mut installed,
                &mut seen,
            )?;
        }

        self.write_config()?;
        self.write_lock(&lock)?;
        self.bundle(BundleOptions {
            minify: self.config.minify,
        })?;
        Ok(installed)
    }

    pub fn remove_aliases(&mut self, aliases: &[String]) -> Result<()> {
        let mut lock = self.load_lock()?;
        for alias in aliases {
            self.config.packages.remove(alias);
        }
        let keep = self.required_package_ids_from_config()?;
        lock.packages
            .retain(|package_id, _| keep.contains(package_id));
        self.write_config()?;
        self.write_lock(&lock)?;
        self.bundle(BundleOptions {
            minify: self.config.minify,
        })?;
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<InstalledPackageSummary>> {
        let lock = self.load_lock()?;
        let mut rows = lock
            .packages
            .into_iter()
            .map(|(package_id, package)| InstalledPackageSummary {
                package_id,
                version: package.version,
                repo: package.repo,
            })
            .collect::<Vec<_>>();
        rows.sort_by(|left, right| left.package_id.cmp(&right.package_id));
        Ok(rows)
    }

    pub fn update_requests(&mut self, requests: &[String]) -> Result<Vec<InstalledPackageSummary>> {
        if requests.is_empty() {
            return self.install_all();
        }

        let registry = self.load_registry()?;
        let mut lock = self.load_lock()?;
        let mut installed = Vec::new();
        let mut seen = HashSet::new();
        for request in requests {
            let source = parse_package_source(request);
            let alias = default_alias_for_source(&source, &registry)?;
            if !self.config.packages.contains_key(&alias) {
                self.config.packages.insert(
                    alias.clone(),
                    default_config_value_for_source(&source, &registry)?,
                );
            }
            self.install_source_recursive(
                &registry,
                &mut lock,
                &source,
                &mut installed,
                &mut seen,
            )?;
        }
        self.write_config()?;
        self.write_lock(&lock)?;
        self.bundle(BundleOptions {
            minify: self.config.minify,
        })?;
        Ok(installed)
    }

    pub fn validate_publish(&self) -> Result<PublishValidation> {
        let manifest_path = self.root.join("xlpkg.json");
        let manifest = self.read_manifest(&manifest_path)?;
        let entry_path = self.root.join(&manifest.entry);
        let source = fs::read_to_string(&entry_path).map_err(|source| CompilerError::Io {
            path: entry_path.clone(),
            source,
        })?;
        let exported_types = source
            .lines()
            .filter_map(|line| {
                let trimmed = line.trim_start();
                trimmed.strip_prefix("export type ").and_then(|rest| {
                    rest.split_once('=')
                        .map(|(head, _)| head.trim().to_string())
                })
            })
            .collect::<Vec<_>>();
        let public_fields = infer_public_field_names(&source);
        if public_fields.is_empty() {
            return Err(CompilerError::Other(
                "package entry point must return a public table".to_string(),
            ));
        }
        Ok(PublishValidation {
            manifest,
            exported_types,
            public_fields,
        })
    }

    pub fn publish_to_local_registry(&self) -> Result<()> {
        let validation = self.validate_publish()?;
        let registry_root = self.root.parent().unwrap_or(&self.root).join("XLpkg");
        let index_path = registry_root.join("index.json");
        if !index_path.is_file() {
            return Err(CompilerError::Other(format!(
                "local registry index not found at {}",
                index_path.display()
            )));
        }
        let contents = fs::read_to_string(&index_path).map_err(|source| CompilerError::Io {
            path: index_path.clone(),
            source,
        })?;
        let mut index: RegistryIndex = serde_json::from_str(&contents)
            .map_err(|source| CompilerError::Other(source.to_string()))?;
        let entry = index
            .packages
            .entry(validation.manifest.name.clone())
            .or_insert_with(|| RegistryPackage {
                repo: validation.manifest.repo.clone(),
                description: validation.manifest.description.clone(),
                latest: validation.manifest.version.clone(),
                versions: Vec::new(),
            });
        entry.repo = validation.manifest.repo.clone();
        entry.description = validation.manifest.description.clone();
        entry.latest = validation.manifest.version.clone();
        if !entry
            .versions
            .iter()
            .any(|version| version == &validation.manifest.version)
        {
            entry.versions.push(validation.manifest.version.clone());
            entry.versions.sort();
        }
        let updated = serde_json::to_string_pretty(&index)
            .map_err(|source| CompilerError::Other(source.to_string()))?;
        fs::write(&index_path, updated).map_err(|source| CompilerError::Io {
            path: index_path,
            source,
        })?;
        Ok(())
    }

    pub fn bundle_output_path(&self) -> PathBuf {
        self.root.join(&self.config.bundle_file)
    }

    fn required_package_ids(&self) -> Result<Vec<String>> {
        let mut ids = self
            .required_package_ids_from_config()?
            .into_iter()
            .collect::<Vec<_>>();
        ids.sort();
        Ok(ids)
    }

    fn required_package_ids_from_config(&self) -> Result<BTreeSet<String>> {
        let registry = self.load_registry()?;
        let mut ids = BTreeSet::new();
        for source in self.config.packages.values() {
            let resolved = resolve_source(&registry, &parse_package_source(source))?;
            ids.insert(resolved.package_id);
        }
        Ok(ids)
    }

    fn load_lock(&self) -> Result<XluauLock> {
        let path = self.root.join("xluau.lock");
        if !path.is_file() {
            return Ok(XluauLock::default());
        }
        let contents = fs::read_to_string(&path).map_err(|source| CompilerError::Io {
            path: path.clone(),
            source,
        })?;
        serde_json::from_str(&contents).map_err(|source| CompilerError::Other(source.to_string()))
    }

    fn write_lock(&self, lock: &XluauLock) -> Result<()> {
        let path = self.root.join("xluau.lock");
        let mut next = lock.clone();
        next.generated = current_timestamp();
        next.xluau = XLUAU_VERSION.to_string();
        let contents = serde_json::to_string_pretty(&next)
            .map_err(|source| CompilerError::Other(source.to_string()))?;
        fs::write(&path, contents).map_err(|source| CompilerError::Io { path, source })
    }

    fn load_registry(&self) -> Result<RegistryIndex> {
        read_registry(&self.root, &self.config.registry)
    }

    fn read_manifest(&self, path: &Path) -> Result<PackageManifest> {
        let contents = fs::read_to_string(path).map_err(|source| CompilerError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        serde_json::from_str(&contents).map_err(|source| CompilerError::Other(source.to_string()))
    }

    fn write_config(&self) -> Result<()> {
        let path = self.root.join("xluau.config.json");
        let contents = serde_json::to_string_pretty(&self.config)
            .map_err(|source| CompilerError::Other(source.to_string()))?;
        fs::write(&path, contents).map_err(|source| CompilerError::Io { path, source })
    }

    fn install_source_recursive(
        &self,
        registry: &RegistryIndex,
        lock: &mut XluauLock,
        source: &PackageSource,
        installed: &mut Vec<InstalledPackageSummary>,
        seen: &mut HashSet<String>,
    ) -> Result<()> {
        let resolved = resolve_source(registry, source)?;
        if !seen.insert(resolved.package_id.clone()) {
            return Ok(());
        }
        let installed_package = self.fetch_and_store_package(&resolved)?;
        for (dependency_name, dependency_version) in &installed_package.manifest.dependencies {
            let dependency_source = PackageSource::Registry {
                name: dependency_name.clone(),
                requested_version: Some(dependency_version.clone()),
            };
            self.install_source_recursive(registry, lock, &dependency_source, installed, seen)?;
        }
        lock.packages.insert(
            installed_package.package_id.clone(),
            LockedPackage {
                registry_name: installed_package.registry_name.clone(),
                repo: installed_package.repo.clone(),
                version: installed_package.version.clone(),
                sha: installed_package.sha.clone(),
                integrity: installed_package.integrity.clone(),
                dependencies: installed_package.manifest.dependencies.clone(),
                entry: installed_package.manifest.entry.clone(),
                source: installed_package.source_hint.clone(),
            },
        );
        installed.push(InstalledPackageSummary {
            package_id: installed_package.package_id,
            version: installed_package.version,
            repo: installed_package.repo,
        });
        Ok(())
    }

    fn fetch_and_store_package(&self, resolved: &ResolvedSource) -> Result<InstalledPackage> {
        let destination = self
            .root
            .join(&self.config.package_dir)
            .join(&resolved.package_id);
        if destination.exists() {
            fs::remove_dir_all(&destination).map_err(|source| CompilerError::Io {
                path: destination.clone(),
                source,
            })?;
        }
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|source| CompilerError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        if let Some(local_path) = &resolved.local_path {
            let source_path = if local_path.is_absolute() {
                local_path.clone()
            } else {
                self.root.join(local_path)
            };
            copy_directory(&source_path, &destination)?;
        } else {
            clone_package_repo(
                &resolved.repo,
                resolved.checkout_ref.as_deref(),
                &destination,
            )?;
        }

        let manifest_path = destination.join("xlpkg.json");
        let manifest = self.read_manifest(&manifest_path)?;
        let sha = if resolved.local_path.is_some() {
            hash_directory(&destination)?
        } else {
            git_head_sha(&destination)?
        };
        let integrity = format!("sha256:{}", hash_directory(&destination)?);
        Ok(InstalledPackage {
            package_id: resolved.package_id.clone(),
            registry_name: resolved.registry_name.clone(),
            repo: resolved.repo.clone(),
            version: manifest.version.clone(),
            sha,
            integrity,
            manifest,
            root: destination,
            source_hint: resolved.source_hint.clone(),
        })
    }

    fn load_installed_package(
        &self,
        lock: &XluauLock,
        package_id: &str,
    ) -> Result<InstalledPackage> {
        let locked = lock.packages.get(package_id).ok_or_else(|| {
            CompilerError::Other(format!(
                "package `{package_id}` is required by config but missing from xluau.lock"
            ))
        })?;
        let root = self.root.join(&self.config.package_dir).join(package_id);
        let manifest = self.read_manifest(&root.join("xlpkg.json"))?;
        Ok(InstalledPackage {
            package_id: package_id.to_string(),
            registry_name: locked.registry_name.clone(),
            repo: locked.repo.clone(),
            version: locked.version.clone(),
            sha: locked.sha.clone(),
            integrity: locked.integrity.clone(),
            manifest,
            root,
            source_hint: locked.source.clone(),
        })
    }

    fn build_package_bundle(
        &self,
        package: &InstalledPackage,
        installed: &[InstalledPackage],
        minify: bool,
    ) -> Result<PackageBundle> {
        let manifest = &package.manifest;
        let entry_path = package.root.join(&manifest.entry);
        let modules = collect_package_modules(package, &entry_path)?;
        let compiler = package_compiler(package, &manifest.dependencies)?;
        let resolver = ModuleResolver::new(package.root.clone(), compiler.config.clone());

        let mut bundled_modules = Vec::new();
        for module_path in &modules {
            let source = fs::read_to_string(module_path).map_err(|source| CompilerError::Io {
                path: module_path.clone(),
                source,
            })?;
            let artifact = compiler.build_file(module_path)?;
            let rewritten = rewrite_bundle_requires(
                &artifact.luau,
                module_path,
                &resolver,
                &self.config.bundle_path,
                &manifest.dependencies,
            )?;
            let logical = logical_module_id(&package.root, module_path);
            bundled_modules.push(BundledModule {
                id: logical,
                luau: if minify {
                    minify_luau(&rewritten)
                } else {
                    rewritten
                },
            });
            let _ = source;
        }

        let entry_source = fs::read_to_string(&entry_path).map_err(|source| CompilerError::Io {
            path: entry_path.clone(),
            source,
        })?;
        let (type_aliases, surface_type) = infer_package_types(&package.package_id, &entry_source);
        let alias_names = self
            .config
            .packages
            .iter()
            .filter_map(|(alias, source)| {
                resolve_source(&self.load_registry().ok()?, &parse_package_source(source))
                    .ok()
                    .filter(|resolved| resolved.package_id == package.package_id)
                    .map(|_| alias.clone())
            })
            .collect::<Vec<_>>();
        let dependency_ids = manifest
            .dependencies
            .keys()
            .filter_map(|name| {
                installed
                    .iter()
                    .find(|candidate| {
                        candidate.registry_name.as_deref() == Some(name.as_str())
                            || candidate.manifest.name == *name
                            || candidate.package_id == *name
                    })
                    .map(|candidate| (name.clone(), candidate.package_id.clone()))
            })
            .collect::<BTreeMap<_, _>>();
        let module_iife = render_package_iife(
            &package.package_id,
            &bundled_modules,
            &manifest.entry,
            &dependency_ids,
        );

        Ok(PackageBundle {
            package_id: package.package_id.clone(),
            alias_names,
            type_aliases,
            surface_type,
            module_iife,
        })
    }
}

pub fn ensure_bundle_for_project(root: &Path, config: &XluauConfig) -> Result<()> {
    if config.packages.is_empty() || config.bundle_path == "__XLUAU_PACKAGES__" {
        return Ok(());
    }
    PackageManager {
        root: root.to_path_buf(),
        config: config.clone(),
    }
    .ensure_bundle()
}

fn default_lock_version() -> u32 {
    1
}

fn default_generated_timestamp() -> String {
    current_timestamp()
}

fn default_xluau_version() -> String {
    XLUAU_VERSION.to_string()
}

fn default_entry_file() -> String {
    "init.xl".to_string()
}

fn current_timestamp() -> String {
    OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

fn read_registry(root: &Path, location: &str) -> Result<RegistryIndex> {
    let contents = if location.starts_with("http://") || location.starts_with("https://") {
        reqwest::blocking::get(location)
            .map_err(|source| CompilerError::Other(source.to_string()))?
            .error_for_status()
            .map_err(|source| CompilerError::Other(source.to_string()))?
            .text()
            .map_err(|source| CompilerError::Other(source.to_string()))?
    } else {
        let path = normalize_registry_path(root, location);
        fs::read_to_string(&path).map_err(|source| CompilerError::Io { path, source })?
    };
    serde_json::from_str(&contents).map_err(|source| CompilerError::Other(source.to_string()))
}

fn normalize_registry_path(root: &Path, location: &str) -> PathBuf {
    let path = if let Some(path) = location.strip_prefix("file://") {
        PathBuf::from(path)
    } else {
        PathBuf::from(location)
    };
    if path.is_absolute() {
        path
    } else {
        root.join(path)
    }
}

fn parse_package_source(input: &str) -> PackageSource {
    if let Some(repo) = input.strip_prefix("gh:") {
        let (repo, requested_ref) = split_source_ref(repo);
        return PackageSource::GitHub {
            repo: repo.to_string(),
            requested_ref,
        };
    }
    if let Some(path) = input.strip_prefix("file:") {
        let (path, requested_ref) = split_source_ref(path);
        return PackageSource::LocalPath {
            path: PathBuf::from(path),
            requested_ref,
        };
    }
    let (name, requested_version) = split_source_ref(input);
    PackageSource::Registry {
        name: name.to_string(),
        requested_version,
    }
}

fn split_source_ref(input: &str) -> (&str, Option<String>) {
    input
        .rsplit_once('@')
        .map(|(head, tail)| (head, Some(tail.to_string())))
        .unwrap_or((input, None))
}

fn default_alias_for_source(source: &PackageSource, registry: &RegistryIndex) -> Result<String> {
    match source {
        PackageSource::Registry { name, .. } => Ok(name.clone()),
        PackageSource::GitHub { repo, .. } => Ok(repo_tail(repo)),
        PackageSource::LocalPath { path, .. } => path
            .file_name()
            .and_then(|value| value.to_str())
            .map(|value| value.to_string())
            .ok_or_else(|| {
                CompilerError::Other(format!("invalid local package path {}", path.display()))
            }),
    }
    .and_then(|alias| {
        if registry.packages.contains_key(&alias) || !alias.is_empty() {
            Ok(alias)
        } else {
            Err(CompilerError::Other(
                "package alias cannot be empty".to_string(),
            ))
        }
    })
}

fn default_config_value_for_source(
    source: &PackageSource,
    registry: &RegistryIndex,
) -> Result<String> {
    match source {
        PackageSource::Registry { .. } => {
            resolve_source(registry, source).map(|resolved| resolved.package_id)
        }
        PackageSource::GitHub {
            repo,
            requested_ref,
        } => Ok(match requested_ref {
            Some(requested_ref) => format!("gh:{repo}@{requested_ref}"),
            None => format!("gh:{repo}"),
        }),
        PackageSource::LocalPath {
            path,
            requested_ref,
        } => Ok(match requested_ref {
            Some(requested_ref) => format!("file:{}@{requested_ref}", path.display()),
            None => format!("file:{}", path.display()),
        }),
    }
}

fn resolve_source(registry: &RegistryIndex, source: &PackageSource) -> Result<ResolvedSource> {
    match source {
        PackageSource::Registry {
            name,
            requested_version,
        } => {
            if let Some(entry) = registry.packages.get(name) {
                let version = requested_version
                    .clone()
                    .unwrap_or_else(|| entry.latest.clone());
                return Ok(ResolvedSource {
                    package_id: repo_tail(&entry.repo),
                    registry_name: Some(name.clone()),
                    repo: entry.repo.clone(),
                    checkout_ref: Some(tag_name(&version)),
                    source_hint: Some(name.clone()),
                    local_path: None,
                });
            }
            if let Some((registry_name, entry)) = registry
                .packages
                .iter()
                .find(|(_, entry)| repo_tail(&entry.repo) == *name)
            {
                let version = requested_version
                    .clone()
                    .unwrap_or_else(|| entry.latest.clone());
                return Ok(ResolvedSource {
                    package_id: repo_tail(&entry.repo),
                    registry_name: Some(registry_name.clone()),
                    repo: entry.repo.clone(),
                    checkout_ref: Some(tag_name(&version)),
                    source_hint: Some(name.clone()),
                    local_path: None,
                });
            }
            Err(CompilerError::Other(format!(
                "unknown package `{name}` in registry"
            )))
        }
        PackageSource::GitHub {
            repo,
            requested_ref,
        } => Ok(ResolvedSource {
            package_id: repo_tail(repo),
            registry_name: None,
            repo: repo.clone(),
            checkout_ref: requested_ref.clone(),
            source_hint: Some(match requested_ref {
                Some(requested_ref) => format!("gh:{repo}@{requested_ref}"),
                None => format!("gh:{repo}"),
            }),
            local_path: None,
        }),
        PackageSource::LocalPath {
            path,
            requested_ref,
        } => Ok(ResolvedSource {
            package_id: path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("local-package")
                .to_string(),
            registry_name: None,
            repo: path.display().to_string(),
            checkout_ref: requested_ref.clone(),
            source_hint: Some(match requested_ref {
                Some(requested_ref) => format!("file:{}@{requested_ref}", path.display()),
                None => format!("file:{}", path.display()),
            }),
            local_path: Some(path.clone()),
        }),
    }
}

fn repo_tail(repo: &str) -> String {
    repo.rsplit('/')
        .next()
        .unwrap_or(repo)
        .trim_end_matches(".git")
        .to_string()
}

fn tag_name(version: &str) -> String {
    if version.starts_with('v') {
        version.to_string()
    } else {
        format!("v{version}")
    }
}

fn clone_package_repo(repo: &str, checkout_ref: Option<&str>, destination: &Path) -> Result<()> {
    let remote = if repo.contains("://") {
        repo.to_string()
    } else {
        format!("https://github.com/{repo}.git")
    };
    run_git(&[
        "clone",
        "--depth",
        "1",
        &remote,
        destination.to_string_lossy().as_ref(),
    ])?;
    if let Some(checkout_ref) = checkout_ref {
        run_git_in(
            destination,
            &["fetch", "--tags", "--depth", "1", "origin", checkout_ref],
        )?;
        run_git_in(destination, &["checkout", checkout_ref])?;
    }
    let git_dir = destination.join(".git");
    if git_dir.is_dir() {
        fs::remove_dir_all(&git_dir).map_err(|source| CompilerError::Io {
            path: git_dir,
            source,
        })?;
    }
    Ok(())
}

fn run_git(args: &[&str]) -> Result<()> {
    let status = Command::new("git")
        .args(args)
        .status()
        .map_err(|source| CompilerError::Other(source.to_string()))?;
    if status.success() {
        Ok(())
    } else {
        Err(CompilerError::Other(format!(
            "git {} failed with status {status}",
            args.join(" ")
        )))
    }
}

fn run_git_in(dir: &Path, args: &[&str]) -> Result<()> {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .map_err(|source| CompilerError::Other(source.to_string()))?;
    if status.success() {
        Ok(())
    } else {
        Err(CompilerError::Other(format!(
            "git {} failed with status {status}",
            args.join(" ")
        )))
    }
}

fn git_head_sha(dir: &Path) -> Result<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir)
        .output()
        .map_err(|source| CompilerError::Other(source.to_string()))?;
    if !output.status.success() {
        return Err(CompilerError::Other(
            "git rev-parse HEAD failed".to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn copy_directory(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination).map_err(|source_error| CompilerError::Io {
        path: destination.to_path_buf(),
        source: source_error,
    })?;
    for entry in fs::read_dir(source).map_err(|source_error| CompilerError::Io {
        path: source.to_path_buf(),
        source: source_error,
    })? {
        let entry = entry.map_err(|source_error| CompilerError::Other(source_error.to_string()))?;
        let entry_path = entry.path();
        let target = destination.join(entry.file_name());
        if entry_path.is_dir() {
            copy_directory(&entry_path, &target)?;
        } else {
            fs::copy(&entry_path, &target).map_err(|source_error| CompilerError::Io {
                path: target,
                source: source_error,
            })?;
        }
    }
    Ok(())
}

fn hash_directory(root: &Path) -> Result<String> {
    let mut files = Vec::new();
    collect_files(root, &mut files)?;
    files.sort();
    let mut hasher = Sha256::new();
    for file in files {
        let relative = file.strip_prefix(root).unwrap_or(&file);
        hasher.update(relative.to_string_lossy().as_bytes());
        let bytes = fs::read(&file).map_err(|source| CompilerError::Io {
            path: file.clone(),
            source,
        })?;
        hasher.update(&bytes);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn collect_files(root: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(root).map_err(|source| CompilerError::Io {
        path: root.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| CompilerError::Other(source.to_string()))?;
        let path = entry.path();
        if path.file_name().and_then(|name| name.to_str()) == Some(".git") {
            continue;
        }
        if path.is_dir() {
            collect_files(&path, files)?;
        } else {
            files.push(path);
        }
    }
    Ok(())
}

fn package_compiler(
    package: &InstalledPackage,
    dependencies: &BTreeMap<String, String>,
) -> Result<Compiler> {
    let mut config = XluauConfig::default();
    config.include = vec!["**/*.xl".to_string()];
    config.base_dir = PathBuf::new();
    config.out_dir = PathBuf::from(".xluau_package_build");
    config.packages = dependencies
        .keys()
        .map(|dependency| (dependency.clone(), dependency.clone()))
        .collect();
    config.bundle_path = "__XLUAU_PACKAGES__".to_string();
    Ok(Compiler {
        root: package.root.clone(),
        config,
    })
}

fn collect_package_modules(package: &InstalledPackage, entry_path: &Path) -> Result<Vec<PathBuf>> {
    let config = package_compiler(package, &package.manifest.dependencies)?.config;
    let resolver = ModuleResolver::new(package.root.clone(), config);
    let mut seen = HashSet::new();
    let mut ordered = Vec::new();
    collect_package_module_recursive(&resolver, entry_path, &mut seen, &mut ordered)?;
    Ok(ordered)
}

fn collect_package_module_recursive(
    resolver: &ModuleResolver,
    path: &Path,
    seen: &mut HashSet<PathBuf>,
    ordered: &mut Vec<PathBuf>,
) -> Result<()> {
    let normalized = path.to_path_buf();
    if !seen.insert(normalized.clone()) {
        return Ok(());
    }
    let source = fs::read_to_string(&normalized).map_err(|source| CompilerError::Io {
        path: normalized.clone(),
        source,
    })?;
    for dependency in resolver.collect_dependencies(&source, &normalized)? {
        if dependency.specifier.starts_with('.') {
            collect_package_module_recursive(
                resolver,
                &dependency.resolved.source_path,
                seen,
                ordered,
            )?;
        }
    }
    ordered.push(normalized);
    Ok(())
}

fn logical_module_id(root: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(root).unwrap_or(path);
    let mut normalized = relative.to_string_lossy().replace('\\', "/");
    if let Some(stripped) = normalized.strip_suffix(".xl") {
        normalized = stripped.to_string();
    } else if let Some(stripped) = normalized.strip_suffix(".luau") {
        normalized = stripped.to_string();
    } else if let Some(stripped) = normalized.strip_suffix(".lua") {
        normalized = stripped.to_string();
    }
    if normalized.ends_with("/init") {
        normalized.truncate(normalized.len() - "/init".len());
    }
    if normalized.is_empty() {
        "init".to_string()
    } else {
        normalized
    }
}

fn rewrite_bundle_requires(
    source: &str,
    current_path: &Path,
    resolver: &ModuleResolver,
    bundle_path: &str,
    dependencies: &BTreeMap<String, String>,
) -> Result<String> {
    let mut rewritten = source.to_string();
    let calls = find_require_calls(source)?;
    for call in calls.into_iter().rev() {
        if call.specifier.starts_with('.') {
            let resolved = resolver
                .resolve_require_path(current_path, &call.specifier)?
                .ok_or_else(|| {
                    CompilerError::Other(format!("unable to resolve {}", call.specifier))
                })?;
            let module_id = logical_module_id(
                current_path.parent().unwrap_or(Path::new(".")),
                &resolved.source_path,
            );
            rewritten.replace_range(
                call.start..call.end,
                &format!("__xlpkg_require({})", quote_string(&module_id)),
            );
            continue;
        }
        if let Some(alias) = call.specifier.strip_prefix('@')
            && dependencies.contains_key(alias)
        {
            let replacement = format!("__deps.{}", sanitize_identifier(alias));
            if call.has_parens {
                rewritten.replace_range(call.start..call.end, &replacement);
            } else {
                rewritten.replace_range(call.start..call.end, &replacement);
            }
            continue;
        }
        if call.specifier == bundle_path {
            rewritten.replace_range(call.start..call.end, "__deps");
        }
    }
    Ok(rewritten)
}

#[derive(Debug, Clone)]
struct RequireCallInfo {
    start: usize,
    end: usize,
    specifier: String,
    has_parens: bool,
}

fn find_require_calls(source: &str) -> Result<Vec<RequireCallInfo>> {
    let tokens = crate::lexer::Lexer::new(source).tokenize()?;
    let mut calls = Vec::new();
    let mut index = 0usize;
    while index < tokens.len() {
        let token = &tokens[index];
        if token.kind == crate::lexer::TokenKind::Identifier && token.lexeme == "require" {
            if let Some(string_token) = tokens.get(index + 1)
                && string_token.kind == crate::lexer::TokenKind::String
            {
                calls.push(RequireCallInfo {
                    start: token.span.start,
                    end: string_token.span.end,
                    specifier: decode_string_token(&string_token.lexeme),
                    has_parens: false,
                });
                index += 2;
                continue;
            }
            if matches!(
                tokens.get(index + 1).map(|token| &token.kind),
                Some(crate::lexer::TokenKind::Symbol(
                    crate::lexer::Symbol::LParen
                ))
            ) && matches!(
                tokens.get(index + 2).map(|token| &token.kind),
                Some(crate::lexer::TokenKind::String)
            ) && matches!(
                tokens.get(index + 3).map(|token| &token.kind),
                Some(crate::lexer::TokenKind::Symbol(
                    crate::lexer::Symbol::RParen
                ))
            ) {
                let string_token = &tokens[index + 2];
                let end_token = &tokens[index + 3];
                calls.push(RequireCallInfo {
                    start: token.span.start,
                    end: end_token.span.end,
                    specifier: decode_string_token(&string_token.lexeme),
                    has_parens: true,
                });
                index += 4;
                continue;
            }
        }
        index += 1;
    }
    Ok(calls)
}

fn decode_string_token(text: &str) -> String {
    if text.len() >= 2 {
        text[1..text.len() - 1].to_string()
    } else {
        text.to_string()
    }
}

fn quote_string(text: &str) -> String {
    let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

fn render_packages_bundle(packages: &[PackageBundle]) -> String {
    let mut lines = vec![
        "-- Generated by xluau. Do not edit manually.".to_string(),
        format!(
            "-- xluau v{} | generated {}",
            XLUAU_VERSION,
            current_timestamp()
        ),
        String::new(),
    ];
    for package in packages {
        for alias in &package.type_aliases {
            lines.push(alias.clone());
        }
        lines.push(package.surface_type.clone());
        lines.push(String::new());
    }
    for package in packages {
        lines.push(package.module_iife.clone());
        lines.push(String::new());
    }
    lines.push("return {".to_string());
    for package in packages {
        let package_type_name = sanitize_identifier(&package.package_id);
        for alias in &package.alias_names {
            lines.push(format!(
                "    {} = _{} :: {},",
                sanitize_identifier(alias),
                sanitize_identifier(&package.package_id),
                package_type_name
            ));
        }
    }
    lines.push("}".to_string());
    lines.join("\n")
}

fn render_package_iife(
    package_id: &str,
    modules: &[BundledModule],
    entry_file: &str,
    dependency_ids: &BTreeMap<String, String>,
) -> String {
    let entry_id = if entry_file == "init.xl" {
        "init".to_string()
    } else {
        Path::new(entry_file)
            .with_extension("")
            .to_string_lossy()
            .replace('\\', "/")
    };
    let local_name = sanitize_identifier(package_id);
    let deps_table = if dependency_ids.is_empty() {
        "{}".to_string()
    } else {
        format!(
            "{{ {} }}",
            dependency_ids
                .iter()
                .map(|(alias, package_id)| format!(
                    "{} = _{}",
                    sanitize_identifier(alias),
                    sanitize_identifier(package_id)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let mut lines = vec![format!("local _{} = (function(__deps)", local_name)];
    lines.push("    local __modules = {}".to_string());
    lines.push("    local __cache = {}".to_string());
    lines.push("    local function __xlpkg_require(name)".to_string());
    lines.push("        if __cache[name] ~= nil then return __cache[name] end".to_string());
    lines.push("        local module_loader = __modules[name]".to_string());
    lines.push("        if module_loader == nil then error(\"unknown package module: \" .. tostring(name)) end".to_string());
    lines.push("        local value = module_loader()".to_string());
    lines.push("        __cache[name] = value".to_string());
    lines.push("        return value".to_string());
    lines.push("    end".to_string());
    for module in modules {
        lines.push(format!(
            "    __modules[{}] = function()",
            quote_string(&module.id)
        ));
        for line in module.luau.lines() {
            lines.push(format!("        {line}"));
        }
        lines.push("    end".to_string());
    }
    lines.push(format!(
        "    return __xlpkg_require({})",
        quote_string(&entry_id)
    ));
    lines.push(format!("end)({deps_table})"));
    lines.join("\n")
}

fn minify_luau(source: &str) -> String {
    source
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("--"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn sanitize_identifier(text: &str) -> String {
    let mut output = String::new();
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            output.push(ch);
        } else {
            output.push('_');
        }
    }
    if output.is_empty() {
        "_".to_string()
    } else {
        output
    }
}

fn infer_package_types(package_id: &str, source: &str) -> (Vec<String>, String) {
    let exported_types = collect_exported_types(source);
    let exported_names = exported_types
        .iter()
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();
    let mut aliases = Vec::new();
    for (name, body) in exported_types {
        aliases.push(format!(
            "export type {}_{} = {}",
            sanitize_identifier(package_id),
            name,
            rewrite_exported_type_references(package_id, &body, &exported_names)
        ));
    }
    let fields = infer_public_field_names(source)
        .into_iter()
        .map(|field| format!("    {}: any,", field))
        .collect::<Vec<_>>();
    let type_alias = format!(
        "export type {} = {{\n{}\n}}",
        sanitize_identifier(package_id),
        fields.join("\n")
    );
    (aliases, type_alias)
}

fn collect_exported_types(source: &str) -> Vec<(String, String)> {
    let mut exported = Vec::new();
    let lines = source.lines().collect::<Vec<_>>();
    let mut index = 0;
    while index < lines.len() {
        let trimmed = lines[index].trim();
        if let Some(rest) = trimmed.strip_prefix("export type ")
            && let Some((name, body_start)) = rest.split_once('=')
        {
            let name = name.trim().to_string();
            let mut body_lines = vec![body_start.trim().to_string()];
            let mut brace_depth =
                body_start.matches('{').count() as i32 - body_start.matches('}').count() as i32;
            let mut paren_depth =
                body_start.matches('(').count() as i32 - body_start.matches(')').count() as i32;
            let mut bracket_depth =
                body_start.matches('[').count() as i32 - body_start.matches(']').count() as i32;
            while brace_depth > 0 || paren_depth > 0 || bracket_depth > 0 {
                index += 1;
                if index >= lines.len() {
                    break;
                }
                let next = lines[index].trim();
                body_lines.push(next.to_string());
                brace_depth += next.matches('{').count() as i32 - next.matches('}').count() as i32;
                paren_depth += next.matches('(').count() as i32 - next.matches(')').count() as i32;
                bracket_depth +=
                    next.matches('[').count() as i32 - next.matches(']').count() as i32;
            }
            exported.push((name, body_lines.join("\n")));
        }
        index += 1;
    }
    exported
}

fn rewrite_exported_type_references(
    package_id: &str,
    body: &str,
    exported_names: &[String],
) -> String {
    let prefix = format!("{}_", sanitize_identifier(package_id));
    let mut rewritten = body.to_string();
    for name in exported_names {
        let target = format!("{prefix}{name}");
        rewritten = replace_identifier_tokens(&rewritten, name, &target);
    }
    rewritten
}

fn replace_identifier_tokens(source: &str, needle: &str, replacement: &str) -> String {
    let mut output = String::with_capacity(source.len());
    let chars = source.chars().collect::<Vec<_>>();
    let needle_chars = needle.chars().collect::<Vec<_>>();
    let mut index = 0;
    while index < chars.len() {
        let matches = index + needle_chars.len() <= chars.len()
            && chars[index..index + needle_chars.len()] == needle_chars[..];
        if matches {
            let before_ok = index == 0 || !is_identifier_char(chars[index - 1]);
            let after_index = index + needle_chars.len();
            let after_ok = after_index == chars.len() || !is_identifier_char(chars[after_index]);
            if before_ok && after_ok {
                output.push_str(replacement);
                index = after_index;
                continue;
            }
        }
        output.push(chars[index]);
        index += 1;
    }
    output
}

fn is_identifier_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

fn infer_public_field_names(source: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut lines = source.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        if trimmed == "return {" || trimmed.starts_with("return {") {
            let mut inline = trimmed.trim_start_matches("return").trim().to_string();
            if inline.starts_with('{') && inline.ends_with('}') && inline != "{" {
                inline = inline
                    .trim_start_matches('{')
                    .trim_end_matches('}')
                    .to_string();
                names.extend(extract_table_keys(&inline));
                break;
            }
            while let Some(next) = lines.peek() {
                let candidate = next.trim();
                if candidate == "}" || candidate == "end" {
                    break;
                }
                names.extend(extract_table_keys(candidate));
                lines.next();
            }
        }
    }
    names.sort();
    names.dedup();
    names
}

fn extract_table_keys(line: &str) -> Vec<String> {
    let candidate = line.trim_end_matches(',').trim();
    if let Some((name, _)) = candidate.split_once('=') {
        return vec![sanitize_identifier(name.trim())];
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{
        PackageManager, RegistryIndex, RegistryPackage, collect_exported_types,
        default_alias_for_source, default_config_value_for_source, infer_package_types,
        infer_public_field_names, parse_package_source, read_registry,
    };
    use crate::config::XluauConfig;

    fn temp_root(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("xluau_pkg_{name}_{nonce}"));
        fs::create_dir_all(&root).expect("temp");
        root
    }

    fn write_file(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent");
        }
        fs::write(path, contents).expect("write");
    }

    use std::path::Path;

    #[test]
    fn reads_local_registry() {
        let root = temp_root("registry");
        let index = root.join("index.json");
        write_file(
            &index,
            r#"{"version":1,"packages":{"http":{"repo":"xluau-lang/xluau-http","description":"HTTP","latest":"1.2.0","versions":["1.2.0"]}}}"#,
        );
        let registry = read_registry(&root, index.to_string_lossy().as_ref()).unwrap();
        assert_eq!(registry.packages["http"].repo, "xluau-lang/xluau-http");
    }

    #[test]
    fn defaults_alias_and_config_value_for_registry_source() {
        let registry = RegistryIndex {
            version: 1,
            packages: BTreeMap::from([(
                "http".to_string(),
                RegistryPackage {
                    repo: "xluau-lang/xluau-http".to_string(),
                    description: String::new(),
                    latest: "1.0.0".to_string(),
                    versions: vec!["1.0.0".to_string()],
                },
            )]),
        };
        let source = parse_package_source("http");
        assert_eq!(
            default_alias_for_source(&source, &registry).unwrap(),
            "http"
        );
        assert_eq!(
            default_config_value_for_source(&source, &registry).unwrap(),
            "xluau-http"
        );
    }

    #[test]
    fn infers_public_field_names_from_return_table() {
        let fields = infer_public_field_names(
            r#"
local function get() end
return {
    get = get,
    post = function() end,
}
"#,
        );
        assert_eq!(fields, vec!["get".to_string(), "post".to_string()]);
    }

    #[test]
    fn collects_multiline_exported_types() {
        let exported = collect_exported_types(
            r#"
export type JsonPrimitive = nil | boolean | number | string
export type JsonError = {
    message: string,
    position: number,
}
"#,
        );
        assert_eq!(exported.len(), 2);
        assert_eq!(exported[0].0, "JsonPrimitive");
        assert_eq!(exported[1].0, "JsonError");
        assert!(exported[1].1.contains("message: string"));
        assert!(exported[1].1.contains("position: number"));
    }

    #[test]
    fn rewrites_exported_type_references_for_bundle_surface() {
        let (aliases, surface) = infer_package_types(
            "xluau-json",
            r#"
export type JsonPrimitive = nil | boolean | number | string
export type JsonArray = { JsonValue }
export type JsonObject = { [string]: JsonValue }
export type JsonValue = JsonPrimitive | JsonArray | JsonObject

return {
    decode = function() end,
}
"#,
        );
        assert!(aliases.contains(
            &"export type xluau_json_JsonPrimitive = nil | boolean | number | string".to_string()
        ));
        assert!(
            aliases.contains(
                &"export type xluau_json_JsonArray = { xluau_json_JsonValue }".to_string()
            )
        );
        assert!(aliases.contains(
            &"export type xluau_json_JsonObject = { [string]: xluau_json_JsonValue }".to_string()
        ));
        assert!(aliases.contains(&"export type xluau_json_JsonValue = xluau_json_JsonPrimitive | xluau_json_JsonArray | xluau_json_JsonObject".to_string()));
        assert!(surface.contains("decode: any"));
    }

    #[test]
    fn resolves_relative_registry_path_from_project_root() {
        let root = temp_root("relative_registry");
        write_file(
            &root.join("registry/index.json"),
            r#"{"version":1,"packages":{"json":{"repo":"XLuau/xluau-json","description":"JSON","latest":"1.0.0","versions":["1.0.0"]}}}"#,
        );
        let config = XluauConfig {
            registry: "registry/index.json".to_string(),
            ..XluauConfig::default()
        };
        write_file(
            &root.join("xluau.config.json"),
            &serde_json::to_string_pretty(&config).unwrap(),
        );
        let manager = PackageManager::discover(&root).unwrap();
        let registry = manager.load_registry().unwrap();
        assert_eq!(registry.packages["json"].repo, "XLuau/xluau-json");
    }

    #[test]
    fn installs_relative_local_package_path_from_project_root() {
        let root = temp_root("relative_local_package");
        let package_repo = root.join("packages/json");
        write_file(
            &package_repo.join("xlpkg.json"),
            r#"{"name":"json","version":"1.0.0","repo":"local/json","entry":"init.xl"}"#,
        );
        write_file(
            &package_repo.join("init.xl"),
            r#"
return {
    decode = function(value) return value end,
}
"#,
        );
        let config = XluauConfig {
            packages: BTreeMap::from([("json".to_string(), "file:../packages/json".to_string())]),
            registry: root.join("index.json").to_string_lossy().to_string(),
            ..XluauConfig::default()
        };
        write_file(&root.join("index.json"), r#"{"version":1,"packages":{}}"#);
        write_file(
            &root.join("app/xluau.config.json"),
            &serde_json::to_string_pretty(&config).unwrap(),
        );
        write_file(&root.join("app/src/main.xl"), "return nil");

        let manager = PackageManager::discover(&root.join("app")).unwrap();
        let installed = manager.install_all().unwrap();
        assert_eq!(installed[0].package_id, "json");
        assert!(root.join("app/xluau_packages/json/init.xl").is_file());
    }

    #[test]
    fn installs_local_file_package_and_bundles() {
        let root = temp_root("install_bundle");
        let package_repo = root.join("http_pkg");
        write_file(
            &package_repo.join("xlpkg.json"),
            r#"{"name":"http","version":"1.0.0","repo":"local/http","entry":"init.xl"}"#,
        );
        write_file(
            &package_repo.join("init.xl"),
            r#"
local function get(url: string): string
    return url
end

return {
    get = get,
}
"#,
        );
        let config = XluauConfig {
            packages: BTreeMap::from([(
                "http".to_string(),
                format!("file:{}", package_repo.display()),
            )]),
            registry: root.join("index.json").to_string_lossy().to_string(),
            ..XluauConfig::default()
        };
        write_file(&root.join("index.json"), r#"{"version":1,"packages":{}}"#);
        write_file(
            &root.join("xluau.config.json"),
            &serde_json::to_string_pretty(&config).unwrap(),
        );
        let mut manager = PackageManager::discover(&root).unwrap();
        manager
            .install_requests(&[format!("file:{}", package_repo.display())])
            .unwrap();
        let bundle = fs::read_to_string(root.join("packages.luau")).unwrap();
        assert!(bundle.contains("local _http_pkg ="));
        assert!(bundle.contains("http = _http_pkg :: http_pkg"));
    }

    #[test]
    fn validates_publish_shape() {
        let root = temp_root("publish");
        write_file(
            &root.join("xlpkg.json"),
            r#"{"name":"mypackage","version":"1.0.0","repo":"me/mypackage","entry":"init.xl"}"#,
        );
        write_file(
            &root.join("init.xl"),
            r#"
export type MyOptions = { timeout: number? }
local function doThing(input: string): string
    return input
end
return {
    doThing = doThing,
}
"#,
        );
        let manager = PackageManager::discover(&root).unwrap();
        let validation = manager.validate_publish().unwrap();
        assert_eq!(validation.manifest.name, "mypackage");
        assert_eq!(validation.public_fields, vec!["doThing".to_string()]);
        assert_eq!(validation.exported_types, vec!["MyOptions".to_string()]);
    }
}
