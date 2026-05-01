//! Compiler-side artifacts for orv.
//!
//! The production code generator is still a roadmap item. This crate currently
//! owns small compiler artifacts that can be derived from HIR without emitting a
//! server binary or client WASM bundle.

use std::collections::HashSet;

use orv_diagnostics::Span;
use orv_hir::{
    origin_fingerprint, origin_id, HirBlock, HirCatchClause, HirExpr, HirExprKind, HirFunctionBody,
    HirObjectField, HirPattern, HirProgram, HirStmt, HirStringSegment,
};
use serde::{Deserialize, Serialize};

/// Current origin map schema version.
pub const ORIGIN_MAP_VERSION: u32 = 1;

/// Minimal source origin map for executable HIR nodes.
///
/// This is the first compiler artifact behind SPEC §16.11. It does not wire
/// production telemetry or editor reveal yet; it provides stable ids and spans
/// for runtime/editor layers to attach to later.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OriginMap {
    /// Schema version.
    pub version: u32,
    /// Source-backed executable entries in HIR traversal order.
    pub entries: Vec<OriginEntry>,
}

/// One source-backed executable node.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OriginEntry {
    /// Stable id derived from kind/name/span.
    pub id: String,
    /// Node class, for example `domain`, `route`, `function`, `call`.
    pub kind: String,
    /// Human-readable node name.
    pub name: String,
    /// Source span.
    pub span: OriginSpan,
    /// Compact span fingerprint for production artifacts.
    pub fingerprint: String,
}

/// Serializable source span.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OriginSpan {
    /// HIR file id.
    pub file: u32,
    /// Start byte offset.
    pub start: u32,
    /// End byte offset.
    pub end: u32,
}

/// Build a deterministic origin map from HIR.
#[must_use]
pub fn origin_map(program: &HirProgram) -> OriginMap {
    let mut collector = OriginCollector::default();
    for stmt in &program.items {
        collector.visit_stmt(stmt);
    }
    OriginMap {
        version: ORIGIN_MAP_VERSION,
        entries: collector.entries,
    }
}

#[derive(Default)]
struct OriginCollector {
    entries: Vec<OriginEntry>,
    seen: HashSet<String>,
}

impl OriginCollector {
    fn push(&mut self, kind: &str, name: impl Into<String>, span: Span) {
        if span.file == orv_diagnostics::FileId::DUMMY {
            return;
        }
        let name = name.into();
        let fingerprint = origin_fingerprint(kind, &name, span);
        let id = origin_id(kind, &name, span);
        if !self.seen.insert(id.clone()) {
            return;
        }
        self.entries.push(OriginEntry {
            id,
            kind: kind.to_string(),
            name,
            span: OriginSpan {
                file: span.file.index(),
                start: span.range.start,
                end: span.range.end,
            },
            fingerprint,
        });
    }

    fn visit_stmt(&mut self, stmt: &HirStmt) {
        match stmt {
            HirStmt::Let(stmt) => self.visit_expr(&stmt.init),
            HirStmt::Const(stmt) => self.visit_expr(&stmt.init),
            HirStmt::Function(stmt) => {
                self.push("function", stmt.name.name.clone(), stmt.span);
                self.visit_function_body(&stmt.body);
            }
            HirStmt::Struct(_) | HirStmt::Enum(_) | HirStmt::TypeAlias(_) | HirStmt::Import(_) => {}
            HirStmt::Return(stmt) => {
                if let Some(value) = &stmt.value {
                    self.visit_expr(value);
                }
            }
            HirStmt::Expr(expr) => self.visit_expr(expr),
        }
    }

    fn visit_block(&mut self, block: &HirBlock) {
        for stmt in &block.stmts {
            self.visit_stmt(stmt);
        }
    }

    fn visit_function_body(&mut self, body: &HirFunctionBody) {
        match body {
            HirFunctionBody::Block(block) => self.visit_block(block),
            HirFunctionBody::Expr(expr) => self.visit_expr(expr),
        }
    }

