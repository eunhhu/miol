//! Compiler-side artifacts for orv.
//!
//! The production code generator is still a roadmap item. This crate currently
//! owns small compiler artifacts that can be derived from HIR without emitting a
//! server binary or optimized client WASM bundle. HTML-only entries can plan a
//! static page artifact with no shipped runtime features, and server entries
//! declare native server plan/package/source/command contracts without claiming
//! final native codegen yet.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;

use orv_diagnostics::Span;
use orv_hir::{
    origin_fingerprint, origin_id, BinaryOp, HirBlock, HirCatchClause, HirExpr, HirExprKind,
    HirFunctionBody, HirLetKind, HirObjectField, HirPattern, HirProgram, HirStmt, HirStringSegment,
    HirTypeRef, HirTypeRefKind, NameId, UnaryOp,
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

/// Current native server plan artifact schema version.
pub const NATIVE_SERVER_PLAN_ARTIFACT_VERSION: u32 = 1;

/// Current native runtime image plan artifact schema version.
pub const NATIVE_RUNTIME_IMAGE_PLAN_ARTIFACT_VERSION: u32 = 1;

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

/// Native server output descriptor.
///
/// Direct-lowered HTTP artifacts can use the generated native launcher now;
/// other artifacts record the reference launcher package/source and blockers
/// before the final native server target is complete.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NativeServerPlanArtifact {
    /// Schema version.
    pub schema_version: u32,
    /// Artifact kind.
    pub kind: String,
    /// Planning status, for example `direct_http` or `planned`.
    pub status: String,
    /// Runtime model used before native codegen exists.
    pub runtime: String,
    /// Runtime layers required by this server artifact.
    pub runtime_features: Vec<String>,
    /// Relative server runtime artifact path.
    pub artifact: String,
    /// Relative server launch artifact path.
    pub launcher: String,
    /// Relative generated Rust launcher source path.
    pub source: String,
    /// Relative generated Rust route table source path.
    pub routes_source: String,
    /// Relative generated Rust router dispatch source path.
    pub router_source: String,
    /// Relative generated Rust route handler source path.
    pub handlers_source: String,
    /// Relative generated Rust launcher package path.
    pub package: String,
    /// Relative native runtime image plan artifact path.
    pub runtime_image_plan: String,
    /// Planned final native target.
    pub target: NativeServerTargetArtifact,
    /// Generated reference launcher commands.
    pub commands: NativeServerCommands,
    /// Blockers before this can become a final zero-overhead native runtime.
    pub blocked_by: Vec<String>,
    /// Source-backed listen descriptor used by the reference server.
    pub listen: Option<ServerListenArtifact>,
    /// HTTP route descriptors reachable through this launcher.
    pub routes: Vec<ServerRouteArtifact>,
}

/// Planned final native target descriptor.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NativeServerTargetArtifact {
    /// Target kind, currently `server_binary`.
    pub kind: String,
    /// Planned output path.
    pub path: String,
    /// Transport protocol used by the target.
    pub protocol: String,
}

/// Generated native launcher commands.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NativeServerCommands {
    /// Build command argv.
    pub build: Vec<String>,
    /// Run command argv and environment.
    pub run: NativeServerRunCommand,
}

/// Generated native launcher run command.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NativeServerRunCommand {
    /// Environment variables for the command.
    pub env: HashMap<String, String>,
    /// Run command argv.
    pub command: Vec<String>,
}

/// Native runtime image output descriptor.
///
/// Direct-lowered HTTP artifacts can use the generated native Dockerfile now;
/// dynamic artifacts keep blockers until native codegen can lower them.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NativeRuntimeImagePlanArtifact {
    /// Schema version.
    pub schema_version: u32,
    /// Artifact kind.
    pub kind: String,
    /// Planning status, currently `planned`.
    pub status: String,
    /// Runtime model used before native codegen exists.
    pub runtime: String,
    /// Runtime layers required by this server artifact.
    pub runtime_features: Vec<String>,
    /// Relative server runtime artifact path.
    pub artifact: String,
    /// Relative native server plan artifact path.
    pub native_plan: String,
    /// Current reference container image used by deploy artifacts.
    pub reference_image: String,
    /// Planned final native runtime image target.
    pub target: NativeRuntimeImageTargetArtifact,
    /// Generated Dockerfile for the native image target.
    pub dockerfile: String,
    /// Generated native runtime image build command.
    pub commands: NativeRuntimeImageCommands,
    /// Blockers before this can become a final native runtime image.
    pub blocked_by: Vec<String>,
    /// Source-backed listen descriptor used by the reference server.
    pub listen: Option<ServerListenArtifact>,
    /// HTTP route descriptors reachable through this image.
    pub routes: Vec<ServerRouteArtifact>,
}

/// Planned final native runtime image target descriptor.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NativeRuntimeImageTargetArtifact {
    /// Target kind, currently `oci_image`.
    pub kind: String,
    /// Planned image tag.
    pub image: String,
    /// Native server binary expected inside the image.
    pub binary: String,
    /// Transport protocol used by the image.
    pub protocol: String,
}

/// Generated native runtime image commands.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NativeRuntimeImageCommands {
    /// Build command argv.
    pub build: Vec<String>,
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
    /// `@respond` origin ids contained in this route handler.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub response_origin_ids: Vec<String>,
    /// Lowered `@respond` descriptors contained in this route handler.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub responses: Vec<ServerResponseArtifact>,
}

/// One source-backed response descriptor contained by a route.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ServerResponseArtifact {
    /// `@respond` origin id.
    pub origin_id: String,
    /// Statically known HTTP status, when the status expression is literal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<i64>,
    /// Response body lowering class.
    pub body_kind: String,
    /// Native response guard condition for simple route-level control flow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<ServerResponseConditionArtifact>,
    /// Statically lowered JSON body, when the payload is literal-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_json: Option<String>,
    /// Ordered object JSON fields for mixed literal/domain-backed response bodies.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub body_object_fields: Vec<ServerResponseObjectFieldArtifact>,
    /// Object JSON fields lowered from route params such as `{ id: @param.id }`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub body_route_params: Vec<ServerResponseRouteParamArtifact>,
    /// Object JSON fields lowered from query params such as `{ q: @query.q }`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub body_query_params: Vec<ServerResponseQueryParamArtifact>,
    /// Object JSON fields lowered from the raw request body such as `{ received: @body }`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub body_request_json: Vec<ServerResponseRequestBodyArtifact>,
    /// Object JSON fields lowered from request body fields such as `{ handle: @body.handle }`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub body_request_fields: Vec<ServerResponseRequestBodyFieldArtifact>,
}

/// One native-lowerable response guard condition.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ServerResponseConditionArtifact {
    /// Condition class, for example `request_body_field_eq`, `route_param_eq`, or `query_param_ne`.
    pub kind: String,
    /// Captured field or parameter name used by the condition.
    pub name: String,
    /// Static string value compared by the condition.
    pub value: String,
    /// Captured field or parameter name used as the dynamic comparison operand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operand_name: Option<String>,
    /// Captured operand source used as the dynamic comparison operand.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operand_kind: Option<String>,
}

/// One ordered JSON object field backed by a static value or request domain.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ServerResponseObjectFieldArtifact {
    /// JSON field name in the response body.
    pub field: String,
    /// Field value class: `static_json`, route/query/request captured values, or `request_body_json`.
    pub value_kind: String,
    /// Statically lowered JSON value for `static_json` fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_json: Option<String>,
    /// Captured param or request body field name for domain-backed fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Arithmetic operation applied to a captured numeric field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op: Option<String>,
    /// Static JSON operand for numeric arithmetic operations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operand_json: Option<String>,
    /// Captured operand value class for dynamic numeric arithmetic operations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operand_kind: Option<String>,
    /// Captured operand field name for dynamic numeric arithmetic operations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operand_name: Option<String>,
}

/// One JSON object field backed by a captured route param.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ServerResponseRouteParamArtifact {
    /// JSON field name in the response body.
    pub field: String,
    /// Captured route parameter name.
    pub param: String,
    /// Field value class: `route_param`, `route_param_int`, or `route_param_float`.
    #[serde(
        default = "default_route_param_value_kind",
        skip_serializing_if = "is_default_route_param_value_kind"
    )]
    pub value_kind: String,
    /// Arithmetic operation applied to the captured numeric value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op: Option<String>,
    /// Static JSON operand for numeric arithmetic operations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operand_json: Option<String>,
    /// Captured operand value class for dynamic numeric arithmetic operations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operand_kind: Option<String>,
    /// Captured operand field name for dynamic numeric arithmetic operations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operand_name: Option<String>,
}

/// One JSON object field backed by a captured query param.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ServerResponseQueryParamArtifact {
    /// JSON field name in the response body.
    pub field: String,
    /// Captured query parameter name.
    pub param: String,
    /// Field value class: `query_param`, `query_param_int`, or `query_param_float`.
    #[serde(
        default = "default_query_param_value_kind",
        skip_serializing_if = "is_default_query_param_value_kind"
    )]
    pub value_kind: String,
    /// Arithmetic operation applied to the captured numeric value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op: Option<String>,
    /// Static JSON operand for numeric arithmetic operations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operand_json: Option<String>,
    /// Captured operand value class for dynamic numeric arithmetic operations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operand_kind: Option<String>,
    /// Captured operand field name for dynamic numeric arithmetic operations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operand_name: Option<String>,
}

/// One JSON object field backed by the request body JSON value.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ServerResponseRequestBodyArtifact {
    /// JSON field name in the response body.
    pub field: String,
}

/// One JSON object field backed by a request body object field.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ServerResponseRequestBodyFieldArtifact {
    /// JSON field name in the response body.
    pub field: String,
    /// Request body field name.
    pub name: String,
    /// Field value class: `request_body_field`, `request_body_field_int`, or `request_body_field_float`.
    #[serde(
        default = "default_request_body_field_value_kind",
        skip_serializing_if = "is_default_request_body_field_value_kind"
    )]
    pub value_kind: String,
    /// Optional arithmetic operator applied after numeric parsing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub op: Option<String>,
    /// Static JSON operand for the arithmetic operator.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operand_json: Option<String>,
    /// Captured operand class for dynamic arithmetic operands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operand_kind: Option<String>,
    /// Captured operand field name for dynamic arithmetic operands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operand_name: Option<String>,
}

fn default_request_body_field_value_kind() -> String {
    "request_body_field".to_string()
}

fn is_default_request_body_field_value_kind(value: &str) -> bool {
    value == "request_body_field"
}

fn default_route_param_value_kind() -> String {
    "route_param".to_string()
}

fn is_default_route_param_value_kind(value: &str) -> bool {
    value == "route_param"
}

fn default_query_param_value_kind() -> String {
    "query_param".to_string()
}

fn is_default_query_param_value_kind(value: &str) -> bool {
    value == "query_param"
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
    server_runtime_artifact_with_responses(manifest, origin_map, &responses_by_route, sources)
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
    server_runtime_artifact_with_responses(manifest, origin_map, &responses_by_route, sources)
}

fn server_runtime_artifact_with_responses(
    manifest: &BuildManifest,
    origin_map: &OriginMap,
    responses_by_route: &HashMap<String, Vec<ServerResponseArtifact>>,
    sources: impl IntoIterator<Item = (impl Into<String>, impl Into<String>)>,
) -> ServerRuntimeArtifact {
    let routes = origin_map
        .entries
        .iter()
        .filter(|entry| entry.kind == "route")
        .filter_map(|entry| route_artifact(entry, origin_map, responses_by_route))
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

/// Generate Rust source for the native server launcher bridge.
///
/// This source is still an incremental native launcher, not final native
/// codegen. It verifies that the native plan and server runtime artifact exist,
/// links the generated route/router/handler modules, and serves static-lowered
/// handlers directly through a small HTTP/1 loop.
#[must_use]
pub fn native_server_launcher_source(
    server_artifact_path: &str,
    native_server_plan_path: &str,
    artifact: &ServerRuntimeArtifact,
) -> String {
    if native_server_direct_http_capable(artifact) {
        native_server_direct_launcher_source(
            server_artifact_path,
            native_server_plan_path,
            artifact,
        )
    } else {
        native_server_reference_launcher_source(server_artifact_path, native_server_plan_path)
    }
}

#[allow(clippy::too_many_lines)]
fn native_server_direct_launcher_source(
    server_artifact_path: &str,
    native_server_plan_path: &str,
    artifact: &ServerRuntimeArtifact,
) -> String {
    let port_env = native_server_port_env(artifact).map_or_else(
        || "None".to_string(),
        |env| format!("Some({})", rust_string_literal(env)),
    );
    let mut source = r#"// Generated by orv build. This is an incremental native launcher
// source, not the final zero-overhead native server runtime.

mod routes;
mod router;
mod handlers;

const ORV_SERVER_ARTIFACT: &str = "__ORV_SERVER_ARTIFACT__";
const ORV_NATIVE_SERVER_PLAN: &str = "__ORV_NATIVE_SERVER_PLAN__";
const ORV_DEFAULT_HOST: &str = "127.0.0.1";
const ORV_DEFAULT_PORT: u16 = __ORV_DEFAULT_PORT__;
const ORV_PORT_ENV: Option<&str> = __ORV_PORT_ENV__;

fn main() -> std::process::ExitCode {{
    let _ = routes::ORV_NATIVE_ROUTE_COUNT;
    let _ = routes::orv_native_match_route("__orv_probe__", "__orv_probe__");
    let _ = router::ORV_NATIVE_HANDLER_COUNT;
    let _ = router::orv_native_dispatch("__orv_probe__", "__orv_probe__");
    let _ = handlers::ORV_NATIVE_HANDLER_COUNT;
    let build_dir = orv_build_dir();
    let native_plan = build_dir.join(ORV_NATIVE_SERVER_PLAN);
    if !native_plan.is_file() {{
        eprintln!(
            "missing orv native server plan {{}}; set ORV_BUILD_DIR or run from the generated native launcher path",
            native_plan.display()
        );
        return std::process::ExitCode::FAILURE;
    }}
    let artifact = build_dir.join(ORV_SERVER_ARTIFACT);
    if !artifact.is_file() {{
        eprintln!(
            "missing orv server artifact {{}}; set ORV_BUILD_DIR or run from the generated native launcher path",
            artifact.display()
        );
        return std::process::ExitCode::FAILURE;
    }}
    match orv_native_serve() {{
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {{
            eprintln!(
                "failed to run orv native server using {{}} from {{}}: {{error}}",
                ORV_SERVER_ARTIFACT,
                ORV_NATIVE_SERVER_PLAN
            );
            std::process::ExitCode::FAILURE
        }}
    }}
}}

#[derive(Debug)]
struct OrvNativeHttpRequest {{
    method: String,
    path: String,
    query: Vec<routes::OrvNativeParam>,
    body: String,
    body_fields: Vec<routes::OrvNativeParam>,
}}

fn orv_native_serve() -> std::io::Result<()> {{
    let listener = std::net::TcpListener::bind(orv_native_listen_address())?;
    eprintln!("orv native server listening on {{}}", listener.local_addr()?);
    for stream in listener.incoming() {{
        match stream {{
            Ok(mut stream) => {{
                if let Err(error) = orv_native_handle_connection(&mut stream) {{
                    eprintln!("orv native request failed: {{error}}");
                }}
            }}
            Err(error) => eprintln!("orv native accept failed: {{error}}"),
        }}
    }}
    Ok(())
}}

fn orv_native_listen_address() -> String {{
    let host = std::env::var("ORV_HOST").unwrap_or_else(|_| ORV_DEFAULT_HOST.to_string());
    let port = ORV_PORT_ENV
        .and_then(|variable| std::env::var(variable).ok())
        .or_else(|| std::env::var("ORV_PORT").ok())
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(ORV_DEFAULT_PORT);
    format!("{{host}}:{{port}}")
}}

fn orv_native_handle_connection(stream: &mut std::net::TcpStream) -> std::io::Result<()> {{
    use std::io::Write as _;

    let Some(request) = orv_native_read_request(stream)? else {{
        stream.write_all(&orv_native_plain_response(
            400,
            "Bad Request",
            "invalid HTTP request",
        ))?;
        return Ok(());
    }};
    let dispatch = router::orv_native_dispatch_with_request(
        &request.method,
        &request.path,
        request.query,
        request.body,
        request.body_fields,
    );
    stream.write_all(&orv_native_http_response(dispatch))
}}

fn orv_native_read_request(
    stream: &mut std::net::TcpStream,
) -> std::io::Result<Option<OrvNativeHttpRequest>> {{
    use std::io::Read as _;

    let mut bytes = Vec::new();
    let mut buf = [0_u8; 8192];
    let len = stream.read(&mut buf)?;
    if len == 0 {{
        return Ok(None);
    }}
    bytes.extend_from_slice(&buf[..len]);
    while orv_native_header_end(&bytes).is_none() && bytes.len() < 16 * 1024 {{
        let len = stream.read(&mut buf)?;
        if len == 0 {{
            break;
        }}
        bytes.extend_from_slice(&buf[..len]);
    }}
    let Some(header_end) = orv_native_header_end(&bytes) else {{
        return Ok(None);
    }};
    let head = String::from_utf8_lossy(&bytes[..header_end]);
    let Some(request_line) = head.lines().next() else {{
        return Ok(None);
    }};
    let mut parts = request_line.split_whitespace();
    let (Some(method), Some(target), Some(_version)) = (parts.next(), parts.next(), parts.next())
    else {{
        return Ok(None);
    }};
    let (path, query) = target
        .split_once('?')
        .map_or((target, Vec::new()), |(path, query)| {
            (path, orv_native_parse_query(query))
        });
    let method = method.to_string();
    let path = path.to_string();
    let body_start = header_end + 4;
    let content_length = orv_native_content_length(&head).unwrap_or(0);
    drop(head);
    while bytes.len().saturating_sub(body_start) < content_length {{
        let len = stream.read(&mut buf)?;
        if len == 0 {{
            break;
        }}
        bytes.extend_from_slice(&buf[..len]);
    }}
    let body_end = body_start + content_length.min(bytes.len().saturating_sub(body_start));
    let body = String::from_utf8(bytes[body_start..body_end].to_vec()).unwrap_or_default();
    let body_fields = orv_native_parse_body_fields(&body);
    Ok(Some(OrvNativeHttpRequest {{
        method,
        path,
        query,
        body,
        body_fields,
    }}))
}}

fn orv_native_header_end(bytes: &[u8]) -> Option<usize> {{
    bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
}}

fn orv_native_content_length(head: &str) -> Option<usize> {{
    head.lines().find_map(|line| {{
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("content-length")
            .then(|| value.trim().parse::<usize>().ok())
            .flatten()
    }})
}}

fn orv_native_parse_body_fields(body: &str) -> Vec<routes::OrvNativeParam> {{
    let trimmed = body.trim();
    if trimmed.starts_with('{{') {{
        orv_native_parse_json_object_fields(trimmed)
    }} else {{
        orv_native_parse_query(&body)
    }}
}}

fn orv_native_parse_json_object_fields(raw: &str) -> Vec<routes::OrvNativeParam> {{
    let bytes = raw.as_bytes();
    let mut fields = Vec::new();
    let mut index = 0;
    orv_native_skip_json_ws(bytes, &mut index);
    if bytes.get(index) != Some(&b'{{') {{
        return fields;
    }}
    index += 1;
    loop {{
        orv_native_skip_json_ws(bytes, &mut index);
        if bytes.get(index) == Some(&b'}}') || index >= bytes.len() {{
            break;
        }}
        let Some(name) = orv_native_parse_json_string(bytes, &mut index) else {{
            break;
        }};
        orv_native_skip_json_ws(bytes, &mut index);
        if bytes.get(index) != Some(&b':') {{
            break;
        }}
        index += 1;
        orv_native_skip_json_ws(bytes, &mut index);
        let value = if bytes.get(index) == Some(&b'"') {{
            orv_native_parse_json_string(bytes, &mut index).unwrap_or_default()
        }} else {{
            orv_native_parse_json_atom(bytes, &mut index)
        }};
        fields.push(routes::OrvNativeParam {{ name, value }});
        orv_native_skip_json_ws(bytes, &mut index);
        match bytes.get(index) {{
            Some(b',') => index += 1,
            Some(b'}}') | None => break,
            _ => break,
        }}
    }}
    fields
}}

fn orv_native_parse_json_atom(bytes: &[u8], index: &mut usize) -> String {{
    let start = *index;
    while *index < bytes.len() && !matches!(bytes[*index], b',' | b'}}') {{
        *index += 1;
    }}
    String::from_utf8_lossy(&bytes[start..*index]).trim().to_string()
}}

fn orv_native_parse_json_string(bytes: &[u8], index: &mut usize) -> Option<String> {{
    if bytes.get(*index) != Some(&b'"') {{
        return None;
    }}
    *index += 1;
    let mut out = String::new();
    while *index < bytes.len() {{
        match bytes[*index] {{
            b'"' => {{
                *index += 1;
                return Some(out);
            }}
            b'\\' => {{
                *index += 1;
                let escaped = *bytes.get(*index)?;
                match escaped {{
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'/' => out.push('/'),
                    b'b' => out.push('\u{{08}}'),
                    b'f' => out.push('\u{{0c}}'),
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'u' => {{
                        let end = (*index + 5).min(bytes.len());
                        out.push_str(&String::from_utf8_lossy(&bytes[*index - 1..end]));
                        *index = end.saturating_sub(1);
                    }}
                    byte => out.push(char::from(byte)),
                }}
                *index += 1;
            }}
            byte => {{
                out.push(char::from(byte));
                *index += 1;
            }}
        }}
    }}
    None
}}

fn orv_native_skip_json_ws(bytes: &[u8], index: &mut usize) {{
    while *index < bytes.len() && bytes[*index].is_ascii_whitespace() {{
        *index += 1;
    }}
}}

fn orv_native_parse_query(raw: &str) -> Vec<routes::OrvNativeParam> {{
    raw.split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {{
            let mut parts = pair.splitn(2, '=');
            let name = orv_native_percent_decode_form(parts.next().unwrap_or(""));
            let value = orv_native_percent_decode_form(parts.next().unwrap_or(""));
            routes::OrvNativeParam {{ name, value }}
        }})
        .collect()
}}

fn orv_native_percent_decode_form(raw: &str) -> String {{
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {{
        match bytes[index] {{
            b'+' => {{
                out.push(b' ');
                index += 1;
            }}
            b'%' if index + 2 < bytes.len() => {{
                let hi = orv_native_hex_value(bytes[index + 1]);
                let lo = orv_native_hex_value(bytes[index + 2]);
                if let (Some(hi), Some(lo)) = (hi, lo) {{
                    out.push((hi << 4) | lo);
                    index += 3;
                }} else {{
                    out.push(bytes[index]);
                    index += 1;
                }}
            }}
            byte => {{
                out.push(byte);
                index += 1;
            }}
        }}
    }}
    String::from_utf8(out).unwrap_or_else(|_| raw.to_string())
}}

fn orv_native_hex_value(byte: u8) -> Option<u8> {{
    match byte {{
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }}
}}

fn orv_native_http_response(dispatch: router::OrvNativeDispatch) -> Vec<u8> {{
    let include_body = !orv_native_status_disallows_body(dispatch.status);
    let body = if include_body {{
        dispatch.body.as_bytes()
    }} else {{
        &[]
    }};
    let mut head = format!(
        "HTTP/1.1 {{}} {{}}\r\ncontent-length: {{}}\r\nconnection: close\r\n",
        dispatch.status,
        orv_native_reason(dispatch.status),
        body.len()
    );
    if include_body {{
        head.push_str("content-type: ");
        head.push_str(dispatch.content_type);
        head.push_str("\r\n");
    }}
    if let Some(origin_id) = dispatch.origin_id {{
        head.push_str("x-orv-origin-id: ");
        head.push_str(origin_id);
        head.push_str("\r\n");
    }}
    if let Some(response_origin_id) = dispatch.response_origin_id {{
        head.push_str("x-orv-response-origin-id: ");
        head.push_str(response_origin_id);
        head.push_str("\r\n");
    }}
    head.push_str("\r\n");
    let mut response = head.into_bytes();
    response.extend_from_slice(body);
    response
}}

fn orv_native_status_disallows_body(status: u16) -> bool {{
    (100..=199).contains(&status) || status == 204 || status == 304
}}

fn orv_native_plain_response(status: u16, reason: &str, body: &str) -> Vec<u8> {{
    format!(
        "HTTP/1.1 {{status}} {{reason}}\r\ncontent-type: text/plain\r\ncontent-length: {{}}\r\nconnection: close\r\n\r\n{{body}}",
        body.as_bytes().len()
    )
    .into_bytes()
}}

fn orv_native_reason(status: u16) -> &'static str {{
    match status {{
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        400 => "Bad Request",
        404 => "Not Found",
        500 => "Internal Server Error",
        501 => "Not Implemented",
        _ => "OK",
    }}
}}

