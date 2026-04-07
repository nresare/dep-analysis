// SPDX-License-Identifier: MIT

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use proc_macro2::{Ident, Span};
use serde::{Deserialize, Serialize};
use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{
    Attribute, ExprPath, File, ItemExternCrate, ItemMod, ItemUse, LitStr, Macro, Path as SynPath,
    QSelf, TypePath, UseTree,
};

pub(crate) fn internal_dependencies(
    usages: &[Usage],
    module_paths: &BTreeSet<Vec<String>>,
) -> Vec<InternalDependency> {
    let mut dependencies = BTreeMap::<(PathBuf, String, String), usize>::new();
    for usage in usages {
        let Some(to_module_path) = resolve_internal_module(usage, &module_paths) else {
            continue;
        };
        if usage.module_path == to_module_path {
            continue;
        }

        let from_module = module_path_to_string(&usage.module_path);
        let to_module = module_path_to_string(&to_module_path);
        dependencies
            .entry((usage.file.clone(), from_module, to_module))
            .and_modify(|line| *line = (*line).min(usage.line))
            .or_insert(usage.line);
    }

    let mut dependencies = dependencies
        .into_iter()
        .map(
            |((file, from_module, to_module), line)| InternalDependency {
                file,
                line,
                from_module,
                to_module,
            },
        )
        .collect::<Vec<_>>();

    dependencies.sort_by(|left, right| {
        left.file
            .cmp(&right.file)
            .then_with(|| left.line.cmp(&right.line))
            .then_with(|| left.from_module.cmp(&right.from_module))
            .then_with(|| left.to_module.cmp(&right.to_module))
    });
    dependencies
}

fn resolve_internal_module(
    usage: &Usage,
    module_paths: &BTreeSet<Vec<String>>,
) -> Option<Vec<String>> {
    let path = usage_path_segments(usage);
    let absolute_path = absolute_module_candidate(&usage.module_path, &path, module_paths)?;

    (1..=absolute_path.len())
        .rev()
        .map(|len| absolute_path[..len].to_vec())
        .find(|candidate| module_paths.contains(candidate))
}

fn usage_path_segments(usage: &Usage) -> Vec<String> {
    usage
        .origin
        .split("::")
        .chain(usage.symbol.split("::"))
        .filter(|segment| !segment.is_empty() && *segment != "*")
        .map(ToOwned::to_owned)
        .collect()
}

fn absolute_module_candidate(
    from_module: &[String],
    path: &[String],
    module_paths: &BTreeSet<Vec<String>>,
) -> Option<Vec<String>> {
    let first = path.first()?;
    if first == "crate" {
        return Some(path.to_vec());
    }

    if first == "self" {
        let mut absolute = from_module.to_vec();
        absolute.extend(path.iter().skip(1).cloned());
        return Some(absolute);
    }

    if first == "super" {
        let mut absolute = from_module[..from_module.len().saturating_sub(1)].to_vec();
        absolute.extend(path.iter().skip(1).cloned());
        return Some(absolute);
    }

    let mut relative = from_module.to_vec();
    relative.extend(path.iter().cloned());
    if has_module_prefix(&relative, module_paths) {
        return Some(relative);
    }

    let mut crate_absolute = vec!["crate".to_owned()];
    crate_absolute.extend(path.iter().cloned());
    if has_module_prefix(&crate_absolute, module_paths) {
        return Some(crate_absolute);
    }

    None
}

fn has_module_prefix(path: &[String], module_paths: &BTreeSet<Vec<String>>) -> bool {
    (1..=path.len()).any(|len| module_paths.contains(&path[..len]))
}

fn module_path_to_string(module_path: &[String]) -> String {
    module_path.join("::")
}

#[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct InternalDependency {
    pub(crate) file: PathBuf,
    pub(crate) line: usize,
    pub(crate) from_module: String,
    pub(crate) to_module: String,
}

pub(crate) fn analyze_project(path: impl AsRef<Path>) -> Result<Analysis, String> {
    let mut analyzer = Analyzer::default();
    analyzer.analyze_file(path.as_ref(), vec!["crate".to_owned()])?;
    Ok(analyzer.into_analysis())
}

#[cfg(test)]
pub(crate) fn dependency_usages_for_file(path: impl AsRef<Path>) -> Result<Vec<Usage>, String> {
    Ok(analyze_project(path)?.usages)
}

pub(crate) struct Analysis {
    pub(crate) usages: Vec<Usage>,
    pub(crate) module_paths: BTreeSet<Vec<String>>,
}

