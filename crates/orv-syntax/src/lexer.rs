//! Lexer — `.orv` 소스를 토큰 스트림으로 변환.
//!
//! SPEC.md §2 어휘 구조 준수. 보간/이스케이프 해석은 파서 단계에 미루고,
//! 문자열 리터럴은 원문 내용만 담는다. 정규식은 `r"..."flags` 형식.

use crate::cursor::Cursor;
use crate::token::{Keyword, Token, TokenKind};
use orv_diagnostics::{ByteRange, Diagnostic, FileId, Span};

/// 렉싱 결과 — 토큰 스트림과 수집된 진단.
#[derive(Debug)]
pub struct LexResult {
    /// 토큰 스트림. 마지막 토큰은 항상 `Eof`.
    pub tokens: Vec<Token>,
    /// 수집된 진단(에러 포함).
    pub diagnostics: Vec<Diagnostic>,
    /// 소스 내 개행(`\n`) 바이트 오프셋 — 파서가 "같은 줄" 판정에 사용.
    ///
    /// `base_offset` 이 더해진 절대 오프셋이다. 정렬된 상태로 보장된다.
    pub newlines: Vec<u32>,
}

/// 소스 문자열과 파일 ID를 받아 토큰화한다.
#[must_use]
pub fn lex(source: &str, file: FileId) -> LexResult {
    lex_with_base_offset(source, file, 0)
}

/// [`lex`] 의 확장형 — 모든 토큰/진단 스팬에 `base_offset` 을 더해 기록한다.
///
/// 문자열 보간의 `{expr}` 내부 재-lex 처럼 부분 버퍼를 렉싱할 때, 결과
/// 스팬이 원본 소스 좌표를 가리키도록 하려면 이 함수를 직접 호출한다.
#[must_use]
pub fn lex_with_base_offset(source: &str, file: FileId, base_offset: u32) -> LexResult {
    let mut lx = Lexer::new(source, file, base_offset);
    lx.run();
    // 소스를 한 번 더 훑어 개행 오프셋을 수집한다. 파서 단계에서 두
    // 토큰이 같은 줄에 있는지 판정해야 하는데, 렉서는 공백을 건너뛰느라
    // 이 정보를 토큰에 담지 않는다. 단일 pass 의 비용이 미미하고 별도
    // 자료구조로 두면 기존 토큰 스트림이 바뀌지 않는다.
    let mut newlines = Vec::new();
    for (i, b) in source.bytes().enumerate() {
        if b == b'\n' {
            newlines.push(base_offset + u32::try_from(i).unwrap_or(u32::MAX));
        }
    }
    LexResult {
        tokens: lx.tokens,
        diagnostics: lx.diagnostics,
        newlines,
    }
}

struct Lexer<'src> {
    cursor: Cursor<'src>,
    file: FileId,
    tokens: Vec<Token>,
    diagnostics: Vec<Diagnostic>,
    base_offset: u32,
}

impl<'src> Lexer<'src> {
    fn new(source: &'src str, file: FileId, base_offset: u32) -> Self {
        Self {
            cursor: Cursor::new(source),
            file,
            tokens: Vec::new(),
            diagnostics: Vec::new(),
            base_offset,
        }
    }

    fn span(&self, start: u32, end: u32) -> Span {
        Span::new(
            self.file,
            ByteRange::new(start + self.base_offset, end + self.base_offset),
        )
    }

    fn push(&mut self, kind: TokenKind, start: u32, end: u32) {
        self.tokens.push(Token::new(kind, self.span(start, end)));
    }

    fn error(&mut self, message: impl Into<String>, start: u32, end: u32) {
        self.diagnostics
            .push(Diagnostic::error(message).with_primary(self.span(start, end), ""));
    }

    fn run(&mut self) {
        while !self.cursor.is_eof() {
            self.skip_whitespace_and_comments();
            if self.cursor.is_eof() {
                break;
            }
            self.next_token();
        }
        let end = self.cursor.offset();
        self.push(TokenKind::Eof, end, end);
    }