fn orv_build_dir() -> std::path::PathBuf {{
    if let Some(value) = std::env::var_os("ORV_BUILD_DIR") {{
        return std::path::PathBuf::from(value);
    }}
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent()?.parent()?.parent()?.parent()?.parent().map(std::path::Path::to_path_buf))
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}}
"#
    .replace("{{", "{")
    .replace("}}", "}");
    source = source.replace("__ORV_SERVER_ARTIFACT__", server_artifact_path);
    source = source.replace("__ORV_NATIVE_SERVER_PLAN__", native_server_plan_path);
    source = source.replace(
        "__ORV_DEFAULT_PORT__",
        &native_server_default_port(artifact).to_string(),
    );
    source.replace("__ORV_PORT_ENV__", &port_env)
}

fn native_server_reference_launcher_source(
    server_artifact_path: &str,
    native_server_plan_path: &str,
) -> String {
    let mut source = r#"// Generated by orv build. This is a reference fallback launcher
// source for dynamic routes that are not native-lowered yet.

mod routes;
mod router;
mod handlers;

const ORV_SERVER_ARTIFACT: &str = "__ORV_SERVER_ARTIFACT__";
const ORV_NATIVE_SERVER_PLAN: &str = "__ORV_NATIVE_SERVER_PLAN__";

fn main() -> std::process::ExitCode {{
    let _ = routes::ORV_NATIVE_ROUTE_COUNT;
    let _ = routes::orv_native_match_route("__orv_probe__", "__orv_probe__");
    let _ = router::ORV_NATIVE_HANDLER_COUNT;
    let _ = router::orv_native_dispatch("__orv_probe__", "__orv_probe__");
    let _ = handlers::ORV_NATIVE_HANDLER_COUNT;
    let build_dir = orv_build_dir();
    let native_plan = build_dir.join(ORV_NATIVE_SERVER_PLAN);
    if !native_plan.is_file() {{
        eprintln!(
            "missing orv native server plan {{}}; set ORV_BUILD_DIR or run from the generated native launcher path",
            native_plan.display()
        );
        return std::process::ExitCode::FAILURE;
    }}
    let artifact = build_dir.join(ORV_SERVER_ARTIFACT);
    if !artifact.is_file() {{
        eprintln!(
            "missing orv server artifact {{}}; set ORV_BUILD_DIR or run from the generated native launcher path",
            artifact.display()
        );
        return std::process::ExitCode::FAILURE;
    }}
    orv_native_reference_bridge(artifact)
}}

fn orv_native_reference_bridge(artifact: std::path::PathBuf) -> std::process::ExitCode {{
    let status = std::process::Command::new("orv")
        .arg("run-artifact")
        .arg(artifact)
        .args(std::env::args_os().skip(1))
        .status();
    match status {{
        Ok(status) if status.success() => std::process::ExitCode::SUCCESS,
        Ok(status) => {{
            let code = status
                .code()
                .and_then(|code| u8::try_from(code).ok())
                .unwrap_or(1);
            std::process::ExitCode::from(code)
        }}
        Err(error) => {{
            eprintln!(
                "failed to launch orv reference server using {{}} from {{}}: {{error}}",
                ORV_SERVER_ARTIFACT,
                ORV_NATIVE_SERVER_PLAN
            );
            std::process::ExitCode::FAILURE
        }}
    }}
}}

fn orv_build_dir() -> std::path::PathBuf {{
    if let Some(value) = std::env::var_os("ORV_BUILD_DIR") {{
        return std::path::PathBuf::from(value);
    }}
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent()?.parent()?.parent()?.parent()?.parent().map(std::path::Path::to_path_buf))
        .unwrap_or_else(|| std::path::PathBuf::from("."))
}}
"#
    .replace("{{", "{")
    .replace("}}", "}");
    source = source.replace("__ORV_SERVER_ARTIFACT__", server_artifact_path);
    source.replace("__ORV_NATIVE_SERVER_PLAN__", native_server_plan_path)
}

#[must_use]
pub fn native_server_direct_http_capable(artifact: &ServerRuntimeArtifact) -> bool {
    artifact
        .routes
        .iter()
        .all(native_server_route_has_native_response)
}

fn native_server_route_has_native_response(route: &ServerRouteArtifact) -> bool {
    let route_params = native_server_route_param_names(&route.path);
    if route.responses.len() == 1 {
        let response = &route.responses[0];
        return response.condition.is_none()
            && native_server_response_is_direct(response, &route_params);
    }
    if route.responses.len() < 2 {
        return false;
    }
    route.responses.iter().enumerate().all(|(index, response)| {
        native_server_response_is_direct(response, &route_params)
            && if index + 1 == route.responses.len() {
                response.condition.is_none()
            } else {
                native_server_response_condition_is_direct(response, &route_params)
            }
    })
}

fn native_server_response_is_direct(
    response: &ServerResponseArtifact,
    route_params: &[String],
) -> bool {
    response
        .status
        .and_then(|status| u16::try_from(status).ok())
        .is_some()
        && (response.body_kind == "empty"
            || response.body_json.is_some()
            || native_server_response_uses_object_fields(response, route_params)
            || native_server_response_uses_route_params(response, route_params)
            || native_server_response_uses_query_params(response, route_params)
            || !response.body_request_json.is_empty()
            || native_server_response_uses_request_fields(response, route_params))
}

fn native_server_response_condition_is_direct(
    response: &ServerResponseArtifact,
    route_params: &[String],
) -> bool {
    response.condition.as_ref().is_some_and(|condition| {
        native_response_condition_name_is_direct(
            condition.kind.as_str(),
            &condition.name,
            route_params,
        ) && condition.operand_name.as_deref().is_none_or(|name| {
            let operand_kind = condition
                .operand_kind
                .as_deref()
                .unwrap_or_else(|| native_response_condition_operand_kind(condition.kind.as_str()));
            native_response_condition_operand_is_direct(operand_kind, name, route_params)
        }) && matches!(
            condition.kind.as_str(),
            "request_body_field_eq"
                | "request_body_field_ne"
                | "request_body_field_int_eq"
                | "request_body_field_int_ne"
                | "request_body_field_int_lt"
                | "request_body_field_int_le"
                | "request_body_field_int_gt"
                | "request_body_field_int_ge"
                | "request_body_field_float_eq"
                | "request_body_field_float_ne"
                | "request_body_field_float_lt"
                | "request_body_field_float_le"
                | "request_body_field_float_gt"
                | "request_body_field_float_ge"
                | "route_param_eq"
                | "route_param_ne"
                | "route_param_int_eq"
                | "route_param_int_ne"
                | "route_param_int_lt"
                | "route_param_int_le"
                | "route_param_int_gt"
                | "route_param_int_ge"
                | "route_param_float_eq"
                | "route_param_float_ne"
                | "route_param_float_lt"
                | "route_param_float_le"
                | "route_param_float_gt"
                | "route_param_float_ge"
                | "query_param_eq"
                | "query_param_ne"
                | "query_param_int_eq"
                | "query_param_int_ne"
                | "query_param_int_lt"
                | "query_param_int_le"
                | "query_param_int_gt"
                | "query_param_int_ge"
                | "query_param_float_eq"
                | "query_param_float_ne"
                | "query_param_float_lt"
                | "query_param_float_le"
                | "query_param_float_gt"
                | "query_param_float_ge"
        )
    })
}

fn native_response_condition_operand_kind(kind: &str) -> &'static str {
    match kind {
        "route_param_eq" | "route_param_ne" => "route_param",
        "query_param_eq" | "query_param_ne" => "query_param",
        "request_body_field_eq" | "request_body_field_ne" => "request_body_field",
        "route_param_int_eq" | "route_param_int_ne" | "route_param_int_lt"
        | "route_param_int_le" | "route_param_int_gt" | "route_param_int_ge" => "route_param_int",
        "route_param_float_eq"
        | "route_param_float_ne"
        | "route_param_float_lt"
        | "route_param_float_le"
        | "route_param_float_gt"
        | "route_param_float_ge" => "route_param_float",
        "query_param_int_eq" | "query_param_int_ne" | "query_param_int_lt"
        | "query_param_int_le" | "query_param_int_gt" | "query_param_int_ge" => "query_param_int",
        "query_param_float_eq"
        | "query_param_float_ne"
        | "query_param_float_lt"
        | "query_param_float_le"
        | "query_param_float_gt"
        | "query_param_float_ge" => "query_param_float",
        "request_body_field_int_eq"
        | "request_body_field_int_ne"
        | "request_body_field_int_lt"
        | "request_body_field_int_le"
        | "request_body_field_int_gt"
        | "request_body_field_int_ge" => "request_body_field_int",
        "request_body_field_float_eq"
        | "request_body_field_float_ne"
        | "request_body_field_float_lt"
        | "request_body_field_float_le"
        | "request_body_field_float_gt"
        | "request_body_field_float_ge" => "request_body_field_float",
        _ => "",
    }
}

fn native_response_condition_operand_is_direct(
    operand_kind: &str,
    name: &str,
    route_params: &[String],
) -> bool {
    match operand_kind {
        "route_param" | "route_param_int" | "route_param_float" => {
            route_params.iter().any(|param| param == name)
        }
        "query_param"
        | "query_param_int"
        | "query_param_float"
        | "request_body_field"
        | "request_body_field_int"
        | "request_body_field_float" => !name.is_empty(),
        _ => false,
    }
}

fn native_response_condition_name_is_direct(
    kind: &str,
    name: &str,
    route_params: &[String],
) -> bool {
    native_response_condition_operand_is_direct(
        native_response_condition_operand_kind(kind),
        name,
        route_params,
    )
}

fn native_server_response_uses_object_fields(
    response: &ServerResponseArtifact,
    route_params: &[String],
) -> bool {
    !response.body_object_fields.is_empty()
        && response
            .body_object_fields
            .iter()
            .all(|field| match field.value_kind.as_str() {
                "static_json" => field.value_json.is_some(),
                "route_param" | "route_param_int" | "route_param_float" => {
                    field.name.as_ref().is_some_and(|name| {
                        route_params.iter().any(|route_param| route_param == name)
                    }) && native_captured_field_operation_is_direct(
                        &field.value_kind,
                        field.op.as_deref(),
                        field.operand_json.as_deref(),
                        field.operand_kind.as_deref(),
                        field.operand_name.as_deref(),
                        route_params,
                    )
                }
                "query_param"
                | "query_param_int"
                | "query_param_float"
                | "request_body_field"
                | "request_body_field_int"
                | "request_body_field_float" => {
                    field.name.is_some()
                        && native_captured_field_operation_is_direct(
                            &field.value_kind,
                            field.op.as_deref(),
                            field.operand_json.as_deref(),
                            field.operand_kind.as_deref(),
                            field.operand_name.as_deref(),
                            route_params,
                        )
                }
                "request_body_json" => true,
                _ => false,
            })
}

fn native_server_response_uses_route_params(
    response: &ServerResponseArtifact,
    route_params: &[String],
) -> bool {
    !response.body_route_params.is_empty()
        && response.body_route_params.iter().all(|field| {
            matches!(
                field.value_kind.as_str(),
                "route_param" | "route_param_int" | "route_param_float"
            ) && route_params
                .iter()
                .any(|route_param| route_param == &field.param)
                && native_captured_field_operation_is_direct(
                    &field.value_kind,
                    field.op.as_deref(),
                    field.operand_json.as_deref(),
                    field.operand_kind.as_deref(),
                    field.operand_name.as_deref(),
                    route_params,
                )
        })
}

fn native_server_response_uses_query_params(
    response: &ServerResponseArtifact,
    route_params: &[String],
) -> bool {
    !response.body_query_params.is_empty()
        && response.body_query_params.iter().all(|field| {
            matches!(
                field.value_kind.as_str(),
                "query_param" | "query_param_int" | "query_param_float"
            ) && native_captured_field_operation_is_direct(
                &field.value_kind,
                field.op.as_deref(),
                field.operand_json.as_deref(),
                field.operand_kind.as_deref(),
                field.operand_name.as_deref(),
                route_params,
            )
        })
}

fn native_server_response_uses_request_fields(
    response: &ServerResponseArtifact,
    route_params: &[String],
) -> bool {
    !response.body_request_fields.is_empty()
        && response.body_request_fields.iter().all(|field| {
            matches!(
                field.value_kind.as_str(),
                "request_body_field" | "request_body_field_int" | "request_body_field_float"
            ) && native_captured_field_operation_is_direct(
                &field.value_kind,
                field.op.as_deref(),
                field.operand_json.as_deref(),
                field.operand_kind.as_deref(),
                field.operand_name.as_deref(),
                route_params,
            )
        })
}

fn native_captured_field_operation_is_direct(
    value_kind: &str,
    op: Option<&str>,
    operand_json: Option<&str>,
    operand_kind: Option<&str>,
    operand_name: Option<&str>,
    route_params: &[String],
) -> bool {
    let int_kind = matches!(
        value_kind,
        "route_param_int" | "query_param_int" | "request_body_field_int"
    );
    let float_kind = matches!(
        value_kind,
        "route_param_float" | "query_param_float" | "request_body_field_float"
    );
    match (
        int_kind,
        float_kind,
        op,
        operand_json,
        operand_kind,
        operand_name,
    ) {
        (_, _, None, None, None, None) => true,
        (true, _, Some("add" | "sub" | "mul" | "div" | "rem"), Some(operand), None, None) => {
            operand.parse::<i64>().is_ok()
        }
        (true, _, Some("add" | "sub" | "mul" | "div" | "rem"), None, Some(kind), Some(name)) => {
            native_captured_int_operand_is_direct(kind, name, route_params)
        }
        (_, true, Some("add" | "sub" | "mul" | "div" | "rem"), Some(operand), None, None) => {
            operand
                .parse::<f64>()
                .ok()
                .is_some_and(|value| value.is_finite())
        }
        (_, true, Some("add" | "sub" | "mul" | "div" | "rem"), None, Some(kind), Some(name)) => {
            native_captured_float_operand_is_direct(kind, name, route_params)
        }
        _ => false,
    }
}

fn native_captured_int_operand_is_direct(
    operand_kind: &str,
    operand_name: &str,
    route_params: &[String],
) -> bool {
    match operand_kind {
        "route_param_int" => route_params
            .iter()
            .any(|route_param| route_param == operand_name),
        "query_param_int" | "request_body_field_int" => !operand_name.is_empty(),
        _ => false,
    }
}

fn native_captured_float_operand_is_direct(
    operand_kind: &str,
    operand_name: &str,
    route_params: &[String],
) -> bool {
    match operand_kind {
        "route_param_float" => route_params
            .iter()
            .any(|route_param| route_param == operand_name),
        "query_param_float" | "request_body_field_float" => !operand_name.is_empty(),
        _ => false,
    }
}

fn native_value_kind_uses_json_string(value_kind: &str) -> bool {
    matches!(
        value_kind,
        "route_param" | "query_param" | "request_body_field"
    )
}

fn native_server_route_param_names(path: &str) -> Vec<String> {
    path.split('/')
        .filter_map(|segment| {
            segment
                .strip_prefix(':')
                .map(|name| name.strip_suffix('*').unwrap_or(name).to_string())
        })
        .collect()
}

fn native_server_default_port(artifact: &ServerRuntimeArtifact) -> u16 {
    artifact
        .listen
        .as_ref()
        .and_then(|listen| {
            listen
                .port
                .or_else(|| listen.env.as_ref().and_then(|env| env.default_port))
        })
        .unwrap_or(8080)
}

fn native_server_port_env(artifact: &ServerRuntimeArtifact) -> Option<&str> {
    artifact
        .listen
        .as_ref()
        .and_then(|listen| listen.env.as_ref())
        .map(|env| env.variable.as_str())
}

/// Generate Rust source for the planned native server route table.
///
/// This is a codegen input contract, not the final native router. The emitted
/// source records the server runtime artifact route descriptors as typed Rust
/// constants so the generated launcher and future native backend consume the
/// same method/path/origin-id inventory.
#[must_use]
pub fn native_server_routes_source(artifact: &ServerRuntimeArtifact) -> String {
    let mut source = String::from(
        r#"// Generated by orv build. This is a route table source for the planned
// native server runtime.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OrvNativeRoute {
    pub method: &'static str,
    pub path: &'static str,
    pub origin_id: &'static str,
    pub response_origin_ids: &'static [&'static str],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrvNativeRouteMatch {
    pub route: &'static OrvNativeRoute,
    pub params: Vec<OrvNativeParam>,
    pub query: Vec<OrvNativeParam>,
    pub body: String,
    pub body_fields: Vec<OrvNativeParam>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrvNativeParam {
    pub name: String,
    pub value: String,
}

pub const ORV_NATIVE_ROUTES: &[OrvNativeRoute] = &[
"#,
    );
    for route in &artifact.routes {
        let _ = writeln!(
            source,
            "    OrvNativeRoute {{ method: {}, path: {}, origin_id: {}, response_origin_ids: &[{}] }},",
            rust_string_literal(&route.method),
            rust_string_literal(&route.path),
            rust_string_literal(&route.origin_id),
            route
                .response_origin_ids
                .iter()
                .map(|id| rust_string_literal(id))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    source.push_str(
        r#"];

pub const ORV_NATIVE_ROUTE_COUNT: usize = ORV_NATIVE_ROUTES.len();

pub fn orv_native_match_route(method: &str, path: &str) -> Option<OrvNativeRouteMatch> {
    ORV_NATIVE_ROUTES
        .iter()
        .find_map(|route| {
            if route.method != method {
                return None;
            }
            orv_native_route_path_params(route.path, path)
                .map(|params| OrvNativeRouteMatch {
                    route,
                    params,
                    query: Vec::new(),
                    body: String::new(),
                    body_fields: Vec::new(),
                })
        })
}

#[allow(dead_code)]
pub fn orv_native_param_value<'a>(
    route_match: &'a OrvNativeRouteMatch,
    name: &str,
) -> Option<&'a str> {
    route_match
        .params
        .iter()
        .find(|param| param.name == name)
        .map(|param| param.value.as_str())
}

#[allow(dead_code)]
pub fn orv_native_query_value<'a>(
    route_match: &'a OrvNativeRouteMatch,
    name: &str,
) -> Option<&'a str> {
    route_match
        .query
        .iter()
        .find(|param| param.name == name)
        .map(|param| param.value.as_str())
}

#[allow(dead_code)]
pub fn orv_native_body_json(route_match: &OrvNativeRouteMatch) -> Option<&str> {
    let body = route_match.body.trim();
    if body.is_empty() {
        None
    } else {
        Some(body)
    }
}

#[allow(dead_code)]
pub fn orv_native_body_field_value<'a>(
    route_match: &'a OrvNativeRouteMatch,
    name: &str,
) -> Option<&'a str> {
    route_match
        .body_fields
        .iter()
        .find(|param| param.name == name)
        .map(|param| param.value.as_str())
}

fn orv_native_route_path_params(pattern: &'static str, path: &str) -> Option<Vec<OrvNativeParam>> {
    if pattern == "*" {
        return Some(Vec::new());
    }
    let pattern_segments: Vec<&str> = pattern.split('/').collect();
    let path_segments: Vec<&str> = path.split('/').collect();
    if let Some(rest_segment) = pattern_segments.last() {
        if rest_segment
            .strip_prefix(':')
            .and_then(|segment| segment.strip_suffix('*'))
            .is_some()
        {
            let prefix_len = pattern_segments.len() - 1;
            if path_segments.len() <= prefix_len {
                return None;
            }
            let mut params = Vec::new();
            for (pattern_segment, path_segment) in pattern_segments
                .iter()
                .take(prefix_len)
                .zip(path_segments.iter())
            {
                if let Some(name) = pattern_segment.strip_prefix(':') {
                    let name = name.to_string();
                    params.push(OrvNativeParam {
                        name,
                        value: (*path_segment).to_string(),
                    });
                } else if pattern_segment != path_segment {
                    return None;
                }
            }
            let name = rest_segment
                .strip_prefix(':')
                .and_then(|segment| segment.strip_suffix('*'))
                .unwrap_or("")
                .to_string();
            params.push(OrvNativeParam {
                name,
                value: path_segments[prefix_len..].join("/"),
            });
            return Some(params);
        }
    }
    if pattern_segments.len() != path_segments.len() {
        return None;
    }
    let mut params = Vec::new();
    for (pattern_segment, path_segment) in pattern_segments.iter().zip(path_segments.iter()) {
        if let Some(name) = pattern_segment.strip_prefix(':') {
            let name = name.to_string();
            params.push(OrvNativeParam {
                name,
                value: (*path_segment).to_string(),
            });
        } else if pattern_segment != path_segment {
            return None;
        }
    }
    Some(params)
}
"#,
    );
    source
}

/// Generate Rust source for the planned native server router dispatch bridge.
///
/// This source consumes the generated route table and records the fallback
/// dispatch contract used until route handler body codegen exists.
#[must_use]
pub fn native_server_router_source() -> String {
    r#"// Generated by orv build. This is a router dispatch source for the planned
// native server runtime.

use crate::{handlers, routes};

#[allow(dead_code)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrvNativeDispatch {
    pub status: u16,
    pub content_type: &'static str,
    pub body: String,
    pub origin_id: Option<&'static str>,
    pub response_origin_id: Option<&'static str>,
    pub params: Vec<routes::OrvNativeParam>,
}

pub const ORV_NATIVE_HANDLER_COUNT: usize = handlers::ORV_NATIVE_HANDLER_COUNT;

pub fn orv_native_dispatch(method: &str, path: &str) -> OrvNativeDispatch {
    orv_native_dispatch_with_request(method, path, Vec::new(), String::new(), Vec::new())
}

#[allow(dead_code)]
pub fn orv_native_dispatch_with_query(
    method: &str,
    path: &str,
    query: Vec<routes::OrvNativeParam>,
) -> OrvNativeDispatch {
    orv_native_dispatch_with_request(method, path, query, String::new(), Vec::new())
}

pub fn orv_native_dispatch_with_request(
    method: &str,
    path: &str,
    query: Vec<routes::OrvNativeParam>,
    body: String,
    body_fields: Vec<routes::OrvNativeParam>,
) -> OrvNativeDispatch {
    if let Some(mut route_match) = routes::orv_native_match_route(method, path) {
        route_match.query = query;
        route_match.body = body;
        route_match.body_fields = body_fields;
        let response = handlers::orv_native_handle_route(&route_match);
        return OrvNativeDispatch {
            status: response.status,
            content_type: response.content_type,
            body: response.body,
            origin_id: response.origin_id,
            response_origin_id: response.response_origin_id,
            params: response.params,
        };
    }
    OrvNativeDispatch {
        status: 404,
        content_type: "application/json",
        body: "{\"error\":\"not found\"}".to_string(),
        origin_id: None,
        response_origin_id: None,
        params: Vec::new(),
    }
}
"#
    .to_string()
}

