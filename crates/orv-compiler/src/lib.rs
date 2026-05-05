//! Compiler-side artifacts for orv.
//!
//! The production code generator is still a roadmap item. This crate currently
//! owns small compiler artifacts that can be derived from HIR without emitting a
//! server binary or optimized client WASM bundle. HTML-only entries can plan a
//! static page artifact with no shipped runtime features, and server entries
//! declare native server plan/source contracts without claiming final native
//! codegen yet.

use std::collections::{BTreeSet, HashMap, HashSet};

use orv_diagnostics::Span;
use orv_hir::{
    origin_fingerprint, origin_id, BinaryOp, HirBlock, HirCatchClause, HirExpr, HirExprKind,
    HirFunctionBody, HirLetKind, HirObjectField, HirPattern, HirProgram, HirStmt, HirStringSegment,
    NameId,
};
use serde::{Deserialize, Serialize};

/// Current origin map schema version.
pub const ORIGIN_MAP_VERSION: u32 = 2;

/// Current build manifest schema version.
pub const BUILD_MANIFEST_VERSION: u32 = 1;

/// Current bundle plan schema version.
pub const BUNDLE_PLAN_VERSION: u32 = 1;

/// Current server runtime artifact schema version.
pub const SERVER_RUNTIME_ARTIFACT_VERSION: u32 = 1;

/// Current server launch artifact schema version.
pub const SERVER_LAUNCH_ARTIFACT_VERSION: u32 = 1;

/// Current build source bundle artifact schema version.
pub const SOURCE_BUNDLE_ARTIFACT_VERSION: u32 = 1;

/// Minimal build artifact manifest.
///
/// This is the first compiler-facing build artifact. It records deterministic
/// graph/origin outputs for downstream bundling without claiming production
/// server or WASM code generation yet. HTML-only inputs may include a static
/// page artifact path.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BuildManifest {
    /// Schema version.
    pub schema_version: u32,
    /// Entry `.orv` file used for this artifact set.
    pub entry: String,
    /// Runtime model used by this build artifact.
    pub runtime: String,
    /// Files written into the artifact directory.
    pub artifacts: Vec<BuildArtifact>,
    /// Capability summary derived from compiler artifacts.
    pub capabilities: BuildCapabilities,
}

/// One file emitted by `orv build`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BuildArtifact {
    /// Artifact class, for example `origin_map`.
    pub kind: String,
    /// Relative path inside the build output directory.
    pub path: String,
}

/// Planned production bundle outputs.
///
/// This is a contract for the future bundler. It is intentionally explicit so
/// zero-overhead checks can compare planned outputs with required runtime
/// features before code generation exists.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BundlePlan {
    /// Schema version.
    pub schema_version: u32,
    /// Bundle targets to produce.
    pub bundles: Vec<BundleTarget>,
}

/// One planned bundle target.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BundleTarget {
    /// Bundle class, for example `server_runtime`.
    pub kind: String,
    /// Relative output path inside the build output directory.
    pub path: String,
    /// Runtime layers that this bundle needs.
    pub runtime_features: Vec<String>,
}

/// Server runtime descriptor emitted by the initial bundler path.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ServerRuntimeArtifact {
    /// Schema version.
    pub schema_version: u32,
    /// Entry `.orv` file used for this artifact set.
    pub entry: String,
    /// Runtime model used by this artifact.
    pub runtime: String,
    /// Runtime layers required by this server artifact.
    pub runtime_features: Vec<String>,
    /// HTTP route descriptors.
    pub routes: Vec<ServerRouteArtifact>,
    /// Source-backed listen descriptor, when the server declares one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listen: Option<ServerListenArtifact>,
    /// Source snapshot for reference runner hydration.
    pub source_bundle: ServerSourceBundle,
}

/// Reference server launch descriptor.
///
/// Native server binaries are still roadmap work. This artifact gives deploy
/// tooling a deterministic command/protocol contract for the current reference
/// runner path.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ServerLaunchArtifact {
    /// Schema version.
    pub schema_version: u32,
    /// Runtime model used by this launch descriptor.
    pub runtime: String,
    /// Relative server runtime artifact path.
    pub artifact: String,
    /// Command argv for launching the reference server artifact.
    pub command: Vec<String>,
    /// Transport protocol used by the reference runtime.
    pub protocol: String,
    /// HTTP route descriptors reachable through this launcher.
    pub routes: Vec<ServerRouteArtifact>,
    /// Source-backed listen descriptor used by the reference server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listen: Option<ServerListenArtifact>,
}

