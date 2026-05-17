//! Compiler-side artifacts for orv.
//!
//! The production code generator is still a roadmap item. This crate currently
//! owns small compiler artifacts that can be derived from HIR without emitting a
//! server binary or optimized client WASM bundle. HTML-only entries can plan a
//! static page artifact with no shipped runtime features, and server entries
//! declare native server plan/package/source/command contracts without claiming
//! final native codegen yet.

#![allow(
    clippy::match_same_arms,
    clippy::missing_const_for_fn,
    clippy::needless_raw_string_hashes,
    clippy::too_many_lines,
    clippy::unnecessary_wraps
)]

use std::collections::{HashMap, HashSet};

use orv_diagnostics::Span;
use orv_hir::{
    origin_fingerprint, origin_id, BinaryOp, HirBlock, HirCatchClause, HirExpr, HirExprKind,
    HirFunctionBody, HirLetKind, HirObjectField, HirPattern, HirProgram, HirStmt, HirStringSegment,
    NameId,
};

mod artifacts;
pub use artifacts::*;

// Internal condition value marker used when three dynamic operands mean
// base-vs-triple-product rather than product-vs-product.
const NATIVE_CONDITION_TRIPLE_PRODUCT: &str = "__orv_triple_product";

/// Build a deterministic origin map from HIR.
#[must_use]
pub fn origin_map(program: &HirProgram) -> OriginMap {
    let mut collector = OriginCollector::default();
    collector.index_top_level_functions(program);
    for stmt in &program.items {
        collector.visit_stmt(stmt);
    }
    OriginMap {
        version: ORIGIN_MAP_VERSION,
        entries: collector.entries,
        edges: collector.edges,
    }
}

/// Build a deterministic manifest for compiler-emitted artifacts.
#[must_use]
pub fn build_manifest(entry: impl Into<String>, origin_map: &OriginMap) -> BuildManifest {
    let server_routes = origin_map
        .entries
        .iter()
        .filter(|entry| entry.kind == "route")
        .count();
    let has_server = server_routes > 0
        || origin_map
            .entries
            .iter()
            .any(|entry| entry.kind == "domain" && entry.name == "server");
    let mut runtime_features = runtime_features(origin_map, has_server, server_routes);
    let client_wasm = requires_client_wasm(origin_map, has_server, &runtime_features);
    if client_wasm
        && !runtime_features
            .iter()
            .any(|feature| feature == "client_wasm")
    {
        runtime_features.push("client_wasm".to_string());
    }
    let mut artifacts = vec![
        BuildArtifact {
            kind: "build_manifest".to_string(),
            path: "build-manifest.json".to_string(),
        },
        BuildArtifact {
            kind: "origin_map".to_string(),
            path: "origin-map.json".to_string(),
        },
        BuildArtifact {
            kind: "bundle_plan".to_string(),
            path: "bundle-plan.json".to_string(),
        },
        BuildArtifact {
            kind: "project_graph".to_string(),
            path: "project-graph.json".to_string(),
        },
        BuildArtifact {
            kind: "source_bundle".to_string(),
            path: "source-bundle.json".to_string(),
        },
    ];
    if has_server {
        artifacts.push(BuildArtifact {
            kind: "server_runtime".to_string(),
            path: "server/app.orv-runtime.json".to_string(),
        });
        artifacts.push(BuildArtifact {
            kind: "server_launcher".to_string(),
            path: "server/launch.json".to_string(),
        });
        artifacts.push(BuildArtifact {
            kind: "native_server_plan".to_string(),
            path: "server/native-server.json".to_string(),
        });
        artifacts.push(BuildArtifact {
            kind: "native_runtime_image_plan".to_string(),
            path: "server/runtime-image.json".to_string(),
        });
        artifacts.push(BuildArtifact {
            kind: "native_runtime_image_dockerfile".to_string(),
            path: "server/native/Dockerfile".to_string(),
        });
        artifacts.push(BuildArtifact {
            kind: "native_server_launcher_source".to_string(),
            path: "server/native/main.rs".to_string(),
        });
        artifacts.push(BuildArtifact {
            kind: "native_server_routes_source".to_string(),
            path: "server/native/routes.rs".to_string(),
        });
        artifacts.push(BuildArtifact {
            kind: "native_server_router_source".to_string(),
            path: "server/native/router.rs".to_string(),
        });
        artifacts.push(BuildArtifact {
            kind: "native_server_handlers_source".to_string(),
            path: "server/native/handlers.rs".to_string(),
        });
        artifacts.push(BuildArtifact {
            kind: "native_server_launcher_package".to_string(),
            path: "server/native/Cargo.toml".to_string(),
        });
    }
    if has_static_page(has_server, &runtime_features) {
        artifacts.push(BuildArtifact {
            kind: "static_page".to_string(),
            path: "pages/index.html".to_string(),
        });
    }
    if client_wasm {
        artifacts.push(BuildArtifact {
            kind: "client_manifest".to_string(),
            path: "client/manifest.json".to_string(),
        });
        artifacts.push(BuildArtifact {
            kind: "client_reactive_plan".to_string(),
            path: "client/reactive-plan.json".to_string(),
        });
        artifacts.push(BuildArtifact {
            kind: "client_page".to_string(),
            path: "pages/index.html".to_string(),
        });
        artifacts.push(BuildArtifact {
            kind: "client_js".to_string(),
            path: "client/app.js".to_string(),
        });
        artifacts.push(BuildArtifact {
            kind: "client_wasm".to_string(),
            path: "client/app.wasm".to_string(),
        });
    }
    BuildManifest {
        schema_version: BUILD_MANIFEST_VERSION,
        entry: entry.into(),
        runtime: "reference-interpreter".to_string(),
        artifacts,
        capabilities: BuildCapabilities {
            has_server,
            server_routes,
            client_wasm,
            runtime_features,
        },
    }
}