/// Generate Rust source for the planned native server route handlers.
///
/// This source is still a placeholder body-lowering contract. It centralizes
/// route-origin/response-origin propagation so future native body codegen can
/// replace the 501 response without changing the router shape.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn native_server_handlers_source(artifact: &ServerRuntimeArtifact) -> String {
    let mut source = String::from(
        r"// Generated by orv build. This is a route handler source for the planned
// native server runtime.

use crate::routes;

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OrvNativeHandlerDescriptor {
    pub method: &'static str,
    pub path: &'static str,
    pub route_origin_id: &'static str,
    pub response_origin_ids: &'static [&'static str],
}

#[allow(dead_code)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OrvNativeHandlerResponse {
    pub status: u16,
    pub content_type: &'static str,
    pub body: String,
    pub origin_id: Option<&'static str>,
    pub response_origin_id: Option<&'static str>,
    pub params: Vec<routes::OrvNativeParam>,
}

#[allow(dead_code)]
pub const ORV_NATIVE_HANDLERS: &[OrvNativeHandlerDescriptor] = &[
",
    );
    for route in &artifact.routes {
        let _ = writeln!(
            source,
            "    OrvNativeHandlerDescriptor {{ method: {}, path: {}, route_origin_id: {}, response_origin_ids: &[{}] }},",
            rust_string_literal(&route.method),
            rust_string_literal(&route.path),
            rust_string_literal(&route.origin_id),
            route
                .response_origin_ids
                .iter()
                .map(|id| rust_string_literal(id))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    source.push_str(
        r"];

pub const ORV_NATIVE_HANDLER_COUNT: usize = routes::ORV_NATIVE_ROUTE_COUNT;

pub fn orv_native_handle_route(
    route_match: &routes::OrvNativeRouteMatch,
) -> OrvNativeHandlerResponse {
",
    );
    let mut native_route_count = 0usize;
    let mut uses_route_param_json = false;
    let mut uses_query_param_json = false;
    let mut uses_request_body_field_json = false;
    for route in &artifact.routes {
        if route.responses.len() > 1 && native_server_route_has_native_response(route) {
            native_route_count += 1;
            let _ = writeln!(
                source,
                "    if route_match.route.origin_id == {} {{",
                rust_string_literal(&route.origin_id)
            );
            for response in &route.responses {
                let Some(status) = response.status else {
                    continue;
                };
                if !(100..=999).contains(&status) {
                    continue;
                }
                if let Some(condition) = response.condition.as_ref() {
                    if push_native_response_condition(&mut source, condition) {
                        let _ = push_native_response_body_return(
                            &mut source,
                            response,
                            status,
                            &mut uses_route_param_json,
                            &mut uses_query_param_json,
                            &mut uses_request_body_field_json,
                        );
                        source.push_str("        }\n");
                    }
                } else {
                    let _ = push_native_response_body_return(
                        &mut source,
                        response,
                        status,
                        &mut uses_route_param_json,
                        &mut uses_query_param_json,
                        &mut uses_request_body_field_json,
                    );
                }
            }
            source.push_str("    }\n");
            continue;
        }
        let Some(response) = route.responses.first() else {
            continue;
        };
        let Some(status) = response.status else {
            continue;
        };
        if !(100..=999).contains(&status) {
            continue;
        }
        if let Some(body_json) = response.body_json.as_ref() {
            native_route_count += 1;
            let _ = writeln!(
                source,
                "    if route_match.route.origin_id == {} {{",
                rust_string_literal(&route.origin_id)
            );
            let body_expr = format!("{}.to_string()", rust_string_literal(body_json));
            push_native_handler_response_return(
                &mut source,
                status,
                &body_expr,
                &response.origin_id,
            );
            source.push_str("    }\n");
            continue;
        }
        if response.body_kind == "empty" {
            native_route_count += 1;
            let _ = writeln!(
                &mut source,
                "    if route_match.route.origin_id == {} {{",
                rust_string_literal(&route.origin_id)
            );
            push_native_handler_response_return(
                &mut source,
                status,
                "String::new()",
                &response.origin_id,
            );
            source.push_str("    }\n");
            continue;
        }
        if !response.body_object_fields.is_empty() {
            native_route_count += 1;
            let _ = writeln!(
                source,
                "    if route_match.route.origin_id == {} {{",
                rust_string_literal(&route.origin_id)
            );
            source.push_str("        let mut body = String::from(\"{\");\n");
            for (index, field) in response.body_object_fields.iter().enumerate() {
                if index > 0 {
                    source.push_str("        body.push(',');\n");
                }
                push_native_json_field_prefix(&mut source, &field.field);
                match field.value_kind.as_str() {
                    "static_json" => {
                        let value_json = field.value_json.as_deref().unwrap_or("null");
                        let _ = writeln!(
                            source,
                            "        body.push_str({});",
                            rust_string_literal(value_json)
                        );
                    }
                    "route_param" | "route_param_int" | "route_param_float" => {
                        uses_route_param_json |=
                            native_value_kind_uses_json_string(&field.value_kind);
                        let name = field.name.as_deref().unwrap_or_default();
                        let _ = push_native_route_param_json_value(
                            &mut source,
                            name,
                            &field.value_kind,
                            native_response_object_field_operation(field),
                            &response.origin_id,
                        );
                    }
                    "query_param" | "query_param_int" | "query_param_float" => {
                        uses_query_param_json |=
                            native_value_kind_uses_json_string(&field.value_kind);
                        let name = field.name.as_deref().unwrap_or_default();
                        let _ = push_native_query_param_json_value(
                            &mut source,
                            name,
                            &field.value_kind,
                            native_response_object_field_operation(field),
                            &response.origin_id,
                        );
                    }
                    "request_body_json" => {
                        source.push_str(
                            "        body.push_str(routes::orv_native_body_json(route_match).unwrap_or(\"null\"));\n",
                        );
                    }
                    "request_body_field" => {
                        uses_request_body_field_json |=
                            native_value_kind_uses_json_string(&field.value_kind);
                        let name = field.name.as_deref().unwrap_or_default();
                        let _ = push_native_request_body_field_json_value(
                            &mut source,
                            name,
                            "request_body_field",
                            native_response_object_field_operation(field),
                            &response.origin_id,
                        );
                    }
                    "request_body_field_int" => {
                        uses_request_body_field_json |=
                            native_value_kind_uses_json_string(&field.value_kind);
                        let name = field.name.as_deref().unwrap_or_default();
                        let _ = push_native_request_body_field_json_value(
                            &mut source,
                            name,
                            "request_body_field_int",
                            native_response_object_field_operation(field),
                            &response.origin_id,
                        );
                    }
                    "request_body_field_float" => {
                        uses_request_body_field_json |=
                            native_value_kind_uses_json_string(&field.value_kind);
                        let name = field.name.as_deref().unwrap_or_default();
                        let _ = push_native_request_body_field_json_value(
                            &mut source,
                            name,
                            "request_body_field_float",
                            native_response_object_field_operation(field),
                            &response.origin_id,
                        );
                    }
                    _ => source.push_str("        body.push_str(\"null\");\n"),
                }
            }
            source.push_str("        body.push('}');\n");
            push_native_handler_response_return(&mut source, status, "body", &response.origin_id);
            source.push_str("    }\n");
            continue;
        }
        if !response.body_query_params.is_empty() {
            native_route_count += 1;
            let _ = writeln!(
                source,
                "    if route_match.route.origin_id == {} {{",
                rust_string_literal(&route.origin_id)
            );
            source.push_str("        let mut body = String::from(\"{\");\n");
            for (index, field) in response.body_query_params.iter().enumerate() {
                if index > 0 {
                    source.push_str("        body.push(',');\n");
                }
                push_native_json_field_prefix(&mut source, &field.field);
                uses_query_param_json |= native_value_kind_uses_json_string(&field.value_kind);
                let _ = push_native_query_param_json_value(
                    &mut source,
                    &field.param,
                    &field.value_kind,
                    native_query_param_operation(field),
                    &response.origin_id,
                );
            }
            source.push_str("        body.push('}');\n");
            push_native_handler_response_return(&mut source, status, "body", &response.origin_id);
            source.push_str("    }\n");
            continue;
        }
        if !response.body_request_json.is_empty() {
            native_route_count += 1;
            let _ = writeln!(
                source,
                "    if route_match.route.origin_id == {} {{",
                rust_string_literal(&route.origin_id)
            );
            source.push_str("        let mut body = String::from(\"{\");\n");
            for (index, field) in response.body_request_json.iter().enumerate() {
                if index > 0 {
                    source.push_str("        body.push(',');\n");
                }
                push_native_json_field_prefix(&mut source, &field.field);
                source.push_str(
                    "        body.push_str(routes::orv_native_body_json(route_match).unwrap_or(\"null\"));\n",
                );
            }
            source.push_str("        body.push('}');\n");
            push_native_handler_response_return(&mut source, status, "body", &response.origin_id);
            source.push_str("    }\n");
            continue;
        }
        if !response.body_request_fields.is_empty() {
            native_route_count += 1;
            let _ = writeln!(
                source,
                "    if route_match.route.origin_id == {} {{",
                rust_string_literal(&route.origin_id)
            );
            source.push_str("        let mut body = String::from(\"{\");\n");
            for (index, field) in response.body_request_fields.iter().enumerate() {
                if index > 0 {
                    source.push_str("        body.push(',');\n");
                }
                push_native_json_field_prefix(&mut source, &field.field);
                uses_request_body_field_json |=
                    native_value_kind_uses_json_string(&field.value_kind);
                let _ = push_native_request_body_field_json_value(
                    &mut source,
                    &field.name,
                    &field.value_kind,
                    native_response_field_operation(field),
                    &response.origin_id,
                );
            }
            source.push_str("        body.push('}');\n");
            push_native_handler_response_return(&mut source, status, "body", &response.origin_id);
            source.push_str("    }\n");
            continue;
        }
        if !response.body_route_params.is_empty() {
            native_route_count += 1;
            let _ = writeln!(
                source,
                "    if route_match.route.origin_id == {} {{",
                rust_string_literal(&route.origin_id)
            );
            source.push_str("        let mut body = String::from(\"{\");\n");
            for (index, field) in response.body_route_params.iter().enumerate() {
                if index > 0 {
                    source.push_str("        body.push(',');\n");
                }
                push_native_json_field_prefix(&mut source, &field.field);
                uses_route_param_json |= native_value_kind_uses_json_string(&field.value_kind);
                let _ = push_native_route_param_json_value(
                    &mut source,
                    &field.param,
                    &field.value_kind,
                    native_route_param_operation(field),
                    &response.origin_id,
                );
            }
            source.push_str("        body.push('}');\n");
            push_native_handler_response_return(&mut source, status, "body", &response.origin_id);
            source.push_str("    }\n");
        }
    }
    if native_route_count == artifact.routes.len() {
        source.push_str(
            r#"    unreachable!("orv native static handler table missing route")
}
"#,
        );
    } else {
        source.push_str(
            r#"    OrvNativeHandlerResponse {
        status: 501,
        content_type: "application/json",
        body: "{\"error\":\"native route body lowering pending\"}".to_string(),
        origin_id: Some(route_match.route.origin_id),
        response_origin_id: route_match.route.response_origin_ids.first().copied(),
        params: route_match.params.clone(),
    }
}
"#,
        );
    }
    if uses_route_param_json || uses_query_param_json || uses_request_body_field_json {
        source.push_str(
            r#"
fn orv_native_push_json_string(value: &str, out: &mut String) {
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            ch if ch <= '\u{1f}' => {
                let _ = std::fmt::Write::write_fmt(out, format_args!("\\u{:04x}", u32::from(ch)));
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
}
"#,
        );
    }
    source
}

fn push_native_json_field_prefix(source: &mut String, field: &str) {
    let field_prefix = format!("\"{}\":", json_escaped(field));
    let _ = writeln!(
        source,
        "        body.push_str({});",
        rust_string_literal(&field_prefix)
    );
}

fn push_native_request_body_field_json_value(
    source: &mut String,
    name: &str,
    value_kind: &str,
    operation: NativeCapturedJsonOperation<'_>,
    response_origin_id: &str,
) -> bool {
    let lookup_expr = format!(
        "routes::orv_native_body_field_value(route_match, {})",
        rust_string_literal(name)
    );
    push_native_captured_json_value(
        source,
        &lookup_expr,
        value_kind,
        operation,
        &NativeCapturedJsonKinds {
            string_kind: "request_body_field",
            int_kind: "request_body_field_int",
            float_kind: "request_body_field_float",
            error_prefix: "native request body",
        },
        response_origin_id,
    )
}

fn push_native_route_param_json_value(
    source: &mut String,
    name: &str,
    value_kind: &str,
    operation: NativeCapturedJsonOperation<'_>,
    response_origin_id: &str,
) -> bool {
    let lookup_expr = format!(
        "routes::orv_native_param_value(route_match, {})",
        rust_string_literal(name)
    );
    push_native_captured_json_value(
        source,
        &lookup_expr,
        value_kind,
        operation,
        &NativeCapturedJsonKinds {
            string_kind: "route_param",
            int_kind: "route_param_int",
            float_kind: "route_param_float",
            error_prefix: "native route param",
        },
        response_origin_id,
    )
}

fn push_native_query_param_json_value(
    source: &mut String,
    name: &str,
    value_kind: &str,
    operation: NativeCapturedJsonOperation<'_>,
    response_origin_id: &str,
) -> bool {
    let lookup_expr = format!(
        "routes::orv_native_query_value(route_match, {})",
        rust_string_literal(name)
    );
    push_native_captured_json_value(
        source,
        &lookup_expr,
        value_kind,
        operation,
        &NativeCapturedJsonKinds {
            string_kind: "query_param",
            int_kind: "query_param_int",
            float_kind: "query_param_float",
            error_prefix: "native query param",
        },
        response_origin_id,
    )
}

struct NativeCapturedJsonKinds<'a> {
    string_kind: &'a str,
    int_kind: &'a str,
    float_kind: &'a str,
    error_prefix: &'a str,
}

#[derive(Clone, Copy)]
struct NativeCapturedJsonOperation<'a> {
    op: Option<&'a str>,
    operand_json: Option<&'a str>,
    operand_kind: Option<&'a str>,
    operand_name: Option<&'a str>,
}

fn native_response_field_operation(
    field: &ServerResponseRequestBodyFieldArtifact,
) -> NativeCapturedJsonOperation<'_> {
    NativeCapturedJsonOperation {
        op: field.op.as_deref(),
        operand_json: field.operand_json.as_deref(),
        operand_kind: field.operand_kind.as_deref(),
        operand_name: field.operand_name.as_deref(),
    }
}

fn native_response_object_field_operation(
    field: &ServerResponseObjectFieldArtifact,
) -> NativeCapturedJsonOperation<'_> {
    NativeCapturedJsonOperation {
        op: field.op.as_deref(),
        operand_json: field.operand_json.as_deref(),
        operand_kind: field.operand_kind.as_deref(),
        operand_name: field.operand_name.as_deref(),
    }
}

fn native_route_param_operation(
    field: &ServerResponseRouteParamArtifact,
) -> NativeCapturedJsonOperation<'_> {
    NativeCapturedJsonOperation {
        op: field.op.as_deref(),
        operand_json: field.operand_json.as_deref(),
        operand_kind: field.operand_kind.as_deref(),
        operand_name: field.operand_name.as_deref(),
    }
}

fn native_query_param_operation(
    field: &ServerResponseQueryParamArtifact,
) -> NativeCapturedJsonOperation<'_> {
    NativeCapturedJsonOperation {
        op: field.op.as_deref(),
        operand_json: field.operand_json.as_deref(),
        operand_kind: field.operand_kind.as_deref(),
        operand_name: field.operand_name.as_deref(),
    }
}

