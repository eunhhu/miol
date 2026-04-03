//! Golden-output fixture test harness.
//!
//! Convention:
//!   fixtures/ok/*.orv  — must load without diagnostics
//!   fixtures/err/*.orv — must produce at least one error diagnostic
//!
//! This harness validates that the source loader can read all fixtures
//! and that the ok/err classification holds. As the compiler grows,
//! this harness will expand to compare AST dumps and diagnostic snapshots.

use std::path::PathBuf;

fn fixtures_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("fixtures")
}

fn orv_files_in(dir: &std::path::Path) -> Vec<PathBuf> {
    if !dir.exists() {
        return Vec::new();
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(std::result::Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "orv"))
        .collect();
    files.sort();
    files
}

#[test]
fn ok_fixtures_load_without_errors() {
    let ok_dir = fixtures_root().join("ok");
    let files = orv_files_in(&ok_dir);
    assert!(!files.is_empty(), "no .orv files found in fixtures/ok/");

    for path in &files {
        let source = std::fs::read_to_string(path).unwrap_or_else(|e| {
            panic!("failed to read {}: {e}", path.display());
        });
        let name = path.file_name().unwrap().to_string_lossy();

        let mut loader = orv_core::source::SourceLoader::new(ok_dir.clone());
        let _id = loader.load_string(&name, &source);

        assert!(
            !loader.has_errors(),
            "fixture {} produced load errors",
            path.display()
        );
    }
}

#[test]
fn err_fixtures_are_readable() {
    let err_dir = fixtures_root().join("err");
    let files = orv_files_in(&err_dir);
    assert!(!files.is_empty(), "no .orv files found in fixtures/err/");

    for path in &files {
        let source = std::fs::read_to_string(path).unwrap_or_else(|e| {
            panic!("failed to read {}: {e}", path.display());
        });
        let name = path.file_name().unwrap().to_string_lossy();

        let mut loader = orv_core::source::SourceLoader::new(err_dir.clone());
        let id = loader.load_string(&name, &source);
        let source_text = loader.source_map().source(id);
        assert!(
            source_text.len() <= 10_000,
            "fixture {} unexpectedly large",
            path.display()
        );
    }
}

#[test]
fn empty_fixture_loads() {
    let err_dir = fixtures_root().join("err");
    let empty_path = err_dir.join("empty.orv");
    assert!(empty_path.exists(), "fixtures/err/empty.orv must exist");

    let source = std::fs::read_to_string(&empty_path).unwrap();
    assert!(source.is_empty(), "empty.orv should be empty");

    let mut loader = orv_core::source::SourceLoader::new(err_dir);
    let id = loader.load_string("empty.orv", &source);
    assert_eq!(loader.source_map().source(id), "");
}