    fn skip_whitespace_and_comments(&mut self) {
        loop {
            self.cursor.eat_while(char::is_whitespace);
            if self.cursor.peek() == Some('/') && self.cursor.peek2() == Some('/') {
                self.cursor.eat_while(|c| c != '\n');
            } else {
                break;
            }
        }
    }

    fn next_token(&mut self) {
        let start = self.cursor.offset();
        let Some(c) = self.cursor.peek() else { return };

        // 식별자 / 키워드 / 불리언 / void
        if is_ident_start(c) {
            return self.lex_ident_or_keyword(start);
        }

        // 숫자
        if c.is_ascii_digit() {
            return self.lex_number(start);
        }

        // 문자열
        if c == '"' {
            return self.lex_string(start);
        }

        // 정규식 리터럴 `r"..."flags`
        if c == 'r' && self.cursor.peek2() == Some('"') {
            // 식별자 시작 문자이기도 하므로 식별자 체크보다 앞서야 하지만,
            // is_ident_start('r')가 true이므로 위에서 잡히기 전에 분기 필요.
            // 이 지점에 오려면 is_ident_start에서 걸렸어야 하는데, 순서상 이미 처리됨.
            // 따라서 이 분기는 식별자 분기 후 도달 불가 — 정규식은 식별자 분기에서 처리.
            unreachable!("regex handled in ident path");
        }

        // @ident
        if c == '@' {
            return self.lex_at(start);
        }

        // 구분자/연산자
        self.lex_punct(start);
    }

    fn lex_ident_or_keyword(&mut self, start: u32) {
        // 'r' 다음에 '"'가 오면 정규식 리터럴 — 식별자가 아님.
        if self.cursor.peek() == Some('r') && self.cursor.peek2() == Some('"') {
            self.cursor.advance(); // 'r'
            return self.lex_regex(start);
        }

        self.cursor.eat_while(is_ident_continue);
        let end = self.cursor.offset();
        let text = self.cursor.slice(start, end);

        let kind = match text {
            "true" => TokenKind::True,
            "false" => TokenKind::False,
            _ => match Keyword::match_keyword(text) {
                Some(kw) => TokenKind::Keyword(kw),
                None => TokenKind::Ident(text.to_string()),
            },
        };
        self.push(kind, start, end);
    }

    fn lex_number(&mut self, start: u32) {
        self.cursor.eat_while(|c| c.is_ascii_digit() || c == '_');

        // `.`이 숫자면 float (단, `..` 범위 연산자와 구분)
        let is_float = self.cursor.peek() == Some('.')
            && self.cursor.peek2().is_some_and(|c| c.is_ascii_digit());

        if is_float {
            self.cursor.advance(); // '.'
            self.cursor.eat_while(|c| c.is_ascii_digit() || c == '_');
            let end = self.cursor.offset();
            self.push(
                TokenKind::Float(self.cursor.slice(start, end).to_string()),
                start,
                end,
            );
        } else {
            let end = self.cursor.offset();
            self.push(
                TokenKind::Integer(self.cursor.slice(start, end).to_string()),
                start,
                end,
            );
        }
    }

    fn lex_string(&mut self, start: u32) {
        self.cursor.advance(); // 여는 '"'
        let body_start = self.cursor.offset();

        loop {
            match self.cursor.peek() {
                None => {
                    self.error("unterminated string literal", start, self.cursor.offset());
                    return;
                }
                Some('"') => {
                    let body_end = self.cursor.offset();
                    self.cursor.advance(); // 닫는 '"'
                    let end = self.cursor.offset();
                    let text = self.cursor.slice(body_start, body_end).to_string();
                    self.push(TokenKind::String(text), start, end);
                    return;
                }
                Some('\\') => {
                    self.cursor.advance();
                    // 이스케이프는 파서가 해석. 줄바꿈 허용(이후 보강 가능).
                    self.cursor.advance();
                }
                Some(_) => {
                    self.cursor.advance();
                }
            }
        }
    }