fn push_native_captured_json_value(
    source: &mut String,
    lookup_expr: &str,
    value_kind: &str,
    operation: NativeCapturedJsonOperation<'_>,
    kinds: &NativeCapturedJsonKinds<'_>,
    response_origin_id: &str,
) -> bool {
    match value_kind {
        kind if kind == kinds.string_kind => {
            let _ = writeln!(
                source,
                "        orv_native_push_json_string({lookup_expr}.unwrap_or(\"\"), &mut body);"
            );
            true
        }
        kind if kind == kinds.int_kind => {
            let _ = writeln!(
                source,
                "        match {lookup_expr}.unwrap_or(\"\").trim().parse::<i64>() {{"
            );
            if !push_native_int_success_arm(
                source,
                operation,
                kinds.error_prefix,
                response_origin_id,
            ) {
                return false;
            }
            source.push_str("            Err(_) => {\n");
            let error_body = format!(r#"{{"error":"{} int cast failed"}}"#, kinds.error_prefix);
            let body_expr = format!("{}.to_string()", rust_string_literal(&error_body));
            push_native_handler_response_return(source, 500, &body_expr, response_origin_id);
            source.push_str("            },\n        }\n");
            true
        }
        kind if kind == kinds.float_kind => {
            let _ = writeln!(
                source,
                "        match {lookup_expr}.unwrap_or(\"\").trim().parse::<f64>() {{"
            );
            if !push_native_float_success_arm(
                source,
                operation,
                kinds.error_prefix,
                response_origin_id,
            ) {
                return false;
            }
            source.push_str("            _ => {\n");
            let error_body = format!(r#"{{"error":"{} float cast failed"}}"#, kinds.error_prefix);
            let body_expr = format!("{}.to_string()", rust_string_literal(&error_body));
            push_native_handler_response_return(source, 500, &body_expr, response_origin_id);
            source.push_str("            },\n        }\n");
            true
        }
        _ => false,
    }
}

fn push_native_float_success_arm(
    source: &mut String,
    operation: NativeCapturedJsonOperation<'_>,
    error_prefix: &str,
    response_origin_id: &str,
) -> bool {
    let NativeCapturedJsonOperation {
        op,
        operand_json,
        operand_kind,
        operand_name,
    } = operation;
    match (op, operand_json, operand_kind, operand_name) {
        (None, None, None, None) => {
            source.push_str(
                "            Ok(value) if value.is_finite() => body.push_str(&value.to_string()),\n",
            );
            true
        }
        (Some("add" | "sub" | "mul" | "div" | "rem"), Some(operand_json), None, None) => {
            let Some(operand) = static_float_operand_value(operand_json) else {
                return false;
            };
            let Some(operator) = native_float_arithmetic_operator(op) else {
                return false;
            };
            let _ = writeln!(
                source,
                "            Ok(value) if value.is_finite() => {{\n                let value = value {operator} {operand};"
            );
            push_native_float_arithmetic_result(source, error_prefix, response_origin_id);
            source.push_str("            },\n");
            true
        }
        (
            Some("add" | "sub" | "mul" | "div" | "rem"),
            None,
            Some(
                operand_kind @ ("route_param_float"
                | "query_param_float"
                | "request_body_field_float"),
            ),
            Some(operand_name),
        ) => {
            let Some(operator) = native_float_arithmetic_operator(op) else {
                return false;
            };
            let Some(operand_lookup) = native_float_operand_lookup_expr(operand_kind, operand_name)
            else {
                return false;
            };
            let _ = writeln!(
                source,
                "            Ok(value) if value.is_finite() => match {operand_lookup}.unwrap_or(\"\").trim().parse::<f64>() {{"
            );
            let _ = writeln!(
                source,
                "                Ok(operand) if operand.is_finite() => {{\n                    let value = value {operator} operand;"
            );
            push_native_float_arithmetic_result(source, error_prefix, response_origin_id);
            source.push_str("                },\n                _ => {\n");
            let error_body = format!(
                r#"{{"error":"{} float operand cast failed"}}"#,
                error_prefix
            );
            let body_expr = format!("{}.to_string()", rust_string_literal(&error_body));
            push_native_handler_response_return(source, 500, &body_expr, response_origin_id);
            source.push_str("                },\n            },\n");
            true
        }
        _ => false,
    }
}

fn push_native_float_arithmetic_result(
    source: &mut String,
    error_prefix: &str,
    response_origin_id: &str,
) {
    source.push_str(
        "                if value.is_finite() {\n                    body.push_str(&value.to_string());\n                } else {\n",
    );
    let error_body = format!(r#"{{"error":"{} float arithmetic failed"}}"#, error_prefix);
    let body_expr = format!("{}.to_string()", rust_string_literal(&error_body));
    push_native_handler_response_return(source, 500, &body_expr, response_origin_id);
    source.push_str("                }\n");
}

fn native_float_arithmetic_operator(op: Option<&str>) -> Option<&'static str> {
    match op {
        Some("add") => Some("+"),
        Some("sub") => Some("-"),
        Some("mul") => Some("*"),
        Some("div") => Some("/"),
        Some("rem") => Some("%"),
        _ => None,
    }
}

fn static_float_operand_value(value: &str) -> Option<&str> {
    value
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
        .map(|_| value)
}

fn push_native_int_success_arm(
    source: &mut String,
    operation: NativeCapturedJsonOperation<'_>,
    error_prefix: &str,
    response_origin_id: &str,
) -> bool {
    let NativeCapturedJsonOperation {
        op,
        operand_json,
        operand_kind,
        operand_name,
    } = operation;
    match (op, operand_json, operand_kind, operand_name) {
        (None, None, None, None) => {
            source.push_str("            Ok(value) => body.push_str(&value.to_string()),\n");
            true
        }
        (Some("add" | "sub" | "mul" | "div" | "rem"), Some(operand_json), None, None) => {
            let Some(operand) = operand_json.parse::<i64>().ok() else {
                return false;
            };
            let method = match op {
                Some("add") => "checked_add",
                Some("sub") => "checked_sub",
                Some("mul") => "checked_mul",
                Some("div") => "checked_div",
                Some("rem") => "checked_rem",
                _ => return false,
            };
            let _ = writeln!(
                source,
                "            Ok(value) => match value.{method}({operand}) {{"
            );
            source.push_str(
                "                Some(value) => body.push_str(&value.to_string()),\n                None => {\n",
            );
            let error_body = format!(r#"{{"error":"{} int arithmetic failed"}}"#, error_prefix);
            let body_expr = format!("{}.to_string()", rust_string_literal(&error_body));
            push_native_handler_response_return(source, 500, &body_expr, response_origin_id);
            source.push_str("                },\n            },\n");
            true
        }
        (
            Some("add" | "sub" | "mul" | "div" | "rem"),
            None,
            Some(operand_kind @ ("route_param_int" | "query_param_int" | "request_body_field_int")),
            Some(operand_name),
        ) => {
            let method = match op {
                Some("add") => "checked_add",
                Some("sub") => "checked_sub",
                Some("mul") => "checked_mul",
                Some("div") => "checked_div",
                Some("rem") => "checked_rem",
                _ => return false,
            };
            let Some(operand_lookup) = native_int_operand_lookup_expr(operand_kind, operand_name)
            else {
                return false;
            };
            let _ = writeln!(
                source,
                "            Ok(value) => match {operand_lookup}.unwrap_or(\"\").trim().parse::<i64>() {{"
            );
            let _ = writeln!(
                source,
                "                Ok(operand) => match value.{method}(operand) {{"
            );
            source.push_str(
                "                    Some(value) => body.push_str(&value.to_string()),\n                    None => {\n",
            );
            let error_body = format!(r#"{{"error":"{} int arithmetic failed"}}"#, error_prefix);
            let body_expr = format!("{}.to_string()", rust_string_literal(&error_body));
            push_native_handler_response_return(source, 500, &body_expr, response_origin_id);
            source.push_str(
                "                    },\n                },\n                Err(_) => {\n",
            );
            let error_body = format!(r#"{{"error":"{} int operand cast failed"}}"#, error_prefix);
            let body_expr = format!("{}.to_string()", rust_string_literal(&error_body));
            push_native_handler_response_return(source, 500, &body_expr, response_origin_id);
            source.push_str("                },\n            },\n");
            true
        }
        _ => false,
    }
}

fn native_int_operand_lookup_expr(operand_kind: &str, operand_name: &str) -> Option<String> {
    let lookup = match operand_kind {
        "route_param_int" => "routes::orv_native_param_value",
        "query_param_int" => "routes::orv_native_query_value",
        "request_body_field_int" => "routes::orv_native_body_field_value",
        _ => return None,
    };
    Some(format!(
        "{lookup}(route_match, {})",
        rust_string_literal(operand_name)
    ))
}

fn native_float_operand_lookup_expr(operand_kind: &str, operand_name: &str) -> Option<String> {
    let lookup = match operand_kind {
        "route_param_float" => "routes::orv_native_param_value",
        "query_param_float" => "routes::orv_native_query_value",
        "request_body_field_float" => "routes::orv_native_body_field_value",
        _ => return None,
    };
    Some(format!(
        "{lookup}(route_match, {})",
        rust_string_literal(operand_name)
    ))
}

fn push_native_handler_response_return(
    source: &mut String,
    status: i64,
    body_expr: &str,
    response_origin_id: &str,
) {
    let _ = writeln!(
        source,
        r#"        return OrvNativeHandlerResponse {{
            status: {status},
            content_type: "application/json",
            body: {body_expr},
            origin_id: Some(route_match.route.origin_id),
            response_origin_id: Some({}),
            params: route_match.params.clone(),
        }};
        "#,
        rust_string_literal(response_origin_id)
    );
}

fn push_native_response_condition(
    source: &mut String,
    condition: &ServerResponseConditionArtifact,
) -> bool {
    if push_native_float_response_condition(source, condition) {
        return true;
    }
    if push_native_int_response_condition(source, condition) {
        return true;
    }
    let Some((lookup, operator)) = native_response_condition_lookup(condition.kind.as_str()) else {
        return false;
    };
    if let Some(operand_name) = condition.operand_name.as_deref() {
        let operand_lookup = condition
            .operand_kind
            .as_deref()
            .and_then(native_response_condition_operand_lookup)
            .unwrap_or(lookup);
        let _ = writeln!(
            source,
            "        if {lookup}(route_match, {}) {operator} {operand_lookup}(route_match, {}) {{",
            rust_string_literal(&condition.name),
            rust_string_literal(operand_name)
        );
    } else {
        let _ = writeln!(
            source,
            "        if {lookup}(route_match, {}) {operator} Some({}) {{",
            rust_string_literal(&condition.name),
            rust_string_literal(&condition.value)
        );
    }
    true
}

fn push_native_float_response_condition(
    source: &mut String,
    condition: &ServerResponseConditionArtifact,
) -> bool {
    let Some((lookup, operator)) = native_response_condition_float_lookup(condition.kind.as_str())
    else {
        return false;
    };
    if condition.operand_name.is_some() {
        let Some(operand_lookup) = condition
            .operand_kind
            .as_deref()
            .and_then(native_response_condition_float_operand_lookup)
        else {
            return false;
        };
        let operand_name = condition.operand_name.as_deref().unwrap_or_default();
        let _ = writeln!(
            source,
            "        if match ({lookup}(route_match, {}).unwrap_or(\"\").trim().parse::<f64>(), {operand_lookup}(route_match, {}).unwrap_or(\"\").trim().parse::<f64>()) {{",
            rust_string_literal(&condition.name),
            rust_string_literal(operand_name)
        );
        let _ = writeln!(
            source,
            "            (Ok(value), Ok(operand)) if value.is_finite() && operand.is_finite() => value {operator} operand,"
        );
        source.push_str("            _ => false,\n        } {\n");
        return true;
    }
    let Ok(value) = condition.value.parse::<f64>() else {
        return false;
    };
    if !value.is_finite() {
        return false;
    }
    let _ = writeln!(
        source,
        "        if match {lookup}(route_match, {}).unwrap_or(\"\").trim().parse::<f64>() {{",
        rust_string_literal(&condition.name)
    );
    let _ = writeln!(
        source,
        "            Ok(value) if value.is_finite() => value {operator} {},",
        condition.value
    );
    source.push_str("            _ => false,\n        } {\n");
    true
}

fn native_response_condition_float_lookup(kind: &str) -> Option<(&'static str, &'static str)> {
    let lookup = match kind {
        "request_body_field_float_eq"
        | "request_body_field_float_ne"
        | "request_body_field_float_lt"
        | "request_body_field_float_le"
        | "request_body_field_float_gt"
        | "request_body_field_float_ge" => "routes::orv_native_body_field_value",
        "route_param_float_eq"
        | "route_param_float_ne"
        | "route_param_float_lt"
        | "route_param_float_le"
        | "route_param_float_gt"
        | "route_param_float_ge" => "routes::orv_native_param_value",
        "query_param_float_eq"
        | "query_param_float_ne"
        | "query_param_float_lt"
        | "query_param_float_le"
        | "query_param_float_gt"
        | "query_param_float_ge" => "routes::orv_native_query_value",
        _ => return None,
    };
    let operator = match kind {
        "request_body_field_float_eq" | "route_param_float_eq" | "query_param_float_eq" => "==",
        "request_body_field_float_ne" | "route_param_float_ne" | "query_param_float_ne" => "!=",
        "request_body_field_float_lt" | "route_param_float_lt" | "query_param_float_lt" => "<",
        "request_body_field_float_le" | "route_param_float_le" | "query_param_float_le" => "<=",
        "request_body_field_float_gt" | "route_param_float_gt" | "query_param_float_gt" => ">",
        "request_body_field_float_ge" | "route_param_float_ge" | "query_param_float_ge" => ">=",
        _ => return None,
    };
    Some((lookup, operator))
}

fn native_response_condition_float_operand_lookup(operand_kind: &str) -> Option<&'static str> {
    match operand_kind {
        "request_body_field_float" => Some("routes::orv_native_body_field_value"),
        "route_param_float" => Some("routes::orv_native_param_value"),
        "query_param_float" => Some("routes::orv_native_query_value"),
        _ => None,
    }
}

fn push_native_int_response_condition(
    source: &mut String,
    condition: &ServerResponseConditionArtifact,
) -> bool {
    let Some((lookup, operator)) = native_response_condition_int_lookup(condition.kind.as_str())
    else {
        return false;
    };
    if condition.operand_name.is_some() {
        let Some(operand_lookup) = condition
            .operand_kind
            .as_deref()
            .and_then(native_response_condition_int_operand_lookup)
        else {
            return false;
        };
        let operand_name = condition.operand_name.as_deref().unwrap_or_default();
        let _ = writeln!(
            source,
            "        if match ({lookup}(route_match, {}).unwrap_or(\"\").trim().parse::<i64>(), {operand_lookup}(route_match, {}).unwrap_or(\"\").trim().parse::<i64>()) {{",
            rust_string_literal(&condition.name),
            rust_string_literal(operand_name)
        );
        let _ = writeln!(
            source,
            "            (Ok(value), Ok(operand)) => value {operator} operand,"
        );
        source.push_str("            _ => false,\n        } {\n");
        return true;
    }
    let Ok(value) = condition.value.parse::<i64>() else {
        return false;
    };
    let _ = writeln!(
        source,
        "        if match {lookup}(route_match, {}).unwrap_or(\"\").trim().parse::<i64>() {{",
        rust_string_literal(&condition.name)
    );
    source.push_str("            Ok(value) => {\n");
    let _ = writeln!(
        source,
        "                if value {operator} {value} {{ true }} else {{ false }}"
    );
    source.push_str("            }\n            Err(_) => false,\n        } {\n");
    true
}

fn native_response_condition_int_lookup(kind: &str) -> Option<(&'static str, &'static str)> {
    let lookup = match kind {
        "request_body_field_int_eq"
        | "request_body_field_int_ne"
        | "request_body_field_int_lt"
        | "request_body_field_int_le"
        | "request_body_field_int_gt"
        | "request_body_field_int_ge" => "routes::orv_native_body_field_value",
        "route_param_int_eq" | "route_param_int_ne" | "route_param_int_lt"
        | "route_param_int_le" | "route_param_int_gt" | "route_param_int_ge" => {
            "routes::orv_native_param_value"
        }
        "query_param_int_eq" | "query_param_int_ne" | "query_param_int_lt"
        | "query_param_int_le" | "query_param_int_gt" | "query_param_int_ge" => {
            "routes::orv_native_query_value"
        }
        _ => return None,
    };
    let operator = match kind {
        "request_body_field_int_eq" | "route_param_int_eq" | "query_param_int_eq" => "==",
        "request_body_field_int_ne" | "route_param_int_ne" | "query_param_int_ne" => "!=",
        "request_body_field_int_lt" | "route_param_int_lt" | "query_param_int_lt" => "<",
        "request_body_field_int_le" | "route_param_int_le" | "query_param_int_le" => "<=",
        "request_body_field_int_gt" | "route_param_int_gt" | "query_param_int_gt" => ">",
        "request_body_field_int_ge" | "route_param_int_ge" | "query_param_int_ge" => ">=",
        _ => return None,
    };
    Some((lookup, operator))
}

fn native_response_condition_int_operand_lookup(operand_kind: &str) -> Option<&'static str> {
    match operand_kind {
        "request_body_field_int" => Some("routes::orv_native_body_field_value"),
        "route_param_int" => Some("routes::orv_native_param_value"),
        "query_param_int" => Some("routes::orv_native_query_value"),
        _ => None,
    }
}

fn native_response_condition_operand_lookup(operand_kind: &str) -> Option<&'static str> {
    match operand_kind {
        "request_body_field" => Some("routes::orv_native_body_field_value"),
        "route_param" => Some("routes::orv_native_param_value"),
        "query_param" => Some("routes::orv_native_query_value"),
        _ => None,
    }
}

fn native_response_condition_lookup(kind: &str) -> Option<(&'static str, &'static str)> {
    match kind {
        "request_body_field_eq" => Some(("routes::orv_native_body_field_value", "==")),
        "request_body_field_ne" => Some(("routes::orv_native_body_field_value", "!=")),
        "route_param_eq" => Some(("routes::orv_native_param_value", "==")),
        "route_param_ne" => Some(("routes::orv_native_param_value", "!=")),
        "query_param_eq" => Some(("routes::orv_native_query_value", "==")),
        "query_param_ne" => Some(("routes::orv_native_query_value", "!=")),
        _ => None,
    }
}

fn push_native_response_body_return(
    source: &mut String,
    response: &ServerResponseArtifact,
    status: i64,
    uses_route_param_json: &mut bool,
    uses_query_param_json: &mut bool,
    uses_request_body_field_json: &mut bool,
) -> bool {
    if let Some(body_json) = response.body_json.as_ref() {
        let body_expr = format!("{}.to_string()", rust_string_literal(body_json));
        push_native_handler_response_return(source, status, &body_expr, &response.origin_id);
        return true;
    }
    if response.body_kind == "empty" {
        push_native_handler_response_return(source, status, "String::new()", &response.origin_id);
        return true;
    }
    if !response.body_object_fields.is_empty() {
        if !push_native_object_fields_body(
            source,
            response,
            uses_route_param_json,
            uses_query_param_json,
            uses_request_body_field_json,
        ) {
            return false;
        }
        push_native_handler_response_return(source, status, "body", &response.origin_id);
        return true;
    }
    if !response.body_query_params.is_empty() {
        push_native_query_params_body(source, response, uses_query_param_json);
        push_native_handler_response_return(source, status, "body", &response.origin_id);
        return true;
    }
    if !response.body_request_json.is_empty() {
        push_native_request_json_body(source, response);
        push_native_handler_response_return(source, status, "body", &response.origin_id);
        return true;
    }
    if !response.body_request_fields.is_empty() {
        push_native_request_fields_body(source, response, uses_request_body_field_json);
        push_native_handler_response_return(source, status, "body", &response.origin_id);
        return true;
    }
    if !response.body_route_params.is_empty() {
        push_native_route_params_body(source, response, uses_route_param_json);
        push_native_handler_response_return(source, status, "body", &response.origin_id);
        return true;
    }
    false
}

fn push_native_object_fields_body(
    source: &mut String,
    response: &ServerResponseArtifact,
    uses_route_param_json: &mut bool,
    uses_query_param_json: &mut bool,
    uses_request_body_field_json: &mut bool,
) -> bool {
    source.push_str("        let mut body = String::from(\"{\");\n");
    for (index, field) in response.body_object_fields.iter().enumerate() {
        if index > 0 {
            source.push_str("        body.push(',');\n");
        }
        push_native_json_field_prefix(source, &field.field);
        match field.value_kind.as_str() {
            "static_json" => push_native_static_json_value(source, field),
            "route_param" | "route_param_int" | "route_param_float" => {
                *uses_route_param_json |= native_value_kind_uses_json_string(&field.value_kind);
                let name = field.name.as_deref().unwrap_or_default();
                let _ = push_native_route_param_json_value(
                    source,
                    name,
                    &field.value_kind,
                    native_response_object_field_operation(field),
                    &response.origin_id,
                );
            }
            "query_param" | "query_param_int" | "query_param_float" => {
                *uses_query_param_json |= native_value_kind_uses_json_string(&field.value_kind);
                let name = field.name.as_deref().unwrap_or_default();
                let _ = push_native_query_param_json_value(
                    source,
                    name,
                    &field.value_kind,
                    native_response_object_field_operation(field),
                    &response.origin_id,
                );
            }
            "request_body_json" => {
                source.push_str(
                    "        body.push_str(routes::orv_native_body_json(route_match).unwrap_or(\"null\"));\n",
                );
            }
            "request_body_field" | "request_body_field_int" | "request_body_field_float" => {
                *uses_request_body_field_json |=
                    native_value_kind_uses_json_string(&field.value_kind);
                let name = field.name.as_deref().unwrap_or_default();
                let _ = push_native_request_body_field_json_value(
                    source,
                    name,
                    &field.value_kind,
                    native_response_object_field_operation(field),
                    &response.origin_id,
                );
            }
            _ => return false,
        }
    }
    source.push_str("        body.push('}');\n");
    true
}

fn push_native_static_json_value(source: &mut String, field: &ServerResponseObjectFieldArtifact) {
    let value_json = field.value_json.as_deref().unwrap_or("null");
    let _ = writeln!(
        source,
        "        body.push_str({});",
        rust_string_literal(value_json)
    );
}

fn push_native_query_params_body(
    source: &mut String,
    response: &ServerResponseArtifact,
    uses_query_param_json: &mut bool,
) {
    source.push_str("        let mut body = String::from(\"{\");\n");
    for (index, field) in response.body_query_params.iter().enumerate() {
        if index > 0 {
            source.push_str("        body.push(',');\n");
        }
        push_native_json_field_prefix(source, &field.field);
        *uses_query_param_json |= native_value_kind_uses_json_string(&field.value_kind);
        let _ = push_native_query_param_json_value(
            source,
            &field.param,
            &field.value_kind,
            native_query_param_operation(field),
            &response.origin_id,
        );
    }
    source.push_str("        body.push('}');\n");
}

fn push_native_request_json_body(source: &mut String, response: &ServerResponseArtifact) {
    source.push_str("        let mut body = String::from(\"{\");\n");
    for (index, field) in response.body_request_json.iter().enumerate() {
        if index > 0 {
            source.push_str("        body.push(',');\n");
        }
        push_native_json_field_prefix(source, &field.field);
        source.push_str(
            "        body.push_str(routes::orv_native_body_json(route_match).unwrap_or(\"null\"));\n",
        );
    }
    source.push_str("        body.push('}');\n");
}

fn push_native_request_fields_body(
    source: &mut String,
    response: &ServerResponseArtifact,
    uses_request_body_field_json: &mut bool,
) {
    source.push_str("        let mut body = String::from(\"{\");\n");
    for (index, field) in response.body_request_fields.iter().enumerate() {
        if index > 0 {
            source.push_str("        body.push(',');\n");
        }
        push_native_json_field_prefix(source, &field.field);
        *uses_request_body_field_json |= native_value_kind_uses_json_string(&field.value_kind);
        let _ = push_native_request_body_field_json_value(
            source,
            &field.name,
            &field.value_kind,
            native_response_field_operation(field),
            &response.origin_id,
        );
    }
    source.push_str("        body.push('}');\n");
}

fn push_native_route_params_body(
    source: &mut String,
    response: &ServerResponseArtifact,
    uses_route_param_json: &mut bool,
) {
    source.push_str("        let mut body = String::from(\"{\");\n");
    for (index, field) in response.body_route_params.iter().enumerate() {
        if index > 0 {
            source.push_str("        body.push(',');\n");
        }
        push_native_json_field_prefix(source, &field.field);
        *uses_route_param_json |= native_value_kind_uses_json_string(&field.value_kind);
        let _ = push_native_route_param_json_value(
            source,
            &field.param,
            &field.value_kind,
            native_route_param_operation(field),
            &response.origin_id,
        );
    }
    source.push_str("        body.push('}');\n");
}

fn rust_string_literal(value: &str) -> String {
    let mut out = String::from("\"");
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if ch.is_control() => {
                let _ = write!(out, "\\u{{{:x}}}", u32::from(ch));
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

fn static_integer(expr: &HirExpr) -> Option<i64> {
    match &expr.kind {
        HirExprKind::Integer(value) => value.parse::<i64>().ok(),
        HirExprKind::Unary {
            op: UnaryOp::Neg,
            expr,
        } => static_integer(expr).map(|value| -value),
        HirExprKind::Paren(expr) => static_integer(expr),
        _ => None,
    }
}

fn static_float(expr: &HirExpr) -> Option<String> {
    match &expr.kind {
        HirExprKind::Integer(value) | HirExprKind::Float(value) => value
            .parse::<f64>()
            .ok()
            .filter(|value| value.is_finite())
            .map(|_| value.clone()),
        HirExprKind::Unary {
            op: UnaryOp::Neg,
            expr,
        } => static_float(expr)
            .and_then(|value| (!value.starts_with('-')).then(|| format!("-{value}"))),
        HirExprKind::Paren(expr) => static_float(expr),
        _ => None,
    }
}

fn static_json_payload(expr: &HirExpr) -> Option<String> {
    match &expr.kind {
        HirExprKind::Integer(value) => value.parse::<i64>().ok().map(|value| value.to_string()),
        HirExprKind::Float(value) => value.parse::<f64>().ok().map(|_| value.clone()),
        HirExprKind::String(segments) => static_string_segments(segments).map(|value| {
            let mut out = String::new();
            write_json_string(&value, &mut out);
            out
        }),
        HirExprKind::True => Some("true".to_string()),
        HirExprKind::False => Some("false".to_string()),
        HirExprKind::Void => Some("null".to_string()),
        HirExprKind::Unary {
            op: UnaryOp::Neg,
            expr,
        } => static_json_payload(expr).and_then(|value| {
            if value.starts_with('-')
                || !(value.bytes().all(|byte| byte.is_ascii_digit()) || value.contains('.'))
            {
                None
            } else {
                Some(format!("-{value}"))
            }
        }),
        HirExprKind::Paren(expr) => static_json_payload(expr),
        HirExprKind::Array(items) | HirExprKind::Tuple(items) => {
            let mut out = String::from("[");
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&static_json_payload(item)?);
            }
            out.push(']');
            Some(out)
        }
        HirExprKind::Object(fields) | HirExprKind::TypedObject { fields, .. } => {
            let mut out = String::from("{");
            for (index, field) in fields.iter().enumerate() {
                if field.is_spread {
                    return None;
                }
                if index > 0 {
                    out.push(',');
                }
                write_json_string(&field.name, &mut out);
                out.push(':');
                out.push_str(&static_json_payload(&field.value)?);
            }
            out.push('}');
            Some(out)
        }
        _ => None,
    }
}

fn response_status_disallows_body(status: i64) -> bool {
    (100..=199).contains(&status) || status == 204 || status == 304
}

fn response_payload_is_void(expr: &HirExpr) -> bool {
    match &expr.kind {
        HirExprKind::Void => true,
        HirExprKind::Paren(expr) => response_payload_is_void(expr),
        _ => false,
    }
}

fn server_response_artifact(
    expr: &HirExpr,
    status: &HirExpr,
    payload: &HirExpr,
    condition: Option<ServerResponseConditionArtifact>,
) -> ServerResponseArtifact {
    let status_value = static_integer(status);
    let static_payload_json = static_json_payload(payload);
    let body_empty = response_payload_is_void(payload)
        || (status_value.is_some_and(response_status_disallows_body)
            && static_payload_json.is_some());
    let body_json = (!body_empty).then_some(static_payload_json).flatten();
    let body_object_fields = if body_json.is_none() && !body_empty {
        mixed_object_json_payload(payload).unwrap_or_default()
    } else {
        Vec::new()
    };
    let body_route_params = if body_json.is_none() && body_object_fields.is_empty() && !body_empty {
        route_param_json_payload(payload).unwrap_or_default()
    } else {
        Vec::new()
    };
    let body_query_params = if body_json.is_none()
        && body_object_fields.is_empty()
        && body_route_params.is_empty()
        && !body_empty
    {
        query_param_json_payload(payload).unwrap_or_default()
    } else {
        Vec::new()
    };
    let body_request_json = if body_json.is_none()
        && body_object_fields.is_empty()
        && body_route_params.is_empty()
        && body_query_params.is_empty()
        && !body_empty
    {
        request_body_json_payload(payload).unwrap_or_default()
    } else {
        Vec::new()
    };
    let body_request_fields = if body_json.is_none()
        && body_object_fields.is_empty()
        && body_route_params.is_empty()
        && body_query_params.is_empty()
        && body_request_json.is_empty()
        && !body_empty
    {
        request_body_field_json_payload(payload).unwrap_or_default()
    } else {
        Vec::new()
    };
    let body_kind = if body_empty {
        "empty"
    } else if body_json.is_some() {
        "static_json"
    } else if !body_object_fields.is_empty() {
        "mixed_json"
    } else if !body_route_params.is_empty() {
        "route_param_json"
    } else if !body_query_params.is_empty() {
        "query_param_json"
    } else if !body_request_json.is_empty() {
        "request_body_json"
    } else if !body_request_fields.is_empty() {
        "request_body_field_json"
    } else {
        "dynamic"
    };
    ServerResponseArtifact {
        origin_id: origin_id("domain", "respond", expr.span),
        status: status_value,
        body_kind: body_kind.to_string(),
        condition,
        body_json,
        body_object_fields,
        body_route_params,
        body_query_params,
        body_request_json,
        body_request_fields,
    }
}

fn route_param_json_payload(expr: &HirExpr) -> Option<Vec<ServerResponseRouteParamArtifact>> {
    match &expr.kind {
        HirExprKind::Object(fields) | HirExprKind::TypedObject { fields, .. } => {
            let mut out = Vec::new();
            for field in fields {
                if field.is_spread {
                    return None;
                }
                let value = route_param_field_value(&field.value)?;
                out.push(ServerResponseRouteParamArtifact {
                    field: field.name.clone(),
                    param: value.name,
                    value_kind: value.value_kind,
                    op: value.op,
                    operand_json: value.operand_json,
                    operand_kind: value.operand_kind,
                    operand_name: value.operand_name,
                });
            }
            Some(out)
        }
        HirExprKind::Paren(expr) => route_param_json_payload(expr),
        _ => None,
    }
}

fn query_param_json_payload(expr: &HirExpr) -> Option<Vec<ServerResponseQueryParamArtifact>> {
    match &expr.kind {
        HirExprKind::Object(fields) | HirExprKind::TypedObject { fields, .. } => {
            let mut out = Vec::new();
            for field in fields {
                if field.is_spread {
                    return None;
                }
                let value = query_param_field_value(&field.value)?;
                out.push(ServerResponseQueryParamArtifact {
                    field: field.name.clone(),
                    param: value.name,
                    value_kind: value.value_kind,
                    op: value.op,
                    operand_json: value.operand_json,
                    operand_kind: value.operand_kind,
                    operand_name: value.operand_name,
                });
            }
            Some(out)
        }
        HirExprKind::Paren(expr) => query_param_json_payload(expr),
        _ => None,
    }
}

fn request_body_json_payload(expr: &HirExpr) -> Option<Vec<ServerResponseRequestBodyArtifact>> {
    match &expr.kind {
        HirExprKind::Object(fields) | HirExprKind::TypedObject { fields, .. } => {
            let mut out = Vec::new();
            for field in fields {
                if field.is_spread || !is_request_body_domain(&field.value) {
                    return None;
                }
                out.push(ServerResponseRequestBodyArtifact {
                    field: field.name.clone(),
                });
            }
            Some(out)
        }
        HirExprKind::Paren(expr) => request_body_json_payload(expr),
        _ => None,
    }
}

