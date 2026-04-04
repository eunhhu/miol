//! Two-pass name resolver.
//!
//! - **Pass 1** (`collect_items`): registers all top-level declarations into
//!   the module scope so that forward references work.
//! - **Pass 2** (`resolve_bodies`): walks every expression and statement,
//!   opening child scopes as needed and resolving `Ident` references.

use orv_diagnostics::{Diagnostic, DiagnosticBag, Label};
use orv_span::{Span, Spanned};
use orv_syntax::ast::{
    BindingStmt, DefineItem, Expr, ForStmt, FunctionItem, IfStmt, Item, Module, Pattern, Stmt,
    WhileStmt,
};

use crate::scope::{ScopeId, ScopeKind, ScopeMap};
use crate::symbol::{Symbol, SymbolId, SymbolKind, SymbolTable, Visibility};

/// The result of name resolution.
#[derive(Debug)]
pub struct ResolveResult {
    /// All symbols discovered during resolution.
    pub symbols: SymbolTable,
    /// The scope tree with all name bindings.
    pub scopes: ScopeMap,
    /// The ID of the module-level root scope.
    pub root_scope: ScopeId,
}

/// The resolver state.
pub(crate) struct Resolver {
    pub(crate) symbols: SymbolTable,
    pub(crate) scopes: ScopeMap,
    pub(crate) diagnostics: DiagnosticBag,
    /// The module-level root scope.
    pub(crate) root_scope: ScopeId,
    /// The currently active scope.
    pub(crate) current_scope: ScopeId,
}

impl Resolver {
    /// Creates a new resolver with an empty module scope.
    fn new() -> Self {
        let mut scopes = ScopeMap::new();
        let root = scopes.add(ScopeKind::Module, None);
        Self {
            symbols: SymbolTable::new(),
            scopes,
            diagnostics: DiagnosticBag::new(),
            root_scope: root,
            current_scope: root,
        }
    }

    /// Returns the root (module) scope ID.
    fn root_scope(&self) -> ScopeId {
        self.root_scope
    }

    // ── Helpers ────────────────────────────────────────────────────────

    /// Declares a symbol in the given scope. Emits a duplicate-symbol
    /// diagnostic if the name is already bound in that same scope.
    fn declare_in_scope(
        &mut self,
        scope: ScopeId,
        name: &str,
        name_span: Span,
        kind: SymbolKind,
        visibility: Visibility,
    ) -> SymbolId {
        let sym_id = self.symbols.add(Symbol {
            name: name.to_owned(),
            kind,
            visibility,
            def_span: name_span,
        });

        if let Some(prev_id) = self.scopes.insert(scope, name.to_owned(), sym_id) {
            let prev = self.symbols.get(prev_id);
            self.diagnostics.push(
                Diagnostic::error(format!("duplicate declaration of `{name}`"))
                    .with_label(Label::primary(name_span, "redefined here"))
                    .with_label(Label::secondary(prev.def_span, "previously defined here")),
            );
        }

        sym_id
    }

    /// Declares a symbol in the current scope.
    fn declare(
        &mut self,
        name: &str,
        name_span: Span,
        kind: SymbolKind,
        visibility: Visibility,
    ) -> SymbolId {
        let scope = self.current_scope;
        self.declare_in_scope(scope, name, name_span, kind, visibility)
    }

    /// Opens a new child scope of the given kind, sets it as current,
    /// and returns the new scope ID.
    fn push_scope(&mut self, kind: ScopeKind) -> ScopeId {
        let child = self.scopes.add(kind, Some(self.current_scope));
        self.current_scope = child;
        child
    }

    /// Restores the current scope to the parent of the given scope.
    fn pop_scope(&mut self, scope_id: ScopeId) {
        let parent = self
            .scopes
            .get(scope_id)
            .parent()
            .expect("cannot pop the root scope");
        self.current_scope = parent;
    }

    // ── Pass 1: Collect top-level items ────────────────────────────────

