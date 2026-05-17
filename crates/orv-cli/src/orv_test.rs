#![allow(clippy::redundant_pub_crate)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use orv_diagnostics::{ByteRange, FileId};
use orv_syntax::ast::Stmt;

use super::{byte_position, load_checked_hir_from_sources, project_entry_path};

#[derive(Debug)]
pub(crate) struct OrvTestSummary {
    pub(crate) selected: usize,
    pub(crate) passed: usize,
    pub(crate) failed: usize,
    pub(crate) files: Vec<PathBuf>,
}

#[derive(Clone, Debug)]
struct OrvTestCase {
    file: PathBuf,
    name: String,
    span: ByteRange,
    start_line: usize,
    start_character: usize,
    end_line: usize,
    end_character: usize,
}

pub(super) fn cmd_test(path: &Path, filter: Option<&str>, list: bool) -> anyhow::Result<()> {
    if list {
        let value = orv_test_list_json(path, filter)?;
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    let summary = orv_test_summary(path, filter)?;
    println!("test: {} passed", summary.passed);
    Ok(())
}

pub(crate) fn orv_test_list_json(
    path: &Path,
    filter: Option<&str>,
) -> anyhow::Result<serde_json::Value> {
    let tests = orv_test_cases(path, filter)?
        .into_iter()
        .map(|case| {
            serde_json::json!({
                "path": case.file.display().to_string(),
                "name": case.name,
                "line": case.start_line + 1,
                "column": case.start_character + 1,
                "span": {
                    "start": case.span.start,
                    "end": case.span.end,
                },
                "range": {
                    "start": {
                        "line": case.start_line,
                        "character": case.start_character,
                    },
                    "end": {
                        "line": case.end_line,
                        "character": case.end_character,
                    },
                },
            })
        })
        .collect::<Vec<_>>();
    Ok(serde_json::json!({
        "schema_version": 1,
        "tests": tests,
    }))
}

fn orv_test_cases(path: &Path, filter: Option<&str>) -> anyhow::Result<Vec<OrvTestCase>> {
    let files = orv_test_candidate_files(path)?;
    let mut cases = Vec::new();
    for file in files {
        let source = std::fs::read_to_string(&file)
            .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", file.display()))?;
        for case in orv_test_blocks(&source) {
            if filter.is_none_or(|filter| case.name.contains(filter)) {
                cases.push(OrvTestCase {
                    file: file.clone(),
                    ..case
                });
            }
        }
    }
    Ok(cases)
}

pub(crate) fn orv_test_summary(
    path: &Path,
    filter: Option<&str>,
) -> anyhow::Result<OrvTestSummary> {
    let files = orv_test_candidate_files(path)?;
    let mut summary = OrvTestSummary {
        selected: 0,
        passed: 0,
        failed: 0,
        files: Vec::new(),
    };
    for file in files {
        let source = std::fs::read_to_string(&file)
            .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", file.display()))?;
        let selected_cases = orv_test_blocks(&source)
            .into_iter()
            .filter(|case| filter.is_none_or(|filter| case.name.contains(filter)))
            .collect::<Vec<_>>();
        if selected_cases.is_empty() {
            continue;
        }
        summary.selected += selected_cases.len();
        summary.files.push(file.clone());
        for case in selected_cases {
            let test_source = orv_test_source_with_only_case(&source, case.span);
            let lowered = load_checked_hir_from_sources(&file, &test_source)?;
            let mut output = Vec::new();
            if let Err(err) = orv_runtime::run_with_writer(&lowered.program, &mut output) {
                summary.failed += 1;
                anyhow::bail!("test: {} `{}` failed: {err}", file.display(), case.name);
            }
            summary.passed += 1;
        }
    }
    Ok(summary)
}

fn orv_test_candidate_files(path: &Path) -> anyhow::Result<Vec<PathBuf>> {
    if path.is_dir() {
        let mut files = Vec::new();
        collect_orv_files(path, &mut files)?;
        files.sort();
        return Ok(files);
    }
    let file = project_entry_path(path)?;
    if is_orv_file(&file) {
        return Ok(vec![file]);
    }
    anyhow::bail!("test path must be a .orv file, orv.toml, or directory")
}

fn collect_orv_files(dir: &Path, out: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir)
        .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", dir.display()))?
    {
        let entry = entry.map_err(|e| anyhow::anyhow!("failed to read dir entry: {e}"))?;
        let path = entry.path();
        if path.is_dir() {
            collect_orv_files(&path, out)?;
        } else if is_orv_file(&path) {
            out.push(path);
        }
    }
    Ok(())
}

