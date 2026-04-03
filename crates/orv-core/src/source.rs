use std::path::{Path, PathBuf};

use orv_diagnostics::{Diagnostic, DiagnosticBag};
use orv_span::{FileId, SourceMap};

/// Loads source files from the filesystem into a `SourceMap`.
pub struct SourceLoader {
    source_map: SourceMap,
    diagnostics: DiagnosticBag,
    root: PathBuf,
}

impl SourceLoader {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            source_map: SourceMap::default(),
            diagnostics: DiagnosticBag::new(),
            root: root.into(),
        }
    }

    /// Load a file relative to the project root.
    pub fn load(&mut self, relative_path: &str) -> Option<FileId> {
        let full_path = self.root.join(relative_path);
        self.load_absolute(&full_path, relative_path)
    }

    /// Load a file from an absolute path, using the given display name.
    pub fn load_absolute(&mut self, path: &Path, display_name: &str) -> Option<FileId> {
        match std::fs::read_to_string(path) {
            Ok(source) => {
                let id = self.source_map.add(display_name, source);
                Some(id)
            }
            Err(e) => {
                self.diagnostics.push(Diagnostic::error(format!(
                    "could not read `{display_name}`: {e}"
                )));
                None
            }
        }
    }

    /// Load source from a string (for testing or REPL).
    pub fn load_string(&mut self, name: &str, source: &str) -> FileId {
        self.source_map.add(name, source)
    }

    pub const fn source_map(&self) -> &SourceMap {
        &self.source_map
    }

    pub fn into_parts(self) -> (SourceMap, DiagnosticBag) {
        (self.source_map, self.diagnostics)
    }

    pub fn has_errors(&self) -> bool {
        self.diagnostics.has_errors()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn load_string_works() {
        let mut loader = SourceLoader::new(".");
        let id = loader.load_string("test.orv", "let x = 1");
        assert_eq!(loader.source_map().source(id), "let x = 1");
        assert_eq!(loader.source_map().name(id), "test.orv");
        assert!(!loader.has_errors());
    }

    #[test]
    fn load_missing_file_produces_diagnostic() {
        let mut loader = SourceLoader::new("/nonexistent");
        let result = loader.load("missing.orv");
        assert!(result.is_none());
        assert!(loader.has_errors());
    }

    #[test]
    fn load_real_file() {
        let dir = std::env::temp_dir().join("orv-test-loader");
        std::fs::create_dir_all(&dir).unwrap();
        let file_path = dir.join("hello.orv");
        let mut f = std::fs::File::create(&file_path).unwrap();
        write!(f, "@io.out \"hello\"").unwrap();

        let mut loader = SourceLoader::new(&dir);
        let id = loader.load("hello.orv");
        assert!(id.is_some());
        assert_eq!(loader.source_map().source(id.unwrap()), "@io.out \"hello\"");

        std::fs::remove_dir_all(&dir).ok();
    }
}
