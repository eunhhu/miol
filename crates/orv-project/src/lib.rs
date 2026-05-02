//! 멀티파일 프로젝트 로더 (B3).
//!
//! # 역할
//! entry 파일에서 출발해 `import` 문을 따라 다른 `.orv` 파일을 재귀적으로
//! 로드하고, 전체를 단일 `Program` 으로 병합한다. 파일 시스템 또는 build
//! artifact source bundle 둘 다 같은 병합 규칙을 사용한다. MVP 수준:
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

use std::collections::{HashMap, HashSet};
use std::path::Component;
use std::path::{Path, PathBuf};

use orv_diagnostics::{ByteRange, Diagnostic, FileId, Span};
use orv_syntax::ast::{
    Block, Expr, ExprKind, FunctionBody, ImportStmt, Pattern, Program, Stmt, TypeRef, TypeRefKind,
};

/// 멀티파일 로딩 결과.
#[derive(Debug)]
pub struct LoadedProject {
    /// 병합된 프로그램 — 모든 import 된 파일의 top-level stmt 가 entry 앞에
    /// prepend 된 결과.
    pub program: Program,
    /// 로드된 소스 파일 목록. [`FileId`] 순서와 일치한다.
    pub files: Vec<SourceFile>,
    /// AST 기반 프로젝트 그래프 v1.
    pub graph: ProjectGraph,
    /// 누적 진단 (lex/parse 단계). resolve 이후 단계는 호출자가 수행한다.
    pub diagnostics: Vec<Diagnostic>,
}

/// 로더가 할당한 `FileId`와 실제 파일 내용을 연결하는 source map entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceFile {
    /// 컴파일러 내부 파일 id.
    pub id: FileId,
    /// 실제 파일 경로.
    pub path: PathBuf,
    /// 파일 내용.
    pub source: String,
}

/// 프로젝트 그래프 노드 id.
pub type ProjectNodeId = u32;

/// AST 기반 프로젝트 그래프.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProjectGraph {
    /// 그래프 노드.
    pub nodes: Vec<ProjectNode>,
    /// 그래프 엣지.
    pub edges: Vec<ProjectEdge>,
}

/// 프로젝트 그래프 노드.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectNode {
    /// 노드 id.
    pub id: ProjectNodeId,
    /// 노드 종류.
    pub kind: ProjectNodeKind,
    /// 사람이 읽는 이름.
    pub name: String,
    /// 원본 파일.
    pub file: FileId,
    /// 원본 소스 범위.
    pub span: Span,
}

/// 프로젝트 그래프 노드 종류.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectNodeKind {
    /// 소스 파일.
    File,
    /// import 문.
    Import,
    /// struct 선언.
    Struct,
    /// enum 선언.
    Enum,
    /// type alias 선언.
    TypeAlias,
    /// function 선언.
    Function,
    /// define 선언.
    Define,
    /// 도메인 호출.
    Domain,
}

/// 프로젝트 그래프 엣지.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectEdge {
    /// 출발 노드.
    pub from: ProjectNodeId,
    /// 도착 노드.
    pub to: ProjectNodeId,
    /// 엣지 종류.
    pub kind: ProjectEdgeKind,
}

/// 프로젝트 그래프 엣지 종류.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectEdgeKind {
    /// 파일이 선언/import/domain 노드를 포함한다.
    Contains,
    /// import 노드가 대상 파일을 참조한다.
    Imports,
}

/// entry 파일 경로를 주면 import 를 따라 multi-file 병합을 수행한다.
///
/// # Errors
/// I/O 실패, 혹은 `import` 가 지목한 파일을 찾지 못하면 [`LoadError`] 반환.
pub fn load_project(entry: &Path) -> Result<LoadedProject, LoadError> {
    let mut loader = Loader::default();
    loader.load_file(entry)?;
    Ok(loader.finish())
}

/// Source bundle에서 import 를 따라 multi-file 병합을 수행한다.
///
/// 이 경로는 build artifact처럼 source snapshot 이 이미 주어진 경우 사용한다.
/// 파일 시스템을 읽지 않고, import 후보도 bundle 내부 path 목록에서만 찾는다.
///
/// # Errors
/// Entry 또는 import 대상이 bundle 안에 없으면 [`LoadError`] 반환.
pub fn load_project_from_sources<P, S>(
    entry: &Path,
    sources: impl IntoIterator<Item = (P, S)>,
) -> Result<LoadedProject, LoadError>
where
    P: Into<PathBuf>,
    S: Into<String>,
{
    let mut loader = Loader::from_sources(InMemorySources::new(sources));
    loader.load_file(entry)?;
    Ok(loader.finish())
}

