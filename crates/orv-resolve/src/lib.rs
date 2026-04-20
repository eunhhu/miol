//! 이름 해석 — AST 에 등장하는 모든 식별자에 유일한 [`NameId`] 를 부여한다.
//!
//! # 목적
//! - 인터프리터가 문자열 기반 환경 조회를 버리고 `NameId` 로 바인딩을 찾도록 한다.
//! - 미정의 변수를 AST 단계에서 미리 잡아 런타임 에러 경로를 단순화한다.
//! - 향후 HIR 로 낮출 때 의존할 수 있는 안정적인 바인딩 참조를 제공한다.
//!
//! # 범위 (MVP)
//! 스코프를 여는 구조: 프로그램 루트, 함수/람다 본문, 블록 표현식, `for`
//! 루프의 순환 변수, `catch` 바인딩. `when` 의 `$` 가드에서 쓰이는 특수
//! 식별자 `"$"` 는 해석을 건너뛴다.

#![allow(clippy::module_name_repetitions)]

use std::collections::HashMap;

use orv_diagnostics::{Diagnostic, FileId, Span};
use orv_syntax::ast::{
    Block, CatchClause, ConstStmt, Expr, ExprKind, FunctionBody, FunctionStmt, Ident, LetStmt,
    ObjectField, Param, Pattern, Program, ReturnStmt, Stmt, StringSegment, StructStmt, WhenArm,
};

/// 해석 결과 — 선언/참조 위치를 모두 `NameId` 로 변환한 맵 + 진단 목록.
#[derive(Clone, Debug, Default)]
pub struct ResolveResult {
    /// 스팬 → `NameId` (선언, 참조 공용).
    ///
    /// 같은 `Ident` 값이 여러 곳에 복제돼 있어도 스팬이 유일하므로 Span 을
    /// 키로 쓴다. 바이트 오프셋이 같으면 동일 토큰을 의미한다.
    pub name_of: HashMap<SpanKey, NameId>,
    /// 선언 메타데이터 — `NameId` → (이름, 선언 스팬, 유형).
    pub decls: Vec<Decl>,
    /// 진단 메시지 (주로 미정의 변수).
    pub diagnostics: Vec<Diagnostic>,
}

/// 유일한 바인딩 식별자. HIR 이 이를 참조한다.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct NameId(pub u32);

/// 선언 메타데이터.
#[derive(Clone, Debug)]
pub struct Decl {
    /// 선언된 이름.
    pub name: String,
    /// 선언 스팬.
    pub span: Span,
    /// 바인딩 종류.
    pub kind: DeclKind,
}

/// 선언의 카테고리 — 단순 분류용. 변이 여부나 초기값은 상위 계층에서 재해석한다.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeclKind {
    /// `let`/`let mut`/`let sig` 바인딩.
    Let,
    /// `const` 바인딩.
    Const,
    /// 함수 선언 (이름있는 `function`).
    Function,
    /// `struct` 선언.
    Struct,
    /// 함수 또는 람다의 파라미터.
    Param,
    /// `for` 루프 변수.
    ForVar,
    /// `catch err` 바인딩.
    Catch,
}

/// `Span` 을 HashMap 키로 쓰기 위한 얇은 래퍼. `Span` 이 `Hash` 를 구현하지
/// 않으므로 `(file, start, end)` 튜플을 만든다.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SpanKey {
    /// 파일.
    pub file: FileId,
    /// 시작 오프셋.
    pub start: u32,
    /// 끝 오프셋.
    pub end: u32,
}

impl From<Span> for SpanKey {
    fn from(span: Span) -> Self {
        Self {
            file: span.file,
            start: span.range.start,
            end: span.range.end,
        }
    }
}

/// 프로그램을 해석한다. 결과에는 각 `Ident` 스팬에 대한 `NameId`, 선언
/// 메타데이터, 미정의 변수 진단이 담긴다.
#[must_use]
pub fn resolve(program: &Program) -> ResolveResult {
    let mut resolver = Resolver::new();
    resolver.resolve_program(program);
    resolver.finish()
}

/// 내부 해석기 상태. 스코프 스택과 선언 테이블을 유지한다.
struct Resolver {
    scopes: Vec<Scope>,
    decls: Vec<Decl>,
    name_of: HashMap<SpanKey, NameId>,
    diagnostics: Vec<Diagnostic>,
}

/// 하나의 스코프 — 이름 → `NameId` 매핑. 부모는 스택 위치로 암묵적으로 표현.
#[derive(Debug, Default)]
struct Scope {
    bindings: HashMap<String, NameId>,
}

impl Resolver {
    fn new() -> Self {
        Self {
            scopes: vec![Scope::default()],
            decls: Vec::new(),
            name_of: HashMap::new(),
            diagnostics: Vec::new(),
        }
    }

