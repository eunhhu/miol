//! 멀티파일 프로젝트 로더 (B3).
//!
//! # 역할
//! entry 파일에서 출발해 `import` 문을 따라 다른 `.orv` 파일을 재귀적으로
//! 로드하고, 전체를 단일 `Program` 으로 병합한다. MVP 수준:
//!
//! 1. entry 파일을 파싱한다.
//! 2. `import` 문을 발견하면 path segment 들을 디렉토리/파일 경로로 변환해
//!    파일을 찾는다 (`a.b.c` → `a/b/c.orv`, 없으면 `a/b.orv`).
//! 3. 이미 로드한 파일은 중복 로드하지 않는다 (순환 방지).
//! 4. 로드한 파일들을 한 덩어리의 Program 으로 concatenate — 모든 import 된
//!    모듈의 top-level decl 이 entry 앞에 배치된다.
//!
//! # 범위 밖 (후속)
//! - 파일별 scope 격리 — 현재는 모든 pub/private decl 이 global 로 섞임.
//! - visibility enforcement — `pub` 없는 decl 을 다른 파일이 참조해도 허용.
//! - `.orv` 이외 확장자, 외부 레지스트리 의존성.
//! - 사이클 진단 — 현재는 "이미 로드" 검사로 무한루프만 방지.

#![warn(missing_docs)]

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use orv_diagnostics::{Diagnostic, FileId};
use orv_syntax::ast::{ImportStmt, Program, Stmt};

/// 멀티파일 로딩 결과.
#[derive(Debug)]
pub struct LoadedProject {
    /// 병합된 프로그램 — 모든 import 된 파일의 top-level stmt 가 entry 앞에
    /// prepend 된 결과.
    pub program: Program,
    /// 누적 진단 (lex/parse 단계). resolve 이후 단계는 호출자가 수행한다.
    pub diagnostics: Vec<Diagnostic>,
}

/// entry 파일 경로를 주면 import 를 따라 multi-file 병합을 수행한다.
///
/// # Errors
/// I/O 실패, 혹은 `import` 가 지목한 파일을 찾지 못하면 [`LoadError`] 반환.
pub fn load_project(entry: &Path) -> Result<LoadedProject, LoadError> {
    let mut loader = Loader::default();
    loader.load_file(entry)?;
    let merged_items = loader.take_merged_items();
    let span = merged_items.first().map(Stmt::span).unwrap_or_else(|| {
        orv_diagnostics::Span::new(FileId(0), orv_diagnostics::ByteRange::new(0, 0))
    });
    Ok(LoadedProject {
        program: Program {
            items: merged_items,
            span,
        },
        diagnostics: loader.diagnostics,
    })
}

/// 프로젝트 로딩 에러.
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    /// 파일 시스템 에러.
    #[error("i/o error reading {path}: {source}")]
    Io {
        /// 실패한 경로.
        path: PathBuf,
        /// 원본 에러.
        #[source]
        source: std::io::Error,
    },
    /// import 가 가리키는 파일을 못 찾음.
    #[error("unresolved import `{module}` (tried {tried:?})")]
    UnresolvedImport {
        /// 요청된 모듈 경로.
        module: String,
        /// 시도한 파일 경로들.
        tried: Vec<PathBuf>,
    },
}

#[derive(Default)]
struct Loader {
    visited: HashSet<PathBuf>,
    /// import 된 모듈의 top-level stmt. 역순 import 의 의존 순서를 맞추기 위해
    /// DFS 방문 완료 순으로 push 한다 (dependency first). entry 는 별도 저장해
    /// 맨 끝에 배치한다.
    imported_items: Vec<Stmt>,
    entry_items: Vec<Stmt>,
    diagnostics: Vec<Diagnostic>,
    /// 다음 할당할 FileId. entry = 0, 이후 import 는 1, 2, ...
    next_file_id: u32,
    /// 프로젝트 루트 — entry 파일의 부모 디렉토리. import path 는 이 루트
    /// 기준으로 해석된다 (SPEC §8 의 디렉토리 기반 모듈 경로).
    project_root: Option<PathBuf>,
}

impl Loader {
    fn load_file(&mut self, path: &Path) -> Result<(), LoadError> {
        let canon = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if !self.visited.insert(canon.clone()) {
            return Ok(());
        }
        let is_entry = self.project_root.is_none();
        if is_entry {
            self.project_root = Some(
                canon
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| PathBuf::from(".")),
            );
        }
        let source = std::fs::read_to_string(&canon).map_err(|source| LoadError::Io {
            path: canon.clone(),
            source,
        })?;
        let file_id = FileId(self.next_file_id);
        self.next_file_id += 1;
        let lx = orv_syntax::lex(&source, file_id);
        self.diagnostics.extend(lx.diagnostics);
        let pr = orv_syntax::parse_with_newlines(lx.tokens, file_id, lx.newlines);
        self.diagnostics.extend(pr.diagnostics);