impl Loader {
    fn finish(&mut self) -> LoadedProject {
        let merged_items = self.take_merged_items();
        let span = merged_items.first().map_or_else(
            || orv_diagnostics::Span::new(FileId(0), orv_diagnostics::ByteRange::new(0, 0)),
            Stmt::span,
        );
        let program = Program {
            items: merged_items,
            span,
        };
        let files = std::mem::take(&mut self.files);
        let graph = build_project_graph(&program, &files, self.project_root.as_deref());
        LoadedProject {
            program,
            files,
            graph,
            diagnostics: std::mem::take(&mut self.diagnostics),
        }
    }
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
    /// source bundle 안에서 필요한 파일을 못 찾음.
    #[error("source `{path}` not found in source bundle")]
    MissingSource {
        /// 실패한 경로.
        path: PathBuf,
    },
}

struct InMemorySources {
    files: HashMap<PathBuf, String>,
}

impl InMemorySources {
    fn new<P, S>(sources: impl IntoIterator<Item = (P, S)>) -> Self
    where
        P: Into<PathBuf>,
        S: Into<String>,
    {
        Self {
            files: sources
                .into_iter()
                .map(|(path, source)| {
                    let path = path.into();
                    (normalize_source_path(&path), source.into())
                })
                .collect(),
        }
    }

    fn contains(&self, path: &Path) -> bool {
        self.files.contains_key(&normalize_source_path(path))
    }

    fn get(&self, path: &Path) -> Option<&str> {
        self.files
            .get(&normalize_source_path(path))
            .map(String::as_str)
    }
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
    files: Vec<SourceFile>,
    /// 다음 할당할 `FileId`. entry = 0, 이후 import 는 1, 2, ...
    next_file_id: u32,
    /// 프로젝트 루트 — entry 파일의 부모 디렉토리. import path 는 이 루트
    /// 기준으로 해석된다 (SPEC §8 의 디렉토리 기반 모듈 경로).
    project_root: Option<PathBuf>,
    source_bundle: Option<InMemorySources>,
}

impl Loader {
    fn from_sources(source_bundle: InMemorySources) -> Self {
        Self {
            source_bundle: Some(source_bundle),
            ..Self::default()
        }
    }

    fn load_file(&mut self, path: &Path) -> Result<(), LoadError> {
        let canon = self.resolve_path(path);
        if !self.visited.insert(canon.clone()) {
            return Ok(());
        }
        let is_entry = self.project_root.is_none();
        if is_entry {
            self.project_root = Some(
                canon
                    .parent()
                    .map_or_else(|| PathBuf::from("."), Path::to_path_buf),
            );
        }
        let source = self.read_source(&canon)?;
        let file_id = FileId(self.next_file_id);
        self.next_file_id += 1;
        let lx = orv_syntax::lex(&source, file_id);
        self.diagnostics.extend(lx.diagnostics);
        let pr = orv_syntax::parse_with_newlines(lx.tokens, file_id, lx.newlines);
        self.diagnostics.extend(pr.diagnostics);
        self.files.push(SourceFile {
            id: file_id,
            path: canon,
            source,
        });

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

    fn resolve_path(&self, path: &Path) -> PathBuf {
        if self.source_bundle.is_some() {
            normalize_source_path(path)
        } else {
            path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
        }
    }

    fn read_source(&self, path: &Path) -> Result<String, LoadError> {
        if let Some(bundle) = &self.source_bundle {
            return bundle
                .get(path)
                .map(str::to_string)
                .ok_or_else(|| LoadError::MissingSource {
                    path: path.to_path_buf(),
                });
        }
        std::fs::read_to_string(path).map_err(|source| LoadError::Io {
            path: path.to_path_buf(),
            source,
        })
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
        let candidates = import_candidates(&base, &segments);
        for cand in &candidates {
            if self.source_exists(cand) {
                return self.load_file(cand);
            }
        }
        Err(LoadError::UnresolvedImport {
            module: segments.join("."),
            tried: candidates,
        })
    }

    fn source_exists(&self, path: &Path) -> bool {
        self.source_bundle
            .as_ref()
            .map_or_else(|| path.exists(), |bundle| bundle.contains(path))
    }

    fn take_merged_items(&mut self) -> Vec<Stmt> {
        let mut out = std::mem::take(&mut self.imported_items);
        out.extend(std::mem::take(&mut self.entry_items));
        out
    }
}

fn normalize_source_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(part) => out.push(part),
            Component::RootDir | Component::Prefix(_) => out.push(component.as_os_str()),
        }
    }
    out
}