    /// Registers all top-level declarations in the module scope.
    /// This enables forward references: a function can call another
    /// function declared later in the file.
    #[expect(clippy::too_many_lines)]
    fn collect_items(&mut self, module: &Module) {
        let root = self.root_scope();
        for item in &module.items {
            match item.node() {
                Item::Function(func) => {
                    let vis = if func.is_pub {
                        Visibility::Public
                    } else {
                        Visibility::Private
                    };
                    self.declare_in_scope(
                        root,
                        func.name.node(),
                        func.name.span(),
                        SymbolKind::Function,
                        vis,
                    );
                }
                Item::Define(def) => {
                    let vis = if def.is_pub {
                        Visibility::Public
                    } else {
                        Visibility::Private
                    };
                    self.declare_in_scope(
                        root,
                        def.name.node(),
                        def.name.span(),
                        SymbolKind::Define,
                        vis,
                    );
                }
                Item::Struct(s) => {
                    let vis = if s.is_pub {
                        Visibility::Public
                    } else {
                        Visibility::Private
                    };
                    self.declare_in_scope(
                        root,
                        s.name.node(),
                        s.name.span(),
                        SymbolKind::Struct,
                        vis,
                    );
                }
                Item::Enum(e) => {
                    let vis = if e.is_pub {
                        Visibility::Public
                    } else {
                        Visibility::Private
                    };
                    self.declare_in_scope(
                        root,
                        e.name.node(),
                        e.name.span(),
                        SymbolKind::Enum,
                        vis,
                    );
                }
                Item::TypeAlias(t) => {
                    let vis = if t.is_pub {
                        Visibility::Public
                    } else {
                        Visibility::Private
                    };
                    self.declare_in_scope(
                        root,
                        t.name.node(),
                        t.name.span(),
                        SymbolKind::TypeAlias,
                        vis,
                    );
                }
                Item::Binding(b) => {
                    let vis = if b.is_pub {
                        Visibility::Public
                    } else {
                        Visibility::Private
                    };
                    self.declare_in_scope(
                        root,
                        b.name.node(),
                        b.name.span(),
                        SymbolKind::Variable,
                        vis,
                    );
                }
                Item::Import(imp) => {
                    self.collect_import(root, imp);
                }
                Item::Stmt(_) | Item::Error => {}
            }
        }
    }

    /// Registers imported names in the given scope.
    fn collect_import(&mut self, scope: ScopeId, imp: &orv_syntax::ast::ImportItem) {
        if imp.names.is_empty() {
            // Single import: `import components.Button` or `import components.Button as Btn`
            // The binding name is the alias if present, otherwise the last path segment.
            if let Some(alias) = &imp.alias {
                self.declare_in_scope(
                    scope,
                    alias.node(),
                    alias.span(),
                    SymbolKind::Import,
                    Visibility::Private,
                );
            } else if let Some(last) = imp.path.last() {
                self.declare_in_scope(
                    scope,
                    last.node(),
                    last.span(),
                    SymbolKind::Import,
                    Visibility::Private,
                );
            }
        } else {
            // Destructured import: `import components.{Button, Input}`
            for name in &imp.names {
                self.declare_in_scope(
                    scope,
                    name.node(),
                    name.span(),
                    SymbolKind::Import,
                    Visibility::Private,
                );
            }
        }
    }

    // ── Pass 2: Resolve references ─────────────────────────────────────

    /// Walks all items in the module, resolving identifiers in bodies.
    fn resolve_items(&mut self, module: &Module) {
        for item in &module.items {
            match item.node() {
                Item::Function(func) => self.resolve_function(func),
                Item::Define(def) => self.resolve_define(def),
                Item::Binding(b) => self.resolve_binding_stmt(b),
                Item::Stmt(stmt) => self.resolve_stmt(stmt),
                Item::Import(_)
                | Item::Struct(_)
                | Item::Enum(_)
                | Item::TypeAlias(_)
                | Item::Error => {
                    // Struct/enum/type bodies don't contain resolvable
                    // expressions in the current language.
                }
            }
        }
    }

