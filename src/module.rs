use std::{
    collections::HashMap,
    fs,
    path::{Component, Path, PathBuf},
};

use crate::{
    compiler::{CompilerError, Result},
    config::XluauConfig,
    lexer::{Lexer, Symbol, TokenKind},
};

#[derive(Debug, Clone)]
pub struct ModuleResolver {
    root: PathBuf,
    config: XluauConfig,
}

#[derive(Debug, Clone)]
pub struct ResolvedModule {
    pub source_path: PathBuf,
    pub logical_path: PathBuf,
    pub emitted_require: String,
    pub is_external: bool,
}

#[derive(Debug, Clone)]
pub struct RequireDependency {
    pub specifier: String,
    pub resolved: ResolvedModule,
}

#[derive(Debug, Clone)]
struct RequireCall {
    call_start: usize,
    call_end: usize,
    specifier: String,
}

impl ModuleResolver {
    pub fn new(root: impl Into<PathBuf>, config: XluauConfig) -> Self {
        Self {
            root: root.into(),
            config,
        }
    }

    pub fn rewrite_requires(&self, source: &str, current_path: &Path) -> Result<String> {
        let calls = self.find_require_calls(source)?;
        if calls.is_empty() {
            return Ok(source.to_string());
        }

        let mut rewritten = source.to_string();
        for call in calls.into_iter().rev() {
            let Some(resolved) = self.resolve_static_require(current_path, &call.specifier)? else {
                continue;
            };

            if resolved.is_external {
                rewritten.replace_range(call.call_start..call.call_end, &resolved.emitted_require);
                continue;
            }

            match self.config.target.as_str() {
                "filesystem" => {
                    let replacement =
                        format!("require({})", quote_string(&resolved.emitted_require));
                    rewritten.replace_range(call.call_start..call.call_end, &replacement);
                }
                "roblox" | "custom" => {
                    let replacement = format!("require({})", resolved.emitted_require);
                    rewritten.replace_range(call.call_start..call.call_end, &replacement);
                }
                other => {
                    return Err(CompilerError::Other(format!(
                        "unsupported target `{other}`"
                    )));
                }
            }
        }

        Ok(rewritten)
    }

    pub fn collect_dependencies(
        &self,
        source: &str,
        current_path: &Path,
    ) -> Result<Vec<RequireDependency>> {
        let mut dependencies = Vec::new();
        for call in self.find_require_calls(source)? {
            if let Some(resolved) = self.resolve_static_require(current_path, &call.specifier)? {
                if resolved.is_external {
                    continue;
                }
                dependencies.push(RequireDependency {
                    specifier: call.specifier,
                    resolved,
                });
            }
        }
        Ok(dependencies)
    }

    pub fn resolve_require_path(
        &self,
        current_path: &Path,
        specifier: &str,
    ) -> Result<Option<ResolvedModule>> {
        self.resolve_static_require(current_path, specifier)
    }

    fn find_require_calls(&self, source: &str) -> Result<Vec<RequireCall>> {
        let tokens = Lexer::new(source).tokenize()?;
        let mut calls = Vec::new();

        let mut index = 0usize;
        while index < tokens.len() {
            let token = &tokens[index];
            if token.kind == TokenKind::Identifier
                && token.lexeme == "require"
                && !self.is_member_like(&tokens, index)
            {
                if let Some(string_token) = tokens.get(index + 1)
                    && string_token.kind == TokenKind::String
                {
                    calls.push(RequireCall {
                        call_start: token.span.start,
                        call_end: string_token.span.end,
                        specifier: decode_string_literal(&string_token.lexeme)?,
                    });
                    index += 2;
                    continue;
                }

                if matches!(
                    tokens.get(index + 1).map(|token| &token.kind),
                    Some(TokenKind::Symbol(Symbol::LParen))
                ) && matches!(
                    tokens.get(index + 2).map(|token| &token.kind),
                    Some(TokenKind::String)
                ) && matches!(
                    tokens.get(index + 3).map(|token| &token.kind),
                    Some(TokenKind::Symbol(Symbol::RParen))
                ) {
                    let string_token = &tokens[index + 2];
                    let end_token = &tokens[index + 3];
                    calls.push(RequireCall {
                        call_start: token.span.start,
                        call_end: end_token.span.end,
                        specifier: decode_string_literal(&string_token.lexeme)?,
                    });
                    index += 4;
                    continue;
                }
            }

            index += 1;
        }

        Ok(calls)
    }

