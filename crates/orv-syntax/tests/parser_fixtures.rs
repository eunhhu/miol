use std::path::PathBuf;

use orv_span::FileId;
use orv_syntax::ast::{Expr, Item, Stmt};
use orv_syntax::lexer::Lexer;
use orv_syntax::parser;

fn fixtures_root() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("fixtures")
}

fn parse_fixture(name: &str) -> (orv_syntax::ast::Module, bool) {
    let path = fixtures_root().join("parser").join(name);
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
    let file_id = FileId::new(0);
    let lexer = Lexer::new(&source, file_id);
    let (tokens, lex_diags) = lexer.tokenize();
    let (module, parse_diags) = parser::parse(tokens);
    let has_errors = lex_diags.has_errors() || parse_diags.has_errors();
    (module, has_errors)
}

#[test]
fn hello_parses_cleanly() {
    let (module, has_errors) = parse_fixture("hello.orv");
    assert!(!has_errors, "hello.orv should parse without errors");
    assert!(!module.items.is_empty(), "should have at least one item");
    // The @io.out node should be present as a Stmt containing a Node expr
    match module.items[0].node() {
        Item::Stmt(Stmt::Expr(expr)) => {
            assert!(
                matches!(expr.node(), Expr::Node(_)),
                "expected a Node expr, got {:?}",
                expr.node()
            );
        }
        other => panic!("expected Stmt(Expr(Node(...))), got {:?}", other),
    }
}

#[test]
fn counter_parses_cleanly() {
    let (module, has_errors) = parse_fixture("counter.orv");
    assert!(!has_errors, "counter.orv should parse without errors");
    // Should have one item: a pub define
    assert_eq!(module.items.len(), 1);
    match module.items[0].node() {
        Item::Define(def) => {
            assert!(def.is_pub);
            assert_eq!(*def.name.node(), "CounterPage");
        }
        other => panic!("expected Define, got {:?}", other),
    }
}

#[test]
fn server_basic_parses_cleanly() {
    let (module, has_errors) = parse_fixture("server-basic.orv");
    assert!(!has_errors, "server-basic.orv should parse without errors");
    assert!(!module.items.is_empty());
    // The first item should be a @server node
    match module.items[0].node() {
        Item::Stmt(Stmt::Expr(expr)) => {
            assert!(
                matches!(expr.node(), Expr::Node(_)),
                "expected a Node expr for @server, got {:?}",
                expr.node()
            );
        }
        other => panic!("expected Stmt(Expr(Node(...))), got {:?}", other),
    }
}