/// Build a deterministic bundle plan from manifest capabilities.
#[must_use]
pub fn bundle_plan(manifest: &BuildManifest) -> BundlePlan {
    let mut bundles = Vec::new();
    if manifest.capabilities.has_server {
        bundles.push(BundleTarget {
            kind: "server_runtime".to_string(),
            path: "server/app.orv-runtime.json".to_string(),
            runtime_features: manifest.capabilities.runtime_features.clone(),
        });
        bundles.push(BundleTarget {
            kind: "server_launcher".to_string(),
            path: "server/launch.json".to_string(),
            runtime_features: manifest.capabilities.runtime_features.clone(),
        });
        bundles.push(BundleTarget {
            kind: "native_server_plan".to_string(),
            path: "server/native-server.json".to_string(),
            runtime_features: manifest.capabilities.runtime_features.clone(),
        });
        bundles.push(BundleTarget {
            kind: "native_runtime_image_plan".to_string(),
            path: "server/runtime-image.json".to_string(),
            runtime_features: manifest.capabilities.runtime_features.clone(),
        });
        bundles.push(BundleTarget {
            kind: "native_runtime_image_dockerfile".to_string(),
            path: "server/native/Dockerfile".to_string(),
            runtime_features: manifest.capabilities.runtime_features.clone(),
        });
        bundles.push(BundleTarget {
            kind: "native_server_launcher_source".to_string(),
            path: "server/native/main.rs".to_string(),
            runtime_features: manifest.capabilities.runtime_features.clone(),
        });
        bundles.push(BundleTarget {
            kind: "native_server_routes_source".to_string(),
            path: "server/native/routes.rs".to_string(),
            runtime_features: manifest.capabilities.runtime_features.clone(),
        });
        bundles.push(BundleTarget {
            kind: "native_server_router_source".to_string(),
            path: "server/native/router.rs".to_string(),
            runtime_features: manifest.capabilities.runtime_features.clone(),
        });
        bundles.push(BundleTarget {
            kind: "native_server_handlers_source".to_string(),
            path: "server/native/handlers.rs".to_string(),
            runtime_features: manifest.capabilities.runtime_features.clone(),
        });
        bundles.push(BundleTarget {
            kind: "native_server_launcher_package".to_string(),
            path: "server/native/Cargo.toml".to_string(),
            runtime_features: manifest.capabilities.runtime_features.clone(),
        });
    }
    if has_static_page(
        manifest.capabilities.has_server,
        &manifest.capabilities.runtime_features,
    ) {
        bundles.push(BundleTarget {
            kind: "static_page".to_string(),
            path: "pages/index.html".to_string(),
            runtime_features: Vec::new(),
        });
    }
    if manifest.capabilities.client_wasm {
        bundles.push(BundleTarget {
            kind: "client_manifest".to_string(),
            path: "client/manifest.json".to_string(),
            runtime_features: vec!["client_wasm".to_string()],
        });
        bundles.push(BundleTarget {
            kind: "client_reactive_plan".to_string(),
            path: "client/reactive-plan.json".to_string(),
            runtime_features: vec!["client_wasm".to_string()],
        });
        bundles.push(BundleTarget {
            kind: "client_page".to_string(),
            path: "pages/index.html".to_string(),
            runtime_features: vec!["client_wasm".to_string()],
        });
        bundles.push(BundleTarget {
            kind: "client_js".to_string(),
            path: "client/app.js".to_string(),
            runtime_features: vec!["client_wasm".to_string()],
        });
        bundles.push(BundleTarget {
            kind: "client_wasm".to_string(),
            path: "client/app.wasm".to_string(),
            runtime_features: vec!["client_wasm".to_string()],
        });
    }
    BundlePlan {
        schema_version: BUNDLE_PLAN_VERSION,
        bundles,
    }
}

