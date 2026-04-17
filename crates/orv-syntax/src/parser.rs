//! Parser — 토큰 스트림을 AST로 변환.
//!
//! 1차 구현은 `let`/`let mut`/`let sig`, `const`, 리터럴 표현식, 식별자
//! 참조, void scope 자동 출력 대상인 표현식 스테이트먼트까지를 다룬다.
//! 함수/제어 흐름/도메인/struct는 다음 커밋에서 추가된다.

use crate::ast::{
    BinaryOp, ConstStmt, Expr, ExprKind, Ident, LetKind, LetStmt, Program, Stmt, TypeRef,
    TypeRefKind, UnaryOp,
};
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
            _ => self.parse_expr().map(Stmt::Expr),
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
        // nullable 접미사 `?`
        while self.eat(&TokenKind::Question) {
            let span = ty.span; // 간략 — `?` 위치까지 포함하려면 별도 추적 필요
            ty = TypeRef {
                span,
                kind: TypeRefKind::Nullable(Box::new(ty)),
            };
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
                self.advance();
                ExprKind::String(s.clone())
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
            TokenKind::At(_) => return self.parse_domain_call(),
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

    /// `@name arg1 arg2 ...` 형태의 도메인 호출을 파싱한다.
    ///
    /// MVP 규칙: `@name` 바로 다음에 오는 **원자(prefix) 표현식 하나**를
    /// 인자로 수집한다. 복수 인자와 `key=value` property, `{}` 본문은
    /// 이후 커밋에서 SPEC §9.3/§9.4에 따라 확장한다. 이 규칙은 fixture
    /// 01~04의 `@out "..."` 패턴을 정확히 처리한다.
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
            if let Some(arg) = self.parse_prefix() {
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
        assert!(matches!(s.init.kind, ExprKind::String(ref v) if v == "Alice"));
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
        assert!(matches!(e.kind, ExprKind::String(ref v) if v == "Hello, World!"));
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
        assert!(matches!(args[0].kind, ExprKind::String(ref v) if v == "Hello"));
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