fn import_candidates(base: &Path, segments: &[&str]) -> Vec<PathBuf> {
    // 후보 1: `<root>/a/b/c.orv`
    let mut candidates: Vec<PathBuf> = Vec::new();
    let mut p = base.to_path_buf();
    for seg in segments {
        p.push(seg);
    }
    candidates.push(p.with_extension("orv"));
    // 후보 2: `<root>/a/b/c/mod.orv` 관용.
    let mut p2 = base.to_path_buf();
    for seg in segments {
        p2.push(seg);
    }
    p2.push("mod.orv");
    candidates.push(p2);
    candidates
}

fn build_project_graph(
    program: &Program,
    files: &[SourceFile],
    project_root: Option<&Path>,
) -> ProjectGraph {
    let mut builder = GraphBuilder::new(files);
    for stmt in &program.items {
        builder.collect_stmt(stmt, project_root);
    }
    builder.graph
}

struct GraphBuilder {
    graph: ProjectGraph,
    file_nodes: Vec<(FileId, PathBuf, ProjectNodeId)>,
}

impl GraphBuilder {
    fn new(files: &[SourceFile]) -> Self {
        let mut builder = Self {
            graph: ProjectGraph::default(),
            file_nodes: Vec::new(),
        };
        for file in files {
            let len = u32::try_from(file.source.len()).unwrap_or(u32::MAX);
            let span = Span::new(file.id, ByteRange::new(0, len));
            let id = builder.add_node(
                ProjectNodeKind::File,
                file.path.display().to_string(),
                file.id,
                span,
            );
            builder.file_nodes.push((file.id, file.path.clone(), id));
        }
        builder
    }

    fn add_node(
        &mut self,
        kind: ProjectNodeKind,
        name: impl Into<String>,
        file: FileId,
        span: Span,
    ) -> ProjectNodeId {
        let id = u32::try_from(self.graph.nodes.len()).unwrap_or(u32::MAX);
        self.graph.nodes.push(ProjectNode {
            id,
            kind,
            name: name.into(),
            file,
            span,
        });
        id
    }

    fn add_edge(&mut self, from: ProjectNodeId, to: ProjectNodeId, kind: ProjectEdgeKind) {
        self.graph.edges.push(ProjectEdge { from, to, kind });
    }

    fn file_node(&self, file: FileId) -> Option<ProjectNodeId> {
        self.file_nodes
            .iter()
            .find_map(|(id, _, node)| (*id == file).then_some(*node))
    }

    fn add_child_node(
        &mut self,
        kind: ProjectNodeKind,
        name: impl Into<String>,
        span: Span,
    ) -> ProjectNodeId {
        let node = self.add_node(kind, name, span.file, span);
        if let Some(file) = self.file_node(span.file) {
            self.add_edge(file, node, ProjectEdgeKind::Contains);
        }
        node
    }