    fn visit_expr(&mut self, expr: &HirExpr) {
        match &expr.kind {
            HirExprKind::Out(inner) => {
                self.push("domain", "out", expr.span);
                self.visit_expr(inner);
            }
            HirExprKind::Html(block) => {
                self.push("domain", "html", expr.span);
                self.visit_block(block);
            }
            HirExprKind::Route {
                method,
                path,
                handler,
                ..
            } => {
                self.push("route", format!("{method} {path}"), expr.span);
                self.visit_block(handler);
            }
            HirExprKind::Respond { status, payload } => {
                self.push("domain", "respond", expr.span);
                self.visit_expr(status);
                self.visit_expr(payload);
            }
            HirExprKind::Server {
                listen,
                routes,
                body_stmts,
            } => {
                self.push("domain", "server", expr.span);
                if let Some(listen) = listen {
                    self.visit_expr(listen);
                }
                for route in routes {
                    self.visit_expr(route);
                }
                for stmt in body_stmts {
                    self.visit_stmt(stmt);
                }
            }
            HirExprKind::Domain { name, args, .. } => {
                self.push("domain", name.clone(), expr.span);
                for arg in args {
                    self.visit_expr(arg);
                }
            }
            HirExprKind::Call { callee, args } => {
                self.push("call", call_name(callee), expr.span);
                self.visit_expr(callee);
                for arg in args {
                    self.visit_expr(arg);
                }
            }
            HirExprKind::String(segments) => {
                for segment in segments {
                    if let HirStringSegment::Interp(expr) = segment {
                        self.visit_expr(expr);
                    }
                }
            }
            HirExprKind::Unary { expr, .. }
            | HirExprKind::Paren(expr)
            | HirExprKind::Throw(expr)
            | HirExprKind::Await(expr) => self.visit_expr(expr),
            HirExprKind::Binary { lhs, rhs, .. } => {
                self.visit_expr(lhs);
                self.visit_expr(rhs);
            }
            HirExprKind::Block(block) => self.visit_block(block),
            HirExprKind::If {
                cond,
                then,
                else_branch,
            } => {
                self.visit_expr(cond);
                self.visit_block(then);
                if let Some(expr) = else_branch {
                    self.visit_expr(expr);
                }
            }
            HirExprKind::When { scrutinee, arms } => {
                self.visit_expr(scrutinee);
                for arm in arms {
                    self.visit_pattern(&arm.pattern);
                    self.visit_expr(&arm.body);
                }
            }
            HirExprKind::Assign { value, .. } => self.visit_expr(value),
            HirExprKind::AssignField { object, value, .. } => {
                self.visit_expr(object);
                self.visit_expr(value);
            }
            HirExprKind::AssignIndex {
                object,
                index,
                value,
            } => {
                self.visit_expr(object);
                self.visit_expr(index);
                self.visit_expr(value);
            }
            HirExprKind::For { iter, body, .. } => {
                self.visit_expr(iter);
                self.visit_block(body);
            }
            HirExprKind::While { cond, body } => {
                self.visit_expr(cond);
                self.visit_block(body);
            }
            HirExprKind::Range { start, end, .. } => {
                self.visit_expr(start);
                self.visit_expr(end);
            }
            HirExprKind::Array(items) | HirExprKind::Tuple(items) => {
                for item in items {
                    self.visit_expr(item);
                }
            }
            HirExprKind::Object(fields) | HirExprKind::TypedObject { fields, .. } => {
                self.visit_fields(fields);
            }
            HirExprKind::Index { target, index } => {
                self.visit_expr(target);
                self.visit_expr(index);
            }
            HirExprKind::Slice { target, start, end } => {
                self.visit_expr(target);
                if let Some(start) = start {
                    self.visit_expr(start);
                }
                if let Some(end) = end {
                    self.visit_expr(end);
                }
            }
            HirExprKind::Field { target, .. } | HirExprKind::OptionalField { target, .. } => {
                self.visit_expr(target);
            }
            HirExprKind::Lambda { body, .. } => self.visit_function_body(body),
            HirExprKind::Cast { expr, .. } => self.visit_expr(expr),
            HirExprKind::Try { try_block, catch } => {
                self.visit_block(try_block);
                if let Some(catch) = catch {
                    self.visit_catch(catch);
                }
            }
            HirExprKind::Integer(_)
            | HirExprKind::Float(_)
            | HirExprKind::Regex { .. }
            | HirExprKind::True
            | HirExprKind::False
            | HirExprKind::Void
            | HirExprKind::TypeName(_)
            | HirExprKind::Ident(_)
            | HirExprKind::Break
            | HirExprKind::Continue => {}
        }
    }

