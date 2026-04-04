use orv_diagnostics::{Diagnostic, DiagnosticBag, Label};
use orv_span::Spanned;
use orv_syntax::ast::{BindingStmt, Expr, Item, Module, NodeExpr, Stmt};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DomainContext {
    Neutral,
    Server,
    Route,
    Html,
}

pub fn validate(module: &Module) -> DiagnosticBag {
    let mut validator = Validator {
        diagnostics: DiagnosticBag::new(),
    };
    validator.validate_module(module);
    validator.diagnostics
}

struct Validator {
    diagnostics: DiagnosticBag,
}

impl Validator {
    fn validate_module(&mut self, module: &Module) {
        for item in &module.items {
            self.validate_item(item.node(), DomainContext::Neutral);
        }
    }

    fn validate_item(&mut self, item: &Item, domain: DomainContext) {
        match item {
            Item::Function(function) => self.validate_expr(&function.body, domain),
            Item::Define(define) => {
                let body_domain = match &define.return_domain {
                    Some(return_domain) if return_domain.node().to_string() == "html" => {
                        DomainContext::Html
                    }
                    Some(return_domain) if return_domain.node().to_string() == "server" => {
                        DomainContext::Server
                    }
                    _ => domain,
                };
                self.validate_expr(&define.body, body_domain);
            }
            Item::Binding(binding) => self.validate_binding(binding, domain),
            Item::Stmt(stmt) => self.validate_stmt(stmt, domain),
            Item::Import(_)
            | Item::Struct(_)
            | Item::Enum(_)
            | Item::TypeAlias(_)
            | Item::Error => {}
        }
    }

    fn validate_binding(&mut self, binding: &BindingStmt, domain: DomainContext) {
        if let Some(value) = &binding.value {
            self.validate_expr(value, domain);
        }
    }

    fn validate_stmt(&mut self, stmt: &Stmt, domain: DomainContext) {
        match stmt {
            Stmt::Binding(binding) => self.validate_binding(binding, domain),
            Stmt::Return(expr) => {
                if let Some(expr) = expr {
                    self.validate_expr(expr, domain);
                }
            }
            Stmt::If(if_stmt) => {
                self.validate_expr(&if_stmt.condition, domain);
                self.validate_expr(&if_stmt.then_body, domain);
                if let Some(else_body) = &if_stmt.else_body {
                    self.validate_expr(else_body, domain);
                }
            }
            Stmt::For(for_stmt) => {
                self.validate_expr(&for_stmt.iterable, domain);
                self.validate_expr(&for_stmt.body, domain);
            }
            Stmt::While(while_stmt) => {
                self.validate_expr(&while_stmt.condition, domain);
                self.validate_expr(&while_stmt.body, domain);
            }
            Stmt::Expr(expr) => self.validate_expr(expr, domain),
            Stmt::Error => {}
        }
    }

    fn validate_expr(&mut self, expr: &Spanned<Expr>, domain: DomainContext) {
        match expr.node() {
            Expr::Binary { left, right, .. } => {
                self.validate_expr(left, domain);
                self.validate_expr(right, domain);
            }
            Expr::Unary { operand, .. } => self.validate_expr(operand, domain),
            Expr::Assign { target, value, .. } => {
                self.validate_expr(target, domain);
                self.validate_expr(value, domain);
            }
            Expr::Call { callee, args } => {
                self.validate_expr(callee, domain);
                for arg in args {
                    self.validate_expr(&arg.node().value, domain);
                }
            }
            Expr::Field { object, .. } => self.validate_expr(object, domain),
            Expr::Index { object, index } => {
                self.validate_expr(object, domain);
                self.validate_expr(index, domain);
            }
            Expr::Block(stmts) => {
                for stmt in stmts {
                    self.validate_stmt(stmt.node(), domain);
                }
            }
            Expr::When { subject, arms } => {
                self.validate_expr(subject, domain);
                for arm in arms {
                    self.validate_expr(&arm.node().body, domain);
                }
            }
            Expr::Object(fields) => {
                for field in fields {
                    self.validate_expr(&field.node().value, domain);
                }
            }
            Expr::Map(fields) => {
                for field in fields {
                    self.validate_expr(&field.node().value, domain);
                }
            }
            Expr::Array(items) => {
                for item in items {
                    self.validate_expr(item, domain);
                }
            }
            Expr::Node(node) => self.validate_node(node, domain),
            Expr::Paren(inner) => self.validate_expr(inner, domain),
            Expr::Await(inner) => self.validate_expr(inner, domain),
            Expr::StringInterp(parts) => {
                for part in parts {
                    if let orv_syntax::ast::StringPart::Expr(expr) = part {
                        self.validate_expr(expr, domain);
                    }
                }
            }
            Expr::IntLiteral(_)
            | Expr::FloatLiteral(_)
            | Expr::StringLiteral(_)
            | Expr::BoolLiteral(_)
            | Expr::Void
            | Expr::Ident(_)
            | Expr::Error => {}
        }
    }

