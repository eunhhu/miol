//! Parser — 토큰 스트림을 AST로 변환.
//!
//! 1차 구현은 `let`/`let mut`/`let sig`, `const`, 리터럴 표현식, 식별자
//! 참조, void scope 자동 출력 대상인 표현식 스테이트먼트까지를 다룬다.
//! 함수/제어 흐름/도메인/struct는 다음 커밋에서 추가된다.

use crate::ast::{
    BinaryOp, Block, CatchClause, ConstStmt, Expr, ExprKind, FunctionBody, FunctionStmt, Ident,
    LetKind, LetStmt, ObjectField, Param, Pattern, Program, ReturnStmt, Stmt, StringSegment,
    StructField, StructStmt, TypeRef, TypeRefKind, UnaryOp, WhenArm,
};
use crate::lexer::lex_with_base_offset;
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
    let mut p = Parser::new(tokens, file);
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
}

impl Parser {
    fn new(tokens: Vec<Token>, file: FileId) -> Self {
        Self {
            tokens,
            pos: 0,
            file,
            diagnostics: Vec::new(),
        }
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
                .parse_function(false)
                .map(|s| Stmt::Function(Box::new(s))),
            TokenKind::Keyword(Keyword::Async) => {
                // `async function ...` — Async modifier 소비 후 function 파서로.
                self.advance();
                if !matches!(self.peek_kind(), TokenKind::Keyword(Keyword::Function)) {
                    self.error("expected `function` after `async`");
                    return None;
                }
                self.parse_function(true)
                    .map(|s| Stmt::Function(Box::new(s)))
            }
            TokenKind::Keyword(Keyword::Struct) => {
                self.parse_struct_decl().map(|s| Stmt::Struct(Box::new(s)))
            }
            TokenKind::Keyword(Keyword::Return) => self.parse_return().map(Stmt::Return),
            _ => {
                // `ident = expr` 대입을 표현식 스테이트먼트로 인식.
                // Pratt 파서가 식별자를 먼저 먹은 뒤 `=`를 만나면 대입으로 전환.
                let expr = self.parse_expr()?;
                if matches!(self.peek_kind(), TokenKind::Eq) {
                    // 좌변이 식별자여야 MVP 대입 가능
                    let ExprKind::Ident(ref id) = expr.kind else {
                        self.error("assignment target must be an identifier");
                        return Some(Stmt::Expr(expr));
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
        let name = self.parse_ident("type name")?;
        let mut ty = TypeRef {
            span: name.span,
            kind: TypeRefKind::Named(name),
        };
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
            // 후위 연산자 — 인덱스 `[idx]`.
            if matches!(self.peek_kind(), TokenKind::LBracket) && 30 >= min_bp {
                self.advance();
                let idx = self.parse_expr()?;
                let rbracket = self.expect(&TokenKind::RBracket, "`]`")?;
                let span = lhs.span.join(rbracket.span);
                lhs = Expr {
                    kind: ExprKind::Index {
                        target: Box::new(lhs),
                        index: Box::new(idx),
                    },
                    span,
                };
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
            let name = self.parse_ident("field name")?;
            self.expect(&TokenKind::Colon, "`:`")?;
            let value = self.parse_expr()?;
            let span = name.span.join(value.span);
            fields.push(ObjectField { name, value, span });
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
                // B2 MVP: identity — 피연산자를 평가해 그대로 반환. postfix
                // (call/field/index) 까지 포함해야 `await outer()` 가
                // `Await(Call(outer))` 로 파싱된다. parse_expr 는 binary
                // 까지 먹지만 MVP 는 관대하게 전체 식을 소비한다 (JS 와
                // 약간 다른 precedence 지만 차이가 드러나는 케이스가 없음).
                let expr = self.parse_expr()?;
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

    fn parse_function(&mut self, is_async: bool) -> Option<FunctionStmt> {
        let fn_tok = self.advance(); // `function`
        let name = self.parse_ident("function name")?;
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
            if matches!(self.peek_kind(), TokenKind::Comma) {
                self.advance();
            } else {
                break;
            }
        }
        self.expect(&TokenKind::RParen, "`)`")?;
        let return_ty = if self.eat(&TokenKind::Colon) {
            Some(self.parse_type()?)
        } else {
            None
        };
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
        Some(FunctionStmt {
            name,
            params,
            return_ty,
            body,
            is_async,
            span: fn_tok.span.join(end_span),
        })
    }

    fn parse_struct_decl(&mut self) -> Option<StructStmt> {
        let struct_tok = self.advance(); // `struct`
        let name = self.parse_ident("struct name")?;
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
        let then = self.parse_block()?;
        let else_branch = if matches!(self.peek_kind(), TokenKind::Keyword(Keyword::Else)) {
            self.advance();
            // `else if`는 else 분기에 새 if 표현식을 중첩.
            if matches!(self.peek_kind(), TokenKind::Keyword(Keyword::If)) {
                Some(Box::new(self.parse_if()?))
            } else {
                let block = self.parse_block()?;
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
        let var = self.parse_ident("loop variable")?;
        self.expect(&TokenKind::Keyword(Keyword::In), "`in`")?;
        let iter = self.parse_expr()?;
        let body = self.parse_block()?;
        let span = for_tok.span.join(body.span);
        Some(Expr {
            kind: ExprKind::For {
                var,
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
        let name_ident = Ident {
            name: name.clone(),
            span: at_tok.span,
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
                    let mut sub = Parser::new(inner_lex.tokens, span.file);
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
}
