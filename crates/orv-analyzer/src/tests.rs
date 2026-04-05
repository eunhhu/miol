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

#[test]
fn env_node_uses_contextual_integer_type() {
    let (_, diagnostics) = analyze_source("let port: i32 = @env PORT\n");

    let messages = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>();
    assert!(
        !messages
            .iter()
            .any(|message| message.contains("type mismatch")),
        "unexpected diagnostics: {messages:?}"
    );
}

#[test]
fn env_node_defaults_to_string_without_context() {
    let (_, diagnostics) = analyze_source("let secret: string = @env JWT_SECRET\n");

    let messages = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>();
    assert!(
        !messages
            .iter()
            .any(|message| message.contains("type mismatch")),
        "unexpected diagnostics: {messages:?}"
    );
}

#[test]
fn struct_object_literal_matches_declared_shape() {
    let (_, diagnostics) = analyze_source(
        "struct User {\n  name: string\n  age: i32\n}\nlet user: User = { name: \"sun\", age: 1 }\n",
    );

    let messages = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>();
    assert!(
        !messages.iter().any(|message| {
            message.contains("missing field")
                || message.contains("extra field")
                || message.contains("type mismatch")
        }),
        "unexpected diagnostics: {messages:?}"
    );
}

#[test]
fn struct_object_literal_reports_missing_field() {
    let (_, diagnostics) = analyze_source(
        "struct User {\n  name: string\n  age: i32\n}\nlet user: User = { name: \"sun\" }\n",
    );

    let messages = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>();
    assert!(
        messages
            .iter()
            .any(|message| message.contains("missing field `age`")),
        "unexpected diagnostics: {messages:?}"
    );
}

#[test]
fn function_return_type_mismatch_is_reported() {
    let (_, diagnostics) = analyze_source("function bad(): bool -> 1\n");

    let messages = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>();
    assert!(
        messages
            .iter()
            .any(|message| message.contains("type mismatch: expected `bool`, found `i64`")),
        "unexpected diagnostics: {messages:?}"
    );
}

#[test]
fn nullable_ident_is_narrowed_in_if_then_branch() {
    let (_, diagnostics) = analyze_source(
        "struct User {\n  name: string\n}\nfunction greet(user: User?): string -> {\n  if user != void {\n    user.name\n  } else {\n    \"anonymous\"\n  }\n}\n",
    );

    let messages = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>();
    assert!(
        !messages.iter().any(|message| {
            message.contains("type mismatch")
                || message.contains("unknown field")
                || message.contains("attempted to call")
        }),
        "unexpected diagnostics: {messages:?}"
    );
}

#[test]
fn nullable_ident_is_narrowed_in_if_else_branch() {
    let (_, diagnostics) = analyze_source(
        "struct User {\n  name: string\n}\nfunction greet(user: User?): string -> {\n  if user == void {\n    \"anonymous\"\n  } else {\n    user.name\n  }\n}\n",
    );

    let messages = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>();
    assert!(
        !messages.iter().any(|message| {
            message.contains("type mismatch")
                || message.contains("unknown field")
                || message.contains("attempted to call")
        }),
        "unexpected diagnostics: {messages:?}"
    );
}

#[test]
fn named_arguments_can_be_reordered_for_declared_functions() {
    let (_, diagnostics) = analyze_source(
        "function add(a: i32, b: i32): i32 -> a + b\nlet total: i32 = add(b=10, a=30)\n",
    );

    let messages = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>();
    assert!(
        !messages.iter().any(|message| message.contains("parameter")
            || message.contains("missing required")
            || message.contains("type mismatch")),
        "unexpected diagnostics: {messages:?}"
    );
}

#[test]
fn named_arguments_report_unknown_parameter() {
    let (_, diagnostics) = analyze_source(
        "function add(a: i32, b: i32): i32 -> a + b\nlet total: i32 = add(c=10, a=30)\n",
    );

    let messages = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>();
    assert!(
        messages
            .iter()
            .any(|message| message.contains("has no parameter named `c`")),
        "unexpected diagnostics: {messages:?}"
    );
}

#[test]
fn named_arguments_report_missing_required_parameter() {
    let (_, diagnostics) =
        analyze_source("function add(a: i32, b: i32): i32 -> a + b\nlet total: i32 = add(a=30)\n");

    let messages = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>();
    assert!(
        messages
            .iter()
            .any(|message| message.contains("missing required argument `b`")),
        "unexpected diagnostics: {messages:?}"
    );
}

#[test]
fn hash_map_literal_matches_declared_type() {
    let (_, diagnostics) = analyze_source(
        "let scores: HashMap<string, i32> = #{ alice: 1, bob: 2 }\nlet total: i32 = scores.len()\nlet keys: Vec<string> = scores.keys()\nlet values: Vec<i32> = scores.values()\n",
    );

    let messages = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>();
    assert!(
        !messages
            .iter()
            .any(|message| message.contains("type mismatch") || message.contains("cannot infer")),
        "unexpected diagnostics: {messages:?}"
    );
}

#[test]
fn empty_map_literal_requires_context() {
    let (_, diagnostics) = analyze_source("let scores = #{}\n");

    let messages = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>();
    assert!(
        messages
            .iter()
            .any(|message| message.contains("cannot infer the value type of an empty map literal")),
        "unexpected diagnostics: {messages:?}"
    );
}