    fn finish(self) -> ResolveResult {
        ResolveResult {
            name_of: self.name_of,
            decls: self.decls,
            diagnostics: self.diagnostics,
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(Scope::default());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
        debug_assert!(!self.scopes.is_empty(), "root scope must remain");
    }

    fn declare(&mut self, ident: &Ident, kind: DeclKind) -> NameId {
        let id = NameId(u32::try_from(self.decls.len()).expect("too many bindings"));
        self.decls.push(Decl {
            name: ident.name.clone(),
            span: ident.span,
            kind,
        });
        self.name_of.insert(ident.span.into(), id);
        let scope = self.scopes.last_mut().expect("scope stack non-empty");
        scope.bindings.insert(ident.name.clone(), id);
        id
    }

    fn lookup(&self, name: &str) -> Option<NameId> {
        for scope in self.scopes.iter().rev() {
            if let Some(&id) = scope.bindings.get(name) {
                return Some(id);
            }
        }
        None
    }

    fn resolve_reference(&mut self, ident: &Ident) {
        if ident.name == "$" {
            return;
        }
        match self.lookup(&ident.name) {
            Some(id) => {
                self.name_of.insert(ident.span.into(), id);
            }
            None => {
                self.diagnostics.push(undefined_diagnostic(ident));
            }
        }
    }

    fn resolve_program(&mut self, program: &Program) {
        self.hoist_stmts(&program.items);
        for stmt in &program.items {
            self.resolve_stmt(stmt);
        }
    }

    /// 같은 스코프에서 선언된 함수/구조체는 사전에 등록해 상호/전방 참조를
    /// 허용한다. `let`/`const` 는 초기값 해석 이후 선언돼야 하므로 제외한다.
    fn hoist_stmts(&mut self, stmts: &[Stmt]) {
        for stmt in stmts {
            match stmt {
                Stmt::Function(f) => {
                    self.declare(&f.name, DeclKind::Function);
                }
                Stmt::Struct(s) => {
                    self.declare(&s.name, DeclKind::Struct);
                }
                _ => {}
            }
        }
    }

    fn resolve_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Let(let_stmt) => self.resolve_let(let_stmt),
            Stmt::Const(const_stmt) => self.resolve_const(const_stmt),
            Stmt::Function(func) => self.resolve_function(func),
            Stmt::Struct(struct_stmt) => self.resolve_struct(struct_stmt),
            Stmt::Return(ret) => self.resolve_return(ret),
            Stmt::Expr(expr) => self.resolve_expr(expr),
        }
    }

    fn resolve_let(&mut self, stmt: &LetStmt) {
        self.resolve_expr(&stmt.init);
        self.declare(&stmt.name, DeclKind::Let);
    }

    fn resolve_const(&mut self, stmt: &ConstStmt) {
        self.resolve_expr(&stmt.init);
        self.declare(&stmt.name, DeclKind::Const);
    }

    fn resolve_function(&mut self, func: &FunctionStmt) {
        // 이름은 hoist 단계에서 이미 선언됐거나 (최상위) 여기서 처음 선언된다.
        if !self.is_declared_in_current(&func.name.name) {
            self.declare(&func.name, DeclKind::Function);
        } else {
            self.record_use_in_current(&func.name);
        }
        self.resolve_function_body(&func.params, &func.body);
    }

    fn resolve_struct(&mut self, s: &StructStmt) {
        if !self.is_declared_in_current(&s.name.name) {
            self.declare(&s.name, DeclKind::Struct);
        } else {
            self.record_use_in_current(&s.name);
        }
        // 필드 타입은 현재 해석 범위 밖. 타입 참조는 이후 타입 체커가 다룬다.
    }

    fn resolve_return(&mut self, ret: &ReturnStmt) {
        if let Some(value) = &ret.value {
            self.resolve_expr(value);
        }
    }

    fn resolve_function_body(&mut self, params: &[Param], body: &FunctionBody) {
        self.push_scope();
        for param in params {
            self.declare(&param.name, DeclKind::Param);
        }
        match body {
            FunctionBody::Block(block) => self.resolve_block(block),
            FunctionBody::Expr(expr) => self.resolve_expr(expr),
        }
        self.pop_scope();
    }

    fn resolve_block(&mut self, block: &Block) {
        self.push_scope();
        self.hoist_stmts(&block.stmts);
        for stmt in &block.stmts {
            self.resolve_stmt(stmt);
        }
        self.pop_scope();
    }

    fn resolve_expr(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::Integer(_)
            | ExprKind::Float(_)
            | ExprKind::True
            | ExprKind::False
            | ExprKind::Void
            | ExprKind::Break
            | ExprKind::Continue => {}
            ExprKind::String(segments) => {
                for seg in segments {
                    if let StringSegment::Interp(e) = seg {
                        self.resolve_expr(e);
                    }
                }
            }
            ExprKind::Ident(ident) => self.resolve_reference(ident),
            ExprKind::Unary { expr, .. } => self.resolve_expr(expr),
            ExprKind::Binary { lhs, rhs, .. } => {
                self.resolve_expr(lhs);
                self.resolve_expr(rhs);
            }
            ExprKind::Paren(inner) => self.resolve_expr(inner),
            ExprKind::Domain { args, .. } => {
                for arg in args {
                    self.resolve_expr(arg);
                }
            }
            ExprKind::Block(block) => self.resolve_block(block),
            ExprKind::If {
                cond,
                then,
                else_branch,
            } => {
                self.resolve_expr(cond);
                self.resolve_block(then);
                if let Some(else_expr) = else_branch {
                    self.resolve_expr(else_expr);
                }
            }
            ExprKind::When { scrutinee, arms } => {
                self.resolve_expr(scrutinee);
                for arm in arms {
                    self.resolve_arm(arm);
                }
            }
            ExprKind::Assign { target, value } => {
                self.resolve_reference(target);
                self.resolve_expr(value);
            }
            ExprKind::Call { callee, args } => {
                self.resolve_expr(callee);
                for arg in args {
                    self.resolve_expr(arg);
                }
            }
            ExprKind::For { var, iter, body } => {
                self.resolve_expr(iter);
                self.push_scope();
                self.declare(var, DeclKind::ForVar);
                // 본문 블록은 이미 자체 스코프를 열지만, for 변수는 상위
                // 스코프(현재 push 한)에 속하므로 두 겹이 된다. 이는 블록
                // 마지막의 let 이 for 변수를 가리지 못하게 하는 자연스러운 결과.
                self.resolve_block(body);
                self.pop_scope();
            }
            ExprKind::While { cond, body } => {
                self.resolve_expr(cond);
                self.resolve_block(body);
            }
            ExprKind::Range { start, end, .. } => {
                self.resolve_expr(start);
                self.resolve_expr(end);
            }
            ExprKind::Array(items) => {
                for item in items {
                    self.resolve_expr(item);
                }
            }
            ExprKind::Object(fields) => {
                for field in fields {
                    self.resolve_object_field(field);
                }
            }
            ExprKind::Index { target, index } => {
                self.resolve_expr(target);
                self.resolve_expr(index);
            }
            ExprKind::Field { target, .. } => {
                // 필드 이름은 소유 구조에 따라 해석되므로 여기서는 대상만.
                self.resolve_expr(target);
            }
            ExprKind::Lambda { params, body } => {
                self.resolve_function_body(params, body);
            }
            ExprKind::Throw(inner) => self.resolve_expr(inner),
            ExprKind::Await(inner) => self.resolve_expr(inner),
            ExprKind::Try { try_block, catch } => {
                self.resolve_block(try_block);
                if let Some(clause) = catch {
                    self.resolve_catch(clause);
                }
            }
        }
    }

    fn resolve_arm(&mut self, arm: &WhenArm) {
        self.resolve_pattern(&arm.pattern);
        self.resolve_expr(&arm.body);
    }

    fn resolve_pattern(&mut self, pattern: &Pattern) {
        match pattern {
            Pattern::Wildcard => {}
            Pattern::Literal(expr) => self.resolve_expr(expr),
            Pattern::Range { start, end, .. } => {
                self.resolve_expr(start);
                self.resolve_expr(end);
            }
            Pattern::Guard(expr) => self.resolve_expr(expr),
            Pattern::Not(expr) => self.resolve_expr(expr),
            Pattern::Contains(expr) => self.resolve_expr(expr),
        }
    }

    fn resolve_object_field(&mut self, field: &ObjectField) {
        self.resolve_expr(&field.value);
    }

    fn resolve_catch(&mut self, clause: &CatchClause) {
        self.push_scope();
        if let Some(binding) = &clause.binding {
            self.declare(binding, DeclKind::Catch);
        }
        self.resolve_block(&clause.body);
        self.pop_scope();
    }

    fn is_declared_in_current(&self, name: &str) -> bool {
        self.scopes
            .last()
            .is_some_and(|scope| scope.bindings.contains_key(name))
    }

    fn record_use_in_current(&mut self, ident: &Ident) {
        if let Some(&id) = self
            .scopes
            .last()
            .and_then(|scope| scope.bindings.get(&ident.name))
        {
            self.name_of.insert(ident.span.into(), id);
        }
    }
}