    fn validate_node(&mut self, node: &NodeExpr, domain: DomainContext) {
        let name = node.name.node().to_string();

        self.validate_node_context(node, domain, &name);
        self.validate_node_signature(node, &name);

        for positional in &node.positional {
            self.validate_expr(positional, domain);
        }
        for property in &node.properties {
            self.validate_expr(&property.node().value, domain);
        }

        let child_domain = match name.as_str() {
            "server" => DomainContext::Server,
            "route" => DomainContext::Route,
            "html" | "body" => DomainContext::Html,
            _ => domain,
        };

        if let Some(body) = &node.body {
            self.validate_expr(body, child_domain);
        }
    }

    fn validate_node_context(&mut self, node: &NodeExpr, domain: DomainContext, name: &str) {
        let context_name = match domain {
            DomainContext::Neutral => "@root",
            DomainContext::Server => "@server",
            DomainContext::Route => "@route",
            DomainContext::Html => "@html",
        };

        let invalid = match name {
            "server" => !matches!(domain, DomainContext::Neutral),
            "listen" => !matches!(domain, DomainContext::Server),
            "route" => !matches!(domain, DomainContext::Server),
            "serve" | "response" => !matches!(domain, DomainContext::Route),
            "body" | "div" | "text" | "input" | "button" | "vstack" | "hstack" => {
                !matches!(domain, DomainContext::Html)
            }
            _ => false,
        };

        if invalid {
            self.diagnostics.push(
                Diagnostic::error(format!(
                    "node `@{name}` is not valid in {context_name} context"
                ))
                .with_label(Label::primary(
                    node.name.span(),
                    format!("`@{name}` cannot appear here"),
                )),
            );
        }
    }

    fn validate_node_signature(&mut self, node: &NodeExpr, name: &str) {
        match name {
            "listen" => {
                self.expect_arity(node, 1, "@listen");
                if let Some(value) = node.positional.first() {
                    let valid = matches!(
                        value.node(),
                        Expr::IntLiteral(_) | Expr::Ident(_) | Expr::Node(_)
                    );
                    if !valid {
                        self.diagnostics.push(
                            Diagnostic::error("@listen expects an integer-like port value")
                                .with_label(Label::primary(
                                    value.span(),
                                    "invalid listen port expression",
                                )),
                        );
                    }
                }
            }
            "route" => {
                self.expect_arity(node, 2, "@route");
                if let Some(method) = node.positional.first() {
                    match method.node() {
                        Expr::Ident(value)
                            if matches!(
                                value.as_str(),
                                "*" | "GET"
                                    | "POST"
                                    | "PUT"
                                    | "PATCH"
                                    | "DELETE"
                                    | "HEAD"
                                    | "OPTIONS"
                            ) => {}
                        _ => self.diagnostics.push(
                            Diagnostic::error("@route method must be a bare HTTP verb")
                                .with_label(Label::primary(method.span(), "invalid route method")),
                        ),
                    }
                }
                if let Some(path) = node.positional.get(1) {
                    match path.node() {
                        Expr::Ident(value) if value == "*" || value.starts_with('/') => {}
                        _ => self.diagnostics.push(
                            Diagnostic::error(
                                "@route path must be a bare path literal like `/users`",
                            )
                            .with_label(Label::primary(path.span(), "invalid route path")),
                        ),
                    }
                }
            }
            "response" => {
                self.expect_arity(node, 1, "@response");
                if let Some(status) = node.positional.first()
                    && !matches!(status.node(), Expr::IntLiteral(_))
                {
                    self.diagnostics.push(
                        Diagnostic::error("@response status must be an integer literal")
                            .with_label(Label::primary(status.span(), "invalid response status")),
                    );
                }
            }
            "serve" => {
                self.expect_arity(node, 1, "@serve");
            }
            "env" => {
                self.expect_arity(node, 1, "@env");
                if let Some(name_expr) = node.positional.first()
                    && !matches!(name_expr.node(), Expr::Ident(_) | Expr::StringLiteral(_))
                {
                    self.diagnostics.push(
                        Diagnostic::error("@env expects an environment variable name").with_label(
                            Label::primary(name_expr.span(), "invalid env variable name"),
                        ),
                    );
                }
            }
            _ => {}
        }
    }

    fn expect_arity(&mut self, node: &NodeExpr, expected: usize, label: &str) {
        if node.positional.len() != expected {
            self.diagnostics.push(
                Diagnostic::error(format!(
                    "{label} expects {expected} positional argument(s), got {}",
                    node.positional.len()
                ))
                .with_label(Label::primary(
                    node.name.span(),
                    "wrong number of positional arguments",
                )),
            );
        }
    }
}
