use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use glob::glob;
use luau_parser::parser::Parser as LuauParser;
use thiserror::Error;

use crate::{
    comptime::{ComptimeOptions, expand_program},
    config::XluauConfig,
    emitter::Emitter,
    formatter::format_luau,
    lexer::{Keyword, Lexer, Symbol, Token},
    module::sanitize_roblox_identifier,
    module::{ModuleResolver, detect_circular_dependencies},
    package_manager::ensure_bundle_for_project,
    parser::Parser,
    source_map::{SourceMap, finalize_output},
};

pub type Result<T> = std::result::Result<T, CompilerError>;

#[derive(Debug)]
pub struct BuildArtifact {
    pub input: PathBuf,
    pub output: PathBuf,
    pub luau: String,
    pub rbxmx_output: Option<PathBuf>,
    pub rbxmx: Option<String>,
    pub source_map: Option<SourceMap>,
}

#[derive(Debug)]
struct CompiledOutput {
    luau: String,
    source_map: Option<SourceMap>,
}

#[derive(Debug)]
pub struct ProjectRbxmxArtifact {
    pub output: PathBuf,
    pub rbxmx: String,
}

#[derive(Debug)]
pub struct Compiler {
    pub root: PathBuf,
    pub config: XluauConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RobloxRunContext {
    Legacy,
    Server,
    Client,
}

#[derive(Debug, Clone)]
struct RobloxInstanceKind {
    class_name: String,
    run_context: Option<RobloxRunContext>,
}

#[derive(Debug, Clone)]
struct RobloxSourceSpec {
    instance: RobloxInstanceKind,
    instance_name: String,
    is_init: bool,
}

#[derive(Debug, Clone)]
struct RobloxProjectInstance {
    class_name: String,
    name: String,
    run_context: Option<RobloxRunContext>,
    source: String,
}

#[derive(Debug, Default)]
struct RobloxProjectNode {
    instance: Option<RobloxProjectInstance>,
    children: BTreeMap<String, RobloxProjectNode>,
}

#[derive(Debug, Clone)]
struct RobloxXmlItem {
    class_name: String,
    name: String,
    run_context: Option<RobloxRunContext>,
    source: Option<String>,
    children: Vec<RobloxXmlItem>,
}

#[derive(Debug, Error)]
pub enum CompilerError {
    #[error("io error while accessing {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid config at {path}: {source}")]
    Config {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("lex error: {message}")]
    Lex { message: String },
    #[error("parse error: {message}")]
    Parse { message: String },
    #[error("semantic errors:\n{messages:?}")]
    Semantic { messages: Vec<String> },
    #[error("format error: {message}")]
    Format { message: String },
    #[error("luau validation error: {message}")]
    Validation { message: String },
    #[error("{0}")]
    Other(String),
}

impl Compiler {
    pub fn discover(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        let config = XluauConfig::load_from(&root)?;
        Ok(Self { root, config })
    }

    pub fn compile_source(&self, source: &str) -> Result<String> {
        Ok(self
            .compile_source_with_path(source, Path::new("<memory>"))?
            .luau)
    }

    pub fn compile_source_at_path(&self, source: &str, path: &Path) -> Result<String> {
        Ok(self.compile_source_with_path(source, path)?.luau)
    }

    pub fn build_file(&self, path: &Path) -> Result<BuildArtifact> {
        ensure_bundle_for_project(&self.root, &self.config)?;
        let input = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        };
        self.check_cycles(std::slice::from_ref(&input))?;
        let source = fs::read_to_string(&input).map_err(|source| CompilerError::Io {
            path: input.clone(),
            source,
        })?;
        let compiled = self.compile_source_with_path(&source, &input)?;
        let output = self.output_path_for(&input)?;
        let rbxmx_output = self
            .should_emit_file_rbxmx()
            .then(|| self.rbxmx_output_path_for(&input))
            .transpose()?;
        let rbxmx = if self.should_emit_file_rbxmx() {
            Some(self.build_rbxmx_document(&input, &compiled.luau)?)
        } else {
            None
        };
        Ok(BuildArtifact {
            input,
            output,
            luau: compiled.luau,
            rbxmx_output,
            rbxmx,
            source_map: compiled.source_map,
        })
    }

    pub fn build_project(&self) -> Result<Vec<BuildArtifact>> {
        ensure_bundle_for_project(&self.root, &self.config)?;
        let files = self.collect_project_files()?;
        self.check_cycles(&files)?;

        let mut artifacts = Vec::new();
        for path in files {
            let source = fs::read_to_string(&path).map_err(|source| CompilerError::Io {
                path: path.clone(),
                source,
            })?;
            let compiled = self.compile_source_with_path(&source, &path)?;
            let output = self.output_path_for(&path)?;
            let rbxmx_output = self
                .should_emit_file_rbxmx()
                .then(|| self.rbxmx_output_path_for(&path))
                .transpose()?;
            let rbxmx = if self.should_emit_file_rbxmx() {
                Some(self.build_rbxmx_document(&path, &compiled.luau)?)
            } else {
                None
            };
            artifacts.push(BuildArtifact {
                input: path,
                output,
                luau: compiled.luau,
                rbxmx_output,
                rbxmx,
                source_map: compiled.source_map,
            });
        }
        Ok(artifacts)
    }

