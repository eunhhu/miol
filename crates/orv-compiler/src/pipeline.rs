use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};

use orv_analyzer::{Analysis, analyze};
use orv_core::source::SourceLoader;
use orv_diagnostics::DiagnosticBag;
use orv_project::{ModuleDependency, ProjectGraph, WorkspaceGraph, build_project_graph};
use orv_span::{FileId, SourceMap, Spanned};
use orv_syntax::{ast, lexer::Lexer, parser, token::TokenKind};

#[derive(Debug)]
pub struct FrontendFailure {
    source_map: SourceMap,
    diagnostics: DiagnosticBag,
}

impl FrontendFailure {
    #[must_use]
    pub fn into_parts(self) -> (SourceMap, DiagnosticBag) {
        (self.source_map, self.diagnostics)
    }
}

pub struct LoadedUnit {
    loader: SourceLoader,
    file_id: FileId,
}

impl LoadedUnit {
    #[must_use]
    pub const fn file_id(&self) -> FileId {
        self.file_id
    }

    #[must_use]
    pub const fn source_map(&self) -> &SourceMap {
        self.loader.source_map()
    }

    #[must_use]
    pub fn source_name(&self) -> &str {
        self.source_map().name(self.file_id)
    }

    #[must_use]
    pub fn source(&self) -> &str {
        self.source_map().source(self.file_id)
    }

    #[must_use]
    pub fn tokenize(&self) -> (Vec<Spanned<TokenKind>>, DiagnosticBag) {
        Lexer::new(self.source(), self.file_id).tokenize()
    }

    pub fn parse(self) -> Result<ParsedUnit, FrontendFailure> {
        let (tokens, diagnostics) = self.tokenize();
        if diagnostics.has_errors() {
            return Err(self.into_failure(diagnostics));
        }

        let (module, diagnostics) = parser::parse(tokens);
        if diagnostics.has_errors() {
            return Err(self.into_failure(diagnostics));
        }

        Ok(ParsedUnit {
            loaded: self,
            module,
        })
    }

    fn into_failure(self, diagnostics: DiagnosticBag) -> FrontendFailure {
        let (source_map, loader_diagnostics) = self.loader.into_parts();
        let diagnostics = merge_diagnostics(loader_diagnostics, diagnostics);
        FrontendFailure {
            source_map,
            diagnostics,
        }
    }
}

pub struct ParsedUnit {
    loaded: LoadedUnit,
    module: ast::Module,
}

impl ParsedUnit {
    #[must_use]
    pub const fn file_id(&self) -> FileId {
        self.loaded.file_id()
    }

    #[must_use]
    pub const fn source_map(&self) -> &SourceMap {
        self.loaded.source_map()
    }

    #[must_use]
    pub const fn module(&self) -> &ast::Module {
        &self.module
    }

    #[must_use]
    pub fn dump_ast(&self) -> String {
        parser::dump_ast(&self.module)
    }

    pub fn analyze(self) -> Result<AnalyzedUnit, FrontendFailure> {
        let (analysis, diagnostics) = analyze(&self.module);
        if diagnostics.has_errors() {
            return Err(self.into_failure(diagnostics));
        }

        Ok(AnalyzedUnit {
            parsed: self,
            analysis,
        })
    }

    fn into_failure(self, diagnostics: DiagnosticBag) -> FrontendFailure {
        let (source_map, loader_diagnostics) = self.loaded.loader.into_parts();
        let diagnostics = merge_diagnostics(loader_diagnostics, diagnostics);
        FrontendFailure {
            source_map,
            diagnostics,
        }
    }
}

pub struct AnalyzedUnit {
    parsed: ParsedUnit,
    analysis: Analysis,
}

impl AnalyzedUnit {
    #[must_use]
    pub const fn file_id(&self) -> FileId {
        self.parsed.file_id()
    }

    #[must_use]
    pub const fn source_map(&self) -> &SourceMap {
        self.parsed.source_map()
    }

    #[must_use]
    pub const fn module(&self) -> &ast::Module {
        self.parsed.module()
    }

    #[must_use]
    pub const fn analysis(&self) -> &Analysis {
        &self.analysis
    }

    #[must_use]
    pub fn project_graph(&self) -> ProjectGraph {
        build_project_graph(self.source_map().name(self.file_id()), &self.analysis.hir)
    }
}