fn has_static_page(has_server: bool, runtime_features: &[String]) -> bool {
    !has_server
        && runtime_features
            .iter()
            .any(|feature| feature == "html_renderer")
        && !runtime_features
            .iter()
            .any(|feature| feature == "client_wasm")
}

fn requires_client_wasm(
    origin_map: &OriginMap,
    has_server: bool,
    runtime_features: &[String],
) -> bool {
    let has_html = runtime_features
        .iter()
        .any(|feature| feature == "html_renderer");
    let has_signal = has_html
        && origin_map
            .entries
            .iter()
            .any(|entry| entry.kind == "signal");
    let html_await =
        has_html && !has_server && origin_map.entries.iter().any(|entry| entry.kind == "await");
    has_signal || html_await
}

/// Build a server runtime descriptor from manifest capabilities and origins.
#[must_use]
pub fn server_runtime_artifact(
    manifest: &BuildManifest,
    origin_map: &OriginMap,
    sources: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
) -> ServerRuntimeArtifact {
    let responses_by_route = HashMap::new();
    let policies_by_route = HashMap::new();
    server_runtime_artifact_with_responses(
        manifest,
        origin_map,
        &responses_by_route,
        &policies_by_route,
        sources,
    )
}

/// Build a server runtime descriptor and lower static route response metadata.
#[must_use]
pub fn server_runtime_artifact_with_program(
    manifest: &BuildManifest,
    origin_map: &OriginMap,
    program: &HirProgram,
    sources: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
) -> ServerRuntimeArtifact {
    let responses_by_route = route_response_artifacts(program);
    let policies_by_route = route_policy_artifacts(program);
    server_runtime_artifact_with_responses(
        manifest,
        origin_map,
        &responses_by_route,
        &policies_by_route,
        sources,
    )
}

fn server_runtime_artifact_with_responses(
    manifest: &BuildManifest,
    origin_map: &OriginMap,
    responses_by_route: &HashMap<String, Vec<ServerResponseArtifact>>,
    policies_by_route: &HashMap<String, Vec<ServerRoutePolicyArtifact>>,
    sources: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
) -> ServerRuntimeArtifact {
    let routes = origin_map
        .entries
        .iter()
        .filter(|entry| entry.kind == "route")
        .filter_map(|entry| {
            route_artifact(entry, origin_map, responses_by_route, policies_by_route)
        })
        .collect();
    let listen = origin_map
        .entries
        .iter()
        .find(|entry| entry.kind == "listen")
        .map(listen_artifact);
    ServerRuntimeArtifact {
        schema_version: SERVER_RUNTIME_ARTIFACT_VERSION,
        entry: manifest.entry.clone(),
        runtime: manifest.runtime.clone(),
        runtime_features: manifest.capabilities.runtime_features.clone(),
        routes,
        listen,
        source_bundle: ServerSourceBundle {
            files: sources
                .into_iter()
                .map(|(path, source)| {
                    let source = source.into();
                    ServerSourceFile {
                        path: path.into(),
                        content_hash: content_hash(&source),
                        source,
                    }
                })
                .collect(),
        },
    }
}

/// Build a source bundle artifact for production-to-code reveal.
pub fn source_bundle_artifact(
    entry: impl Into<String>,
    sources: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
) -> SourceBundleArtifact {
    SourceBundleArtifact {
        schema_version: SOURCE_BUNDLE_ARTIFACT_VERSION,
        entry: entry.into(),
        files: sources
            .into_iter()
            .map(|(path, source)| {
                let source = source.into();
                SourceBundleFile {
                    path: path.into(),
                    content_hash: content_hash(&source),
                    source,
                }
            })
            .collect(),
    }
}