    fn lex_regex(&mut self, start: u32) {
        // 'r' 이미 소비됨. 다음 문자는 '"'.
        self.cursor.advance(); // 여는 '"'
        let body_start = self.cursor.offset();

        loop {
            match self.cursor.peek() {
                None => {
                    self.error("unterminated regex literal", start, self.cursor.offset());
                    return;
                }
                Some('"') => {
                    let body_end = self.cursor.offset();
                    self.cursor.advance(); // 닫는 '"'
                                           // 플래그 수집
                    let flags_start = self.cursor.offset();
                    self.cursor.eat_while(|c| c.is_ascii_alphabetic());
                    let flags_end = self.cursor.offset();
                    let pattern = self.cursor.slice(body_start, body_end).to_string();
                    let flags = self.cursor.slice(flags_start, flags_end).to_string();
                    self.push(TokenKind::Regex { pattern, flags }, start, flags_end);
                    return;
                }
                Some('\\') => {
                    self.cursor.advance();
                    self.cursor.advance();
                }
                Some(_) => {
                    self.cursor.advance();
                }
            }
        }
    }

    fn lex_at(&mut self, start: u32) {
        self.cursor.advance(); // '@'
        let name_start = self.cursor.offset();
        if self.cursor.peek().is_some_and(is_ident_start) {
            self.cursor.eat_while(is_ident_continue);
            let end = self.cursor.offset();
            let name = self.cursor.slice(name_start, end).to_string();
            self.push(TokenKind::At(name), start, end);
        } else {
            let end = self.cursor.offset();
            self.error("`@` must be followed by an identifier", start, end);
        }
    }

    fn lex_punct(&mut self, start: u32) {
        let c = self.cursor.advance().unwrap();
        let kind = match c {
            '{' => TokenKind::LBrace,
            '}' => TokenKind::RBrace,
            '(' => TokenKind::LParen,
            ')' => TokenKind::RParen,
            '[' => TokenKind::LBracket,
            ']' => TokenKind::RBracket,
            ',' => TokenKind::Comma,
            ';' => TokenKind::Semicolon,
            ':' => TokenKind::Colon,
            '~' => TokenKind::Tilde,
            '^' => TokenKind::Caret,
            '$' => TokenKind::Dollar,
            '.' => {
                if self.cursor.eat('.') {
                    if self.cursor.eat('=') {
                        TokenKind::DotDotEq
                    } else if self.cursor.eat('.') {
                        TokenKind::DotDotDot
                    } else {
                        TokenKind::DotDot
                    }
                } else {
                    TokenKind::Dot
                }
            }
            '?' => {
                if self.cursor.eat('?') {
                    TokenKind::QuestionQuestion
                } else {
                    TokenKind::Question
                }
            }
            '+' => {
                if self.cursor.eat('=') {
                    TokenKind::PlusEq
                } else {
                    TokenKind::Plus
                }
            }
            '-' => {
                if self.cursor.eat('>') {
                    TokenKind::Arrow
                } else if self.cursor.eat('=') {
                    TokenKind::MinusEq
                } else {
                    TokenKind::Minus
                }
            }
            '*' => {
                if self.cursor.eat('*') {
                    TokenKind::StarStar
                } else {
                    TokenKind::Star
                }
            }
            '/' => TokenKind::Slash,
            '%' => TokenKind::Percent,
            '=' => {
                if self.cursor.eat('=') {
                    TokenKind::EqEq
                } else {
                    TokenKind::Eq
                }
            }
            '!' => {
                if self.cursor.eat('=') {
                    TokenKind::BangEq
                } else {
                    TokenKind::Bang
                }
            }
            '<' => {
                if self.cursor.eat('=') {
                    TokenKind::LtEq
                } else if self.cursor.eat('<') {
                    TokenKind::LtLt
                } else {
                    TokenKind::Lt
                }
            }
            '>' => {
                if self.cursor.eat('=') {
                    TokenKind::GtEq
                } else if self.cursor.eat('>') {
                    TokenKind::GtGt
                } else {
                    TokenKind::Gt
                }
            }
            '&' => {
                if self.cursor.eat('&') {
                    TokenKind::AmpAmp
                } else {
                    TokenKind::Amp
                }
            }
            '|' => {
                if self.cursor.eat('|') {
                    TokenKind::PipePipe
                } else {
                    TokenKind::Pipe
                }
            }
            unknown => {
                let end = self.cursor.offset();
                self.error(format!("unknown character `{unknown}`"), start, end);
                return;
            }
        };
        let end = self.cursor.offset();
        self.push(kind, start, end);
    }
}