pub fn load_path(path: &Path) -> Result<LoadedUnit, FrontendFailure> {
    let absolute = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let root = absolute.parent().unwrap_or_else(|| absolute.as_ref());
    let display_name = path.display().to_string();

    load_path_with_root(&absolute, root, &display_name)
}

pub fn load_project_graph(path: &Path) -> Result<WorkspaceGraph, FrontendFailure> {
    let entry = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let root = entry
        .parent()
        .unwrap_or_else(|| entry.as_ref())
        .to_path_buf();
    let entry_display = display_name_for(&root, &entry);
    let mut queue = VecDeque::from([entry.clone()]);
    let mut visited = BTreeSet::new();
    let mut modules = Vec::new();
    let mut dependencies = Vec::new();

    while let Some(module_path) = queue.pop_front() {
        let canonical = std::fs::canonicalize(&module_path).unwrap_or_else(|_| module_path.clone());
        if !visited.insert(canonical.clone()) {
            continue;
        }

        let analyzed =
            load_path_with_root(&canonical, &root, &display_name_for(&root, &canonical))?
                .parse()?
                .analyze()?;
        let module_name = analyzed.source_map().name(analyzed.file_id()).to_owned();
        let imports = discover_import_modules(analyzed.module(), &root);
        for import in imports {
            dependencies.push(ModuleDependency {
                from: module_name.clone(),
                to: import.display_name.clone(),
            });
            queue.push_back(import.absolute_path);
        }
        modules.push(analyzed.project_graph());
    }

    Ok(WorkspaceGraph {
        entry: entry_display,
        modules,
        dependencies,
    })
}

fn load_path_with_root(
    absolute: &Path,
    root: &Path,
    display_name: &str,
) -> Result<LoadedUnit, FrontendFailure> {
    let mut loader = SourceLoader::new(root);

    if let Some(file_id) = loader.load_absolute(&absolute, &display_name) {
        Ok(LoadedUnit { loader, file_id })
    } else {
        let (source_map, diagnostics) = loader.into_parts();
        Err(FrontendFailure {
            source_map,
            diagnostics,
        })
    }
}

#[must_use]
pub fn load_string(name: &str, source: &str) -> LoadedUnit {
    let mut loader = SourceLoader::new(PathBuf::from("."));
    let file_id = loader.load_string(name, source);
    LoadedUnit { loader, file_id }
}

fn merge_diagnostics(mut initial: DiagnosticBag, next: DiagnosticBag) -> DiagnosticBag {
    for diagnostic in next.into_vec() {
        initial.push(diagnostic);
    }
    initial
}

#[derive(Debug, Clone)]
struct ImportModuleCandidate {
    absolute_path: PathBuf,
    display_name: String,
}

fn discover_import_modules(module: &ast::Module, root: &Path) -> Vec<ImportModuleCandidate> {
    let mut seen = BTreeSet::new();
    let mut imports = Vec::new();

    for item in &module.items {
        let ast::Item::Import(import) = item.node() else {
            continue;
        };

        let path_segments = import
            .path
            .iter()
            .map(|segment| segment.node().as_str())
            .collect::<Vec<_>>();
        if path_segments.is_empty() || path_segments[0].starts_with('@') {
            continue;
        }

        let names = import
            .names
            .iter()
            .map(|name| name.node().as_str())
            .collect::<Vec<_>>();

        if names.is_empty() {
            let candidates = if path_segments.len() > 1 {
                vec![
                    path_segments.clone(),
                    path_segments[..path_segments.len() - 1].to_vec(),
                ]
            } else {
                vec![path_segments.clone()]
            };

            for segments in candidates {
                if let Some(candidate) = resolve_module_file(root, &segments)
                    && seen.insert(candidate.display_name.clone())
                {
                    imports.push(candidate);
                    break;
                }
            }
            continue;
        }

        if names.len() == 1
            && names[0] == "*"
            && let Some(candidate) = resolve_module_file(root, &path_segments)
            && seen.insert(candidate.display_name.clone())
        {
            imports.push(candidate);
            continue;
        }

        if let Some(candidate) = resolve_module_file(root, &path_segments)
            && seen.insert(candidate.display_name.clone())
        {
            imports.push(candidate);
        }

        for name in names {
            let mut segments = path_segments.clone();
            segments.push(name);
            if let Some(candidate) = resolve_module_file(root, &segments)
                && seen.insert(candidate.display_name.clone())
            {
                imports.push(candidate);
            }
        }
    }

    imports
}

