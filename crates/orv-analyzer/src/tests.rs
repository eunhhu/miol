use orv_diagnostics::DiagnosticBag;
use orv_span::FileId;
use orv_syntax::{lexer::Lexer, parser::parse};
use pretty_assertions::assert_eq;

use crate::{Analysis, analyze, dump_hir};

fn analyze_source(src: &str) -> (Analysis, DiagnosticBag) {
    let file = FileId::new(0);
    let lexer = Lexer::new(src, file);
    let (tokens, lex_diags) = lexer.tokenize();
    assert!(!lex_diags.has_errors(), "lexer errors: {lex_diags:?}");
    let (module, parse_diags) = parse(tokens);
    assert!(!parse_diags.has_errors(), "parse errors: {parse_diags:?}");
    analyze(&module)
}

#[test]
fn lowers_function_body_with_resolved_identifier() {
    let (analysis, diagnostics) = analyze_source("let x = 1\nfunction foo() -> x\n");
    assert!(!diagnostics.has_errors());

    let output = dump_hir(&analysis.hir);
    assert!(output.contains("symbol#0 Binding"));
    assert!(output.contains("symbol#1 Function foo scope#1"));
    assert!(output.contains("x@symbol#0"));
}

#[test]
fn lowers_nested_scopes_in_order() {
    let (analysis, diagnostics) = analyze_source(
        "function foo() -> {\n    if true {\n        let x = 1\n        x\n    }\n}\n",
    );
    assert!(!diagnostics.has_errors());

    let output = dump_hir(&analysis.hir);
    assert!(output.contains("Function foo scope#1"));
    assert!(output.contains("block scope#2"));
    assert!(output.contains("then scope#3"));
    assert!(output.contains("block scope#4"));
}

#[test]
fn unresolved_identifier_stays_unresolved_in_hir() {
    let (analysis, diagnostics) = analyze_source("function foo() -> missing\n");
    assert!(diagnostics.has_errors());

    let output = dump_hir(&analysis.hir);
    assert!(output.contains("missing@unresolved"));
}

#[test]
fn duplicate_binding_keeps_original_symbol_reference() {
    let (analysis, diagnostics) =
        analyze_source("function foo() -> {\n    let x = 1\n    let x = 2\n    x\n}\n");
    assert!(diagnostics.has_errors());

    let output = dump_hir(&analysis.hir);
    assert!(output.contains("let symbol#1 x = 1"));
    assert!(output.contains("let symbol#1 x = 2"));
    assert!(output.contains("x@symbol#1"));
}

#[test]
fn hir_dump_matches_simple_function_snapshot() {
    let (analysis, diagnostics) = analyze_source("function greet(name: string) -> name\n");
    assert!(!diagnostics.has_errors());

    assert_eq!(
        dump_hir(&analysis.hir),
        "Module\n  symbol#0 Function greet scope#1\n    Param symbol#1 name: string\n    name@symbol#1\n"
    );
}

#[test]
fn route_atoms_do_not_trigger_unresolved_name_errors() {
    let (_, diagnostics) = analyze_source(
        "@server {\n  @listen 8080\n  @route GET /api/health {\n    return @response 200 { \"status\": \"ok\" }\n  }\n}\n",
    );

    let messages = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>();
    assert!(
        !messages
            .iter()
            .any(|message| message.contains("unresolved name")),
        "unexpected diagnostics: {messages:?}"
    );
}

#[test]
fn html_node_in_server_context_is_rejected() {
    let (_, diagnostics) =
        analyze_source("@server {\n  @listen 8080\n  @div {\n    @text \"bad\"\n  }\n}\n");

    let messages = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>();
    assert!(
        messages
            .iter()
            .any(|message| message.contains("node `@div` is not valid in @server context")),
        "unexpected diagnostics: {messages:?}"
    );
}