/// Source snapshot embedded in the reference runtime artifact.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ServerSourceBundle {
    /// Source files needed to rehydrate the project.
    pub files: Vec<ServerSourceFile>,
}

/// One source file captured for reference runner hydration.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ServerSourceFile {
    /// File path as seen by the build.
    pub path: String,
    /// Stable content hash.
    pub content_hash: String,
    /// Source text.
    pub source: String,
}

/// Build-level source snapshot used by reveal tooling for all bundle types.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SourceBundleArtifact {
    /// Schema version.
    pub schema_version: u32,
    /// Entry `.orv` file used for this artifact set.
    pub entry: String,
    /// Source files needed to reveal production origins.
    pub files: Vec<SourceBundleFile>,
}

/// One source file captured for build-level reveal.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SourceBundleFile {
    /// File path as seen by the build.
    pub path: String,
    /// Stable content hash.
    pub content_hash: String,
    /// Source text.
    pub source: String,
}

/// One compiled HTTP route descriptor.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ServerRouteArtifact {
    /// HTTP method.
    pub method: String,
    /// Route path pattern.
    pub path: String,
    /// Origin id for production-to-code tracing.
    pub origin_id: String,
}

/// One compiled server listen descriptor.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ServerListenArtifact {
    /// Origin id for production-to-code tracing.
    pub origin_id: String,
    /// Human-readable listen expression summary.
    pub name: String,
    /// Statically known port, if the listen expression is an integer literal.
    pub port: Option<u16>,
    /// Environment variable backing the port expression, when statically known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<ServerListenEnvArtifact>,
}

/// Environment-driven server listen descriptor.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ServerListenEnvArtifact {
    /// Environment variable name, for example `PORT`.
    pub variable: String,
    /// Statically known default port from `@env.PORT ?? "8080"`, when present.
    pub default_port: Option<u16>,
}

/// Build capability summary.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BuildCapabilities {
    /// Whether the input contains an executable server domain.
    pub has_server: bool,
    /// Number of compiled HTTP routes.
    pub server_routes: usize,
    /// Whether this artifact set includes client WASM.
    pub client_wasm: bool,
    /// Runtime layers required by the current artifact set.
    pub runtime_features: Vec<String>,
}

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
    /// Parent-child edges derived from HIR traversal.
    pub edges: Vec<OriginEdge>,
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

/// Relationship between source-backed executable nodes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OriginEdge {
    /// Parent origin id.
    pub from: String,
    /// Child origin id.
    pub to: String,
    /// Edge class, for example `contains` or `calls`.
    pub kind: String,
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
            kind: "native_server_launcher_source".to_string(),
            path: "server/native/main.rs".to_string(),
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
            kind: "native_server_launcher_source".to_string(),
            path: "server/native/main.rs".to_string(),
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
    let routes = origin_map
        .entries
        .iter()
        .filter(|entry| entry.kind == "route")
        .filter_map(route_artifact)
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