fn resolve_module_file(root: &Path, segments: &[&str]) -> Option<ImportModuleCandidate> {
    if segments.is_empty() {
        return None;
    }

    let mut absolute = root.to_path_buf();
    for segment in segments {
        absolute.push(segment);
    }
    absolute.set_extension("orv");
    if !absolute.is_file() {
        return None;
    }

    Some(ImportModuleCandidate {
        display_name: display_name_for(root, &absolute),
        absolute_path: absolute,
    })
}

fn display_name_for(root: &Path, absolute: &Path) -> String {
    absolute.strip_prefix(root).map_or_else(
        |_| absolute.display().to_string(),
        |relative| relative.display().to_string(),
    )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn parse_pipeline_succeeds_for_valid_program() {
        let parsed =
            match load_string("hello.orv", "function greet(name: string) -> name\n").parse() {
                Ok(parsed) => parsed,
                Err(_) => panic!("parse should succeed"),
            };

        assert_eq!(parsed.module().items.len(), 1);
        assert_eq!(parsed.source_map().name(parsed.file_id()), "hello.orv");
    }

    #[test]
    fn analyze_pipeline_succeeds_for_valid_program() {
        let parsed = match load_string("counter.orv", "let x = 1\nfunction foo() -> x\n").parse() {
            Ok(parsed) => parsed,
            Err(_) => panic!("parse should succeed"),
        };
        let analyzed = match parsed.analyze() {
            Ok(analyzed) => analyzed,
            Err(_) => panic!("analysis should succeed"),
        };

        assert_eq!(analyzed.analysis().symbols.len(), 2);
        assert_eq!(analyzed.analysis().scopes.len(), 2);
    }

    #[test]
    fn parse_failure_preserves_diagnostics() {
        let failure = match load_string("broken.orv", "function foo( -> void\n").parse() {
            Ok(_) => panic!("parse should fail"),
            Err(failure) => failure,
        };

        let (source_map, diagnostics) = failure.into_parts();
        assert_eq!(source_map.name(FileId::new(0)), "broken.orv");
        assert!(diagnostics.has_errors());
    }

    #[test]
    fn analysis_failure_preserves_source_map() {
        let parsed = match load_string("missing.orv", "function foo() -> missing\n").parse() {
            Ok(parsed) => parsed,
            Err(_) => panic!("parse should succeed"),
        };
        let failure = match parsed.analyze() {
            Ok(_) => panic!("analysis should fail"),
            Err(failure) => failure,
        };

        let (source_map, diagnostics) = failure.into_parts();
        assert_eq!(source_map.name(FileId::new(0)), "missing.orv");
        assert!(diagnostics.has_errors());
    }

    #[test]
    fn project_graph_recursively_loads_local_imports() {
        let root = std::env::temp_dir().join("orv-project-graph-recursive");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("components")).expect("components dir");
        fs::create_dir_all(root.join("libs")).expect("libs dir");
        fs::write(
            root.join("main.orv"),
            "import components.Button\nimport libs.counter\npub define Home() -> @html {\n  @body {\n    let sig count: i32 = 0\n    @Button \"ok\"\n  }\n}\n",
        )
        .expect("main file");
        fs::write(
            root.join("components/Button.orv"),
            "pub define Button(label: string) -> @button label\n",
        )
        .expect("button file");
        fs::write(
            root.join("libs/counter.orv"),
            "pub function counter(): i32 -> 1\n",
        )
        .expect("counter file");

        let graph = load_project_graph(&root.join("main.orv")).expect("project graph should load");
        assert_eq!(graph.entry, "main.orv");
        assert_eq!(graph.modules.len(), 3);
        assert!(
            graph
                .dependencies
                .iter()
                .any(|dependency| dependency.from == "main.orv"
                    && dependency.to == "components/Button.orv")
        );
        assert!(
            graph
                .dependencies
                .iter()
                .any(|dependency| dependency.from == "main.orv"
                    && dependency.to == "libs/counter.orv")
        );

        let _ = fs::remove_dir_all(&root);
    }
}