    fn collect_stmt(&mut self, stmt: &Stmt, project_root: Option<&Path>) {
        match stmt {
            Stmt::Let(stmt) => self.collect_expr(&stmt.init, project_root),
            Stmt::Const(stmt) => self.collect_expr(&stmt.init, project_root),
            Stmt::Function(stmt) => {
                let kind = if stmt.is_define {
                    ProjectNodeKind::Define
                } else {
                    ProjectNodeKind::Function
                };
                self.add_child_node(kind, stmt.name.name.clone(), stmt.span);
                self.collect_function_body(&stmt.body, project_root);
            }
            Stmt::Struct(stmt) => {
                self.add_child_node(ProjectNodeKind::Struct, stmt.name.name.clone(), stmt.span);
                for field in &stmt.fields {
                    self.collect_type_ref(&field.ty, project_root);
                }
            }
            Stmt::Enum(stmt) => {
                self.add_child_node(ProjectNodeKind::Enum, stmt.name.name.clone(), stmt.span);
                for variant in &stmt.variants {
                    self.collect_expr(&variant.value, project_root);
                }
            }
            Stmt::TypeAlias(stmt) => {
                self.add_child_node(
                    ProjectNodeKind::TypeAlias,
                    stmt.name.name.clone(),
                    stmt.span,
                );
                self.collect_type_ref(&stmt.ty, project_root);
            }
            Stmt::Return(stmt) => {
                if let Some(value) = &stmt.value {
                    self.collect_expr(value, project_root);
                }
            }
            Stmt::Import(import) => {
                let import_node =
                    self.add_child_node(ProjectNodeKind::Import, import_name(import), import.span);
                if let Some(target) = self.import_target(import, project_root) {
                    self.add_edge(import_node, target, ProjectEdgeKind::Imports);
                }
            }
            Stmt::Expr(expr) => self.collect_expr(expr, project_root),
        }
    }

    fn collect_function_body(&mut self, body: &FunctionBody, project_root: Option<&Path>) {
        match body {
            FunctionBody::Block(block) => self.collect_block(block, project_root),
            FunctionBody::Expr(expr) => self.collect_expr(expr, project_root),
        }
    }

    fn collect_block(&mut self, block: &Block, project_root: Option<&Path>) {
        for stmt in &block.stmts {
            self.collect_stmt(stmt, project_root);
        }
    }

    // Mirrors `ExprKind` variants so graph extraction stays exhaustive and local.
    #[allow(clippy::too_many_lines)]
    fn collect_expr(&mut self, expr: &Expr, project_root: Option<&Path>) {
        match &expr.kind {
            ExprKind::Domain { name, args } => {
                self.add_child_node(ProjectNodeKind::Domain, name.name.clone(), expr.span);
                for arg in args {
                    self.collect_expr(arg, project_root);
                }
            }
            ExprKind::Unary { expr, .. }
            | ExprKind::Paren(expr)
            | ExprKind::Throw(expr)
            | ExprKind::Await(expr) => self.collect_expr(expr, project_root),
            ExprKind::Binary { lhs, rhs, .. } => {
                self.collect_expr(lhs, project_root);
                self.collect_expr(rhs, project_root);
            }
            ExprKind::Block(block) => self.collect_block(block, project_root),
            ExprKind::If {
                cond,
                then,
                else_branch,
            } => {
                self.collect_expr(cond, project_root);
                self.collect_block(then, project_root);
                if let Some(expr) = else_branch {
                    self.collect_expr(expr, project_root);
                }
            }
            ExprKind::When { scrutinee, arms } => {
                self.collect_expr(scrutinee, project_root);
                for arm in arms {
                    self.collect_pattern(&arm.pattern, project_root);
                    self.collect_expr(&arm.body, project_root);
                }
            }
            ExprKind::Assign { value, .. } => self.collect_expr(value, project_root),
            ExprKind::Call { callee, args } => {
                self.collect_expr(callee, project_root);
                for arg in args {
                    self.collect_expr(arg, project_root);
                }
            }
            ExprKind::AssignField { object, value, .. } => {
                self.collect_expr(object, project_root);
                self.collect_expr(value, project_root);
            }
            ExprKind::AssignIndex {
                object,
                index,
                value,
            } => {
                self.collect_expr(object, project_root);
                self.collect_expr(index, project_root);
                self.collect_expr(value, project_root);
            }
            ExprKind::For { iter, body, .. } => {
                self.collect_expr(iter, project_root);
                self.collect_block(body, project_root);
            }
            ExprKind::While { cond, body } => {
                self.collect_expr(cond, project_root);
                self.collect_block(body, project_root);
            }
            ExprKind::Range { start, end, .. } => {
                self.collect_expr(start, project_root);
                self.collect_expr(end, project_root);
            }
            ExprKind::Array(items) | ExprKind::Tuple(items) => {
                for item in items {
                    self.collect_expr(item, project_root);
                }
            }
            ExprKind::Object(fields) | ExprKind::TypedObject { fields, .. } => {
                for field in fields {
                    self.collect_expr(&field.value, project_root);
                }
            }
            ExprKind::Index { target, index } => {
                self.collect_expr(target, project_root);
                self.collect_expr(index, project_root);
            }
            ExprKind::Slice { target, start, end } => {
                self.collect_expr(target, project_root);
                if let Some(start) = start {
                    self.collect_expr(start, project_root);
                }
                if let Some(end) = end {
                    self.collect_expr(end, project_root);
                }
            }
            ExprKind::Field { target, .. } | ExprKind::OptionalField { target, .. } => {
                self.collect_expr(target, project_root);
            }
            ExprKind::Lambda { body, .. } => self.collect_function_body(body, project_root),
            ExprKind::Cast { expr, ty } => {
                self.collect_expr(expr, project_root);
                self.collect_type_ref(ty, project_root);
            }
            ExprKind::Try { try_block, catch } => {
                self.collect_block(try_block, project_root);
                if let Some(catch) = catch {
                    self.collect_block(&catch.body, project_root);
                }
            }
            ExprKind::String(segments) => {
                for segment in segments {
                    if let orv_syntax::ast::StringSegment::Interp(expr) = segment {
                        self.collect_expr(expr, project_root);
                    }
                }
            }
            ExprKind::Integer(_)
            | ExprKind::Float(_)
            | ExprKind::Regex { .. }
            | ExprKind::True
            | ExprKind::False
            | ExprKind::Void
            | ExprKind::Ident(_)
            | ExprKind::TypeName(_)
            | ExprKind::Break
            | ExprKind::Continue => {}
        }
    }