fn undefined_diagnostic(ident: &Ident) -> Diagnostic {
    Diagnostic::error(format!("undefined variable `{}`", ident.name))
        .with_code("resolve/undefined")
        .with_primary(ident.span, "not found in scope")
}

#[cfg(test)]
mod tests {
    use super::*;
    use orv_syntax::{lex, parse};

    fn parse_source(src: &str) -> Program {
        let lex_result = lex(src, FileId(0));
        assert!(
            lex_result.diagnostics.is_empty(),
            "lex errors: {:?}",
            lex_result.diagnostics
        );
        let parsed = parse(lex_result.tokens, FileId(0));
        assert!(
            parsed.diagnostics.is_empty(),
            "parse errors: {:?}",
            parsed.diagnostics
        );
        parsed.program
    }

    fn resolve_ok(src: &str) -> ResolveResult {
        let program = parse_source(src);
        let result = resolve(&program);
        assert!(
            result.diagnostics.is_empty(),
            "unexpected diagnostics: {:?}",
            result.diagnostics
        );
        result
    }

    #[test]
    fn let_binding_and_use() {
        let result = resolve_ok("let x: int = 1\n@out x");
        // 선언 1개 (x), name_of 에는 선언 스팬과 참조 스팬 두 개가 기록된다.
        assert_eq!(result.decls.len(), 1);
        assert_eq!(result.decls[0].name, "x");
        assert_eq!(result.name_of.len(), 2);
    }