        // import 를 먼저 따라가 의존 파일을 로드한다 (depth-first).
        for stmt in &pr.program.items {
            if let Stmt::Import(import) = stmt {
                self.load_import(import)?;
            }
        }

        // entry 파일은 맨 뒤, import 된 파일은 앞쪽 (의존 먼저).
        if is_entry {
            self.entry_items.extend(pr.program.items);
        } else {
            self.imported_items.extend(pr.program.items);
        }
        Ok(())
    }

    fn load_import(&mut self, import: &ImportStmt) -> Result<(), LoadError> {
        // import path 는 프로젝트 루트 기준. root 가 세팅되지 않은 경우는 없다
        // (entry 가 먼저 세팅). 방어적으로 "." 로 대체.
        let base: PathBuf = self
            .project_root
            .clone()
            .unwrap_or_else(|| PathBuf::from("."));
        let segments: Vec<&str> = import.path.iter().map(|i| i.name.as_str()).collect();
        if segments.is_empty() {
            return Ok(());
        }
        // 후보 1: `<root>/a/b/c.orv`
        let mut candidates: Vec<PathBuf> = Vec::new();
        let mut p = base.clone();
        for seg in &segments {
            p.push(seg);
        }
        candidates.push(p.with_extension("orv"));
        // 후보 2: `<root>/a/b/c/mod.orv` 관용.
        let mut p2 = base;
        for seg in &segments {
            p2.push(seg);
        }
        p2.push("mod.orv");
        candidates.push(p2);
        for cand in &candidates {
            if cand.exists() {
                return self.load_file(cand);
            }
        }
        Err(LoadError::UnresolvedImport {
            module: segments.join("."),
            tried: candidates,
        })
    }

    fn take_merged_items(&mut self) -> Vec<Stmt> {
        let mut out = std::mem::take(&mut self.imported_items);
        out.extend(std::mem::take(&mut self.entry_items));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// tempdir 안에서 파일 트리를 만들고 entry 로더를 호출한다.
    ///
    /// 여러 테스트가 같은 프로세스에서 병렬 실행되므로 atomic counter 로
    /// 고유 이름을 부여해 경로 충돌을 방지한다.
    fn run_in_tempdir(tree: &[(&str, &str)], entry: &str) -> Result<LoadedProject, LoadError> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let n = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("orv_test_{}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for (rel, content) in tree {
            let path = dir.join(rel);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            let mut f = std::fs::File::create(&path).unwrap();
            f.write_all(content.as_bytes()).unwrap();
        }
        load_project(&dir.join(entry))
    }

    #[test]
    fn single_import_merges_file() {
        let r = run_in_tempdir(
            &[
                ("models/user.orv", "pub struct User { name: string }"),
                (
                    "main.orv",
                    "import models.user.User\nlet u: User = { name: \"x\" }",
                ),
            ],
            "main.orv",
        )
        .unwrap();
        // 병합된 program 은 models/user.orv 의 struct + main 의 import + let.
        // items 순서: import된 파일 먼저 (struct), entry 의 import + let 이후.
        let kinds: Vec<&str> = r
            .program
            .items
            .iter()
            .map(|s| match s {
                Stmt::Struct(_) => "struct",
                Stmt::Import(_) => "import",
                Stmt::Let(_) => "let",
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, vec!["struct", "import", "let"]);
    }

    #[test]
    fn cycle_is_broken_by_visited_set() {
        // A imports B, B imports A — 순환 에러 없이 각 파일 한 번씩만 로드.
        let r = run_in_tempdir(
            &[
                ("a.orv", "import b.X\npub struct X {}"),
                ("b.orv", "import a.X"),
            ],
            "a.orv",
        )
        .unwrap();
        // struct X 가 정확히 한 번만 있어야 한다.
        let struct_count = r
            .program
            .items
            .iter()
            .filter(|s| matches!(s, Stmt::Struct(_)))
            .count();
        assert_eq!(struct_count, 1);
    }

    #[test]
    fn unresolved_import_returns_error() {
        let err =
            run_in_tempdir(&[("main.orv", "import does.not.exist.X")], "main.orv").unwrap_err();
        match err {
            LoadError::UnresolvedImport { module, .. } => {
                assert_eq!(module, "does.not.exist");
            }
            _ => panic!("expected UnresolvedImport, got {err:?}"),
        }
    }
}
