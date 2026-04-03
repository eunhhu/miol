use std::path::{Path, PathBuf};

use orv_analyzer::{Analysis, analyze};
use orv_core::source::SourceLoader;
use orv_diagnostics::DiagnosticBag;
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
}

pub fn load_path(path: &Path) -> Result<LoadedUnit, FrontendFailure> {
    let absolute = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let root = absolute.parent().unwrap_or_else(|| absolute.as_ref());
    let display_name = path.display().to_string();

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

#[cfg(test)]
mod tests {
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
}
