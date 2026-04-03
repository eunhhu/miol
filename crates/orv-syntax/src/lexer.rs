use orv_diagnostics::{Diagnostic, DiagnosticBag, Label};
use orv_span::{FileId, Span, Spanned};

use crate::token::{TokenKind, lookup_keyword};

/// Returns the byte length of a UTF-8 character given its first byte.
#[expect(clippy::match_same_arms)]
const fn utf8_char_len(first_byte: u8) -> u32 {
    match first_byte {
        0..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xFF => 4,
        _ => 1,
    }
}

/// A single-pass byte scanner that produces a sequence of [`Spanned<TokenKind>`]
/// tokens along with any [`DiagnosticBag`] errors encountered.
pub struct Lexer<'src> {
    source: &'src [u8],
    file: FileId,
    pos: u32,
    diagnostics: DiagnosticBag,
}

impl<'src> Lexer<'src> {
    /// Creates a new `Lexer` for the given source string and file id.
    pub fn new(source: &'src str, file: FileId) -> Self {
        Self {
            source: source.as_bytes(),
            file,
            pos: 0,
            diagnostics: DiagnosticBag::new(),
        }
    }

    /// Scans the entire source and returns all tokens followed by an `Eof` token,
    /// plus the accumulated diagnostics.
    pub fn tokenize(mut self) -> (Vec<Spanned<TokenKind>>, DiagnosticBag) {
        let mut tokens = Vec::new();
        // Track nested string interpolation depth so that `}` inside an
        // interpolation resumes string scanning instead of producing `RBrace`.
        let mut interp_depth: u32 = 0;
        let mut brace_depth_stack: Vec<u32> = Vec::new();
        loop {
            let tok = self.next_token();
            let is_eof = *tok.node() == TokenKind::Eof;

            match tok.node() {
                TokenKind::StringInterpStart(_) => {
                    // Entering an interpolation hole — push the current brace
                    // nesting depth so we know when the matching `}` closes the
                    // interpolation rather than a block.
                    brace_depth_stack.push(interp_depth);
                    interp_depth = 0;
                    tokens.push(tok);
                }
                TokenKind::LBrace if !brace_depth_stack.is_empty() => {
                    interp_depth += 1;
                    tokens.push(tok);
                }
                TokenKind::RBrace if !brace_depth_stack.is_empty() && interp_depth == 0 => {
                    // This `}` closes the interpolation hole — resume string
                    // scanning instead of emitting RBrace.
                    let cont = self.continue_string();
                    match cont.node() {
                        TokenKind::StringInterpMiddle(_) => {
                            // Another `{` found — stay in interpolation mode.
                            // Push the old depth back then start a new level.
                            let old = brace_depth_stack.pop().unwrap_or(0);
                            brace_depth_stack.push(old);
                            interp_depth = 0;
                            tokens.push(cont);
                        }
                        TokenKind::StringInterpEnd(_) => {
                            // String closed — restore the outer brace depth.
                            interp_depth = brace_depth_stack.pop().unwrap_or(0);
                            tokens.push(cont);
                        }
                        _ => {
                            // Error token from unterminated string.
                            interp_depth = brace_depth_stack.pop().unwrap_or(0);
                            tokens.push(cont);
                        }
                    }
                }
                TokenKind::RBrace if !brace_depth_stack.is_empty() => {
                    interp_depth -= 1;
                    tokens.push(tok);
                }
                _ => {
                    tokens.push(tok);
                }
            }

            if is_eof {
                break;
            }
        }
        (tokens, self.diagnostics)
    }

    /// Resumes scanning a string after an interpolation `}` has been consumed by
    /// the caller. Returns either a `StringInterpMiddle` (another `{` found) or a
    /// `StringInterpEnd` (closing `"` found), or an error token if the string is
    /// unterminated.
    pub fn continue_string(&mut self) -> Spanned<TokenKind> {
        self.lex_string_body(false)
    }

    // ── private helpers ──────────────────────────────────────────────────────

    fn peek(&self) -> Option<&u8> {
        self.source.get(self.pos as usize)
    }

    fn peek_at(&self, offset: u32) -> Option<u8> {
        self.source.get((self.pos + offset) as usize).copied()
    }