fn is_orv_file(path: &Path) -> bool {
    path.extension().and_then(std::ffi::OsStr::to_str) == Some("orv")
}

fn orv_test_blocks(source: &str) -> Vec<OrvTestCase> {
    let lexed = orv_syntax::lex(source, FileId(0));
    let mut blocks = Vec::new();
    for index in 0..lexed.tokens.len().saturating_sub(1) {
        let head = &lexed.tokens[index];
        if !matches!(&head.kind, orv_syntax::TokenKind::Ident(name) if name == "test") {
            continue;
        }
        let name_token = &lexed.tokens[index + 1];
        let orv_syntax::TokenKind::String(name) = &name_token.kind else {
            continue;
        };
        if !matches!(
            lexed.tokens.get(index + 2).map(|token| &token.kind),
            Some(orv_syntax::TokenKind::LBrace)
        ) {
            continue;
        }
        let Some(end) = orv_test_block_end(&lexed.tokens[index + 2..]) else {
            continue;
        };
        let (start_line, start_character) = byte_position(source, head.span.range.start);
        let (end_line, end_character) = byte_position(source, end);
        blocks.push(OrvTestCase {
            file: PathBuf::new(),
            name: name.clone(),
            span: ByteRange::new(head.span.range.start, end),
            start_line,
            start_character,
            end_line,
            end_character,
        });
    }
    blocks
}

fn orv_test_block_end(tokens: &[orv_syntax::Token]) -> Option<u32> {
    let mut depth = 0usize;
    let mut saw_block = false;
    for token in tokens {
        match token.kind {
            orv_syntax::TokenKind::LBrace => {
                saw_block = true;
                depth += 1;
            }
            orv_syntax::TokenKind::RBrace if saw_block => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(token.span.range.end);
                }
            }
            orv_syntax::TokenKind::Eof => return None,
            _ => {}
        }
    }
    None
}

fn orv_test_source_with_only_case(source: &str, selected: ByteRange) -> String {
    let mut filtered = String::with_capacity(source.len());
    let mut cursor = 0usize;
    for case in orv_test_blocks(source) {
        let start = usize::try_from(case.span.start).unwrap_or(usize::MAX);
        let end = usize::try_from(case.span.end).unwrap_or(usize::MAX);
        if start > source.len() || end > source.len() || start > end {
            continue;
        }
        filtered.push_str(&source[cursor..start]);
        if case.span == selected {
            filtered.push_str(&source[start..end]);
        } else {
            filtered.push_str(&orv_blank_source_slice(&source[start..end]));
        }
        cursor = end;
    }
    filtered.push_str(&source[cursor..]);
    filtered
}

fn orv_blank_source_slice(source: &str) -> String {
    source
        .bytes()
        .map(|byte| match byte {
            b'\n' | b'\r' => char::from(byte),
            _ => ' ',
        })
        .collect()
}

pub(super) fn orv_test_source_bundle(
    entry: &Path,
    entry_source: &str,
) -> anyhow::Result<Vec<(PathBuf, String)>> {
    let root = entry.parent().unwrap_or_else(|| Path::new("."));
    let mut sources = BTreeMap::new();
    sources.insert(entry.to_path_buf(), entry_source.to_string());
    let mut stack = orv_test_import_paths(root, entry_source);
    while let Some(path) = stack.pop() {
        if sources.contains_key(&path) {
            continue;
        }
        let source = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("failed to read {}: {e}", path.display()))?;
        stack.extend(orv_test_import_paths(root, &source));
        sources.insert(path, source);
    }
    Ok(sources.into_iter().collect())
}

fn orv_test_import_paths(root: &Path, source: &str) -> Vec<PathBuf> {
    let lexed = orv_syntax::lex(source, FileId(0));
    let parsed = orv_syntax::parse_with_newlines(lexed.tokens, FileId(0), lexed.newlines);
    parsed
        .program
        .items
        .iter()
        .filter_map(|stmt| match stmt {
            Stmt::Import(import) => orv_test_import_candidates(root, import)
                .into_iter()
                .find(|candidate| candidate.is_file()),
            _ => None,
        })
        .collect()
}

fn orv_test_import_candidates(root: &Path, import: &orv_syntax::ast::ImportStmt) -> Vec<PathBuf> {
    let mut path = root.to_path_buf();
    for segment in &import.path {
        path.push(&segment.name);
    }
    let mut mod_path = path.clone();
    mod_path.push("mod.orv");
    vec![path.with_extension("orv"), mod_path]
}