    #[test]
    fn function_parameters_scope() {
        let result = resolve_ok("function add(a: int, b: int): int -> a + b");
        // 함수 자체 + 파라미터 a + 파라미터 b = 3개 선언.
        assert_eq!(result.decls.len(), 3);
        let names: Vec<&str> = result.decls.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"add"));
        assert!(names.contains(&"a"));
        assert!(names.contains(&"b"));
    }

    #[test]
    fn undefined_variable_reports_diagnostic() {
        let program = parse_source("@out ghost");
        let result = resolve(&program);
        assert_eq!(result.diagnostics.len(), 1);
        assert!(result.diagnostics[0]
            .message
            .contains("undefined variable `ghost`"));
    }

    #[test]
    fn mutual_function_reference_allowed() {
        let src = "function a(): int -> b()\nfunction b(): int -> 1";
        let result = resolve_ok(src);
        // 각 함수 이름 1회 + b 참조가 a 의 본문에서 NameId 로 매핑돼야 한다.
        let b_refs: usize = result
            .decls
            .iter()
            .filter(|d| d.name == "b" && d.kind == DeclKind::Function)
            .count();
        assert_eq!(b_refs, 1);
    }

    #[test]
    fn block_shadowing() {
        let src = "let x: int = 1\n{ let x: int = 2\n@out x }\n@out x";
        let result = resolve_ok(src);
        // 바깥 x, 안쪽 x 두 선언이 있어야 한다.
        let x_count = result.decls.iter().filter(|d| d.name == "x").count();
        assert_eq!(x_count, 2);
    }

    #[test]
    fn for_loop_variable_scoped() {
        let src = "for i in 0..3 { @out i }";
        let result = resolve_ok(src);
        let i_decl = result
            .decls
            .iter()
            .find(|d| d.name == "i")
            .expect("for var declared");
        assert_eq!(i_decl.kind, DeclKind::ForVar);
    }

    #[test]
    fn for_variable_not_visible_after_loop() {
        let src = "for i in 0..3 { @out i }\n@out i";
        let program = parse_source(src);
        let result = resolve(&program);
        assert_eq!(
            result.diagnostics.len(),
            1,
            "second `i` should be undefined"
        );
    }

    #[test]
    fn lambda_captures_outer() {
        let src = "let n: int = 5\nlet f = () -> n + 1";
        let result = resolve_ok(src);
        // n 참조가 람다 안에서 바깥 선언으로 해결되는지.
        // 두 개의 n 스팬(선언 + 참조)이 같은 NameId.
        let n_ids: Vec<NameId> = result
            .name_of
            .values()
            .copied()
            .filter(|id| result.decls[id.0 as usize].name == "n")
            .collect();
        assert!(n_ids.len() >= 2);
        assert!(n_ids.windows(2).all(|w| w[0] == w[1]));
    }

    #[test]
    fn catch_binding_declared() {
        let src = "try { throw 1 } catch err { @out err }";
        let result = resolve_ok(src);
        let err_decl = result
            .decls
            .iter()
            .find(|d| d.name == "err")
            .expect("catch binding declared");
        assert_eq!(err_decl.kind, DeclKind::Catch);
    }

    #[test]
    fn dollar_in_when_guard_ignored() {
        let src = "when 5 { $ > 1 -> 1, _ -> 0 }";
        let program = parse_source(src);
        let result = resolve(&program);
        assert!(
            result.diagnostics.is_empty(),
            "$ should be treated as implicit scrutinee ref, not undefined: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn let_cannot_reference_itself_before_declaration() {
        // let x = x — 초기값 해석 시점에 x 는 아직 선언되지 않았다.
        let program = parse_source("let x: int = x");
        let result = resolve(&program);
        assert_eq!(result.diagnostics.len(), 1);
    }
}