#[cfg(test)]
pub(crate) fn dependency_usages(source: &str) -> syn::Result<Vec<FileLocalUsage>> {
    let file = syn::parse_file(source)?;
    Ok(usages_from_file(&file))
}

#[cfg(test)]
fn usages_from_file(file: &File) -> Vec<FileLocalUsage> {
    file_analysis_from_file(file).usages
}

fn file_analysis_from_file(file: &File) -> FileAnalysis {
    let mut visitor = DependencyVisitor::default();
    visitor.visit_file(file);
    visitor.into_analysis()
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct Usage {
    pub(crate) file: PathBuf,
    pub(crate) module_path: Vec<String>,
    pub(crate) line: usize,
    pub(crate) origin: String,
    pub(crate) symbol: String,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct FileLocalUsage {
    pub(crate) module_path_suffix: Vec<String>,
    pub(crate) line: usize,
    pub(crate) origin: String,
    pub(crate) symbol: String,
}

struct FileAnalysis {
    usages: Vec<FileLocalUsage>,
    inline_module_paths: Vec<Vec<String>>,
}

#[derive(Default)]
struct Analyzer {
    seen_files: HashSet<PathBuf>,
    pub(crate) module_paths: BTreeSet<Vec<String>>,
    usages: Vec<Usage>,
}

impl Analyzer {
    fn analyze_file(&mut self, path: &Path, module_path: Vec<String>) -> Result<(), String> {
        let path = normalize_path(path);
        if !self.seen_files.insert(path.clone()) {
            return Ok(());
        }
        self.module_paths.insert(module_path.clone());

        let source = fs::read_to_string(&path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        let file = syn::parse_file(&source)
            .map_err(|error| format!("failed to parse {}: {error}", path.display()))?;

        let file_analysis = file_analysis_from_file(&file);
        self.module_paths.extend(
            file_analysis
                .inline_module_paths
                .into_iter()
                .map(|inline_path| module_path.iter().cloned().chain(inline_path).collect()),
        );
        self.usages
            .extend(file_analysis.usages.into_iter().map(|usage| {
                Usage {
                    file: path.clone(),
                    module_path: module_path
                        .iter()
                        .cloned()
                        .chain(usage.module_path_suffix)
                        .collect(),
                    line: usage.line,
                    origin: usage.origin,
                    symbol: usage.symbol,
                }
            }));

        let module_dir = module_dir_for_child_modules(&path);
        for module in module_files_referenced_by(&file, &module_dir, &module_path) {
            self.analyze_file(&module.file, module.module_path)?;
        }

        Ok(())
    }

    fn into_analysis(self) -> Analysis {
        Analysis {
            usages: self.usages,
            module_paths: self.module_paths,
        }
    }
}

#[derive(Default)]
struct DependencyVisitor {
    usages: BTreeMap<(Vec<String>, String, String), usize>,
    pub(crate) module_path: Vec<String>,
    inline_module_paths: Vec<Vec<String>>,
}

impl DependencyVisitor {
    fn add_usage(&mut self, usage: Option<FileLocalUsage>) {
        let Some(mut usage) = usage else {
            return;
        };
        usage.module_path_suffix = self.module_path.clone();

        self.usages
            .entry((usage.module_path_suffix, usage.origin, usage.symbol))
            .and_modify(|line| *line = (*line).min(usage.line))
            .or_insert(usage.line);
    }

    fn add_use_tree(&mut self, tree: &UseTree, prefix: &mut Vec<String>) {
        match tree {
            UseTree::Path(path) => {
                prefix.push(ident_to_string(&path.ident));
                self.add_use_tree(&path.tree, prefix);
                prefix.pop();
            }
            UseTree::Name(name) => {
                prefix.push(ident_to_string(&name.ident));
                self.add_usage(usage_from_segments(
                    prefix.iter().cloned(),
                    span_line(name.ident.span()),
                    SymbolStyle::Plain,
                ));
                prefix.pop();
            }
            UseTree::Rename(rename) => {
                prefix.push(ident_to_string(&rename.ident));
                self.add_usage(usage_from_segments(
                    prefix.iter().cloned(),
                    span_line(rename.ident.span()),
                    SymbolStyle::Plain,
                ));
                prefix.pop();
            }
            UseTree::Glob(glob) => {
                self.add_usage(usage_from_segments(
                    prefix.iter().cloned().chain(["*".to_owned()]),
                    span_line(glob.star_token.span()),
                    SymbolStyle::Plain,
                ));
            }
            UseTree::Group(group) => {
                for item in &group.items {
                    self.add_use_tree(item, prefix);
                }
            }
        }
    }

    fn add_qualified_path(&mut self, path: &SynPath, line: usize, style: SymbolStyle) {
        if path.leading_colon.is_none() && path.segments.len() < 2 {
            return;
        }

        self.add_usage(usage_from_segments(
            path.segments
                .iter()
                .map(|segment| ident_to_string(&segment.ident)),
            line,
            style,
        ));
    }

    fn add_qself_trait_path(&mut self, path: &SynPath, qself: &QSelf, line: usize) {
        if qself.position == 0 {
            return;
        }

        self.add_usage(usage_from_segments(
            path.segments
                .iter()
                .take(qself.position)
                .map(|segment| ident_to_string(&segment.ident)),
            line,
            SymbolStyle::Plain,
        ));
    }

    fn into_analysis(self) -> FileAnalysis {
        let mut usages = self
            .usages
            .into_iter()
            .map(
                |((module_path_suffix, origin, symbol), line)| FileLocalUsage {
                    module_path_suffix,
                    line,
                    origin,
                    symbol,
                },
            )
            .collect::<Vec<_>>();

        usages.sort_by(|left, right| {
            left.line
                .cmp(&right.line)
                .then_with(|| left.origin.cmp(&right.origin))
                .then_with(|| left.symbol.cmp(&right.symbol))
        });
        FileAnalysis {
            usages,
            inline_module_paths: self.inline_module_paths,
        }
    }
}

impl<'ast> Visit<'ast> for DependencyVisitor {
    fn visit_attribute(&mut self, attribute: &'ast Attribute) {
        self.add_qualified_path(
            attribute.path(),
            span_line(attribute.path().span()),
            SymbolStyle::Plain,
        );
        visit::visit_attribute(self, attribute);
    }

    fn visit_expr_path(&mut self, expr_path: &'ast ExprPath) {
        if let Some(qself) = &expr_path.qself {
            self.add_qself_trait_path(&expr_path.path, qself, span_line(expr_path.path.span()));
        }
        self.add_qualified_path(
            &expr_path.path,
            span_line(expr_path.path.span()),
            SymbolStyle::Plain,
        );
        visit::visit_expr_path(self, expr_path);
    }

    fn visit_item_extern_crate(&mut self, item: &'ast ItemExternCrate) {
        self.add_usage(Some(FileLocalUsage {
            module_path_suffix: Vec::new(),
            line: span_line(item.ident.span()),
            origin: "extern crate".to_owned(),
            symbol: ident_to_string(&item.ident),
        }));
        visit::visit_item_extern_crate(self, item);
    }

    fn visit_item_use(&mut self, item: &'ast ItemUse) {
        self.add_use_tree(&item.tree, &mut Vec::new());
    }

    fn visit_item_mod(&mut self, item_mod: &'ast ItemMod) {
        let Some((_, items)) = &item_mod.content else {
            return;
        };

        self.module_path.push(ident_to_string(&item_mod.ident));
        self.inline_module_paths.push(self.module_path.clone());
        for item in items {
            self.visit_item(item);
        }
        self.module_path.pop();
    }

    fn visit_macro(&mut self, mac: &'ast Macro) {
        self.add_qualified_path(&mac.path, span_line(mac.path.span()), SymbolStyle::Macro);
        visit::visit_macro(self, mac);
    }

    fn visit_type_path(&mut self, type_path: &'ast TypePath) {
        if let Some(qself) = &type_path.qself {
            self.add_qself_trait_path(&type_path.path, qself, span_line(type_path.path.span()));
        }
        self.add_qualified_path(
            &type_path.path,
            span_line(type_path.path.span()),
            SymbolStyle::Plain,
        );
        visit::visit_type_path(self, type_path);
    }
}

#[derive(Clone, Copy)]
enum SymbolStyle {
    Plain,
    Macro,
}

fn usage_from_segments(
    segments: impl IntoIterator<Item = String>,
    line: usize,
    style: SymbolStyle,
) -> Option<FileLocalUsage> {
    let segments = segments.into_iter().collect::<Vec<_>>();
    let first = segments.first()?;

    if !is_reportable_origin_root(first) {
        return None;
    }

    let (origin, symbol) = if is_local_root(first) {
        if segments.len() < 3 {
            return None;
        }
        (segments[..2].join("::"), segments[2..].join("::"))
    } else {
        if segments.len() < 2 {
            return None;
        }
        (
            segments[..segments.len() - 1].join("::"),
            segments.last()?.to_owned(),
        )
    };

    if symbol == "*" {
        return Some(FileLocalUsage {
            module_path_suffix: Vec::new(),
            line,
            origin,
            symbol,
        });
    }

    if !symbol.split("::").all(is_identifier) {
        return None;
    }

    Some(FileLocalUsage {
        module_path_suffix: Vec::new(),
        line,
        origin,
        symbol: match style {
            SymbolStyle::Plain => symbol,
            SymbolStyle::Macro => format!("{symbol}!"),
        },
    })
}

fn module_files_referenced_by(
    file: &File,
    module_dir: &Path,
    module_path: &[String],
) -> Vec<ModuleFile> {
    let mut visitor = ModuleFileVisitor {
        module_dir: module_dir.to_owned(),
        module_path: module_path.to_vec(),
        module_files: Vec::new(),
    };
    visitor.visit_file(file);
    visitor.module_files
}

struct ModuleFile {
    file: PathBuf,
    pub(crate) module_path: Vec<String>,
}

struct ModuleFileVisitor {
    module_dir: PathBuf,
    pub(crate) module_path: Vec<String>,
    module_files: Vec<ModuleFile>,
}

impl<'ast> Visit<'ast> for ModuleFileVisitor {
    fn visit_item_mod(&mut self, item_mod: &'ast ItemMod) {
        let module_name = ident_to_string(&item_mod.ident);
        if let Some((_, items)) = &item_mod.content {
            let previous_dir = self.module_dir.clone();
            self.module_dir = self.module_dir.join(&module_name);
            self.module_path.push(module_name);
            for item in items {
                self.visit_item(item);
            }
            self.module_path.pop();
            self.module_dir = previous_dir;
            return;
        }

        if let Some(module_file) = module_file_for(item_mod, &self.module_dir) {
            let mut module_path = self.module_path.clone();
            module_path.push(module_name);
            self.module_files.push(ModuleFile {
                file: module_file,
                module_path,
            });
        }
    }
}

fn module_file_for(item_mod: &ItemMod, module_dir: &Path) -> Option<PathBuf> {
    if item_mod.content.is_some() {
        return None;
    }

    if let Some(path) = path_attribute(item_mod) {
        return Some(module_dir.join(path.value()));
    }

    let module_name = ident_to_string(&item_mod.ident);
    let flat = module_dir.join(format!("{module_name}.rs"));
    if flat.is_file() {
        return Some(flat);
    }

    let nested = module_dir.join(&module_name).join("mod.rs");
    if nested.is_file() {
        return Some(nested);
    }

    Some(flat)
}

fn path_attribute(item_mod: &ItemMod) -> Option<LitStr> {
    item_mod.attrs.iter().find_map(|attribute| {
        if !attribute.path().is_ident("path") {
            return None;
        }
        attribute.parse_args::<LitStr>().ok()
    })
}

fn module_dir_for_child_modules(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    match path.file_name().and_then(|name| name.to_str()) {
        Some("lib.rs" | "main.rs" | "mod.rs") => parent.to_owned(),
        _ => parent.join(path.file_stem().unwrap_or_default()),
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_owned())
}

fn ident_to_string(ident: &Ident) -> String {
    ident.to_string().trim_start_matches("r#").to_owned()
}

fn span_line(span: Span) -> usize {
    span.start().line
}

fn is_reportable_origin_root(candidate: &str) -> bool {
    is_identifier(candidate) && !is_builtin_or_prelude_root(candidate)
}

fn is_local_root(candidate: &str) -> bool {
    matches!(candidate, "self" | "super" | "crate")
}

fn is_builtin_or_prelude_root(candidate: &str) -> bool {
    matches!(
        candidate,
        "_" | "Self"
            | "std"
            | "core"
            | "alloc"
            | "bool"
            | "char"
            | "str"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "isize"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "usize"
            | "f32"
            | "f64"
            | "Option"
            | "Some"
            | "None"
            | "Result"
            | "Ok"
            | "Err"
            | "Vec"
            | "String"
            | "Box"
            | "ToString"
            | "From"
            | "Into"
            | "AsRef"
            | "AsMut"
            | "Default"
            | "Clone"
            | "Copy"
            | "Drop"
            | "Debug"
            | "Display"
            | "Iterator"
            | "IntoIterator"
            | "Send"
            | "Sync"
            | "Sized"
    )
}

fn is_identifier(candidate: &str) -> bool {
    let mut bytes = candidate.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };

    is_identifier_start(first) && bytes.all(is_identifier_continue)
}

fn is_identifier_start(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphabetic()
}

fn is_identifier_continue(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphanumeric()
}