/// Build a reference server launch descriptor for a runtime artifact.
#[must_use]
pub fn server_launch_artifact(
    artifact_path: impl Into<String>,
    artifact: &ServerRuntimeArtifact,
) -> ServerLaunchArtifact {
    let artifact_path = artifact_path.into();
    ServerLaunchArtifact {
        schema_version: SERVER_LAUNCH_ARTIFACT_VERSION,
        runtime: artifact.runtime.clone(),
        artifact: artifact_path.clone(),
        command: vec!["orv".to_string(), "run-artifact".to_string(), artifact_path],
        protocol: "http1".to_string(),
        routes: artifact.routes.clone(),
        listen: artifact.listen.clone(),
    }
}

mod native;
pub use native::{
    native_server_direct_http_capable, native_server_handlers_source,
    native_server_launcher_source, native_server_router_source, native_server_routes_source,
};

mod server_artifacts;
use server_artifacts::{
    content_hash, listen_artifact, route_artifact, route_policy_artifacts,
    route_response_artifacts, runtime_features,
};
pub use server_artifacts::{verify_server_runtime_artifact, verify_source_bundle_artifact};

#[derive(Default)]
struct OriginCollector {
    entries: Vec<OriginEntry>,
    edges: Vec<OriginEdge>,
    seen: HashSet<String>,
    seen_edges: HashSet<(String, String, String)>,
    parents: Vec<String>,
    function_origins: HashMap<NameId, String>,
}

impl OriginCollector {
    fn index_top_level_functions(&mut self, program: &HirProgram) {
        for stmt in &program.items {
            if let HirStmt::Function(stmt) = stmt {
                self.index_function_origin(stmt.name.id, &stmt.name.name, stmt.span);
            }
        }
    }

    fn index_function_origin(&mut self, id: NameId, name: &str, span: Span) {
        if span.file != orv_diagnostics::FileId::DUMMY {
            self.function_origins
                .insert(id, origin_id("function", name, span));
        }
    }

