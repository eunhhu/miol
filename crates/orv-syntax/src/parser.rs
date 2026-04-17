//! Parser — 토큰 스트림을 AST로 변환.
//!
//! 1차 구현은 `let`/`let mut`/`let sig`, `const`, 리터럴 표현식, 식별자
//! 참조, void scope 자동 출력 대상인 표현식 스테이트먼트까지를 다룬다.
//! 함수/제어 흐름/도메인/struct는 다음 커밋에서 추가된다.

use crate::ast::{
    BinaryOp, Block, ConstStmt, Expr, ExprKind, FunctionBody, FunctionStmt, Ident, LetKind,
    LetStmt, Param, Pattern, Program, ReturnStmt, Stmt, StringSegment, TypeRef, TypeRefKind,
    UnaryOp, WhenArm,
};
use crate::lexer::lex;
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
                .parse_function()
                .map(|s| Stmt::Function(Box::new(s))),
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
            if matches!(self.peek_kind(), TokenKind::DotDot | TokenKind::DotDotEq)
                && 15 >= min_bp
            {
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
                let lparen = self.advance();
                let inner = self.parse_expr()?;
                let rparen = self.expect(&TokenKind::RParen, "`)`")?;
                let span = lparen.span.join(rparen.span);
                return Some(Expr {
                    kind: ExprKind::Paren(Box::new(inner)),
                    span,
                });
            }
            TokenKind::LBracket => return self.parse_array_literal(),
            TokenKind::LBrace => return self.parse_block_expr(),
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

    fn parse_function(&mut self) -> Option<FunctionStmt> {
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
            span: fn_tok.span.join(end_span),
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
        let end_span = else_branch
            .as_ref()
            .map_or(then.span, |e| e.span);
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

    /// 문자열 원문(따옴표 제외)을 보간 세그먼트로 쪼갠다.
    ///
    /// SPEC §2.4 규칙:
    /// - `{expr}`은 보간, 중괄호 내부는 orv 표현식
    /// - `\{`, `\}`, `\n`, `\t`, `\\`, `\"`는 이스케이프
    /// - 중괄호가 짝이 안 맞으면 진단 수집 후 리터럴로 처리
    fn parse_string_segments(&mut self, raw: &str, span: Span) -> Vec<StringSegment> {
        let mut segments = Vec::new();
        let mut literal = String::new();
        let mut chars = raw.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\\' => {
                    // 이스케이프 해제
                    match chars.next() {
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
                }
                '{' => {
                    // 보간 시작
                    if !literal.is_empty() {
                        segments.push(StringSegment::Str(std::mem::take(&mut literal)));
                    }
                    // `{...}` 내부 원문 수집 (중첩 `{}` 미지원 MVP — 1단계만)
                    let mut inner = String::new();
                    let mut depth = 1u32;
                    for ic in chars.by_ref() {
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
                    // 내부를 별도 렉서+파서로 돌려 표현식 추출
                    let inner_lex = lex(&inner, span.file);
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
    fn is_domain_arg_start(&self) -> bool {
        matches!(
            self.peek_kind(),
            TokenKind::Integer(_)
                | TokenKind::Float(_)
                | TokenKind::String(_)
                | TokenKind::True
                | TokenKind::False
                | TokenKind::Keyword(Keyword::Void)
                | TokenKind::Ident(_)
                | TokenKind::Regex { .. }
                | TokenKind::At(_)
                | TokenKind::LParen
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lex;
    use orv_diagnostics::FileId;

    fn parse_str(src: &str) -> ParseResult {
        let lx = lex(src, FileId(0));
        assert!(lx.diagnostics.is_empty(), "lex errors: {:?}", lx.diagnostics);
        parse(lx.tokens, FileId(0))
    }

    /// 단일 리터럴 세그먼트 문자열인지 검사하고 내용 반환.
    fn plain_string(expr: &Expr) -> Option<&str> {
        let ExprKind::String(segs) = &expr.kind else { return None };
        if segs.len() != 1 {
            return None;
        }
        let StringSegment::Str(s) = &segs[0] else { return None };
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
        assert!(matches!(rhs.kind, ExprKind::Binary { op: BinaryOp::Mul, .. }));
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
        assert!(matches!(inner.kind, ExprKind::Binary { op: BinaryOp::Add, .. }));
    }

    #[test]
    fn pow_is_right_associative() {
        // 2 ** 3 ** 2 → 2 ** (3 ** 2)
        let r = parse_str("2 ** 3 ** 2");
        assert!(r.diagnostics.is_empty());
        let (op, _, rhs) = binary_of(&r.program.items[0]);
        assert_eq!(*op, BinaryOp::Pow);
        assert!(matches!(rhs.kind, ExprKind::Binary { op: BinaryOp::Pow, .. }));
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
        assert!(matches!(lhs.kind, ExprKind::Unary { op: UnaryOp::Not, .. }));
    }

    #[test]
    fn comparison_and_logical() {
        // a < b && c >= d  → (a < b) && (c >= d)
        let r = parse_str("a < b && c >= d");
        assert!(r.diagnostics.is_empty());
        let (op, lhs, rhs) = binary_of(&r.program.items[0]);
        assert_eq!(*op, BinaryOp::And);
        assert!(matches!(lhs.kind, ExprKind::Binary { op: BinaryOp::Lt, .. }));
        assert!(matches!(rhs.kind, ExprKind::Binary { op: BinaryOp::Ge, .. }));
    }

    #[test]
    fn coalesce_lowest() {
        // a ?? b || c → a ?? (b || c)
        let r = parse_str("a ?? b || c");
        assert!(r.diagnostics.is_empty());
        let (op, _, rhs) = binary_of(&r.program.items[0]);
        assert_eq!(*op, BinaryOp::Coalesce);
        assert!(matches!(rhs.kind, ExprKind::Binary { op: BinaryOp::Or, .. }));
    }

    #[test]
    fn let_with_binary_init() {
        let r = parse_str("let n: int = 1 + 2 * 3");
        assert!(r.diagnostics.is_empty());
        let Stmt::Let(s) = &r.program.items[0] else {
            panic!();
        };
        assert!(matches!(s.init.kind, ExprKind::Binary { op: BinaryOp::Add, .. }));
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
        let Stmt::Expr(e) = &r.program.items[0] else { panic!() };
        let segs = segments_of(e);
        assert_eq!(segs.len(), 1);
        assert!(matches!(&segs[0], StringSegment::Str(s) if s == "hello"));
    }

    #[test]
    fn string_interpolation_basic() {
        let r = parse_str(r#""Hello, {name}!""#);
        assert!(r.diagnostics.is_empty());
        let Stmt::Expr(e) = &r.program.items[0] else { panic!() };
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
        let Stmt::Expr(e) = &r.program.items[0] else { panic!() };
        let segs = segments_of(e);
        assert_eq!(segs.len(), 2);
        let StringSegment::Interp(inner) = &segs[1] else { panic!() };
        assert!(matches!(inner.kind, ExprKind::Binary { op: BinaryOp::Add, .. }));
    }

    #[test]
    fn string_escapes() {
        let r = parse_str(r#""a\tb\nc\{d\}e""#);
        assert!(r.diagnostics.is_empty(), "{:?}", r.diagnostics);
        let Stmt::Expr(e) = &r.program.items[0] else { panic!() };
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
}