#[test]
fn when_enum_variant_binding_resolves_inside_arm_body() {
    let (analysis, diagnostics) = analyze_source(
        "enum Result {\n  Ok(i32)\n  Err(string)\n}\nfunction unwrap(result: Result): i32 -> when result {\n  Result.Ok(value) -> value\n  Result.Err(_) -> 0\n}\n",
    );

    let messages = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>();
    assert!(
        !diagnostics.has_errors(),
        "unexpected diagnostics: {messages:?}"
    );

    let output = dump_hir(&analysis.hir);
    assert!(output.contains("when result@symbol#"));
    assert!(output.contains("arm scope#"));
    assert!(output.contains("value@symbol#"));
}

#[test]
fn when_requires_exhaustive_enum_coverage_without_wildcard() {
    let (_, diagnostics) = analyze_source(
        "enum Result {\n  Ok(i32)\n  Err(string)\n}\nfunction unwrap(result: Result): i32 -> when result {\n  Result.Ok(value) -> value\n}\n",
    );

    let messages = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>();
    assert!(
        messages
            .iter()
            .any(|message| message.contains("non-exhaustive `when`")),
        "unexpected diagnostics: {messages:?}"
    );
}

#[test]
fn when_variant_payload_arity_is_checked() {
    let (_, diagnostics) = analyze_source(
        "enum Result {\n  Ok(i32)\n}\nfunction unwrap(result: Result): i32 -> when result {\n  Result.Ok(left, right) -> left\n}\n",
    );

    let messages = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>();
    assert!(
        messages
            .iter()
            .any(|message| message.contains("expects 1 field(s), found 2")),
        "unexpected diagnostics: {messages:?}"
    );
}

#[test]
fn route_fetch_response_shape_is_available_in_same_scope() {
    let (_, diagnostics) = analyze_source(
        "@server {\n  let getUsers = @route GET /api/users {\n    let users = [\"kim\"]\n    return @response 200 { users: users }\n  }\n\n  @route GET / {\n    let page = @html {\n      @body {\n        let sig data = await getUsers.fetch()\n        data.users.len()\n      }\n    }\n    @serve page\n  }\n}\n",
    );

    let messages = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>();
    assert!(
        !messages.iter().any(|message| {
            message.contains("type mismatch")
                || message.contains("unknown field")
                || message.contains("`.fetch()`")
        }),
        "unexpected diagnostics: {messages:?}"
    );
}

#[test]
fn route_fetch_requires_path_params() {
    let (_, diagnostics) = analyze_source(
        "@server {\n  let getUser = @route GET /api/users/:id {\n    return @response 200 { user: \"kim\" }\n  }\n\n  @route GET / {\n    let page = @html {\n      @body {\n        await getUser.fetch()\n      }\n    }\n    @serve page\n  }\n}\n",
    );

    let messages = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>();
    assert!(
        messages
            .iter()
            .any(|message| { message.contains("requires `param={...}`") }),
        "unexpected diagnostics: {messages:?}"
    );
}

#[test]
fn route_fetch_rejects_body_on_get_routes() {
    let (_, diagnostics) = analyze_source(
        "@server {\n  let getUser = @route GET /api/users/:id {\n    return @response 200 { user: \"kim\" }\n  }\n\n  @route GET / {\n    let page = @html {\n      @body {\n        await getUser.fetch(param={ id: \"42\" }, body={ force: \"true\" })\n      }\n    }\n    @serve page\n  }\n}\n",
    );

    let messages = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>();
    assert!(
        messages
            .iter()
            .any(|message| message.contains("does not accept a request body")),
        "unexpected diagnostics: {messages:?}"
    );
}

#[test]
fn fetch_on_non_route_symbol_is_rejected() {
    let (_, diagnostics) =
        analyze_source("function bad() -> {\n  let value = 1\n  value.fetch()\n}\n");

    let messages = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>();
    assert!(
        messages
            .iter()
            .any(|message| message.contains("`.fetch()` is only valid on route references")),
        "unexpected diagnostics: {messages:?}"
    );
}

#[test]
fn route_request_accessors_have_expected_types() {
    let (_, diagnostics) = analyze_source(
        "@server {\n  let createUser = @route POST /api/users/:id {\n    let id: string? = @param \"id\"\n    let page: string? = @query \"page\"\n    let auth: string? = @header \"Authorization\"\n    let method: string = @method\n    let path: string = @path\n    let ctx: string = @context \"requestId\"\n    let payload: HashMap<string, string> = @body\n    return @response 200 { ok: true }\n  }\n}\n",
    );

    let messages = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>();
    assert!(
        !messages.iter().any(|message| {
            message.contains("type mismatch")
                || message.contains("only valid inside a route handler")
                || message.contains("expects")
        }),
        "unexpected diagnostics: {messages:?}"
    );
}

#[test]
fn route_param_accessor_rejects_unknown_path_key() {
    let (_, diagnostics) = analyze_source(
        "@server {\n  let getUser = @route GET /api/users/:id {\n    let slug = @param \"slug\"\n    return @response 200 { ok: true }\n  }\n}\n",
    );

    let messages = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>();
    assert!(
        messages
            .iter()
            .any(|message| message.contains("@param `slug` is not declared")),
        "unexpected diagnostics: {messages:?}"
    );
}

#[test]
fn route_accessor_outside_route_is_rejected() {
    let (_, diagnostics) = analyze_source("function bad() -> @param \"id\"\n");

    let messages = diagnostics
        .iter()
        .map(|diagnostic| diagnostic.message.as_str())
        .collect::<Vec<_>>();
    assert!(
        messages
            .iter()
            .any(|message| message.contains("`@param` is only valid inside a route handler")),
        "unexpected diagnostics: {messages:?}"
    );
}