    fn is_member_like(&self, tokens: &[crate::lexer::Token], index: usize) -> bool {
        matches!(
            index
                .checked_sub(1)
                .and_then(|prev| tokens.get(prev).map(|token| &token.kind)),
            Some(TokenKind::Symbol(Symbol::Dot) | TokenKind::Symbol(Symbol::Colon))
        )
    }

    fn resolve_static_require(
        &self,
        current_path: &Path,
        specifier: &str,
    ) -> Result<Option<ResolvedModule>> {
        if let Some(package_alias) = self.resolve_package_alias(specifier)? {
            return Ok(Some(ResolvedModule {
                source_path: self.root.join(&self.config.bundle_file),
                logical_path: PathBuf::from(&self.config.bundle_file),
                emitted_require: format!(
                    "require({}).{}",
                    quote_string(&self.config.bundle_path),
                    sanitize_identifier(&package_alias)
                ),
                is_external: true,
            }));
        }

        if specifier.starts_with('.') {
            let Some(source_path) =
                self.try_resolve_relative_source_path(current_path, specifier)?
            else {
                return Ok(None);
            };
            let logical_path = self.logical_module_path(&source_path)?;
            return Ok(Some(ResolvedModule {
                source_path,
                logical_path,
                emitted_require: specifier.to_string(),
                is_external: false,
            }));
        }

        let Some(base_candidate) = self.resolve_specifier_base(current_path, specifier)? else {
            return Ok(None);
        };
        let source_path = self.resolve_source_path(&base_candidate)?;
        let logical_path = self.logical_module_path(&source_path)?;
        let emitted_require = self.emit_target_path(current_path, &logical_path)?;

        Ok(Some(ResolvedModule {
            source_path,
            logical_path,
            emitted_require,
            is_external: false,
        }))
    }

    fn resolve_specifier_base(
        &self,
        current_path: &Path,
        specifier: &str,
    ) -> Result<Option<PathBuf>> {
        if specifier.starts_with('.') {
            return self.try_resolve_relative_source_path(current_path, specifier);
        }
        if specifier.starts_with('@') {
            return self.resolve_alias_base(specifier).map(Some);
        }

        Ok(None)
    }

    fn resolve_package_alias(&self, specifier: &str) -> Result<Option<String>> {
        let Some(alias) = specifier.strip_prefix('@') else {
            return Ok(None);
        };
        if alias.contains('/') {
            return Ok(None);
        }
        if self.config.paths.contains_key(&format!("@{alias}"))
            || self.config.paths.contains_key(&format!("@{alias}/*"))
        {
            return Err(CompilerError::Other(format!(
                "package alias `@{alias}` conflicts with a path alias"
            )));
        }
        Ok(self
            .config
            .packages
            .contains_key(alias)
            .then(|| alias.to_string()))
    }

    fn resolve_alias_base(&self, specifier: &str) -> Result<PathBuf> {
        let aliases = self
            .config
            .paths
            .iter()
            .collect::<Vec<(&String, &String)>>();

        let mut wildcard_match: Option<(usize, PathBuf)> = None;
        let mut direct_match: Option<(usize, PathBuf)> = None;

        for (alias, target) in aliases {
            if let Some(prefix) = alias.strip_suffix("/*") {
                if let Some(suffix) = specifier.strip_prefix(prefix) {
                    let suffix = suffix.strip_prefix('/').unwrap_or(suffix);
                    let mapped = target.strip_suffix("/*").unwrap_or(target);
                    let candidate = self.root.join(mapped).join(suffix);
                    let score = prefix.len();
                    if wildcard_match
                        .as_ref()
                        .map(|(best, _)| score > *best)
                        .unwrap_or(true)
                    {
                        wildcard_match = Some((score, candidate));
                    }
                }
                continue;
            }

            if specifier == alias || specifier.starts_with(&format!("{alias}/")) {
                let suffix = specifier
                    .strip_prefix(alias)
                    .unwrap_or_default()
                    .strip_prefix('/')
                    .unwrap_or_default();
                let candidate = self.root.join(target).join(suffix);
                let score = alias.len();
                if direct_match
                    .as_ref()
                    .map(|(best, _)| score > *best)
                    .unwrap_or(true)
                {
                    direct_match = Some((score, candidate));
                }
            }
        }

        direct_match
            .or(wildcard_match)
            .map(|(_, path)| path)
            .ok_or_else(|| CompilerError::Other(format!("unknown require alias `{specifier}`")))
    }

