use std::path::PathBuf;

use orv_span::FileId;
use orv_syntax::lexer::Lexer;
use orv_syntax::token::TokenKind;

fn fixtures_root() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("fixtures")
}

fn lex_fixture(name: &str) -> (Vec<TokenKind>, bool) {
    let path = fixtures_root().join("lexer").join(name);
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
    let lexer = Lexer::new(&source, FileId::new(0));
    let (tokens, diags) = lexer.tokenize();
    let kinds: Vec<TokenKind> = tokens.into_iter().map(|t| t.node().clone()).collect();
    (kinds, diags.has_errors())
}

#[test]
fn hello_fixture_lexes_cleanly() {
    let (tokens, has_errors) = lex_fixture("hello.orv");
    assert!(!has_errors, "hello.orv should lex without errors");
    assert!(tokens.contains(&TokenKind::At));
    assert!(tokens.contains(&TokenKind::StringLiteral("Hello, orv!".into())));
    assert!(tokens.last() == Some(&TokenKind::Eof));
}

#[test]
fn operators_fixture_lexes_cleanly() {
    let (tokens, has_errors) = lex_fixture("operators.orv");
    assert!(!has_errors, "operators.orv should lex without errors");
    assert!(tokens.contains(&TokenKind::Let));
    assert!(tokens.contains(&TokenKind::Plus));
    assert!(tokens.contains(&TokenKind::Star));
    assert!(tokens.contains(&TokenKind::GtEq));
    assert!(tokens.contains(&TokenKind::Return));
}

#[test]
fn string_interp_fixture_lexes_cleanly() {
    let (tokens, _has_errors) = lex_fixture("string-interp.orv");
    // The plain string for `name` lexes cleanly.
    assert!(tokens.contains(&TokenKind::StringLiteral("World".into())));
    // The interpolated string produces a StringInterpStart token.
    assert!(tokens.contains(&TokenKind::StringInterpStart("Hello, ".into())));
}