    /// Advances past the current byte and returns it.
    ///
    /// # Panics
    ///
    /// Panics if called at end of source.
    #[expect(dead_code)]
    fn advance(&mut self) -> u8 {
        let b = self.source[self.pos as usize];
        self.pos += 1;
        b
    }

    /// Skips spaces and tabs (horizontal whitespace) but not newlines.
    fn skip_horizontal_whitespace(&mut self) {
        while let Some(&b' ' | &b'\t') = self.peek() {
            self.pos += 1;
        }
    }

    const fn make_token(&self, start: u32, kind: TokenKind) -> Spanned<TokenKind> {
        Spanned::new(kind, Span::new(self.file, start, self.pos))
    }

    // ── scanning ─────────────────────────────────────────────────────────────

    #[expect(clippy::too_many_lines)]
    fn next_token(&mut self) -> Spanned<TokenKind> {
        self.skip_horizontal_whitespace();

        let start = self.pos;

        let Some(&byte) = self.peek() else {
            return self.make_token(start, TokenKind::Eof);
        };

        // 1. Newlines
        if byte == b'\n' {
            self.pos += 1;
            return self.make_token(start, TokenKind::Newline);
        }
        if byte == b'\r' && self.peek_at(1) == Some(b'\n') {
            self.pos += 2;
            return self.make_token(start, TokenKind::Newline);
        }

        // 2. Single-char tokens
        match byte {
            b'@' => {
                self.pos += 1;
                return self.make_token(start, TokenKind::At);
            }
            b'%' => {
                self.pos += 1;
                return self.make_token(start, TokenKind::Percent);
            }
            b'{' => {
                self.pos += 1;
                return self.make_token(start, TokenKind::LBrace);
            }
            b'}' => {
                self.pos += 1;
                return self.make_token(start, TokenKind::RBrace);
            }
            b'(' => {
                self.pos += 1;
                return self.make_token(start, TokenKind::LParen);
            }
            b')' => {
                self.pos += 1;
                return self.make_token(start, TokenKind::RParen);
            }
            b'[' => {
                self.pos += 1;
                return self.make_token(start, TokenKind::LBracket);
            }
            b']' => {
                self.pos += 1;
                return self.make_token(start, TokenKind::RBracket);
            }
            b',' => {
                self.pos += 1;
                return self.make_token(start, TokenKind::Comma);
            }
            b'?' => {
                self.pos += 1;
                return self.make_token(start, TokenKind::Question);
            }
            b'#' => {
                self.pos += 1;
                return self.make_token(start, TokenKind::Hash);
            }
            b'$' => {
                self.pos += 1;
                return self.make_token(start, TokenKind::Dollar);
            }
            _ => {}
        }

        // 3. Multi-char with lookahead
        match byte {
            b':' => {
                self.pos += 1;
                if self.peek() == Some(&b':') {
                    self.pos += 1;
                    return self.make_token(start, TokenKind::ColonColon);
                }
                return self.make_token(start, TokenKind::Colon);
            }
            b'&' => {
                self.pos += 1;
                if self.peek() == Some(&b'&') {
                    self.pos += 1;
                    return self.make_token(start, TokenKind::AmpAmp);
                }
                return self.make_token(start, TokenKind::Amp);
            }
            b'=' => {
                self.pos += 1;
                if self.peek() == Some(&b'=') {
                    self.pos += 1;
                    return self.make_token(start, TokenKind::EqEq);
                }
                return self.make_token(start, TokenKind::Eq);
            }
            b'!' => {
                self.pos += 1;
                if self.peek() == Some(&b'=') {
                    self.pos += 1;
                    return self.make_token(start, TokenKind::BangEq);
                }
                return self.make_token(start, TokenKind::Bang);
            }
            b'<' => {
                self.pos += 1;
                if self.peek() == Some(&b'=') {
                    self.pos += 1;
                    return self.make_token(start, TokenKind::LtEq);
                }
                return self.make_token(start, TokenKind::Lt);
            }
            b'>' => {
                self.pos += 1;
                if self.peek() == Some(&b'=') {
                    self.pos += 1;
                    return self.make_token(start, TokenKind::GtEq);
                }
                return self.make_token(start, TokenKind::Gt);
            }
            b'+' => {
                self.pos += 1;
                if self.peek() == Some(&b'=') {
                    self.pos += 1;
                    return self.make_token(start, TokenKind::PlusEq);
                }
                return self.make_token(start, TokenKind::Plus);
            }
            b'-' => {
                self.pos += 1;
                if self.peek() == Some(&b'=') {
                    self.pos += 1;
                    return self.make_token(start, TokenKind::MinusEq);
                }
                if self.peek() == Some(&b'>') {
                    self.pos += 1;
                    return self.make_token(start, TokenKind::Arrow);
                }
                return self.make_token(start, TokenKind::Minus);
            }
            b'*' => {
                self.pos += 1;
                return self.make_token(start, TokenKind::Star);
            }
            b'|' => {
                self.pos += 1;
                if self.peek() == Some(&b'>') {
                    self.pos += 1;
                    return self.make_token(start, TokenKind::PipeGt);
                }
                if self.peek() == Some(&b'|') {
                    self.pos += 1;
                    return self.make_token(start, TokenKind::PipePipe);
                }
                return self.make_token(start, TokenKind::Pipe);
            }
            b'.' => {
                self.pos += 1;
                if self.peek() == Some(&b'.') {
                    self.pos += 1;
                    if self.peek() == Some(&b'=') {
                        self.pos += 1;
                        return self.make_token(start, TokenKind::DotDotEq);
                    }
                    if self.peek() == Some(&b'.') {
                        self.pos += 1;
                        return self.make_token(start, TokenKind::Ellipsis);
                    }
                    return self.make_token(start, TokenKind::DotDot);
                }
                return self.make_token(start, TokenKind::Dot);
            }
            _ => {}
        }

        // 4. Comments and slash
        if byte == b'/' {
            self.pos += 1;
            if self.peek() == Some(&b'/') {
                // Line comment: consume to end of line (not including newline)
                self.pos += 1;
                while let Some(&b) = self.peek() {
                    if b == b'\n' {
                        break;
                    }
                    self.pos += 1;
                }
                // Recurse to get the next real token
                return self.next_token();
            }
            if self.peek() == Some(&b'*') {
                self.pos += 1;
                return self.lex_block_comment(start);
            }
            return self.make_token(start, TokenKind::Slash);
        }

        // 5. Identifiers and keywords
        if byte.is_ascii_alphabetic() || byte == b'_' {
            return self.lex_ident(start);
        }

        // 6. Numbers
        if byte.is_ascii_digit() {
            return self.lex_number(start);
        }

        // 7. Strings
        if byte == b'"' {
            self.pos += 1;
            return self.lex_string_body(true);
        }

        // 8. Fallback: unknown character
        let char_len = utf8_char_len(byte);
        let char_bytes = &self.source[start as usize..(start + char_len) as usize];
        let char_str = std::str::from_utf8(char_bytes).unwrap_or("?");
        let span = Span::new(self.file, start, start + char_len);
        self.diagnostics.push(
            Diagnostic::error(format!("unexpected character `{char_str}`"))
                .with_label(Label::primary(span, "unexpected character")),
        );
        self.pos += char_len;
        self.make_token(start, TokenKind::Error)
    }