fn is_ident_start(c: char) -> bool {
    c == '_' || c.is_ascii_alphabetic()
}

fn is_ident_continue(c: char) -> bool {
    c == '_' || c.is_ascii_alphanumeric()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::Keyword;

    fn kinds(src: &str) -> Vec<TokenKind> {
        let r = lex(src, FileId(0));
        assert!(r.diagnostics.is_empty(), "diagnostics: {:?}", r.diagnostics);
        r.tokens.into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn empty_produces_eof() {
        let r = lex("", FileId(0));
        assert_eq!(r.tokens.len(), 1);
        assert_eq!(r.tokens[0].kind, TokenKind::Eof);
    }

    #[test]
    fn whitespace_and_comments_skipped() {
        let ks = kinds("  // comment\n  // another\n  42");
        assert_eq!(ks, vec![TokenKind::Integer("42".into()), TokenKind::Eof]);
    }

    #[test]
    fn keywords_matched() {
        let ks = kinds("let mut sig const function if else when for in");
        assert_eq!(
            ks,
            vec![
                TokenKind::Keyword(Keyword::Let),
                TokenKind::Keyword(Keyword::Mut),
                TokenKind::Keyword(Keyword::Sig),
                TokenKind::Keyword(Keyword::Const),
                TokenKind::Keyword(Keyword::Function),
                TokenKind::Keyword(Keyword::If),
                TokenKind::Keyword(Keyword::Else),
                TokenKind::Keyword(Keyword::When),
                TokenKind::Keyword(Keyword::For),
                TokenKind::Keyword(Keyword::In),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn booleans_and_void() {
        let ks = kinds("true false void");
        assert_eq!(
            ks,
            vec![
                TokenKind::True,
                TokenKind::False,
                TokenKind::Keyword(Keyword::Void),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn identifiers() {
        let ks = kinds("foo _bar baz_42");
        assert_eq!(
            ks,
            vec![
                TokenKind::Ident("foo".into()),
                TokenKind::Ident("_bar".into()),
                TokenKind::Ident("baz_42".into()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn integers_and_floats() {
        let ks = kinds("42 0 1_000 3.14 1_000.5");
        assert_eq!(
            ks,
            vec![
                TokenKind::Integer("42".into()),
                TokenKind::Integer("0".into()),
                TokenKind::Integer("1_000".into()),
                TokenKind::Float("3.14".into()),
                TokenKind::Float("1_000.5".into()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn range_vs_float_disambiguated() {
        // `0..10` — 범위, float 아님
        let ks = kinds("0..10");
        assert_eq!(
            ks,
            vec![
                TokenKind::Integer("0".into()),
                TokenKind::DotDot,
                TokenKind::Integer("10".into()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn strings_captured_as_raw_bytes() {
        let ks = kinds(r#""hello, {name}!""#);
        assert_eq!(
            ks,
            vec![TokenKind::String("hello, {name}!".into()), TokenKind::Eof]
        );
    }

    #[test]
    fn regex_with_flags() {
        let ks = kinds(r#"r"[a-z]+"gi"#);
        assert_eq!(
            ks,
            vec![
                TokenKind::Regex {
                    pattern: "[a-z]+".into(),
                    flags: "gi".into(),
                },
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn at_directive() {
        let ks = kinds("@out @route @db.find");
        assert_eq!(
            ks,
            vec![
                TokenKind::At("out".into()),
                TokenKind::At("route".into()),
                TokenKind::At("db".into()),
                TokenKind::Dot,
                TokenKind::Ident("find".into()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn operators_multi_char() {
        let ks = kinds("== != <= >= && || ?? .. ..= ... -> += -= ** << >>");
        assert_eq!(
            ks,
            vec![
                TokenKind::EqEq,
                TokenKind::BangEq,
                TokenKind::LtEq,
                TokenKind::GtEq,
                TokenKind::AmpAmp,
                TokenKind::PipePipe,
                TokenKind::QuestionQuestion,
                TokenKind::DotDot,
                TokenKind::DotDotEq,
                TokenKind::DotDotDot,
                TokenKind::Arrow,
                TokenKind::PlusEq,
                TokenKind::MinusEq,
                TokenKind::StarStar,
                TokenKind::LtLt,
                TokenKind::GtGt,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn punct_singles() {
        let ks = kinds("{ } ( ) [ ] , ; : . ? + - * / % = ! < > & | ^ ~");
        assert_eq!(
            ks,
            vec![
                TokenKind::LBrace,
                TokenKind::RBrace,
                TokenKind::LParen,
                TokenKind::RParen,
                TokenKind::LBracket,
                TokenKind::RBracket,
                TokenKind::Comma,
                TokenKind::Semicolon,
                TokenKind::Colon,
                TokenKind::Dot,
                TokenKind::Question,
                TokenKind::Plus,
                TokenKind::Minus,
                TokenKind::Star,
                TokenKind::Slash,
                TokenKind::Percent,
                TokenKind::Eq,
                TokenKind::Bang,
                TokenKind::Lt,
                TokenKind::Gt,
                TokenKind::Amp,
                TokenKind::Pipe,
                TokenKind::Caret,
                TokenKind::Tilde,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn multibyte_source_offsets() {
        // '한' = 3 bytes, '"한글"' → span covers 8 bytes total
        let r = lex(r#""한글""#, FileId(0));
        assert!(r.diagnostics.is_empty());
        assert_eq!(r.tokens.len(), 2);
        assert_eq!(r.tokens[0].kind, TokenKind::String("한글".into()));
        assert_eq!(r.tokens[0].span.range.len(), 8); // 2 quotes + 2*3 bytes
    }

    #[test]
    fn unterminated_string_reports_error() {
        let r = lex(r#""hello"#, FileId(0));
        assert_eq!(r.diagnostics.len(), 1);
        assert_eq!(r.diagnostics[0].severity, orv_diagnostics::Severity::Error);
    }

    #[test]
    fn unknown_character_reports_error() {
        let r = lex("`", FileId(0));
        assert_eq!(r.diagnostics.len(), 1);
    }

    #[test]
    fn let_declaration_end_to_end() {
        let ks = kinds(r#"let name: string = "Alice""#);
        assert_eq!(
            ks,
            vec![
                TokenKind::Keyword(Keyword::Let),
                TokenKind::Ident("name".into()),
                TokenKind::Colon,
                TokenKind::Ident("string".into()),
                TokenKind::Eq,
                TokenKind::String("Alice".into()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn function_declaration_end_to_end() {
        let ks = kinds("function add(a: int, b: int): int -> { a + b }");
        assert_eq!(
            ks,
            vec![
                TokenKind::Keyword(Keyword::Function),
                TokenKind::Ident("add".into()),
                TokenKind::LParen,
                TokenKind::Ident("a".into()),
                TokenKind::Colon,
                TokenKind::Ident("int".into()),
                TokenKind::Comma,
                TokenKind::Ident("b".into()),
                TokenKind::Colon,
                TokenKind::Ident("int".into()),
                TokenKind::RParen,
                TokenKind::Colon,
                TokenKind::Ident("int".into()),
                TokenKind::Arrow,
                TokenKind::LBrace,
                TokenKind::Ident("a".into()),
                TokenKind::Plus,
                TokenKind::Ident("b".into()),
                TokenKind::RBrace,
                TokenKind::Eof,
            ]
        );
    }
}