    fn push(&mut self, kind: &str, name: impl Into<String>, span: Span) -> Option<String> {
        if span.file == orv_diagnostics::FileId::DUMMY {
            return None;
        }
        let name = name.into();
        let fingerprint = origin_fingerprint(kind, &name, span);
        let id = origin_id(kind, &name, span);
        if let Some(parent) = self.parents.last().cloned() {
            self.push_edge(parent, id.clone(), "contains");
        }
        if self.seen.insert(id.clone()) {
            self.entries.push(OriginEntry {
                id: id.clone(),
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
        Some(id)
    }

    fn push_edge(&mut self, from: String, to: String, kind: &str) {
        if from == to {
            return;
        }
        let kind = kind.to_string();
        if self
            .seen_edges
            .insert((from.clone(), to.clone(), kind.clone()))
        {
            self.edges.push(OriginEdge { from, to, kind });
        }
    }

    fn with_origin(
        &mut self,
        kind: &str,
        name: impl Into<String>,
        span: Span,
        f: impl FnOnce(&mut Self),
    ) {
        if let Some(id) = self.push(kind, name, span) {
            self.parents.push(id);
            f(self);
            self.parents.pop();
        } else {
            f(self);
        }
    }

    fn visit_stmt(&mut self, stmt: &HirStmt) {
        match stmt {
            HirStmt::Let(stmt) => {
                if stmt.kind == HirLetKind::Signal {
                    self.with_origin("signal", stmt.name.name.clone(), stmt.span, |this| {
                        this.visit_expr(&stmt.init);
                    });
                } else {
                    self.visit_expr(&stmt.init);
                }
            }
            HirStmt::Const(stmt) => self.visit_expr(&stmt.init),
            HirStmt::Function(stmt) => {
                self.index_function_origin(stmt.name.id, &stmt.name.name, stmt.span);
                self.with_origin("function", stmt.name.name.clone(), stmt.span, |this| {
                    this.visit_function_body(&stmt.body);
                });
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
                self.with_origin("domain", "out", expr.span, |this| {
                    this.visit_expr(inner);
                });
            }
            HirExprKind::Html(block) => {
                self.with_origin("domain", "html", expr.span, |this| {
                    this.visit_block(block);
                });
            }
            HirExprKind::Route {
                method,
                path,
                handler,
                ..
            } => {
                self.with_origin("route", format!("{method} {path}"), expr.span, |this| {
                    this.visit_block(handler);
                });
            }
            HirExprKind::Respond { status, payload } => {
                self.with_origin("domain", "respond", expr.span, |this| {
                    this.visit_expr(status);
                    this.visit_expr(payload);
                });
            }
            HirExprKind::Server {
                listen,
                routes,
                body_stmts,
            } => {
                self.with_origin("domain", "server", expr.span, |this| {
                    if let Some(listen) = listen {
                        this.with_origin("listen", listen_name(listen), listen.span, |this| {
                            this.visit_expr(listen);
                        });
                    }
                    for route in routes {
                        this.visit_expr(route);
                    }
                    for stmt in body_stmts {
                        this.visit_stmt(stmt);
                    }
                });
            }
            HirExprKind::Domain { name, args, .. } => {
                self.with_origin("domain", name.clone(), expr.span, |this| {
                    for arg in args {
                        this.visit_expr(arg);
                    }
                });
            }
            HirExprKind::Call { callee, args } => {
                let name = call_name(callee);
                let call_id = origin_id("call", &name, expr.span);
                self.with_origin("call", name, expr.span, |this| {
                    this.visit_expr(callee);
                    for arg in args {
                        this.visit_expr(arg);
                    }
                });
                if expr.span.file != orv_diagnostics::FileId::DUMMY {
                    if let Some(target) =
                        call_target(callee).and_then(|id| self.function_origins.get(&id).cloned())
                    {
                        self.push_edge(call_id, target, "calls");
                    }
                }
            }
            HirExprKind::String(segments) => {
                for segment in segments {
                    if let HirStringSegment::Interp(expr) = segment {
                        self.visit_expr(expr);
                    }
                }
            }
            HirExprKind::Await(expr) => {
                self.with_origin("await", "await", expr.span, |this| {
                    this.visit_expr(expr);
                });
            }
            HirExprKind::Unary { expr, .. }
            | HirExprKind::Paren(expr)
            | HirExprKind::Throw(expr)
            | HirExprKind::Cast { expr, .. } => self.visit_expr(expr),
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

fn call_target(callee: &HirExpr) -> Option<NameId> {
    match &callee.kind {
        HirExprKind::Ident(ident) => Some(ident.id),
        _ => None,
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

fn listen_name(expr: &HirExpr) -> String {
    if let Some(env) = listen_env_from_expr(expr) {
        if let Some(default_port) = env.default_port {
            return format!("port env {} default {default_port}", env.variable);
        }
        return format!("port env {}", env.variable);
    }
    match &expr.kind {
        HirExprKind::Integer(raw) => format!("port {raw}"),
        HirExprKind::Ident(ident) => format!("port {}", ident.name),
        HirExprKind::Field { .. }
        | HirExprKind::OptionalField { .. }
        | HirExprKind::Domain { .. }
        | HirExprKind::TypeName(_) => format!("port {}", call_name(expr)),
        _ => "port <expr>".to_string(),
    }
}

fn listen_env_from_expr(expr: &HirExpr) -> Option<ServerListenEnvArtifact> {
    let HirExprKind::Call { callee, args } = &expr.kind else {
        return None;
    };
    if call_name(callee) != "int.from" || args.len() != 1 {
        return None;
    }
    let arg = args.first()?;
    let (env_expr, default_port) = match &arg.kind {
        HirExprKind::Binary {
            op: BinaryOp::Coalesce,
            lhs,
            rhs,
        } => (lhs.as_ref(), string_port(rhs.as_ref())),
        _ => (arg, None),
    };
    let variable = env_variable(env_expr)?;
    Some(ServerListenEnvArtifact {
        variable,
        default_port,
    })
}

fn env_variable(expr: &HirExpr) -> Option<String> {
    let HirExprKind::Field { target, field, .. } = &expr.kind else {
        return None;
    };
    let HirExprKind::Domain { name, args, .. } = &target.kind else {
        return None;
    };
    if name == "env" && args.is_empty() {
        Some(field.clone())
    } else {
        None
    }
}

fn string_port(expr: &HirExpr) -> Option<u16> {
    let HirExprKind::String(segments) = &expr.kind else {
        return None;
    };
    let [HirStringSegment::Str(raw)] = segments.as_slice() else {
        return None;
    };
    raw.parse::<u16>().ok()
}

#[cfg(test)]
mod tests;