/// Verify that a server runtime artifact is internally consistent.
///
/// This checks the source bundle hashes and route descriptor shape. It does not
/// replace type checking; it validates that an emitted artifact was not
/// accidentally corrupted before a runner hydrates it.
///
/// # Errors
///
/// Returns all validation failures when source hashes do not match, source
/// bundle files are missing paths, or route descriptors are incomplete.
pub fn verify_server_runtime_artifact(artifact: &ServerRuntimeArtifact) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    if artifact.source_bundle.files.is_empty() {
        errors.push("source bundle is empty".to_string());
    }
    for file in &artifact.source_bundle.files {
        if file.path.is_empty() {
            errors.push("source bundle contains file with empty path".to_string());
        }
        let actual = content_hash(&file.source);
        if file.content_hash != actual {
            errors.push(format!(
                "content hash mismatch for {}: expected {}, got {}",
                file.path, file.content_hash, actual
            ));
        }
    }
    for route in &artifact.routes {
        if route.method.is_empty() {
            errors.push("route method is empty".to_string());
        }
        if route.path.is_empty() {
            errors.push("route path is empty".to_string());
        }
        if route.origin_id.is_empty() {
            errors.push(format!(
                "route {} {} has empty origin id",
                route.method, route.path
            ));
        }
    }
    if let Some(listen) = &artifact.listen {
        if listen.origin_id.is_empty() {
            errors.push("listen origin_id is empty".to_string());
        }
        if listen.name.is_empty() {
            errors.push("listen name is empty".to_string());
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Verify that a build source bundle artifact is internally consistent.
///
/// # Errors
///
/// Returns all validation failures when source hashes do not match or source
/// bundle files are missing paths.
pub fn verify_source_bundle_artifact(artifact: &SourceBundleArtifact) -> Result<(), Vec<String>> {
    let mut errors = Vec::new();
    if artifact.schema_version != SOURCE_BUNDLE_ARTIFACT_VERSION {
        errors.push(format!(
            "unsupported source bundle schema_version {}",
            artifact.schema_version
        ));
    }
    if artifact.entry.trim().is_empty() {
        errors.push("source bundle entry is empty".to_string());
    }
    if artifact.files.is_empty() {
        errors.push("source bundle is empty".to_string());
    }
    for file in &artifact.files {
        if file.path.trim().is_empty() {
            errors.push("source bundle contains file with empty path".to_string());
        }
        let actual = content_hash(&file.source);
        if file.content_hash != actual {
            errors.push(format!(
                "content hash mismatch for {}: expected {}, got {}",
                file.path, file.content_hash, actual
            ));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn route_artifact(entry: &OriginEntry) -> Option<ServerRouteArtifact> {
    let (method, path) = entry.name.split_once(' ')?;
    Some(ServerRouteArtifact {
        method: method.to_string(),
        path: path.to_string(),
        origin_id: entry.id.clone(),
    })
}

fn listen_artifact(entry: &OriginEntry) -> ServerListenArtifact {
    ServerListenArtifact {
        origin_id: entry.id.clone(),
        name: entry.name.clone(),
        port: listen_port(&entry.name),
        env: listen_env_from_name(&entry.name),
    }
}

fn listen_port(name: &str) -> Option<u16> {
    name.strip_prefix("port ")?.parse::<u16>().ok()
}

fn listen_env_from_name(name: &str) -> Option<ServerListenEnvArtifact> {
    let rest = name.strip_prefix("port env ")?;
    let (variable, default_port) = if let Some((variable, default)) = rest.split_once(" default ") {
        (variable, default.parse::<u16>().ok())
    } else {
        (rest, None)
    };
    Some(ServerListenEnvArtifact {
        variable: variable.to_string(),
        default_port,
    })
}

fn content_hash(source: &str) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in source.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("fnv1a64:{hash:016x}")
}

fn runtime_features(origin_map: &OriginMap, has_server: bool, server_routes: usize) -> Vec<String> {
    let mut features = BTreeSet::new();
    if has_server {
        features.insert("http_server");
    }
    if server_routes > 0 {
        features.insert("router");
    }
    for entry in &origin_map.entries {
        if entry.kind != "domain" {
            continue;
        }
        match entry.name.as_str() {
            "db" => {
                features.insert("in_memory_db");
            }
            "html" => {
                features.insert("html_renderer");
            }
            "out" => {
                features.insert("console_io");
            }
            "serve" => {
                features.insert("static_file_server");
            }
            _ => {}
        }
    }
    features.into_iter().map(str::to_string).collect()
}

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
        assert!(names.contains(&"port 8080"), "{names:?}");
        assert!(names.contains(&"GET /ping"), "{names:?}");
        assert!(names.contains(&"respond"), "{names:?}");
        assert!(map
            .entries
            .iter()
            .all(|entry| entry.span.start < entry.span.end));
    }

    #[test]
    fn origin_map_collects_traversal_parent_edges() {
        let program = lower(
            r"@server {
  @listen 8080
  @route GET /ping {
    @respond 200 { ok: true }
  }
}",
        );
        let map = origin_map(&program);
        let server = map
            .entries
            .iter()
            .find(|entry| entry.kind == "domain" && entry.name == "server")
            .expect("server origin");
        let route = map
            .entries
            .iter()
            .find(|entry| entry.kind == "route" && entry.name == "GET /ping")
            .expect("route origin");
        let listen = map
            .entries
            .iter()
            .find(|entry| entry.kind == "listen" && entry.name == "port 8080")
            .expect("listen origin");
        let respond = map
            .entries
            .iter()
            .find(|entry| entry.kind == "domain" && entry.name == "respond")
            .expect("respond origin");

        assert!(map.edges.iter().any(|edge| {
            edge.kind == "contains" && edge.from == server.id && edge.to == listen.id
        }));
        assert!(map
            .edges
            .iter()
            .any(|edge| edge.kind == "contains" && edge.from == server.id && edge.to == route.id));
        assert!(map.edges.iter().any(|edge| {
            edge.kind == "contains" && edge.from == route.id && edge.to == respond.id
        }));
    }

    #[test]
    fn origin_map_collects_call_target_edges() {
        let program = lower(
            r#"function greet(name: string): string -> "hi {name}"
@out greet("orv")"#,
        );
        let map = origin_map(&program);
        let function = map
            .entries
            .iter()
            .find(|entry| entry.kind == "function" && entry.name == "greet")
            .expect("function origin");
        let call = map
            .entries
            .iter()
            .find(|entry| entry.kind == "call" && entry.name == "greet")
            .expect("call origin");

        assert!(map
            .edges
            .iter()
            .any(|edge| edge.kind == "calls" && edge.from == call.id && edge.to == function.id));
    }

    #[test]
    fn origin_map_collects_forward_call_target_edges() {
        let program = lower(
            r#"function useGreet(): string -> greet("orv")
function greet(name: string): string -> "hi {name}""#,
        );
        let map = origin_map(&program);
        let function = map
            .entries
            .iter()
            .find(|entry| entry.kind == "function" && entry.name == "greet")
            .expect("function origin");
        let call = map
            .entries
            .iter()
            .find(|entry| entry.kind == "call" && entry.name == "greet")
            .expect("call origin");

        assert!(map
            .edges
            .iter()
            .any(|edge| edge.kind == "calls" && edge.from == call.id && edge.to == function.id));
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

    #[test]
    fn origin_map_records_signal_and_await_client_markers() {
        let program = lower(
            r#"let sig count: int = 0
@out await count"#,
        );
        let map = origin_map(&program);

        assert!(map
            .entries
            .iter()
            .any(|entry| entry.kind == "signal" && entry.name == "count"));
        assert!(map.entries.iter().any(|entry| entry.kind == "await"));
    }

    #[test]
    fn build_manifest_declares_reference_artifacts_and_route_count() {
        let program = lower(
            r"@server {
  @listen 8080
  @route GET /ping {
    @respond 200 { ok: true }
  }
}",
        );
        let map = origin_map(&program);
        let manifest = build_manifest("fixtures/e2e/hello.orv", &map);

        assert_eq!(manifest.schema_version, BUILD_MANIFEST_VERSION);
        assert_eq!(manifest.entry, "fixtures/e2e/hello.orv");
        assert_eq!(manifest.runtime, "reference-interpreter");
        assert_eq!(manifest.capabilities.server_routes, 1);
        assert!(manifest.capabilities.has_server);
        assert!(!manifest.capabilities.client_wasm);
        assert!(manifest
            .artifacts
            .iter()
            .any(|artifact| artifact.kind == "origin_map" && artifact.path == "origin-map.json"));
        assert!(manifest.artifacts.iter().any(|artifact| {
            artifact.kind == "project_graph" && artifact.path == "project-graph.json"
        }));
        assert!(manifest.artifacts.iter().any(|artifact| {
            artifact.kind == "source_bundle" && artifact.path == "source-bundle.json"
        }));
    }

    #[test]
    fn build_manifest_declares_only_required_runtime_features() {
        let program = lower(
            r#"@server {
  @listen 8080
  @route GET /ping {
    @respond 200 await @db.find("User", { name: "Ada" })
  }
}"#,
        );
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);

        assert_eq!(
            manifest.capabilities.runtime_features,
            vec!["http_server", "in_memory_db", "router"]
        );
        assert!(!manifest
            .capabilities
            .runtime_features
            .contains(&"html_renderer".to_string()));
        assert!(!manifest
            .capabilities
            .runtime_features
            .contains(&"static_file_server".to_string()));
    }

    #[test]
    fn bundle_plan_declares_server_runtime_without_client_wasm() {
        let program = lower(
            r"@server {
  @listen 8080
  @route GET /ping {
    @respond 200 { ok: true }
  }
}",
        );
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let plan = bundle_plan(&manifest);

        assert_eq!(plan.schema_version, BUNDLE_PLAN_VERSION);
        assert!(plan.bundles.iter().any(|bundle| {
            bundle.kind == "server_runtime"
                && bundle.path == "server/app.orv-runtime.json"
                && bundle.runtime_features.contains(&"http_server".to_string())
                && bundle.runtime_features.contains(&"router".to_string())
        }));
        assert!(!plan
            .bundles
            .iter()
            .any(|bundle| bundle.kind == "client_wasm"));
    }

    #[test]
    fn bundle_plan_declares_server_launch_artifact() {
        let program = lower(
            r"@server {
  @listen 8080
  @route GET /ping {
    @respond 200 { ok: true }
  }
}",
        );
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let plan = bundle_plan(&manifest);

        assert!(plan.bundles.iter().any(|bundle| {
            bundle.kind == "server_launcher"
                && bundle.path == "server/launch.json"
                && bundle.runtime_features.contains(&"http_server".to_string())
                && bundle.runtime_features.contains(&"router".to_string())
        }));
    }

    #[test]
    fn bundle_plan_declares_native_server_plan_contract() {
        let program = lower(
            r"@server {
  @listen 8080
  @route GET /ping {
    @respond 200 { ok: true }
  }
}",
        );
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let plan = bundle_plan(&manifest);

        assert!(plan.bundles.iter().any(|bundle| {
            bundle.kind == "native_server_plan"
                && bundle.path == "server/native-server.json"
                && bundle.runtime_features.contains(&"http_server".to_string())
                && bundle.runtime_features.contains(&"router".to_string())
        }));
        assert!(plan.bundles.iter().any(|bundle| {
            bundle.kind == "native_server_launcher_source"
                && bundle.path == "server/native/main.rs"
                && bundle.runtime_features.contains(&"http_server".to_string())
                && bundle.runtime_features.contains(&"router".to_string())
        }));
    }

    #[test]
    fn build_manifest_declares_server_artifacts() {
        let program = lower(
            r"@server {
  @listen 8080
  @route GET /ping {
    @respond 200 { ok: true }
  }
}",
        );
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);

        assert!(manifest.artifacts.iter().any(|artifact| {
            artifact.kind == "server_runtime" && artifact.path == "server/app.orv-runtime.json"
        }));
        assert!(manifest.artifacts.iter().any(|artifact| {
            artifact.kind == "server_launcher" && artifact.path == "server/launch.json"
        }));
        assert!(manifest.artifacts.iter().any(|artifact| {
            artifact.kind == "native_server_plan" && artifact.path == "server/native-server.json"
        }));
        assert!(manifest.artifacts.iter().any(|artifact| {
            artifact.kind == "native_server_launcher_source"
                && artifact.path == "server/native/main.rs"
        }));
    }

    #[test]
    fn build_manifest_declares_static_page_artifact_for_html_only() {
        let program = lower(r#"@out @html { @body { @h1 "Home" } }"#);
        let map = origin_map(&program);
        let manifest = build_manifest("page.orv", &map);

        assert!(!manifest.capabilities.has_server);
        assert!(manifest.artifacts.iter().any(|artifact| {
            artifact.kind == "static_page" && artifact.path == "pages/index.html"
        }));
    }

    #[test]
    fn bundle_plan_declares_static_page_zero_runtime_for_html_only() {
        let program = lower(r#"@out @html { @body { @h1 "Home" } }"#);
        let map = origin_map(&program);
        let manifest = build_manifest("page.orv", &map);
        let plan = bundle_plan(&manifest);

        let page = plan
            .bundles
            .iter()
            .find(|bundle| bundle.kind == "static_page")
            .expect("static page bundle");
        assert_eq!(page.path, "pages/index.html");
        assert!(page.runtime_features.is_empty());
        assert!(!plan
            .bundles
            .iter()
            .any(|bundle| bundle.kind == "server_runtime"));
        assert!(!plan
            .bundles
            .iter()
            .any(|bundle| bundle.kind == "client_wasm"));
    }

    #[test]
    fn bundle_plan_declares_client_bootstrap_targets_for_signal_html() {
        let program = lower(
            r#"let sig count: int = 0
@out @html { @body { @p count } }"#,
        );
        let map = origin_map(&program);
        let manifest = build_manifest("page.orv", &map);
        let plan = bundle_plan(&manifest);

        assert!(manifest.capabilities.client_wasm);
        assert!(manifest
            .capabilities
            .runtime_features
            .contains(&"client_wasm".to_string()));
        assert!(!manifest
            .artifacts
            .iter()
            .any(|artifact| artifact.kind == "static_page"));
        assert!(manifest
            .artifacts
            .iter()
            .any(|artifact| artifact.kind == "client_wasm" && artifact.path == "client/app.wasm"));
        assert!(manifest
            .artifacts
            .iter()
            .any(|artifact| artifact.kind == "client_js" && artifact.path == "client/app.js"));
        assert!(manifest
            .artifacts
            .iter()
            .any(|artifact| artifact.kind == "client_page" && artifact.path == "pages/index.html"));
        assert!(plan.bundles.iter().any(|bundle| {
            bundle.kind == "client_wasm"
                && bundle.path == "client/app.wasm"
                && bundle.runtime_features == vec!["client_wasm"]
        }));
        assert!(plan.bundles.iter().any(|bundle| {
            bundle.kind == "client_js"
                && bundle.path == "client/app.js"
                && bundle.runtime_features == vec!["client_wasm"]
        }));
        assert!(plan.bundles.iter().any(|bundle| {
            bundle.kind == "client_page"
                && bundle.path == "pages/index.html"
                && bundle.runtime_features == vec!["client_wasm"]
        }));
        assert!(!plan
            .bundles
            .iter()
            .any(|bundle| bundle.kind == "static_page"));
    }

    #[test]
    fn bundle_plan_keeps_signal_without_html_out_of_client_wasm() {
        let program = lower(
            r#"let sig count: int = 0
@out count"#,
        );
        let map = origin_map(&program);
        let manifest = build_manifest("page.orv", &map);
        let plan = bundle_plan(&manifest);

        assert!(!manifest.capabilities.client_wasm);
        assert!(!manifest
            .capabilities
            .runtime_features
            .contains(&"client_wasm".to_string()));
        assert!(!manifest
            .artifacts
            .iter()
            .any(|artifact| artifact.kind == "client_wasm"));
        assert!(!plan
            .bundles
            .iter()
            .any(|bundle| bundle.kind == "client_wasm"));
    }

    #[test]
    fn server_runtime_artifact_declares_routes_and_runtime_features() {
        let program = lower(
            r"@server {
  @listen 8080
  @route GET /ping {
    @respond 200 { ok: true }
  }
}",
        );
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let artifact = server_runtime_artifact(
            &manifest,
            &map,
            [(
                "server.orv",
                r"@server {
  @listen 8080
  @route GET /ping {
    @respond 200 { ok: true }
  }
}",
            )],
        );

        assert_eq!(artifact.schema_version, SERVER_RUNTIME_ARTIFACT_VERSION);
        assert_eq!(artifact.entry, "server.orv");
        assert_eq!(artifact.runtime, "reference-interpreter");
        assert_eq!(artifact.routes.len(), 1);
        assert_eq!(artifact.routes[0].method, "GET");
        assert_eq!(artifact.routes[0].path, "/ping");
        assert!(artifact.routes[0].origin_id.starts_with("ori_"));
        let listen = artifact.listen.as_ref().expect("listen descriptor");
        assert_eq!(listen.port, Some(8080));
        assert!(listen.origin_id.starts_with("ori_"));
        assert_eq!(artifact.runtime_features, vec!["http_server", "router"]);
        assert_eq!(artifact.source_bundle.files.len(), 1);
        assert_eq!(artifact.source_bundle.files[0].path, "server.orv");
        assert!(artifact.source_bundle.files[0]
            .source
            .contains("@route GET /ping"));
        assert!(artifact.source_bundle.files[0]
            .content_hash
            .starts_with("fnv1a64:"));
    }

    #[test]
    fn server_runtime_artifact_records_env_listen_descriptor() {
        let src = r#"@server {
  @listen int.from(@env.PORT ?? "8080")
  @route GET /ping {
    @respond 200 { ok: true }
  }
}"#;
        let program = lower(src);
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let artifact = server_runtime_artifact(&manifest, &map, [("server.orv", src)]);

        let listen = artifact.listen.as_ref().expect("listen descriptor");
        assert_eq!(listen.port, None);
        let env = listen.env.as_ref().expect("listen env descriptor");
        assert_eq!(env.variable, "PORT");
        assert_eq!(env.default_port, Some(8080));
    }

    #[test]
    fn server_runtime_artifact_verification_rejects_hash_mismatch() {
        let program = lower(
            r"@server {
  @listen 8080
  @route GET /ping {
    @respond 200 { ok: true }
  }
}",
        );
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let mut artifact = server_runtime_artifact(&manifest, &map, [("server.orv", "@server {}")]);
        artifact.source_bundle.files[0]
            .source
            .push_str("\n@out \"changed\"");

        let errors = verify_server_runtime_artifact(&artifact).expect_err("hash mismatch");
        assert!(errors
            .iter()
            .any(|error| error.contains("content hash mismatch for server.orv")));
    }
}