    /// Resolves a function: opens a body scope, binds params, walks the body.
    fn resolve_function(&mut self, func: &FunctionItem) {
        let scope = self.push_scope(ScopeKind::Function);
        for param in &func.params {
            self.declare(
                param.node().name.node(),
                param.node().name.span(),
                SymbolKind::Parameter,
                Visibility::Private,
            );
            // Resolve default value if present.
            if let Some(default) = &param.node().default {
                self.resolve_expr(default);
            }
        }
        self.resolve_expr(&func.body);
        self.pop_scope(scope);
    }

    /// Resolves a define: opens a body scope, binds params, walks the body.
    fn resolve_define(&mut self, def: &DefineItem) {
        let scope = self.push_scope(ScopeKind::Define);
        for param in &def.params {
            self.declare(
                param.node().name.node(),
                param.node().name.span(),
                SymbolKind::Parameter,
                Visibility::Private,
            );
            if let Some(default) = &param.node().default {
                self.resolve_expr(default);
            }
        }
        self.resolve_expr(&def.body);
        self.pop_scope(scope);
    }

    /// Resolves a binding statement's initializer expression.
    fn resolve_binding_stmt(&mut self, binding: &BindingStmt) {
        // The name was already declared in Pass 1 (for top-level) or by
        // the block walker (for local bindings). Resolve the initializer.
        if let Some(value) = &binding.value {
            self.resolve_expr(value);
        }
    }

    // ── Statement resolution ───────────────────────────────────────────

