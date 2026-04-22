//! Parser — 토큰 스트림을 AST로 변환.
//!
//! 1차 구현은 `let`/`let mut`/`let sig`, `const`, 리터럴 표현식, 식별자
//! 참조, void scope 자동 출력 대상인 표현식 스테이트먼트까지를 다룬다.
//! 함수/제어 흐름/도메인/struct는 다음 커밋에서 추가된다.

use crate::ast::{
    BinaryOp, Block, CatchClause, ConstStmt, EnumStmt, EnumVariant, Expr, ExprKind, FunctionBody,
    FunctionStmt, Ident, ImportStmt, LetKind, LetStmt, ObjectField, Param, Pattern, Program,
    ReturnStmt, Stmt, StringSegment, StructField, StructStmt, TokenSlot, TypeRef, TypeRefKind,
    UnaryOp, WhenArm,
};
use crate::lexer::lex_with_base_offset;

/// Expr 하나를 즉시 평가되는 Block 으로 감싼다 — ternary 의 then 분기처럼
/// `If.then: Block` 을 요구하는 자리에서 사용. Expr kind 가 이미 Block 이면
/// 그대로 풀어낸다.
fn expr_to_block(e: Expr) -> Block {
    if let ExprKind::Block(b) = e.kind {
        return b;
    }
    let span = e.span;
    Block {
        stmts: vec![Stmt::Expr(e)],
        span,
    }
}
use crate::token::{Keyword, Token, TokenKind};
use orv_diagnostics::{ByteRange, Diagnostic, FileId, Span};

/// 파싱 결과 — AST와 수집된 진단.
#[derive(Debug)]
pub struct ParseResult {
    /// 파싱된 프로그램.
    pub program: Program,
    /// 에러/경고 진단.
    pub diagnostics: Vec<Diagnostic>,
}

/// 토큰 스트림을 받아 프로그램을 파싱한다.
#[must_use]
pub fn parse(tokens: Vec<Token>, file: FileId) -> ParseResult {
    parse_with_newlines(tokens, file, Vec::new())
}

/// [`parse`] 의 확장형 — 소스 내 개행 오프셋을 같이 받아 파서가 "같은 줄" 을
/// 판정할 수 있게 한다. 줄바꿈으로 문장이 나뉘는 paren-less 도메인 호출
/// (`@fs.write`, `@Auth` 등) 에서 다음 줄 stmt 를 잘못 인자로 흡수하는 문제를
/// 막기 위함.
#[must_use]
pub fn parse_with_newlines(tokens: Vec<Token>, file: FileId, newlines: Vec<u32>) -> ParseResult {
    let mut p = Parser::new(tokens, file, newlines);
    let items = p.parse_program();
    let span = p.file_span();
    ParseResult {
        program: Program { items, span },
        diagnostics: p.diagnostics,
    }
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    file: FileId,
    diagnostics: Vec<Diagnostic>,
    /// 소스 내 개행 위치(정렬된 바이트 오프셋). 비어 있으면 줄 정보 없음.
    newlines: Vec<u32>,
}

/// `function`/`define` 선언 파서 플래그.
///
/// `async`/`pub`/`define` modifier 들이 `parse_function` 한 자리에 모이므로
/// 각 조합을 bool 3 개로 전달한다.
#[derive(Default, Clone, Copy)]
struct FunctionFlags {
    is_async: bool,
    is_define: bool,
    is_pub: bool,
}

impl Parser {
    fn new(tokens: Vec<Token>, file: FileId, newlines: Vec<u32>) -> Self {
        Self {
            tokens,
            pos: 0,
            file,
            diagnostics: Vec::new(),
            newlines,
        }
    }

    /// `[a, b)` 범위에 개행이 포함돼 있으면 true.
    ///
    /// 파서의 `newlines` 가 비어 있으면 정보가 없어 항상 false — 기존
    /// 동작(같은 줄 가정) 을 유지한다. 이진 탐색으로 O(log n).
    fn has_newline_between(&self, a: u32, b: u32) -> bool {
        if a >= b || self.newlines.is_empty() {
            return false;
        }
        // `a..b` 에 걸치는 개행 오프셋이 하나라도 있는지.
        match self.newlines.binary_search(&a) {
            Ok(_) => true,
            Err(idx) => idx < self.newlines.len() && self.newlines[idx] < b,
        }
    }

    /// 이전 소비 토큰의 끝 오프셋과 현재 토큰의 시작 오프셋 사이에 개행이
    /// 있으면 true. 파서 내부 루프에서 "다음 줄로 넘어갔는지" 판정용.
    fn newline_before_cur(&self) -> bool {
        if self.pos == 0 {
            return false;
        }
        let prev_end = self.tokens[self.pos - 1].span.range.end;
        let cur_start = self.peek().span.range.start;
        self.has_newline_between(prev_end, cur_start)
    }

    // ── 커서 유틸 ──

    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn peek_kind(&self) -> &TokenKind {
        &self.peek().kind
    }

    fn advance(&mut self) -> Token {
        let tok = self.tokens[self.pos].clone();
        if !matches!(tok.kind, TokenKind::Eof) {
            self.pos += 1;
        }
        tok
    }