    fn collect_pattern(&mut self, pattern: &Pattern, project_root: Option<&Path>) {
        match pattern {
            Pattern::Literal(expr)
            | Pattern::Guard(expr)
            | Pattern::Not(expr)
            | Pattern::Contains(expr) => self.collect_expr(expr, project_root),
            Pattern::Range { start, end, .. } => {
                self.collect_expr(start, project_root);
                self.collect_expr(end, project_root);
            }
            Pattern::Wildcard => {}
        }
    }

    fn collect_type_ref(&mut self, ty: &TypeRef, project_root: Option<&Path>) {
        match &ty.kind {
            TypeRefKind::Nullable(inner) | TypeRefKind::Array(inner) => {
                self.collect_type_ref(inner, project_root);
            }
            TypeRefKind::Union(items) | TypeRefKind::Tuple(items) => {
                for item in items {
                    self.collect_type_ref(item, project_root);
                }
            }
            TypeRefKind::InlineObject(fields) => {
                for (_, ty) in fields {
                    self.collect_type_ref(ty, project_root);
                }
            }
            TypeRefKind::Named(_) | TypeRefKind::Pattern(_) => {}
        }
        if let Some(where_clause) = &ty.where_clause {
            self.collect_expr(where_clause, project_root);
        }
    }

    fn import_target(
        &self,
        import: &ImportStmt,
        project_root: Option<&Path>,
    ) -> Option<ProjectNodeId> {
        let root = project_root?;
        let segments: Vec<&str> = import.path.iter().map(|i| i.name.as_str()).collect();
        for candidate in import_candidates(root, &segments) {
            let canon = candidate.canonicalize().unwrap_or(candidate);
            if let Some((_, _, node)) = self.file_nodes.iter().find(|(_, path, _)| *path == canon) {
                return Some(*node);
            }
        }
        None
    }
}