    pub fn collect_project_files(&self) -> Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        for pattern in &self.config.include {
            let absolute = self.root.join(pattern);
            let Some(pattern) = absolute.to_str() else {
                return Err(CompilerError::Other(format!(
                    "unsupported glob pattern path: {}",
                    absolute.display()
                )));
            };
            for entry in glob(pattern).map_err(|error| CompilerError::Other(error.to_string()))? {
                let path = entry.map_err(|error| CompilerError::Other(error.to_string()))?;
                if self
                    .config
                    .exclude
                    .iter()
                    .any(|exclude| path.to_string_lossy().contains(exclude))
                {
                    continue;
                }
                files.push(path);
            }
        }
        files.sort();
        Ok(files)
    }

    pub fn write_artifact(&self, artifact: &BuildArtifact) -> Result<()> {
        if let Some(parent) = artifact.output.parent() {
            fs::create_dir_all(parent).map_err(|source| CompilerError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        fs::write(&artifact.output, &artifact.luau).map_err(|source| CompilerError::Io {
            path: artifact.output.clone(),
            source,
        })?;
        if let (Some(path), Some(contents)) = (&artifact.rbxmx_output, &artifact.rbxmx) {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|source| CompilerError::Io {
                    path: parent.to_path_buf(),
                    source,
                })?;
            }
            fs::write(path, contents).map_err(|source| CompilerError::Io {
                path: path.clone(),
                source,
            })?;
        }
        if let Some(source_map) = &artifact.source_map {
            let map_path = artifact.output.with_extension("luau.map");
            let contents = serde_json::to_string_pretty(source_map)
                .map_err(|source| CompilerError::Other(source.to_string()))?;
            fs::write(&map_path, contents).map_err(|source| CompilerError::Io {
                path: map_path,
                source,
            })?;
        }
        Ok(())
    }

    pub fn build_project_rbxmx(
        &self,
        artifacts: &[BuildArtifact],
    ) -> Result<Option<ProjectRbxmxArtifact>> {
        if !self.should_emit_project_rbxmx() {
            return Ok(None);
        }

        let mut root = RobloxProjectNode::default();
        for artifact in artifacts {
            let relative = self.source_relative_path(&artifact.input);
            let spec = self.roblox_source_spec(&relative)?;
            self.insert_project_rbxmx_item(&mut root, &relative, &spec, &artifact.luau)?;
        }

        let root_name = self
            .config
            .roblox_output
            .project_rbxmx
            .root_name
            .clone()
            .unwrap_or_else(|| self.project_name());
        let root_item = root.into_root_item(
            self.config
                .roblox_output
                .project_rbxmx
                .root_class_name
                .clone(),
            sanitize_roblox_identifier(&root_name),
        );

        Ok(Some(ProjectRbxmxArtifact {
            output: self.project_rbxmx_output_path(),
            rbxmx: self.render_rbxmx_document(&root_item),
        }))
    }

    pub fn write_project_rbxmx(&self, artifact: &ProjectRbxmxArtifact) -> Result<()> {
        if let Some(parent) = artifact.output.parent() {
            fs::create_dir_all(parent).map_err(|source| CompilerError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        fs::write(&artifact.output, &artifact.rbxmx).map_err(|source| CompilerError::Io {
            path: artifact.output.clone(),
            source,
        })?;
        Ok(())
    }

    fn output_path_for(&self, input: &Path) -> Result<PathBuf> {
        let mut output = self
            .root
            .join(&self.config.out_dir)
            .join(self.output_relative_path(input)?);
        output.set_extension("luau");
        Ok(output)
    }

    fn rbxmx_output_path_for(&self, input: &Path) -> Result<PathBuf> {
        let mut output = self.output_path_for(input)?;
        output.set_extension("rbxmx");
        Ok(output)
    }

    fn validate_luau(&self, source: &str) -> Result<()> {
        let cst = Self::parse_luau(source, "<memory>");
        if cst.errors.is_empty()
            || Self::wrapped_chunk_is_valid(source)
            || Self::validation_shadow_is_valid(source)
        {
            Ok(())
        } else {
            Err(CompilerError::Validation {
                message: cst
                    .errors
                    .iter()
                    .map(|error| format!("{error:?}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            })
        }
    }

    fn parse_luau(source: &str, uri: &str) -> luau_parser::types::Pointer<luau_parser::types::Cst> {
        let mut parser = LuauParser::new(source);
        parser.parse(uri)
    }

    fn wrapped_chunk_is_valid(source: &str) -> bool {
        let wrapped = format!("do\n{source}\nend");
        Self::parse_luau(&wrapped, "<memory:wrapped>")
            .errors
            .is_empty()
    }

    fn validation_shadow_is_valid(source: &str) -> bool {
        let shadow = Self::sanitize_readonly_type_fields(source);
        shadow != source
            && (Self::parse_luau(&shadow, "<memory:shadow>")
                .errors
                .is_empty()
                || Self::wrapped_chunk_is_valid(&shadow))
    }

    fn sanitize_readonly_type_fields(source: &str) -> String {
        let mut lines = source
            .lines()
            .map(|line| {
                let indent_len = line.len().saturating_sub(line.trim_start().len());
                let (indent, trimmed) = line.split_at(indent_len);
                if let Some(rest) = trimmed.strip_prefix("read ")
                    && let Some(colon_index) = rest.find(':')
                {
                    let key = rest[..colon_index].trim();
                    if !key.is_empty()
                        && key
                            .chars()
                            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '"' | '\''))
                    {
                        return format!("{indent}{rest}");
                    }
                }
                line.to_string()
            })
            .collect::<Vec<_>>()
            .join("\n");
        if source.ends_with('\n') {
            lines.push('\n');
        }
        lines
    }

    fn compile_source_with_path(&self, source: &str, path: &Path) -> Result<CompiledOutput> {
        let resolver = self.module_resolver();
        let rewritten_source = resolver.rewrite_requires(source, path)?;
        let should_force_xluau = Lexer::new(source)
            .tokenize()
            .ok()
            .map(|tokens| self.contains_xluau_tokens(&tokens))
            .unwrap_or(false);
        let emitted_file = self
            .output_path_for(path)
            .unwrap_or_else(|_| PathBuf::from("<memory>.luau"));
        let source_name = path.to_string_lossy().replace('\\', "/");
        if !should_force_xluau && self.validate_luau(&rewritten_source).is_ok() {
            let formatted = format_luau(&rewritten_source)?;
            let source_map = self.config.source_maps.then(|| {
                finalize_output(&formatted, self.config.line_pragmas, path, &emitted_file).1
            });
            return Ok(CompiledOutput {
                luau: formatted,
                source_map,
            });
        }

        let tokens = Lexer::new(source).tokenize()?;
        let mut parser = Parser::new(source, tokens);
        let program = expand_program(
            &parser.parse_program()?,
            ComptimeOptions {
                http: (&self.config.comptime_http).into(),
            },
        )?;
        let mut emitter = Emitter::with_options(
            self.config.luau_target.clone(),
            self.config.task_adapter == "roblox" || self.config.target == "roblox",
            Some(source_name),
            self.config.source_maps || self.config.line_pragmas,
        );
        let raw = emitter.emit_program(&program)?;
        let rewritten_output = resolver.rewrite_requires(&raw, path)?;
        self.validate_luau(&rewritten_output)?;
        let formatted = format_luau(&rewritten_output)?;
        let (luau, source_map) =
            finalize_output(&formatted, self.config.line_pragmas, path, &emitted_file);
        Ok(CompiledOutput {
            luau,
            source_map: self.config.source_maps.then_some(source_map),
        })
    }

    fn module_resolver(&self) -> ModuleResolver {
        ModuleResolver::new(self.root.clone(), self.config.clone())
    }

    fn is_roblox_target(&self) -> bool {
        self.config.target == "roblox"
    }

    fn build_rbxmx_document(&self, input: &Path, luau: &str) -> Result<String> {
        let relative = self.source_relative_path(input);
        let spec = self.roblox_source_spec(&relative)?;
        let item = RobloxXmlItem {
            class_name: spec.instance.class_name,
            name: spec.instance_name,
            run_context: spec.instance.run_context,
            source: Some(luau.to_string()),
            children: Vec::new(),
        };
        Ok(self.render_rbxmx_document(&item))
    }

    fn should_emit_file_rbxmx(&self) -> bool {
        self.is_roblox_target() && self.config.roblox_output.emit_rbxmx
    }

    fn should_emit_project_rbxmx(&self) -> bool {
        self.is_roblox_target() && self.config.roblox_output.project_rbxmx.enabled
    }

    fn source_relative_path(&self, input: &Path) -> PathBuf {
        let root_relative = input.strip_prefix(&self.root).unwrap_or(input);
        root_relative
            .strip_prefix(&self.config.base_dir)
            .unwrap_or(root_relative)
            .to_path_buf()
    }

    fn output_relative_path(&self, input: &Path) -> Result<PathBuf> {
        let relative = self.source_relative_path(input);
        if !self.is_roblox_target() {
            return Ok(relative);
        }

        self.roblox_output_relative_path(&relative)
    }

    fn project_name(&self) -> String {
        self.root
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("project")
            .to_string()
    }

    fn project_rbxmx_output_path(&self) -> PathBuf {
        self.config
            .roblox_output
            .project_rbxmx
            .path
            .as_ref()
            .map(|path| {
                if path.is_absolute() {
                    path.clone()
                } else {
                    self.root.join(path)
                }
            })
            .unwrap_or_else(|| {
                self.root
                    .join(&self.config.out_dir)
                    .join(format!(
                        "{}.rbxmx",
                        sanitize_roblox_identifier(&self.project_name())
                    ))
            })
    }

    fn roblox_output_relative_path(&self, relative: &Path) -> Result<PathBuf> {
        let mut output = PathBuf::new();
        for component in relative.parent().into_iter().flat_map(Path::components) {
            let name = component.as_os_str().to_str().ok_or_else(|| {
                CompilerError::Other(format!(
                    "unsupported Roblox output path component in {}",
                    relative.display()
                ))
            })?;
            output.push(sanitize_roblox_identifier(name));
        }

        let file_name = relative
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| {
                CompilerError::Other(format!(
                    "invalid source filename for Roblox output path: {}",
                    relative.display()
                ))
            })?;
        let stem = relative
            .file_stem()
            .and_then(|value| value.to_str())
            .ok_or_else(|| {
                CompilerError::Other(format!(
                    "invalid source filename for Roblox output path: {}",
                    relative.display()
                ))
            })?;

        let safe_stem = self.roblox_output_file_stem(stem);
        let extension = file_name
            .strip_prefix(stem)
            .unwrap_or_default()
            .trim_start_matches('.');

        if extension.is_empty() {
            output.push(safe_stem);
        } else {
            output.push(format!("{safe_stem}.{extension}"));
        }

        Ok(output)
    }

    fn roblox_output_file_stem(&self, stem: &str) -> String {
        let mut suffixes = [
            self.config.roblox_output.suffixes.server.as_str(),
            self.config.roblox_output.suffixes.legacy.as_str(),
            self.config.roblox_output.suffixes.client.as_str(),
            self.config.roblox_output.suffixes.local.as_str(),
        ];
        suffixes.sort_by_key(|suffix| std::cmp::Reverse(suffix.len()));

        for suffix in suffixes {
            if suffix.is_empty() || !stem.ends_with(suffix) {
                continue;
            }
            let stripped = &stem[..stem.len() - suffix.len()];
            if stripped.is_empty() {
                return sanitize_roblox_identifier(stem);
            }
            return format!("{}{}", sanitize_roblox_identifier(stripped), suffix);
        }

        sanitize_roblox_identifier(stem)
    }

    fn roblox_source_spec(&self, relative: &Path) -> Result<RobloxSourceSpec> {
        let stem = relative
            .file_stem()
            .and_then(|value| value.to_str())
            .ok_or_else(|| {
                CompilerError::Other(format!(
                    "invalid source filename for Roblox output: {}",
                    relative.display()
                ))
            })?;
        let (instance, stripped_stem) = self.roblox_instance_kind_for_stem(stem)?;
        let is_init = self
            .config
            .index_files
            .iter()
            .any(|index| index.eq_ignore_ascii_case(&stripped_stem));
        let instance_name = self.roblox_instance_name_for(relative, &stripped_stem, is_init)?;
        Ok(RobloxSourceSpec {
            instance,
            instance_name,
            is_init,
        })
    }

    fn roblox_instance_kind_for_stem(&self, stem: &str) -> Result<(RobloxInstanceKind, String)> {
        let stem = stem.to_string();
        let mut matched: Option<(usize, RobloxInstanceKind, String)> = None;
        for (suffix, class_name, run_context) in [
            (
                self.config.roblox_output.suffixes.server.as_str(),
                "Script",
                Some(RobloxRunContext::Server),
            ),
            (
                self.config.roblox_output.suffixes.legacy.as_str(),
                "Script",
                Some(RobloxRunContext::Legacy),
            ),
            (
                self.config.roblox_output.suffixes.client.as_str(),
                "Script",
                Some(RobloxRunContext::Client),
            ),
            (
                self.config.roblox_output.suffixes.local.as_str(),
                "LocalScript",
                None,
            ),
        ] {
            if suffix.is_empty() || !stem.ends_with(suffix) {
                continue;
            }
            let stripped = stem[..stem.len() - suffix.len()].to_string();
            if stripped.is_empty() {
                return Err(CompilerError::Other(format!(
                    "invalid Roblox source filename `{stem}` for suffix `{suffix}`"
                )));
            }
            let candidate = (
                suffix.len(),
                RobloxInstanceKind {
                    class_name: class_name.to_string(),
                    run_context,
                },
                stripped,
            );
            if matched
                .as_ref()
                .map(|(length, _, _)| candidate.0 > *length)
                .unwrap_or(true)
            {
                matched = Some(candidate);
            }
        }

        Ok(matched
            .map(|(_, instance, stripped)| (instance, stripped))
            .unwrap_or((
                RobloxInstanceKind {
                    class_name: "ModuleScript".to_string(),
                    run_context: None,
                },
                stem,
            )))
    }

    fn roblox_instance_name_for(
        &self,
        relative: &Path,
        stripped_stem: &str,
        is_init: bool,
    ) -> Result<String> {
        if !is_init {
            return Ok(sanitize_roblox_identifier(stripped_stem));
        }

        if let Some(parent) = relative
            .parent()
            .and_then(|value| value.file_name())
            .and_then(|value| value.to_str())
        {
            return Ok(sanitize_roblox_identifier(parent));
        }

        Ok(sanitize_roblox_identifier(&self.project_name()))
    }

    fn insert_project_rbxmx_item(
        &self,
        root: &mut RobloxProjectNode,
        relative: &Path,
        spec: &RobloxSourceSpec,
        luau: &str,
    ) -> Result<()> {
        let parent = relative.parent().unwrap_or_else(|| Path::new(""));
        let mut node = root;
        for component in parent.components() {
            let name = component.as_os_str().to_str().ok_or_else(|| {
                CompilerError::Other(format!(
                    "unsupported Roblox project path: {}",
                    relative.display()
                ))
            })?;
            node = node
                .children
                .entry(sanitize_roblox_identifier(name))
                .or_default();
        }

        let instance = RobloxProjectInstance {
            class_name: spec.instance.class_name.clone(),
            name: spec.instance_name.clone(),
            run_context: spec.instance.run_context,
            source: luau.to_string(),
        };

        if spec.is_init {
            if node.instance.is_some() {
                return Err(CompilerError::Other(format!(
                    "duplicate Roblox init output for {}",
                    relative.display()
                )));
            }
            node.instance = Some(instance);
            return Ok(());
        }

        let child = node.children.entry(spec.instance_name.clone()).or_default();
        if child.instance.is_some() || !child.children.is_empty() {
            return Err(CompilerError::Other(format!(
                "conflicting Roblox output path for {}",
                relative.display()
            )));
        }
        child.instance = Some(instance);
        Ok(())
    }

    fn render_rbxmx_document(&self, root: &RobloxXmlItem) -> String {
        let mut xml = String::from(concat!(
            "<roblox xmlns:xmime=\"http://www.w3.org/2005/05/xmlmime\" ",
            "xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" ",
            "xsi:noNamespaceSchemaLocation=\"http://www.roblox.com/roblox.xsd\" version=\"4\">\n"
        ));
        let mut next_referent = 0usize;
        self.render_rbxmx_item(root, 1, &mut next_referent, &mut xml);
        xml.push_str("</roblox>\n");
        xml
    }

    fn render_rbxmx_item(
        &self,
        item: &RobloxXmlItem,
        indent_level: usize,
        next_referent: &mut usize,
        xml: &mut String,
    ) {
        let indent = "    ".repeat(indent_level);
        let property_indent = "    ".repeat(indent_level + 1);
        let value_indent = "    ".repeat(indent_level + 2);
        let referent = format!("RBX{}", *next_referent);
        *next_referent += 1;

        xml.push_str(&format!(
            "{indent}<Item class=\"{}\" referent=\"{referent}\">\n",
            xml_escape(&item.class_name)
        ));
        xml.push_str(&format!("{property_indent}<Properties>\n"));
        xml.push_str(&format!(
            "{value_indent}<string name=\"Name\">{}</string>\n",
            xml_escape(&item.name)
        ));
        if let Some(run_context) = item.run_context {
            xml.push_str(&format!(
                "{value_indent}<token name=\"RunContext\">{}</token>\n",
                run_context.token_value()
            ));
        }
        if let Some(source) = &item.source {
            xml.push_str(&format!(
                "{value_indent}<ProtectedString name=\"Source\">{}</ProtectedString>\n",
                xml_escape(source)
            ));
        }
        xml.push_str(&format!("{property_indent}</Properties>\n"));
        for child in &item.children {
            self.render_rbxmx_item(child, indent_level + 1, next_referent, xml);
        }
        xml.push_str(&format!("{indent}</Item>\n"));
    }

    fn check_cycles(&self, entry_points: &[PathBuf]) -> Result<()> {
        detect_circular_dependencies(&self.module_resolver(), entry_points)
    }

    fn contains_xluau_tokens(&self, tokens: &[Token]) -> bool {
        tokens.iter().any(|token| match token.kind {
            crate::lexer::TokenKind::Keyword(
                Keyword::Const
                | Keyword::Comptime
                | Keyword::Enum
                | Keyword::Switch
                | Keyword::Match
                | Keyword::Case
                | Keyword::Default
                | Keyword::Fallthrough
                | Keyword::Object
                | Keyword::Extends
                | Keyword::Task
                | Keyword::Yield
                | Keyword::Spawn
                | Keyword::Fire
                | Keyword::On
                | Keyword::Once
                | Keyword::Signal
                | Keyword::State
                | Keyword::Catch
                | Keyword::Watch,
            ) => true,
            crate::lexer::TokenKind::Symbol(
                Symbol::DoubleQuestion | Symbol::DoubleQuestionEqual | Symbol::PipeGreater,
            ) => true,
            _ => false,
        })
    }
}

impl RobloxRunContext {
    fn token_value(self) -> u8 {
        match self {
            Self::Legacy => 0,
            Self::Server => 1,
            Self::Client => 2,
        }
    }
}

impl RobloxProjectNode {
    fn into_root_item(self, fallback_class_name: String, fallback_name: String) -> RobloxXmlItem {
        let children = self
            .children
            .into_iter()
            .map(|(name, child)| child.into_xml_item(name))
            .collect();
        if let Some(instance) = self.instance {
            RobloxXmlItem {
                class_name: instance.class_name,
                name: fallback_name,
                run_context: instance.run_context,
                source: Some(instance.source),
                children,
            }
        } else {
            RobloxXmlItem {
                class_name: fallback_class_name,
                name: fallback_name,
                run_context: None,
                source: None,
                children,
            }
        }
    }

    fn into_xml_item(self, fallback_name: String) -> RobloxXmlItem {
        let children = self
            .children
            .into_iter()
            .map(|(name, child)| child.into_xml_item(name))
            .collect();
        if let Some(instance) = self.instance {
            RobloxXmlItem {
                class_name: instance.class_name,
                name: instance.name,
                run_context: instance.run_context,
                source: Some(instance.source),
                children,
            }
        } else {
            RobloxXmlItem {
                class_name: "Folder".to_string(),
                name: fallback_name,
                run_context: None,
                source: None,
                children,
            }
        }
    }
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::Compiler;
    use crate::{module::sanitize_roblox_identifier, package_manager::PackageManager};

    fn compiler() -> Compiler {
        Compiler::discover(".").expect("compiler")
    }

    fn temp_project(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("xluau_{name}_{nonce}"));
        fs::create_dir_all(&root).expect("temp project root");
        root
    }

    fn write_file(root: &Path, relative: &str, contents: &str) {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("parent");
        }
        fs::write(path, contents).expect("write file");
    }

    #[test]
    fn resolves_package_requires_through_generated_bundle() {
        let root = temp_project("package_bundle_require");
        let package_repo = root.join("http_pkg");
        write_file(
            &package_repo,
            "xlpkg.json",
            r#"{"name":"http","version":"1.0.0","repo":"local/http","entry":"init.xl"}"#,
        );
        write_file(
            &package_repo,
            "init.xl",
            r#"
local function get(url: string): string
    return url
end

return {
    get = get,
}
"#,
        );
        write_file(
            &root,
            "xluau.config.json",
            &format!(
                r#"{{
  "include": ["src/**/*.xl"],
  "registry": "{}",
  "packages": {{
    "http": "file:{}"
  }}
}}"#,
                root.join("index.json")
                    .display()
                    .to_string()
                    .replace('\\', "\\\\"),
                package_repo.display().to_string().replace('\\', "\\\\")
            ),
        );
        write_file(&root, "index.json", r#"{"version":1,"packages":{}}"#);
        write_file(
            &root,
            "src/main.xl",
            r#"
local http = require "@http"
return http.get("https://example.com")
"#,
        );
        let mut manager = PackageManager::discover(&root).unwrap();
        manager
            .install_requests(&[format!("file:{}", package_repo.display())])
            .unwrap();
        let compiler = Compiler::discover(&root).unwrap();
        let artifact = compiler.build_file(&root.join("src/main.xl")).unwrap();
        assert!(artifact.luau.contains(r#"require("./packages.luau").http"#));
        assert!(root.join("packages.luau").is_file());
    }

    #[test]
    fn lowers_nullish_and_ternary() {
        let source = r#"
local timeout = config.timeout ?? 30
local label = isAdmin ? "admin" : "user"
"#;
        let output = compiler().compile_source(source).unwrap();
        assert!(output.contains("local _lhs0 = config.timeout"));
        assert!(output.contains("if _lhs0 ~= nil then _lhs0 else 30"));
        assert!(output.contains(r#"if isAdmin then "admin" else "user""#));
    }

    #[test]
    fn lowers_optional_chain_and_pipe() {
        let source = r#"
local words = str |> :lower() |> :split(" ")
local hp = entity?.GetHealth()
local first = list?.[1]
"#;
        let output = compiler().compile_source(source).unwrap();
        assert!(output.contains(r#"local _pipe0 = str:lower()"#));
        assert!(output.contains(r#"_pipe0:split(" ")"#));
        assert!(output.contains("local _opt"));
        assert!(output.contains("GetHealth"));
        assert!(output.contains("[1]"));
    }

    #[test]
    fn lowers_const_and_destructuring() {
        let source = r#"
const PI = 3.14
local { x, y: posY, role = "user" } = point
"#;
        let output = compiler().compile_source(source).unwrap();
        assert!(output.contains("local PI = 3.14"));
        assert!(output.contains("local x ="));
        assert!(output.contains("local posY ="));
        assert!(output.contains(r#"if _d"#));
    }

    #[test]
    fn rejects_const_reassignment() {
        let source = r#"
const PI = 3.14
PI = 4
"#;
        let err = compiler().compile_source(source).unwrap_err();
        assert!(format!("{err}").contains("cannot assign to const"));
    }

    #[test]
    fn lowers_nullish_assignment_pipe_placeholders_and_destructured_scopes() {
        let source = r#"
config.retries ??= getRetries()
local result = numbers |> filter(_, isEven) |> map(_, double)

function update({ x, y }: Point, [head, ...tail]: ArrayLike)
    return x, y, head, tail
end

for { name, score } in players do
    print(name, score)
end
"#;
        let output = compiler().compile_source(source).unwrap();
        assert!(output.contains("if config.retries == nil then"));
        assert!(output.contains("config.retries = getRetries()"));
        assert!(output.contains("filter(numbers, isEven)"));
        assert!(output.contains("map("));
        assert!(output.contains("function update(_param"));
        assert!(output.contains("local head ="));
        assert!(output.contains("for _for"));
        assert!(output.contains("local name ="));
        assert!(output.contains("local score ="));
    }

    #[test]
    fn accepts_wide_luau_grammar_via_passthrough() {
        let source = r##"
local foo: { [string]: number, bar: (number, string) -> () } = { bar = function(_x, _y) end }
foo.bar(1, "x")
foo["a"] += 1
local cast = foo :: any
local label = if cast then `value: {cast}` else "none"
export type Box<T = string> = { value: T }
for key, value in foo do
    if key then
        continue
    end
end
"##;
        let output = compiler().compile_source(source).unwrap();
        assert!(output.contains("foo[\"a\"] += 1"));
        assert!(output.contains("foo :: any"));
        assert!(output.contains("`value: {cast}`"));
        assert!(output.contains("export type Box"));
        assert!(output.contains("continue"));
    }

    #[test]
    fn supports_mixed_xluau_with_luau_compound_and_type_assertion() {
        let source = r#"
local count = (value :: number) ?? 0
stats.total += 1
"#;
        let output = compiler().compile_source(source).unwrap();
        assert!(output.contains("(value :: number)"));
        assert!(output.contains("stats.total = stats.total + 1"));
        assert!(output.contains("if _lhs"));
    }

    #[test]
    fn accepts_top_level_return_in_luau_passthrough() {
        let source = r#"
local value = 1
return value
"#;
        let output = compiler().compile_source(source).unwrap();
        assert!(output.contains("local value = 1"));
        assert!(output.contains("return value"));
    }

    #[test]
    fn resolves_aliases_and_index_files_for_filesystem_target() {
        let root = temp_project("phase3_filesystem");
        write_file(
            &root,
            "xluau.config.json",
            r#"{
  "include": ["src/**/*.xl"],
  "baseDir": "src",
  "target": "filesystem",
  "paths": {
    "@shared": "./src/shared"
  }
}"#,
        );
        write_file(
            &root,
            "src/main.xl",
            r#"local utils = require "@shared/utils"
local math = require("@shared/math")
"#,
        );
        write_file(&root, "src/shared/utils/init.xl", "return {}");
        write_file(&root, "src/shared/math.xl", "return {}");

        let compiler = Compiler::discover(&root).unwrap();
        let artifact = compiler.build_file(&root.join("src/main.xl")).unwrap();
        assert!(artifact.luau.contains(r#"require("./src/shared/utils")"#));
        assert!(artifact.luau.contains(r#"require("./src/shared/math")"#));
    }

    #[test]
    fn resolves_wildcard_aliases_for_filesystem_target() {
        let root = temp_project("phase3_filesystem_wildcard");
        write_file(
            &root,
            "xluau.config.json",
            r#"{
  "include": ["src/**/*.xl"],
  "baseDir": "src",
  "target": "filesystem",
  "paths": {
    "@shared/*": "./src/shared/*"
  }
}"#,
        );
        write_file(
            &root,
            "src/main.xl",
            r#"local math = require "@shared/math""#,
        );
        write_file(&root, "src/shared/math.xl", "return {}");

        let compiler = Compiler::discover(&root).unwrap();
        let artifact = compiler.build_file(&root.join("src/main.xl")).unwrap();
        assert!(artifact.luau.contains(r#"require("./src/shared/math")"#));
    }

    #[test]
    fn resolves_aliases_for_roblox_target() {
        let root = temp_project("phase3_roblox");
        write_file(
            &root,
            "xluau.config.json",
            r#"{
  "include": ["src/**/*.xl"],
  "baseDir": "src",
  "target": "roblox",
  "paths": {
    "@shared": "./src/shared"
  }
}"#,
        );
        write_file(
            &root,
            "src/server/main.xl",
            r#"local math = require "@shared/math""#,
        );
        write_file(&root, "src/shared/math.xl", "return {}");

        let compiler = Compiler::discover(&root).unwrap();
        let artifact = compiler
            .build_file(&root.join("src/server/main.xl"))
            .unwrap();
        assert!(
            artifact
                .luau
                .contains("require(script.Parent.Parent.shared.math)")
        );
    }

    #[test]
    fn resolves_aliases_for_custom_target() {
        let root = temp_project("phase3_custom");
        write_file(
            &root,
            "xluau.config.json",
            r#"{
  "include": ["src/**/*.xl"],
  "baseDir": "src",
  "target": "custom",
  "customTargetFunction": "resolveModule",
  "paths": {
    "@shared": "./src/shared"
  }
}"#,
        );
        write_file(
            &root,
            "src/main.xl",
            r#"local math = require "@shared/math""#,
        );
        write_file(&root, "src/shared/math.xl", "return {}");

        let compiler = Compiler::discover(&root).unwrap();
        let artifact = compiler.build_file(&root.join("src/main.xl")).unwrap();
        assert!(
            artifact
                .luau
                .contains(r#"require(resolveModule("shared/math"))"#)
        );
    }

    #[test]
    fn detects_circular_dependencies() {
        let root = temp_project("phase3_cycle");
        write_file(
            &root,
            "xluau.config.json",
            r#"{
  "include": ["src/**/*.xl"],
  "baseDir": "src",
  "target": "filesystem",
  "paths": {
    "@app": "./src"
  }
}"#,
        );
        write_file(&root, "src/main.xl", r#"local a = require "@app/a""#);
        write_file(&root, "src/a.xl", r#"local b = require "@app/b""#);
        write_file(&root, "src/b.xl", r#"local c = require "@app/c""#);
        write_file(&root, "src/c.xl", r#"local a = require "@app/a""#);

        let compiler = Compiler::discover(&root).unwrap();
        let error = compiler.build_file(&root.join("src/main.xl")).unwrap_err();
        assert!(format!("{error}").contains("Circular dependency detected"));
        assert!(format!("{error}").contains("src/a.xl"));
        assert!(format!("{error}").contains("src/b.xl"));
        assert!(format!("{error}").contains("src/c.xl"));
    }

    #[test]
    fn leaves_non_alias_string_requires_unchanged() {
        let source = r#"
local sibling = require "./sibling"
local parent = require("../parent")
"#;
        let output = compiler().compile_source(source).unwrap();
        assert!(output.contains(r#"require "./sibling""#));
        assert!(output.contains(r#"require("../parent")"#));
    }

    #[test]
    fn lowers_phase4_switch_enum_and_do_expression() {
        let source = r#"
enum Direction
    North
    South
    East
    West
end

local dir: Direction = Direction.North

switch dir
    case Direction.North
        print("north")
    case Direction.South
        print("south")
    case Direction.East
        print("east")
    case Direction.West
        print("west")
end

local label = switch dir
    case Direction.North then "N"
    case Direction.South then "S"
    default then "?"
end

local distance = do
    local dx = b.x - a.x
    local dy = b.y - a.y
    math.sqrt(dx ^ 2 + dy ^ 2)
end
"#;
        let output = compiler().compile_source(source).unwrap();
        assert!(output.contains("type Direction = \"North\" | \"South\" | \"East\" | \"West\""));
        assert!(output.contains("local Direction = table.freeze({"));
        assert!(output.contains("local _sw"));
        assert!(output.contains("local _swexpr"));
        assert!(output.contains("local _de"));
        assert!(output.contains("math.sqrt"));
    }

    #[test]
    fn lowers_phase4_match_and_comprehensions() {
        let source = r#"
type Result = { kind: "ok", value: number } | { kind: "err", error: string }

local doubled = { x * 2 for _, x in numbers if x > 0 }
local byValue = { [x] = x ^ 2 for _, x in numbers }
local flat = { value for _, row in matrix for _, value in row }

match result
    { kind = "ok", value = v }
        print(v)
    { kind = "err", error = e }
        print(e)
end
"#;
        let output = compiler().compile_source(source).unwrap();
        assert!(output.contains("local _comp"));
        assert!(output.contains("table.insert("));
        assert!(output.contains("for _, x in numbers do"));
        assert!(output.contains("local _mbind"));
        assert!(output.contains("local _mcond"));
        assert!(output.contains("print(v)"));
        assert!(output.contains("print(e)"));
    }

    #[test]
    fn rejects_non_exhaustive_switch_over_union() {
        let source = r#"
type Direction = "North" | "South"
local dir: Direction = "North"

switch dir
    case "North"
        print("north")
end
"#;
        let err = compiler().compile_source(source).unwrap_err();
        assert!(format!("{err}").contains("non-exhaustive switch"));
        assert!(format!("{err}").contains("South"));
    }

    #[test]
    fn rejects_non_exhaustive_match_over_discriminated_union() {
        let source = r#"
type Result = { kind: "ok", value: number } | { kind: "err", error: string }
local result: Result = { kind = "ok", value = 1 }

match result
    { kind = "ok", value = v }
        print(v)
end
"#;
        let err = compiler().compile_source(source).unwrap_err();
        assert!(format!("{err}").contains("non-exhaustive match"));
        assert!(format!("{err}").contains("err"));
    }

    #[test]
    fn lowers_phase5_generics_and_explicit_type_arguments() {
        let source = r#"
local function max<T extends Comparable>(a: T, b: T): T
    return if a > b then a else b
end

local function makeEmpty<T>(): T
    return nil :: any
end

local function fetch<T, Err = string>(url: string): Result<T, Err>
    return nil :: any
end

local empty = makeEmpty::<{ x: number, y: number }>()
local user = fetch::<User>("/api/user")
"#;
        let output = compiler().compile_source(source).unwrap();
        assert!(
            output.contains("local function max<T>(a: (T & Comparable), b: (T & Comparable)): T")
        );
        assert!(output.contains("((makeEmpty :: () -> { x: number, y: number }))()"));
        assert!(output.contains("((fetch :: (string) -> Result<User, string>))(\"/api/user\")"));
    }

    #[test]
    fn lowers_phase5_type_utilities_and_freeze() {
        let source = r#"
type Config = {
    readonly host: string,
    port: number,
    timeout: number?,
}

type PartialConfig = Partial<Config>
type HostConfig = Pick<Config, "host" | "port">
type Flags = Record<"debug" | "verbose", boolean>
type Present = Exclude<"ok" | "err" | nil, nil>

local function fetchUser(id: string, retries: number): User
    return nil :: any
end

type UserResult = ReturnType<typeof(fetchUser)>
type FetchParams = Parameters<typeof(fetchUser)>
type RetrySnapshot = Partial<Readonly<Pick<Config, "timeout">>>

const DEFAULTS = freeze {
    timeout = 30,
    retries = 3,
    host = "localhost",
}

type Defaults = Readonly<typeof(DEFAULTS)>
"#;
        let output = compiler().compile_source(source).unwrap();
        assert!(output.contains(
            "type Config = {\n    read host: string,\n    port: number,\n    timeout: number?,\n}"
        ));
        assert!(output.contains("type PartialConfig = {\n    read host: string?,\n    port: number?,\n    timeout: number?,\n}"));
        assert!(
            output.contains("type HostConfig = {\n    read host: string,\n    port: number,\n}")
        );
        assert!(output.contains("type Flags = { debug: boolean, verbose: boolean }"));
        assert!(output.contains("type Present = \"ok\" | \"err\""));
        assert!(output.contains("type UserResult = User"));
        assert!(output.contains("type FetchParams = (string, number)"));
        assert!(output.contains("type RetrySnapshot = {\n    read timeout: number?,\n}"));
        assert!(output.contains(
            "local DEFAULTS = table.freeze({timeout = 30, retries = 3, host = \"localhost\"})"
        ));
        assert!(output.contains("type Defaults = {\n    read timeout: number,\n    read retries: number,\n    read host: string,\n}"));
    }

    #[test]
    fn lowers_phase5_readonly_for_legacy_target() {
        let root = temp_project("phase5_legacy_readonly");
        write_file(
            &root,
            "xluau.config.json",
            r#"{
  "include": ["src/**/*.xl"],
  "luauTarget": "legacy"
}"#,
        );
        write_file(
            &root,
            "src/main.xl",
            r#"
type Config = {
    readonly host: string,
    port: number,
}
"#,
        );

        let compiler = Compiler::discover(&root).unwrap();
        let artifact = compiler.build_file(&root.join("src/main.xl")).unwrap();
        assert!(artifact.luau.contains("type Config = {\n    host: string,  -- @readonly (XLuau-enforced)\n    port: number,\n}"));
        assert!(!artifact.luau.contains("readonly host"));
        assert!(!artifact.luau.contains("read host"));
    }

    #[test]
    fn utility_types_resolve_object_signatures() {
        let source = r#"
object Hero
    name: string

    function new(name: string): Hero
        self.name = name
    end

    function label(prefix: string): string
        return prefix .. self.name
    end
end

type HeroCtor = ReturnType<typeof(Hero.new)>
type HeroLabelArgs = Parameters<typeof(Hero.label)>
"#;
        let output = compiler().compile_source(source).unwrap();
        assert!(
            output.contains("type Hero = { name: string, label: (self: Hero, string) -> string }")
        );
        assert!(output.contains("type HeroCtor = Hero"));
        assert!(output.contains("type HeroLabelArgs = (Hero, string)"));
    }

    #[test]
    fn lowers_phase6_objects_and_inheritance() {
        let source = r#"
object Animal
    name: string
    sound: string

    function new(name: string, sound: string): Animal
        self.name = name
        self.sound = sound
    end

    function speak(): string
        return self.sound
    end

    function static create(name: string): Animal
        return Animal.new(name, "...")
    end
end

object Dog extends Animal
    breed: string

    function new(name: string, breed: string): Dog
        super.new(name, "Woof")
        self.breed = breed
    end

    function speak(): string
        return super.speak(self)
    end
end
"#;
        let output = compiler().compile_source(source).unwrap();
        assert!(output.contains(
            "type Animal = { name: string, sound: string, speak: (self: Animal) -> string }"
        ));
        assert!(output.contains("local Animal = {}"));
        assert!(output.contains("Animal.__index = Animal"));
        assert!(output.contains("function Animal.new(name: string, sound: string): Animal"));
        assert!(output.contains("local self = setmetatable({} :: Animal, Animal)"));
        assert!(output.contains("function Animal:speak(): string"));
        assert!(output.contains("function Animal.create(name: string): Animal"));
        assert!(
            output.contains("type Dog = Animal & { breed: string, speak: (self: Dog) -> string }")
        );
        assert!(output.contains("setmetatable(Dog, { __index = Animal })"));
        assert!(output.contains("local self = Animal.new(name, \"Woof\") :: Dog"));
        assert!(output.contains("setmetatable(self, Dog)"));
        assert!(output.contains("return Animal.speak(self)"));
    }

    #[test]
    fn lowers_phase6_task_functions_and_spawn() {
        let source = r#"
task function loadPlayer(id: number): Player
    local data = yield fetchData(id)
    local inv = yield fetchInventory(id)
    return buildPlayer(data, inv)
end

spawn loadPlayer(42)
    then |player|
        setupHUD(player)
    catch |err|
        warn("Failed:", err)
end
"#;
        let output = compiler().compile_source(source).unwrap();
        assert!(output.contains("local function loadPlayer(id: number): thread"));
        assert!(output.contains("return coroutine.create(function()"));
        assert!(output.contains("local data = coroutine.yield(fetchData(id))"));
        assert!(output.contains("local inv = coroutine.yield(fetchInventory(id))"));
        assert!(output.contains("local _co"));
        assert!(output.contains("coroutine.resume("));
        assert!(output.contains("coroutine.status("));
        assert!(output.contains("local player ="));
        assert!(output.contains("setupHUD(player)"));
        assert!(output.contains("local err ="));
        assert!(output.contains("warn(\"Failed:\", err)"));
    }

    #[test]
    fn rejects_yield_outside_task_function() {
        let source = r#"
local value = yield fetchData(1)
"#;
        let err = compiler().compile_source(source).unwrap_err();
        assert!(format!("{err}").contains("`yield` is only valid inside a task function"));
    }

    #[test]
    fn lowers_phase6_spawn_for_roblox_adapter() {
        let root = temp_project("phase6_roblox_spawn");
        write_file(
            &root,
            "xluau.config.json",
            r#"{
  "include": ["src/**/*.xl"],
  "target": "roblox",
  "taskAdapter": "roblox"
}"#,
        );
        write_file(
            &root,
            "src/main.xl",
            r#"
task function animate(model: Model)
    yield waitForFrame()
    return model
end

spawn animate(workspace.Model)
"#,
        );

        let compiler = Compiler::discover(&root).unwrap();
        let artifact = compiler.build_file(&root.join("src/main.xl")).unwrap();
        assert!(artifact.luau.contains("task.spawn(function()"));
        assert!(artifact.luau.contains("local _co"));
        assert!(artifact.luau.contains("coroutine.resume("));
    }

    #[test]
    fn lowers_phase7_signals() {
        let source = r#"
signal OnPlayerJoined: (player: Player)
signal OnDied

local conn = on OnPlayerJoined |player|
    setupHUD(player)
end

on OnPlayerJoined |player|
    print(player.Name)
end

once OnDied ||
    cleanup()
end

fire OnPlayerJoined(player)
fire OnDied
"#;
        let output = compiler().compile_source(source).unwrap();
        assert!(output.contains("type _Signal_OnPlayerJoined = {"));
        assert!(output.contains("_handlers: { (player: Player) -> () },"));
        assert!(output.contains("local OnPlayerJoined: _Signal_OnPlayerJoined = {"));
        assert!(output.contains("local _conn0 = OnPlayerJoined:connect(function(player)"));
        assert!(output.contains("local conn = _conn0"));
        assert!(output.contains("OnPlayerJoined:connect(function(player)"));
        assert!(output.contains("OnDied:once(function()"));
        assert!(output.contains("OnPlayerJoined:fire(player)"));
        assert!(output.contains("OnDied:fire()"));
    }

    #[test]
    fn lowers_phase7_reactive_state() {
        let source = r#"
state playerCount: number = 0

watch playerCount |old, new|
    updatePlayerCountUI(new)
end

playerCount = playerCount + 1
playerCount += 2

state currentMap: string? = nil

watch currentMap |old, new|
    loadMap(new)
end

currentMap ??= "Lobby"
"#;
        let output = compiler().compile_source(source).unwrap();
        assert!(output.contains("local playerCount: number = 0"));
        assert!(output.contains("local _watchers_playerCount_"));
        assert!(output.contains("table.insert(_watchers_playerCount_"));
        assert!(output.contains("function(old, new)"));
        assert!(output.contains("do\n    local _old"));
        assert!(output.contains("playerCount = (playerCount + 1)"));
        assert!(output.contains("playerCount = playerCount + 2"));
        assert!(output.contains("_w(_old"));
        assert!(output.contains("local currentMap: string? = nil"));
        assert!(output.contains("currentMap = \"Lobby\""));
    }

    #[test]
    fn lowers_phase8_pattern_literals() {
        let source = r#"
const DATE_PATTERN = pattern`{%d+}-{%d+}-{%d+}`
local year, month, day = date:match(pattern`{year:%d+}-{month:%d+}-{day:%d+}`)
"#;
        let output = compiler().compile_source(source).unwrap();
        assert!(output.contains(r#"local DATE_PATTERN = "(%d+)-(%d+)-(%d+)""#));
        assert!(output.contains(r#"date:match("(%d+)-(%d+)-(%d+)")"#));
    }

    #[test]
    fn emits_phase8_source_maps_and_line_pragmas() {
        let root = temp_project("phase8_sourcemaps");
        write_file(
            &root,
            "xluau.config.json",
            r#"{
  "include": ["src/**/*.xl"],
  "sourceMaps": true,
  "linePragmas": true
}"#,
        );
        write_file(
            &root,
            "src/main.xl",
            r#"
const DATE_PATTERN = pattern`{%d+}-{%d+}-{%d+}`
local year, month, day = date:match(pattern`{year:%d+}-{month:%d+}-{day:%d+}`)
"#,
        );

        let compiler = Compiler::discover(&root).unwrap();
        let artifact = compiler.build_file(&root.join("src/main.xl")).unwrap();
        assert!(artifact.luau.contains("--@line 2"));
        assert!(artifact.source_map.is_some());
        let map = artifact.source_map.unwrap();
        assert_eq!(map.version, 1);
        assert!(map.source_file.ends_with("src/main.xl"));
        assert!(map.emitted_file.ends_with("out/main.luau"));
        assert!(!map.mappings.is_empty());
    }

    #[test]
    fn lowers_comptime_const_into_runtime_literal() {
        let source = r#"
comptime const X = 3
local y = comptime X
"#;
        let output = compiler().compile_source(source).unwrap();
        assert!(output.contains("local y = 3"));
        assert!(!output.contains("comptime"));
    }

    #[test]
    fn lowers_comptime_string_concat() {
        let source = r#"
comptime const NAME = "rbxup" .. "-plugin"
local x = comptime NAME
"#;
        let output = compiler().compile_source(source).unwrap();
        assert!(output.contains(r#"local x = "rbxup-plugin""#));
    }

    #[test]
    fn lowers_comptime_function_calls() {
        let source = r#"
comptime function makeName(prefix: string, name: string): string
    return prefix .. "-" .. name
end

local pluginName = comptime makeName("rbxup", "plugin")
"#;
        let output = compiler().compile_source(source).unwrap();
        assert!(output.contains(r#"local pluginName = "rbxup-plugin""#));
    }

    #[test]
    fn lowers_comptime_if_true_branch() {
        let source = r#"
comptime const ENABLED = true

comptime if ENABLED then
    print("yes")
else
    print("no")
end
"#;
        let output = compiler().compile_source(source).unwrap();
        assert!(output.contains(r#"print("yes")"#));
        assert!(!output.contains(r#"print("no")"#));
    }

    #[test]
    fn lowers_comptime_if_false_branch() {
        let source = r#"
comptime const ENABLED = false

comptime if ENABLED then
    print("yes")
else
    print("no")
end
"#;
        let output = compiler().compile_source(source).unwrap();
        assert!(output.contains(r#"print("no")"#));
        assert!(!output.contains(r#"print("yes")"#));
    }

    #[test]
    fn lowers_comptime_switch() {
        let source = r#"
comptime const TARGET = "roblox"

comptime switch TARGET
    case "roblox"
        print("roblox")
    default
        print("other")
end
"#;
        let output = compiler().compile_source(source).unwrap();
        assert!(output.contains(r#"print("roblox")"#));
        assert!(!output.contains(r#"print("other")"#));
    }

    #[test]
    fn rejects_runtime_values_in_comptime_expressions() {
        let source = r#"
local x = getValue()
comptime const y = x
"#;
        let err = compiler().compile_source(source).unwrap_err();
        assert!(
            format!("{err}").contains("Cannot use runtime local 'x' in a compile-time expression.")
        );
    }

    #[test]
    fn rejects_unsupported_compile_time_builtins() {
        let source = r#"
comptime const now = os.clock()
"#;
        let err = compiler().compile_source(source).unwrap_err();
        assert!(format!("{err}").contains("Function 'os.clock' is not available at compile time."));
    }

    #[test]
    fn rejects_non_boolean_comptime_if_conditions() {
        let source = r#"
comptime if "yes" then
    print("bad")
end
"#;
        let err = compiler().compile_source(source).unwrap_err();
        assert!(format!("{err}").contains("comptime if condition must evaluate to a boolean."));
    }

    #[test]
    fn lowers_comptime_table_generation_and_freeze() {
        let source = r#"
comptime const MAIN_PROPERTIES = freeze {
    BasePart = {
        "CFrame",
        "Size",
        "Anchored",
    },
}

comptime function makeSet(list)
    local out = {}
    for _, value in list do
        out[value] = true
    end
    return freeze(out)
end

const BASEPART_MAIN_SET = comptime makeSet(MAIN_PROPERTIES.BasePart)
"#;
        let output = compiler().compile_source(source).unwrap();
        assert!(output.contains("local BASEPART_MAIN_SET = table.freeze({"));
        assert!(output.contains("CFrame = true"));
        assert!(output.contains("Size = true"));
        assert!(output.contains("Anchored = true"));
    }

    #[test]
    fn uses_roblox_suffix_defaults_for_file_exports() {
        let root = temp_project("roblox_rbxmx");
        write_file(
            &root,
            "xluau.config.json",
            r#"{
  "include": ["src/**/*.xl"],
  "target": "roblox"
}"#,
        );
        write_file(
            &root,
            "src/bootstrap.server.xl",
            "return script.Name .. \" < \" .. script.ClassName",
        );
        write_file(&root, "src/player.local.xl", "return script.Name");
        write_file(&root, "src/compat.legacy.xl", "return script.Name");
        write_file(&root, "src/shared.xl", "return script.Name");

        let compiler = Compiler::discover(&root).unwrap();
        let artifact = compiler
            .build_file(&root.join("src/bootstrap.server.xl"))
            .unwrap();
        let rbxmx_path = artifact.rbxmx_output.clone().expect("rbxmx path");
        let rbxmx = artifact.rbxmx.clone().expect("rbxmx contents");
        assert!(
            artifact
                .output
                .ends_with(Path::new("out/bootstrap.server.luau"))
        );
        assert!(rbxmx_path.ends_with(Path::new("out/bootstrap.server.rbxmx")));
        assert!(rbxmx.contains(r#"<Item class="Script" referent="RBX0">"#));
        assert!(rbxmx.contains(r#"<string name="Name">bootstrap</string>"#));
        assert!(rbxmx.contains(r#"<token name="RunContext">1</token>"#));
        assert!(rbxmx.contains("return script.Name .. &quot; &lt; &quot; .. script.ClassName"));

        compiler.write_artifact(&artifact).unwrap();
        assert!(artifact.output.is_file());
        assert!(rbxmx_path.is_file());

        let local_script = compiler
            .build_file(&root.join("src/player.local.xl"))
            .unwrap()
            .rbxmx
            .expect("local script rbxmx");
        assert!(local_script.contains(r#"<Item class="LocalScript" referent="RBX0">"#));
        assert!(local_script.contains(r#"<string name="Name">player</string>"#));
        assert!(!local_script.contains("RunContext"));

        let legacy_script = compiler
            .build_file(&root.join("src/compat.legacy.xl"))
            .unwrap()
            .rbxmx
            .expect("legacy script rbxmx");
        assert!(legacy_script.contains(r#"<Item class="Script" referent="RBX0">"#));
        assert!(legacy_script.contains(r#"<string name="Name">compat</string>"#));
        assert!(legacy_script.contains(r#"<token name="RunContext">0</token>"#));

        let module_script = compiler
            .build_file(&root.join("src/shared.xl"))
            .unwrap()
            .rbxmx
            .expect("module script rbxmx");
        assert!(module_script.contains(r#"<Item class="ModuleScript" referent="RBX0">"#));
        assert!(module_script.contains(r#"<string name="Name">shared</string>"#));
    }

    #[test]
    fn uses_parent_folder_name_for_roblox_init_modules() {
        let root = temp_project("roblox_rbxmx_init");
        write_file(
            &root,
            "xluau.config.json",
            r#"{
  "include": ["src/**/*.xl"],
  "target": "roblox"
}"#,
        );
        write_file(&root, "src/shared/init.server.xl", "return 1");

        let compiler = Compiler::discover(&root).unwrap();
        let artifact = compiler
            .build_file(&root.join("src/shared/init.server.xl"))
            .unwrap();
        let rbxmx = artifact.rbxmx.expect("rbxmx contents");
        assert!(rbxmx.contains(r#"<Item class="Script" referent="RBX0">"#));
        assert!(rbxmx.contains(r#"<string name="Name">shared</string>"#));
        assert!(rbxmx.contains(r#"<token name="RunContext">1</token>"#));
    }

    #[test]
    fn supports_custom_roblox_suffix_config() {
        let root = temp_project("roblox_rbxmx_suffixes");
        write_file(
            &root,
            "xluau.config.json",
            r#"{
  "include": ["src/**/*.xl"],
  "target": "roblox",
  "robloxOutput": {
    "suffixes": {
      "server": ".srv",
      "legacy": ".old",
      "client": ".cli",
      "local": ".loc"
    }
  }
}"#,
        );
        write_file(&root, "src/game.srv.xl", "return 1");

        let compiler = Compiler::discover(&root).unwrap();
        let artifact = compiler.build_file(&root.join("src/game.srv.xl")).unwrap();
        let rbxmx = artifact.rbxmx.expect("rbxmx contents");
        assert!(rbxmx.contains(r#"<Item class="Script" referent="RBX0">"#));
        assert!(rbxmx.contains(r#"<string name="Name">game</string>"#));
        assert!(rbxmx.contains(r#"<token name="RunContext">1</token>"#));
    }

    #[test]
    fn sanitizes_roblox_names_for_exports() {
        let root = temp_project("roblox_rbxmx_charsafe");
        write_file(
            &root,
            "xluau.config.json",
            r#"{
  "include": ["src/**/*.xl"],
  "target": "roblox",
  "robloxOutput": {
    "projectRbxmx": {
      "enabled": true
    }
  }
}"#,
        );
        write_file(
            &root,
            "src/bridge-api/localhost-client.local.xl",
            "return 1",
        );

        let compiler = Compiler::discover(&root).unwrap();
        let artifact = compiler
            .build_file(&root.join("src/bridge-api/localhost-client.local.xl"))
            .unwrap();
        let rbxmx = artifact.rbxmx.expect("rbxmx contents");
        let rbxmx_path = artifact.rbxmx_output.expect("rbxmx output path");
        assert!(
            artifact
                .output
                .ends_with(Path::new("out/bridge_api/localhost_client.local.luau"))
        );
        assert!(
            rbxmx_path.ends_with(Path::new("out/bridge_api/localhost_client.local.rbxmx"))
        );
        assert!(rbxmx.contains(r#"<string name="Name">localhost_client</string>"#));

        let project = compiler
            .build_project_rbxmx(&compiler.build_project().unwrap())
            .unwrap()
            .expect("project rbxmx");
        let expected_project_name = format!(
            "{}.rbxmx",
            sanitize_roblox_identifier(
                root.file_name()
                    .and_then(|value| value.to_str())
                    .expect("temp project name"),
            )
        );
        assert!(project.output.ends_with(Path::new("out").join(expected_project_name)));
        assert!(
            project
                .rbxmx
                .contains(r#"<string name="Name">bridge_api</string>"#)
        );
        assert!(
            project
                .rbxmx
                .contains(r#"<string name="Name">localhost_client</string>"#)
        );
    }

    #[test]
    fn builds_project_rbxmx_with_init_folders_and_custom_path() {
        let root = temp_project("roblox_project_rbxmx");
        write_file(
            &root,
            "xluau.config.json",
            r#"{
  "include": ["src/**/*.xl"],
  "target": "roblox",
  "robloxOutput": {
    "projectRbxmx": {
      "enabled": true,
      "path": "generated/game.rbxmx",
      "rootName": "GameRoot",
      "rootClassName": "Folder"
    }
  }
}"#,
        );
        write_file(&root, "src/Server/init.server.xl", "return 1");
        write_file(&root, "src/Server/util.xl", "return 2");
        write_file(&root, "src/Player/controller.local.xl", "return 3");
        write_file(&root, "src/Shared/math.xl", "return 4");

        let compiler = Compiler::discover(&root).unwrap();
        let artifacts = compiler.build_project().unwrap();
        let project = compiler
            .build_project_rbxmx(&artifacts)
            .unwrap()
            .expect("project rbxmx");

        assert!(project.output.ends_with(Path::new("generated/game.rbxmx")));
        assert!(
            project
                .rbxmx
                .contains(r#"<Item class="Folder" referent="RBX0">"#)
        );
        assert!(
            project
                .rbxmx
                .contains(r#"<string name="Name">GameRoot</string>"#)
        );
        assert!(
            project
                .rbxmx
                .contains(r#"<Item class="Script" referent="RBX"#)
        );
        assert!(
            project
                .rbxmx
                .contains(r#"<string name="Name">Server</string>"#)
        );
        assert!(
            project
                .rbxmx
                .contains(r#"<token name="RunContext">1</token>"#)
        );
        assert!(
            project
                .rbxmx
                .contains(r#"<Item class="ModuleScript" referent="RBX"#)
        );
        assert!(
            project
                .rbxmx
                .contains(r#"<string name="Name">util</string>"#)
        );
        assert!(
            project
                .rbxmx
                .contains(r#"<Item class="LocalScript" referent="RBX"#)
        );
        assert!(
            project
                .rbxmx
                .contains(r#"<string name="Name">controller</string>"#)
        );

        let server_index = project
            .rbxmx
            .find(r#"<string name="Name">Server</string>"#)
            .unwrap();
        let util_index = project
            .rbxmx
            .find(r#"<string name="Name">util</string>"#)
            .unwrap();
        assert!(server_index < util_index);
        let server_window_start = server_index.saturating_sub(120);
        let server_window = &project.rbxmx[server_window_start..server_index];
        assert!(server_window.contains(r#"class="Script""#));
        assert!(!server_window.contains(r#"class="Folder""#));

        compiler.write_project_rbxmx(&project).unwrap();
        assert!(project.output.is_file());
    }

    #[test]
    fn strips_base_dir_from_emitted_output_paths() {
        let root = temp_project("base_dir_output_paths");
        write_file(
            &root,
            "xluau.config.json",
            r#"{
  "include": ["src/**/*.xl"],
  "outDir": "generated",
  "baseDir": "src"
}"#,
        );
        write_file(&root, "src/bridge/messages.xl", "return 1");

        let compiler = Compiler::discover(&root).unwrap();
        let artifact = compiler
            .build_file(&root.join("src/bridge/messages.xl"))
            .unwrap();

        assert!(
            artifact
                .output
                .ends_with(Path::new("generated/bridge/messages.luau"))
        );
    }
}