fn request_body_field_json_payload(
    expr: &HirExpr,
) -> Option<Vec<ServerResponseRequestBodyFieldArtifact>> {
    match &expr.kind {
        HirExprKind::Object(fields) | HirExprKind::TypedObject { fields, .. } => {
            let mut out = Vec::new();
            for field in fields {
                if field.is_spread {
                    return None;
                }
                let value = request_body_field_value(&field.value)?;
                out.push(ServerResponseRequestBodyFieldArtifact {
                    field: field.name.clone(),
                    name: value.name,
                    value_kind: value.value_kind,
                    op: value.op,
                    operand_json: value.operand_json,
                    operand_kind: value.operand_kind,
                    operand_name: value.operand_name,
                });
            }
            Some(out)
        }
        HirExprKind::Paren(expr) => request_body_field_json_payload(expr),
        _ => None,
    }
}

fn mixed_object_json_payload(expr: &HirExpr) -> Option<Vec<ServerResponseObjectFieldArtifact>> {
    match &expr.kind {
        HirExprKind::Object(fields) | HirExprKind::TypedObject { fields, .. } => {
            let mut out = Vec::new();
            let mut has_static = false;
            let mut has_dynamic = false;
            for field in fields {
                if field.is_spread {
                    return None;
                }
                out.push(mixed_response_object_field(
                    field,
                    &mut has_static,
                    &mut has_dynamic,
                )?);
            }
            (has_static && has_dynamic).then_some(out)
        }
        HirExprKind::Paren(expr) => mixed_object_json_payload(expr),
        _ => None,
    }
}

fn mixed_response_object_field(
    field: &HirObjectField,
    has_static: &mut bool,
    has_dynamic: &mut bool,
) -> Option<ServerResponseObjectFieldArtifact> {
    if let Some(value_json) = static_json_payload(&field.value) {
        *has_static = true;
        return Some(ServerResponseObjectFieldArtifact {
            field: field.name.clone(),
            value_kind: "static_json".to_string(),
            value_json: Some(value_json),
            name: None,
            op: None,
            operand_json: None,
            operand_kind: None,
            operand_name: None,
        });
    }
    if let Some(value) = route_param_field_value(&field.value) {
        *has_dynamic = true;
        return Some(ServerResponseObjectFieldArtifact {
            field: field.name.clone(),
            value_kind: value.value_kind,
            value_json: None,
            name: Some(value.name),
            op: value.op,
            operand_json: value.operand_json,
            operand_kind: value.operand_kind,
            operand_name: value.operand_name,
        });
    }
    if let Some(value) = query_param_field_value(&field.value) {
        *has_dynamic = true;
        return Some(ServerResponseObjectFieldArtifact {
            field: field.name.clone(),
            value_kind: value.value_kind,
            value_json: None,
            name: Some(value.name),
            op: value.op,
            operand_json: value.operand_json,
            operand_kind: value.operand_kind,
            operand_name: value.operand_name,
        });
    }
    if is_request_body_domain(&field.value) {
        *has_dynamic = true;
        return Some(ServerResponseObjectFieldArtifact {
            field: field.name.clone(),
            value_kind: "request_body_json".to_string(),
            value_json: None,
            name: None,
            op: None,
            operand_json: None,
            operand_kind: None,
            operand_name: None,
        });
    }
    if let Some(value) = request_body_field_value(&field.value) {
        *has_dynamic = true;
        return Some(ServerResponseObjectFieldArtifact {
            field: field.name.clone(),
            value_kind: value.value_kind,
            value_json: None,
            name: Some(value.name),
            op: value.op,
            operand_json: value.operand_json,
            operand_kind: value.operand_kind,
            operand_name: value.operand_name,
        });
    }
    None
}

fn route_param_field_name(expr: &HirExpr) -> Option<String> {
    match &expr.kind {
        HirExprKind::Field { target, field, .. } if is_route_param_domain(target) => {
            Some(field.clone())
        }
        HirExprKind::Paren(expr) => route_param_field_name(expr),
        _ => None,
    }
}

fn query_param_field_name(expr: &HirExpr) -> Option<String> {
    match &expr.kind {
        HirExprKind::Field { target, field, .. } if is_query_param_domain(target) => {
            Some(field.clone())
        }
        HirExprKind::Paren(expr) => query_param_field_name(expr),
        _ => None,
    }
}

fn route_param_field_value(expr: &HirExpr) -> Option<CapturedResponseValue> {
    match &expr.kind {
        HirExprKind::Binary { op, lhs, rhs }
            if matches!(
                op,
                BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem
            ) =>
        {
            captured_numeric_response_operation(route_param_field_value(lhs)?, *op, rhs)
        }
        HirExprKind::Cast { expr, ty } if is_integer_type_ref(ty) => Some(captured_response_value(
            route_param_field_name(expr)?,
            "route_param_int",
        )),
        HirExprKind::Cast { expr, ty } if is_float_type_ref(ty) => Some(captured_response_value(
            route_param_field_name(expr)?,
            "route_param_float",
        )),
        HirExprKind::Paren(expr) => route_param_field_value(expr),
        _ => Some(captured_response_value(
            route_param_field_name(expr)?,
            "route_param",
        )),
    }
}

fn query_param_field_value(expr: &HirExpr) -> Option<CapturedResponseValue> {
    match &expr.kind {
        HirExprKind::Binary { op, lhs, rhs }
            if matches!(
                op,
                BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem
            ) =>
        {
            captured_numeric_response_operation(query_param_field_value(lhs)?, *op, rhs)
        }
        HirExprKind::Cast { expr, ty } if is_integer_type_ref(ty) => Some(captured_response_value(
            query_param_field_name(expr)?,
            "query_param_int",
        )),
        HirExprKind::Cast { expr, ty } if is_float_type_ref(ty) => Some(captured_response_value(
            query_param_field_name(expr)?,
            "query_param_float",
        )),
        HirExprKind::Paren(expr) => query_param_field_value(expr),
        _ => Some(captured_response_value(
            query_param_field_name(expr)?,
            "query_param",
        )),
    }
}

fn request_body_field_name(expr: &HirExpr) -> Option<String> {
    match &expr.kind {
        HirExprKind::Field { target, field, .. } if is_request_body_domain(target) => {
            Some(field.clone())
        }
        HirExprKind::Paren(expr) => request_body_field_name(expr),
        _ => None,
    }
}

struct CapturedResponseValue {
    name: String,
    value_kind: String,
    op: Option<String>,
    operand_json: Option<String>,
    operand_kind: Option<String>,
    operand_name: Option<String>,
}

fn captured_response_value(name: String, value_kind: &str) -> CapturedResponseValue {
    CapturedResponseValue {
        name,
        value_kind: value_kind.to_string(),
        op: None,
        operand_json: None,
        operand_kind: None,
        operand_name: None,
    }
}

fn captured_numeric_response_operation(
    mut value: CapturedResponseValue,
    op: BinaryOp,
    rhs: &HirExpr,
) -> Option<CapturedResponseValue> {
    if value.op.is_some() {
        return None;
    }
    value.op = Some(
        match op {
            BinaryOp::Add => "add",
            BinaryOp::Sub => "sub",
            BinaryOp::Mul => "mul",
            BinaryOp::Div => "div",
            BinaryOp::Rem => "rem",
            _ => return None,
        }
        .to_string(),
    );
    if value.value_kind.ends_with("_int") {
        if let Some(operand) = static_integer(rhs) {
            value.operand_json = Some(operand.to_string());
            return Some(value);
        }
        let operand = captured_integer_operand(rhs)?;
        value.operand_kind = Some(operand.value_kind);
        value.operand_name = Some(operand.name);
        return Some(value);
    }
    if value.value_kind.ends_with("_float") {
        if let Some(operand) = static_float(rhs) {
            value.operand_json = Some(operand);
            return Some(value);
        }
        let operand = captured_float_operand(rhs)?;
        value.operand_kind = Some(operand.value_kind);
        value.operand_name = Some(operand.name);
        return Some(value);
    }
    None
}

struct CapturedIntegerOperand {
    value_kind: String,
    name: String,
}

struct CapturedFloatOperand {
    value_kind: String,
    name: String,
}

fn captured_integer_operand(expr: &HirExpr) -> Option<CapturedIntegerOperand> {
    if let Some(name) = captured_route_param_integer_name(expr) {
        return Some(CapturedIntegerOperand {
            value_kind: "route_param_int".to_string(),
            name,
        });
    }
    if let Some(name) = captured_query_param_integer_name(expr) {
        return Some(CapturedIntegerOperand {
            value_kind: "query_param_int".to_string(),
            name,
        });
    }
    if let Some(name) = captured_request_body_field_integer_name(expr) {
        return Some(CapturedIntegerOperand {
            value_kind: "request_body_field_int".to_string(),
            name,
        });
    }
    None
}

fn captured_float_operand(expr: &HirExpr) -> Option<CapturedFloatOperand> {
    if let Some(name) = captured_route_param_float_name(expr) {
        return Some(CapturedFloatOperand {
            value_kind: "route_param_float".to_string(),
            name,
        });
    }
    if let Some(name) = captured_query_param_float_name(expr) {
        return Some(CapturedFloatOperand {
            value_kind: "query_param_float".to_string(),
            name,
        });
    }
    if let Some(name) = captured_request_body_field_float_name(expr) {
        return Some(CapturedFloatOperand {
            value_kind: "request_body_field_float".to_string(),
            name,
        });
    }
    None
}

fn captured_route_param_integer_name(expr: &HirExpr) -> Option<String> {
    match &expr.kind {
        HirExprKind::Cast { expr, ty } if is_integer_type_ref(ty) => route_param_field_name(expr),
        HirExprKind::Paren(expr) => captured_route_param_integer_name(expr),
        _ => None,
    }
}

fn captured_route_param_float_name(expr: &HirExpr) -> Option<String> {
    match &expr.kind {
        HirExprKind::Cast { expr, ty } if is_float_type_ref(ty) => route_param_field_name(expr),
        HirExprKind::Paren(expr) => captured_route_param_float_name(expr),
        _ => None,
    }
}

fn captured_query_param_integer_name(expr: &HirExpr) -> Option<String> {
    match &expr.kind {
        HirExprKind::Cast { expr, ty } if is_integer_type_ref(ty) => query_param_field_name(expr),
        HirExprKind::Paren(expr) => captured_query_param_integer_name(expr),
        _ => None,
    }
}

fn captured_query_param_float_name(expr: &HirExpr) -> Option<String> {
    match &expr.kind {
        HirExprKind::Cast { expr, ty } if is_float_type_ref(ty) => query_param_field_name(expr),
        HirExprKind::Paren(expr) => captured_query_param_float_name(expr),
        _ => None,
    }
}

fn captured_request_body_field_integer_name(expr: &HirExpr) -> Option<String> {
    match &expr.kind {
        HirExprKind::Cast { expr, ty } if is_integer_type_ref(ty) => request_body_field_name(expr),
        HirExprKind::Paren(expr) => captured_request_body_field_integer_name(expr),
        _ => None,
    }
}

fn captured_request_body_field_float_name(expr: &HirExpr) -> Option<String> {
    match &expr.kind {
        HirExprKind::Cast { expr, ty } if is_float_type_ref(ty) => request_body_field_name(expr),
        HirExprKind::Paren(expr) => captured_request_body_field_float_name(expr),
        _ => None,
    }
}

fn request_body_field_value(expr: &HirExpr) -> Option<CapturedResponseValue> {
    match &expr.kind {
        HirExprKind::Binary { op, lhs, rhs }
            if matches!(
                op,
                BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem
            ) =>
        {
            captured_numeric_response_operation(request_body_field_value(lhs)?, *op, rhs)
        }
        HirExprKind::Cast { expr, ty } if is_integer_type_ref(ty) => Some(captured_response_value(
            request_body_field_name(expr)?,
            "request_body_field_int",
        )),
        HirExprKind::Cast { expr, ty } if is_float_type_ref(ty) => Some(captured_response_value(
            request_body_field_name(expr)?,
            "request_body_field_float",
        )),
        HirExprKind::Paren(expr) => request_body_field_value(expr),
        _ => Some(captured_response_value(
            request_body_field_name(expr)?,
            "request_body_field",
        )),
    }
}

fn is_float_type_ref(ty: &HirTypeRef) -> bool {
    matches!(
        &ty.kind,
        HirTypeRefKind::Named(name) if matches!(name.as_str(), "float" | "double")
    )
}

fn is_integer_type_ref(ty: &HirTypeRef) -> bool {
    matches!(
        &ty.kind,
        HirTypeRefKind::Named(name)
            if matches!(
                name.as_str(),
                "int" | "uint" | "byte" | "ubyte" | "short" | "ushort" | "long" | "ulong"
            )
    )
}

fn is_route_param_domain(expr: &HirExpr) -> bool {
    match &expr.kind {
        HirExprKind::Domain { name, args, .. } => name == "param" && args.is_empty(),
        HirExprKind::Paren(expr) => is_route_param_domain(expr),
        _ => false,
    }
}

fn is_query_param_domain(expr: &HirExpr) -> bool {
    match &expr.kind {
        HirExprKind::Domain { name, args, .. } => name == "query" && args.is_empty(),
        HirExprKind::Paren(expr) => is_query_param_domain(expr),
        _ => false,
    }
}

fn is_request_body_domain(expr: &HirExpr) -> bool {
    match &expr.kind {
        HirExprKind::Domain { name, args, .. } => name == "body" && args.is_empty(),
        HirExprKind::Paren(expr) => is_request_body_domain(expr),
        _ => false,
    }
}

fn static_string_segments(segments: &[HirStringSegment]) -> Option<String> {
    let mut out = String::new();
    for segment in segments {
        match segment {
            HirStringSegment::Str(value) => out.push_str(value),
            HirStringSegment::Interp(_) => return None,
        }
    }
    Some(out)
}

fn static_string_expr(expr: &HirExpr) -> Option<String> {
    match &expr.kind {
        HirExprKind::String(segments) => static_string_segments(segments),
        HirExprKind::Paren(expr) => static_string_expr(expr),
        _ => None,
    }
}

fn native_response_condition(expr: &HirExpr) -> Option<ServerResponseConditionArtifact> {
    match &expr.kind {
        HirExprKind::Binary { op, lhs, rhs }
            if matches!(
                op,
                BinaryOp::Eq
                    | BinaryOp::Ne
                    | BinaryOp::Lt
                    | BinaryOp::Le
                    | BinaryOp::Gt
                    | BinaryOp::Ge
            ) =>
        {
            native_captured_float_response_condition(*op, lhs, rhs)
                .or_else(|| native_captured_int_response_condition(*op, lhs, rhs))
                .or_else(|| {
                    matches!(op, BinaryOp::Eq | BinaryOp::Ne)
                        .then(|| native_captured_response_condition(*op, lhs, rhs))
                        .flatten()
                })
        }
        HirExprKind::Paren(expr) => native_response_condition(expr),
        _ => None,
    }
}

fn native_captured_float_response_condition(
    op: BinaryOp,
    lhs: &HirExpr,
    rhs: &HirExpr,
) -> Option<ServerResponseConditionArtifact> {
    if let Some(left) = captured_condition_float_operand(lhs) {
        if let Some(value) = static_float(rhs) {
            return Some(ServerResponseConditionArtifact {
                kind: condition_kind_for_float_operand(op, left.kind)?,
                name: left.name,
                value,
                operand_name: None,
                operand_kind: None,
            });
        }
        let right = captured_condition_float_operand(rhs)?;
        return Some(ServerResponseConditionArtifact {
            kind: condition_kind_for_float_operand(op, left.kind)?,
            name: left.name,
            value: String::new(),
            operand_name: Some(right.name),
            operand_kind: Some(right.kind.to_string()),
        });
    }
    let right = captured_condition_float_operand(rhs)?;
    Some(ServerResponseConditionArtifact {
        kind: condition_kind_for_float_operand(reverse_comparison_op(op)?, right.kind)?,
        name: right.name,
        value: static_float(lhs)?,
        operand_name: None,
        operand_kind: None,
    })
}

fn captured_condition_float_operand(expr: &HirExpr) -> Option<CapturedConditionOperand> {
    let operand = captured_float_operand(expr)?;
    let kind = match operand.value_kind.as_str() {
        "request_body_field_float" => "request_body_field_float",
        "route_param_float" => "route_param_float",
        "query_param_float" => "query_param_float",
        _ => return None,
    };
    Some(CapturedConditionOperand {
        kind,
        name: operand.name,
    })
}

fn native_captured_int_response_condition(
    op: BinaryOp,
    lhs: &HirExpr,
    rhs: &HirExpr,
) -> Option<ServerResponseConditionArtifact> {
    if let Some(left) = captured_condition_int_operand(lhs) {
        if let Some(value) = static_integer(rhs) {
            return Some(ServerResponseConditionArtifact {
                kind: condition_kind_for_int_operand(op, left.kind)?,
                name: left.name,
                value: value.to_string(),
                operand_name: None,
                operand_kind: None,
            });
        }
        let right = captured_condition_int_operand(rhs)?;
        return Some(ServerResponseConditionArtifact {
            kind: condition_kind_for_int_operand(op, left.kind)?,
            name: left.name,
            value: String::new(),
            operand_name: Some(right.name),
            operand_kind: Some(right.kind.to_string()),
        });
    }
    let right = captured_condition_int_operand(rhs)?;
    Some(ServerResponseConditionArtifact {
        kind: condition_kind_for_int_operand(reverse_comparison_op(op)?, right.kind)?,
        name: right.name,
        value: static_integer(lhs)?.to_string(),
        operand_name: None,
        operand_kind: None,
    })
}

fn captured_condition_int_operand(expr: &HirExpr) -> Option<CapturedConditionOperand> {
    let operand = captured_integer_operand(expr)?;
    let kind = match operand.value_kind.as_str() {
        "request_body_field_int" => "request_body_field_int",
        "route_param_int" => "route_param_int",
        "query_param_int" => "query_param_int",
        _ => return None,
    };
    Some(CapturedConditionOperand {
        kind,
        name: operand.name,
    })
}

const fn reverse_comparison_op(op: BinaryOp) -> Option<BinaryOp> {
    match op {
        BinaryOp::Eq => Some(BinaryOp::Eq),
        BinaryOp::Ne => Some(BinaryOp::Ne),
        BinaryOp::Lt => Some(BinaryOp::Gt),
        BinaryOp::Le => Some(BinaryOp::Ge),
        BinaryOp::Gt => Some(BinaryOp::Lt),
        BinaryOp::Ge => Some(BinaryOp::Le),
        _ => None,
    }
}

fn native_captured_response_condition(
    op: BinaryOp,
    lhs: &HirExpr,
    rhs: &HirExpr,
) -> Option<ServerResponseConditionArtifact> {
    if let Some(left) = captured_condition_operand(lhs) {
        if let Some(value) = static_string_expr(rhs) {
            return Some(ServerResponseConditionArtifact {
                kind: condition_kind_for_operand(op, left.kind)?,
                name: left.name,
                value,
                operand_name: None,
                operand_kind: None,
            });
        }
        let right = captured_condition_operand(rhs)?;
        return Some(ServerResponseConditionArtifact {
            kind: condition_kind_for_operand(op, left.kind)?,
            name: left.name,
            value: String::new(),
            operand_name: Some(right.name),
            operand_kind: Some(right.kind.to_string()),
        });
    }
    let right = captured_condition_operand(rhs)?;
    let value = static_string_expr(lhs)?;
    Some(ServerResponseConditionArtifact {
        kind: condition_kind_for_operand(op, right.kind)?,
        name: right.name,
        value,
        operand_name: None,
        operand_kind: None,
    })
}

struct CapturedConditionOperand {
    kind: &'static str,
    name: String,
}

fn captured_condition_operand(expr: &HirExpr) -> Option<CapturedConditionOperand> {
    if let Some(name) = request_body_field_name(expr) {
        return Some(CapturedConditionOperand {
            kind: "request_body_field",
            name,
        });
    }
    if let Some(name) = route_param_field_name(expr) {
        return Some(CapturedConditionOperand {
            kind: "route_param",
            name,
        });
    }
    if let Some(name) = query_param_field_name(expr) {
        return Some(CapturedConditionOperand {
            kind: "query_param",
            name,
        });
    }
    None
}

fn condition_kind_for_operand(op: BinaryOp, operand_kind: &str) -> Option<String> {
    let kind = match (operand_kind, op) {
        ("request_body_field", BinaryOp::Eq) => "request_body_field_eq",
        ("request_body_field", BinaryOp::Ne) => "request_body_field_ne",
        ("route_param", BinaryOp::Eq) => "route_param_eq",
        ("route_param", BinaryOp::Ne) => "route_param_ne",
        ("query_param", BinaryOp::Eq) => "query_param_eq",
        ("query_param", BinaryOp::Ne) => "query_param_ne",
        _ => return None,
    };
    Some(kind.to_string())
}

fn condition_kind_for_int_operand(op: BinaryOp, operand_kind: &str) -> Option<String> {
    let kind = match (operand_kind, op) {
        ("request_body_field_int", BinaryOp::Eq) => "request_body_field_int_eq",
        ("request_body_field_int", BinaryOp::Ne) => "request_body_field_int_ne",
        ("request_body_field_int", BinaryOp::Lt) => "request_body_field_int_lt",
        ("request_body_field_int", BinaryOp::Le) => "request_body_field_int_le",
        ("request_body_field_int", BinaryOp::Gt) => "request_body_field_int_gt",
        ("request_body_field_int", BinaryOp::Ge) => "request_body_field_int_ge",
        ("route_param_int", BinaryOp::Eq) => "route_param_int_eq",
        ("route_param_int", BinaryOp::Ne) => "route_param_int_ne",
        ("route_param_int", BinaryOp::Lt) => "route_param_int_lt",
        ("route_param_int", BinaryOp::Le) => "route_param_int_le",
        ("route_param_int", BinaryOp::Gt) => "route_param_int_gt",
        ("route_param_int", BinaryOp::Ge) => "route_param_int_ge",
        ("query_param_int", BinaryOp::Eq) => "query_param_int_eq",
        ("query_param_int", BinaryOp::Ne) => "query_param_int_ne",
        ("query_param_int", BinaryOp::Lt) => "query_param_int_lt",
        ("query_param_int", BinaryOp::Le) => "query_param_int_le",
        ("query_param_int", BinaryOp::Gt) => "query_param_int_gt",
        ("query_param_int", BinaryOp::Ge) => "query_param_int_ge",
        _ => return None,
    };
    Some(kind.to_string())
}

fn condition_kind_for_float_operand(op: BinaryOp, operand_kind: &str) -> Option<String> {
    let kind = match (operand_kind, op) {
        ("request_body_field_float", BinaryOp::Eq) => "request_body_field_float_eq",
        ("request_body_field_float", BinaryOp::Ne) => "request_body_field_float_ne",
        ("request_body_field_float", BinaryOp::Lt) => "request_body_field_float_lt",
        ("request_body_field_float", BinaryOp::Le) => "request_body_field_float_le",
        ("request_body_field_float", BinaryOp::Gt) => "request_body_field_float_gt",
        ("request_body_field_float", BinaryOp::Ge) => "request_body_field_float_ge",
        ("route_param_float", BinaryOp::Eq) => "route_param_float_eq",
        ("route_param_float", BinaryOp::Ne) => "route_param_float_ne",
        ("route_param_float", BinaryOp::Lt) => "route_param_float_lt",
        ("route_param_float", BinaryOp::Le) => "route_param_float_le",
        ("route_param_float", BinaryOp::Gt) => "route_param_float_gt",
        ("route_param_float", BinaryOp::Ge) => "route_param_float_ge",
        ("query_param_float", BinaryOp::Eq) => "query_param_float_eq",
        ("query_param_float", BinaryOp::Ne) => "query_param_float_ne",
        ("query_param_float", BinaryOp::Lt) => "query_param_float_lt",
        ("query_param_float", BinaryOp::Le) => "query_param_float_le",
        ("query_param_float", BinaryOp::Gt) => "query_param_float_gt",
        ("query_param_float", BinaryOp::Ge) => "query_param_float_ge",
        _ => return None,
    };
    Some(kind.to_string())
}