fn import_name(import: &ImportStmt) -> String {
    let mut name = import
        .path
        .iter()
        .map(|segment| segment.name.as_str())
        .collect::<Vec<_>>()
        .join(".");
    if import.glob {
        if !name.is_empty() {
            name.push('.');
        }
        name.push('*');
    } else if !import.items.is_empty() {
        for item in &import.items {
            if !name.is_empty() {
                name.push('.');
            }
            name.push_str(&item.name);
        }
    }
    name
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
    fn load_project_from_sources_merges_imported_bundle_files() {
        let r = load_project_from_sources(
            Path::new("main.orv"),
            [
                (
                    PathBuf::from("main.orv"),
                    "import models.user.User\nlet u: User = { name: \"x\" }",
                ),
                (
                    PathBuf::from("models/user.orv"),
                    "pub struct User { name: string }",
                ),
            ],
        )
        .unwrap();

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
        assert!(r
            .graph
            .edges
            .iter()
            .any(|edge| edge.kind == ProjectEdgeKind::Imports));
    }

    #[test]
    fn loaded_project_tracks_source_files_by_file_id() {
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

        assert_eq!(r.files.len(), 2);
        assert_eq!(r.files[0].id, FileId(0));
        assert!(r.files[0].path.ends_with("main.orv"));
        assert!(r.files[0].source.contains("import models.user.User"));
        assert_eq!(r.files[1].id, FileId(1));
        assert!(r.files[1].path.ends_with("models/user.orv"));
        assert!(r.files[1].source.contains("pub struct User"));
    }

    #[test]
    fn project_graph_collects_files_imports_declarations_and_domains() {
        let r = run_in_tempdir(
            &[
                ("models/user.orv", "pub struct User { name: string }"),
                (
                    "main.orv",
                    r#"import models.user.User
type UserId = int
function greet(name: string): string -> "hi {name}"
define Card() -> @div { @slot }
@server {
  @route GET /users {
    @respond 200 { ok: true }
  }
}"#,
                ),
            ],
            "main.orv",
        )
        .unwrap();

        assert!(r
            .graph
            .nodes
            .iter()
            .any(|n| n.kind == ProjectNodeKind::File && n.name.ends_with("main.orv")));
        assert!(r
            .graph
            .nodes
            .iter()
            .any(|n| n.kind == ProjectNodeKind::File && n.name.ends_with("models/user.orv")));
        assert!(r
            .graph
            .nodes
            .iter()
            .any(|n| n.kind == ProjectNodeKind::Import && n.name == "models.user.User"));
        assert!(r
            .graph
            .nodes
            .iter()
            .any(|n| n.kind == ProjectNodeKind::Struct && n.name == "User"));
        assert!(r
            .graph
            .nodes
            .iter()
            .any(|n| n.kind == ProjectNodeKind::TypeAlias && n.name == "UserId"));
        assert!(r
            .graph
            .nodes
            .iter()
            .any(|n| n.kind == ProjectNodeKind::Function && n.name == "greet"));
        assert!(r
            .graph
            .nodes
            .iter()
            .any(|n| n.kind == ProjectNodeKind::Define && n.name == "Card"));
        for domain in ["server", "route", "respond", "div", "slot"] {
            assert!(
                r.graph
                    .nodes
                    .iter()
                    .any(|n| n.kind == ProjectNodeKind::Domain && n.name == domain),
                "missing domain node {domain}"
            );
        }
    }

    #[test]
    fn project_graph_links_files_to_children_and_import_targets() {
        let r = run_in_tempdir(
            &[
                ("models/user.orv", "pub struct User { name: string }"),
                ("main.orv", "import models.user.User\n@out \"ok\""),
            ],
            "main.orv",
        )
        .unwrap();

        let main = r
            .graph
            .nodes
            .iter()
            .find(|n| n.kind == ProjectNodeKind::File && n.name.ends_with("main.orv"))
            .expect("main file node")
            .id;
        let user = r
            .graph
            .nodes
            .iter()
            .find(|n| n.kind == ProjectNodeKind::File && n.name.ends_with("models/user.orv"))
            .expect("user file node")
            .id;
        let import = r
            .graph
            .nodes
            .iter()
            .find(|n| n.kind == ProjectNodeKind::Import && n.name == "models.user.User")
            .expect("import node")
            .id;
        let out = r
            .graph
            .nodes
            .iter()
            .find(|n| n.kind == ProjectNodeKind::Domain && n.name == "out")
            .expect("out domain node")
            .id;

        assert!(r
            .graph
            .edges
            .iter()
            .any(|e| e.kind == ProjectEdgeKind::Contains && e.from == main && e.to == import));
        assert!(r
            .graph
            .edges
            .iter()
            .any(|e| e.kind == ProjectEdgeKind::Contains && e.from == main && e.to == out));
        assert!(r
            .graph
            .edges
            .iter()
            .any(|e| e.kind == ProjectEdgeKind::Imports && e.from == import && e.to == user));
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
            LoadError::Io { .. } | LoadError::MissingSource { .. } => {
                panic!("expected UnresolvedImport, got {err:?}");
            }
        }
    }
}