    fn eat(&mut self, kind: &TokenKind) -> bool {
        if self.peek_kind() == kind {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, kind: &TokenKind, what: &str) -> Option<Token> {
        if self.peek_kind() == kind {
            Some(self.advance())
        } else {
            self.error(format!(
                "expected {what}, found {}",
                describe(self.peek_kind())
            ));
            None
        }
    }

    fn error(&mut self, message: impl Into<String>) {
        let span = self.peek().span;
        self.diagnostics
            .push(Diagnostic::error(message).with_primary(span, ""));
    }

    fn file_span(&self) -> Span {
        let end = self
            .tokens
            .last()
            .map(|t| t.span.range.end)
            .unwrap_or_default();
        Span::new(self.file, ByteRange::new(0, end))
    }

    // ── 프로그램 ──

    fn parse_program(&mut self) -> Vec<Stmt> {
        let mut items = Vec::new();
        while !matches!(self.peek_kind(), TokenKind::Eof) {
            let start_pos = self.pos;
            match self.parse_stmt() {
                Some(s) => items.push(s),
                None => {
                    // 무한 루프 방지: 에러 후 한 토큰 이상 전진.
                    if self.pos == start_pos {
                        self.advance();
                    }
                }
            }
        }
        items
    }

    fn parse_stmt(&mut self) -> Option<Stmt> {
        match self.peek_kind() {
            TokenKind::Keyword(Keyword::Let) => self.parse_let().map(|s| Stmt::Let(Box::new(s))),
            TokenKind::Keyword(Keyword::Const) => {
                self.parse_const().map(|s| Stmt::Const(Box::new(s)))
            }
            TokenKind::Keyword(Keyword::Function) => self
                .parse_function(FunctionFlags::default())
                .map(|s| Stmt::Function(Box::new(s))),
            TokenKind::Keyword(Keyword::Async) => {
                // `async function ...` — Async modifier 소비 후 function 파서로.
                self.advance();
                if !matches!(self.peek_kind(), TokenKind::Keyword(Keyword::Function)) {
                    self.error("expected `function` after `async`");
                    return None;
                }
                self.parse_function(FunctionFlags {
                    is_async: true,
                    ..FunctionFlags::default()
                })
                .map(|s| Stmt::Function(Box::new(s)))
            }
            TokenKind::Keyword(Keyword::Define) => self
                .parse_function(FunctionFlags {
                    is_define: true,
                    ..FunctionFlags::default()
                })
                .map(|s| Stmt::Function(Box::new(s))),
            TokenKind::Keyword(Keyword::Pub) => {
                // `pub function`, `pub define`, `pub async function` 셋 모두 수용.
                self.advance();
                let mut flags = FunctionFlags {
                    is_pub: true,
                    ..FunctionFlags::default()
                };
                if matches!(self.peek_kind(), TokenKind::Keyword(Keyword::Async)) {
                    self.advance();
                    flags.is_async = true;
                    if !matches!(self.peek_kind(), TokenKind::Keyword(Keyword::Function)) {
                        self.error("expected `function` after `pub async`");
                        return None;
                    }
                }
                match self.peek_kind() {
                    TokenKind::Keyword(Keyword::Function) => {
                        self.parse_function(flags).map(|s| Stmt::Function(Box::new(s)))
                    }
                    TokenKind::Keyword(Keyword::Define) => {
                        flags.is_define = true;
                        self.parse_function(flags).map(|s| Stmt::Function(Box::new(s)))
                    }
                    // SPEC §8.2: `pub` 은 struct/enum/const 에도 적용 가능.
                    // MVP 는 visibility 플래그 저장을 생략하고 파싱만 허용
                    // — 현재 멀티파일 병합은 모든 top-level 선언을 export.
                    TokenKind::Keyword(Keyword::Struct) => self
                        .parse_struct_decl()
                        .map(|s| Stmt::Struct(Box::new(s))),
                    TokenKind::Keyword(Keyword::Const) => self
                        .parse_const()
                        .map(|s| Stmt::Const(Box::new(s))),
                    _ => {
                        self.error(
                            "expected `function`, `define`, `struct`, or `const` after `pub`",
                        );
                        None
                    }
                }
            }
            TokenKind::Keyword(Keyword::Struct) => {
                self.parse_struct_decl().map(|s| Stmt::Struct(Box::new(s)))
            }
            TokenKind::Keyword(Keyword::Enum) => self
                .parse_enum_decl()
                .map(|s| Stmt::Enum(Box::new(s))),
            TokenKind::Keyword(Keyword::Return) => self.parse_return().map(Stmt::Return),
            TokenKind::Keyword(Keyword::Import) => {
                self.parse_import().map(|s| Stmt::Import(Box::new(s)))
            }
            // SPEC §14.1 `test "name" { body }` — 즉시 실행되는 익명 블록으로
            //   desugar. runtime 에서 그냥 block 으로 실행한다. 실패(throw)는
            //   상위 try/catch 로.
            // SPEC §14.2 `assert expr` — `if !expr { throw "..." }` 로 desugar.
            TokenKind::Ident(n) if n == "test" => self.parse_test_stmt(),
            TokenKind::Ident(n) if n == "assert" => self.parse_assert_stmt(),
            // SPEC §7.3 `spawn { body }` — MVP 는 동기 실행. block 을 그대로
            // 표현식 stmt 로 래핑해 즉시 평가시킨다. 반환값은 없음 (Void).
            TokenKind::Ident(n) if n == "spawn" => self.parse_spawn_stmt(),
            _ => {
                // `ident = expr` 대입을 표현식 스테이트먼트로 인식.
                // Pratt 파서가 식별자를 먼저 먹은 뒤 `=`를 만나면 대입으로 전환.
                let expr = self.parse_expr()?;
                if matches!(self.peek_kind(), TokenKind::Eq) {
                    // 좌변 종류별 분기:
                    // - Ident: 기존 단순 대입.
                    // - Field: 필드 mutation → AssignField.
                    match &expr.kind {
                        ExprKind::Ident(_) => {}
                        ExprKind::Field { target, field } => {
                            let object = (**target).clone();
                            let fname = field.clone();
                            self.advance(); // `=`
                            let value = self.parse_expr()?;
                            let span = expr.span.join(value.span);
                            return Some(Stmt::Expr(Expr {
                                kind: ExprKind::AssignField {
                                    object: Box::new(object),
                                    field: fname,
                                    value: Box::new(value),
                                },
                                span,
                            }));
                        }
                        _ => {
                            self.error(
                                "assignment target must be an identifier or `obj.field`",
                            );
                            return Some(Stmt::Expr(expr));
                        }
                    }
                    let ExprKind::Ident(ref id) = expr.kind else {
                        unreachable!("handled above");
                    };
                    let target = id.clone();
                    self.advance(); // `=`
                    let value = self.parse_expr()?;
                    let span = expr.span.join(value.span);
                    return Some(Stmt::Expr(Expr {
                        kind: ExprKind::Assign {
                            target,
                            value: Box::new(value),
                        },
                        span,
                    }));
                }
                Some(Stmt::Expr(expr))
            }
        }
    }

    // ── let/const ──

    fn parse_let(&mut self) -> Option<LetStmt> {
        let let_tok = self.advance(); // `let`
        let kind = match self.peek_kind() {
            TokenKind::Keyword(Keyword::Mut) => {
                self.advance();
                LetKind::Mutable
            }
            TokenKind::Keyword(Keyword::Sig) => {
                self.advance();
                LetKind::Signal
            }
            _ => LetKind::Immutable,
        };
        let name = self.parse_ident("variable name")?;
        let ty = if self.eat(&TokenKind::Colon) {
            Some(self.parse_type()?)
        } else {
            None
        };
        self.expect(&TokenKind::Eq, "`=`")?;
        let init = self.parse_expr()?;
        let span = let_tok.span.join(init.span);
        Some(LetStmt {
            kind,
            name,
            ty,
            init,
            span,
        })
    }

    fn parse_const(&mut self) -> Option<ConstStmt> {
        let const_tok = self.advance(); // `const`
        let name = self.parse_ident("constant name")?;
        let ty = if self.eat(&TokenKind::Colon) {
            Some(self.parse_type()?)
        } else {
            None
        };
        self.expect(&TokenKind::Eq, "`=`")?;
        let init = self.parse_expr()?;
        let span = const_tok.span.join(init.span);
        Some(ConstStmt {
            name,
            ty,
            init,
            span,
        })
    }

    // ── 식별자 / 타입 ──

    fn parse_ident(&mut self, what: &str) -> Option<Ident> {
        match self.peek_kind().clone() {
            TokenKind::Ident(name) => {
                let tok = self.advance();
                Some(Ident {
                    name,
                    span: tok.span,
                })
            }
            _ => {
                self.error(format!(
                    "expected {what}, found {}",
                    describe(self.peek_kind())
                ));
                None
            }
        }
    }

    fn parse_type(&mut self) -> Option<TypeRef> {
        // SPEC §4.1: `void` 는 원시 타입이지만 토큰이 예약어라 일반 ident
        // 경로로는 잡히지 않는다. 타입 자리에서만 Keyword::Void 를 Named("void")
        // 으로 합성해 받아들인다 — 표현식 자리의 `void` 리터럴은 그대로 유지.
        let name = if matches!(self.peek_kind(), TokenKind::Keyword(Keyword::Void)) {
            let tok = self.advance();
            Ident {
                name: "void".to_string(),
                span: tok.span,
            }
        } else {
            self.parse_ident("type name")?
        };
        let mut ty = TypeRef {
            span: name.span,
            kind: TypeRefKind::Named(name),
        };
        // SPEC §4.7 generic 타입 인자 `T<U, V>` — MVP 는 소비만, 타입 값은
        // Named(...) 그대로 유지한다. 실제 parameterization 은 후속 stage.
        if matches!(self.peek_kind(), TokenKind::Lt) {
            self.advance(); // `<`
            loop {
                if matches!(self.peek_kind(), TokenKind::Gt | TokenKind::Eof) {
                    break;
                }
                // 중첩 generic 도 재귀 수용.
                let _ = self.parse_type();
                if matches!(self.peek_kind(), TokenKind::Comma) {
                    self.advance();
                }
            }
            let _ = self.eat(&TokenKind::Gt);
        }
        // SPEC §4.10 스키마 제약 `string(3..50)`, `int(min=1, max=99)` 등.
        // MVP 는 토큰만 소비 — 실제 검증은 후속 stage. `(` 로 시작하는 모든
        // 제약 목록을 `)` 까지 삼킨다. depth counting 으로 중첩 paren 도 처리.
        if matches!(self.peek_kind(), TokenKind::LParen) {
            let mut depth = 0i32;
            loop {
                match self.peek_kind() {
                    TokenKind::LParen => {
                        depth += 1;
                        self.advance();
                    }
                    TokenKind::RParen => {
                        depth -= 1;
                        self.advance();
                        if depth == 0 {
                            break;
                        }
                    }
                    TokenKind::Eof => break,
                    _ => {
                        self.advance();
                    }
                }
            }
        }
        // 접미사 — `?` (nullable), `[]` (array). 순서/반복 자유.
        loop {
            if self.eat(&TokenKind::Question) {
                let span = ty.span;
                ty = TypeRef {
                    span,
                    kind: TypeRefKind::Nullable(Box::new(ty)),
                };
            } else if matches!(self.peek_kind(), TokenKind::LBracket) {
                // `[` 다음에 `]`가 바로 오는 경우만 타입 `T[]`로 소비한다.
                // 인덱싱과 구분하기 위해 안쪽이 비어 있어야 한다.
                let next = self.tokens.get(self.pos + 1).map(|t| &t.kind);
                if matches!(next, Some(TokenKind::RBracket)) {
                    self.advance(); // `[`
                    self.advance(); // `]`
                    let span = ty.span;
                    ty = TypeRef {
                        span,
                        kind: TypeRefKind::Array(Box::new(ty)),
                    };
                } else {
                    break;
                }
            } else {
                break;
            }
        }
        Some(ty)
    }

    // ── 표현식 (Pratt) ──

    fn parse_expr(&mut self) -> Option<Expr> {
        self.parse_expr_bp(0)
    }

    /// SPEC §6.2 삼항의 한 분기 파싱. `{` 이면 block expression, 아니면 일반
    /// 표현식 하나. ternary 경계를 명확히 하기 위해 bp 3 이상 (coalesce / ternary
    /// 제외) 부터 허용해 바깥쪽 `:` 를 삼키지 않는다.
    fn parse_ternary_branch(&mut self) -> Option<Expr> {
        if matches!(self.peek_kind(), TokenKind::LBrace) {
            self.parse_block_expr()
        } else {
            self.parse_expr_bp(3)
        }
    }

    /// binding power(bp) 기반 Pratt parser.
    /// `min_bp` 이상의 좌결합 연산자만 소비한다.
    fn parse_expr_bp(&mut self, min_bp: u8) -> Option<Expr> {
        let mut lhs = self.parse_prefix()?;

        loop {
            // 후위 연산자 — 함수 호출 `(args)`.
            if matches!(self.peek_kind(), TokenKind::LParen) && 30 >= min_bp {
                lhs = self.finish_call(lhs)?;
                continue;
            }
            // 후위 연산자 — 인덱스 `[idx]` 또는 슬라이스 `[a:b]` / `[:b]` /
            // `[a:]` / `[:]`. `[` 뒤 첫 토큰이 `:` 면 start 생략, 아니면
            // 표현식을 파싱 후 다음 토큰이 `:` 이면 슬라이스로 승격한다.
            if matches!(self.peek_kind(), TokenKind::LBracket) && 30 >= min_bp {
                self.advance();
                let start = if matches!(self.peek_kind(), TokenKind::Colon) {
                    None
                } else {
                    Some(self.parse_expr()?)
                };
                if matches!(self.peek_kind(), TokenKind::Colon) {
                    self.advance(); // `:`
                    let end = if matches!(self.peek_kind(), TokenKind::RBracket) {
                        None
                    } else {
                        Some(self.parse_expr()?)
                    };
                    let rbracket = self.expect(&TokenKind::RBracket, "`]`")?;
                    let span = lhs.span.join(rbracket.span);
                    lhs = Expr {
                        kind: ExprKind::Slice {
                            target: Box::new(lhs),
                            start: start.map(Box::new),
                            end: end.map(Box::new),
                        },
                        span,
                    };
                } else {
                    let Some(idx) = start else {
                        // `[]` 빈 인덱스는 허용되지 않는다.
                        self.error("empty index `[]` is not allowed");
                        return None;
                    };
                    let rbracket = self.expect(&TokenKind::RBracket, "`]`")?;
                    let span = lhs.span.join(rbracket.span);
                    lhs = Expr {
                        kind: ExprKind::Index {
                            target: Box::new(lhs),
                            index: Box::new(idx),
                        },
                        span,
                    };
                }
                continue;
            }
            // 후위 연산자 — 필드 `.field`.
            if matches!(self.peek_kind(), TokenKind::Dot) && 30 >= min_bp {
                self.advance();
                let field = self.parse_ident("field name")?;
                let span = lhs.span.join(field.span);
                lhs = Expr {
                    kind: ExprKind::Field {
                        target: Box::new(lhs),
                        field,
                    },
                    span,
                };
                continue;
            }
            // SPEC §4.9 `expr as <type>` — 타입 캐스팅. 후위 연산자로 처리해
            // `a + b as int` 는 `a + (b as int)` 로, `x as int + 1` 은
            // `(x as int) + 1` 로 해석된다 (Rust 와 동일).
            if matches!(self.peek_kind(), TokenKind::Keyword(Keyword::As)) && 22 >= min_bp {
                self.advance(); // `as`
                let ty = self.parse_type()?;
                let span = lhs.span.join(ty.span);
                lhs = Expr {
                    kind: ExprKind::Cast {
                        expr: Box::new(lhs),
                        ty,
                    },
                    span,
                };
                continue;
            }

            // 범위 연산자 `..`, `..=` — 특수 AST 노드 Range로.
            // bp는 비교 연산자와 산술 사이에 둔다 (SPEC §2.5).
            if matches!(self.peek_kind(), TokenKind::DotDot | TokenKind::DotDotEq) && 15 >= min_bp {
                let inclusive = matches!(self.peek_kind(), TokenKind::DotDotEq);
                self.advance();
                let rhs = self.parse_expr_bp(16)?;
                let span = lhs.span.join(rhs.span);
                lhs = Expr {
                    kind: ExprKind::Range {
                        start: Box::new(lhs),
                        end: Box::new(rhs),
                        inclusive,
                    },
                    span,
                };
                continue;
            }

            // SPEC §6.2 삼항: `cond ? A : B`. A/B 는 block or expression.
            // bp=2 — coalesce(1) 보다 조금 높게, logical or(2/3) 와 비슷한 수준.
            // min_bp 가 3 이상이면 skip.
            if matches!(self.peek_kind(), TokenKind::Question) && 2 >= min_bp {
                self.advance(); // `?`
                let then_branch = self.parse_ternary_branch()?;
                self.expect(&TokenKind::Colon, "`:`")?;
                let else_branch = self.parse_ternary_branch()?;
                let span = lhs.span.join(else_branch.span);
                let then_block = expr_to_block(then_branch);
                let else_expr = Box::new(else_branch);
                lhs = Expr {
                    kind: ExprKind::If {
                        cond: Box::new(lhs),
                        then: then_block,
                        else_branch: Some(else_expr),
                    },
                    span,
                };
                continue;
            }

            let Some((op, lbp, rbp)) = self.peek_binop() else {
                break;
            };
            if lbp < min_bp {
                break;
            }
            self.advance();
            let rhs = self.parse_expr_bp(rbp)?;
            let span = lhs.span.join(rhs.span);
            lhs = Expr {
                kind: ExprKind::Binary {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                span,
            };
        }
        Some(lhs)
    }

    /// `{` 뒤에 `ident :` 또는 `}`가 바로 오면 객체 리터럴.
    fn looks_like_object_literal(&self) -> bool {
        let after_lbrace = self.tokens.get(self.pos + 1).map(|t| &t.kind);
        match after_lbrace {
            Some(TokenKind::RBrace) => true,
            // SPEC §2.5: `{...expr}` spread 시작도 object literal.
            Some(TokenKind::DotDotDot) => true,
            Some(TokenKind::Ident(_)) => matches!(
                self.tokens.get(self.pos + 2).map(|t| &t.kind),
                Some(TokenKind::Colon)
            ),
            _ => false,
        }
    }

    fn parse_object_literal(&mut self) -> Option<Expr> {
        let lbrace = self.advance(); // `{`
        let mut fields = Vec::new();
        while !matches!(self.peek_kind(), TokenKind::RBrace | TokenKind::Eof) {
            // SPEC §2.5 spread — `{ ...base, key: value }` 형태.
            if matches!(self.peek_kind(), TokenKind::DotDotDot) {
                let dotdotdot = self.advance();
                let value = self.parse_expr()?;
                let span = dotdotdot.span.join(value.span);
                let sentinel = Ident {
                    name: "__spread__".to_string(),
                    span: dotdotdot.span,
                };
                fields.push(ObjectField {
                    name: sentinel,
                    value,
                    is_spread: true,
                    span,
                });
                if matches!(self.peek_kind(), TokenKind::Comma) {
                    self.advance();
                }
                continue;
            }
            let name = self.parse_ident("field name")?;
            self.expect(&TokenKind::Colon, "`:`")?;
            let value = self.parse_expr()?;
            let span = name.span.join(value.span);
            fields.push(ObjectField {
                name,
                value,
                is_spread: false,
                span,
            });
            if matches!(self.peek_kind(), TokenKind::Comma) {
                self.advance();
            }
        }
        let rbrace = self.expect(&TokenKind::RBrace, "`}`")?;
        Some(Expr {
            kind: ExprKind::Object(fields),
            span: lbrace.span.join(rbrace.span),
        })
    }

    /// `(` 뒤가 람다의 파라미터 목록처럼 보이는지 — `()` 또는
    /// `(ident ... ) ->` 패턴.
    fn looks_like_lambda(&self) -> bool {
        // 빈 파라미터 `() ->`
        let after = self.tokens.get(self.pos + 1).map(|t| &t.kind);
        if matches!(after, Some(TokenKind::RParen)) {
            return matches!(
                self.tokens.get(self.pos + 2).map(|t| &t.kind),
                Some(TokenKind::Arrow)
            );
        }
        // `(ident ...)` 탐색 — 중첩 괄호 없이 매칭되는 `)` 이후 `->` 여부.
        if !matches!(after, Some(TokenKind::Ident(_))) {
            return false;
        }
        let mut depth = 1i32;
        let mut i = self.pos + 1;
        while let Some(tok) = self.tokens.get(i) {
            match &tok.kind {
                TokenKind::LParen => depth += 1,
                TokenKind::RParen => {
                    depth -= 1;
                    if depth == 0 {
                        return matches!(
                            self.tokens.get(i + 1).map(|t| &t.kind),
                            Some(TokenKind::Arrow)
                        );
                    }
                }
                TokenKind::Eof | TokenKind::LBrace | TokenKind::RBrace => return false,
                _ => {}
            }
            i += 1;
        }
        false
    }

    fn parse_lambda(&mut self) -> Option<Expr> {
        let lparen = self.advance(); // `(`
        let mut params = Vec::new();
        while !matches!(self.peek_kind(), TokenKind::RParen | TokenKind::Eof) {
            let name = self.parse_ident("parameter name")?;
            let ty = if self.eat(&TokenKind::Colon) {
                Some(self.parse_type()?)
            } else {
                None
            };
            let span = name.span;
            params.push(Param { name, ty, span });
            if matches!(self.peek_kind(), TokenKind::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        self.expect(&TokenKind::RParen, "`)`")?;
        self.expect(&TokenKind::Arrow, "`->`")?;
        let body = if matches!(self.peek_kind(), TokenKind::LBrace) {
            FunctionBody::Block(self.parse_block()?)
        } else {
            FunctionBody::Expr(self.parse_expr()?)
        };
        let end_span = match &body {
            FunctionBody::Block(b) => b.span,
            FunctionBody::Expr(e) => e.span,
        };
        Some(Expr {
            kind: ExprKind::Lambda {
                params,
                body: Box::new(body),
            },
            span: lparen.span.join(end_span),
        })
    }

    fn parse_try(&mut self) -> Option<Expr> {
        let try_tok = self.advance(); // `try`
        let try_block = self.parse_block()?;
        let (catch, end_span) = if matches!(self.peek_kind(), TokenKind::Keyword(Keyword::Catch)) {
            let catch_tok = self.advance();
            let binding = if matches!(self.peek_kind(), TokenKind::Ident(_)) {
                Some(self.parse_ident("error binding")?)
            } else {
                None
            };
            let ty = if self.eat(&TokenKind::Colon) {
                Some(self.parse_type()?)
            } else {
                None
            };
            let body = self.parse_block()?;
            let span = catch_tok.span.join(body.span);
            let body_span = body.span;
            (
                Some(CatchClause {
                    binding,
                    ty,
                    body,
                    span,
                }),
                body_span,
            )
        } else {
            (None, try_block.span)
        };
        Some(Expr {
            kind: ExprKind::Try { try_block, catch },
            span: try_tok.span.join(end_span),
        })
    }

    fn parse_array_literal(&mut self) -> Option<Expr> {
        let lbracket = self.advance(); // `[`
        let mut elems = Vec::new();
        while !matches!(self.peek_kind(), TokenKind::RBracket | TokenKind::Eof) {
            let e = self.parse_expr()?;
            elems.push(e);
            if matches!(self.peek_kind(), TokenKind::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        let rbracket = self.expect(&TokenKind::RBracket, "`]`")?;
        Some(Expr {
            kind: ExprKind::Array(elems),
            span: lbracket.span.join(rbracket.span),
        })
    }

    /// `callee` 다음에 `(` 가 확인된 상태에서 호출식을 완성한다.
    fn finish_call(&mut self, callee: Expr) -> Option<Expr> {
        self.advance(); // `(`
        let mut args = Vec::new();
        while !matches!(self.peek_kind(), TokenKind::RParen | TokenKind::Eof) {
            let arg = self.parse_expr()?;
            args.push(arg);
            if matches!(self.peek_kind(), TokenKind::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        let rparen = self.expect(&TokenKind::RParen, "`)`")?;
        let span = callee.span.join(rparen.span);
        Some(Expr {
            kind: ExprKind::Call {
                callee: Box::new(callee),
                args,
            },
            span,
        })
    }

    fn parse_prefix(&mut self) -> Option<Expr> {
        let start_tok = self.peek().clone();
        // 전위 단항 `!x`, `-x`, `~x`
        let unary = match &start_tok.kind {
            TokenKind::Bang => Some(UnaryOp::Not),
            TokenKind::Minus => Some(UnaryOp::Neg),
            TokenKind::Tilde => Some(UnaryOp::BitNot),
            _ => None,
        };
        if let Some(op) = unary {
            self.advance();
            let expr = self.parse_prefix()?;
            let span = start_tok.span.join(expr.span);
            return Some(Expr {
                kind: ExprKind::Unary {
                    op,
                    expr: Box::new(expr),
                },
                span,
            });
        }
        self.parse_atom()
    }

    fn parse_atom(&mut self) -> Option<Expr> {
        let tok = self.peek().clone();
        let kind = match &tok.kind {
            TokenKind::Integer(s) => {
                self.advance();
                ExprKind::Integer(s.clone())
            }
            TokenKind::Float(s) => {
                self.advance();
                ExprKind::Float(s.clone())
            }
            TokenKind::String(s) => {
                let raw = s.clone();
                let str_tok = self.advance();
                let segments = self.parse_string_segments(&raw, str_tok.span);
                return Some(Expr {
                    kind: ExprKind::String(segments),
                    span: str_tok.span,
                });
            }
            TokenKind::True => {
                self.advance();
                ExprKind::True
            }
            TokenKind::False => {
                self.advance();
                ExprKind::False
            }
            TokenKind::Keyword(Keyword::Void) => {
                self.advance();
                ExprKind::Void
            }
            TokenKind::Ident(name) => {
                let name_s = name.clone();
                let ident_tok = self.advance();
                ExprKind::Ident(Ident {
                    name: name_s,
                    span: ident_tok.span,
                })
            }
            TokenKind::LParen => {
                // 람다 리터럴 vs 괄호 표현식 구분:
                //   `()` 또는 `(ident [:, ])` 패턴은 람다로 시도.
                if self.looks_like_lambda() {
                    return self.parse_lambda();
                }
                let lparen = self.advance();
                let inner = self.parse_expr()?;
                let rparen = self.expect(&TokenKind::RParen, "`)`")?;
                let span = lparen.span.join(rparen.span);
                return Some(Expr {
                    kind: ExprKind::Paren(Box::new(inner)),
                    span,
                });
            }
            TokenKind::Keyword(Keyword::Throw) => {
                let throw_tok = self.advance();
                let expr = self.parse_expr()?;
                let span = throw_tok.span.join(expr.span);
                return Some(Expr {
                    kind: ExprKind::Throw(Box::new(expr)),
                    span,
                });
            }
            TokenKind::Keyword(Keyword::Await) => {
                let await_tok = self.advance();
                // B2 MVP: identity — 피연산자를 평가해 그대로 반환. 다만
                // precedence 는 prefix unary 와 같아야 하므로, 전체 식이 아니라
                // "postfix 체인까지 포함한 단항 피연산자" 만 소비한다.
                //
                // `await outer().x`  -> Await(Field(Call(outer), x))
                // `-await 1 + 2`     -> Binary(Add, Unary(Neg, Await(1)), 2)
                //
                // 현재 Pratt 구현에서 postfix call/field/index 의 bp 가 30 이라
                // 그 이상(min_bp=30)까지만 읽으면 binary 연산자는 바깥 루프로
                // 남기고 postfix 만 피연산자 안에 포함시킬 수 있다.
                let expr = self.parse_expr_bp(30)?;
                let span = await_tok.span.join(expr.span);
                return Some(Expr {
                    kind: ExprKind::Await(Box::new(expr)),
                    span,
                });
            }
            TokenKind::Keyword(Keyword::Try) => return self.parse_try(),
            TokenKind::LBracket => return self.parse_array_literal(),
            TokenKind::LBrace => {
                // 객체 리터럴 vs 블록 구분:
                // `{` 다음이 `ident :` 또는 `}`(빈 객체)이면 객체 리터럴.
                // 그 외에는 블록 표현식.
                if self.looks_like_object_literal() {
                    return self.parse_object_literal();
                }
                return self.parse_block_expr();
            }
            TokenKind::Keyword(Keyword::If) => return self.parse_if(),
            TokenKind::Keyword(Keyword::When) => return self.parse_when(),
            TokenKind::Keyword(Keyword::For) => return self.parse_for(),
            TokenKind::Keyword(Keyword::While) => return self.parse_while(),
            TokenKind::Keyword(Keyword::Break) => {
                let t = self.advance();
                return Some(Expr {
                    kind: ExprKind::Break,
                    span: t.span,
                });
            }
            TokenKind::Keyword(Keyword::Continue) => {
                let t = self.advance();
                return Some(Expr {
                    kind: ExprKind::Continue,
                    span: t.span,
                });
            }
            TokenKind::At(_) => return self.parse_domain_call(),
            TokenKind::Dollar => {
                // `$`는 when 가드 내 현재 값 참조(SPEC §4.10, §6.3).
                // 식별자 `$`로 취급해 환경 조회로 풀어낸다.
                let dollar_tok = self.advance();
                ExprKind::Ident(Ident {
                    name: "$".to_string(),
                    span: dollar_tok.span,
                })
            }
            _ => {
                self.error(format!(
                    "expected expression, found {}",
                    describe(self.peek_kind())
                ));
                return None;
            }
        };
        Some(Expr {
            kind,
            span: tok.span,
        })
    }

    fn parse_function(&mut self, flags: FunctionFlags) -> Option<FunctionStmt> {
        let fn_tok = self.advance(); // `function` or `define`
        let name = self.parse_ident("function name")?;
        // SPEC §4.7 generic: `function id<T>(...)`. MVP 는 파서만 — 파라미터
        // 이름은 수집하지만 runtime/type-check 에 반영하지 않는다. 구조적 노이즈
        // 없이 사양 예제 파싱을 통과시키는 것이 목적.
        self.skip_generic_params();
        self.expect(&TokenKind::LParen, "`(`")?;
        let mut params = Vec::new();
        while !matches!(self.peek_kind(), TokenKind::RParen | TokenKind::Eof) {
            let pname = self.parse_ident("parameter name")?;
            let ty = if self.eat(&TokenKind::Colon) {
                Some(self.parse_type()?)
            } else {
                None
            };
            let span = pname.span;
            params.push(Param {
                name: pname,
                ty,
                span,
            });
            // SPEC §5.1 + §9.3 멀티라인 signature: `,` 는 옵션. 현재 lexer 는
            // 줄바꿈을 토큰으로 방출하지 않으므로 다음이 ident 면 계속, `)` 면
            // 종료. `,` 가 명시되면 소비만 하고 구분자 역할.
            if matches!(self.peek_kind(), TokenKind::Comma) {
                self.advance();
                continue;
            }
            if matches!(self.peek_kind(), TokenKind::Ident(_)) {
                continue;
            }
            break;
        }
        self.expect(&TokenKind::RParen, "`)`")?;
        let return_ty = if self.eat(&TokenKind::Colon) {
            Some(self.parse_type()?)
        } else {
            None
        };
        self.expect(&TokenKind::Arrow, "`->`")?;
        let (body, token_slots) = if matches!(self.peek_kind(), TokenKind::LBrace) {
            let (block, slots) = self.parse_function_block_with_token_slots(flags.is_define)?;
            (FunctionBody::Block(block), slots)
        } else {
            (FunctionBody::Expr(self.parse_expr()?), Vec::new())
        };
        let end_span = match &body {
            FunctionBody::Block(b) => b.span,
            FunctionBody::Expr(e) => e.span,
        };
        Some(FunctionStmt {
            name,
            params,
            return_ty,
            body,
            is_async: flags.is_async,
            is_define: flags.is_define,
            is_pub: flags.is_pub,
            token_slots,
            span: fn_tok.span.join(end_span),
        })
    }

    /// SPEC §9.4: define body 진입 직후 `token name: T` 또는 `token { ... }`
    /// 선언을 소비한다. 일반 `function` 이면 감지해도 오류를 내기보다는
    /// slot 으로 누적만 하고 runtime 이 noop 로 처리한다 (MVP 관용).
    ///
    /// 반환: (나머지 stmts 로 구성된 body block, 수집된 token slot 들).
    /// `is_define` 이 false 면 slot 은 빈 벡터로 강제 — 일반 함수 body 의
    /// `token` 식별자는 평범한 참조로 남는다.
    fn parse_function_block_with_token_slots(
        &mut self,
        is_define: bool,
    ) -> Option<(Block, Vec<TokenSlot>)> {
        let lbrace = self.expect(&TokenKind::LBrace, "`{`")?;
        let mut token_slots = Vec::new();
        if is_define {
            // `token` 은 contextual keyword — 일반 ident 지만 function body
            // 최상단에서만 slot 선언의 시작으로 해석된다. slot 이 아닌 stmt 가
            // 한 번 나오면 그 뒤의 `token` 은 평범한 식별자 참조.
            loop {
                if !self.peek_is_token_keyword() {
                    break;
                }
                match self.parse_token_slot() {
                    Some(mut slots) => token_slots.append(&mut slots),
                    None => break,
                }
            }
        }
        let mut stmts = Vec::new();
        while !matches!(self.peek_kind(), TokenKind::RBrace | TokenKind::Eof) {
            let start_pos = self.pos;
            match self.parse_stmt() {
                Some(s) => stmts.push(s),
                None => {
                    if self.pos == start_pos {
                        self.advance();
                    }
                }
            }
        }
        let rbrace = self.expect(&TokenKind::RBrace, "`}`")?;
        let span = lbrace.span.join(rbrace.span);
        Some((Block { stmts, span }, token_slots))
    }

    /// `token` 식별자 시작 여부. 렉서가 contextual keyword 를 모르므로
    /// `TokenKind::Ident("token")` 체크.
    fn peek_is_token_keyword(&self) -> bool {
        matches!(self.peek_kind(), TokenKind::Ident(s) if s == "token")
    }

    /// SPEC §9.4 token slot 선언을 파싱한다.
    ///
    /// 두 형태:
    /// - `token name: T` — 단일 slot
    /// - `token { name: T, ... }` — block, 여러 slot 을 한 번에
    ///
    /// 파싱 실패 (형식 불일치) 시 토큰을 복원하지 않고 None 반환.
    fn parse_token_slot(&mut self) -> Option<Vec<TokenSlot>> {
        // contextual keyword 'token' 소비.
        let token_tok = self.advance();
        let start_span = token_tok.span;
        if matches!(self.peek_kind(), TokenKind::LBrace) {
            self.advance(); // `{`
            let mut slots = Vec::new();
            while !matches!(self.peek_kind(), TokenKind::RBrace | TokenKind::Eof) {
                let name = self.parse_ident("token slot name")?;
                self.expect(&TokenKind::Colon, "`:`")?;
                let ty = self.parse_type()?;
                let span = name.span.join(ty.span);
                slots.push(TokenSlot { name, ty, span });
                if matches!(self.peek_kind(), TokenKind::Comma) {
                    self.advance();
                }
            }
            let _ = self.expect(&TokenKind::RBrace, "`}`")?;
            Some(slots)
        } else {
            let name = self.parse_ident("token slot name")?;
            self.expect(&TokenKind::Colon, "`:`")?;
            let ty = self.parse_type()?;
            let span = start_span.join(ty.span);
            Some(vec![TokenSlot { name, ty, span }])
        }
    }

    /// SPEC §4.4 `enum Name { V1 = expr, V2 = expr, ... }`.
    fn parse_enum_decl(&mut self) -> Option<EnumStmt> {
        let enum_tok = self.advance(); // `enum`
        let name = self.parse_ident("enum name")?;
        self.expect(&TokenKind::LBrace, "`{`")?;
        let mut variants = Vec::new();
        while !matches!(self.peek_kind(), TokenKind::RBrace | TokenKind::Eof) {
            let vname = self.parse_ident("enum variant name")?;
            // `= expr` 값 지정. SPEC 예제는 모두 명시적 값.
            let value = if self.eat(&TokenKind::Eq) {
                self.parse_expr()?
            } else {
                // MVP: 값 생략 시 Void 리터럴 — auto-increment 는 후속.
                Expr {
                    kind: ExprKind::Void,
                    span: vname.span,
                }
            };
            let vspan = vname.span.join(value.span);
            variants.push(EnumVariant {
                name: vname,
                value,
                span: vspan,
            });
            // `,` 선택 — newline 구분자도 허용.
            if matches!(self.peek_kind(), TokenKind::Comma) {
                self.advance();
            }
        }
        let rbrace = self.expect(&TokenKind::RBrace, "`}`")?;
        Some(EnumStmt {
            name,
            variants,
            span: enum_tok.span.join(rbrace.span),
        })
    }

    /// SPEC §4.7 generic 파라미터 `<T, U>` 를 소비한다. 이름은 수집하지 않으며
    /// runtime 에 영향 주지 않는다 — 파서 수용성만 제공하는 MVP 스텁.
    fn skip_generic_params(&mut self) {
        if !matches!(self.peek_kind(), TokenKind::Lt) {
            return;
        }
        self.advance(); // `<`
        loop {
            if matches!(self.peek_kind(), TokenKind::Gt | TokenKind::Eof) {
                break;
            }
            // 각 segment 는 ident (간단 MVP — bound/default 는 후속).
            match self.peek_kind() {
                TokenKind::Ident(_) => {
                    self.advance();
                }
                _ => {
                    // 알 수 없는 토큰이 와도 일단 소비해 무한루프 방지.
                    self.advance();
                }
            }
            if matches!(self.peek_kind(), TokenKind::Comma) {
                self.advance();
            }
        }
        let _ = self.eat(&TokenKind::Gt);
    }

    fn parse_struct_decl(&mut self) -> Option<StructStmt> {
        let struct_tok = self.advance(); // `struct`
        let name = self.parse_ident("struct name")?;
        // SPEC §4.7: `struct Box<T>` generic 수용.
        self.skip_generic_params();
        self.expect(&TokenKind::LBrace, "`{`")?;
        let mut fields = Vec::new();
        while !matches!(self.peek_kind(), TokenKind::RBrace | TokenKind::Eof) {
            let fname = self.parse_ident("field name")?;
            self.expect(&TokenKind::Colon, "`:`")?;
            let ty = self.parse_type()?;
            let span = fname.span.join(ty.span);
            fields.push(StructField {
                name: fname,
                ty,
                span,
            });
            // `,` 또는 줄바꿈(현재 lexer 미지원 — 다음 필드 이름 또는 `}`로 판별).
            if matches!(self.peek_kind(), TokenKind::Comma) {
                self.advance();
            }
        }
        let rbrace = self.expect(&TokenKind::RBrace, "`}`")?;
        Some(StructStmt {
            name,
            fields,
            span: struct_tok.span.join(rbrace.span),
        })
    }

    /// SPEC §7.3 `spawn { body }` — MVP 는 block 으로 desugar. 실제 스레드/태스크
    /// 생성은 후속. handle 반환 없이 동기 실행.
    fn parse_spawn_stmt(&mut self) -> Option<Stmt> {
        let _spawn_tok = self.advance();
        let block = self.parse_block()?;
        let span = block.span;
        Some(Stmt::Expr(Expr {
            kind: ExprKind::Block(block),
            span,
        }))
    }

    /// SPEC §14.1 `test "name" { body }` — 즉시 실행되는 block 으로 desugar.
    fn parse_test_stmt(&mut self) -> Option<Stmt> {
        let test_tok = self.advance(); // `test`
        // name string
        let name_tok = self.advance();
        let TokenKind::String(_) = &name_tok.kind else {
            self.error("expected string after `test`");
            return None;
        };
        let block = self.parse_block()?;
        let block_span = block.span;
        // `test "name" { body }` 를 실행-가능한 block expr 로 desugar.
        // 이름은 진단에 유용하지만 runtime 는 현재 무시 — 후속 stage 에서
        // registry 추가.
        let _ = test_tok;
        Some(Stmt::Expr(Expr {
            kind: ExprKind::Block(block),
            span: block_span,
        }))
    }

    /// SPEC §14.2 `assert expr` — `if !expr { throw "assertion failed" }` 로
    /// desugar.
    fn parse_assert_stmt(&mut self) -> Option<Stmt> {
        let assert_tok = self.advance(); // `assert`
        let cond = self.parse_expr()?;
        let cond_span = cond.span;
        // `!cond`
        let negated = Expr {
            kind: ExprKind::Unary {
                op: crate::ast::UnaryOp::Not,
                expr: Box::new(cond),
            },
            span: cond_span,
        };
        let throw_expr = Expr {
            kind: ExprKind::Throw(Box::new(Expr {
                kind: ExprKind::String(vec![StringSegment::Str("assertion failed".to_string())]),
                span: assert_tok.span,
            })),
            span: assert_tok.span,
        };
        let then_block = Block {
            stmts: vec![Stmt::Expr(throw_expr)],
            span: assert_tok.span,
        };
        let span = assert_tok.span.join(cond_span);
        Some(Stmt::Expr(Expr {
            kind: ExprKind::If {
                cond: Box::new(negated),
                then: then_block,
                else_branch: None,
            },
            span,
        }))
    }

    /// SPEC §8.1 `import` — 세 형태를 공통 AST 로 파싱한다.
    ///
    /// 문법:
    /// - `import a.b.c` → path=[a,b], items=[c], glob=false
    /// - `import a.b.{X, Y}` → path=[a,b], items=[X,Y], glob=false
    /// - `import a.b.*` → path=[a,b], items=[], glob=true
    fn parse_import(&mut self) -> Option<ImportStmt> {
        let import_tok = self.advance(); // `import`
        let start_span = import_tok.span;
        let mut segments: Vec<Ident> = vec![self.parse_ident("module path segment")?];
        let mut items: Vec<Ident> = Vec::new();
        let mut glob = false;
        let mut end_span = segments.last().unwrap().span;

        while matches!(self.peek_kind(), TokenKind::Dot) {
            self.advance(); // `.`
            match self.peek_kind() {
                TokenKind::Star => {
                    let star = self.advance();
                    glob = true;
                    end_span = star.span;
                    break;
                }
                TokenKind::LBrace => {
                    self.advance(); // `{`
                    while !matches!(self.peek_kind(), TokenKind::RBrace | TokenKind::Eof) {
                        let name = self.parse_ident("import item name")?;
                        items.push(name);
                        if matches!(self.peek_kind(), TokenKind::Comma) {
                            self.advance();
                        }
                    }
                    let rbrace = self.expect(&TokenKind::RBrace, "`}`")?;
                    end_span = rbrace.span;
                    break;
                }
                TokenKind::Ident(_) => {
                    let seg = self.parse_ident("module path segment")?;
                    end_span = seg.span;
                    segments.push(seg);
                }
                _ => {
                    self.error("expected identifier, `{`, or `*` after `.`");
                    return None;
                }
            }
        }

        // 단일 `import a.b.c` — 마지막 segment 를 item 으로 승격한다. SPEC 은
        // "import models.user.User" 를 User 하나를 가져오는 것으로 정의한다.
        // {X, Y}/glob 형태가 아니었다면 path 의 마지막이 가져올 이름.
        if items.is_empty() && !glob && segments.len() >= 2 {
            let last = segments.pop().unwrap();
            items.push(last);
        }

        Some(ImportStmt {
            path: segments,
            items,
            glob,
            span: start_span.join(end_span),
        })
    }

    fn parse_return(&mut self) -> Option<ReturnStmt> {
        let ret_tok = self.advance(); // `return`
                                      // return 뒤에 표현식이 올 수 있으면 파싱
        let (value, span) = if self.is_expr_start() {
            let expr = self.parse_expr()?;
            let span = ret_tok.span.join(expr.span);
            (Some(expr), span)
        } else {
            (None, ret_tok.span)
        };
        Some(ReturnStmt { value, span })
    }

    /// 현재 토큰이 표현식 시작으로 쓰일 수 있는지.
    fn is_expr_start(&self) -> bool {
        matches!(
            self.peek_kind(),
            TokenKind::Integer(_)
                | TokenKind::Float(_)
                | TokenKind::String(_)
                | TokenKind::True
                | TokenKind::False
                | TokenKind::Keyword(Keyword::Void)
                | TokenKind::Keyword(Keyword::If)
                | TokenKind::Keyword(Keyword::When)
                | TokenKind::Ident(_)
                | TokenKind::Regex { .. }
                | TokenKind::At(_)
                | TokenKind::Dollar
                | TokenKind::LParen
                | TokenKind::LBrace
                | TokenKind::Bang
                | TokenKind::Minus
                | TokenKind::Tilde
        )
    }

    /// `{ ... }` 블록 또는 `: <stmt>` 한 줄 조건문을 파싱한다.
    ///
    /// SPEC §6.1: `if cond : @out "..."` 같은 한 줄 조건문을 지원한다. 동일
    /// 규약이 `else` / `for`/`while` 본문에도 적용된다 — 개행 기반 "한 줄"
    /// 동작이 필요한 곳에서 재사용한다. `:` 분기는 단일 stmt 하나만 소비한다.
    fn parse_block_or_single_line(&mut self) -> Option<Block> {
        if matches!(self.peek_kind(), TokenKind::Colon) {
            let colon = self.advance();
            let stmt = self.parse_stmt()?;
            let span = colon.span.join(stmt.span());
            return Some(Block {
                stmts: vec![stmt],
                span,
            });
        }
        self.parse_block()
    }

    fn parse_block(&mut self) -> Option<Block> {
        let lbrace = self.expect(&TokenKind::LBrace, "`{`")?;
        let mut stmts = Vec::new();
        while !matches!(self.peek_kind(), TokenKind::RBrace | TokenKind::Eof) {
            let start = self.pos;
            match self.parse_stmt() {
                Some(s) => stmts.push(s),
                None => {
                    if self.pos == start {
                        self.advance();
                    }
                }
            }
            // 선택적 `;`
            while matches!(self.peek_kind(), TokenKind::Semicolon) {
                self.advance();
            }
        }
        let rbrace = self.expect(&TokenKind::RBrace, "`}`")?;
        Some(Block {
            stmts,
            span: lbrace.span.join(rbrace.span),
        })
    }

    fn parse_block_expr(&mut self) -> Option<Expr> {
        let block = self.parse_block()?;
        let span = block.span;
        Some(Expr {
            kind: ExprKind::Block(block),
            span,
        })
    }

    fn parse_if(&mut self) -> Option<Expr> {
        let if_tok = self.advance(); // `if`
        let cond = self.parse_expr()?;
        // SPEC §6.1: 한 줄 조건문 `if cond : <stmt>` — `{` 대신 `:` 를 만나면
        // 뒤따르는 단일 stmt 를 블록 한 덩어리로 감싼다. else 분기는 같은
        // 규약을 반복 적용한다.
        let then = self.parse_block_or_single_line()?;
        let else_branch = if matches!(self.peek_kind(), TokenKind::Keyword(Keyword::Else)) {
            self.advance();
            // `else if`는 else 분기에 새 if 표현식을 중첩.
            if matches!(self.peek_kind(), TokenKind::Keyword(Keyword::If)) {
                Some(Box::new(self.parse_if()?))
            } else {
                let block = self.parse_block_or_single_line()?;
                let span = block.span;
                Some(Box::new(Expr {
                    kind: ExprKind::Block(block),
                    span,
                }))
            }
        } else {
            None
        };
        let end_span = else_branch.as_ref().map_or(then.span, |e| e.span);
        Some(Expr {
            kind: ExprKind::If {
                cond: Box::new(cond),
                then,
                else_branch,
            },
            span: if_tok.span.join(end_span),
        })
    }

    fn parse_when(&mut self) -> Option<Expr> {
        let when_tok = self.advance(); // `when`
        let scrutinee = self.parse_expr()?;
        self.expect(&TokenKind::LBrace, "`{`")?;
        let mut arms = Vec::new();
        while !matches!(self.peek_kind(), TokenKind::RBrace | TokenKind::Eof) {
            let pat = self.parse_pattern()?;
            self.expect(&TokenKind::Arrow, "`->`")?;
            let body = self.parse_expr()?;
            arms.push(WhenArm { pattern: pat, body });
            // 선택적 구분자
            while matches!(self.peek_kind(), TokenKind::Semicolon | TokenKind::Comma) {
                self.advance();
            }
        }
        let rbrace = self.expect(&TokenKind::RBrace, "`}`")?;
        Some(Expr {
            kind: ExprKind::When {
                scrutinee: Box::new(scrutinee),
                arms,
            },
            span: when_tok.span.join(rbrace.span),
        })
    }

    fn parse_for(&mut self) -> Option<Expr> {
        let for_tok = self.advance(); // `for`
        // SPEC §6.4: `for (item, index) in arr` 형태의 tuple destructuring 지원.
        // `(` 로 시작하면 단일 ident 2개를 쉼표로 구분해 수집.
        let (var, index_var) = if matches!(self.peek_kind(), TokenKind::LParen) {
            self.advance(); // `(`
            let first = self.parse_ident("loop variable")?;
            self.expect(&TokenKind::Comma, "`,`")?;
            let second = self.parse_ident("index variable")?;
            self.expect(&TokenKind::RParen, "`)`")?;
            (first, Some(second))
        } else {
            (self.parse_ident("loop variable")?, None)
        };
        self.expect(&TokenKind::Keyword(Keyword::In), "`in`")?;
        let iter = self.parse_expr()?;
        let body = self.parse_block()?;
        let span = for_tok.span.join(body.span);
        Some(Expr {
            kind: ExprKind::For {
                var,
                index_var,
                iter: Box::new(iter),
                body,
            },
            span,
        })
    }

    fn parse_while(&mut self) -> Option<Expr> {
        let while_tok = self.advance(); // `while`
        let cond = self.parse_expr()?;
        let body = self.parse_block()?;
        let span = while_tok.span.join(body.span);
        Some(Expr {
            kind: ExprKind::While {
                cond: Box::new(cond),
                body,
            },
            span,
        })
    }

    fn parse_pattern(&mut self) -> Option<Pattern> {
        // `_` 와일드카드
        if matches!(self.peek_kind(), TokenKind::Ident(n) if n == "_") {
            self.advance();
            return Some(Pattern::Wildcard);
        }
        // `!EXPR` — negation 패턴 (SPEC §6.3). 일반 unary `!` 표현식과
        // 모호하지만 pattern 위치에서는 항상 negation 으로 해석한다.
        if matches!(self.peek_kind(), TokenKind::Bang) {
            self.advance();
            let inner = self.parse_expr()?;
            return Some(Pattern::Not(inner));
        }
        // `in EXPR` — contains 패턴. `in` 은 for 구문에서도 쓰이지만
        // pattern 시작 토큰이면 contains 로 해석.
        if matches!(self.peek_kind(), TokenKind::Keyword(Keyword::In)) {
            self.advance();
            let inner = self.parse_expr()?;
            return Some(Pattern::Contains(inner));
        }
        // 리터럴 / 범위 / 가드 — 공통으로 표현식을 한 번 파싱 후 분기.
        let first = self.parse_expr()?;
        // `$`로 시작하는 표현식은 가드로 취급 (비교/논리 결과 bool).
        if matches!(first.kind, ExprKind::Ident(ref id) if id.name == "$")
            || contains_dollar(&first)
        {
            return Some(Pattern::Guard(first));
        }
        // Range 표현식은 Pattern::Range로 재분류.
        if let ExprKind::Range {
            start,
            end,
            inclusive,
        } = first.kind
        {
            return Some(Pattern::Range {
                start: *start,
                end: *end,
                inclusive,
            });
        }
        Some(Pattern::Literal(first))
    }

    /// `@name arg` 형태의 도메인 호출을 파싱한다.
    ///
    /// MVP 규칙: `@name` 다음 인자가 올 수 있으면 **하나의 완전한
    /// 표현식**(연산자 전부 결합)을 인자로 받는다. 복수 인자/
    /// `key=value` property/`{}` 본문은 이후 커밋에서 SPEC §9.3~§9.5에
    /// 따라 확장한다.
    fn parse_domain_call(&mut self) -> Option<Expr> {
        let at_tok = self.advance();
        let TokenKind::At(name) = &at_tok.kind else {
            unreachable!("parse_domain_call called on non-@ token");
        };
        // SPEC §9.6: dotted path `@Parent.Child.Inner` — 각 segment 가 대문자로
        // 시작하면 nested define 경로로 결합한다. 첫 segment 가 소문자(`@env.X`,
        // `@request.method` 같은 field access) 면 이 경로로 들어오지 않고
        // 기존 parse_domain_call + postfix Field 체인이 처리한다.
        let mut name_str = name.clone();
        let mut end_span = at_tok.span;
        let is_dotted_candidate = name_str
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_uppercase());
        if is_dotted_candidate {
            while matches!(self.peek_kind(), TokenKind::Dot) {
                // 다음 토큰이 대문자 Ident 인지 먼저 확인 (필드 접근은 field
                // name 이 소문자일 수 있음 — `@Layout.header` 같은 경우는
                // field access 로 남겨 둔다).
                let next_is_upper = matches!(
                    self.tokens.get(self.pos + 1).map(|t| &t.kind),
                    Some(TokenKind::Ident(s)) if s.chars().next().is_some_and(|c| c.is_ascii_uppercase())
                );
                if !next_is_upper {
                    break;
                }
                self.advance(); // `.`
                let seg = self.parse_ident("nested domain segment")?;
                name_str.push('.');
                name_str.push_str(&seg.name);
                end_span = seg.span;
            }
        }
        let name_ident = Ident {
            name: name_str,
            span: at_tok.span.join(end_span),
        };

        // `@route` / `@respond` / `@server` 는 인자 수/형태가 특수해 전용
        // 서브루틴으로 분기한다. 일반 도메인의 1-인자 규약(`@out x`,
        // `@html {...}`)을 건드리지 않기 위해 이름 기반 분기.
        if name_ident.name == "route" {
            return self.parse_route_call(name_ident);
        }
        if name_ident.name == "respond" {
            return self.parse_respond_call(name_ident);
        }
        if name_ident.name == "server" {
            return self.parse_server_call(name_ident);
        }
        if name_ident.name == "redirect" {
            return self.parse_redirect_call(name_ident);
        }

        // SPEC 부록 I/O 도메인 — `@fs.read <path>`, `@fs.write <path> <content>
        // <encoding>`, `@process.run <cmd>`, `@fetch <METHOD> <url>`.
        //
        // 공통 규약: `@name.method` dotted chain 혹은 `@name` 단독 뒤에 공백
        // 구분 positional 인자들이 온다. `test.txt` / `./test.txt` 같은 bare
        // path 토큰열은 자동으로 문자열 리터럴로 합성되며, `GET` 같은 ident
        // 는 자연스럽게 String 토큰으로 받아 쓴다.
        //
        // 결과 AST: `Call { callee: Field{Domain, method}, args }` 로 떨어뜨려
        // interp 의 기존 BoundMethod 호출 경로를 그대로 탄다. `@fetch` 는
        // dotted chain 이 없는 특수 케이스라 `Domain` 을 그대로 callee 로 쓴다.
        if matches!(name_ident.name.as_str(), "fs" | "process")
            && matches!(self.peek_kind(), TokenKind::Dot)
        {
            return self.parse_io_domain_call(name_ident, at_tok.span);
        }
        if name_ident.name == "fetch" {
            return self.parse_fetch_call(name_ident, at_tok.span);
        }

        // C_html-min: 대문자로 시작하는 `@Name(args...)` 는 사용자 정의 도메인
        // invoke. multi-arg positional 을 받는다. 소문자 `@name arg` 는 기존
        // 1-인자 규약 유지.
        let is_user_domain = name_ident
            .name
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_uppercase());
        if is_user_domain && matches!(self.peek_kind(), TokenKind::LParen) {
            self.advance(); // `(`
            let mut args = Vec::new();
            while !matches!(self.peek_kind(), TokenKind::RParen | TokenKind::Eof) {
                let arg = self.parse_expr()?;
                args.push(arg);
                if matches!(self.peek_kind(), TokenKind::Comma) {
                    self.advance();
                } else {
                    break;
                }
            }
            let rparen = self.expect(&TokenKind::RParen, "`)`")?;
            return Some(Expr {
                kind: ExprKind::Domain {
                    name: name_ident,
                    args,
                },
                span: at_tok.span.join(rparen.span),
            });
        }

        // SPEC §9.3/§9.4: 대문자 user-domain 의 paren-less 호출은 property
        // (`key=value`) 와 token (positional) 을 공백 구분 시퀀스로 받는다.
        // property 는 `ExprKind::Assign{target, value}` 로 AST 에 담기며, 나머지
        // positional 값은 token 으로 수집된다. block literal 은 반드시 마지막.
        //
        // `@Auth` 처럼 인자 없는 middleware 호출도 이 경로로 자연스럽게 0-arg.
        // 후속 stmt (let/@out 등)가 인자로 잘못 흡수되지 않도록 stmt 시작
        // 토큰은 `is_domain_arg_start` 에서 거부한다. `ident =` 선행은 property
        // 로 흡수 (아래 prop-lookahead 참고).
        if is_user_domain {
            let mut args = Vec::new();
            let mut end_span = at_tok.span;
            loop {
                // property lookahead: `ident =` 형태면 property 로 소비한다.
                // lambda 나 함수 호출 등 일반 표현식에서 `ident =` 는 stmt-level
                // 대입이라 여기 오지 않음. 같은 줄의 `@Name name="..."` 패턴만 매칭.
                if self.looks_like_prop_arg() {
                    let key = self.parse_ident("property name")?;
                    self.expect(&TokenKind::Eq, "`=`")?;
                    let value = self.parse_expr()?;
                    let span = key.span.join(value.span);
                    end_span = span;
                    args.push(Expr {
                        kind: ExprKind::Assign {
                            target: key,
                            value: Box::new(value),
                        },
                        span,
                    });
                    continue;
                }
                // SPEC §10.4 boolean shorthand: `@btn disabled` 처럼 `ident`
                // 단독으로 등장하고 다음 토큰이 새 arg / stmt 경계면 `ident=true`
                // 로 인코딩한다. `ident` 뒤에 `.`(field), `[`(index),
                // `(`(call), `+`/`-`/`*`/`/` 같은 연산자가 오면 일반 표현식이라
                // shorthand 가 아니다.
                if self.looks_like_bool_shorthand() {
                    let key = self.parse_ident("property name")?;
                    let span = key.span;
                    end_span = span;
                    let true_expr = Expr { kind: ExprKind::True, span };
                    args.push(Expr {
                        kind: ExprKind::Assign {
                            target: key,
                            value: Box::new(true_expr),
                        },
                        span,
                    });
                    continue;
                }
                // 일반 인자 (token 또는 block). 새 stmt 시작으로 해석될 만한
                // 토큰이면 종료.
                if !self.is_domain_arg_start() {
                    break;
                }
                // block literal 이 아닌 새 줄의 stmt 시작도 보호. 현재 간이
                // 규칙: `@` 토큰이 보이면 그것이 새로운 domain stmt 의 시작일
                // 가능성이 높다. SPEC 은 한 줄에 여러 domain 호출이 붙지
                // 않으므로 (`@A @B` 는 없음) 안전하게 종료.
                if matches!(self.peek_kind(), TokenKind::At(_)) {
                    break;
                }
                // 줄바꿈이 있고 다음 토큰이 block `{` 이 아니면 stmt 경계.
                // `@Layout\n{...}` 처럼 블록 본문을 다음 줄에 두는 패턴은
                // 계속 허용하기 위해 `{` 만 예외로 둔다.
                if self.newline_before_cur()
                    && !matches!(self.peek_kind(), TokenKind::LBrace)
                {
                    break;
                }
                let arg = self.parse_expr()?;
                end_span = arg.span;
                // block 은 관례상 마지막.
                let is_block = matches!(arg.kind, ExprKind::Block(_));
                args.push(arg);
                if is_block {
                    break;
                }
            }
            return Some(Expr {
                kind: ExprKind::Domain {
                    name: name_ident,
                    args,
                },
                span: at_tok.span.join(end_span),
            });
        }

        // SPEC §9.5 `@content` 는 언제나 0-arg marker 로만 쓰인다. 일반 1-인자
        // 규약을 적용하면 바로 뒤에 오는 `@out "..."` 같은 stmt 를 잘못 흡수해
        // `@content { ... }` 본문 뒤의 코드가 인자로 먹힌다. 명시적으로 0-arg.
        if name_ident.name == "content" {
            return Some(Expr {
                kind: ExprKind::Domain {
                    name: name_ident,
                    args: Vec::new(),
                },
                span: at_tok.span,
            });
        }

        let mut args = Vec::new();
        let mut end_span = at_tok.span;
        if self.is_domain_arg_start() {
            if let Some(arg) = self.parse_expr() {
                end_span = arg.span;
                args.push(arg);
            }
        }

        Some(Expr {
            kind: ExprKind::Domain {
                name: name_ident,
                args,
            },
            span: at_tok.span.join(end_span),
        })
    }

    /// `@route METHOD PATH { body }` 를 `Domain { args: [Ident, String, Block] }`
    /// 형태로 파싱한다. HIR 로 낮출 때 전용 variant 로 분해된다.
    ///
    /// path 합성은 토큰 원문을 차례로 이어붙인다. 소스 문자열이 파서에
    /// 없으므로 각 토큰을 [`token_source_repr`] 로 재구성한다. SPEC 상 path
    /// 에는 공백이 없으므로 이 방식이 충분하다.
    fn parse_route_call(&mut self, name_ident: Ident) -> Option<Expr> {
        let start_span = name_ident.span;

        // method — ident(GET/POST/...) 또는 `*`. 사용자 스코프와 겹치지
        // 않도록 String 리터럴로 보존한다. Ident 로 두면 resolver 가 미정의
        // 변수로 진단한다.
        //
        // A2a nested route group: method 슬롯에 `/` (Slash) 가 바로 오면
        // `@route /prefix { @route METHOD /suffix { ... } }` 형태. 이 경우
        // method 자리에 sentinel `""` 를 넣어 analyzer 가 "그룹" 으로
        // 인식하게 한다. HIR 까지 가기 전에 analyzer 가 unfold 해서
        // HIR::Route 의 method 는 항상 non-empty 이다.
        let method_expr = match self.peek_kind().clone() {
            TokenKind::Ident(m) => {
                let tok = self.advance();
                Expr {
                    kind: ExprKind::String(vec![StringSegment::Str(m)]),
                    span: tok.span,
                }
            }
            TokenKind::Star => {
                let tok = self.advance();
                Expr {
                    kind: ExprKind::String(vec![StringSegment::Str("*".to_string())]),
                    span: tok.span,
                }
            }
            TokenKind::Slash => {
                // group mode — method 없음. 현재 span 만 빌려 온다 (토큰은
                // 소비하지 않음; path 파싱이 동일 토큰부터 이어간다).
                let span = self.peek().span;
                Expr {
                    kind: ExprKind::String(vec![StringSegment::Str(String::new())]),
                    span,
                }
            }
            _ => {
                self.diagnostics.push(
                    Diagnostic::error("expected HTTP method after `@route`")
                        .with_primary(self.peek().span, ""),
                );
                return None;
            }
        };

        // path — `/` 또는 `*` 로 시작하는 토큰을 `{` 만날 때까지 이어 붙임.
        let path_start = self.peek().span;
        let mut path_end = path_start;
        let mut path_text = String::new();
        if !matches!(self.peek_kind(), TokenKind::Slash | TokenKind::Star) {
            self.diagnostics.push(
                Diagnostic::error("expected path starting with `/` or `*` after HTTP method")
                    .with_primary(self.peek().span, ""),
            );
            return None;
        }
        while !matches!(self.peek_kind(), TokenKind::LBrace | TokenKind::Eof) {
            let tok = self.advance();
            path_end = tok.span;
            path_text.push_str(&token_source_repr(&tok.kind));
        }
        let path_span = path_start.join(path_end);
        let path_expr = Expr {
            kind: ExprKind::String(vec![StringSegment::Str(path_text)]),
            span: path_span,
        };

        // body — `{ ... }` 블록.
        let block = self.parse_block()?;
        let block_span = block.span;
        let block_expr = Expr {
            kind: ExprKind::Block(block),
            span: block_span,
        };

        Some(Expr {
            kind: ExprKind::Domain {
                name: name_ident,
                args: vec![method_expr, path_expr, block_expr],
            },
            span: start_span.join(block_span),
        })
    }

    /// `@respond <status> <payload>?` 을 `Domain { args: [status, payload] }`
    /// 2-인자 형태로 파싱한다. `payload` 가 생략되면 `void` 리터럴로 채워
    /// 항상 2-인자 규약을 유지한다 (HIR lowering 이 자리수로 분해).
    ///
    /// status 자리는 일반 표현식이지만 실전에서는 정수 리터럴이다 — SPEC
    /// §11.4 가 모두 숫자 코드를 쓴다. payload 는 object literal (`{k: v}`)
    /// 또는 기존 값/표현식(빈 `{}` 포함)을 그대로 받는다. 새 문법이 아니라
    /// "인자 2개 규약" 만 추가 — 학습 비용 0.
    fn parse_respond_call(&mut self, name_ident: Ident) -> Option<Expr> {
        let start_span = name_ident.span;
        let status_expr = self.parse_expr()?;
        let mut end_span = status_expr.span;

        // payload — `{` 로 시작하면 object literal(빈 `{}` 포함) 또는 블록.
        // 그 외 도메인-인자 시작 토큰이면 일반 표현식. 그 밖에는 생략으로
        // 간주하고 `void` 를 채운다.
        let payload_expr = if self.is_domain_arg_start() {
            let e = self.parse_expr()?;
            end_span = e.span;
            e
        } else {
            Expr {
                kind: ExprKind::Void,
                span: status_expr.span,
            }
        };

        Some(Expr {
            kind: ExprKind::Domain {
                name: name_ident,
                args: vec![status_expr, payload_expr],
            },
            span: start_span.join(end_span),
        })
    }

    /// SPEC §11.9 `@redirect` — 1 또는 2 인자 특화 파싱.
    ///
    /// 형태:
    /// - `@redirect "url"` — 302 + Location.
    /// - `@redirect 301 "url"` — 명시적 status.
    ///
    /// 일반 1-인자 규약으로 떨어뜨리면 `@redirect 301` 뒤에 `"url"` 이 후속
    /// stmt 로 파싱되며 연결이 끊긴다. 여기서 "URL-ish" 토큰을 한 번 더 볼지
    /// 판정해 두 번째 인자를 수집한다.
    fn parse_redirect_call(&mut self, name_ident: Ident) -> Option<Expr> {
        let start_span = name_ident.span;
        let first = self.parse_expr()?;
        let mut end_span = first.span;
        let mut args = vec![first];
        // 두 번째 인자가 이어질 수 있는 토큰이면 수집.
        if self.is_domain_arg_start() && !matches!(self.peek_kind(), TokenKind::At(_)) {
            let second = self.parse_expr()?;
            end_span = second.span;
            args.push(second);
        }
        Some(Expr {
            kind: ExprKind::Domain {
                name: name_ident,
                args,
            },
            span: start_span.join(end_span),
        })
    }

    /// SPEC 부록 `@fs.read` / `@fs.write` / `@process.run` 같은 lowercase dotted
    /// I/O 도메인을 `Call { callee: Field{Domain, method}, args }` 로 파싱한다.
    ///
    /// 메서드 체인(`.read`) 을 먼저 수집한 뒤 positional 인자를 공백 구분으로
    /// 흡수한다. bare path (`test.txt`, `./test.txt`) 는 [`Self::try_parse_bare_path`]
    /// 가 문자열 리터럴로 합성한다.
    fn parse_io_domain_call(&mut self, name_ident: Ident, at_span: Span) -> Option<Expr> {
        // `.method` 체인 수집 — SPEC 예제는 single method 이지만 확장 여지로
        // 반복을 허용한다.
        let receiver = Expr {
            kind: ExprKind::Domain {
                name: name_ident.clone(),
                args: Vec::new(),
            },
            span: name_ident.span,
        };
        let mut callee = receiver;
        while matches!(self.peek_kind(), TokenKind::Dot) {
            // 다음 토큰이 ident 가 아니면 path 시작 (`@fs ./a`) 이므로 멈춘다.
            if !matches!(
                self.tokens.get(self.pos + 1).map(|t| &t.kind),
                Some(TokenKind::Ident(_))
            ) {
                break;
            }
            self.advance(); // `.`
            let field = self.parse_ident("method name")?;
            let span = callee.span.join(field.span);
            callee = Expr {
                kind: ExprKind::Field {
                    target: Box::new(callee),
                    field,
                },
                span,
            };
        }

        // `@fs.write(a, b)` 처럼 괄호 호출 문법도 지원한다. 공백-구분
        // positional 과 독립적으로 받아들이기 위해 callee 뒤에 `(` 가 바로
        // 붙으면 일반 함수 호출로 흡수한다. 여기서 소비하지 않고 postfix
        // `(` 핸들러 (parse_expr_bp) 에 맡기면 되지만 본 함수는 domain 엔트리
        // 라 직접 처리.
        if matches!(self.peek_kind(), TokenKind::LParen) && !self.newline_before_cur() {
            self.advance(); // `(`
            let mut args = Vec::new();
            while !matches!(self.peek_kind(), TokenKind::RParen | TokenKind::Eof) {
                let arg = self.parse_expr()?;
                args.push(arg);
                if matches!(self.peek_kind(), TokenKind::Comma) {
                    self.advance();
                } else {
                    break;
                }
            }
            let rparen = self.expect(&TokenKind::RParen, "`)`")?;
            return Some(Expr {
                kind: ExprKind::Call {
                    callee: Box::new(callee),
                    args,
                },
                span: at_span.join(rparen.span),
            });
        }

        let mut args = Vec::new();
        let mut end_span = callee.span;
        // 줄바꿈은 I/O 도메인 호출의 stmt 경계 — 다음 줄 stmt 가 인자로
        // 흡수되는 것을 막기 위해 개행이 있으면 루프 종료.
        while !matches!(self.peek_kind(), TokenKind::At(_))
            && self.is_io_arg_start()
            && !self.newline_before_cur()
        {
            if let Some(path) = self.try_parse_bare_path() {
                end_span = path.span;
                args.push(path);
                continue;
            }
            let arg = self.parse_expr()?;
            end_span = arg.span;
            args.push(arg);
        }

        Some(Expr {
            kind: ExprKind::Call {
                callee: Box::new(callee),
                args,
            },
            span: at_span.join(end_span),
        })
    }

    /// SPEC 부록 `@fetch <METHOD> <url>` — dotted chain 이 없는 1~2 arg 도메인.
    ///
    /// 일반 1-arg 규약으로는 `GET "url"` 에서 url 이 잘려나가므로 여기서 2 arg
    /// 을 모두 흡수한다. MVP 런타임은 아직 실제 HTTP 요청을 보내지 않지만
    /// 파서가 원형을 보존하는 것이 목표.
    fn parse_fetch_call(&mut self, name_ident: Ident, at_span: Span) -> Option<Expr> {
        let mut args = Vec::new();
        let mut end_span = name_ident.span;
        // 첫 인자는 HTTP method. 관례상 `GET`/`POST`/... 같은 bare ident 로
        // 쓰므로 변수 참조가 아니라 string 리터럴로 고정 해석한다. 따옴표로
        // 감싼 `"GET"` 도 parse_expr 경로에서 자연스럽게 받아 들인다.
        if matches!(self.peek_kind(), TokenKind::Ident(_)) && !self.newline_before_cur() {
            let tok = self.advance();
            let TokenKind::Ident(name) = tok.kind else {
                unreachable!("peeked Ident");
            };
            end_span = tok.span;
            args.push(Expr {
                kind: ExprKind::String(vec![StringSegment::Str(name)]),
                span: tok.span,
            });
        }
        while !matches!(self.peek_kind(), TokenKind::At(_))
            && self.is_io_arg_start()
            && !self.newline_before_cur()
        {
            if let Some(path) = self.try_parse_bare_path() {
                end_span = path.span;
                args.push(path);
                continue;
            }
            let arg = self.parse_expr()?;
            end_span = arg.span;
            args.push(arg);
        }
        Some(Expr {
            kind: ExprKind::Domain {
                name: name_ident,
                args,
            },
            span: at_span.join(end_span),
        })
    }

    /// I/O 도메인 positional arg 시작 토큰. 일반 arg-start 에 더해 `.` / `/` 를
    /// 허용해 bare path (`./foo`, `/abs`) 를 받아 낸다.
    fn is_io_arg_start(&self) -> bool {
        self.is_domain_arg_start()
            || matches!(
                self.peek_kind(),
                TokenKind::Dot | TokenKind::Slash
            )
    }

    /// 인접 토큰 열을 shell-style path 리터럴로 합성한다.
    ///
    /// 반환값은 `String` 세그먼트 하나인 `Expr`. 경로로 간주하려면 `.` 또는
    /// `/` 가 한 번 이상 포함돼야 한다 — 단일 ident 는 일반 표현식에 맡긴다
    /// (변수 참조와 모호함 방지). 토큰 사이에 공백이 끼면 (span.end != next.start)
    /// 즉시 종료한다.
    fn try_parse_bare_path(&mut self) -> Option<Expr> {
        let saved = self.pos;
        let first_tok = self.peek();
        if !matches!(
            first_tok.kind,
            TokenKind::Ident(_) | TokenKind::Dot | TokenKind::Slash
        ) {
            return None;
        }
        let first_span = first_tok.span;
        let mut text = String::new();
        let mut prev_end = first_span.range.start;
        let mut end_span = first_span;

        loop {
            let tok = self.peek();
            if !text.is_empty() && tok.span.range.start != prev_end {
                break;
            }
            match &tok.kind {
                TokenKind::Ident(s) => text.push_str(s),
                TokenKind::Dot => text.push('.'),
                TokenKind::Slash => text.push('/'),
                TokenKind::Minus => text.push('-'),
                TokenKind::Integer(s) => text.push_str(s),
                _ => break,
            }
            let consumed = self.advance();
            end_span = consumed.span;
            prev_end = consumed.span.range.end;
        }

        if !text.contains('.') && !text.contains('/') && !text.contains('-') {
            // 단일 ident 만 소비한 경우 — 원위치로 복원하고 shell 토큰이
            // 아님을 알린다. `-` 를 포함하면 `utf-8` / `-v` 같은 옵션 토큰
            // 으로 인정.
            self.pos = saved;
            return None;
        }
        Some(Expr {
            kind: ExprKind::String(vec![StringSegment::Str(text)]),
            span: first_span.join(end_span),
        })
    }

    /// `@server { ... }` 를 `Domain { args: [Block] }` 형태로 파싱한다.
    ///
    /// 일반 도메인 경로가 아니라 전용 서브루틴을 두는 이유: generic
    /// `parse_domain_call` 이 `{}` 를 `parse_expr` 로 보내면 `{}` 가 빈 object
    /// literal 로 낮춰진다 (`looks_like_object_literal()` true). `@server` 는
    /// 블록 본문이 필수라 여기서 `parse_block()` 을 직접 호출해 블록을
    /// 강제한다. `@route` body 와 동일한 패턴.
    ///
    /// 블록 안에는 `@listen N`, `@route METHOD /path { ... }`, `@out "boot"`
    /// 같은 기타 도메인이 자유롭게 올 수 있다. 분류(listen/routes/body_stmts)
    /// 는 analyzer (C5a-3) 가 수행한다.
    fn parse_server_call(&mut self, name_ident: Ident) -> Option<Expr> {
        let start_span = name_ident.span;
        if !matches!(self.peek_kind(), TokenKind::LBrace) {
            self.diagnostics.push(
                Diagnostic::error("expected `{` block body after `@server`")
                    .with_primary(self.peek().span, ""),
            );
            return None;
        }
        let block = self.parse_block()?;
        let block_span = block.span;
        let block_expr = Expr {
            kind: ExprKind::Block(block),
            span: block_span,
        };
        Some(Expr {
            kind: ExprKind::Domain {
                name: name_ident,
                args: vec![block_expr],
            },
            span: start_span.join(block_span),
        })
    }

    /// 문자열 원문(따옴표 제외)을 보간 세그먼트로 쪼갠다.
    ///
    /// SPEC §2.4 규칙:
    /// - `{expr}`은 보간, 중괄호 내부는 orv 표현식
    /// - `\{`, `\}`, `\n`, `\t`, `\\`, `\"`는 이스케이프
    /// - 중괄호가 짝이 안 맞으면 진단 수집 후 리터럴로 처리
    fn parse_string_segments(&mut self, raw: &str, span: Span) -> Vec<StringSegment> {
        let mut segments = Vec::new();
        let mut literal = String::new();
        // `raw` 는 문자열 리터럴의 양 끝 `"` 를 뺀 원문. 그래서 `raw` 내에서
        // 인덱스 `i` 는 원본 소스의 `span.range.start + 1 + i` 에 해당한다.
        // 보간 `{expr}` 내부를 재-lex 할 때 이 절대 오프셋을 base 로 넘겨
        // 생성되는 모든 토큰 스팬이 원본 좌표를 가리키도록 한다.
        let content_base: u32 = span.range.start + 1;
        let mut chars = raw.char_indices().peekable();
        while let Some((idx, c)) = chars.next() {
            match c {
                '\\' => {
                    // 이스케이프 해제
                    match chars.next().map(|(_, c)| c) {
                        Some('n') => literal.push('\n'),
                        Some('t') => literal.push('\t'),
                        Some('r') => literal.push('\r'),
                        Some('\\') => literal.push('\\'),
                        Some('"') => literal.push('"'),
                        Some('{') => literal.push('{'),
                        Some('}') => literal.push('}'),
                        Some(other) => {
                            // 알 수 없는 이스케이프는 그대로 보존(에러 대신 관용 처리)
                            literal.push('\\');
                            literal.push(other);
                        }
                        None => literal.push('\\'),
                    }
                    let _ = idx;
                }
                '{' => {
                    // 보간 시작
                    if !literal.is_empty() {
                        segments.push(StringSegment::Str(std::mem::take(&mut literal)));
                    }
                    // `{...}` 내부 원문 수집 (중첩 `{}` 미지원 MVP — 1단계만)
                    let inner_start = idx + 1; // `{` 다음 바이트.
                    let mut inner = String::new();
                    let mut depth = 1u32;
                    for (_, ic) in chars.by_ref() {
                        if ic == '{' {
                            depth += 1;
                            inner.push(ic);
                        } else if ic == '}' {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                            inner.push(ic);
                        } else {
                            inner.push(ic);
                        }
                    }
                    if depth != 0 {
                        self.diagnostics.push(
                            Diagnostic::error("unterminated `{` in string interpolation")
                                .with_primary(span, ""),
                        );
                        break;
                    }
                    // 내부를 별도 렉서+파서로 돌리되, 생성되는 스팬이 원본
                    // 소스 좌표를 가리키도록 base offset 을 넘긴다. 그렇지
                    // 않으면 resolver 의 Span-기반 이름 테이블이 서로 다른
                    // interpolation 사이트에서 충돌한다.
                    let base = content_base + u32::try_from(inner_start).unwrap_or(0);
                    let inner_lex = lex_with_base_offset(&inner, span.file, base);
                    for d in inner_lex.diagnostics {
                        self.diagnostics.push(d);
                    }
                    let mut sub =
                        Parser::new(inner_lex.tokens, span.file, inner_lex.newlines);
                    match sub.parse_expr() {
                        Some(expr) => segments.push(StringSegment::Interp(expr)),
                        None => {
                            self.diagnostics.push(
                                Diagnostic::error("invalid expression inside `{...}`")
                                    .with_primary(span, ""),
                            );
                        }
                    }
                    for d in sub.diagnostics {
                        self.diagnostics.push(d);
                    }
                }
                '}' => {
                    // 짝이 없는 `}` — 이스케이프를 권장
                    self.diagnostics.push(
                        Diagnostic::error("unexpected `}` in string; use `\\}` to escape")
                            .with_primary(span, ""),
                    );
                    literal.push('}');
                }
                other => literal.push(other),
            }
        }
        if !literal.is_empty() || segments.is_empty() {
            segments.push(StringSegment::Str(literal));
        }
        segments
    }

    /// SPEC §9.3: user-domain 호출에서 `ident =` 선두면 property 인자.
    /// `ident` 다음 토큰이 `=` 이어야 하며, 그 뒤에 일반 대입/비교 연산자
    /// (`==`) 와 혼동되지 않도록 `Eq` 단일 토큰만 매칭한다.
    fn looks_like_prop_arg(&self) -> bool {
        if !matches!(self.peek_kind(), TokenKind::Ident(_)) {
            return false;
        }
        matches!(
            self.tokens.get(self.pos + 1).map(|t| &t.kind),
            Some(TokenKind::Eq)
        )
    }

    /// SPEC §10.4 boolean shorthand: `@btn disabled` — `ident` 단독으로
    /// 다음 토큰이 새 arg 시작 또는 stmt 경계면 `ident=true` 축약.
    /// property 와 완전히 분리하려 field/call/index/산술 등이 뒤따르면 false.
    fn looks_like_bool_shorthand(&self) -> bool {
        if !matches!(self.peek_kind(), TokenKind::Ident(_)) {
            return false;
        }
        let next = self.tokens.get(self.pos + 1).map(|t| &t.kind);
        // property 종료/다음 arg 시작 후보.
        let is_terminator = matches!(
            next,
            Some(TokenKind::RBrace)
                | Some(TokenKind::RParen)
                | Some(TokenKind::Eof)
                | Some(TokenKind::Keyword(_))
                | Some(TokenKind::At(_))
                | Some(TokenKind::Ident(_))
                | Some(TokenKind::String(_))
                | Some(TokenKind::Integer(_))
                | Some(TokenKind::Float(_))
                | Some(TokenKind::True)
                | Some(TokenKind::False)
        );
        // 아래 토큰은 일반 표현식으로 이어진다 — shorthand 아님.
        // `.`/`[`/`(`/`=`/`==`/`!=`/`+`/`-`/`*`/`/`/`%`/`<`/`>`/`<=`/`>=`/
        // `&&`/`||`/`&`/`|`/`^`/`<<`/`>>`/`??`/`,`/`:`/`?` 등.
        if !is_terminator {
            return false;
        }
        // `)` 로 끝나는 경우는 paren-less user-domain 호출 컨텍스트가 아니라
        // 일반 함수 인자 리스트 내부일 가능성이 있다. paren-less 경로는
        // `)` 를 만나지 않으므로 여기 도달했다면 바깥이 `)` 다. 안전하게 true.
        true
    }

    /// 현재 토큰이 도메인 인자로 쓰일 수 있는 시작 토큰인지.
    /// `let`/`const`/`}`/`)`/이항 연산자/EOF/다른 스테이트먼트 시작은
    /// 인자로 간주되지 않는다.
    ///
    /// `{` 도 인자 시작으로 허용한다 — `@html { ... }` 처럼 블록 본문을
    /// 갖는 도메인 호출을 지원하기 위함. 기존 도메인(`@out` 등)은 블록을
    /// 넘기지 않으므로 회귀가 없다.
    fn is_domain_arg_start(&self) -> bool {
        matches!(
            self.peek_kind(),
            TokenKind::Integer(_)
                | TokenKind::Float(_)
                | TokenKind::String(_)
                | TokenKind::True
                | TokenKind::False
                | TokenKind::Keyword(Keyword::Void)
                | TokenKind::Keyword(Keyword::Await)
                | TokenKind::Ident(_)
                | TokenKind::Regex { .. }
                | TokenKind::At(_)
                | TokenKind::LParen
                | TokenKind::LBrace
                | TokenKind::Bang
                | TokenKind::Minus
                | TokenKind::Tilde
        )
    }

    /// 현재 토큰이 이항 연산자면 `(op, lbp, rbp)` 반환.
    /// bp가 클수록 강하게 결합한다. 좌결합은 `rbp = lbp + 1`.
    /// SPEC §2.5 순서를 따라 산술 > 비트시프트 > 비교 > 비트 > 논리 > 널병합.
    fn peek_binop(&self) -> Option<(BinaryOp, u8, u8)> {
        Some(match self.peek_kind() {
            // 널 병합 — 우결합, 가장 낮음
            TokenKind::QuestionQuestion => (BinaryOp::Coalesce, 1, 1),
            // 논리 OR/AND
            TokenKind::PipePipe => (BinaryOp::Or, 2, 3),
            TokenKind::AmpAmp => (BinaryOp::And, 4, 5),
            // 비트 OR/XOR/AND
            TokenKind::Pipe => (BinaryOp::BitOr, 6, 7),
            TokenKind::Caret => (BinaryOp::BitXor, 8, 9),
            TokenKind::Amp => (BinaryOp::BitAnd, 10, 11),
            // 비교
            TokenKind::EqEq => (BinaryOp::Eq, 12, 13),
            TokenKind::BangEq => (BinaryOp::Ne, 12, 13),
            TokenKind::Lt => (BinaryOp::Lt, 14, 15),
            TokenKind::Gt => (BinaryOp::Gt, 14, 15),
            TokenKind::LtEq => (BinaryOp::Le, 14, 15),
            TokenKind::GtEq => (BinaryOp::Ge, 14, 15),
            // 시프트
            TokenKind::LtLt => (BinaryOp::Shl, 16, 17),
            TokenKind::GtGt => (BinaryOp::Shr, 16, 17),
            // 산술
            TokenKind::Plus => (BinaryOp::Add, 18, 19),
            TokenKind::Minus => (BinaryOp::Sub, 18, 19),
            TokenKind::Star => (BinaryOp::Mul, 20, 21),
            TokenKind::Slash => (BinaryOp::Div, 20, 21),
            TokenKind::Percent => (BinaryOp::Rem, 20, 21),
            // 거듭제곱 — 우결합
            TokenKind::StarStar => (BinaryOp::Pow, 23, 22),
            _ => return None,
        })
    }
}

/// 표현식 트리에 `$` 참조가 있는지 재귀 탐색.
fn contains_dollar(expr: &Expr) -> bool {
    match &expr.kind {
        ExprKind::Ident(id) => id.name == "$",
        ExprKind::Unary { expr, .. } => contains_dollar(expr),
        ExprKind::Binary { lhs, rhs, .. } => contains_dollar(lhs) || contains_dollar(rhs),
        ExprKind::Paren(inner) => contains_dollar(inner),
        // `$.field` / `$[idx]` / `$.method(...)` 같이 `$` 에서 파생된 표현은
        // 모두 guard 로 취급돼야 when arm 의 dollar 슬롯을 사용할 수 있다.
        ExprKind::Field { target, .. } => contains_dollar(target),
        ExprKind::Index { target, index } => contains_dollar(target) || contains_dollar(index),
        ExprKind::Call { callee, args } => {
            contains_dollar(callee) || args.iter().any(contains_dollar)
        }
        ExprKind::Range {
            start,
            end,
            inclusive: _,
        } => contains_dollar(start) || contains_dollar(end),
        _ => false,
    }
}

fn describe(k: &TokenKind) -> String {
    match k {
        TokenKind::Integer(_) => "integer".into(),
        TokenKind::Float(_) => "float".into(),
        TokenKind::String(_) => "string".into(),
        TokenKind::Regex { .. } => "regex".into(),
        TokenKind::True => "`true`".into(),
        TokenKind::False => "`false`".into(),
        TokenKind::Ident(n) => format!("identifier `{n}`"),
        TokenKind::At(n) => format!("`@{n}`"),
        TokenKind::Keyword(kw) => format!("keyword `{kw:?}`").to_lowercase(),
        TokenKind::Eof => "end of file".into(),
        other => format!("`{other:?}`"),
    }
}

/// `@route` 경로 합성용 — 각 토큰을 소스에 실제로 적힌 문자열로 복원한다.
/// 공백이 없다고 가정되는 path 영역에서만 쓰이므로 완벽한 round-trip 은
/// 필요 없다. 경로에 실제 등장할 수 있는 토큰만 처리.
fn token_source_repr(kind: &TokenKind) -> String {
    match kind {
        TokenKind::Slash => "/".into(),
        TokenKind::Star => "*".into(),
        TokenKind::Colon => ":".into(),
        TokenKind::Dot => ".".into(),
        TokenKind::Minus => "-".into(),
        TokenKind::Ident(name) => name.clone(),
        TokenKind::Integer(s) | TokenKind::Float(s) => s.clone(),
        TokenKind::True => "true".into(),
        TokenKind::False => "false".into(),
        // path 에 등장하지 않아야 하는 토큰들이 여기로 떨어지면 빈 문자열을
        // 반환한다 — 상위 호출자가 `{`/`Eof` 를 만나기 전에 멈추므로 실제로
        // 도달할 일은 없다.
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lex;
    use orv_diagnostics::FileId;

    fn parse_str(src: &str) -> ParseResult {
        let lx = lex(src, FileId(0));
        assert!(
            lx.diagnostics.is_empty(),
            "lex errors: {:?}",
            lx.diagnostics
        );
        parse(lx.tokens, FileId(0))
    }

    /// 단일 리터럴 세그먼트 문자열인지 검사하고 내용 반환.
    fn plain_string(expr: &Expr) -> Option<&str> {
        let ExprKind::String(segs) = &expr.kind else {
            return None;
        };
        if segs.len() != 1 {
            return None;
        }
        let StringSegment::Str(s) = &segs[0] else {
            return None;
        };
        Some(s)
    }

    #[test]
    fn empty_program() {
        let r = parse_str("");
        assert!(r.diagnostics.is_empty());
        assert!(r.program.items.is_empty());
    }

    #[test]
    fn let_immutable() {
        let r = parse_str(r#"let name: string = "Alice""#);
        assert!(r.diagnostics.is_empty());
        assert_eq!(r.program.items.len(), 1);
        let Stmt::Let(s) = &r.program.items[0] else {
            panic!("expected let");
        };
        assert_eq!(s.kind, LetKind::Immutable);
        assert_eq!(s.name.name, "name");
        assert!(s.ty.is_some());
        assert_eq!(plain_string(&s.init), Some("Alice"));
    }

    #[test]
    fn let_mut() {
        let r = parse_str("let mut count: int = 0");
        assert!(r.diagnostics.is_empty());
        let Stmt::Let(s) = &r.program.items[0] else {
            panic!();
        };
        assert_eq!(s.kind, LetKind::Mutable);
    }

    #[test]
    fn let_sig() {
        let r = parse_str("let sig score: int = 0");
        assert!(r.diagnostics.is_empty());
        let Stmt::Let(s) = &r.program.items[0] else {
            panic!();
        };
        assert_eq!(s.kind, LetKind::Signal);
    }

    #[test]
    fn let_without_type() {
        let r = parse_str("let x = 42");
        assert!(r.diagnostics.is_empty());
        let Stmt::Let(s) = &r.program.items[0] else {
            panic!();
        };
        assert!(s.ty.is_none());
        assert!(matches!(s.init.kind, ExprKind::Integer(ref v) if v == "42"));
    }

    #[test]
    fn const_decl() {
        let r = parse_str("const PI: float = 3.14");
        assert!(r.diagnostics.is_empty());
        let Stmt::Const(c) = &r.program.items[0] else {
            panic!();
        };
        assert_eq!(c.name.name, "PI");
        assert!(matches!(c.init.kind, ExprKind::Float(ref v) if v == "3.14"));
    }

    #[test]
    fn nullable_type() {
        let r = parse_str("let maybe: string? = void");
        assert!(r.diagnostics.is_empty());
        let Stmt::Let(s) = &r.program.items[0] else {
            panic!();
        };
        let ty = s.ty.as_ref().unwrap();
        assert!(matches!(ty.kind, TypeRefKind::Nullable(_)));
    }

    #[test]
    fn as_cast_postfix() {
        // SPEC §4.9: `expr as <type>` 는 후위 연산자.
        let r = parse_str("let new_n: short = n as short");
        assert!(
            r.diagnostics.is_empty(),
            "unexpected diagnostics: {:?}",
            r.diagnostics
        );
        let Stmt::Let(s) = &r.program.items[0] else {
            panic!();
        };
        let ExprKind::Cast { expr, ty } = &s.init.kind else {
            panic!("expected Cast, got {:?}", s.init.kind);
        };
        assert!(matches!(expr.kind, ExprKind::Ident(ref id) if id.name == "n"));
        assert!(matches!(&ty.kind, TypeRefKind::Named(id) if id.name == "short"));
    }

    #[test]
    fn as_cast_binds_tighter_than_addition() {
        // `a + b as int` → `a + (b as int)` (Rust 와 동일).
        let r = parse_str("a + b as int");
        assert!(r.diagnostics.is_empty(), "diags: {:?}", r.diagnostics);
        let Stmt::Expr(e) = &r.program.items[0] else {
            panic!();
        };
        let ExprKind::Binary { op, lhs, rhs } = &e.kind else {
            panic!("expected Binary, got {:?}", e.kind);
        };
        assert_eq!(*op, BinaryOp::Add);
        assert!(matches!(lhs.kind, ExprKind::Ident(ref id) if id.name == "a"));
        assert!(matches!(&rhs.kind, ExprKind::Cast { .. }));
    }

    #[test]
    fn void_type_annotation() {
        // SPEC §4.1: `void` 는 원시 타입 — 어노테이션 자리에서 허용된다.
        // 값 자리의 `void` 리터럴과는 독립적으로 인식되어야 한다.
        let r = parse_str("let x: void = void");
        assert!(
            r.diagnostics.is_empty(),
            "unexpected diagnostics: {:?}",
            r.diagnostics
        );
        let Stmt::Let(s) = &r.program.items[0] else {
            panic!();
        };
        let ty = s.ty.as_ref().unwrap();
        let TypeRefKind::Named(id) = &ty.kind else {
            panic!("expected Named(void), got {:?}", ty.kind);
        };
        assert_eq!(id.name, "void");
        assert!(matches!(s.init.kind, ExprKind::Void));
    }

    #[test]
    fn multiple_statements() {
        let r = parse_str(
            r#"
            let a: int = 1
            let b: int = 2
            "hello"
            42
            "#,
        );
        assert!(r.diagnostics.is_empty());
        assert_eq!(r.program.items.len(), 4);
        assert!(matches!(r.program.items[0], Stmt::Let(_)));
        assert!(matches!(r.program.items[1], Stmt::Let(_)));
        assert!(matches!(r.program.items[2], Stmt::Expr(_)));
        assert!(matches!(r.program.items[3], Stmt::Expr(_)));
    }

    #[test]
    fn expr_statement_literals() {
        let r = parse_str(r#""Hello, World!""#);
        assert!(r.diagnostics.is_empty());
        let Stmt::Expr(e) = &r.program.items[0] else {
            panic!();
        };
        assert_eq!(plain_string(e), Some("Hello, World!"));
    }

    #[test]
    fn ident_reference() {
        let r = parse_str("let x = 1\nx");
        assert!(r.diagnostics.is_empty());
        let Stmt::Expr(e) = &r.program.items[1] else {
            panic!();
        };
        assert!(matches!(e.kind, ExprKind::Ident(ref id) if id.name == "x"));
    }

    #[test]
    fn missing_eq_reports_error() {
        let r = parse_str("let x 42");
        assert!(!r.diagnostics.is_empty());
    }

    #[test]
    fn missing_name_reports_error() {
        let r = parse_str("let = 42");
        assert!(!r.diagnostics.is_empty());
    }

    #[test]
    fn spans_cover_declaration() {
        let r = parse_str("let x = 42");
        assert!(r.diagnostics.is_empty());
        let Stmt::Let(s) = &r.program.items[0] else {
            panic!();
        };
        // `let x = 42` = 10 bytes
        assert_eq!(s.span.range.start, 0);
        assert_eq!(s.span.range.end, 10);
    }

    // ── 이항 연산자 ──

    fn binary_of(stmt: &Stmt) -> (&BinaryOp, &Expr, &Expr) {
        let Stmt::Expr(e) = stmt else {
            panic!("expected expr stmt");
        };
        let ExprKind::Binary { op, lhs, rhs } = &e.kind else {
            panic!("expected binary expr, got {:?}", e.kind);
        };
        (op, lhs, rhs)
    }

    #[test]
    fn addition() {
        let r = parse_str("1 + 2");
        assert!(r.diagnostics.is_empty());
        let (op, lhs, rhs) = binary_of(&r.program.items[0]);
        assert_eq!(*op, BinaryOp::Add);
        assert!(matches!(lhs.kind, ExprKind::Integer(ref v) if v == "1"));
        assert!(matches!(rhs.kind, ExprKind::Integer(ref v) if v == "2"));
    }

    #[test]
    fn precedence_mul_over_add() {
        // 1 + 2 * 3 → 1 + (2 * 3)
        let r = parse_str("1 + 2 * 3");
        assert!(r.diagnostics.is_empty());
        let (op, _, rhs) = binary_of(&r.program.items[0]);
        assert_eq!(*op, BinaryOp::Add);
        assert!(matches!(
            rhs.kind,
            ExprKind::Binary {
                op: BinaryOp::Mul,
                ..
            }
        ));
    }

    #[test]
    fn precedence_paren_overrides() {
        // (1 + 2) * 3
        let r = parse_str("(1 + 2) * 3");
        assert!(r.diagnostics.is_empty());
        let (op, lhs, _) = binary_of(&r.program.items[0]);
        assert_eq!(*op, BinaryOp::Mul);
        let ExprKind::Paren(inner) = &lhs.kind else {
            panic!();
        };
        assert!(matches!(
            inner.kind,
            ExprKind::Binary {
                op: BinaryOp::Add,
                ..
            }
        ));
    }

    #[test]
    fn pow_is_right_associative() {
        // 2 ** 3 ** 2 → 2 ** (3 ** 2)
        let r = parse_str("2 ** 3 ** 2");
        assert!(r.diagnostics.is_empty());
        let (op, _, rhs) = binary_of(&r.program.items[0]);
        assert_eq!(*op, BinaryOp::Pow);
        assert!(matches!(
            rhs.kind,
            ExprKind::Binary {
                op: BinaryOp::Pow,
                ..
            }
        ));
    }

    #[test]
    fn unary_neg() {
        let r = parse_str("-5");
        assert!(r.diagnostics.is_empty());
        let Stmt::Expr(e) = &r.program.items[0] else {
            panic!();
        };
        let ExprKind::Unary { op, expr } = &e.kind else {
            panic!();
        };
        assert_eq!(*op, UnaryOp::Neg);
        assert!(matches!(expr.kind, ExprKind::Integer(ref v) if v == "5"));
    }

    #[test]
    fn unary_not_precedence() {
        // !a && b → (!a) && b
        let r = parse_str("!a && b");
        assert!(r.diagnostics.is_empty());
        let (op, lhs, _) = binary_of(&r.program.items[0]);
        assert_eq!(*op, BinaryOp::And);
        assert!(matches!(
            lhs.kind,
            ExprKind::Unary {
                op: UnaryOp::Not,
                ..
            }
        ));
    }

    #[test]
    fn await_has_prefix_precedence_over_addition() {
        // -await 1 + 2 → (- (await 1)) + 2
        let r = parse_str("-await 1 + 2");
        assert!(r.diagnostics.is_empty());
        let (op, lhs, rhs) = binary_of(&r.program.items[0]);
        assert_eq!(*op, BinaryOp::Add);
        let ExprKind::Unary {
            op: UnaryOp::Neg,
            expr,
        } = &lhs.kind
        else {
            panic!("lhs should be unary neg");
        };
        let ExprKind::Await(inner) = &expr.kind else {
            panic!("unary operand should be await");
        };
        assert!(matches!(inner.kind, ExprKind::Integer(ref v) if v == "1"));
        assert!(matches!(rhs.kind, ExprKind::Integer(ref v) if v == "2"));
    }

    #[test]
    fn await_operand_includes_postfix_chain() {
        // await outer().x → Await(Field(Call(outer), x))
        let r = parse_str("await outer().x");
        assert!(r.diagnostics.is_empty());
        let Stmt::Expr(e) = &r.program.items[0] else {
            panic!("expected expr stmt");
        };
        let ExprKind::Await(inner) = &e.kind else {
            panic!("expected await expr");
        };
        let ExprKind::Field { target, field } = &inner.kind else {
            panic!("await operand should include field access");
        };
        assert_eq!(field.name, "x");
        let ExprKind::Call { callee, args } = &target.kind else {
            panic!("field target should be call");
        };
        assert!(args.is_empty());
        assert!(matches!(
            callee.kind,
            ExprKind::Ident(Ident { ref name, .. }) if name == "outer"
        ));
    }

    #[test]
    fn comparison_and_logical() {
        // a < b && c >= d  → (a < b) && (c >= d)
        let r = parse_str("a < b && c >= d");
        assert!(r.diagnostics.is_empty());
        let (op, lhs, rhs) = binary_of(&r.program.items[0]);
        assert_eq!(*op, BinaryOp::And);
        assert!(matches!(
            lhs.kind,
            ExprKind::Binary {
                op: BinaryOp::Lt,
                ..
            }
        ));
        assert!(matches!(
            rhs.kind,
            ExprKind::Binary {
                op: BinaryOp::Ge,
                ..
            }
        ));
    }

    #[test]
    fn coalesce_lowest() {
        // a ?? b || c → a ?? (b || c)
        let r = parse_str("a ?? b || c");
        assert!(r.diagnostics.is_empty());
        let (op, _, rhs) = binary_of(&r.program.items[0]);
        assert_eq!(*op, BinaryOp::Coalesce);
        assert!(matches!(
            rhs.kind,
            ExprKind::Binary {
                op: BinaryOp::Or,
                ..
            }
        ));
    }

    #[test]
    fn let_with_binary_init() {
        let r = parse_str("let n: int = 1 + 2 * 3");
        assert!(r.diagnostics.is_empty());
        let Stmt::Let(s) = &r.program.items[0] else {
            panic!();
        };
        assert!(matches!(
            s.init.kind,
            ExprKind::Binary {
                op: BinaryOp::Add,
                ..
            }
        ));
    }

    #[test]
    fn unclosed_paren_reports_error() {
        let r = parse_str("(1 + 2");
        assert!(!r.diagnostics.is_empty());
    }

    // ── 도메인 호출 ──

    fn domain_of(stmt: &Stmt) -> (&Ident, &[Expr]) {
        let Stmt::Expr(e) = stmt else {
            panic!("expected expr stmt");
        };
        let ExprKind::Domain { name, args } = &e.kind else {
            panic!("expected domain call, got {:?}", e.kind);
        };
        (name, args)
    }

    #[test]
    fn domain_call_with_string() {
        let r = parse_str(r#"@out "Hello""#);
        assert!(r.diagnostics.is_empty());
        let (name, args) = domain_of(&r.program.items[0]);
        assert_eq!(name.name, "out");
        assert_eq!(args.len(), 1);
        assert_eq!(plain_string(&args[0]), Some("Hello"));
    }

    #[test]
    fn domain_call_with_ident() {
        let r = parse_str(
            r#"
            let name: string = "Alice"
            @out name
            "#,
        );
        assert!(r.diagnostics.is_empty());
        assert_eq!(r.program.items.len(), 2);
        let (name, args) = domain_of(&r.program.items[1]);
        assert_eq!(name.name, "out");
        assert_eq!(args.len(), 1);
        assert!(matches!(args[0].kind, ExprKind::Ident(ref id) if id.name == "name"));
    }

    #[test]
    fn domain_call_without_args() {
        let r = parse_str("@in");
        assert!(r.diagnostics.is_empty());
        let (name, args) = domain_of(&r.program.items[0]);
        assert_eq!(name.name, "in");
        assert!(args.is_empty());
    }

    #[test]
    fn consecutive_domain_calls() {
        let r = parse_str(
            r#"
            @out "first"
            @out "second"
            @out 42
            "#,
        );
        assert!(r.diagnostics.is_empty());
        assert_eq!(r.program.items.len(), 3);
    }

    // ── 문자열 보간 ──

    fn segments_of(expr: &Expr) -> &[StringSegment] {
        let ExprKind::String(s) = &expr.kind else {
            panic!("expected string literal, got {:?}", expr.kind);
        };
        s
    }

    #[test]
    fn string_plain_single_segment() {
        let r = parse_str(r#""hello""#);
        assert!(r.diagnostics.is_empty());
        let Stmt::Expr(e) = &r.program.items[0] else {
            panic!()
        };
        let segs = segments_of(e);
        assert_eq!(segs.len(), 1);
        assert!(matches!(&segs[0], StringSegment::Str(s) if s == "hello"));
    }

    #[test]
    fn string_interpolation_basic() {
        let r = parse_str(r#""Hello, {name}!""#);
        assert!(r.diagnostics.is_empty());
        let Stmt::Expr(e) = &r.program.items[0] else {
            panic!()
        };
        let segs = segments_of(e);
        assert_eq!(segs.len(), 3);
        assert!(matches!(&segs[0], StringSegment::Str(s) if s == "Hello, "));
        assert!(matches!(&segs[1], StringSegment::Interp(_)));
        assert!(matches!(&segs[2], StringSegment::Str(s) if s == "!"));
    }

    #[test]
    fn string_interpolation_with_expression() {
        // {a + b}
        let r = parse_str(r#""sum: {a + b}""#);
        assert!(r.diagnostics.is_empty());
        let Stmt::Expr(e) = &r.program.items[0] else {
            panic!()
        };
        let segs = segments_of(e);
        assert_eq!(segs.len(), 2);
        let StringSegment::Interp(inner) = &segs[1] else {
            panic!()
        };
        assert!(matches!(
            inner.kind,
            ExprKind::Binary {
                op: BinaryOp::Add,
                ..
            }
        ));
    }

    #[test]
    fn string_escapes() {
        let r = parse_str(r#""a\tb\nc\{d\}e""#);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        let Stmt::Expr(e) = &r.program.items[0] else {
            panic!()
        };
        assert_eq!(plain_string(e), Some("a\tb\nc{d}e"));
    }

    #[test]
    fn string_unterminated_interp_reports_error() {
        let r = parse_str(r#""hello {name""#);
        assert!(!r.diagnostics.is_empty());
    }

    #[test]
    fn domain_does_not_swallow_let() {
        let r = parse_str(
            r#"
            @out "hi"
            let x = 1
            "#,
        );
        assert!(r.diagnostics.is_empty());
        assert_eq!(r.program.items.len(), 2);
        let (name, args) = domain_of(&r.program.items[0]);
        assert_eq!(name.name, "out");
        assert_eq!(args.len(), 1);
        assert!(matches!(r.program.items[1], Stmt::Let(_)));
    }

    fn route_args(stmt: &Stmt) -> &Vec<Expr> {
        let Stmt::Expr(e) = stmt else {
            panic!("expected expr stmt, got {stmt:?}");
        };
        let ExprKind::Domain { name, args } = &e.kind else {
            panic!("expected domain");
        };
        assert_eq!(name.name, "route");
        args
    }

    fn route_method_and_path(stmt: &Stmt) -> (String, String) {
        let args = route_args(stmt);
        assert_eq!(args.len(), 3, "@route expects 3 args (method, path, body)");
        let method = plain_string(&args[0])
            .expect("method must be plain string literal")
            .to_string();
        let path = plain_string(&args[1])
            .expect("path must be plain string literal")
            .to_string();
        assert!(
            matches!(args[2].kind, ExprKind::Block(_)),
            "body must be block, got {:?}",
            args[2].kind
        );
        (method, path)
    }

    #[test]
    fn route_get_static_path() {
        let r = parse_str(r#"@route GET /api/users { @out "list" }"#);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        let (method, path) = route_method_and_path(&r.program.items[0]);
        assert_eq!(method, "GET");
        assert_eq!(path, "/api/users");
    }

    #[test]
    fn route_with_param() {
        let r = parse_str(r#"@route POST /users/:id { @out "create" }"#);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        let (method, path) = route_method_and_path(&r.program.items[0]);
        assert_eq!(method, "POST");
        assert_eq!(path, "/users/:id");
    }

    #[test]
    fn route_with_multiple_params() {
        let r = parse_str(r#"@route DELETE /api/v1/users/:userId/posts/:postId { @out "x" }"#);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        let (method, path) = route_method_and_path(&r.program.items[0]);
        assert_eq!(method, "DELETE");
        assert_eq!(path, "/api/v1/users/:userId/posts/:postId");
    }

    #[test]
    fn route_wildcard() {
        let r = parse_str(r#"@route GET * { @out "fallback" }"#);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        let (method, path) = route_method_and_path(&r.program.items[0]);
        assert_eq!(method, "GET");
        assert_eq!(path, "*");
    }

    #[test]
    fn route_preserves_existing_single_arg_domains() {
        // `@out x` 와 `@html { ... }` 는 기존 동작 유지 — route 전용
        // 파싱이 다른 도메인에 누수되면 안 된다.
        let r = parse_str(
            r#"
            @out "hi"
            @out @html { @p "x" }
        "#,
        );
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert_eq!(r.program.items.len(), 2);
    }

    fn respond_args(stmt: &Stmt) -> &Vec<Expr> {
        let Stmt::Expr(e) = stmt else {
            panic!("expected expr stmt");
        };
        let ExprKind::Domain { name, args } = &e.kind else {
            panic!("expected domain");
        };
        assert_eq!(name.name, "respond");
        args
    }

    #[test]
    fn respond_with_status_and_object_payload() {
        let r = parse_str(r#"@respond 200 { ok: true, data: "x" }"#);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        let args = respond_args(&r.program.items[0]);
        assert_eq!(args.len(), 2);
        assert!(matches!(args[0].kind, ExprKind::Integer(ref n) if n == "200"));
        assert!(matches!(args[1].kind, ExprKind::Object(_)));
    }

    #[test]
    fn respond_with_empty_object_payload() {
        // `@respond 204 {}` — 빈 body 규약.
        let r = parse_str("@respond 204 {}");
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        let args = respond_args(&r.program.items[0]);
        assert_eq!(args.len(), 2);
        assert!(matches!(args[1].kind, ExprKind::Object(ref fs) if fs.is_empty()));
    }

    #[test]
    fn respond_without_payload_fills_void() {
        // payload 생략 — lowering 에서 void 로 채워지는 원재료.
        let r = parse_str("@respond 204");
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        let args = respond_args(&r.program.items[0]);
        assert_eq!(args.len(), 2);
        assert!(matches!(args[1].kind, ExprKind::Void));
    }

    #[test]
    fn respond_inside_route_body() {
        // route handler 안에서 respond — 가장 흔한 형태.
        let r = parse_str(r#"@route GET /api { @respond 200 { msg: "hi" } }"#);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        let args = route_args(&r.program.items[0]);
        let ExprKind::Block(block) = &args[2].kind else {
            panic!("route body must be block");
        };
        let Stmt::Expr(inner) = &block.stmts[0] else {
            panic!("expected expr stmt in handler");
        };
        let ExprKind::Domain { name, args: r_args } = &inner.kind else {
            panic!("expected domain call");
        };
        assert_eq!(name.name, "respond");
        assert_eq!(r_args.len(), 2);
    }

    // --- @server ---
    //
    // `@server { ... }` 는 generic `parse_domain_call` 의 block-인자 경로를
    // 그대로 탄다. 즉 AST 상에서는 `Domain { name: "server", args: [Block] }`
    // 형태로 나오며, 블록 안의 `@listen`/`@route`/`@out` 등은 block 의 stmt
    // 로 각각 도메인 호출로 보존된다. analyzer(C5a-3) 가 이 stmt 들을 3 갈래
    // (listen/routes/body_stmts) 로 분류한다.

    fn server_block_stmts(stmt: &Stmt) -> &[Stmt] {
        let Stmt::Expr(expr) = stmt else {
            panic!("expected expression statement at top level");
        };
        let ExprKind::Domain { name, args } = &expr.kind else {
            panic!("expected domain call, got {:?}", expr.kind);
        };
        assert_eq!(name.name, "server");
        assert_eq!(args.len(), 1, "@server should have single block argument");
        let ExprKind::Block(block) = &args[0].kind else {
            panic!("expected block arg, got {:?}", args[0].kind);
        };
        &block.stmts
    }

    #[test]
    fn server_empty_block() {
        let r = parse_str("@server {}");
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        let stmts = server_block_stmts(&r.program.items[0]);
        assert!(stmts.is_empty());
    }

    #[test]
    fn server_with_listen_and_route() {
        let r = parse_str(r#"@server { @listen 8080 @route GET / { @respond 200 {} } }"#);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        let stmts = server_block_stmts(&r.program.items[0]);
        assert_eq!(stmts.len(), 2);

        // 첫 stmt: @listen 8080 — Domain { name: "listen", args: [Integer] }
        let Stmt::Expr(first) = &stmts[0] else {
            panic!("expected expr stmt");
        };
        let ExprKind::Domain { name, args } = &first.kind else {
            panic!("expected domain");
        };
        assert_eq!(name.name, "listen");
        assert_eq!(args.len(), 1);
        assert!(matches!(args[0].kind, ExprKind::Integer(ref n) if n == "8080"));

        // 두 번째 stmt: @route GET / { ... } — Domain { name: "route", ... }
        let Stmt::Expr(second) = &stmts[1] else {
            panic!("expected expr stmt");
        };
        let ExprKind::Domain { name, .. } = &second.kind else {
            panic!("expected domain");
        };
        assert_eq!(name.name, "route");
    }

    #[test]
    fn server_preserves_misc_domain_stmts() {
        // SPEC §11.1 예제는 `@out "서버 시작..."` 같은 기타 도메인을
        // server 블록 안에서 사용한다. 파서는 이를 그대로 block stmt 로
        // 유지해야 하며 (drop/reject 금지), 분류는 analyzer 의 몫이다.
        let r = parse_str(r#"@server { @out "boot" @listen 80 @route GET / { @respond 200 {} } }"#);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        let stmts = server_block_stmts(&r.program.items[0]);
        assert_eq!(stmts.len(), 3);
        let Stmt::Expr(out_stmt) = &stmts[0] else {
            panic!("expected expr stmt");
        };
        let ExprKind::Domain { name, .. } = &out_stmt.kind else {
            panic!("expected domain call");
        };
        assert_eq!(name.name, "out");
    }

    #[test]
    fn server_multiple_routes() {
        let r = parse_str(
            r#"@server {
                @listen 8080
                @route GET /a { @respond 200 {} }
                @route POST /b { @respond 201 {} }
                @route GET /c/:id { @respond 200 {} }
            }"#,
        );
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        let stmts = server_block_stmts(&r.program.items[0]);
        // @listen + 3 routes.
        assert_eq!(stmts.len(), 4);
    }

    // ── SPEC §8 Import ──

    fn extract_import(stmt: &Stmt) -> &ImportStmt {
        if let Stmt::Import(i) = stmt { i } else {
            panic!("expected import stmt, got {stmt:?}");
        }
    }

    #[test]
    fn import_single_name() {
        let r = parse_str("import models.user.User");
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        let i = extract_import(&r.program.items[0]);
        assert!(!i.glob);
        assert_eq!(i.path.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), vec!["models", "user"]);
        assert_eq!(i.items.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), vec!["User"]);
    }

    #[test]
    fn import_brace_selection() {
        let r = parse_str("import models.post.{Post, PostCard}");
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        let i = extract_import(&r.program.items[0]);
        assert!(!i.glob);
        assert_eq!(i.path.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), vec!["models", "post"]);
        assert_eq!(i.items.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), vec!["Post", "PostCard"]);
    }

    #[test]
    fn import_glob() {
        let r = parse_str("import utils.format.*");
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        let i = extract_import(&r.program.items[0]);
        assert!(i.glob);
        assert_eq!(i.path.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), vec!["utils", "format"]);
        assert!(i.items.is_empty());
    }

    #[test]
    fn pub_struct_and_pub_const_accepted() {
        // pub struct / pub const 허용 — B3 import 지원을 위함.
        let r = parse_str(
            r#"pub struct User { name: string, age: int }
pub const PI: float = 3.14"#,
        );
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        assert!(matches!(r.program.items[0], Stmt::Struct(_)));
        assert!(matches!(r.program.items[1], Stmt::Const(_)));
    }
}
