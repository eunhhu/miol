//! Parser — 토큰 스트림을 AST로 변환.
//!
//! 1차 구현은 `let`/`let mut`/`let sig`, `const`, 리터럴 표현식, 식별자
//! 참조, void scope 자동 출력 대상인 표현식 스테이트먼트까지를 다룬다.
//! 함수/제어 흐름/도메인/struct는 다음 커밋에서 추가된다.

use crate::ast::{
    ConstStmt, Expr, ExprKind, Ident, LetKind, LetStmt, Program, Stmt, TypeRef, TypeRefKind,
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

    // ── 표현식 (원자만) ──

    fn parse_expr(&mut self) -> Option<Expr> {
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
}