    fn resolve_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Binding(b) => {
                // Local binding: declare in current scope, then resolve init.
                self.declare(
                    b.name.node(),
                    b.name.span(),
                    SymbolKind::Variable,
                    Visibility::Private,
                );
                if let Some(value) = &b.value {
                    self.resolve_expr(value);
                }
            }
            Stmt::Return(maybe_expr) => {
                if let Some(expr) = maybe_expr {
                    self.resolve_expr(expr);
                }
            }
            Stmt::If(if_stmt) => self.resolve_if(if_stmt),
            Stmt::For(for_stmt) => self.resolve_for(for_stmt),
            Stmt::While(while_stmt) => self.resolve_while(while_stmt),
            Stmt::Expr(expr) => self.resolve_expr(expr),
            Stmt::Error => {}
        }
    }

    fn resolve_if(&mut self, if_stmt: &IfStmt) {
        self.resolve_expr(&if_stmt.condition);

        let then_scope = self.push_scope(ScopeKind::IfBranch);
        self.resolve_expr(&if_stmt.then_body);
        self.pop_scope(then_scope);

        if let Some(else_body) = &if_stmt.else_body {
            let else_scope = self.push_scope(ScopeKind::IfBranch);
            self.resolve_expr(else_body);
            self.pop_scope(else_scope);
        }
    }

    fn resolve_for(&mut self, for_stmt: &ForStmt) {
        // The iterable is evaluated in the outer scope.
        self.resolve_expr(&for_stmt.iterable);

        let scope = self.push_scope(ScopeKind::ForLoop);
        self.declare(
            for_stmt.binding.node(),
            for_stmt.binding.span(),
            SymbolKind::LoopVariable,
            Visibility::Private,
        );
        self.resolve_expr(&for_stmt.body);
        self.pop_scope(scope);
    }

    fn resolve_while(&mut self, while_stmt: &WhileStmt) {
        self.resolve_expr(&while_stmt.condition);

        let scope = self.push_scope(ScopeKind::WhileLoop);
        self.resolve_expr(&while_stmt.body);
        self.pop_scope(scope);
    }

    // ── Expression resolution ──────────────────────────────────────────

    #[expect(clippy::too_many_lines)]
    fn resolve_expr(&mut self, expr: &Spanned<Expr>) {
        match expr.node() {
            Expr::Ident(name) => {
                if self.scopes.lookup(self.current_scope, name).is_none() {
                    self.diagnostics.push(
                        Diagnostic::error(format!("unresolved name `{name}`"))
                            .with_label(Label::primary(expr.span(), "not found in this scope")),
                    );
                }
            }
            Expr::Binary { left, right, .. } => {
                self.resolve_expr(left);
                self.resolve_expr(right);
            }
            Expr::Unary { operand, .. } => {
                self.resolve_expr(operand);
            }
            Expr::Assign { target, value, .. } => {
                self.resolve_expr(target);
                self.resolve_expr(value);
            }
            Expr::Call { callee, args } => {
                self.resolve_expr(callee);
                for arg in args {
                    self.resolve_expr(&arg.node().value);
                }
            }
            Expr::Field { object, .. } => {
                self.resolve_expr(object);
            }
            Expr::Index { object, index } => {
                self.resolve_expr(object);
                self.resolve_expr(index);
            }
            Expr::Block(stmts) => {
                let scope = self.push_scope(ScopeKind::Block);
                for stmt in stmts {
                    self.resolve_stmt(stmt.node());
                }
                self.pop_scope(scope);
            }
            Expr::When { subject, arms } => {
                self.resolve_expr(subject);
                for arm in arms {
                    let scope = self.push_scope(ScopeKind::WhenArm);
                    self.declare_pattern_bindings(&arm.node().pattern);
                    self.resolve_expr(&arm.node().body);
                    self.pop_scope(scope);
                }
            }
            Expr::Object(fields) => {
                for field in fields {
                    self.resolve_expr(&field.node().value);
                }
            }
            Expr::Map(fields) => {
                for field in fields {
                    self.resolve_expr(&field.node().value);
                }
            }
            Expr::Array(elems) => {
                for elem in elems {
                    self.resolve_expr(elem);
                }
            }
            Expr::Node(node_expr) => {
                let node_name = node_expr.name.node().to_string();
                for (index, pos) in node_expr.positional.iter().enumerate() {
                    if should_resolve_node_positional(&node_name, index, pos.node()) {
                        self.resolve_expr(pos);
                    }
                }
                for prop in &node_expr.properties {
                    self.resolve_expr(&prop.node().value);
                }
                if let Some(body) = &node_expr.body {
                    self.resolve_expr(body);
                }
            }
            Expr::Paren(inner) => {
                self.resolve_expr(inner);
            }
            Expr::Await(inner) => {
                self.resolve_expr(inner);
            }
            Expr::StringInterp(parts) => {
                for part in parts {
                    if let orv_syntax::ast::StringPart::Expr(e) = part {
                        self.resolve_expr(e);
                    }
                }
            }
            // Literals and error have nothing to resolve.
            Expr::IntLiteral(_)
            | Expr::FloatLiteral(_)
            | Expr::StringLiteral(_)
            | Expr::BoolLiteral(_)
            | Expr::Void
            | Expr::Error => {}
        }
    }

    fn declare_pattern_bindings(&mut self, pattern: &Spanned<Pattern>) {
        match pattern.node() {
            Pattern::Binding(name) => {
                self.declare(
                    name,
                    pattern.span(),
                    SymbolKind::Variable,
                    Visibility::Private,
                );
            }
            Pattern::Variant { fields, .. } => {
                for field in fields {
                    self.declare_pattern_bindings(field);
                }
            }
            Pattern::Wildcard
            | Pattern::IntLiteral(_)
            | Pattern::FloatLiteral(_)
            | Pattern::StringLiteral(_)
            | Pattern::BoolLiteral(_)
            | Pattern::Void
            | Pattern::Error => {}
        }
    }
}

fn should_resolve_node_positional(node_name: &str, index: usize, expr: &Expr) -> bool {
    match (node_name, index, expr) {
        ("route", 0, Expr::Ident(method))
            if matches!(
                method.as_str(),
                "*" | "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS"
            ) =>
        {
            false
        }
        ("route", 1, Expr::Ident(path)) if is_path_like_atom(path) || path == "*" => false,
        ("serve", _, Expr::Ident(path)) if is_path_like_atom(path) => false,
        ("env", 0, Expr::Ident(_)) | ("env", 0, Expr::StringLiteral(_)) => false,
        _ => true,
    }
}