    fn visit_fields(&mut self, fields: &[HirObjectField]) {
        for field in fields {
            self.visit_expr(&field.value);
        }
    }

    fn visit_catch(&mut self, catch: &HirCatchClause) {
        self.visit_block(&catch.body);
    }

    fn visit_pattern(&mut self, pattern: &HirPattern) {
        match pattern {
            HirPattern::Literal(expr)
            | HirPattern::Guard(expr)
            | HirPattern::Not(expr)
            | HirPattern::Contains(expr) => self.visit_expr(expr),
            HirPattern::Range { start, end, .. } => {
                self.visit_expr(start);
                self.visit_expr(end);
            }
            HirPattern::Wildcard => {}
        }
    }
}

fn call_name(callee: &HirExpr) -> String {
    match &callee.kind {
        HirExprKind::Ident(ident) => ident.name.clone(),
        HirExprKind::Field { target, field, .. } => {
            format!("{}.{}", call_name(target), field)
        }
        HirExprKind::OptionalField { target, field, .. } => {
            format!("{}?.{}", call_name(target), field)
        }
        HirExprKind::Domain { name, .. } => format!("@{name}"),
        HirExprKind::TypeName(name) => name.clone(),
        _ => "<expr>".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orv_diagnostics::FileId;
    use orv_syntax::{lex, parse_with_newlines};

    fn lower(src: &str) -> orv_hir::HirProgram {
        let lx = lex(src, FileId(0));
        assert!(lx.diagnostics.is_empty(), "lex: {:?}", lx.diagnostics);
        let pr = parse_with_newlines(lx.tokens, FileId(0), lx.newlines);
        assert!(pr.diagnostics.is_empty(), "parse: {:?}", pr.diagnostics);
        let resolved = orv_resolve::resolve(&pr.program);
        assert!(
            resolved.diagnostics.is_empty(),
            "resolve: {:?}",
            resolved.diagnostics
        );
        let lowered = orv_analyzer::lower_with_diagnostics(&pr.program, &resolved);
        assert!(
            lowered.diagnostics.is_empty(),
            "analyzer: {:?}",
            lowered.diagnostics
        );
        lowered.program
    }

    #[test]
    fn origin_map_collects_server_route_and_response_nodes() {
        let program = lower(
            r"@server {
  @listen 8080
  @route GET /ping {
    @respond 200 { ok: true }
  }
}",
        );
        let map = origin_map(&program);
        let names: Vec<_> = map
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        assert_eq!(map.version, ORIGIN_MAP_VERSION);
        assert!(names.contains(&"server"), "{names:?}");
        assert!(names.contains(&"GET /ping"), "{names:?}");
        assert!(names.contains(&"respond"), "{names:?}");
        assert!(map
            .entries
            .iter()
            .all(|entry| entry.span.start < entry.span.end));
    }

    #[test]
    fn origin_map_ids_are_unique_and_stable() {
        let program = lower(
            r#"function greet(name: string): string -> "hi {name}"
@out greet("orv")"#,
        );
        let first = origin_map(&program);
        let second = origin_map(&program);
        let ids: HashSet<_> = first.entries.iter().map(|entry| &entry.id).collect();
        assert_eq!(ids.len(), first.entries.len());
        assert_eq!(first, second);
        assert!(first
            .entries
            .iter()
            .any(|entry| entry.kind == "function" && entry.name == "greet"));
        assert!(first
            .entries
            .iter()
            .any(|entry| entry.kind == "domain" && entry.name == "out"));
        assert!(first
            .entries
            .iter()
            .any(|entry| entry.kind == "call" && entry.name == "greet"));
    }
}
