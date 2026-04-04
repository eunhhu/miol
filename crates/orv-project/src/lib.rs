use std::collections::BTreeSet;

use orv_hir::{Expr, ItemKind, Module, NodeExpr, Pattern, ResolvedName, Stmt};
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ProjectGraph {
    pub module: String,
    pub imports: Vec<ImportEdge>,
    pub functions: Vec<String>,
    pub defines: Vec<DefineInfo>,
    pub structs: Vec<String>,
    pub enums: Vec<String>,
    pub type_aliases: Vec<String>,
    pub pages: Vec<PageInfo>,
    pub signals: Vec<SignalInfo>,
    pub routes: Vec<RouteInfo>,
    pub fetches: Vec<FetchEdge>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct WorkspaceGraph {
    pub entry: String,
    pub modules: Vec<ProjectGraph>,
    pub dependencies: Vec<ModuleDependency>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ImportEdge {
    pub path: String,
    pub names: Vec<String>,
    pub alias: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DefineInfo {
    pub name: String,
    pub return_domain: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PageInfo {
    pub owner: String,
    pub domain: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SignalInfo {
    pub owner: String,
    pub name: String,
    pub dependencies: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RouteInfo {
    pub method: String,
    pub path: String,
    pub action: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FetchEdge {
    pub owner: String,
    pub target: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ModuleDependency {
    pub from: String,
    pub to: String,
}

pub fn build_project_graph(module_name: impl Into<String>, module: &Module) -> ProjectGraph {
    let mut graph = ProjectGraph {
        module: module_name.into(),
        imports: Vec::new(),
        functions: Vec::new(),
        defines: Vec::new(),
        structs: Vec::new(),
        enums: Vec::new(),
        type_aliases: Vec::new(),
        pages: Vec::new(),
        signals: Vec::new(),
        routes: Vec::new(),
        fetches: Vec::new(),
    };

    let mut walker = GraphWalker { graph: &mut graph };
    walker.walk_module(module);
    graph
}

pub fn dump_project_graph(graph: &ProjectGraph) -> String {
    let mut out = String::new();
    out.push_str("Project Graph\n");
    out.push_str("=============\n");
    out.push_str(&format!("module: {}\n", graph.module));
    out.push_str(&format!("imports: {}\n", graph.imports.len()));
    for import in &graph.imports {
        out.push_str(&format!(
            "- import {}{}{}\n",
            import.path,
            if import.names.is_empty() {
                String::new()
            } else {
                format!(".{{{}}}", import.names.join(", "))
            },
            import
                .alias
                .as_ref()
                .map_or_else(String::new, |alias| format!(" as {alias}"))
        ));
    }

    out.push_str(&format!("functions: {}\n", graph.functions.len()));
    for function in &graph.functions {
        out.push_str(&format!("- function {function}\n"));
    }

    out.push_str(&format!("defines: {}\n", graph.defines.len()));
    for define in &graph.defines {
        let domain = define.return_domain.as_deref().unwrap_or("none");
        out.push_str(&format!("- define {} -> @{domain}\n", define.name));
    }

    out.push_str(&format!(
        "types: {} structs, {} enums, {} aliases\n",
        graph.structs.len(),
        graph.enums.len(),
        graph.type_aliases.len()
    ));
    for name in &graph.structs {
        out.push_str(&format!("- struct {name}\n"));
    }
    for name in &graph.enums {
        out.push_str(&format!("- enum {name}\n"));
    }
    for name in &graph.type_aliases {
        out.push_str(&format!("- type {name}\n"));
    }

    out.push_str(&format!("pages: {}\n", graph.pages.len()));
    for page in &graph.pages {
        out.push_str(&format!("- page {} ({})\n", page.owner, page.domain));
    }

    out.push_str(&format!("signals: {}\n", graph.signals.len()));
    for signal in &graph.signals {
        let deps = if signal.dependencies.is_empty() {
            "none".to_owned()
        } else {
            signal.dependencies.join(", ")
        };
        out.push_str(&format!(
            "- signal {}.{} deps: {}\n",
            signal.owner, signal.name, deps
        ));
    }

    out.push_str(&format!("routes: {}\n", graph.routes.len()));
    for route in &graph.routes {
        out.push_str(&format!(
            "- route {} {} -> {}\n",
            route.method, route.path, route.action
        ));
    }

    out.push_str(&format!("fetches: {}\n", graph.fetches.len()));
    for fetch in &graph.fetches {
        out.push_str(&format!("- fetch {} -> {}\n", fetch.owner, fetch.target));
    }

    out
}

pub fn dump_workspace_graph(graph: &WorkspaceGraph) -> String {
    let mut out = String::new();
    out.push_str("Project Graph\n");
    out.push_str("=============\n");
    out.push_str(&format!("entry: {}\n", graph.entry));
    out.push_str(&format!("modules: {}\n", graph.modules.len()));
    out.push_str(&format!("dependencies: {}\n", graph.dependencies.len()));

    let import_count: usize = graph
        .modules
        .iter()
        .map(|module| module.imports.len())
        .sum();
    let function_count: usize = graph
        .modules
        .iter()
        .map(|module| module.functions.len())
        .sum();
    let define_count: usize = graph
        .modules
        .iter()
        .map(|module| module.defines.len())
        .sum();
    let page_count: usize = graph.modules.iter().map(|module| module.pages.len()).sum();
    let signal_count: usize = graph
        .modules
        .iter()
        .map(|module| module.signals.len())
        .sum();
    let route_count: usize = graph.modules.iter().map(|module| module.routes.len()).sum();
    let fetch_count: usize = graph
        .modules
        .iter()
        .map(|module| module.fetches.len())
        .sum();
    out.push_str(&format!("imports: {import_count}\n"));
    out.push_str(&format!("functions: {function_count}\n"));
    out.push_str(&format!("defines: {define_count}\n"));
    out.push_str(&format!("pages: {page_count}\n"));
    out.push_str(&format!("signals: {signal_count}\n"));
    out.push_str(&format!("routes: {route_count}\n"));
    out.push_str(&format!("fetches: {fetch_count}\n"));

    for dependency in &graph.dependencies {
        out.push_str(&format!("- dep {} -> {}\n", dependency.from, dependency.to));
    }

    for module in &graph.modules {
        out.push_str(&format!("\n[module] {}\n", module.module));
        for page in &module.pages {
            out.push_str(&format!("- page {} ({})\n", page.owner, page.domain));
        }
        for signal in &module.signals {
            let deps = if signal.dependencies.is_empty() {
                "none".to_owned()
            } else {
                signal.dependencies.join(", ")
            };
            out.push_str(&format!(
                "- signal {}.{} deps: {}\n",
                signal.owner, signal.name, deps
            ));
        }
        for route in &module.routes {
            out.push_str(&format!(
                "- route {} {} -> {}\n",
                route.method, route.path, route.action
            ));
        }
        for fetch in &module.fetches {
            out.push_str(&format!("- fetch {} -> {}\n", fetch.owner, fetch.target));
        }
    }

    out
}

struct GraphWalker<'a> {
    graph: &'a mut ProjectGraph,
}

impl<'a> GraphWalker<'a> {
    fn walk_module(&mut self, module: &Module) {
        for item in &module.items {
            match &item.kind {
                ItemKind::Import(import) => self.graph.imports.push(ImportEdge {
                    path: import.path.join("."),
                    names: import.names.clone(),
                    alias: import.alias.clone(),
                }),
                ItemKind::Function(function) => {
                    self.graph.functions.push(function.name.clone());
                    self.walk_expr(&function.body, &function.name);
                }
                ItemKind::Define(define) => {
                    self.graph.defines.push(DefineInfo {
                        name: define.name.clone(),
                        return_domain: define.return_domain.clone(),
                    });
                    if define.return_domain.as_deref() == Some("html") {
                        self.graph.pages.push(PageInfo {
                            owner: define.name.clone(),
                            domain: "html".to_owned(),
                        });
                    }
                    self.walk_expr(&define.body, &define.name);
                }
                ItemKind::Struct(item) => {
                    self.graph.structs.push(item.name.clone());
                }
                ItemKind::Enum(item) => {
                    self.graph.enums.push(item.name.clone());
                }
                ItemKind::TypeAlias(item) => {
                    self.graph.type_aliases.push(item.name.clone());
                }
                ItemKind::Binding(binding) => {
                    self.walk_binding(binding, "<module>");
                }
                ItemKind::Stmt(stmt) => self.walk_stmt(stmt, "<module>"),
                ItemKind::Error => {}
            }
        }
    }

    fn walk_stmt(&mut self, stmt: &Stmt, owner: &str) {
        match stmt {
            Stmt::Binding(binding) => self.walk_binding(binding, owner),
            Stmt::Return(expr) => {
                if let Some(expr) = expr {
                    self.walk_expr(expr, owner);
                }
            }
            Stmt::If(if_stmt) => {
                self.walk_expr(&if_stmt.condition, owner);
                self.walk_expr(&if_stmt.then_body, owner);
                if let Some(else_body) = &if_stmt.else_body {
                    self.walk_expr(else_body, owner);
                }
            }
            Stmt::For(for_stmt) => {
                self.walk_expr(&for_stmt.iterable, owner);
                self.walk_expr(&for_stmt.body, owner);
            }
            Stmt::While(while_stmt) => {
                self.walk_expr(&while_stmt.condition, owner);
                self.walk_expr(&while_stmt.body, owner);
            }
            Stmt::Expr(expr) => self.walk_expr(expr, owner),
            Stmt::Error => {}
        }
    }

    fn walk_binding(&mut self, binding: &orv_hir::Binding, owner: &str) {
        if binding.is_sig {
            let mut dependencies = BTreeSet::new();
            if let Some(value) = &binding.value {
                collect_dependencies(value, &mut dependencies);
                self.walk_expr(value, owner);
            }
            self.graph.signals.push(SignalInfo {
                owner: owner.to_owned(),
                name: binding.name.clone(),
                dependencies: dependencies.into_iter().collect(),
            });
            return;
        }

        if let Some(value) = &binding.value {
            self.walk_expr(value, owner);
        }
    }

    fn walk_expr(&mut self, expr: &Expr, owner: &str) {
        match expr {
            Expr::Binary { left, right, .. } => {
                self.walk_expr(left, owner);
                self.walk_expr(right, owner);
            }
            Expr::Unary { operand, .. } => self.walk_expr(operand, owner),
            Expr::Assign { target, value, .. } => {
                self.walk_expr(target, owner);
                self.walk_expr(value, owner);
            }
            Expr::Call { callee, args } => {
                if let Expr::Field { object, field } = callee.as_ref()
                    && field == "fetch"
                    && let Expr::Ident(ResolvedName { name, .. }) = object.as_ref()
                {
                    self.graph.fetches.push(FetchEdge {
                        owner: owner.to_owned(),
                        target: name.clone(),
                    });
                }
                self.walk_expr(callee, owner);
                for arg in args {
                    self.walk_expr(&arg.value, owner);
                }
            }
            Expr::Field { object, .. } => self.walk_expr(object, owner),
            Expr::Index { object, index } => {
                self.walk_expr(object, owner);
                self.walk_expr(index, owner);
            }
            Expr::Block { stmts, .. } => {
                for stmt in stmts {
                    self.walk_stmt(stmt, owner);
                }
            }
            Expr::When { subject, arms } => {
                self.walk_expr(subject, owner);
                for arm in arms {
                    self.walk_expr(&arm.body, owner);
                }
            }
            Expr::Object(fields) | Expr::Map(fields) => {
                for field in fields {
                    self.walk_expr(&field.value, owner);
                }
            }
            Expr::Array(items) => {
                for item in items {
                    self.walk_expr(item, owner);
                }
            }
            Expr::Node(node) => {
                self.record_node(node, owner);
                for positional in &node.positional {
                    self.walk_expr(positional, owner);
                }
                for property in &node.properties {
                    self.walk_expr(&property.value, owner);
                }
                if let Some(body) = &node.body {
                    self.walk_expr(body, owner);
                }
            }
            Expr::Paren(inner) | Expr::Await(inner) => self.walk_expr(inner, owner),
            Expr::StringInterp(parts) => {
                for part in parts {
                    if let orv_hir::StringPart::Expr(expr) = part {
                        self.walk_expr(expr, owner);
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

    fn record_node(&mut self, node: &NodeExpr, owner: &str) {
        if node.name == "html" {
            self.graph.pages.push(PageInfo {
                owner: owner.to_owned(),
                domain: "html-inline".to_owned(),
            });
        }

        if node.name == "route"
            && let Some(route) = project_route(node)
        {
            self.graph.routes.push(route);
        }
    }
}

fn project_route(node: &NodeExpr) -> Option<RouteInfo> {
    let method = expr_atom(node.positional.first()?)?;
    let path = expr_atom(node.positional.get(1)?)?;
    Some(RouteInfo {
        method,
        path,
        action: route_action(node.body.as_deref()),
    })
}

fn route_action(body: Option<&Expr>) -> String {
    let Some(Expr::Block { stmts, .. }) = body else {
        return "unknown".to_owned();
    };

    for stmt in stmts {
        match stmt {
            Stmt::Return(Some(Expr::Node(node))) | Stmt::Expr(Expr::Node(node)) => {
                if node.name == "response" {
                    return "json-response".to_owned();
                }
                if node.name == "serve" {
                    return match node.positional.first() {
                        Some(Expr::Node(html)) if is_html_like(&html.name) => {
                            "html-serve".to_owned()
                        }
                        Some(_) => "static-serve".to_owned(),
                        None => "serve".to_owned(),
                    };
                }
            }
            _ => {}
        }
    }

    "unknown".to_owned()
}

fn is_html_like(name: &str) -> bool {
    matches!(name, "html" | "body" | "div" | "text")
}

fn expr_atom(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Ident(name) => Some(name.name.clone()),
        Expr::StringLiteral(value) => Some(value.clone()),
        _ => None,
    }
}

fn collect_dependencies(expr: &Expr, out: &mut BTreeSet<String>) {
    match expr {
        Expr::Ident(name) => {
            out.insert(name.name.clone());
        }
        Expr::Binary { left, right, .. } => {
            collect_dependencies(left, out);
            collect_dependencies(right, out);
        }
        Expr::Unary { operand, .. } => collect_dependencies(operand, out),
        Expr::Assign { target, value, .. } => {
            collect_dependencies(target, out);
            collect_dependencies(value, out);
        }
        Expr::Call { callee, args } => {
            collect_dependencies(callee, out);
            for arg in args {
                collect_dependencies(&arg.value, out);
            }
        }
        Expr::Field { object, .. } => collect_dependencies(object, out),
        Expr::Index { object, index } => {
            collect_dependencies(object, out);
            collect_dependencies(index, out);
        }
        Expr::Block { stmts, .. } => {
            for stmt in stmts {
                match stmt {
                    Stmt::Binding(binding) => {
                        if let Some(value) = &binding.value {
                            collect_dependencies(value, out);
                        }
                    }
                    Stmt::Return(Some(expr)) | Stmt::Expr(expr) => collect_dependencies(expr, out),
                    Stmt::If(if_stmt) => {
                        collect_dependencies(&if_stmt.condition, out);
                        collect_dependencies(&if_stmt.then_body, out);
                        if let Some(else_body) = &if_stmt.else_body {
                            collect_dependencies(else_body, out);
                        }
                    }
                    Stmt::For(for_stmt) => {
                        collect_dependencies(&for_stmt.iterable, out);
                        collect_dependencies(&for_stmt.body, out);
                    }
                    Stmt::While(while_stmt) => {
                        collect_dependencies(&while_stmt.condition, out);
                        collect_dependencies(&while_stmt.body, out);
                    }
                    Stmt::Return(None) | Stmt::Error => {}
                }
            }
        }
        Expr::When { subject, arms } => {
            collect_dependencies(subject, out);
            for arm in arms {
                let mut arm_dependencies = BTreeSet::new();
                collect_dependencies(&arm.body, &mut arm_dependencies);
                let mut bindings = BTreeSet::new();
                collect_pattern_bindings(&arm.pattern, &mut bindings);
                for binding in bindings {
                    arm_dependencies.remove(&binding);
                }
                out.extend(arm_dependencies);
            }
        }
        Expr::Object(fields) | Expr::Map(fields) => {
            for field in fields {
                collect_dependencies(&field.value, out);
            }
        }
        Expr::Array(items) => {
            for item in items {
                collect_dependencies(item, out);
            }
        }
        Expr::Node(node) => {
            for positional in &node.positional {
                collect_dependencies(positional, out);
            }
            for property in &node.properties {
                collect_dependencies(&property.value, out);
            }
            if let Some(body) = &node.body {
                collect_dependencies(body, out);
            }
        }
        Expr::Paren(inner) | Expr::Await(inner) => collect_dependencies(inner, out),
        Expr::StringInterp(parts) => {
            for part in parts {
                if let orv_hir::StringPart::Expr(expr) = part {
                    collect_dependencies(expr, out);
                }
            }
        }
        Expr::IntLiteral(_)
        | Expr::FloatLiteral(_)
        | Expr::StringLiteral(_)
        | Expr::BoolLiteral(_)
        | Expr::Void
        | Expr::Error => {}
    }
}

fn collect_pattern_bindings(pattern: &Pattern, out: &mut BTreeSet<String>) {
    match pattern {
        Pattern::Binding(name) => {
            out.insert(name.clone());
        }
        Pattern::Variant { fields, .. } => {
            for field in fields {
                collect_pattern_bindings(field, out);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dump_includes_routes_pages_and_signals() {
        let graph = ProjectGraph {
            module: "fixture.orv".to_owned(),
            imports: vec![ImportEdge {
                path: "ui".to_owned(),
                names: vec!["Button".to_owned()],
                alias: None,
            }],
            functions: vec!["helper".to_owned()],
            defines: vec![DefineInfo {
                name: "CounterPage".to_owned(),
                return_domain: Some("html".to_owned()),
            }],
            structs: vec!["User".to_owned()],
            enums: Vec::new(),
            type_aliases: Vec::new(),
            pages: vec![PageInfo {
                owner: "CounterPage".to_owned(),
                domain: "html".to_owned(),
            }],
            signals: vec![SignalInfo {
                owner: "CounterPage".to_owned(),
                name: "count".to_owned(),
                dependencies: Vec::new(),
            }],
            routes: vec![RouteInfo {
                method: "GET".to_owned(),
                path: "/".to_owned(),
                action: "static-serve".to_owned(),
            }],
            fetches: Vec::new(),
        };

        let dump = dump_project_graph(&graph);
        assert!(dump.contains("Project Graph"));
        assert!(dump.contains("page CounterPage (html)"));
        assert!(dump.contains("signal CounterPage.count deps: none"));
        assert!(dump.contains("route GET / -> static-serve"));
    }

    #[test]
    fn workspace_dump_includes_dependency_edges() {
        let graph = WorkspaceGraph {
            entry: "main.orv".to_owned(),
            modules: vec![ProjectGraph {
                module: "main.orv".to_owned(),
                imports: Vec::new(),
                functions: Vec::new(),
                defines: vec![DefineInfo {
                    name: "Home".to_owned(),
                    return_domain: Some("html".to_owned()),
                }],
                structs: Vec::new(),
                enums: Vec::new(),
                type_aliases: Vec::new(),
                pages: vec![PageInfo {
                    owner: "Home".to_owned(),
                    domain: "html".to_owned(),
                }],
                signals: Vec::new(),
                routes: Vec::new(),
                fetches: Vec::new(),
            }],
            dependencies: vec![ModuleDependency {
                from: "main.orv".to_owned(),
                to: "components/Button.orv".to_owned(),
            }],
        };

        let dump = dump_workspace_graph(&graph);
        assert!(dump.contains("entry: main.orv"));
        assert!(dump.contains("modules: 1"));
        assert!(dump.contains("dependencies: 1"));
        assert!(dump.contains("- dep main.orv -> components/Button.orv"));
    }
}