fn is_path_like_atom(value: &str) -> bool {
    value.starts_with('/') || value.starts_with("./") || value.starts_with("../")
}

/// Resolves all names in a parsed module.
///
/// Returns the resolution result and any diagnostics emitted.
pub fn resolve(module: &Module) -> (ResolveResult, DiagnosticBag) {
    let mut resolver = Resolver::new();

    // Pass 1: collect top-level declarations.
    resolver.collect_items(module);

    // Pass 2: resolve references in all bodies.
    resolver.resolve_items(module);

    let root = resolver.root_scope();
    let result = ResolveResult {
        symbols: resolver.symbols,
        scopes: resolver.scopes,
        root_scope: root,
    };
    (result, resolver.diagnostics)
}

#[cfg(test)]
mod tests {
    use orv_span::FileId;
    use orv_syntax::lexer::Lexer;
    use orv_syntax::parser::parse;
    use pretty_assertions::assert_eq;

    use super::*;

    /// Helper: lex + parse + resolve, returning the result and diagnostics.
    fn resolve_source(src: &str) -> (ResolveResult, DiagnosticBag) {
        let file = FileId::new(0);
        let lexer = Lexer::new(src, file);
        let (tokens, lex_diags) = lexer.tokenize();
        assert!(!lex_diags.has_errors(), "lexer errors: {lex_diags:?}");
        let (module, parse_diags) = parse(tokens);
        assert!(!parse_diags.has_errors(), "parse errors: {parse_diags:?}");
        resolve(&module)
    }

    // ── Task 3: Pass 1 tests ──────────────────────────────────────────

    #[test]
    fn collect_function() {
        let (result, diags) = resolve_source("function greet() -> void");
        assert!(!diags.has_errors());
        assert_eq!(result.symbols.len(), 1);

        let sym = result.symbols.get(SymbolId::new(0));
        assert_eq!(sym.name, "greet");
        assert_eq!(sym.kind, SymbolKind::Function);
        assert_eq!(sym.visibility, Visibility::Private);
    }

    #[test]
    fn collect_pub_function() {
        let (result, diags) = resolve_source("pub function hello() -> void");
        assert!(!diags.has_errors());

        let sym = result.symbols.get(SymbolId::new(0));
        assert_eq!(sym.name, "hello");
        assert_eq!(sym.visibility, Visibility::Public);
    }

    #[test]
    fn collect_define() {
        let (result, diags) = resolve_source("pub define Button() -> @html { void }");
        assert!(!diags.has_errors());
        assert_eq!(result.symbols.len(), 1);

        let sym = result.symbols.get(SymbolId::new(0));
        assert_eq!(sym.name, "Button");
        assert_eq!(sym.kind, SymbolKind::Define);
        assert_eq!(sym.visibility, Visibility::Public);
    }

    #[test]
    fn collect_struct() {
        let (result, diags) = resolve_source("struct Point {\n  x: i32\n  y: i32\n}");
        assert!(!diags.has_errors());

        let sym = result.symbols.get(SymbolId::new(0));
        assert_eq!(sym.name, "Point");
        assert_eq!(sym.kind, SymbolKind::Struct);
    }

    #[test]
    fn collect_enum() {
        let (result, diags) = resolve_source("enum Color {\n  Red\n  Green\n  Blue\n}");
        assert!(!diags.has_errors());

        let sym = result.symbols.get(SymbolId::new(0));
        assert_eq!(sym.name, "Color");
        assert_eq!(sym.kind, SymbolKind::Enum);
    }

    #[test]
    fn collect_type_alias() {
        let (result, diags) = resolve_source("type Name = string");
        assert!(!diags.has_errors());

        let sym = result.symbols.get(SymbolId::new(0));
        assert_eq!(sym.name, "Name");
        assert_eq!(sym.kind, SymbolKind::TypeAlias);
    }

