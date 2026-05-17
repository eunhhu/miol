use std::collections::HashMap;

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
    /// Route-local security and traffic policies discovered from the handler.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub policies: Vec<ServerRoutePolicyArtifact>,
}

/// One route-local policy advertised by server/deploy/native artifacts.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ServerRoutePolicyArtifact {
    /// Policy kind, for example `csrf`, `session`, `auth`, or `rate_limit`.
    pub kind: String,
    /// Source origin id for declarative policy domains.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_id: Option<String>,
    /// Whether the policy requires an authenticated/session-backed request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
    /// Required role for auth role policies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Static rate-limit key expression summary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    /// Whether this policy explicitly disables a built-in default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exempt: Option<bool>,
    /// Request limit for built-in route rate limits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Rate-limit window in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_seconds: Option<u32>,
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
    /// Static numeric scale applied to the dynamic comparison operand before comparing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operand_scale_json: Option<String>,
    /// Secondary captured operand source for product-shaped dynamic comparison operands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secondary_operand_kind: Option<String>,
    /// Secondary captured operand name for product-shaped dynamic comparison operands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secondary_operand_name: Option<String>,
    /// Tertiary captured operand source for product-product dynamic comparison operands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tertiary_operand_kind: Option<String>,
    /// Tertiary captured operand name for product-product dynamic comparison operands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tertiary_operand_name: Option<String>,
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
    /// Secondary captured operand value class for product-shaped dynamic numeric arithmetic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secondary_operand_kind: Option<String>,
    /// Secondary captured operand field name for product-shaped dynamic numeric arithmetic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secondary_operand_name: Option<String>,
    /// Tertiary captured operand value class for product-product dynamic numeric arithmetic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tertiary_operand_kind: Option<String>,
    /// Tertiary captured operand field name for product-product dynamic numeric arithmetic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tertiary_operand_name: Option<String>,
}

/// One JSON object field backed by a captured route param.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ServerResponseRouteParamArtifact {
    /// JSON field name in the response body.
    pub field: String,
    /// Captured route parameter name.
    pub param: String,
    /// Field value class: `route_param`, `route_param_int`, `route_param_float`, or `route_param_bool`.
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
    /// Secondary captured operand value class for product-shaped dynamic numeric arithmetic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secondary_operand_kind: Option<String>,
    /// Secondary captured operand field name for product-shaped dynamic numeric arithmetic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secondary_operand_name: Option<String>,
    /// Tertiary captured operand value class for product-product dynamic numeric arithmetic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tertiary_operand_kind: Option<String>,
    /// Tertiary captured operand field name for product-product dynamic numeric arithmetic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tertiary_operand_name: Option<String>,
}

/// One JSON object field backed by a captured query param.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ServerResponseQueryParamArtifact {
    /// JSON field name in the response body.
    pub field: String,
    /// Captured query parameter name.
    pub param: String,
    /// Field value class: `query_param`, `query_param_int`, `query_param_float`, or `query_param_bool`.
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
    /// Secondary captured operand value class for product-shaped dynamic numeric arithmetic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secondary_operand_kind: Option<String>,
    /// Secondary captured operand field name for product-shaped dynamic numeric arithmetic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secondary_operand_name: Option<String>,
    /// Tertiary captured operand value class for product-product dynamic numeric arithmetic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tertiary_operand_kind: Option<String>,
    /// Tertiary captured operand field name for product-product dynamic numeric arithmetic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tertiary_operand_name: Option<String>,
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
    /// Field value class: `request_body_field`, `request_body_field_int`, `request_body_field_float`, or `request_body_field_bool`.
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
    /// Secondary captured operand class for product-shaped dynamic arithmetic operands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secondary_operand_kind: Option<String>,
    /// Secondary captured operand field name for product-shaped dynamic arithmetic operands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secondary_operand_name: Option<String>,
    /// Tertiary captured operand class for product-product dynamic arithmetic operands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tertiary_operand_kind: Option<String>,
    /// Tertiary captured operand field name for product-product dynamic arithmetic operands.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tertiary_operand_name: Option<String>,
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