fn guarded_route_response_artifacts(handler: &HirBlock) -> Option<Vec<ServerResponseArtifact>> {
    if handler.stmts.len() < 2 {
        return None;
    }
    let mut out = Vec::with_capacity(handler.stmts.len());
    for (index, stmt) in handler.stmts.iter().enumerate() {
        if index + 1 == handler.stmts.len() {
            let (respond, status, payload) = response_expr_from_stmt(stmt)?;
            out.push(server_response_artifact(respond, status, payload, None));
            continue;
        }
        let HirStmt::Expr(expr) = stmt else {
            return None;
        };
        let HirExprKind::If {
            cond,
            then,
            else_branch: None,
        } = &expr.kind
        else {
            return None;
        };
        if then.stmts.len() != 1 {
            return None;
        }
        let condition = native_response_condition(cond)?;
        let (respond, status, payload) = response_expr_from_stmt(&then.stmts[0])?;
        out.push(server_response_artifact(
            respond,
            status,
            payload,
            Some(condition),
        ));
    }
    Some(out)
}

fn response_expr_from_stmt(stmt: &HirStmt) -> Option<(&HirExpr, &HirExpr, &HirExpr)> {
    let HirStmt::Expr(expr) = stmt else {
        return None;
    };
    response_expr(expr)
}

fn response_expr(expr: &HirExpr) -> Option<(&HirExpr, &HirExpr, &HirExpr)> {
    match &expr.kind {
        HirExprKind::Respond { status, payload } => Some((expr, status, payload)),
        HirExprKind::Paren(expr) => response_expr(expr),
        _ => None,
    }
}

fn write_json_string(value: &str, out: &mut String) {
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            ch if ch <= '\u{1f}' => {
                let _ = write!(out, "\\u{:04x}", u32::from(ch));
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
}

fn json_escaped(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            ch if ch <= '\u{1f}' => {
                let _ = write!(out, "\\u{:04x}", u32::from(ch));
            }
            ch => out.push(ch),
        }
    }
    out
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

fn route_artifact(
    entry: &OriginEntry,
    origin_map: &OriginMap,
    responses_by_route: &HashMap<String, Vec<ServerResponseArtifact>>,
) -> Option<ServerRouteArtifact> {
    let (method, path) = entry.name.split_once(' ')?;
    Some(ServerRouteArtifact {
        method: method.to_string(),
        path: path.to_string(),
        origin_id: entry.id.clone(),
        response_origin_ids: route_response_origin_ids(&entry.id, origin_map),
        responses: responses_by_route
            .get(&entry.id)
            .cloned()
            .unwrap_or_default(),
    })
}

fn route_response_artifacts(program: &HirProgram) -> HashMap<String, Vec<ServerResponseArtifact>> {
    let mut out = HashMap::new();
    for stmt in &program.items {
        collect_stmt_response_artifacts(stmt, None, &mut out);
    }
    out
}

fn collect_stmt_response_artifacts(
    stmt: &HirStmt,
    route_origin_id: Option<&str>,
    out: &mut HashMap<String, Vec<ServerResponseArtifact>>,
) {
    match stmt {
        HirStmt::Let(stmt) => collect_expr_response_artifacts(&stmt.init, route_origin_id, out),
        HirStmt::Const(stmt) => collect_expr_response_artifacts(&stmt.init, route_origin_id, out),
        HirStmt::Function(stmt) => {
            collect_function_body_response_artifacts(&stmt.body, route_origin_id, out);
        }
        HirStmt::Return(stmt) => {
            if let Some(value) = &stmt.value {
                collect_expr_response_artifacts(value, route_origin_id, out);
            }
        }
        HirStmt::Expr(expr) => collect_expr_response_artifacts(expr, route_origin_id, out),
        HirStmt::Struct(_) | HirStmt::Enum(_) | HirStmt::TypeAlias(_) | HirStmt::Import(_) => {}
    }
}

fn collect_block_response_artifacts(
    block: &HirBlock,
    route_origin_id: Option<&str>,
    out: &mut HashMap<String, Vec<ServerResponseArtifact>>,
) {
    for stmt in &block.stmts {
        collect_stmt_response_artifacts(stmt, route_origin_id, out);
    }
}

fn collect_function_body_response_artifacts(
    body: &HirFunctionBody,
    route_origin_id: Option<&str>,
    out: &mut HashMap<String, Vec<ServerResponseArtifact>>,
) {
    match body {
        HirFunctionBody::Block(block) => {
            collect_block_response_artifacts(block, route_origin_id, out);
        }
        HirFunctionBody::Expr(expr) => collect_expr_response_artifacts(expr, route_origin_id, out),
    }
}