    #[test]
    fn collect_let_binding() {
        let (result, diags) = resolve_source("let count = 42");
        assert!(!diags.has_errors());

        let sym = result.symbols.get(SymbolId::new(0));
        assert_eq!(sym.name, "count");
        assert_eq!(sym.kind, SymbolKind::Variable);
    }

    #[test]
    fn collect_const_binding() {
        let (result, diags) = resolve_source("const MAX = 100");
        assert!(!diags.has_errors());

        let sym = result.symbols.get(SymbolId::new(0));
        assert_eq!(sym.name, "MAX");
        assert_eq!(sym.kind, SymbolKind::Variable);
    }

    #[test]
    fn collect_import_single() {
        let (result, diags) = resolve_source("import components.Button");
        assert!(!diags.has_errors());
        assert_eq!(result.symbols.len(), 1);

        let sym = result.symbols.get(SymbolId::new(0));
        assert_eq!(sym.name, "Button");
        assert_eq!(sym.kind, SymbolKind::Import);
    }

    #[test]
    fn collect_import_aliased() {
        let (result, diags) = resolve_source("import components.Button as Btn");
        assert!(!diags.has_errors());
        assert_eq!(result.symbols.len(), 1);

        let sym = result.symbols.get(SymbolId::new(0));
        assert_eq!(sym.name, "Btn");
        assert_eq!(sym.kind, SymbolKind::Import);
    }

    #[test]
    fn collect_import_destructured() {
        let (result, diags) = resolve_source("import components.{Button, Input}");
        assert!(!diags.has_errors());
        assert_eq!(result.symbols.len(), 2);

        let names: Vec<&str> = result
            .symbols
            .iter()
            .map(|(_, s)| s.name.as_str())
            .collect();
        assert_eq!(names, vec!["Button", "Input"]);
    }

    #[test]
    fn collect_multiple_items() {
        let src = "\
function greet() -> void
pub define App() -> @html { void }
let x = 1
struct Point {
    x: i32
    y: i32
}
";
        let (result, diags) = resolve_source(src);
        assert!(!diags.has_errors());
        assert_eq!(result.symbols.len(), 4);

        let names: Vec<&str> = result
            .symbols
            .iter()
            .map(|(_, s)| s.name.as_str())
            .collect();
        assert_eq!(names, vec!["greet", "App", "x", "Point"]);
    }

    #[test]
    fn duplicate_top_level_detected() {
        let src = "\
function foo() -> void
function foo() -> void
";
        let (_, diags) = resolve_source(src);
        assert!(diags.has_errors());
        assert_eq!(diags.len(), 1);

        let d = diags.iter().next().unwrap();
        assert!(d.message.contains("duplicate declaration of `foo`"));
    }

    // ── Task 4: Pass 2 tests ──────────────────────────────────────────

    #[test]
    fn resolve_ident_in_function_body() {
        let src = "\
let x = 1
function foo() -> x
";
        let (_, diags) = resolve_source(src);
        assert!(!diags.has_errors());
    }

    #[test]
    fn unresolved_ident_reported() {
        let src = "function foo() -> bar";
        let (_, diags) = resolve_source(src);
        assert!(diags.has_errors());
        assert_eq!(diags.len(), 1);

        let d = diags.iter().next().unwrap();
        assert!(d.message.contains("unresolved name `bar`"));
    }

    #[test]
    fn function_param_resolves_in_body() {
        let src = "function greet(name: string) -> name";
        let (result, diags) = resolve_source(src);
        assert!(!diags.has_errors());

        // greet + name (param) = 2 symbols
        assert_eq!(result.symbols.len(), 2);
    }

    #[test]
    fn define_param_resolves_in_body() {
        let src = "define Card(title: string) -> @html { title }";
        let (_, diags) = resolve_source(src);
        assert!(!diags.has_errors());
    }