    fn try_resolve_relative_source_path(
        &self,
        current_path: &Path,
        specifier: &str,
    ) -> Result<Option<PathBuf>> {
        let base = current_path.parent().unwrap_or(&self.root).join(specifier);
        match self.resolve_source_path(&base) {
            Ok(path) => Ok(Some(path)),
            Err(CompilerError::Other(message))
                if message.contains("unable to resolve require target") =>
            {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    fn resolve_source_path(&self, candidate: &Path) -> Result<PathBuf> {
        if candidate.is_file() {
            return Ok(normalize_path(candidate));
        }

        if candidate.extension().is_none() {
            for extension in &self.config.extensions {
                let ext = extension.trim_start_matches('.');
                let file = candidate.with_extension(ext);
                if file.is_file() {
                    return Ok(normalize_path(&file));
                }
            }
        }

        if candidate.is_dir() {
            for index in &self.config.index_files {
                for extension in &self.config.extensions {
                    let ext = extension.trim_start_matches('.');
                    let file = candidate.join(index).with_extension(ext);
                    if file.is_file() {
                        return Ok(normalize_path(&file));
                    }
                }
            }
        }

        Err(CompilerError::Other(format!(
            "unable to resolve require target `{}`",
            candidate.display()
        )))
    }

    fn logical_module_path(&self, source_path: &Path) -> Result<PathBuf> {
        let relative = source_path.strip_prefix(&self.root).map_err(|_| {
            CompilerError::Other(format!(
                "resolved module path {} is outside project root {}",
                source_path.display(),
                self.root.display()
            ))
        })?;

        let file_stem = source_path
            .file_stem()
            .and_then(|value| value.to_str())
            .ok_or_else(|| {
                CompilerError::Other(format!("invalid module filename {}", source_path.display()))
            })?;

        if self
            .config
            .index_files
            .iter()
            .any(|index| index == file_stem)
        {
            Ok(relative
                .parent()
                .unwrap_or_else(|| Path::new(""))
                .to_path_buf())
        } else {
            Ok(relative.with_extension(""))
        }
    }

    fn emit_target_path(&self, current_path: &Path, logical_path: &Path) -> Result<String> {
        match self.config.target.as_str() {
            "filesystem" => Ok(format!("./{}", path_to_luau(logical_path))),
            "roblox" => self.emit_roblox_path(current_path, logical_path),
            "custom" => {
                let Some(function_name) = &self.config.custom_target_function else {
                    return Err(CompilerError::Other(
                        "target `custom` requires `customTargetFunction` in xluau.config.json"
                            .to_string(),
                    ));
                };
                let module_id = self.module_id(logical_path)?;
                Ok(format!(
                    "{}(\"{}\")",
                    function_name,
                    path_to_luau(&module_id)
                ))
            }
            other => Err(CompilerError::Other(format!(
                "unsupported target `{other}`"
            ))),
        }
    }

    fn emit_roblox_path(&self, current_path: &Path, logical_path: &Path) -> Result<String> {
        let current = self.absolute(current_path);
        let current_relative = current
            .strip_prefix(self.root.join(&self.config.base_dir))
            .or_else(|_| current.strip_prefix(&self.root))
            .map_err(|_| {
                CompilerError::Other(format!(
                    "cannot compute roblox require path for {}",
                    current.display()
                ))
            })?;

        let parent_depth = current_relative
            .parent()
            .map(path_component_count)
            .unwrap_or(0)
            + 1;
        let module_id = self.module_id(logical_path)?;

        let mut expr = "script".to_string();
        for _ in 0..parent_depth {
            expr.push_str(".Parent");
        }

        for segment in module_id.components().filter_map(component_to_identifier) {
            expr.push('.');
            expr.push_str(segment);
        }

        Ok(expr)
    }

    fn module_id(&self, logical_path: &Path) -> Result<PathBuf> {
        let base_dir = normalize_path(&self.config.base_dir);
        if base_dir.as_os_str().is_empty() {
            return Ok(logical_path.to_path_buf());
        }

        logical_path
            .strip_prefix(&base_dir)
            .map(Path::to_path_buf)
            .map_err(|_| {
                CompilerError::Other(format!(
                    "resolved module path {} is outside baseDir {}",
                    logical_path.display(),
                    self.config.base_dir.display()
                ))
            })
    }

    fn absolute(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            normalize_path(path)
        } else {
            normalize_path(&self.root.join(path))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VisitState {
    Visiting,
    Visited,
}

pub fn detect_circular_dependencies(
    resolver: &ModuleResolver,
    entry_points: &[PathBuf],
) -> Result<()> {
    let mut states = HashMap::<PathBuf, VisitState>::new();
    let mut stack = Vec::<PathBuf>::new();

    for entry in entry_points {
        visit_dependency(resolver, &normalize_path(entry), &mut states, &mut stack)?;
    }

    Ok(())
}

fn visit_dependency(
    resolver: &ModuleResolver,
    path: &Path,
    states: &mut HashMap<PathBuf, VisitState>,
    stack: &mut Vec<PathBuf>,
) -> Result<()> {
    let path = normalize_path(path);
    if let Some(state) = states.get(&path) {
        if *state == VisitState::Visited {
            return Ok(());
        }

        if *state == VisitState::Visiting {
            let start = stack
                .iter()
                .position(|candidate| *candidate == path)
                .unwrap_or(0);
            let mut cycle = stack[start..].to_vec();
            cycle.push(path.clone());
            return Err(CompilerError::Other(format!(
                "Circular dependency detected\n  {}",
                cycle
                    .iter()
                    .map(|node| display_path(&resolver.root, node))
                    .collect::<Vec<_>>()
                    .join(" -> ")
            )));
        }
    }

    states.insert(path.clone(), VisitState::Visiting);
    stack.push(path.clone());

    let source = fs::read_to_string(&path).map_err(|source| CompilerError::Io {
        path: path.clone(),
        source,
    })?;

    for dependency in resolver.collect_dependencies(&source, &path)? {
        visit_dependency(resolver, &dependency.resolved.source_path, states, stack)?;
    }

    stack.pop();
    states.insert(path, VisitState::Visited);
    Ok(())
}

fn decode_string_literal(text: &str) -> Result<String> {
    if text.len() >= 2 {
        let quote = text.chars().next().unwrap_or_default();
        let end = text.chars().last().unwrap_or_default();
        if (quote == '"' || quote == '\'') && quote == end {
            return Ok(text[1..text.len() - 1].to_string());
        }
    }

    Err(CompilerError::Other(format!(
        "unsupported require string literal `{text}`"
    )))
}

fn quote_string(text: &str) -> String {
    let escaped = text.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
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
    output
}

fn path_to_luau(path: &Path) -> String {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => parts.push(value.to_string_lossy().to_string()),
            Component::CurDir => parts.push(".".to_string()),
            Component::ParentDir => parts.push("..".to_string()),
            _ => {}
        }
    }
    parts.join("/")
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn path_component_count(path: &Path) -> usize {
    path.components()
        .filter(|component| matches!(component, Component::Normal(_)))
        .count()
}

fn component_to_identifier(component: std::path::Component<'_>) -> Option<&str> {
    match component {
        Component::Normal(value) => value.to_str(),
        _ => None,
    }
}

fn display_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}