    fn lex_block_comment(&mut self, start: u32) -> Spanned<TokenKind> {
        // We have already consumed `/*`; now scan with nesting support.
        let mut depth: u32 = 1;
        loop {
            match self.peek() {
                None => {
                    let span = Span::new(self.file, start, self.pos);
                    self.diagnostics.push(
                        Diagnostic::error("unterminated block comment")
                            .with_label(Label::primary(span, "block comment opened here")),
                    );
                    return self.make_token(start, TokenKind::Error);
                }
                Some(&b'/') if self.peek_at(1) == Some(b'*') => {
                    self.pos += 2;
                    depth += 1;
                }
                Some(&b'*') if self.peek_at(1) == Some(b'/') => {
                    self.pos += 2;
                    depth -= 1;
                    if depth == 0 {
                        // Recurse to get the next real token
                        return self.next_token();
                    }
                }
                Some(_) => {
                    self.pos += 1;
                }
            }
        }
    }

    fn lex_ident(&mut self, start: u32) -> Spanned<TokenKind> {
        while let Some(&b) = self.peek() {
            if b.is_ascii_alphanumeric() || b == b'_' {
                self.pos += 1;
            } else {
                break;
            }
        }
        let text =
            std::str::from_utf8(&self.source[start as usize..self.pos as usize]).unwrap_or("");
        let kind = lookup_keyword(text).unwrap_or_else(|| TokenKind::Ident(text.to_owned()));
        self.make_token(start, kind)
    }

