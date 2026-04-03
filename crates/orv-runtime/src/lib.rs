use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use orv_hir::{Expr, ItemKind, Module, NodeExpr, Stmt};
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Program {
    pub adapter: AdapterKind,
    pub server: ServerProgram,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AdapterKind {
    DirectMatch,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ServerProgram {
    pub listen: u16,
    pub routes: Vec<Route>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Route {
    pub method: String,
    pub path: String,
    pub action: RouteAction,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum RouteAction {
    JsonResponse { status: u16, body: String },
    StaticServe { target: String },
    HtmlServe { html: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request<'a> {
    pub method: &'a str,
    pub path: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteResponse {
    pub adapter: AdapterKind,
    pub status: u16,
    pub content_type: &'static str,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildArtifacts {
    pub manifest_path: PathBuf,
    pub adapter_source_path: PathBuf,
    pub binary_path: PathBuf,
}

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("{0}")]
    Message(String),
    #[error("serialization failed: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("i/o failed: {0}")]
    Io(#[from] std::io::Error),
}

pub fn compile_program(module: &Module) -> Result<Program, RuntimeError> {
    let server_node = find_server_node(module)
        .ok_or_else(|| RuntimeError::Message("no top-level @server entry was found".to_owned()))?;

    Ok(Program {
        adapter: AdapterKind::DirectMatch,
        server: compile_server(server_node)?,
    })
}

pub fn execute_request(
    program: &Program,
    request: &Request<'_>,
) -> Result<RouteResponse, RuntimeError> {
    for route in &program.server.routes {
        if route.method == request.method && route.path == request.path {
            return Ok(match &route.action {
                RouteAction::JsonResponse { status, body } => RouteResponse {
                    adapter: program.adapter,
                    status: *status,
                    content_type: "application/json",
                    body: body.clone(),
                },
                RouteAction::StaticServe { target } => RouteResponse {
                    adapter: program.adapter,
                    status: 200,
                    content_type: "text/plain; charset=utf-8",
                    body: format!("serve {target}"),
                },
                RouteAction::HtmlServe { html } => RouteResponse {
                    adapter: program.adapter,
                    status: 200,
                    content_type: "text/html; charset=utf-8",
                    body: html.clone(),
                },
            });
        }
    }

    Err(RuntimeError::Message(format!(
        "no route matched {} {}",
        request.method, request.path
    )))
}

pub fn render_response(response: &RouteResponse) -> String {
    format!(
        "adapter: {}\nstatus: {}\ncontent-type: {}\nbody: {}\n",
        match response.adapter {
            AdapterKind::DirectMatch => "direct-match",
        },
        response.status,
        response.content_type,
        response.body,
    )
}

pub fn emit_build(program: &Program, output_dir: &Path) -> Result<BuildArtifacts, RuntimeError> {
    fs::create_dir_all(output_dir)?;

    let manifest_path = output_dir.join("program.json");
    fs::write(&manifest_path, serde_json::to_vec_pretty(program)?)?;

    let adapter_source_path = output_dir.join("direct_adapter.rs");
    let adapter_source = generate_direct_adapter_source(program);
    fs::write(&adapter_source_path, adapter_source)?;

    let binary_name = format!("orv-app{}", std::env::consts::EXE_SUFFIX);
    let binary_path = output_dir.join(binary_name);
    let status = Command::new("rustc")
        .arg(&adapter_source_path)
        .arg("-O")
        .arg("-o")
        .arg(&binary_path)
        .status()?;
    if !status.success() {
        return Err(RuntimeError::Message(
            "rustc failed while compiling the direct adapter binary".to_owned(),
        ));
    }

    Ok(BuildArtifacts {
        manifest_path,
        adapter_source_path,
        binary_path,
    })
}

fn find_server_node(module: &Module) -> Option<&NodeExpr> {
    module.items.iter().find_map(|item| match &item.kind {
        ItemKind::Stmt(Stmt::Expr(Expr::Node(node))) if node.name == "server" => Some(node),
        _ => None,
    })
}

fn compile_server(node: &NodeExpr) -> Result<ServerProgram, RuntimeError> {
    let body = expect_block(node.body.as_deref(), "@server body")?;
    let mut listen = None;
    let mut routes = Vec::new();

    for stmt in body {
        match stmt {
            Stmt::Expr(Expr::Node(child)) if child.name == "listen" => {
                listen = Some(parse_listen(child)?);
            }
            Stmt::Expr(Expr::Node(child)) if child.name == "route" => {
                routes.push(compile_route(child)?);
            }
            Stmt::Error => {}
            _ => {}
        }
    }

    Ok(ServerProgram {
        listen: listen.unwrap_or(8080),
        routes,
    })
}

fn compile_route(node: &NodeExpr) -> Result<Route, RuntimeError> {
    if node.positional.len() < 2 {
        return Err(RuntimeError::Message(
            "@route requires method and path positional arguments".to_owned(),
        ));
    }

    let method = expr_atom(&node.positional[0], "route method")?;
    let path = expr_atom(&node.positional[1], "route path")?;
    let body = expect_block(node.body.as_deref(), "@route body")?;

    let action = compile_route_action(body)?;
    Ok(Route {
        method,
        path,
        action,
    })
}

fn compile_route_action(body: &[Stmt]) -> Result<RouteAction, RuntimeError> {
    for stmt in body {
        match stmt {
            Stmt::Return(Some(Expr::Node(node))) if node.name == "response" => {
                return compile_response(node);
            }
            Stmt::Expr(Expr::Node(node)) if node.name == "response" => {
                return compile_response(node);
            }
            Stmt::Expr(Expr::Node(node)) if node.name == "serve" => {
                return compile_serve(node);
            }
            Stmt::Error => {}
            _ => {}
        }
    }

    Err(RuntimeError::Message(
        "route body must return @response or execute @serve".to_owned(),
    ))
}

fn compile_response(node: &NodeExpr) -> Result<RouteAction, RuntimeError> {
    let Some(status_expr) = node.positional.first() else {
        return Err(RuntimeError::Message(
            "@response requires an HTTP status code".to_owned(),
        ));
    };
    let status = match status_expr {
        Expr::IntLiteral(value) => u16::try_from(*value)
            .map_err(|_| RuntimeError::Message(format!("invalid HTTP status code `{value}`")))?,
        other => {
            return Err(RuntimeError::Message(format!(
                "@response status must be an integer literal, got {other:?}"
            )));
        }
    };

    let body = match node.body.as_deref() {
        None => String::new(),
        Some(expr) => json_body(expr)?,
    };

    Ok(RouteAction::JsonResponse { status, body })
}

fn compile_serve(node: &NodeExpr) -> Result<RouteAction, RuntimeError> {
    let Some(target) = node.positional.first() else {
        return Err(RuntimeError::Message(
            "@serve requires a target expression".to_owned(),
        ));
    };

    match target {
        Expr::Ident(name) if is_path_like(&name.name) => Ok(RouteAction::StaticServe {
            target: name.name.clone(),
        }),
        Expr::Node(html) if is_html_like(&html.name) => Ok(RouteAction::HtmlServe {
            html: render_html_node(html),
        }),
        other => Err(RuntimeError::Message(format!(
            "unsupported @serve target for direct adapter: {other:?}"
        ))),
    }
}

fn parse_listen(node: &NodeExpr) -> Result<u16, RuntimeError> {
    let Some(value) = node.positional.first() else {
        return Err(RuntimeError::Message(
            "@listen requires a port value".to_owned(),
        ));
    };

    match value {
        Expr::IntLiteral(port) => u16::try_from(*port)
            .map_err(|_| RuntimeError::Message(format!("invalid listen port `{port}`"))),
        other => Err(RuntimeError::Message(format!(
            "@listen currently supports integer literal ports in the runtime path, got {other:?}"
        ))),
    }
}

fn expect_block<'a>(expr: Option<&'a Expr>, context: &str) -> Result<&'a [Stmt], RuntimeError> {
    match expr {
        Some(Expr::Block { stmts, .. }) => Ok(stmts),
        other => Err(RuntimeError::Message(format!(
            "{context} must be a block, got {other:?}"
        ))),
    }
}

fn expr_atom(expr: &Expr, label: &str) -> Result<String, RuntimeError> {
    match expr {
        Expr::Ident(name) => Ok(name.name.clone()),
        Expr::StringLiteral(value) => Ok(value.clone()),
        other => Err(RuntimeError::Message(format!(
            "{label} must be a bare atom, got {other:?}"
        ))),
    }
}

fn json_body(expr: &Expr) -> Result<String, RuntimeError> {
    let value = json_value(expr)?;
    Ok(serde_json::to_string(&value)?)
}

fn json_value(expr: &Expr) -> Result<serde_json::Value, RuntimeError> {
    match expr {
        Expr::IntLiteral(value) => Ok(serde_json::Value::from(*value)),
        Expr::FloatLiteral(value) => Ok(serde_json::Value::from(*value)),
        Expr::StringLiteral(value) => Ok(serde_json::Value::from(value.clone())),
        Expr::BoolLiteral(value) => Ok(serde_json::Value::from(*value)),
        Expr::Void => Ok(serde_json::Value::Null),
        Expr::Object(fields) => {
            let mut map = serde_json::Map::with_capacity(fields.len());
            for field in fields {
                map.insert(field.key.clone(), json_value(&field.value)?);
            }
            Ok(serde_json::Value::Object(map))
        }
        Expr::Array(items) => {
            let mut values = Vec::with_capacity(items.len());
            for item in items {
                values.push(json_value(item)?);
            }
            Ok(serde_json::Value::Array(values))
        }
        Expr::Paren(inner) => json_value(inner),
        other => Err(RuntimeError::Message(format!(
            "direct adapter can only bake literal JSON bodies, got {other:?}"
        ))),
    }
}

fn render_html_node(node: &NodeExpr) -> String {
    match node.name.as_str() {
        "html" | "body" | "div" => {
            let tag = node.name.as_str();
            let inner = node
                .body
                .as_deref()
                .map(render_html_expr)
                .unwrap_or_default();
            format!("<{tag}>{inner}</{tag}>")
        }
        "text" => node
            .positional
            .iter()
            .filter_map(|expr| match expr {
                Expr::StringLiteral(value) => Some(value.as_str()),
                _ => None,
            })
            .collect(),
        _ => String::new(),
    }
}

fn render_html_expr(expr: &Expr) -> String {
    match expr {
        Expr::Block { stmts, .. } => stmts
            .iter()
            .filter_map(|stmt| match stmt {
                Stmt::Expr(Expr::Node(node)) => Some(render_html_node(node)),
                Stmt::Expr(Expr::StringLiteral(value)) => Some(value.clone()),
                _ => None,
            })
            .collect(),
        Expr::Node(node) => render_html_node(node),
        Expr::StringLiteral(value) => value.clone(),
        _ => String::new(),
    }
}

fn is_path_like(name: &str) -> bool {
    name.starts_with('/') || name.starts_with("./") || name.starts_with("../")
}

fn is_html_like(name: &str) -> bool {
    matches!(name, "html" | "body" | "div" | "text")
}

fn generate_direct_adapter_source(program: &Program) -> String {
    let mut arms = String::new();
    for route in &program.server.routes {
        let (status, content_type, body) = match &route.action {
            RouteAction::JsonResponse { status, body } => {
                (*status, "application/json", body.clone())
            }
            RouteAction::StaticServe { target } => {
                (200, "text/plain; charset=utf-8", format!("serve {target}"))
            }
            RouteAction::HtmlServe { html } => (200, "text/html; charset=utf-8", html.clone()),
        };
        arms.push_str(&format!(
            "        ({}, {}) => print_response({}, {}, {}),\n",
            rust_string(&route.method),
            rust_string(&route.path),
            status,
            rust_string(content_type),
            rust_string(&body),
        ));
    }

    format!(
        "use std::env;\n\nfn main() {{\n    let method = env::args().nth(1).unwrap_or_else(|| \"GET\".to_owned());\n    let path = env::args().nth(2).unwrap_or_else(|| \"/\".to_owned());\n    match (method.as_str(), path.as_str()) {{\n{arms}        _ => {{\n            eprintln!(\"no route matched {{}} {{}}\", method, path);\n            std::process::exit(1);\n        }}\n    }}\n}}\n\nfn print_response(status: u16, content_type: &str, body: &str) {{\n    println!(\"adapter: direct-match\");\n    println!(\"status: {{}}\", status);\n    println!(\"content-type: {{}}\", content_type);\n    println!(\"body: {{}}\", body);\n}}\n"
    )
}

fn rust_string(value: &str) -> String {
    format!("{value:?}")
}

#[cfg(test)]
mod tests {
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    fn render_response_has_stable_text_format() {
        let response = RouteResponse {
            adapter: AdapterKind::DirectMatch,
            status: 200,
            content_type: "application/json",
            body: "{\"ok\":true}".to_owned(),
        };

        assert_eq!(
            render_response(&response),
            "adapter: direct-match\nstatus: 200\ncontent-type: application/json\nbody: {\"ok\":true}\n"
        );
    }
}