#[allow(clippy::too_many_lines)]
fn collect_expr_response_artifacts(
    expr: &HirExpr,
    route_origin_id: Option<&str>,
    out: &mut HashMap<String, Vec<ServerResponseArtifact>>,
) {
    match &expr.kind {
        HirExprKind::Route {
            method,
            path,
            handler,
            ..
        } => {
            let name = format!("{method} {path}");
            let route_origin = origin_id("route", &name, expr.span);
            if let Some(responses) = guarded_route_response_artifacts(handler) {
                out.entry(route_origin).or_default().extend(responses);
            } else {
                collect_block_response_artifacts(handler, Some(&route_origin), out);
            }
        }
        HirExprKind::Respond { status, payload } => {
            if let Some(route_origin_id) = route_origin_id {
                out.entry(route_origin_id.to_string())
                    .or_default()
                    .push(server_response_artifact(expr, status, payload, None));
            }
            collect_expr_response_artifacts(status, route_origin_id, out);
            collect_expr_response_artifacts(payload, route_origin_id, out);
        }
        HirExprKind::Server {
            listen,
            routes,
            body_stmts,
        } => {
            if let Some(listen) = listen {
                collect_expr_response_artifacts(listen, route_origin_id, out);
            }
            for route in routes {
                collect_expr_response_artifacts(route, route_origin_id, out);
            }
            for stmt in body_stmts {
                collect_stmt_response_artifacts(stmt, route_origin_id, out);
            }
        }
        HirExprKind::Out(inner)
        | HirExprKind::Unary { expr: inner, .. }
        | HirExprKind::Paren(inner)
        | HirExprKind::Throw(inner)
        | HirExprKind::Await(inner)
        | HirExprKind::Cast { expr: inner, .. } => {
            collect_expr_response_artifacts(inner, route_origin_id, out);
        }
        HirExprKind::Html(block) | HirExprKind::Block(block) => {
            collect_block_response_artifacts(block, route_origin_id, out);
        }
        HirExprKind::Domain { args, .. } => {
            for arg in args {
                collect_expr_response_artifacts(arg, route_origin_id, out);
            }
        }
        HirExprKind::Call { callee, args } => {
            collect_expr_response_artifacts(callee, route_origin_id, out);
            for arg in args {
                collect_expr_response_artifacts(arg, route_origin_id, out);
            }
        }
        HirExprKind::String(segments) => {
            for segment in segments {
                if let HirStringSegment::Interp(expr) = segment {
                    collect_expr_response_artifacts(expr, route_origin_id, out);
                }
            }
        }
        HirExprKind::Binary { lhs, rhs, .. } => {
            collect_expr_response_artifacts(lhs, route_origin_id, out);
            collect_expr_response_artifacts(rhs, route_origin_id, out);
        }
        HirExprKind::If {
            cond,
            then,
            else_branch,
        } => {
            collect_expr_response_artifacts(cond, route_origin_id, out);
            collect_block_response_artifacts(then, route_origin_id, out);
            if let Some(expr) = else_branch {
                collect_expr_response_artifacts(expr, route_origin_id, out);
            }
        }
        HirExprKind::When { scrutinee, arms } => {
            collect_expr_response_artifacts(scrutinee, route_origin_id, out);
            for arm in arms {
                collect_pattern_response_artifacts(&arm.pattern, route_origin_id, out);
                collect_expr_response_artifacts(&arm.body, route_origin_id, out);
            }
        }
        HirExprKind::Assign { value, .. } => {
            collect_expr_response_artifacts(value, route_origin_id, out);
        }
        HirExprKind::AssignField { object, value, .. } => {
            collect_expr_response_artifacts(object, route_origin_id, out);
            collect_expr_response_artifacts(value, route_origin_id, out);
        }
        HirExprKind::AssignIndex {
            object,
            index,
            value,
        } => {
            collect_expr_response_artifacts(object, route_origin_id, out);
            collect_expr_response_artifacts(index, route_origin_id, out);
            collect_expr_response_artifacts(value, route_origin_id, out);
        }
        HirExprKind::For { iter, body, .. } => {
            collect_expr_response_artifacts(iter, route_origin_id, out);
            collect_block_response_artifacts(body, route_origin_id, out);
        }
        HirExprKind::While { cond, body } => {
            collect_expr_response_artifacts(cond, route_origin_id, out);
            collect_block_response_artifacts(body, route_origin_id, out);
        }
        HirExprKind::Range { start, end, .. } => {
            collect_expr_response_artifacts(start, route_origin_id, out);
            collect_expr_response_artifacts(end, route_origin_id, out);
        }
        HirExprKind::Array(items) | HirExprKind::Tuple(items) => {
            for item in items {
                collect_expr_response_artifacts(item, route_origin_id, out);
            }
        }
        HirExprKind::Object(fields) | HirExprKind::TypedObject { fields, .. } => {
            for field in fields {
                collect_expr_response_artifacts(&field.value, route_origin_id, out);
            }
        }
        HirExprKind::Index { target, index } => {
            collect_expr_response_artifacts(target, route_origin_id, out);
            collect_expr_response_artifacts(index, route_origin_id, out);
        }
        HirExprKind::Slice { target, start, end } => {
            collect_expr_response_artifacts(target, route_origin_id, out);
            if let Some(start) = start {
                collect_expr_response_artifacts(start, route_origin_id, out);
            }
            if let Some(end) = end {
                collect_expr_response_artifacts(end, route_origin_id, out);
            }
        }
        HirExprKind::Field { target, .. } | HirExprKind::OptionalField { target, .. } => {
            collect_expr_response_artifacts(target, route_origin_id, out);
        }
        HirExprKind::Lambda { body, .. } => {
            collect_function_body_response_artifacts(body, route_origin_id, out);
        }
        HirExprKind::Try { try_block, catch } => {
            collect_block_response_artifacts(try_block, route_origin_id, out);
            if let Some(catch) = catch {
                collect_block_response_artifacts(&catch.body, route_origin_id, out);
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

fn collect_pattern_response_artifacts(
    pattern: &HirPattern,
    route_origin_id: Option<&str>,
    out: &mut HashMap<String, Vec<ServerResponseArtifact>>,
) {
    match pattern {
        HirPattern::Literal(expr)
        | HirPattern::Guard(expr)
        | HirPattern::Not(expr)
        | HirPattern::Contains(expr) => collect_expr_response_artifacts(expr, route_origin_id, out),
        HirPattern::Range { start, end, .. } => {
            collect_expr_response_artifacts(start, route_origin_id, out);
            collect_expr_response_artifacts(end, route_origin_id, out);
        }
        HirPattern::Wildcard => {}
    }
}

fn route_response_origin_ids(route_origin_id: &str, origin_map: &OriginMap) -> Vec<String> {
    let mut response_origin_ids = Vec::new();
    for edge in origin_map
        .edges
        .iter()
        .filter(|edge| edge.from == route_origin_id && edge.kind == "contains")
    {
        let Some(entry) = origin_map
            .entries
            .iter()
            .find(|entry| entry.id == edge.to && entry.kind == "domain" && entry.name == "respond")
        else {
            continue;
        };
        if !response_origin_ids.contains(&entry.id) {
            response_origin_ids.push(entry.id.clone());
        }
    }
    response_origin_ids
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
        match (entry.kind.as_str(), entry.name.as_str()) {
            ("domain", "db") => {
                features.insert("in_memory_db");
            }
            ("call", "@db.connect") => {
                features.insert("db_adapter");
            }
            ("call", "@payment.connect") => {
                features.insert("payment_adapter");
            }
            ("call", "@shipping.connect") => {
                features.insert("shipping_adapter");
            }
            ("domain", "html") => {
                features.insert("html_renderer");
            }
            ("domain", "out") => {
                features.insert("console_io");
            }
            ("domain", "serve") => {
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
            r"let sig count: int = 0
@out await count",
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
    fn build_manifest_declares_db_adapter_runtime_feature_for_db_connect() {
        let program = lower(
            r#"@server {
  @listen 8080
  let external = @db.connect "file://data/app.wal.jsonl"
  @route GET /ping {
    @respond 200 external.find("User", { name: "Ada" })
  }
}"#,
        );
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);

        assert!(manifest
            .capabilities
            .runtime_features
            .contains(&"in_memory_db".to_string()));
        assert!(manifest
            .capabilities
            .runtime_features
            .contains(&"db_adapter".to_string()));
    }

    #[test]
    fn build_manifest_declares_commerce_adapter_runtime_features() {
        let program = lower(
            r#"@server {
  @listen 8080
  let payments = @payment.connect("test://local")
  let shipping = @shipping.connect("test://local")
  @route POST /checkout {
    let captured = payments.capture({ orderId: "o_1", amount: 42, method: "card" })
    let booked = shipping.book({ orderId: "o_1", carrier: "local", address: "Seoul" })
    @respond 200 { payment: captured.provider, shipment: booked.provider }
  }
}"#,
        );
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);

        assert!(manifest
            .capabilities
            .runtime_features
            .contains(&"payment_adapter".to_string()));
        assert!(manifest
            .capabilities
            .runtime_features
            .contains(&"shipping_adapter".to_string()));
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
        assert!(plan.bundles.iter().any(|bundle| {
            bundle.kind == "native_runtime_image_dockerfile"
                && bundle.path == "server/native/Dockerfile"
                && bundle.runtime_features.contains(&"http_server".to_string())
                && bundle.runtime_features.contains(&"router".to_string())
        }));
        assert!(plan.bundles.iter().any(|bundle| {
            bundle.kind == "native_server_routes_source"
                && bundle.path == "server/native/routes.rs"
                && bundle.runtime_features.contains(&"http_server".to_string())
                && bundle.runtime_features.contains(&"router".to_string())
        }));
        assert!(plan.bundles.iter().any(|bundle| {
            bundle.kind == "native_server_router_source"
                && bundle.path == "server/native/router.rs"
                && bundle.runtime_features.contains(&"http_server".to_string())
                && bundle.runtime_features.contains(&"router".to_string())
        }));
        assert!(plan.bundles.iter().any(|bundle| {
            bundle.kind == "native_server_launcher_package"
                && bundle.path == "server/native/Cargo.toml"
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
        assert!(manifest.artifacts.iter().any(|artifact| {
            artifact.kind == "native_runtime_image_dockerfile"
                && artifact.path == "server/native/Dockerfile"
        }));
        assert!(manifest.artifacts.iter().any(|artifact| {
            artifact.kind == "native_server_routes_source"
                && artifact.path == "server/native/routes.rs"
        }));
        assert!(manifest.artifacts.iter().any(|artifact| {
            artifact.kind == "native_server_router_source"
                && artifact.path == "server/native/router.rs"
        }));
        assert!(manifest.artifacts.iter().any(|artifact| {
            artifact.kind == "native_server_launcher_package"
                && artifact.path == "server/native/Cargo.toml"
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
            .any(|artifact| artifact.kind == "client_manifest"
                && artifact.path == "client/manifest.json"));
        assert!(manifest
            .artifacts
            .iter()
            .any(|artifact| artifact.kind == "client_reactive_plan"
                && artifact.path == "client/reactive-plan.json"));
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
            bundle.kind == "client_manifest"
                && bundle.path == "client/manifest.json"
                && bundle.runtime_features == vec!["client_wasm"]
        }));
        assert!(plan.bundles.iter().any(|bundle| {
            bundle.kind == "client_reactive_plan"
                && bundle.path == "client/reactive-plan.json"
                && bundle.runtime_features == vec!["client_wasm"]
        }));
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
        assert_eq!(artifact.routes[0].response_origin_ids.len(), 1);
        assert!(artifact.routes[0].response_origin_ids[0].starts_with("ori_"));
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
    fn server_runtime_artifact_with_program_records_static_response_body() {
        let src = r#"@server {
  @listen 0
  @route GET /ping {
    @respond 200 { ok: true, msg: "pong" }
  }
}"#;
        let program = lower(src);
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let artifact =
            server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);
        let response = &artifact.routes[0].responses[0];
        let source = native_server_handlers_source(&artifact);

        assert_eq!(response.status, Some(200));
        assert_eq!(response.body_kind, "static_json");
        assert_eq!(
            response.body_json.as_deref(),
            Some(r#"{"ok":true,"msg":"pong"}"#)
        );
        assert!(source.contains("status: 200"));
        assert!(source.contains(r#"body: "{\"ok\":true,\"msg\":\"pong\"}""#));
        assert!(!source.contains("native route body lowering pending"));
    }

    #[test]
    fn server_runtime_artifact_lowers_no_content_response_to_empty_native_body() {
        let src = r"@server {
  @listen 8080
  @route DELETE /items/:id {
    @respond 204 {}
  }
}";
        let program = lower(src);
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let artifact =
            server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);
        let response = &artifact.routes[0].responses[0];
        let handlers = native_server_handlers_source(&artifact);
        let launcher = native_server_launcher_source(
            "server/app.orv-runtime.json",
            "server/native-server.json",
            &artifact,
        );

        assert_eq!(response.status, Some(204));
        assert_eq!(response.body_kind, "empty");
        assert!(response.body_json.is_none());
        assert!(handlers.contains("status: 204"));
        assert!(handlers.contains("body: String::new()"));
        assert!(!handlers.contains("native route body lowering pending"));
        assert!(launcher.contains("orv_native_status_disallows_body(dispatch.status)"));
        assert!(launcher.contains("content-length: {}"));
    }

    #[test]
    fn server_runtime_artifact_lowers_route_param_response_body() {
        let src = r"@server {
  @listen 8080
  @route GET /users/:id {
    @respond 200 { id: @param.id }
  }
}";
        let program = lower(src);
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let artifact =
            server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);
        let response = &artifact.routes[0].responses[0];
        let handlers = native_server_handlers_source(&artifact);
        let launcher = native_server_launcher_source(
            "server/app.orv-runtime.json",
            "server/native-server.json",
            &artifact,
        );

        assert_eq!(response.status, Some(200));
        assert_eq!(response.body_kind, "route_param_json");
        assert!(response.body_json.is_none());
        assert!(handlers.contains("routes::orv_native_param_value(route_match, \"id\")"));
        assert!(handlers.contains("orv_native_push_json_string("));
        assert!(handlers.contains("body.push_str(\"\\\"id\\\":\");"));
        assert!(!handlers.contains("native route body lowering pending"));
        assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
        assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
    }

    #[test]
    fn native_server_launcher_falls_back_for_unknown_route_param_response() {
        let src = r"@server {
  @listen 8080
  @route GET /users/:id {
    @respond 200 { name: @param.name }
  }
}";
        let program = lower(src);
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let artifact =
            server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);
        let response = &artifact.routes[0].responses[0];
        let launcher = native_server_launcher_source(
            "server/app.orv-runtime.json",
            "server/native-server.json",
            &artifact,
        );

        assert_eq!(response.body_kind, "route_param_json");
        assert!(launcher.contains("fn orv_native_reference_bridge("));
        assert!(launcher.contains(r#"std::process::Command::new("orv")"#));
        assert!(!launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    }

    #[test]
    fn server_runtime_artifact_lowers_query_param_response_body() {
        let src = r"@server {
  @listen 8080
  @route GET /search {
    @respond 200 { q: @query.q }
  }
}";
        let program = lower(src);
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let artifact =
            server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);
        let response = &artifact.routes[0].responses[0];
        let native_route_table = native_server_routes_source(&artifact);
        let native_router_dispatch = native_server_router_source();
        let handlers = native_server_handlers_source(&artifact);
        let launcher = native_server_launcher_source(
            "server/app.orv-runtime.json",
            "server/native-server.json",
            &artifact,
        );

        assert_eq!(response.status, Some(200));
        assert_eq!(response.body_kind, "query_param_json");
        assert!(response.body_json.is_none());
        assert_eq!(response.body_query_params[0].field, "q");
        assert_eq!(response.body_query_params[0].param, "q");
        assert!(native_route_table.contains("pub query: Vec<OrvNativeParam>"));
        assert!(native_route_table.contains("pub fn orv_native_query_value<'a>("));
        assert!(native_router_dispatch.contains("pub fn orv_native_dispatch_with_query("));
        assert!(handlers.contains("routes::orv_native_query_value(route_match, \"q\")"));
        assert!(handlers.contains("orv_native_push_json_string("));
        assert!(launcher.contains("query: Vec<routes::OrvNativeParam>"));
        assert!(launcher.contains("orv_native_parse_query(query)"));
        assert!(launcher.contains("router::orv_native_dispatch_with_request("));
        assert!(launcher.contains("request.body"));
        assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
        assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
    }

    #[test]
    fn server_runtime_artifact_lowers_request_body_response_body() {
        let src = r"@server {
  @listen 8080
  @route POST /echo {
    @respond 201 { received: @body }
  }
}";
        let program = lower(src);
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let artifact =
            server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);
        let response = &artifact.routes[0].responses[0];
        let native_route_table = native_server_routes_source(&artifact);
        let native_router_dispatch = native_server_router_source();
        let handlers = native_server_handlers_source(&artifact);
        let launcher = native_server_launcher_source(
            "server/app.orv-runtime.json",
            "server/native-server.json",
            &artifact,
        );

        assert_eq!(response.status, Some(201));
        assert_eq!(response.body_kind, "request_body_json");
        assert!(response.body_json.is_none());
        assert_eq!(response.body_request_json[0].field, "received");
        assert!(native_route_table.contains("pub body: String"));
        assert!(native_route_table.contains("pub fn orv_native_body_json("));
        assert!(native_router_dispatch.contains("pub fn orv_native_dispatch_with_request("));
        assert!(handlers.contains("routes::orv_native_body_json(route_match).unwrap_or(\"null\")"));
        assert!(!handlers.contains("orv_native_push_json_string("));
        assert!(handlers.contains("body.push_str(\"\\\"received\\\":\");"));
        assert!(launcher.contains("body: String"));
        assert!(launcher.contains("orv_native_content_length("));
        assert!(launcher.contains("router::orv_native_dispatch_with_request("));
        assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
        assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
    }

    #[test]
    fn server_runtime_artifact_lowers_request_body_field_response_body() {
        let src = r"@server {
  @listen 8080
  @route POST /members {
    @respond 201 { handle: @body.handle, email: @body.email }
  }
}";
        let program = lower(src);
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let artifact =
            server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);
        let response = &artifact.routes[0].responses[0];
        let native_route_table = native_server_routes_source(&artifact);
        let handlers = native_server_handlers_source(&artifact);
        let launcher = native_server_launcher_source(
            "server/app.orv-runtime.json",
            "server/native-server.json",
            &artifact,
        );

        assert_eq!(response.status, Some(201));
        assert_eq!(response.body_kind, "request_body_field_json");
        assert!(response.body_json.is_none());
        assert_eq!(response.body_request_fields[0].field, "handle");
        assert_eq!(response.body_request_fields[0].name, "handle");
        assert_eq!(response.body_request_fields[1].field, "email");
        assert_eq!(response.body_request_fields[1].name, "email");
        assert!(native_route_table.contains("pub body_fields: Vec<OrvNativeParam>"));
        assert!(native_route_table.contains("pub fn orv_native_body_field_value<'a>("));
        assert!(handlers.contains("routes::orv_native_body_field_value(route_match, \"handle\")"));
        assert!(handlers.contains("routes::orv_native_body_field_value(route_match, \"email\")"));
        assert!(handlers.contains("orv_native_push_json_string("));
        assert!(launcher.contains("orv_native_parse_body_fields("));
        assert!(launcher.contains("orv_native_parse_json_object_fields("));
        assert!(launcher.contains("orv_native_parse_query(&body)"));
        assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
        assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
    }

    #[test]
    fn server_runtime_artifact_lowers_mixed_static_and_request_body_field_response_body() {
        let src = r#"@server {
  @listen 8080
  @route POST /orders {
    @respond 404 { err: "product_not_found", sku: @body.sku }
  }
}"#;
        let program = lower(src);
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let artifact =
            server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);
        let response = &artifact.routes[0].responses[0];
        let handlers = native_server_handlers_source(&artifact);
        let launcher = native_server_launcher_source(
            "server/app.orv-runtime.json",
            "server/native-server.json",
            &artifact,
        );

        assert_eq!(response.status, Some(404));
        assert_eq!(response.body_kind, "mixed_json");
        assert!(response.body_json.is_none());
        assert_eq!(response.body_object_fields[0].field, "err");
        assert_eq!(response.body_object_fields[0].value_kind, "static_json");
        assert_eq!(
            response.body_object_fields[0].value_json.as_deref(),
            Some(r#""product_not_found""#)
        );
        assert_eq!(response.body_object_fields[1].field, "sku");
        assert_eq!(
            response.body_object_fields[1].value_kind,
            "request_body_field"
        );
        assert_eq!(response.body_object_fields[1].name.as_deref(), Some("sku"));
        assert!(handlers.contains("body.push_str(\"\\\"err\\\":\");"));
        assert!(handlers.contains("body.push_str(\"\\\"product_not_found\\\"\");"));
        assert!(handlers.contains("routes::orv_native_body_field_value(route_match, \"sku\")"));
        assert!(handlers.contains("orv_native_push_json_string("));
        assert!(!handlers.contains("native route body lowering pending"));
        assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
        assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
    }

    #[test]
    fn native_server_launcher_lowers_mixed_static_and_route_query_arithmetic_response_body() {
        let src = r#"@server {
  @listen 8080
  @route GET /products/:id/mixed {
    @respond 200 {
      kind: "calc",
      next_id: (@param.id as int) + 1,
      prev_page: (@query.page as int) - 1
    }
  }
}"#;
        let program = lower(src);
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let artifact =
            server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);
        let response = &artifact.routes[0].responses[0];
        let handlers = native_server_handlers_source(&artifact);
        let launcher = native_server_launcher_source(
            "server/app.orv-runtime.json",
            "server/native-server.json",
            &artifact,
        );

        assert_eq!(response.body_kind, "mixed_json");
        assert_eq!(response.body_object_fields.len(), 3);
        assert_eq!(response.body_object_fields[1].field, "next_id");
        assert_eq!(response.body_object_fields[1].value_kind, "route_param_int");
        assert_eq!(response.body_object_fields[1].name.as_deref(), Some("id"));
        assert_eq!(response.body_object_fields[1].op.as_deref(), Some("add"));
        assert_eq!(
            response.body_object_fields[1].operand_json.as_deref(),
            Some("1")
        );
        assert_eq!(response.body_object_fields[2].field, "prev_page");
        assert_eq!(response.body_object_fields[2].value_kind, "query_param_int");
        assert_eq!(response.body_object_fields[2].name.as_deref(), Some("page"));
        assert_eq!(response.body_object_fields[2].op.as_deref(), Some("sub"));
        assert_eq!(
            response.body_object_fields[2].operand_json.as_deref(),
            Some("1")
        );
        assert!(handlers.contains("value.checked_add(1)"));
        assert!(handlers.contains("value.checked_sub(1)"));
        assert!(!handlers.contains("native route body lowering pending"));
        assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
        assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
    }

    #[test]
    fn native_server_launcher_falls_back_for_dynamic_multi_response_routes() {
        let src = r#"@server {
  @listen 8080
  @route POST /orders {
    if @body.sku == "" {
      @respond 404 { err: "missing_sku" }
    }
    @respond 201 { quantity: (@body.quantity as int) ** (@body.bonus as int) }
  }
}"#;
        let program = lower(src);
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let artifact =
            server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);
        let launcher = native_server_launcher_source(
            "server/app.orv-runtime.json",
            "server/native-server.json",
            &artifact,
        );

        assert_eq!(artifact.routes[0].responses.len(), 2);
        assert!(launcher.contains("fn orv_native_reference_bridge("));
        assert!(launcher.contains(r#"std::process::Command::new("orv")"#));
        assert!(!launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    }

    #[test]
    fn native_server_launcher_lowers_request_body_int_cast_response_body() {
        let src = r#"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { quantity: @body.quantity as int }
  }
}"#;
        let program = lower(src);
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let artifact =
            server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);
        let response = &artifact.routes[0].responses[0];
        let handlers = native_server_handlers_source(&artifact);
        let launcher = native_server_launcher_source(
            "server/app.orv-runtime.json",
            "server/native-server.json",
            &artifact,
        );

        assert_eq!(response.body_kind, "request_body_field_json");
        assert_eq!(response.body_request_fields[0].field, "quantity");
        assert_eq!(response.body_request_fields[0].name, "quantity");
        assert_eq!(
            response.body_request_fields[0].value_kind,
            "request_body_field_int"
        );
        assert!(handlers.contains(".trim().parse::<i64>()"));
        assert!(handlers.contains("body.push_str(&value.to_string())"));
        assert!(!handlers.contains("orv_native_push_json_string(routes::orv_native_body_field_value(route_match, \"quantity\")"));
        assert!(!handlers.contains("native route body lowering pending"));
        assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
        assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
    }

    #[test]
    fn native_server_launcher_lowers_request_body_int_add_literal_response_body() {
        let src = r"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { quantity: (@body.quantity as int) + 1 }
  }
}";
        let program = lower(src);
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let artifact =
            server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);
        let response = &artifact.routes[0].responses[0];
        let handlers = native_server_handlers_source(&artifact);
        let launcher = native_server_launcher_source(
            "server/app.orv-runtime.json",
            "server/native-server.json",
            &artifact,
        );

        assert_eq!(response.body_kind, "request_body_field_json");
        assert_eq!(response.body_request_fields[0].field, "quantity");
        assert_eq!(response.body_request_fields[0].name, "quantity");
        assert_eq!(
            response.body_request_fields[0].value_kind,
            "request_body_field_int"
        );
        assert_eq!(response.body_request_fields[0].op.as_deref(), Some("add"));
        assert_eq!(
            response.body_request_fields[0].operand_json.as_deref(),
            Some("1")
        );
        assert!(handlers.contains(".trim().parse::<i64>()"));
        assert!(handlers.contains("value.checked_add(1)"));
        assert!(handlers.contains("body.push_str(&value.to_string())"));
        assert!(!handlers.contains("native route body lowering pending"));
        assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
        assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
    }

    #[test]
    fn native_server_launcher_lowers_request_body_int_mul_literal_response_body() {
        let src = r"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { cents: (@body.quantity as int) * 100 }
  }
}";
        let program = lower(src);
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let artifact =
            server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);
        let response = &artifact.routes[0].responses[0];
        let handlers = native_server_handlers_source(&artifact);
        let launcher = native_server_launcher_source(
            "server/app.orv-runtime.json",
            "server/native-server.json",
            &artifact,
        );

        assert_eq!(response.body_kind, "request_body_field_json");
        assert_eq!(response.body_request_fields[0].field, "cents");
        assert_eq!(response.body_request_fields[0].name, "quantity");
        assert_eq!(
            response.body_request_fields[0].value_kind,
            "request_body_field_int"
        );
        assert_eq!(response.body_request_fields[0].op.as_deref(), Some("mul"));
        assert_eq!(
            response.body_request_fields[0].operand_json.as_deref(),
            Some("100")
        );
        assert!(handlers.contains(".trim().parse::<i64>()"));
        assert!(handlers.contains("value.checked_mul(100)"));
        assert!(handlers.contains("body.push_str(&value.to_string())"));
        assert!(!handlers.contains("native route body lowering pending"));
        assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
        assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
    }

    #[test]
    fn native_server_launcher_lowers_request_body_int_mul_field_response_body() {
        let src = r"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { total: (@body.quantity as int) * (@body.unit_price as int) }
  }
}";
        let program = lower(src);
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let artifact =
            server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);
        let response = &artifact.routes[0].responses[0];
        let handlers = native_server_handlers_source(&artifact);
        let launcher = native_server_launcher_source(
            "server/app.orv-runtime.json",
            "server/native-server.json",
            &artifact,
        );

        assert_eq!(response.body_kind, "request_body_field_json");
        assert_eq!(response.body_request_fields[0].field, "total");
        assert_eq!(response.body_request_fields[0].name, "quantity");
        assert_eq!(
            response.body_request_fields[0].value_kind,
            "request_body_field_int"
        );
        assert_eq!(response.body_request_fields[0].op.as_deref(), Some("mul"));
        assert_eq!(
            response.body_request_fields[0].operand_kind.as_deref(),
            Some("request_body_field_int")
        );
        assert_eq!(
            response.body_request_fields[0].operand_name.as_deref(),
            Some("unit_price")
        );
        assert!(handlers.contains("routes::orv_native_body_field_value(route_match, \"quantity\")"));
        assert!(
            handlers.contains("routes::orv_native_body_field_value(route_match, \"unit_price\")")
        );
        assert!(handlers.contains("value.checked_mul(operand)"));
        assert!(!handlers.contains("native route body lowering pending"));
        assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
        assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
    }

    #[test]
    fn native_server_launcher_lowers_request_body_int_sub_field_response_body() {
        let src = r"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { due: (@body.total as int) - (@body.discount as int) }
  }
}";
        let program = lower(src);
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let artifact =
            server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);
        let response = &artifact.routes[0].responses[0];
        let handlers = native_server_handlers_source(&artifact);
        let launcher = native_server_launcher_source(
            "server/app.orv-runtime.json",
            "server/native-server.json",
            &artifact,
        );

        assert_eq!(response.body_kind, "request_body_field_json");
        assert_eq!(response.body_request_fields[0].field, "due");
        assert_eq!(response.body_request_fields[0].name, "total");
        assert_eq!(
            response.body_request_fields[0].value_kind,
            "request_body_field_int"
        );
        assert_eq!(response.body_request_fields[0].op.as_deref(), Some("sub"));
        assert_eq!(
            response.body_request_fields[0].operand_kind.as_deref(),
            Some("request_body_field_int")
        );
        assert_eq!(
            response.body_request_fields[0].operand_name.as_deref(),
            Some("discount")
        );
        assert!(handlers.contains("routes::orv_native_body_field_value(route_match, \"total\")"));
        assert!(handlers.contains("routes::orv_native_body_field_value(route_match, \"discount\")"));
        assert!(handlers.contains("value.checked_sub(operand)"));
        assert!(!handlers.contains("native route body lowering pending"));
        assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
        assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
    }

    #[test]
    fn native_server_launcher_lowers_request_body_int_div_field_response_body() {
        let src = r"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { share: (@body.total as int) / (@body.parts as int) }
  }
}";
        let program = lower(src);
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let artifact =
            server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);
        let response = &artifact.routes[0].responses[0];
        let handlers = native_server_handlers_source(&artifact);
        let launcher = native_server_launcher_source(
            "server/app.orv-runtime.json",
            "server/native-server.json",
            &artifact,
        );

        assert_eq!(response.body_kind, "request_body_field_json");
        assert_eq!(response.body_request_fields[0].field, "share");
        assert_eq!(response.body_request_fields[0].name, "total");
        assert_eq!(
            response.body_request_fields[0].value_kind,
            "request_body_field_int"
        );
        assert_eq!(response.body_request_fields[0].op.as_deref(), Some("div"));
        assert_eq!(
            response.body_request_fields[0].operand_kind.as_deref(),
            Some("request_body_field_int")
        );
        assert_eq!(
            response.body_request_fields[0].operand_name.as_deref(),
            Some("parts")
        );
        assert!(handlers.contains("routes::orv_native_body_field_value(route_match, \"total\")"));
        assert!(handlers.contains("routes::orv_native_body_field_value(route_match, \"parts\")"));
        assert!(handlers.contains("value.checked_div(operand)"));
        assert!(!handlers.contains("native route body lowering pending"));
        assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
        assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
    }

    #[test]
    fn native_server_launcher_lowers_request_body_int_rem_field_response_body() {
        let src = r"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { remainder: (@body.total as int) % (@body.parts as int) }
  }
}";
        let program = lower(src);
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let artifact =
            server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);
        let response = &artifact.routes[0].responses[0];
        let handlers = native_server_handlers_source(&artifact);
        let launcher = native_server_launcher_source(
            "server/app.orv-runtime.json",
            "server/native-server.json",
            &artifact,
        );

        assert_eq!(response.body_kind, "request_body_field_json");
        assert_eq!(response.body_request_fields[0].field, "remainder");
        assert_eq!(response.body_request_fields[0].name, "total");
        assert_eq!(
            response.body_request_fields[0].value_kind,
            "request_body_field_int"
        );
        assert_eq!(response.body_request_fields[0].op.as_deref(), Some("rem"));
        assert_eq!(
            response.body_request_fields[0].operand_kind.as_deref(),
            Some("request_body_field_int")
        );
        assert_eq!(
            response.body_request_fields[0].operand_name.as_deref(),
            Some("parts")
        );
        assert!(handlers.contains("routes::orv_native_body_field_value(route_match, \"total\")"));
        assert!(handlers.contains("routes::orv_native_body_field_value(route_match, \"parts\")"));
        assert!(handlers.contains("value.checked_rem(operand)"));
        assert!(!handlers.contains("native route body lowering pending"));
        assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
        assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
    }

    #[test]
    fn native_server_launcher_lowers_request_body_float_cast_response_body() {
        let src = r#"@server {
  @listen 8080
  @route POST /payments {
    @respond 201 { amount: @body.amount as float }
  }
}"#;
        let program = lower(src);
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let artifact =
            server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);
        let response = &artifact.routes[0].responses[0];
        let handlers = native_server_handlers_source(&artifact);
        let launcher = native_server_launcher_source(
            "server/app.orv-runtime.json",
            "server/native-server.json",
            &artifact,
        );

        assert_eq!(response.body_kind, "request_body_field_json");
        assert_eq!(response.body_request_fields[0].field, "amount");
        assert_eq!(response.body_request_fields[0].name, "amount");
        assert_eq!(
            response.body_request_fields[0].value_kind,
            "request_body_field_float"
        );
        assert!(handlers.contains(".trim().parse::<f64>()"));
        assert!(handlers.contains("body.push_str(&value.to_string())"));
        assert!(!handlers.contains("orv_native_push_json_string(routes::orv_native_body_field_value(route_match, \"amount\")"));
        assert!(!handlers.contains("native route body lowering pending"));
        assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
        assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
    }

    #[test]
    fn native_server_launcher_lowers_request_body_float_mul_field_response_body() {
        let src = r#"@server {
  @listen 8080
  @route POST /payments {
    @respond 201 { total: (@body.price as float) * (@body.quantity as float) }
  }
}"#;
        let program = lower(src);
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let artifact =
            server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);
        let response = &artifact.routes[0].responses[0];
        let handlers = native_server_handlers_source(&artifact);
        let launcher = native_server_launcher_source(
            "server/app.orv-runtime.json",
            "server/native-server.json",
            &artifact,
        );

        assert_eq!(response.body_kind, "request_body_field_json");
        assert_eq!(response.body_request_fields[0].field, "total");
        assert_eq!(response.body_request_fields[0].name, "price");
        assert_eq!(
            response.body_request_fields[0].value_kind,
            "request_body_field_float"
        );
        assert_eq!(response.body_request_fields[0].op.as_deref(), Some("mul"));
        assert_eq!(
            response.body_request_fields[0].operand_kind.as_deref(),
            Some("request_body_field_float")
        );
        assert_eq!(
            response.body_request_fields[0].operand_name.as_deref(),
            Some("quantity")
        );
        assert!(handlers.contains("routes::orv_native_body_field_value(route_match, \"price\")"));
        assert!(handlers.contains("routes::orv_native_body_field_value(route_match, \"quantity\")"));
        assert!(handlers.contains("let value = value * operand;"));
        assert!(handlers.contains("if value.is_finite()"));
        assert!(!handlers.contains("native route body lowering pending"));
        assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
        assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
    }

    #[test]
    fn native_server_launcher_lowers_route_param_int_cast_response_body() {
        let src = r"@server {
  @listen 8080
  @route GET /products/:id {
    @respond 200 { id: @param.id as int }
  }
}";
        let program = lower(src);
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let artifact =
            server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);
        let response = &artifact.routes[0].responses[0];
        let handlers = native_server_handlers_source(&artifact);
        let launcher = native_server_launcher_source(
            "server/app.orv-runtime.json",
            "server/native-server.json",
            &artifact,
        );

        assert_eq!(response.body_kind, "route_param_json");
        assert_eq!(response.body_route_params[0].field, "id");
        assert_eq!(response.body_route_params[0].param, "id");
        assert_eq!(response.body_route_params[0].value_kind, "route_param_int");
        assert!(handlers.contains(".trim().parse::<i64>()"));
        assert!(handlers.contains("body.push_str(&value.to_string())"));
        assert!(!handlers.contains(
            "orv_native_push_json_string(routes::orv_native_param_value(route_match, \"id\")"
        ));
        assert!(!handlers.contains("native route body lowering pending"));
        assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
        assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
    }

    #[test]
    fn native_server_launcher_lowers_route_param_int_static_arithmetic_response_body() {
        let src = r"@server {
  @listen 8080
  @route GET /products/:id {
    @respond 200 {
      prev: (@param.id as int) - 1,
      doubled: (@param.id as int) * 2,
      half: (@param.id as int) / 2,
      parity: (@param.id as int) % 2
    }
  }
}";
        let program = lower(src);
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let artifact =
            server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);
        let response = &artifact.routes[0].responses[0];
        let handlers = native_server_handlers_source(&artifact);
        let launcher = native_server_launcher_source(
            "server/app.orv-runtime.json",
            "server/native-server.json",
            &artifact,
        );

        assert_eq!(response.body_kind, "route_param_json");
        assert_eq!(response.body_route_params.len(), 4);
        assert_eq!(response.body_route_params[0].op.as_deref(), Some("sub"));
        assert_eq!(
            response.body_route_params[0].operand_json.as_deref(),
            Some("1")
        );
        assert_eq!(response.body_route_params[1].op.as_deref(), Some("mul"));
        assert_eq!(
            response.body_route_params[1].operand_json.as_deref(),
            Some("2")
        );
        assert_eq!(response.body_route_params[2].op.as_deref(), Some("div"));
        assert_eq!(
            response.body_route_params[2].operand_json.as_deref(),
            Some("2")
        );
        assert_eq!(response.body_route_params[3].op.as_deref(), Some("rem"));
        assert_eq!(
            response.body_route_params[3].operand_json.as_deref(),
            Some("2")
        );
        assert!(handlers.contains("value.checked_sub(1)"));
        assert!(handlers.contains("value.checked_mul(2)"));
        assert!(handlers.contains("value.checked_div(2)"));
        assert!(handlers.contains("value.checked_rem(2)"));
        assert!(!handlers.contains("native route body lowering pending"));
        assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
        assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
    }

    #[test]
    fn native_server_launcher_lowers_route_param_int_captured_arithmetic_response_body() {
        let src = r"@server {
  @listen 8080
  @route GET /products/:id/:offset {
    @respond 200 { shifted: (@param.id as int) + (@param.offset as int) }
  }
}";
        let program = lower(src);
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let artifact =
            server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);
        let response = &artifact.routes[0].responses[0];
        let handlers = native_server_handlers_source(&artifact);
        let launcher = native_server_launcher_source(
            "server/app.orv-runtime.json",
            "server/native-server.json",
            &artifact,
        );

        assert_eq!(response.body_kind, "route_param_json");
        assert_eq!(response.body_route_params[0].field, "shifted");
        assert_eq!(response.body_route_params[0].param, "id");
        assert_eq!(response.body_route_params[0].value_kind, "route_param_int");
        assert_eq!(response.body_route_params[0].op.as_deref(), Some("add"));
        assert_eq!(
            response.body_route_params[0].operand_kind.as_deref(),
            Some("route_param_int")
        );
        assert_eq!(
            response.body_route_params[0].operand_name.as_deref(),
            Some("offset")
        );
        assert!(handlers.contains("routes::orv_native_param_value(route_match, \"id\")"));
        assert!(handlers.contains("routes::orv_native_param_value(route_match, \"offset\")"));
        assert!(handlers.contains("value.checked_add(operand)"));
        assert!(!handlers.contains("native route body lowering pending"));
        assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
        assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
    }

    #[test]
    fn native_server_launcher_lowers_query_param_float_cast_response_body() {
        let src = r"@server {
  @listen 8080
  @route GET /search {
    @respond 200 { page: @query.page as float }
  }
}";
        let program = lower(src);
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let artifact =
            server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);
        let response = &artifact.routes[0].responses[0];
        let handlers = native_server_handlers_source(&artifact);
        let launcher = native_server_launcher_source(
            "server/app.orv-runtime.json",
            "server/native-server.json",
            &artifact,
        );

        assert_eq!(response.body_kind, "query_param_json");
        assert_eq!(response.body_query_params[0].field, "page");
        assert_eq!(response.body_query_params[0].param, "page");
        assert_eq!(
            response.body_query_params[0].value_kind,
            "query_param_float"
        );
        assert!(handlers.contains(".trim().parse::<f64>()"));
        assert!(handlers.contains("body.push_str(&value.to_string())"));
        assert!(!handlers.contains(
            "orv_native_push_json_string(routes::orv_native_query_value(route_match, \"page\")"
        ));
        assert!(!handlers.contains("native route body lowering pending"));
        assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
        assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
    }

    #[test]
    fn native_server_launcher_lowers_query_param_int_add_literal_response_body() {
        let src = r"@server {
  @listen 8080
  @route GET /search {
    @respond 200 { next: (@query.page as int) + 1 }
  }
}";
        let program = lower(src);
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let artifact =
            server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);
        let response = &artifact.routes[0].responses[0];
        let handlers = native_server_handlers_source(&artifact);
        let launcher = native_server_launcher_source(
            "server/app.orv-runtime.json",
            "server/native-server.json",
            &artifact,
        );

        assert_eq!(response.body_kind, "query_param_json");
        assert_eq!(response.body_query_params[0].field, "next");
        assert_eq!(response.body_query_params[0].param, "page");
        assert_eq!(response.body_query_params[0].value_kind, "query_param_int");
        assert_eq!(response.body_query_params[0].op.as_deref(), Some("add"));
        assert_eq!(
            response.body_query_params[0].operand_json.as_deref(),
            Some("1")
        );
        assert!(handlers.contains("routes::orv_native_query_value(route_match, \"page\")"));
        assert!(handlers.contains("value.checked_add(1)"));
        assert!(!handlers.contains("native route body lowering pending"));
        assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
        assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
    }

    #[test]
    fn native_server_launcher_lowers_query_param_int_captured_arithmetic_response_body() {
        let src = r"@server {
  @listen 8080
  @route GET /search {
    @respond 200 { next: (@query.page as int) + (@query.step as int) }
  }
}";
        let program = lower(src);
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let artifact =
            server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);
        let response = &artifact.routes[0].responses[0];
        let handlers = native_server_handlers_source(&artifact);
        let launcher = native_server_launcher_source(
            "server/app.orv-runtime.json",
            "server/native-server.json",
            &artifact,
        );

        assert_eq!(response.body_kind, "query_param_json");
        assert_eq!(response.body_query_params[0].field, "next");
        assert_eq!(response.body_query_params[0].param, "page");
        assert_eq!(response.body_query_params[0].value_kind, "query_param_int");
        assert_eq!(response.body_query_params[0].op.as_deref(), Some("add"));
        assert_eq!(
            response.body_query_params[0].operand_kind.as_deref(),
            Some("query_param_int")
        );
        assert_eq!(
            response.body_query_params[0].operand_name.as_deref(),
            Some("step")
        );
        assert!(handlers.contains("routes::orv_native_query_value(route_match, \"page\")"));
        assert!(handlers.contains("routes::orv_native_query_value(route_match, \"step\")"));
        assert!(handlers.contains("value.checked_add(operand)"));
        assert!(!handlers.contains("native route body lowering pending"));
        assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
        assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
    }

    #[test]
    fn native_server_launcher_lowers_query_param_int_static_arithmetic_response_body() {
        let src = r"@server {
  @listen 8080
  @route GET /search {
    @respond 200 {
      prev: (@query.page as int) - 1,
      doubled: (@query.page as int) * 2,
      half: (@query.page as int) / 2,
      parity: (@query.page as int) % 2
    }
  }
}";
        let program = lower(src);
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let artifact =
            server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);
        let response = &artifact.routes[0].responses[0];
        let handlers = native_server_handlers_source(&artifact);
        let launcher = native_server_launcher_source(
            "server/app.orv-runtime.json",
            "server/native-server.json",
            &artifact,
        );

        assert_eq!(response.body_kind, "query_param_json");
        assert_eq!(response.body_query_params.len(), 4);
        assert_eq!(response.body_query_params[0].op.as_deref(), Some("sub"));
        assert_eq!(
            response.body_query_params[0].operand_json.as_deref(),
            Some("1")
        );
        assert_eq!(response.body_query_params[1].op.as_deref(), Some("mul"));
        assert_eq!(
            response.body_query_params[1].operand_json.as_deref(),
            Some("2")
        );
        assert_eq!(response.body_query_params[2].op.as_deref(), Some("div"));
        assert_eq!(
            response.body_query_params[2].operand_json.as_deref(),
            Some("2")
        );
        assert_eq!(response.body_query_params[3].op.as_deref(), Some("rem"));
        assert_eq!(
            response.body_query_params[3].operand_json.as_deref(),
            Some("2")
        );
        assert!(handlers.contains("value.checked_sub(1)"));
        assert!(handlers.contains("value.checked_mul(2)"));
        assert!(handlers.contains("value.checked_div(2)"));
        assert!(handlers.contains("value.checked_rem(2)"));
        assert!(!handlers.contains("native route body lowering pending"));
        assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
        assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
    }

    #[test]
    fn native_server_launcher_lowers_simple_guarded_multi_response_route() {
        let src = r#"@server {
  @listen 8080
  @route POST /orders {
    if @body.sku == "" {
      @respond 400 { err: "missing_sku" }
    }
    @respond 201 { sku: @body.sku }
  }
}"#;
        let program = lower(src);
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let artifact =
            server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);
        let handlers = native_server_handlers_source(&artifact);
        let launcher = native_server_launcher_source(
            "server/app.orv-runtime.json",
            "server/native-server.json",
            &artifact,
        );

        assert_eq!(artifact.routes[0].responses.len(), 2);
        assert_eq!(
            artifact.routes[0].responses[0]
                .condition
                .as_ref()
                .map(|condition| condition.kind.as_str()),
            Some("request_body_field_eq")
        );
        assert_eq!(
            artifact.routes[0].responses[0]
                .condition
                .as_ref()
                .map(|condition| condition.name.as_str()),
            Some("sku")
        );
        assert_eq!(
            artifact.routes[0].responses[0]
                .condition
                .as_ref()
                .map(|condition| condition.value.as_str()),
            Some("")
        );
        assert!(artifact.routes[0].responses[1].condition.is_none());
        assert!(handlers
            .contains("routes::orv_native_body_field_value(route_match, \"sku\") == Some(\"\")"));
        assert!(handlers.contains("status: 400"));
        assert!(handlers.contains("status: 201"));
        assert!(!handlers.contains("native route body lowering pending"));
        assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
        assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
    }

    #[test]
    fn native_server_launcher_lowers_body_field_comparison_guard() {
        let src = r#"@server {
  @listen 8080
  @route POST /members {
    if @body.password != @body.confirm {
      @respond 400 { err: "password_mismatch" }
    }
    @respond 201 { email: @body.email }
  }
}"#;
        let program = lower(src);
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let artifact =
            server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);
        let handlers = native_server_handlers_source(&artifact);
        let launcher = native_server_launcher_source(
            "server/app.orv-runtime.json",
            "server/native-server.json",
            &artifact,
        );

        let condition = artifact.routes[0].responses[0]
            .condition
            .as_ref()
            .expect("guard condition");
        assert_eq!(condition.kind, "request_body_field_ne");
        assert_eq!(condition.name, "password");
        assert_eq!(condition.operand_name.as_deref(), Some("confirm"));
        assert!(artifact.routes[0].responses[1].condition.is_none());
        assert!(handlers.contains(
            "routes::orv_native_body_field_value(route_match, \"password\") != routes::orv_native_body_field_value(route_match, \"confirm\")"
        ));
        assert!(handlers.contains("status: 400"));
        assert!(handlers.contains("status: 201"));
        assert!(!handlers.contains("native route body lowering pending"));
        assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
        assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
    }

    #[test]
    fn native_server_launcher_lowers_body_field_query_param_comparison_guard() {
        let src = r#"@server {
  @listen 8080
  @route POST /sessions {
    if @body.token == @query.token {
      @respond 201 { ok: true }
    }
    @respond 401 { err: "token_mismatch" }
  }
}"#;
        let program = lower(src);
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let artifact =
            server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);
        let handlers = native_server_handlers_source(&artifact);
        let launcher = native_server_launcher_source(
            "server/app.orv-runtime.json",
            "server/native-server.json",
            &artifact,
        );

        let condition = artifact.routes[0].responses[0]
            .condition
            .as_ref()
            .expect("guard condition");
        assert_eq!(condition.kind, "request_body_field_eq");
        assert_eq!(condition.name, "token");
        assert_eq!(condition.operand_name.as_deref(), Some("token"));
        assert_eq!(condition.operand_kind.as_deref(), Some("query_param"));
        assert!(artifact.routes[0].responses[1].condition.is_none());
        assert!(handlers.contains(
            "routes::orv_native_body_field_value(route_match, \"token\") == routes::orv_native_query_value(route_match, \"token\")"
        ));
        assert!(!handlers.contains("native route body lowering pending"));
        assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
        assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
    }

    #[test]
    fn native_server_launcher_lowers_request_body_int_comparison_guard() {
        let src = r#"@server {
  @listen 8080
  @route POST /orders {
    if (@body.quantity as int) > 0 {
      @respond 201 { accepted: true, quantity: @body.quantity as int }
    }
    @respond 400 { err: "bad_quantity" }
  }
}"#;
        let program = lower(src);
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let artifact =
            server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);
        let handlers = native_server_handlers_source(&artifact);
        let launcher = native_server_launcher_source(
            "server/app.orv-runtime.json",
            "server/native-server.json",
            &artifact,
        );

        let condition = artifact.routes[0].responses[0]
            .condition
            .as_ref()
            .expect("guard condition");
        assert_eq!(condition.kind, "request_body_field_int_gt");
        assert_eq!(condition.name, "quantity");
        assert_eq!(condition.value, "0");
        assert!(condition.operand_name.is_none());
        assert!(condition.operand_kind.is_none());
        assert!(artifact.routes[0].responses[1].condition.is_none());
        assert!(handlers.contains(
            "routes::orv_native_body_field_value(route_match, \"quantity\").unwrap_or(\"\").trim().parse::<i64>()"
        ));
        assert!(handlers.contains("if value > 0 {"));
        assert!(!handlers.contains("native route body lowering pending"));
        assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
        assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
    }

    #[test]
    fn native_server_launcher_lowers_request_body_int_captured_comparison_guard() {
        let src = r#"@server {
  @listen 8080
  @route POST /orders {
    if (@body.quantity as int) <= (@body.stock as int) {
      @respond 201 { accepted: true, quantity: @body.quantity as int }
    }
    @respond 409 { err: "out_of_stock" }
  }
}"#;
        let program = lower(src);
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let artifact =
            server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);
        let handlers = native_server_handlers_source(&artifact);
        let launcher = native_server_launcher_source(
            "server/app.orv-runtime.json",
            "server/native-server.json",
            &artifact,
        );

        let condition = artifact.routes[0].responses[0]
            .condition
            .as_ref()
            .expect("guard condition");
        assert_eq!(condition.kind, "request_body_field_int_le");
        assert_eq!(condition.name, "quantity");
        assert_eq!(condition.operand_name.as_deref(), Some("stock"));
        assert_eq!(
            condition.operand_kind.as_deref(),
            Some("request_body_field_int")
        );
        assert!(artifact.routes[0].responses[1].condition.is_none());
        assert!(handlers.contains(
            "routes::orv_native_body_field_value(route_match, \"quantity\").unwrap_or(\"\").trim().parse::<i64>()"
        ));
        assert!(handlers.contains(
            "routes::orv_native_body_field_value(route_match, \"stock\").unwrap_or(\"\").trim().parse::<i64>()"
        ));
        assert!(handlers.contains("(Ok(value), Ok(operand)) => value <= operand"));
        assert!(!handlers.contains("native route body lowering pending"));
        assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
        assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
    }

    #[test]
    fn native_server_launcher_lowers_request_body_float_comparison_guard() {
        let src = r#"@server {
  @listen 8080
  @route POST /payments {
    if (@body.amount as float) > 0.0 {
      @respond 201 { accepted: true, amount: @body.amount as float }
    }
    @respond 400 { err: "bad_amount" }
  }
}"#;
        let program = lower(src);
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let artifact =
            server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);
        let handlers = native_server_handlers_source(&artifact);
        let launcher = native_server_launcher_source(
            "server/app.orv-runtime.json",
            "server/native-server.json",
            &artifact,
        );

        let condition = artifact.routes[0].responses[0]
            .condition
            .as_ref()
            .expect("guard condition");
        assert_eq!(condition.kind, "request_body_field_float_gt");
        assert_eq!(condition.name, "amount");
        assert_eq!(condition.value, "0.0");
        assert!(condition.operand_name.is_none());
        assert!(condition.operand_kind.is_none());
        assert!(artifact.routes[0].responses[1].condition.is_none());
        assert!(handlers.contains(
            "routes::orv_native_body_field_value(route_match, \"amount\").unwrap_or(\"\").trim().parse::<f64>()"
        ));
        assert!(handlers.contains("Ok(value) if value.is_finite() => value > 0.0"));
        assert!(!handlers.contains("native route body lowering pending"));
        assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
        assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
    }

    #[test]
    fn native_server_launcher_lowers_request_body_float_captured_comparison_guard() {
        let src = r#"@server {
  @listen 8080
  @route POST /payments {
    if (@body.amount as float) <= (@query.limit as float) {
      @respond 201 { accepted: true, amount: @body.amount as float }
    }
    @respond 409 { err: "amount_over_limit" }
  }
}"#;
        let program = lower(src);
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let artifact =
            server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);
        let handlers = native_server_handlers_source(&artifact);
        let launcher = native_server_launcher_source(
            "server/app.orv-runtime.json",
            "server/native-server.json",
            &artifact,
        );

        let condition = artifact.routes[0].responses[0]
            .condition
            .as_ref()
            .expect("guard condition");
        assert_eq!(condition.kind, "request_body_field_float_le");
        assert_eq!(condition.name, "amount");
        assert_eq!(condition.operand_name.as_deref(), Some("limit"));
        assert_eq!(condition.operand_kind.as_deref(), Some("query_param_float"));
        assert!(artifact.routes[0].responses[1].condition.is_none());
        assert!(handlers.contains(
            "routes::orv_native_body_field_value(route_match, \"amount\").unwrap_or(\"\").trim().parse::<f64>()"
        ));
        assert!(handlers.contains(
            "routes::orv_native_query_value(route_match, \"limit\").unwrap_or(\"\").trim().parse::<f64>()"
        ));
        assert!(handlers.contains(
            "(Ok(value), Ok(operand)) if value.is_finite() && operand.is_finite() => value <= operand"
        ));
        assert!(!handlers.contains("native route body lowering pending"));
        assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
        assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
    }

    #[test]
    fn native_server_launcher_lowers_route_param_static_guard() {
        let src = r#"@server {
  @listen 8080
  @route GET /products/:kind {
    if @param.kind == "sale" {
      @respond 200 { kind: @param.kind }
    }
    @respond 200 { kind: "regular" }
  }
}"#;
        let program = lower(src);
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let artifact =
            server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);
        let handlers = native_server_handlers_source(&artifact);
        let launcher = native_server_launcher_source(
            "server/app.orv-runtime.json",
            "server/native-server.json",
            &artifact,
        );

        let condition = artifact.routes[0].responses[0]
            .condition
            .as_ref()
            .expect("guard condition");
        assert_eq!(condition.kind, "route_param_eq");
        assert_eq!(condition.name, "kind");
        assert_eq!(condition.value, "sale");
        assert!(artifact.routes[0].responses[1].condition.is_none());
        assert!(handlers
            .contains("routes::orv_native_param_value(route_match, \"kind\") == Some(\"sale\")"));
        assert!(!handlers.contains("native route body lowering pending"));
        assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
        assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
    }

    #[test]
    fn native_server_launcher_lowers_query_param_static_guard() {
        let src = r#"@server {
  @listen 8080
  @route GET /search {
    if @query.mode != "compact" {
      @respond 200 { mode: @query.mode }
    }
    @respond 200 { mode: "compact" }
  }
}"#;
        let program = lower(src);
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let artifact =
            server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);
        let handlers = native_server_handlers_source(&artifact);
        let launcher = native_server_launcher_source(
            "server/app.orv-runtime.json",
            "server/native-server.json",
            &artifact,
        );

        let condition = artifact.routes[0].responses[0]
            .condition
            .as_ref()
            .expect("guard condition");
        assert_eq!(condition.kind, "query_param_ne");
        assert_eq!(condition.name, "mode");
        assert_eq!(condition.value, "compact");
        assert!(artifact.routes[0].responses[1].condition.is_none());
        assert!(handlers.contains(
            "routes::orv_native_query_value(route_match, \"mode\") != Some(\"compact\")"
        ));
        assert!(!handlers.contains("native route body lowering pending"));
        assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
        assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
    }

    #[test]
    fn native_server_routes_source_declares_typed_route_table() {
        let src = r"@server {
  @listen 8080
  @route GET /ping {
    @respond 200 { ok: true }
  }
}";
        let program = lower(src);
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let artifact = server_runtime_artifact(&manifest, &map, [("server.orv", src)]);
        let source = native_server_routes_source(&artifact);
        let route_origin = &artifact.routes[0].origin_id;

        assert!(source.contains("pub struct OrvNativeRoute"));
        assert!(source.contains("pub response_origin_ids: &'static [&'static str]"));
        assert!(source.contains("pub const ORV_NATIVE_ROUTES"));
        assert!(source.contains("OrvNativeRoute { method: \"GET\", path: \"/ping\", origin_id:"));
        assert!(source.contains("pub fn orv_native_match_route("));
        assert!(source.contains("orv_native_route_path_params(route.path, path)"));
        assert!(source.contains(&format!("origin_id: \"{route_origin}\"")));
        assert!(source.contains(&format!(
            "response_origin_ids: &[{}]",
            rust_string_literal(&artifact.routes[0].response_origin_ids[0])
        )));
        assert!(
            source.contains("pub const ORV_NATIVE_ROUTE_COUNT: usize = ORV_NATIVE_ROUTES.len();")
        );
    }

    #[test]
    fn native_server_routes_source_generates_param_route_matcher() {
        let src = r"@server {
  @listen 8080
  @route GET /products/:sku {
    @respond 200 { ok: true }
  }
}";
        let program = lower(src);
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let artifact = server_runtime_artifact(&manifest, &map, [("server.orv", src)]);
        let source = native_server_routes_source(&artifact);

        assert!(source.contains("path: \"/products/:sku\""));
        assert!(source.contains("orv_native_route_path_params(route.path, path)"));
        assert!(
            source.contains("fn orv_native_route_path_params(pattern: &'static str, path: &str)")
        );
        assert!(source.contains("pattern_segment.strip_prefix(':')"));
        assert!(source.contains("pattern_segments.len() != path_segments.len()"));
    }

    #[test]
    fn native_server_routes_source_generates_param_capture_contract() {
        let src = r"@server {
  @listen 8080
  @route GET /products/:sku {
    @respond 200 { ok: true }
  }
}";
        let program = lower(src);
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let artifact = server_runtime_artifact(&manifest, &map, [("server.orv", src)]);
        let source = native_server_routes_source(&artifact);

        assert!(source.contains("pub struct OrvNativeRouteMatch"));
        assert!(source.contains("pub struct OrvNativeParam"));
        assert!(source.contains("pub params: Vec<OrvNativeParam>"));
        assert!(source.contains("OrvNativeParam {"));
        assert!(source.contains("if let Some(name) = pattern_segment.strip_prefix(':')"));
        assert!(source.contains("name,"));
        assert!(source.contains("value: (*path_segment).to_string()"));
    }

    #[test]
    fn native_server_routes_source_generates_param_lookup_helper() {
        let src = r"@server {
  @listen 8080
  @route GET /products/:sku {
    @respond 200 { ok: true }
  }
}";
        let program = lower(src);
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let artifact = server_runtime_artifact(&manifest, &map, [("server.orv", src)]);
        let source = native_server_routes_source(&artifact);

        assert!(source.contains("#[allow(dead_code)]"));
        assert!(source.contains("pub fn orv_native_param_value<'a>("));
        assert!(source.contains("route_match: &'a OrvNativeRouteMatch"));
        assert!(source.contains(") -> Option<&'a str>"));
        assert!(source.contains(".find(|param| param.name == name)"));
        assert!(source.contains(".map(|param| param.value.as_str())"));
    }

    #[test]
    fn native_server_router_source_generates_dispatch_contract() {
        let source = native_server_router_source();

        assert!(source.contains("use crate::{handlers, routes};"));
        assert!(source.contains("pub struct OrvNativeDispatch"));
        assert!(source.contains(
            "pub const ORV_NATIVE_HANDLER_COUNT: usize = handlers::ORV_NATIVE_HANDLER_COUNT;"
        ));
        assert!(source.contains("pub fn orv_native_dispatch(method: &str, path: &str)"));
        assert!(source.contains("routes::orv_native_match_route(method, path)"));
        assert!(source.contains("handlers::orv_native_handle_route(&route_match)"));
        assert!(source.contains("origin_id: response.origin_id"));
        assert!(source.contains("pub response_origin_id: Option<&'static str>"));
        assert!(source.contains("response_origin_id: response.response_origin_id"));
        assert!(source.contains("params: response.params"));
        assert!(source.contains("status: 404"));
    }

    #[test]
    fn native_server_handlers_source_generates_response_origin_contract() {
        let src = r"@server {
  @listen 8080
  @route GET /products/:sku {
    @respond 200 { ok: true }
  }
}";
        let program = lower(src);
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let artifact = server_runtime_artifact(&manifest, &map, [("server.orv", src)]);
        let source = native_server_handlers_source(&artifact);
        let response_origin = &artifact.routes[0].response_origin_ids[0];

        assert!(source.contains("use crate::routes;"));
        assert!(source.contains("pub struct OrvNativeHandlerDescriptor"));
        assert!(source.contains("pub struct OrvNativeHandlerResponse"));
        assert!(source.contains("pub const ORV_NATIVE_HANDLERS"));
        assert!(source.contains("pub const ORV_NATIVE_HANDLER_COUNT"));
        assert!(source.contains("pub fn orv_native_handle_route("));
        assert!(source.contains("route_origin_id:"));
        assert!(source.contains(response_origin));
        assert!(source.contains("status: 501"));
        assert!(source.contains("native route body lowering pending"));
        assert!(source.contains(
            "response_origin_id: route_match.route.response_origin_ids.first().copied()"
        ));
    }

    #[test]
    fn native_server_routes_source_generates_named_wildcard_matcher() {
        let src = r"@server {
  @listen 8080
  @route GET /assets/:rest* {
    @respond 200 { ok: true }
  }
}";
        let program = lower(src);
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let artifact = server_runtime_artifact(&manifest, &map, [("server.orv", src)]);
        let source = native_server_routes_source(&artifact);

        assert!(source.contains("path: \"/assets/:rest*\""));
        assert!(source.contains("strip_prefix(':')"));
        assert!(source.contains("segment.strip_suffix('*')"));
        assert!(source.contains("path_segments.len() <= prefix_len"));
        assert!(source.contains("take(prefix_len)"));
    }

    #[test]
    fn native_server_launcher_source_declares_direct_http_dispatch() {
        let src = r"@server {
  @listen 8080
  @route GET /ping {
    @respond 200 { ok: true }
  }
}";
        let program = lower(src);
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let artifact =
            server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);
        let source = native_server_launcher_source(
            "server/app.orv-runtime.json",
            "server/native-server.json",
            &artifact,
        );

        assert!(source.contains("mod routes;"));
        assert!(source.contains("mod router;"));
        assert!(
            source.contains(r#"const ORV_SERVER_ARTIFACT: &str = "server/app.orv-runtime.json";"#)
        );
        assert!(
            source.contains(r#"const ORV_NATIVE_SERVER_PLAN: &str = "server/native-server.json";"#)
        );
        assert!(source.contains("routes::ORV_NATIVE_ROUTE_COUNT"));
        assert!(
            source.contains(r#"routes::orv_native_match_route("__orv_probe__", "__orv_probe__")"#)
        );
        assert!(source.contains("router::ORV_NATIVE_HANDLER_COUNT"));
        assert!(source.contains(r#"router::orv_native_dispatch("__orv_probe__", "__orv_probe__")"#));
        assert!(source.contains("native_plan.is_file()"));
        assert!(source.contains("artifact.is_file()"));
        assert!(source.contains("fn orv_build_dir() -> std::path::PathBuf"));
        assert!(source.contains(r#"std::env::var_os("ORV_BUILD_DIR")"#));
        assert!(source.contains("std::env::current_exe()"));
        assert!(source.contains("path.parent()?.parent()?.parent()?.parent()?.parent()"));
        assert!(source.contains("fn orv_native_serve() -> std::io::Result<()>"));
        assert!(source.contains("std::net::TcpListener::bind(orv_native_listen_address())"));
        assert!(source.contains("const ORV_DEFAULT_PORT: u16 = 8080;"));
        assert!(source.contains("router::orv_native_dispatch_with_request("));
        assert!(source.contains("request.body"));
        assert!(source.contains("fn orv_native_http_response("));
        assert!(!source.contains(r#"std::process::Command::new("orv")"#));
        assert!(!source.contains(r#".arg("run-artifact")"#));
    }

    #[test]
    fn native_server_launcher_source_uses_reference_fallback_for_dynamic_handlers() {
        let src = r"@server {
  @listen 8080
  @route POST /echo {
    @respond 201 { received: (@body.id as int) ** (@body.extra as int) }
  }
}";
        let program = lower(src);
        let map = origin_map(&program);
        let manifest = build_manifest("server.orv", &map);
        let artifact =
            server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);

        let source = native_server_launcher_source(
            "server/app.orv-runtime.json",
            "server/native-server.json",
            &artifact,
        );

        assert!(source.contains("fn orv_native_reference_bridge("));
        assert!(source.contains(r#"std::process::Command::new("orv")"#));
        assert!(source.contains(r#".arg("run-artifact")"#));
        assert!(source.contains("std::env::args_os().skip(1)"));
        assert!(!source.contains("fn orv_native_serve() -> std::io::Result<()>"));
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