    fn lex_number(&mut self, start: u32) -> Spanned<TokenKind> {
        // Consume integer digits
        while let Some(&b) = self.peek() {
            if b.is_ascii_digit() {
                self.pos += 1;
            } else {
                break;
            }
        }

        // Check for float: `.` followed by a digit (not `..` or `.<ident>`)
        let is_float =
            self.peek() == Some(&b'.') && self.peek_at(1).is_some_and(|b| b.is_ascii_digit());

        if is_float {
            self.pos += 1; // consume `.`
            while let Some(&b) = self.peek() {
                if b.is_ascii_digit() {
                    self.pos += 1;
                } else {
                    break;
                }
            }
            let text =
                std::str::from_utf8(&self.source[start as usize..self.pos as usize]).unwrap_or("0");
            let value: f64 = text.parse().unwrap_or(0.0);
            self.make_token(start, TokenKind::FloatLiteral(value))
        } else {
            let text =
                std::str::from_utf8(&self.source[start as usize..self.pos as usize]).unwrap_or("0");
            let value: i64 = text.parse().unwrap_or(0);
            self.make_token(start, TokenKind::IntLiteral(value))
        }
    }

    /// Scans the body of a string literal starting just after the opening `"`.
    ///
    /// `is_start` is `true` for a fresh string and `false` for a continuation
    /// after an interpolation `}`.  Returns:
    /// - `StringLiteral` if `is_start` and no `{` found
    /// - `StringInterpStart` if `is_start` and `{` found
    /// - `StringInterpMiddle` if `!is_start` and `{` found
    /// - `StringInterpEnd` if `!is_start` and `"` found
    fn lex_string_body(&mut self, is_start: bool) -> Spanned<TokenKind> {
        let token_start = self.pos.saturating_sub(u32::from(is_start));
        let mut buf = String::new();

        loop {
            match self.peek() {
                None | Some(&b'\n') => {
                    let span = Span::new(self.file, token_start, self.pos);
                    self.diagnostics.push(
                        Diagnostic::error("unterminated string literal")
                            .with_label(Label::primary(span, "string opened here")),
                    );
                    return self.make_token(token_start, TokenKind::Error);
                }
                Some(&b'"') => {
                    self.pos += 1;
                    let kind = if is_start {
                        TokenKind::StringLiteral(buf)
                    } else {
                        TokenKind::StringInterpEnd(buf)
                    };
                    return self.make_token(token_start, kind);
                }
                Some(&b'{') => {
                    self.pos += 1;
                    let kind = if is_start {
                        TokenKind::StringInterpStart(buf)
                    } else {
                        TokenKind::StringInterpMiddle(buf)
                    };
                    return self.make_token(token_start, kind);
                }
                Some(&b'\\') => {
                    self.pos += 1;
                    match self.peek() {
                        Some(&b'n') => {
                            self.pos += 1;
                            buf.push('\n');
                        }
                        Some(&b't') => {
                            self.pos += 1;
                            buf.push('\t');
                        }
                        Some(&b'r') => {
                            self.pos += 1;
                            buf.push('\r');
                        }
                        Some(&b'\\') => {
                            self.pos += 1;
                            buf.push('\\');
                        }
                        Some(&b'"') => {
                            self.pos += 1;
                            buf.push('"');
                        }
                        Some(&b'{') => {
                            self.pos += 1;
                            buf.push('{');
                        }
                        Some(&b'}') => {
                            self.pos += 1;
                            buf.push('}');
                        }
                        _ => {
                            // Unknown escape — keep the backslash literally
                            buf.push('\\');
                        }
                    }
                }
                Some(&b) => {
                    // Handle UTF-8 multi-byte characters
                    let char_len = utf8_char_len(b);
                    let start_idx = self.pos as usize;
                    let end_idx = start_idx + char_len as usize;
                    if let Ok(s) = std::str::from_utf8(&self.source[start_idx..end_idx]) {
                        buf.push_str(s);
                    }
                    self.pos += char_len;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use orv_diagnostics::DiagnosticBag;
    use orv_span::FileId;

    use super::Lexer;
    use crate::token::TokenKind;

    fn lex(source: &str) -> Vec<TokenKind> {
        let (tokens, _) = lex_with_diags(source);
        tokens
    }

    fn lex_with_diags(source: &str) -> (Vec<TokenKind>, DiagnosticBag) {
        let file = FileId::new(0);
        let lexer = Lexer::new(source, file);
        let (spanned, diags) = lexer.tokenize();
        let kinds = spanned.into_iter().map(|s| s.node().clone()).collect();
        (kinds, diags)
    }

    #[test]
    fn empty_source() {
        assert_eq!(lex(""), vec![TokenKind::Eof]);
    }

    #[test]
    fn whitespace_only() {
        assert_eq!(lex("   \t  "), vec![TokenKind::Eof]);
    }

    #[test]
    fn single_char_tokens() {
        let tokens = lex("@%{}()[],:?#$");
        assert_eq!(
            tokens,
            vec![
                TokenKind::At,
                TokenKind::Percent,
                TokenKind::LBrace,
                TokenKind::RBrace,
                TokenKind::LParen,
                TokenKind::RParen,
                TokenKind::LBracket,
                TokenKind::RBracket,
                TokenKind::Comma,
                TokenKind::Colon,
                TokenKind::Question,
                TokenKind::Hash,
                TokenKind::Dollar,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn multi_char_tokens() {
        let tokens = lex(":: &&");
        assert_eq!(
            tokens,
            vec![TokenKind::ColonColon, TokenKind::AmpAmp, TokenKind::Eof]
        );
    }

    #[test]
    fn newlines() {
        let tokens = lex("@\n%");
        assert_eq!(
            tokens,
            vec![
                TokenKind::At,
                TokenKind::Newline,
                TokenKind::Percent,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn crlf_newline() {
        let tokens = lex("@\r\n%");
        assert_eq!(
            tokens,
            vec![
                TokenKind::At,
                TokenKind::Newline,
                TokenKind::Percent,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn unknown_char_emits_error() {
        let (tokens, diags) = lex_with_diags("~");
        assert_eq!(tokens, vec![TokenKind::Error, TokenKind::Eof]);
        assert!(diags.has_errors());
    }

    #[test]
    fn comparison_operators() {
        let tokens = lex("== != < <= > >=");
        assert_eq!(
            tokens,
            vec![
                TokenKind::EqEq,
                TokenKind::BangEq,
                TokenKind::Lt,
                TokenKind::LtEq,
                TokenKind::Gt,
                TokenKind::GtEq,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn assignment_operators() {
        let tokens = lex("= += -=");
        assert_eq!(
            tokens,
            vec![
                TokenKind::Eq,
                TokenKind::PlusEq,
                TokenKind::MinusEq,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn arithmetic_operators() {
        let tokens = lex("+ - * /");
        assert_eq!(
            tokens,
            vec![
                TokenKind::Plus,
                TokenKind::Minus,
                TokenKind::Star,
                TokenKind::Slash,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn arrow_and_pipe() {
        let tokens = lex("-> |> || | !");
        assert_eq!(
            tokens,
            vec![
                TokenKind::Arrow,
                TokenKind::PipeGt,
                TokenKind::PipePipe,
                TokenKind::Pipe,
                TokenKind::Bang,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn dot_variants() {
        let tokens = lex(". .. ..= ...");
        assert_eq!(
            tokens,
            vec![
                TokenKind::Dot,
                TokenKind::DotDot,
                TokenKind::DotDotEq,
                TokenKind::Ellipsis,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn line_comment_stripped() {
        // `@\n// comment\n%` → At, Newline, Newline, Percent, Eof
        let tokens = lex("@\n// comment\n%");
        assert_eq!(
            tokens,
            vec![
                TokenKind::At,
                TokenKind::Newline,
                TokenKind::Newline,
                TokenKind::Percent,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn doc_comment_stripped() {
        // `/// doc\n@` → Newline, At, Eof
        let tokens = lex("/// doc\n@");
        assert_eq!(
            tokens,
            vec![TokenKind::Newline, TokenKind::At, TokenKind::Eof]
        );
    }

    #[test]
    fn block_comment_stripped() {
        let tokens = lex("@ /* comment */ %");
        assert_eq!(
            tokens,
            vec![TokenKind::At, TokenKind::Percent, TokenKind::Eof]
        );
    }

    #[test]
    fn nested_block_comments() {
        let tokens = lex("@ /* outer /* inner */ still */ %");
        assert_eq!(
            tokens,
            vec![TokenKind::At, TokenKind::Percent, TokenKind::Eof]
        );
    }

    #[test]
    fn unterminated_block_comment() {
        let (_, diags) = lex_with_diags("/* not closed");
        assert!(diags.has_errors());
    }

    #[test]
    fn slash_alone_is_operator() {
        let tokens = lex("/");
        assert_eq!(tokens, vec![TokenKind::Slash, TokenKind::Eof]);
    }

    #[test]
    fn identifiers() {
        let tokens = lex("foo bar_baz _x A1");
        assert_eq!(
            tokens,
            vec![
                TokenKind::Ident("foo".into()),
                TokenKind::Ident("bar_baz".into()),
                TokenKind::Ident("_x".into()),
                TokenKind::Ident("A1".into()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn keywords_recognized() {
        let tokens = lex("let mut sig const function define");
        assert_eq!(
            tokens,
            vec![
                TokenKind::Let,
                TokenKind::Mut,
                TokenKind::Sig,
                TokenKind::Const,
                TokenKind::Function,
                TokenKind::Define,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn keyword_prefix_is_ident() {
        let tokens = lex("lettuce define2");
        assert_eq!(
            tokens,
            vec![
                TokenKind::Ident("lettuce".into()),
                TokenKind::Ident("define2".into()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn integer_literals() {
        let tokens = lex("0 42 12345");
        assert_eq!(
            tokens,
            vec![
                TokenKind::IntLiteral(0),
                TokenKind::IntLiteral(42),
                TokenKind::IntLiteral(12345),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    #[expect(clippy::approx_constant)]
    fn float_literals() {
        let tokens = lex("3.14 0.5");
        assert_eq!(
            tokens,
            vec![
                TokenKind::FloatLiteral(3.14),
                TokenKind::FloatLiteral(0.5),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn dot_after_int_is_not_float() {
        // `42..100` → IntLiteral(42), DotDot, IntLiteral(100)
        let tokens = lex("42..100");
        assert_eq!(
            tokens,
            vec![
                TokenKind::IntLiteral(42),
                TokenKind::DotDot,
                TokenKind::IntLiteral(100),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn number_before_dot_method() {
        // `42.len` → IntLiteral(42), Dot, Ident("len")
        let tokens = lex("42.len");
        assert_eq!(
            tokens,
            vec![
                TokenKind::IntLiteral(42),
                TokenKind::Dot,
                TokenKind::Ident("len".into()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn simple_string() {
        let tokens = lex(r#""hello""#);
        assert_eq!(
            tokens,
            vec![TokenKind::StringLiteral("hello".into()), TokenKind::Eof]
        );
    }

    #[test]
    fn string_with_escapes() {
        // "a\nb" in source (the \n is a literal backslash-n escape sequence)
        let tokens = lex("\"a\\nb\"");
        assert_eq!(
            tokens,
            vec![TokenKind::StringLiteral("a\nb".into()), TokenKind::Eof]
        );
    }

    #[test]
    fn string_interpolation_start() {
        // "Hello {" produces StringInterpStart("Hello ")
        let tokens = lex(r#""Hello {"#);
        assert_eq!(
            tokens,
            vec![
                TokenKind::StringInterpStart("Hello ".into()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn unterminated_string() {
        let (_, diags) = lex_with_diags("\"not closed");
        assert!(diags.has_errors());
    }

    #[test]
    fn string_with_unicode() {
        let tokens = lex("\"안녕\"");
        assert_eq!(
            tokens,
            vec![TokenKind::StringLiteral("안녕".into()), TokenKind::Eof]
        );
    }

    #[test]
    fn empty_string() {
        let tokens = lex("\"\"");
        assert_eq!(
            tokens,
            vec![TokenKind::StringLiteral(String::new()), TokenKind::Eof]
        );
    }

    #[test]
    fn escaped_braces_in_string() {
        // "\{not interp\}" → StringLiteral("{not interp}")
        let tokens = lex("\"\\{not interp\\}\"");
        assert_eq!(
            tokens,
            vec![
                TokenKind::StringLiteral("{not interp}".into()),
                TokenKind::Eof,
            ]
        );
    }
}