    #[test]
    fn forward_reference_works() {
        // `foo` calls `bar` which is defined later — Pass 1 collected both first.
        let src = "\
function foo() -> bar()
function bar() -> 42
";
        let (_, diags) = resolve_source(src);
        assert!(!diags.has_errors());
    }

    #[test]
    fn import_name_resolves() {
        let src = "\
import components.Button
function render() -> Button()
";
        let (_, diags) = resolve_source(src);
        assert!(!diags.has_errors());
    }

    #[test]
    fn binary_expr_resolves_both_sides() {
        let src = "\
let a = 1
let b = 2
let c = a + b
";
        let (_, diags) = resolve_source(src);
        assert!(!diags.has_errors());
    }

    #[test]
    fn call_expr_resolves_callee_and_args() {
        let src = "\
let x = 10
function foo(n: i32) -> n
let result = foo(x)
";
        let (_, diags) = resolve_source(src);
        assert!(!diags.has_errors());
    }

    #[test]
    fn node_positional_resolves() {
        let src = "\
let msg = \"hello\"
@io.out msg
";
        let (_, diags) = resolve_source(src);
        assert!(!diags.has_errors());
    }

    // ── Task 5: Param scoping edge cases ──────────────────────────────

    #[test]
    fn multiple_params_all_resolve() {
        let src = "function add(a: i32, b: i32) -> a + b";
        let (result, diags) = resolve_source(src);
        assert!(!diags.has_errors());
        // add + a + b = 3 symbols
        assert_eq!(result.symbols.len(), 3);
    }

    #[test]
    fn param_shadows_outer() {
        let src = "\
let x = 10
function foo(x: i32) -> x
";
        let (_, diags) = resolve_source(src);
        // No error — `x` in body resolves to the parameter, not the outer `let x`.
        assert!(!diags.has_errors());
    }

    #[test]
    fn param_default_resolves_outer() {
        let src = "\
let default_size = 16
function make(size: i32 = default_size) -> size
";
        let (_, diags) = resolve_source(src);
        // Default value `default_size` resolves to the top-level let.
        assert!(!diags.has_errors());
    }

    #[test]
    fn define_multiple_params() {
        let src = "define Layout(width: i32, height: i32) -> @html { width + height }";
        let (result, diags) = resolve_source(src);
        assert!(!diags.has_errors());
        // Layout + width + height = 3 symbols
        assert_eq!(result.symbols.len(), 3);
    }

    #[test]
    fn unresolved_in_default_value() {
        let src = "function foo(x: i32 = unknown) -> x";
        let (_, diags) = resolve_source(src);
        assert!(diags.has_errors());
        let d = diags.iter().next().unwrap();
        assert!(d.message.contains("unresolved name `unknown`"));
    }

    // ── Task 6: Block scoping tests ───────────────────────────────────

    #[test]
    fn block_scope_isolates_locals() {
        let src = "\
function foo() -> {
    let inner = 1
    inner
}
";
        let (_, diags) = resolve_source(src);
        assert!(!diags.has_errors());
    }

    #[test]
    fn for_loop_binding_scoped() {
        let src = "\
let items = 0
function foo() -> {
    for item of items {
        item
    }
}
";
        let (_, diags) = resolve_source(src);
        assert!(!diags.has_errors());
    }

    #[test]
    fn for_loop_var_not_visible_outside() {
        // `item` is declared in the for-loop scope and should not
        // leak out. In our implementation the for body is a child scope,
        // so `item` used after the loop is unresolved.
        let src = "\
function foo() -> {
    for item of [1, 2, 3] {
        item
    }
    item
}
";
        let (_, diags) = resolve_source(src);
        assert!(diags.has_errors());
        let d = diags.iter().next().unwrap();
        assert!(d.message.contains("unresolved name `item`"));
    }

    #[test]
    fn if_branch_scope_isolated() {
        let src = "\
function foo() -> {
    if true {
        let inside = 1
    }
    inside
}
";
        let (_, diags) = resolve_source(src);
        assert!(diags.has_errors());
        let d = diags.iter().next().unwrap();
        assert!(d.message.contains("unresolved name `inside`"));
    }

    #[test]
    fn while_body_scope_isolated() {
        let src = "\
function foo() -> {
    while true {
        let w = 1
    }
    w
}
";
        let (_, diags) = resolve_source(src);
        assert!(diags.has_errors());
        let d = diags.iter().next().unwrap();
        assert!(d.message.contains("unresolved name `w`"));
    }

    #[test]
    fn local_shadows_in_block() {
        let src = "\
let x = 1
function foo() -> {
    let x = 2
    x
}
";
        let (_, diags) = resolve_source(src);
        // No error — inner `x` shadows outer `x`.
        assert!(!diags.has_errors());
    }

    #[test]
    fn nested_block_scoping() {
        let src = "\
function foo() -> {
    let a = 1
    {
        let b = 2
        a + b
    }
}
";
        let (_, diags) = resolve_source(src);
        assert!(!diags.has_errors());
    }

    #[test]
    fn if_else_both_scoped() {
        let src = "\
let cond = true
function foo() -> {
    if cond {
        let t = 1
        t
    } else {
        let f = 2
        f
    }
}
";
        let (_, diags) = resolve_source(src);
        assert!(!diags.has_errors());
    }

    // ── Task 7: Duplicate symbol detection tests ──────────────────────

    #[test]
    fn duplicate_let_binding() {
        let src = "\
let x = 1
let x = 2
";
        let (_, diags) = resolve_source(src);
        assert!(diags.has_errors());
        assert_eq!(diags.len(), 1);
        let d = diags.iter().next().unwrap();
        assert!(d.message.contains("duplicate declaration of `x`"));
        // Should have both primary and secondary labels.
        assert_eq!(d.labels.len(), 2);
        assert!(d.labels[0].is_primary);
        assert!(!d.labels[1].is_primary);
    }

    #[test]
    fn duplicate_struct() {
        let src = "\
struct Foo {
    a: i32
}
struct Foo {
    b: i32
}
";
        let (_, diags) = resolve_source(src);
        assert!(diags.has_errors());
        let d = diags.iter().next().unwrap();
        assert!(d.message.contains("duplicate declaration of `Foo`"));
    }

    #[test]
    fn duplicate_function_and_let() {
        // A `let` and a `function` with the same name are also a conflict.
        let src = "\
function foo() -> void
let foo = 42
";
        let (_, diags) = resolve_source(src);
        assert!(diags.has_errors());
        let d = diags.iter().next().unwrap();
        assert!(d.message.contains("duplicate declaration of `foo`"));
    }

    #[test]
    fn duplicate_import_names() {
        let src = "\
import a.{Foo}
import b.{Foo}
";
        let (_, diags) = resolve_source(src);
        assert!(diags.has_errors());
        let d = diags.iter().next().unwrap();
        assert!(d.message.contains("duplicate declaration of `Foo`"));
    }

    #[test]
    fn import_alias_collision() {
        let src = "\
import a.Foo
import b.Bar as Foo
";
        let (_, diags) = resolve_source(src);
        assert!(diags.has_errors());
        let d = diags.iter().next().unwrap();
        assert!(d.message.contains("duplicate declaration of `Foo`"));
    }

    #[test]
    fn no_duplicate_in_different_scopes() {
        // Same name in different scopes is fine (shadowing, not duplication).
        let src = "\
let x = 1
function foo() -> {
    let x = 2
    x
}
";
        let (_, diags) = resolve_source(src);
        assert!(!diags.has_errors());
    }

    #[test]
    fn duplicate_in_same_block() {
        let src = "\
function foo() -> {
    let a = 1
    let a = 2
    a
}
";
        let (_, diags) = resolve_source(src);
        assert!(diags.has_errors());
        let d = diags.iter().next().unwrap();
        assert!(d.message.contains("duplicate declaration of `a`"));
    }
}
