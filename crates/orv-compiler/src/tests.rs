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

    assert!(map
        .edges
        .iter()
        .any(|edge| { edge.kind == "contains" && edge.from == server.id && edge.to == listen.id }));
    assert!(map
        .edges
        .iter()
        .any(|edge| edge.kind == "contains" && edge.from == server.id && edge.to == route.id));
    assert!(map
        .edges
        .iter()
        .any(|edge| { edge.kind == "contains" && edge.from == route.id && edge.to == respond.id }));
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
fn build_manifest_declares_security_runtime_features() {
    let program = lower(
        r#"@server {
  @listen 8080
  @route GET /admin {
    @Auth required role="admin"
    @respond 200 { ok: true }
  }
  @route GET /account/sessions {
    @session required
    @respond 200 { sessionId: @session.id }
  }
  @route POST /members/login {
    @csrf
    @respond 201 { ok: true }
  }
  @route POST /checkout {
    @csrf
    @respond 201 { ok: true }
  }
}"#,
    );
    let map = origin_map(&program);
    let manifest = build_manifest("server.orv", &map);

    for feature in [
        "auth_roles",
        "csrf_protection",
        "rate_limit",
        "session_cookies",
    ] {
        assert!(
            manifest
                .capabilities
                .runtime_features
                .contains(&feature.to_string()),
            "missing {feature} in {:?}",
            manifest.capabilities.runtime_features
        );
    }
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
        artifact.kind == "native_server_launcher_source" && artifact.path == "server/native/main.rs"
    }));
    assert!(manifest.artifacts.iter().any(|artifact| {
        artifact.kind == "native_runtime_image_dockerfile"
            && artifact.path == "server/native/Dockerfile"
    }));
    assert!(manifest.artifacts.iter().any(|artifact| {
        artifact.kind == "native_server_routes_source" && artifact.path == "server/native/routes.rs"
    }));
    assert!(manifest.artifacts.iter().any(|artifact| {
        artifact.kind == "native_server_router_source" && artifact.path == "server/native/router.rs"
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
    assert!(manifest
        .artifacts
        .iter()
        .any(|artifact| { artifact.kind == "static_page" && artifact.path == "pages/index.html" }));
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
fn server_runtime_artifact_records_route_security_policies() {
    let src = r#"@server {
  @listen 8080
  @route GET /admin {
    @Auth required role="admin"
    @respond 200 { ok: true }
  }
  @route GET /account/sessions {
    @session required
    @respond 200 { ok: true }
  }
  @route POST /checkout {
    @csrf
    @respond 201 { ok: true }
  }
}"#;
    let program = lower(src);
    let map = origin_map(&program);
    let manifest = build_manifest("server.orv", &map);
    let artifact =
        server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);

    let admin = artifact
        .routes
        .iter()
        .find(|route| route.path == "/admin")
        .expect("admin route");
    assert!(admin.policies.iter().any(|policy| {
        policy.kind == "auth"
            && policy.required == Some(true)
            && policy.role.as_deref() == Some("admin")
            && policy
                .origin_id
                .as_deref()
                .is_some_and(|origin_id| origin_id.starts_with("ori_"))
    }));

    let sessions = artifact
        .routes
        .iter()
        .find(|route| route.path == "/account/sessions")
        .expect("sessions route");
    assert!(sessions.policies.iter().any(|policy| {
        policy.kind == "session"
            && policy.required == Some(true)
            && policy
                .origin_id
                .as_deref()
                .is_some_and(|origin_id| origin_id.starts_with("ori_"))
    }));

    let checkout = artifact
        .routes
        .iter()
        .find(|route| route.path == "/checkout")
        .expect("checkout route");
    assert!(checkout.policies.iter().any(|policy| {
        policy.kind == "csrf"
            && policy.required == Some(true)
            && policy
                .origin_id
                .as_deref()
                .is_some_and(|origin_id| origin_id.starts_with("ori_"))
    }));
    assert!(checkout.policies.iter().any(|policy| {
        policy.kind == "rate_limit"
            && policy.origin_id.is_none()
            && policy.limit == Some(10)
            && policy.window_seconds == Some(60)
    }));

    verify_server_runtime_artifact(&artifact).expect("policy artifact verifies");
}

#[test]
fn server_runtime_artifact_records_explicit_rate_limit_policy() {
    let src = r#"@server {
  @listen 8080
  @route POST /limited {
    @rateLimit key=@body.memberId limit=2 window="1m"
    @respond 201 { ok: true }
  }
}"#;
    let program = lower(src);
    let map = origin_map(&program);
    let manifest = build_manifest("server.orv", &map);
    let artifact =
        server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);
    let route = artifact
        .routes
        .iter()
        .find(|route| route.path == "/limited")
        .expect("limited route");

    assert!(artifact
        .runtime_features
        .contains(&"rate_limit".to_string()));
    assert_eq!(route.policies.len(), 1);
    let policy = &route.policies[0];
    assert_eq!(policy.kind, "rate_limit");
    assert!(policy
        .origin_id
        .as_deref()
        .is_some_and(|origin_id| origin_id.starts_with("ori_")));
    assert_eq!(policy.key.as_deref(), Some("@body.memberId"));
    assert_eq!(policy.limit, Some(2));
    assert_eq!(policy.window_seconds, Some(60));
    assert_eq!(policy.exempt, None);
    verify_server_runtime_artifact(&artifact).expect("rate-limit policy verifies");
}

#[test]
fn server_runtime_artifact_records_rate_limit_exemption_without_default() {
    let src = r#"@server {
  @listen 8080
  @route POST /checkout {
    @rateLimit exempt
    @respond 201 { ok: true }
  }
}"#;
    let program = lower(src);
    let map = origin_map(&program);
    let manifest = build_manifest("server.orv", &map);
    let artifact =
        server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);
    let route = artifact
        .routes
        .iter()
        .find(|route| route.path == "/checkout")
        .expect("checkout route");

    assert_eq!(route.policies.len(), 1);
    let policy = &route.policies[0];
    assert_eq!(policy.kind, "rate_limit");
    assert!(policy
        .origin_id
        .as_deref()
        .is_some_and(|origin_id| origin_id.starts_with("ori_")));
    assert_eq!(policy.exempt, Some(true));
    assert_eq!(policy.limit, None);
    assert_eq!(policy.window_seconds, None);
    verify_server_runtime_artifact(&artifact).expect("exempt policy verifies");
}

#[test]
fn server_runtime_artifact_records_csrf_exemption_policy() {
    let src = r#"@server {
  @listen 8080
  @route POST /webhooks/custom {
    @csrf exempt
    @respond 200 { ok: true }
  }
}"#;
    let program = lower(src);
    let map = origin_map(&program);
    let manifest = build_manifest("server.orv", &map);
    let artifact =
        server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);
    let route = artifact
        .routes
        .iter()
        .find(|route| route.path == "/webhooks/custom")
        .expect("webhook route");

    assert_eq!(route.policies.len(), 1);
    let policy = &route.policies[0];
    assert_eq!(policy.kind, "csrf");
    assert!(policy
        .origin_id
        .as_deref()
        .is_some_and(|origin_id| origin_id.starts_with("ori_")));
    assert_eq!(policy.required, Some(false));
    assert_eq!(policy.exempt, Some(true));
    verify_server_runtime_artifact(&artifact).expect("csrf exempt policy verifies");
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
fn server_runtime_artifact_lowers_route_param_with_static_suffix_response_body() {
    let src = r"@server {
  @listen 8080
  @route GET /calendar/:userId.ics {
    @respond 200 { id: @param.userId }
  }
}";
    let program = lower(src);
    let map = origin_map(&program);
    let manifest = build_manifest("server.orv", &map);
    let artifact =
        server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);
    let response = &artifact.routes[0].responses[0];
    let handlers = native_server_handlers_source(&artifact);
    let routes = native_server_routes_source(&artifact);
    let launcher = native_server_launcher_source(
        "server/app.orv-runtime.json",
        "server/native-server.json",
        &artifact,
    );

    assert_eq!(response.status, Some(200));
    assert_eq!(response.body_kind, "route_param_json");
    assert_eq!(response.body_route_params[0].param, "userId");
    assert!(handlers.contains("routes::orv_native_param_value(route_match, \"userId\")"));
    assert!(routes.contains("orv_native_route_param_segment"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
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
fn native_server_launcher_lowers_mixed_dynamic_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { sku: @body.sku, coupon: @query.coupon }
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

    assert_eq!(response.body_kind, "mixed_json");
    assert_eq!(response.body_object_fields.len(), 2);
    assert_eq!(response.body_object_fields[0].field, "sku");
    assert_eq!(
        response.body_object_fields[0].value_kind,
        "request_body_field"
    );
    assert_eq!(response.body_object_fields[0].name.as_deref(), Some("sku"));
    assert_eq!(response.body_object_fields[1].field, "coupon");
    assert_eq!(response.body_object_fields[1].value_kind, "query_param");
    assert_eq!(
        response.body_object_fields[1].name.as_deref(),
        Some("coupon")
    );
    assert!(handlers.contains("routes::orv_native_body_field_value(route_match, \"sku\")"));
    assert!(handlers.contains("routes::orv_native_query_value(route_match, \"coupon\")"));
    assert!(handlers.contains("orv_native_push_json_string("));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_request_body_query_string_eq_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /sessions {
    @respond 201 { matches: @body.token == @query.token }
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
    assert_eq!(response.body_request_fields[0].field, "matches");
    assert_eq!(response.body_request_fields[0].name, "token");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field"
    );
    assert_eq!(response.body_request_fields[0].op.as_deref(), Some("eq"));
    assert_eq!(
        response.body_request_fields[0].operand_kind.as_deref(),
        Some("query_param")
    );
    assert_eq!(
        response.body_request_fields[0].operand_name.as_deref(),
        Some("token")
    );
    assert!(handlers.contains("if value == operand"));
    assert!(handlers.contains("body.push_str(\"true\")"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_request_body_query_string_concat_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /sessions {
    @respond 201 { label: @body.first + @query.suffix }
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
    assert_eq!(response.body_request_fields[0].field, "label");
    assert_eq!(response.body_request_fields[0].name, "first");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field"
    );
    assert_eq!(
        response.body_request_fields[0].op.as_deref(),
        Some("concat")
    );
    assert_eq!(
        response.body_request_fields[0].operand_kind.as_deref(),
        Some("query_param")
    );
    assert_eq!(
        response.body_request_fields[0].operand_name.as_deref(),
        Some("suffix")
    );
    assert!(handlers.contains("value.push_str(operand)"));
    assert!(handlers.contains("orv_native_push_json_string(&value, &mut body)"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_static_prefix_string_concat_response_body() {
    let src = r#"@server {
  @listen 8080
  @route POST /products {
    @respond 201 { label: "sku-" + @body.sku }
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
    assert_eq!(response.body_request_fields[0].field, "label");
    assert_eq!(response.body_request_fields[0].name, "sku");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field"
    );
    assert_eq!(
        response.body_request_fields[0].op.as_deref(),
        Some("concat_prefix")
    );
    assert_eq!(
        response.body_request_fields[0].operand_json.as_deref(),
        Some("sku-")
    );
    assert!(handlers.contains("let mut value = String::from(\"sku-\")"));
    assert!(handlers.contains("value.push_str(routes::orv_native_body_field_value"));
    assert!(handlers.contains("orv_native_push_json_string(&value, &mut body)"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_static_prefix_string_interpolation_response_body() {
    let src = r#"@server {
  @listen 8080
  @route POST /products {
    @respond 201 { label: "sku-{@body.sku}" }
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
    assert_eq!(response.body_request_fields[0].field, "label");
    assert_eq!(response.body_request_fields[0].name, "sku");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field"
    );
    assert_eq!(
        response.body_request_fields[0].op.as_deref(),
        Some("concat_prefix")
    );
    assert_eq!(
        response.body_request_fields[0].operand_json.as_deref(),
        Some("sku-")
    );
    assert!(handlers.contains("let mut value = String::from(\"sku-\")"));
    assert!(handlers.contains("value.push_str(routes::orv_native_body_field_value"));
    assert!(handlers.contains("orv_native_push_json_string(&value, &mut body)"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_static_suffix_string_interpolation_response_body() {
    let src = r#"@server {
  @listen 8080
  @route POST /products {
    @respond 201 { label: "{@body.sku}-sku" }
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
    assert_eq!(response.body_request_fields[0].field, "label");
    assert_eq!(response.body_request_fields[0].name, "sku");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field"
    );
    assert_eq!(
        response.body_request_fields[0].op.as_deref(),
        Some("concat")
    );
    assert_eq!(
        response.body_request_fields[0].operand_json.as_deref(),
        Some("-sku")
    );
    assert!(handlers.contains("let mut value = String::from(routes::orv_native_body_field_value"));
    assert!(handlers.contains("value.push_str(\"-sku\")"));
    assert!(handlers.contains("orv_native_push_json_string(&value, &mut body)"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_static_affix_string_interpolation_response_body() {
    let src = r#"@server {
  @listen 8080
  @route POST /products {
    @respond 201 { label: "sku-{@body.sku}-v1" }
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
    assert_eq!(response.body_request_fields[0].field, "label");
    assert_eq!(response.body_request_fields[0].name, "sku");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field"
    );
    assert_eq!(
        response.body_request_fields[0].op.as_deref(),
        Some("concat_affix")
    );
    assert!(handlers.contains("let mut value = String::from(\"sku-\")"));
    assert!(handlers.contains("value.push_str(routes::orv_native_body_field_value"));
    assert!(handlers.contains("value.push_str(\"-v1\")"));
    assert!(handlers.contains("orv_native_push_json_string(&value, &mut body)"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_joined_string_interpolation_response_body() {
    let src = r#"@server {
  @listen 8080
  @route POST /labels {
    @respond 201 { label: "{@body.first}-{@query.suffix}" }
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
    assert_eq!(response.body_request_fields[0].field, "label");
    assert_eq!(response.body_request_fields[0].name, "first");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field"
    );
    assert_eq!(
        response.body_request_fields[0].op.as_deref(),
        Some("concat_join")
    );
    assert_eq!(
        response.body_request_fields[0].operand_json.as_deref(),
        Some("-")
    );
    assert_eq!(
        response.body_request_fields[0].operand_kind.as_deref(),
        Some("query_param")
    );
    assert_eq!(
        response.body_request_fields[0].operand_name.as_deref(),
        Some("suffix")
    );
    assert!(handlers.contains("value.push_str(\"-\")"));
    assert!(handlers.contains("value.push_str(operand)"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_static_left_int_add_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { next: 1 + (@body.quantity as int) }
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
    assert_eq!(response.body_request_fields[0].field, "next");
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
    assert!(handlers.contains("match value.checked_add(1)"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_static_left_int_sub_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { remaining: 10 - (@body.quantity as int) }
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
    assert_eq!(response.body_request_fields[0].field, "remaining");
    assert_eq!(response.body_request_fields[0].name, "quantity");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field_int"
    );
    assert_eq!(response.body_request_fields[0].op.as_deref(), Some("rsub"));
    assert_eq!(
        response.body_request_fields[0].operand_json.as_deref(),
        Some("10")
    );
    assert!(handlers.contains("match 10_i64.checked_sub(value)"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_static_left_int_div_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { unit: 100 / (@body.parts as int) }
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
    assert_eq!(response.body_request_fields[0].field, "unit");
    assert_eq!(response.body_request_fields[0].name, "parts");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field_int"
    );
    assert_eq!(response.body_request_fields[0].op.as_deref(), Some("rdiv"));
    assert_eq!(
        response.body_request_fields[0].operand_json.as_deref(),
        Some("100")
    );
    assert!(handlers.contains("match 100_i64.checked_div(value)"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_static_left_int_rem_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { remainder: 10 % (@body.parts as int) }
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
    assert_eq!(response.body_request_fields[0].name, "parts");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field_int"
    );
    assert_eq!(response.body_request_fields[0].op.as_deref(), Some("rrem"));
    assert_eq!(
        response.body_request_fields[0].operand_json.as_deref(),
        Some("10")
    );
    assert!(handlers.contains("match 10_i64.checked_rem(value)"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_static_left_int_pow_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { total: 2 ** (@body.exp as int) }
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
    assert_eq!(response.body_request_fields[0].name, "exp");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field_int"
    );
    assert_eq!(response.body_request_fields[0].op.as_deref(), Some("rpow"));
    assert_eq!(
        response.body_request_fields[0].operand_json.as_deref(),
        Some("2")
    );
    assert!(handlers.contains("2_i64.checked_pow(u32::try_from(value).unwrap_or(0))"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_static_left_int_mul_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { doubled: 2 * (@body.quantity as int) }
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
    assert_eq!(response.body_request_fields[0].field, "doubled");
    assert_eq!(response.body_request_fields[0].name, "quantity");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field_int"
    );
    assert_eq!(response.body_request_fields[0].op.as_deref(), Some("mul"));
    assert_eq!(
        response.body_request_fields[0].operand_json.as_deref(),
        Some("2")
    );
    assert!(handlers.contains("match value.checked_mul(2)"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_static_left_int_comparison_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { below_limit: 10 > (@body.quantity as int) }
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
    assert_eq!(response.body_request_fields[0].field, "below_limit");
    assert_eq!(response.body_request_fields[0].name, "quantity");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field_int"
    );
    assert_eq!(response.body_request_fields[0].op.as_deref(), Some("lt"));
    assert_eq!(
        response.body_request_fields[0].operand_json.as_deref(),
        Some("10")
    );
    assert!(handlers.contains("if value < 10"));
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
fn native_server_launcher_lowers_nested_scaled_int_multi_response_route() {
    let src = r#"@server {
  @listen 8080
  @route POST /orders {
    if @body.sku == "" {
      @respond 404 { err: "missing_sku" }
    }
    @respond 201 { quantity: (@body.quantity as int) + ((@body.bonus as int) * 2) }
  }
}"#;
    let program = lower(src);
    let map = origin_map(&program);
    let manifest = build_manifest("server.orv", &map);
    let artifact =
        server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);
    let response = &artifact.routes[0].responses[1];
    let handlers = native_server_handlers_source(&artifact);
    let launcher = native_server_launcher_source(
        "server/app.orv-runtime.json",
        "server/native-server.json",
        &artifact,
    );

    assert_eq!(artifact.routes[0].responses.len(), 2);
    assert_eq!(response.body_kind, "request_body_field_json");
    assert_eq!(response.body_request_fields[0].field, "quantity");
    assert_eq!(response.body_request_fields[0].name, "quantity");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field_int"
    );
    assert_eq!(
        response.body_request_fields[0].op.as_deref(),
        Some("add_scaled")
    );
    assert_eq!(
        response.body_request_fields[0].operand_kind.as_deref(),
        Some("request_body_field_int")
    );
    assert_eq!(
        response.body_request_fields[0].operand_name.as_deref(),
        Some("bonus")
    );
    assert_eq!(
        response.body_request_fields[0].operand_json.as_deref(),
        Some("2")
    );
    assert!(handlers.contains("operand.checked_mul(2)"));
    assert!(handlers.contains("value.checked_add(operand)"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains("fn orv_native_reference_bridge("));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_nested_product_int_add_response_body() {
    let src = r#"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { total: (@body.fee as int) + ((@body.quantity as int) * (@body.unit_price as int)) }
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
    assert_eq!(response.body_request_fields[0].name, "fee");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field_int"
    );
    assert_eq!(
        response.body_request_fields[0].op.as_deref(),
        Some("add_product")
    );
    assert_eq!(
        response.body_request_fields[0].operand_kind.as_deref(),
        Some("request_body_field_int")
    );
    assert_eq!(
        response.body_request_fields[0].operand_name.as_deref(),
        Some("quantity")
    );
    assert_eq!(
        response.body_request_fields[0]
            .secondary_operand_kind
            .as_deref(),
        Some("request_body_field_int")
    );
    assert_eq!(
        response.body_request_fields[0]
            .secondary_operand_name
            .as_deref(),
        Some("unit_price")
    );
    assert!(handlers.contains("routes::orv_native_body_field_value(route_match, \"quantity\")"));
    assert!(handlers.contains("routes::orv_native_body_field_value(route_match, \"unit_price\")"));
    assert!(handlers.contains("product_left.checked_mul(product_right)"));
    assert!(handlers.contains("value.checked_add(operand)"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains("fn orv_native_reference_bridge("));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_scaled_left_int_add_response_body() {
    let src = r#"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { quantity: ((@body.bonus as int) * 2) + (@body.quantity as int) }
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
    assert_eq!(
        response.body_request_fields[0].op.as_deref(),
        Some("add_scaled")
    );
    assert_eq!(
        response.body_request_fields[0].operand_kind.as_deref(),
        Some("request_body_field_int")
    );
    assert_eq!(
        response.body_request_fields[0].operand_name.as_deref(),
        Some("bonus")
    );
    assert_eq!(
        response.body_request_fields[0].operand_json.as_deref(),
        Some("2")
    );
    assert!(handlers.contains("operand.checked_mul(2)"));
    assert!(handlers.contains("value.checked_add(operand)"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains("fn orv_native_reference_bridge("));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_scaled_left_int_sub_response_body() {
    let src = r#"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { quantity: ((@body.bonus as int) * 2) - (@body.quantity as int) }
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
    assert_eq!(
        response.body_request_fields[0].op.as_deref(),
        Some("rsub_scaled")
    );
    assert_eq!(
        response.body_request_fields[0].operand_kind.as_deref(),
        Some("request_body_field_int")
    );
    assert_eq!(
        response.body_request_fields[0].operand_name.as_deref(),
        Some("bonus")
    );
    assert_eq!(
        response.body_request_fields[0].operand_json.as_deref(),
        Some("2")
    );
    assert!(handlers.contains("operand.checked_mul(2)"));
    assert!(handlers.contains("operand.checked_sub(value)"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains("fn orv_native_reference_bridge("));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_nested_scaled_int_mul_response_body() {
    let src = r#"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { cents: (@body.quantity as int) * ((@body.unit_price as int) * 100) }
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
    assert_eq!(response.body_request_fields[0].field, "cents");
    assert_eq!(response.body_request_fields[0].name, "quantity");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field_int"
    );
    assert_eq!(
        response.body_request_fields[0].op.as_deref(),
        Some("mul_scaled")
    );
    assert_eq!(
        response.body_request_fields[0].operand_kind.as_deref(),
        Some("request_body_field_int")
    );
    assert_eq!(
        response.body_request_fields[0].operand_name.as_deref(),
        Some("unit_price")
    );
    assert_eq!(
        response.body_request_fields[0].operand_json.as_deref(),
        Some("100")
    );
    assert!(handlers.contains("operand.checked_mul(100)"));
    assert!(handlers.contains("value.checked_mul(operand)"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains("fn orv_native_reference_bridge("));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_scaled_left_int_mul_response_body() {
    let src = r#"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { cents: ((@body.unit_price as int) * 100) * (@body.quantity as int) }
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
    assert_eq!(response.body_request_fields[0].field, "cents");
    assert_eq!(response.body_request_fields[0].name, "quantity");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field_int"
    );
    assert_eq!(
        response.body_request_fields[0].op.as_deref(),
        Some("mul_scaled")
    );
    assert_eq!(
        response.body_request_fields[0].operand_kind.as_deref(),
        Some("request_body_field_int")
    );
    assert_eq!(
        response.body_request_fields[0].operand_name.as_deref(),
        Some("unit_price")
    );
    assert_eq!(
        response.body_request_fields[0].operand_json.as_deref(),
        Some("100")
    );
    assert!(handlers.contains("operand.checked_mul(100)"));
    assert!(handlers.contains("value.checked_mul(operand)"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains("fn orv_native_reference_bridge("));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_nested_scaled_int_div_response_body() {
    let src = r#"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { unit: (@body.total as int) / ((@body.parts as int) * 100) }
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
    assert_eq!(response.body_request_fields[0].field, "unit");
    assert_eq!(response.body_request_fields[0].name, "total");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field_int"
    );
    assert_eq!(
        response.body_request_fields[0].op.as_deref(),
        Some("div_scaled")
    );
    assert_eq!(
        response.body_request_fields[0].operand_kind.as_deref(),
        Some("request_body_field_int")
    );
    assert_eq!(
        response.body_request_fields[0].operand_name.as_deref(),
        Some("parts")
    );
    assert_eq!(
        response.body_request_fields[0].operand_json.as_deref(),
        Some("100")
    );
    assert!(handlers.contains("operand.checked_mul(100)"));
    assert!(handlers.contains("value.checked_div(operand)"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains("fn orv_native_reference_bridge("));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_scaled_left_int_rem_response_body() {
    let src = r#"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { remainder: ((@body.total as int) * 10) % (@body.parts as int) }
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
    assert_eq!(response.body_request_fields[0].field, "remainder");
    assert_eq!(response.body_request_fields[0].name, "parts");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field_int"
    );
    assert_eq!(
        response.body_request_fields[0].op.as_deref(),
        Some("rrem_scaled")
    );
    assert_eq!(
        response.body_request_fields[0].operand_kind.as_deref(),
        Some("request_body_field_int")
    );
    assert_eq!(
        response.body_request_fields[0].operand_name.as_deref(),
        Some("total")
    );
    assert_eq!(
        response.body_request_fields[0].operand_json.as_deref(),
        Some("10")
    );
    assert!(handlers.contains("operand.checked_mul(10)"));
    assert!(handlers.contains("operand.checked_rem(value)"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains("fn orv_native_reference_bridge("));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
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
fn native_server_launcher_lowers_request_body_int_neg_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { quantity: -(@body.quantity as int) }
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
    assert_eq!(response.body_request_fields[0].op.as_deref(), Some("neg"));
    assert!(handlers.contains("value.checked_neg()"));
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
    assert!(handlers.contains("routes::orv_native_body_field_value(route_match, \"unit_price\")"));
    assert!(handlers.contains("value.checked_mul(operand)"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_request_body_int_pow_field_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { total: (@body.quantity as int) ** (@body.bonus as int) }
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

    assert_eq!(response.body_request_fields[0].op.as_deref(), Some("pow"));
    assert_eq!(
        response.body_request_fields[0].operand_kind.as_deref(),
        Some("request_body_field_int")
    );
    assert_eq!(
        response.body_request_fields[0].operand_name.as_deref(),
        Some("bonus")
    );
    assert!(handlers.contains("checked_pow"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_invalid_static_int_pow_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { total: (@body.quantity as int) ** -1 }
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

    assert_eq!(response.body_request_fields[0].op.as_deref(), Some("pow"));
    assert_eq!(
        response.body_request_fields[0].operand_json.as_deref(),
        Some("-1")
    );
    assert!(handlers.contains("native request body int arithmetic failed"));
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
fn native_server_launcher_lowers_request_body_int_captured_comparison_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { available: (@body.quantity as int) <= (@body.stock as int) }
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
    assert_eq!(response.body_request_fields[0].field, "available");
    assert_eq!(response.body_request_fields[0].name, "quantity");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field_int"
    );
    assert_eq!(response.body_request_fields[0].op.as_deref(), Some("le"));
    assert_eq!(
        response.body_request_fields[0].operand_kind.as_deref(),
        Some("request_body_field_int")
    );
    assert_eq!(
        response.body_request_fields[0].operand_name.as_deref(),
        Some("stock")
    );
    assert!(handlers.contains("if value <= operand"));
    assert!(handlers.contains("body.push_str(\"true\")"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_request_body_int_scaled_comparison_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { available: (@body.quantity as int) <= ((@body.stock as int) * 10) }
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
    assert_eq!(response.body_request_fields[0].field, "available");
    assert_eq!(response.body_request_fields[0].name, "quantity");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field_int"
    );
    assert_eq!(
        response.body_request_fields[0].op.as_deref(),
        Some("le_scaled")
    );
    assert_eq!(
        response.body_request_fields[0].operand_kind.as_deref(),
        Some("request_body_field_int")
    );
    assert_eq!(
        response.body_request_fields[0].operand_name.as_deref(),
        Some("stock")
    );
    assert_eq!(
        response.body_request_fields[0].operand_json.as_deref(),
        Some("10")
    );
    assert!(handlers.contains("operand.checked_mul(10)"));
    assert!(handlers.contains("if value <= operand"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_scaled_left_int_comparison_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { covered: ((@body.minimum as int) * 100) <= (@body.total as int) }
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
    assert_eq!(response.body_request_fields[0].field, "covered");
    assert_eq!(response.body_request_fields[0].name, "total");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field_int"
    );
    assert_eq!(
        response.body_request_fields[0].op.as_deref(),
        Some("ge_scaled")
    );
    assert_eq!(
        response.body_request_fields[0].operand_kind.as_deref(),
        Some("request_body_field_int")
    );
    assert_eq!(
        response.body_request_fields[0].operand_name.as_deref(),
        Some("minimum")
    );
    assert_eq!(
        response.body_request_fields[0].operand_json.as_deref(),
        Some("100")
    );
    assert!(handlers.contains("operand.checked_mul(100)"));
    assert!(handlers.contains("if value >= operand"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_request_body_int_product_comparison_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { covered: (@body.total as int) <= ((@body.quantity as int) * (@body.unit_price as int)) }
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
    assert_eq!(response.body_request_fields[0].field, "covered");
    assert_eq!(response.body_request_fields[0].name, "total");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field_int"
    );
    assert_eq!(
        response.body_request_fields[0].op.as_deref(),
        Some("le_product")
    );
    assert_eq!(
        response.body_request_fields[0].operand_kind.as_deref(),
        Some("request_body_field_int")
    );
    assert_eq!(
        response.body_request_fields[0].operand_name.as_deref(),
        Some("quantity")
    );
    assert_eq!(
        response.body_request_fields[0]
            .secondary_operand_kind
            .as_deref(),
        Some("request_body_field_int")
    );
    assert_eq!(
        response.body_request_fields[0]
            .secondary_operand_name
            .as_deref(),
        Some("unit_price")
    );
    assert!(handlers.contains("product_left.checked_mul(product_right)"));
    assert!(handlers.contains("if value <= operand"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_request_body_int_product_div_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { bundles: (@body.total as int) / ((@body.quantity as int) * (@body.unit_price as int)) }
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
    assert_eq!(response.body_request_fields[0].field, "bundles");
    assert_eq!(response.body_request_fields[0].name, "total");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field_int"
    );
    assert_eq!(
        response.body_request_fields[0].op.as_deref(),
        Some("div_product")
    );
    assert_eq!(
        response.body_request_fields[0].operand_name.as_deref(),
        Some("quantity")
    );
    assert_eq!(
        response.body_request_fields[0]
            .secondary_operand_name
            .as_deref(),
        Some("unit_price")
    );
    assert!(handlers.contains("product_left.checked_mul(product_right)"));
    assert!(handlers.contains("value.checked_div(operand)"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_product_left_int_rem_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { remainder: ((@body.quantity as int) * (@body.unit_price as int)) % (@body.total as int) }
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
        response.body_request_fields[0].op.as_deref(),
        Some("rrem_product")
    );
    assert_eq!(
        response.body_request_fields[0].operand_name.as_deref(),
        Some("quantity")
    );
    assert_eq!(
        response.body_request_fields[0]
            .secondary_operand_name
            .as_deref(),
        Some("unit_price")
    );
    assert!(handlers.contains("product_left.checked_mul(product_right)"));
    assert!(handlers.contains("operand.checked_rem(value)"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_product_static_int_add_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { total: ((@body.quantity as int) * (@body.unit_price as int)) + 25 }
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
        response.body_request_fields[0].op.as_deref(),
        Some("add_product_static")
    );
    assert_eq!(
        response.body_request_fields[0].operand_name.as_deref(),
        Some("unit_price")
    );
    assert_eq!(
        response.body_request_fields[0].operand_json.as_deref(),
        Some("25")
    );
    assert!(handlers.contains("value.checked_mul(product_right)"));
    assert!(handlers.contains("product.checked_add(25)"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_static_left_product_int_sub_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { remaining: 1000 - ((@body.quantity as int) * (@body.unit_price as int)) }
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
    assert_eq!(response.body_request_fields[0].field, "remaining");
    assert_eq!(response.body_request_fields[0].name, "quantity");
    assert_eq!(
        response.body_request_fields[0].op.as_deref(),
        Some("rsub_product_static")
    );
    assert_eq!(
        response.body_request_fields[0].operand_name.as_deref(),
        Some("unit_price")
    );
    assert_eq!(
        response.body_request_fields[0].operand_json.as_deref(),
        Some("1000")
    );
    assert!(handlers.contains("value.checked_mul(product_right)"));
    assert!(handlers.contains("1000_i64.checked_sub(product)"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_product_static_int_comparison_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { covered: ((@body.quantity as int) * (@body.unit_price as int)) <= 1000 }
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
    assert_eq!(response.body_request_fields[0].field, "covered");
    assert_eq!(response.body_request_fields[0].name, "quantity");
    assert_eq!(
        response.body_request_fields[0].op.as_deref(),
        Some("le_product_static")
    );
    assert_eq!(
        response.body_request_fields[0].operand_name.as_deref(),
        Some("unit_price")
    );
    assert_eq!(
        response.body_request_fields[0].operand_json.as_deref(),
        Some("1000")
    );
    assert!(handlers.contains("value.checked_mul(product_right)"));
    assert!(handlers.contains("if product <= 1000"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_static_left_product_int_comparison_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { covered: 1000 >= ((@body.quantity as int) * (@body.unit_price as int)) }
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

    assert_eq!(
        response.body_request_fields[0].op.as_deref(),
        Some("le_product_static")
    );
    assert_eq!(
        response.body_request_fields[0].operand_json.as_deref(),
        Some("1000")
    );
    assert!(handlers.contains("if product <= 1000"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_product_product_int_add_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { total: ((@body.quantity as int) * (@body.unit_price as int)) + ((@body.bonus as int) * (@body.bonus_price as int)) }
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
        response.body_request_fields[0].op.as_deref(),
        Some("add_product_product")
    );
    assert_eq!(
        response.body_request_fields[0].operand_name.as_deref(),
        Some("unit_price")
    );
    assert_eq!(
        response.body_request_fields[0]
            .secondary_operand_name
            .as_deref(),
        Some("bonus")
    );
    assert_eq!(
        response.body_request_fields[0]
            .tertiary_operand_name
            .as_deref(),
        Some("bonus_price")
    );
    assert!(handlers.contains("value.checked_mul(left_right)"));
    assert!(handlers.contains("right_left.checked_mul(right_right)"));
    assert!(handlers.contains("left_product.checked_add(right_product)"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_scaled_product_int_add_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { total: (@body.base as int) + (((@body.quantity as int) * (@body.unit_price as int)) * 100) }
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
    assert_eq!(response.body_request_fields[0].name, "base");
    assert_eq!(
        response.body_request_fields[0].op.as_deref(),
        Some("add_scaled_product")
    );
    assert_eq!(
        response.body_request_fields[0].operand_json.as_deref(),
        Some("100")
    );
    assert_eq!(
        response.body_request_fields[0].operand_name.as_deref(),
        Some("quantity")
    );
    assert_eq!(
        response.body_request_fields[0]
            .secondary_operand_name
            .as_deref(),
        Some("unit_price")
    );
    assert!(handlers.contains("partial_product.checked_mul(100)"));
    assert!(handlers.contains("value.checked_add(operand)"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_left_scaled_product_int_add_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { total: (((@body.quantity as int) * (@body.unit_price as int)) * 100) + (@body.base as int) }
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

    assert_eq!(response.body_request_fields[0].field, "total");
    assert_eq!(response.body_request_fields[0].name, "base");
    assert_eq!(
        response.body_request_fields[0].op.as_deref(),
        Some("add_scaled_product")
    );
    assert_eq!(
        response.body_request_fields[0].operand_json.as_deref(),
        Some("100")
    );
    assert!(handlers.contains("value.checked_add(operand)"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_scaled_product_int_comparison_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { covered: (@body.total as int) <= (((@body.quantity as int) * (@body.unit_price as int)) * 100) }
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

    assert_eq!(response.body_request_fields[0].field, "covered");
    assert_eq!(response.body_request_fields[0].name, "total");
    assert_eq!(
        response.body_request_fields[0].op.as_deref(),
        Some("le_scaled_product")
    );
    assert_eq!(
        response.body_request_fields[0].operand_json.as_deref(),
        Some("100")
    );
    assert!(handlers.contains("partial_product.checked_mul(100)"));
    assert!(handlers.contains("if value <= operand"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_triple_product_int_add_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { total: (@body.base as int) + (((@body.quantity as int) * (@body.unit_price as int)) * (@body.bundle_count as int)) }
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
    assert_eq!(response.body_request_fields[0].name, "base");
    assert_eq!(
        response.body_request_fields[0].op.as_deref(),
        Some("add_triple_product")
    );
    assert_eq!(
        response.body_request_fields[0].operand_name.as_deref(),
        Some("quantity")
    );
    assert_eq!(
        response.body_request_fields[0]
            .secondary_operand_name
            .as_deref(),
        Some("unit_price")
    );
    assert_eq!(
        response.body_request_fields[0]
            .tertiary_operand_name
            .as_deref(),
        Some("bundle_count")
    );
    assert!(handlers.contains("partial_product.checked_mul(third_product)"));
    assert!(handlers.contains("value.checked_add(triple_product)"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_left_triple_product_int_add_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { total: (((@body.quantity as int) * (@body.unit_price as int)) * (@body.bundle_count as int)) + (@body.base as int) }
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

    assert_eq!(response.body_request_fields[0].field, "total");
    assert_eq!(response.body_request_fields[0].name, "base");
    assert_eq!(
        response.body_request_fields[0].op.as_deref(),
        Some("add_triple_product")
    );
    assert_eq!(
        response.body_request_fields[0].operand_name.as_deref(),
        Some("quantity")
    );
    assert!(handlers.contains("value.checked_add(triple_product)"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_triple_product_int_comparison_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { covered: (@body.total as int) <= (((@body.quantity as int) * (@body.unit_price as int)) * (@body.bundle_count as int)) }
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

    assert_eq!(response.body_request_fields[0].field, "covered");
    assert_eq!(response.body_request_fields[0].name, "total");
    assert_eq!(
        response.body_request_fields[0].op.as_deref(),
        Some("le_triple_product")
    );
    assert_eq!(
        response.body_request_fields[0].operand_name.as_deref(),
        Some("quantity")
    );
    assert_eq!(
        response.body_request_fields[0]
            .secondary_operand_name
            .as_deref(),
        Some("unit_price")
    );
    assert_eq!(
        response.body_request_fields[0]
            .tertiary_operand_name
            .as_deref(),
        Some("bundle_count")
    );
    assert!(handlers.contains("partial_product.checked_mul(third_product)"));
    assert!(handlers.contains("value <= triple_product"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_left_triple_product_int_comparison_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { covered: (((@body.quantity as int) * (@body.unit_price as int)) * (@body.bundle_count as int)) <= (@body.total as int) }
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

    assert_eq!(response.body_request_fields[0].field, "covered");
    assert_eq!(response.body_request_fields[0].name, "total");
    assert_eq!(
        response.body_request_fields[0].op.as_deref(),
        Some("ge_triple_product")
    );
    assert_eq!(
        response.body_request_fields[0].operand_name.as_deref(),
        Some("quantity")
    );
    assert!(handlers.contains("value >= triple_product"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_product_product_int_comparison_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { covered: ((@body.quantity as int) * (@body.unit_price as int)) <= ((@body.stock as int) * (@body.stock_price as int)) }
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

    assert_eq!(response.body_request_fields[0].field, "covered");
    assert_eq!(
        response.body_request_fields[0].op.as_deref(),
        Some("le_product_product")
    );
    assert_eq!(
        response.body_request_fields[0].operand_name.as_deref(),
        Some("unit_price")
    );
    assert_eq!(
        response.body_request_fields[0]
            .secondary_operand_name
            .as_deref(),
        Some("stock")
    );
    assert_eq!(
        response.body_request_fields[0]
            .tertiary_operand_name
            .as_deref(),
        Some("stock_price")
    );
    assert!(handlers.contains("left_product"));
    assert!(handlers.contains("right_product"));
    assert!(handlers.contains("if left_product <= right_product"));
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
    assert!(!handlers.contains(
        "orv_native_push_json_string(routes::orv_native_body_field_value(route_match, \"amount\")"
    ));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_request_body_float_neg_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /payments {
    @respond 201 { amount: -(@body.amount as float) }
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
    assert_eq!(response.body_request_fields[0].field, "amount");
    assert_eq!(response.body_request_fields[0].name, "amount");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field_float"
    );
    assert_eq!(response.body_request_fields[0].op.as_deref(), Some("neg"));
    assert!(handlers.contains("let value = -value;"));
    assert!(handlers.contains("if value.is_finite()"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_static_left_float_sub_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /payments {
    @respond 201 { remaining: 100.5 - (@body.amount as float) }
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
    assert_eq!(response.body_request_fields[0].field, "remaining");
    assert_eq!(response.body_request_fields[0].name, "amount");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field_float"
    );
    assert_eq!(response.body_request_fields[0].op.as_deref(), Some("rsub"));
    assert_eq!(
        response.body_request_fields[0].operand_json.as_deref(),
        Some("100.5")
    );
    assert!(handlers.contains("let value = 100.5 - value;"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_static_left_float_div_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /payments {
    @respond 201 { ratio: 100.0 / (@body.amount as float) }
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
    assert_eq!(response.body_request_fields[0].field, "ratio");
    assert_eq!(response.body_request_fields[0].name, "amount");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field_float"
    );
    assert_eq!(response.body_request_fields[0].op.as_deref(), Some("rdiv"));
    assert_eq!(
        response.body_request_fields[0].operand_json.as_deref(),
        Some("100.0")
    );
    assert!(handlers.contains("let value = 100.0 / value;"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_static_left_float_rem_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /payments {
    @respond 201 { remainder: 10.5 % (@body.amount as float) }
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
    assert_eq!(response.body_request_fields[0].name, "amount");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field_float"
    );
    assert_eq!(response.body_request_fields[0].op.as_deref(), Some("rrem"));
    assert_eq!(
        response.body_request_fields[0].operand_json.as_deref(),
        Some("10.5")
    );
    assert!(handlers.contains("let value = 10.5 % value;"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_static_left_float_pow_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /payments {
    @respond 201 { total: 2.0 ** (@body.exp as float) }
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
    assert_eq!(response.body_request_fields[0].name, "exp");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field_float"
    );
    assert_eq!(response.body_request_fields[0].op.as_deref(), Some("rpow"));
    assert_eq!(
        response.body_request_fields[0].operand_json.as_deref(),
        Some("2.0")
    );
    assert!(handlers.contains("let value = (2.0_f64).powf(value);"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_request_body_query_float_comparison_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /payments {
    @respond 201 { under_limit: (@body.amount as float) <= (@query.limit as float) }
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
    assert_eq!(response.body_request_fields[0].field, "under_limit");
    assert_eq!(response.body_request_fields[0].name, "amount");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field_float"
    );
    assert_eq!(response.body_request_fields[0].op.as_deref(), Some("le"));
    assert_eq!(
        response.body_request_fields[0].operand_kind.as_deref(),
        Some("query_param_float")
    );
    assert_eq!(
        response.body_request_fields[0].operand_name.as_deref(),
        Some("limit")
    );
    assert!(handlers.contains("if value <= operand"));
    assert!(handlers.contains("operand.is_finite()"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_request_body_bool_cast_response_body() {
    let src = r#"@server {
  @listen 8080
  @route POST /members {
    @respond 201 { subscribed: @body.subscribed as bool }
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
    assert_eq!(response.body_request_fields[0].field, "subscribed");
    assert_eq!(response.body_request_fields[0].name, "subscribed");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field_bool"
    );
    assert!(handlers.contains("match routes::orv_native_body_field_value(route_match, \"subscribed\").unwrap_or(\"\").trim()"));
    assert!(handlers.contains(r#""true" => body.push_str("true")"#));
    assert!(handlers.contains(r#""false" => body.push_str("false")"#));
    assert!(!handlers.contains("orv_native_push_json_string(routes::orv_native_body_field_value(route_match, \"subscribed\")"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_request_body_bool_not_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /members {
    @respond 201 { active: !(@body.disabled as bool) }
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
    assert_eq!(response.body_request_fields[0].field, "active");
    assert_eq!(response.body_request_fields[0].name, "disabled");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field_bool"
    );
    assert_eq!(response.body_request_fields[0].op.as_deref(), Some("not"));
    assert!(handlers.contains("match routes::orv_native_body_field_value(route_match, \"disabled\").unwrap_or(\"\").trim()"));
    assert!(handlers.contains(r#""true" => body.push_str("false")"#));
    assert!(handlers.contains(r#""false" => body.push_str("true")"#));
    assert!(!handlers.contains("orv_native_push_json_string(routes::orv_native_body_field_value(route_match, \"disabled\")"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_request_body_bool_eq_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /members {
    @respond 201 { subscribed: (@body.subscribed as bool) == true }
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
    assert_eq!(response.body_request_fields[0].field, "subscribed");
    assert_eq!(response.body_request_fields[0].name, "subscribed");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field_bool"
    );
    assert_eq!(response.body_request_fields[0].op.as_deref(), Some("eq"));
    assert_eq!(
        response.body_request_fields[0].operand_json.as_deref(),
        Some("true")
    );
    assert!(handlers.contains("match routes::orv_native_body_field_value(route_match, \"subscribed\").unwrap_or(\"\").trim()"));
    assert!(handlers.contains(r#""true" => body.push_str("true")"#));
    assert!(handlers.contains(r#""false" => body.push_str("false")"#));
    assert!(!handlers.contains("orv_native_push_json_string(routes::orv_native_body_field_value(route_match, \"subscribed\")"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_request_body_query_bool_eq_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /members {
    @respond 201 { matches: (@body.subscribed as bool) == (@query.expected as bool) }
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
    assert_eq!(response.body_request_fields[0].field, "matches");
    assert_eq!(response.body_request_fields[0].name, "subscribed");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field_bool"
    );
    assert_eq!(response.body_request_fields[0].op.as_deref(), Some("eq"));
    assert_eq!(
        response.body_request_fields[0].operand_kind.as_deref(),
        Some("query_param_bool")
    );
    assert_eq!(
        response.body_request_fields[0].operand_name.as_deref(),
        Some("expected")
    );
    assert!(handlers.contains("match (routes::orv_native_body_field_value(route_match, \"subscribed\").unwrap_or(\"\").trim(), routes::orv_native_query_value(route_match, \"expected\").unwrap_or(\"\").trim())"));
    assert!(handlers.contains(r#"("true", "true") => body.push_str("true")"#));
    assert!(handlers.contains(r#"("true", "false") => body.push_str("false")"#));
    assert!(handlers.contains(r#"("false", "true") => body.push_str("false")"#));
    assert!(handlers.contains(r#"("false", "false") => body.push_str("true")"#));
    assert!(!handlers.contains("orv_native_push_json_string(routes::orv_native_body_field_value(route_match, \"subscribed\")"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_request_body_query_bool_and_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /members {
    @respond 201 { eligible: (@body.subscribed as bool) && (@query.verified as bool) }
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
    assert_eq!(response.body_request_fields[0].field, "eligible");
    assert_eq!(response.body_request_fields[0].name, "subscribed");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field_bool"
    );
    assert_eq!(response.body_request_fields[0].op.as_deref(), Some("and"));
    assert_eq!(
        response.body_request_fields[0].operand_kind.as_deref(),
        Some("query_param_bool")
    );
    assert_eq!(
        response.body_request_fields[0].operand_name.as_deref(),
        Some("verified")
    );
    assert!(handlers.contains("match (routes::orv_native_body_field_value(route_match, \"subscribed\").unwrap_or(\"\").trim(), routes::orv_native_query_value(route_match, \"verified\").unwrap_or(\"\").trim())"));
    assert!(handlers.contains(r#"("true", "true") => body.push_str("true")"#));
    assert!(handlers.contains(r#"("true", "false") => body.push_str("false")"#));
    assert!(handlers.contains(r#"("false", "true") => body.push_str("false")"#));
    assert!(handlers.contains(r#"("false", "false") => body.push_str("false")"#));
    assert!(!handlers.contains("orv_native_push_json_string(routes::orv_native_body_field_value(route_match, \"subscribed\")"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_request_body_query_bool_or_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /members {
    @respond 201 { eligible: (@body.subscribed as bool) || (@query.override as bool) }
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
    assert_eq!(response.body_request_fields[0].field, "eligible");
    assert_eq!(response.body_request_fields[0].name, "subscribed");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field_bool"
    );
    assert_eq!(response.body_request_fields[0].op.as_deref(), Some("or"));
    assert_eq!(
        response.body_request_fields[0].operand_kind.as_deref(),
        Some("query_param_bool")
    );
    assert_eq!(
        response.body_request_fields[0].operand_name.as_deref(),
        Some("override")
    );
    assert!(handlers.contains("match (routes::orv_native_body_field_value(route_match, \"subscribed\").unwrap_or(\"\").trim(), routes::orv_native_query_value(route_match, \"override\").unwrap_or(\"\").trim())"));
    assert!(handlers.contains(r#"("true", "true") => body.push_str("true")"#));
    assert!(handlers.contains(r#"("true", "false") => body.push_str("true")"#));
    assert!(handlers.contains(r#"("false", "true") => body.push_str("true")"#));
    assert!(handlers.contains(r#"("false", "false") => body.push_str("false")"#));
    assert!(!handlers.contains("orv_native_push_json_string(routes::orv_native_body_field_value(route_match, \"subscribed\")"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_negated_request_body_query_bool_and_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /members {
    @respond 201 { eligible: !(@body.suspended as bool) && (@query.verified as bool) }
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
    assert_eq!(response.body_request_fields[0].field, "eligible");
    assert_eq!(response.body_request_fields[0].name, "suspended");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field_bool"
    );
    assert_eq!(
        response.body_request_fields[0].op.as_deref(),
        Some("not_and")
    );
    assert_eq!(
        response.body_request_fields[0].operand_kind.as_deref(),
        Some("query_param_bool")
    );
    assert_eq!(
        response.body_request_fields[0].operand_name.as_deref(),
        Some("verified")
    );
    assert!(handlers.contains("match (routes::orv_native_body_field_value(route_match, \"suspended\").unwrap_or(\"\").trim(), routes::orv_native_query_value(route_match, \"verified\").unwrap_or(\"\").trim())"));
    assert!(handlers.contains(r#"("true", "true") => body.push_str("false")"#));
    assert!(handlers.contains(r#"("true", "false") => body.push_str("false")"#));
    assert!(handlers.contains(r#"("false", "true") => body.push_str("true")"#));
    assert!(handlers.contains(r#"("false", "false") => body.push_str("false")"#));
    assert!(!handlers.contains("orv_native_push_json_string(routes::orv_native_body_field_value(route_match, \"suspended\")"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_negated_request_body_query_bool_or_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /members {
    @respond 201 { eligible: !(@body.suspended as bool) || (@query.override as bool) }
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
    assert_eq!(response.body_request_fields[0].field, "eligible");
    assert_eq!(response.body_request_fields[0].name, "suspended");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field_bool"
    );
    assert_eq!(
        response.body_request_fields[0].op.as_deref(),
        Some("not_or")
    );
    assert_eq!(
        response.body_request_fields[0].operand_kind.as_deref(),
        Some("query_param_bool")
    );
    assert_eq!(
        response.body_request_fields[0].operand_name.as_deref(),
        Some("override")
    );
    assert!(handlers.contains("match (routes::orv_native_body_field_value(route_match, \"suspended\").unwrap_or(\"\").trim(), routes::orv_native_query_value(route_match, \"override\").unwrap_or(\"\").trim())"));
    assert!(handlers.contains(r#"("true", "true") => body.push_str("true")"#));
    assert!(handlers.contains(r#"("true", "false") => body.push_str("false")"#));
    assert!(handlers.contains(r#"("false", "true") => body.push_str("true")"#));
    assert!(handlers.contains(r#"("false", "false") => body.push_str("true")"#));
    assert!(!handlers.contains("orv_native_push_json_string(routes::orv_native_body_field_value(route_match, \"suspended\")"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_request_body_negated_query_bool_and_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /members {
    @respond 201 { eligible: (@body.subscribed as bool) && !(@query.blocked as bool) }
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
    assert_eq!(response.body_request_fields[0].field, "eligible");
    assert_eq!(response.body_request_fields[0].name, "subscribed");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field_bool"
    );
    assert_eq!(
        response.body_request_fields[0].op.as_deref(),
        Some("and_not")
    );
    assert_eq!(
        response.body_request_fields[0].operand_kind.as_deref(),
        Some("query_param_bool")
    );
    assert_eq!(
        response.body_request_fields[0].operand_name.as_deref(),
        Some("blocked")
    );
    assert!(handlers.contains("match (routes::orv_native_body_field_value(route_match, \"subscribed\").unwrap_or(\"\").trim(), routes::orv_native_query_value(route_match, \"blocked\").unwrap_or(\"\").trim())"));
    assert!(handlers.contains(r#"("true", "true") => body.push_str("false")"#));
    assert!(handlers.contains(r#"("true", "false") => body.push_str("true")"#));
    assert!(handlers.contains(r#"("false", "true") => body.push_str("false")"#));
    assert!(handlers.contains(r#"("false", "false") => body.push_str("false")"#));
    assert!(!handlers.contains("orv_native_push_json_string(routes::orv_native_body_field_value(route_match, \"subscribed\")"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_request_body_negated_query_bool_or_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /members {
    @respond 201 { eligible: (@body.subscribed as bool) || !(@query.blocked as bool) }
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
    assert_eq!(response.body_request_fields[0].field, "eligible");
    assert_eq!(response.body_request_fields[0].name, "subscribed");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field_bool"
    );
    assert_eq!(
        response.body_request_fields[0].op.as_deref(),
        Some("or_not")
    );
    assert_eq!(
        response.body_request_fields[0].operand_kind.as_deref(),
        Some("query_param_bool")
    );
    assert_eq!(
        response.body_request_fields[0].operand_name.as_deref(),
        Some("blocked")
    );
    assert!(handlers.contains("match (routes::orv_native_body_field_value(route_match, \"subscribed\").unwrap_or(\"\").trim(), routes::orv_native_query_value(route_match, \"blocked\").unwrap_or(\"\").trim())"));
    assert!(handlers.contains(r#"("true", "true") => body.push_str("true")"#));
    assert!(handlers.contains(r#"("true", "false") => body.push_str("true")"#));
    assert!(handlers.contains(r#"("false", "true") => body.push_str("false")"#));
    assert!(handlers.contains(r#"("false", "false") => body.push_str("true")"#));
    assert!(!handlers.contains("orv_native_push_json_string(routes::orv_native_body_field_value(route_match, \"subscribed\")"));
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
fn native_server_launcher_lowers_product_static_float_add_response_body() {
    let src = r#"@server {
  @listen 8080
  @route POST /payments {
    @respond 201 { total: ((@body.price as float) * (@body.quantity as float)) + 1.25 }
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
    assert_eq!(
        response.body_request_fields[0].op.as_deref(),
        Some("add_product_static")
    );
    assert_eq!(
        response.body_request_fields[0].operand_kind.as_deref(),
        Some("request_body_field_float")
    );
    assert_eq!(
        response.body_request_fields[0].operand_name.as_deref(),
        Some("quantity")
    );
    assert_eq!(
        response.body_request_fields[0].operand_json.as_deref(),
        Some("1.25")
    );
    assert!(handlers.contains("let product = value * product_right;"));
    assert!(handlers.contains("let value = product + 1.25;"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_product_static_float_comparison_response_body() {
    let src = r#"@server {
  @listen 8080
  @route POST /payments {
    @respond 201 { under_limit: ((@body.price as float) * (@body.quantity as float)) <= 40.0 }
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
    assert_eq!(response.body_request_fields[0].field, "under_limit");
    assert_eq!(response.body_request_fields[0].name, "price");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field_float"
    );
    assert_eq!(
        response.body_request_fields[0].op.as_deref(),
        Some("le_product_static")
    );
    assert_eq!(
        response.body_request_fields[0].operand_name.as_deref(),
        Some("quantity")
    );
    assert_eq!(
        response.body_request_fields[0].operand_json.as_deref(),
        Some("40.0")
    );
    assert!(handlers.contains("let product = value * product_right;"));
    assert!(handlers.contains("if product <= 40.0"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_product_product_float_add_response_body() {
    let src = r#"@server {
  @listen 8080
  @route POST /payments {
    @respond 201 { total: ((@body.price as float) * (@body.quantity as float)) + ((@body.fee as float) * (@body.fee_units as float)) }
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
    assert_eq!(
        response.body_request_fields[0].op.as_deref(),
        Some("add_product_product")
    );
    assert_eq!(
        response.body_request_fields[0].operand_name.as_deref(),
        Some("quantity")
    );
    assert_eq!(
        response.body_request_fields[0]
            .secondary_operand_name
            .as_deref(),
        Some("fee")
    );
    assert_eq!(
        response.body_request_fields[0]
            .tertiary_operand_name
            .as_deref(),
        Some("fee_units")
    );
    assert!(handlers.contains("let left_product = value * left_right;"));
    assert!(handlers.contains("let right_product = right_left * right_right;"));
    assert!(handlers.contains("let value = left_product + right_product;"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_scaled_product_float_add_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /payments {
    @respond 201 { total: (@body.base as float) + (((@body.price as float) * (@body.quantity as float)) * 0.5) }
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
    assert_eq!(response.body_request_fields[0].name, "base");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field_float"
    );
    assert_eq!(
        response.body_request_fields[0].op.as_deref(),
        Some("add_scaled_product")
    );
    assert_eq!(
        response.body_request_fields[0].operand_json.as_deref(),
        Some("0.5")
    );
    assert_eq!(
        response.body_request_fields[0].operand_name.as_deref(),
        Some("price")
    );
    assert_eq!(
        response.body_request_fields[0]
            .secondary_operand_name
            .as_deref(),
        Some("quantity")
    );
    assert!(handlers.contains("let operand = partial_product * 0.5;"));
    assert!(handlers.contains("let value = value + operand;"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_scaled_product_float_comparison_response_body() {
    let src = r"@server {
  @listen 8080
  @route POST /payments {
    @respond 201 { under_limit: (@body.total as float) <= (((@body.price as float) * (@body.quantity as float)) * 0.5) }
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

    assert_eq!(response.body_request_fields[0].field, "under_limit");
    assert_eq!(response.body_request_fields[0].name, "total");
    assert_eq!(
        response.body_request_fields[0].op.as_deref(),
        Some("le_scaled_product")
    );
    assert_eq!(
        response.body_request_fields[0].operand_json.as_deref(),
        Some("0.5")
    );
    assert!(handlers.contains("let operand = partial_product * 0.5;"));
    assert!(handlers.contains("if value <= operand"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_triple_product_float_add_response_body() {
    let src = r#"@server {
  @listen 8080
  @route POST /payments {
    @respond 201 { total: (@body.base as float) + (((@body.price as float) * (@body.quantity as float)) * (@body.multiplier as float)) }
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
    assert_eq!(response.body_request_fields[0].name, "base");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field_float"
    );
    assert_eq!(
        response.body_request_fields[0].op.as_deref(),
        Some("add_triple_product")
    );
    assert_eq!(
        response.body_request_fields[0].operand_name.as_deref(),
        Some("price")
    );
    assert_eq!(
        response.body_request_fields[0]
            .secondary_operand_name
            .as_deref(),
        Some("quantity")
    );
    assert_eq!(
        response.body_request_fields[0]
            .tertiary_operand_name
            .as_deref(),
        Some("multiplier")
    );
    assert!(
        handlers.contains("let triple_product = first_product * second_product * third_product;")
    );
    assert!(handlers.contains("let value = value + triple_product;"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_triple_product_float_comparison_response_body() {
    let src = r#"@server {
  @listen 8080
  @route POST /payments {
    @respond 201 { under_limit: (@body.total as float) <= (((@body.price as float) * (@body.quantity as float)) * (@body.multiplier as float)) }
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

    assert_eq!(response.body_request_fields[0].field, "under_limit");
    assert_eq!(response.body_request_fields[0].name, "total");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field_float"
    );
    assert_eq!(
        response.body_request_fields[0].op.as_deref(),
        Some("le_triple_product")
    );
    assert_eq!(
        response.body_request_fields[0].operand_name.as_deref(),
        Some("price")
    );
    assert_eq!(
        response.body_request_fields[0]
            .secondary_operand_name
            .as_deref(),
        Some("quantity")
    );
    assert_eq!(
        response.body_request_fields[0]
            .tertiary_operand_name
            .as_deref(),
        Some("multiplier")
    );
    assert!(
        handlers.contains("let triple_product = first_product * second_product * third_product;")
    );
    assert!(handlers.contains("value <= triple_product"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_left_triple_product_float_comparison_response_body() {
    let src = r#"@server {
  @listen 8080
  @route POST /payments {
    @respond 201 { under_limit: (((@body.price as float) * (@body.quantity as float)) * (@body.multiplier as float)) <= (@body.total as float) }
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

    assert_eq!(response.body_request_fields[0].field, "under_limit");
    assert_eq!(response.body_request_fields[0].name, "total");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field_float"
    );
    assert_eq!(
        response.body_request_fields[0].op.as_deref(),
        Some("ge_triple_product")
    );
    assert_eq!(
        response.body_request_fields[0].operand_name.as_deref(),
        Some("price")
    );
    assert!(handlers.contains("value >= triple_product"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_product_product_float_comparison_response_body() {
    let src = r#"@server {
  @listen 8080
  @route POST /payments {
    @respond 201 { under_limit: ((@body.price as float) * (@body.quantity as float)) <= ((@body.limit_price as float) * (@body.limit_units as float)) }
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
    assert_eq!(response.body_request_fields[0].field, "under_limit");
    assert_eq!(response.body_request_fields[0].name, "price");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field_float"
    );
    assert_eq!(
        response.body_request_fields[0].op.as_deref(),
        Some("le_product_product")
    );
    assert_eq!(
        response.body_request_fields[0].operand_name.as_deref(),
        Some("quantity")
    );
    assert_eq!(
        response.body_request_fields[0]
            .secondary_operand_name
            .as_deref(),
        Some("limit_price")
    );
    assert_eq!(
        response.body_request_fields[0]
            .tertiary_operand_name
            .as_deref(),
        Some("limit_units")
    );
    assert!(handlers.contains("let left_product = value * left_right;"));
    assert!(handlers.contains("let right_product = right_left * right_right;"));
    assert!(handlers.contains("if left_product <= right_product"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_request_body_float_pow_field_response_body() {
    let src = r#"@server {
  @listen 8080
  @route POST /payments {
    @respond 201 { total: (@body.base as float) ** (@body.exp as float) }
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
    assert_eq!(response.body_request_fields[0].name, "base");
    assert_eq!(
        response.body_request_fields[0].value_kind,
        "request_body_field_float"
    );
    assert_eq!(response.body_request_fields[0].op.as_deref(), Some("pow"));
    assert_eq!(
        response.body_request_fields[0].operand_kind.as_deref(),
        Some("request_body_field_float")
    );
    assert_eq!(
        response.body_request_fields[0].operand_name.as_deref(),
        Some("exp")
    );
    assert!(handlers.contains("routes::orv_native_body_field_value(route_match, \"base\")"));
    assert!(handlers.contains("routes::orv_native_body_field_value(route_match, \"exp\")"));
    assert!(handlers.contains("let value = value.powf(operand);"));
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
fn native_server_launcher_lowers_route_param_bool_cast_response_body() {
    let src = r"@server {
  @listen 8080
  @route GET /features/:enabled {
    @respond 200 { enabled: @param.enabled as bool }
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
    assert_eq!(response.body_route_params[0].field, "enabled");
    assert_eq!(response.body_route_params[0].param, "enabled");
    assert_eq!(response.body_route_params[0].value_kind, "route_param_bool");
    assert!(handlers.contains(
        "match routes::orv_native_param_value(route_match, \"enabled\").unwrap_or(\"\").trim()"
    ));
    assert!(handlers.contains(r#""true" => body.push_str("true")"#));
    assert!(handlers.contains(r#""false" => body.push_str("false")"#));
    assert!(!handlers.contains(
        "orv_native_push_json_string(routes::orv_native_param_value(route_match, \"enabled\")"
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
fn native_server_launcher_lowers_route_param_float_arithmetic_response_body() {
    let src = r"@server {
  @listen 8080
  @route GET /products/:price/:tax {
    @respond 200 {
      discounted: (@param.price as float) * 0.5,
      taxed: (@param.price as float) + (@param.tax as float),
      remaining: 100.0 - (@param.price as float),
      power: 2.0 ** (@param.tax as float)
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
    assert_eq!(response.body_route_params[0].field, "discounted");
    assert_eq!(response.body_route_params[0].param, "price");
    assert_eq!(
        response.body_route_params[0].value_kind,
        "route_param_float"
    );
    assert_eq!(response.body_route_params[0].op.as_deref(), Some("mul"));
    assert_eq!(
        response.body_route_params[0].operand_json.as_deref(),
        Some("0.5")
    );
    assert_eq!(response.body_route_params[1].field, "taxed");
    assert_eq!(response.body_route_params[1].param, "price");
    assert_eq!(response.body_route_params[1].op.as_deref(), Some("add"));
    assert_eq!(
        response.body_route_params[1].operand_kind.as_deref(),
        Some("route_param_float")
    );
    assert_eq!(
        response.body_route_params[1].operand_name.as_deref(),
        Some("tax")
    );
    assert_eq!(response.body_route_params[2].op.as_deref(), Some("rsub"));
    assert_eq!(
        response.body_route_params[2].operand_json.as_deref(),
        Some("100.0")
    );
    assert_eq!(response.body_route_params[3].op.as_deref(), Some("rpow"));
    assert_eq!(
        response.body_route_params[3].operand_json.as_deref(),
        Some("2.0")
    );
    assert!(handlers.contains("routes::orv_native_param_value(route_match, \"price\")"));
    assert!(handlers.contains("routes::orv_native_param_value(route_match, \"tax\")"));
    assert!(handlers.contains(".trim().parse::<f64>()"));
    assert!(handlers.contains("let value = value * 0.5;"));
    assert!(handlers.contains("let value = value + operand;"));
    assert!(handlers.contains("let value = 100.0 - value;"));
    assert!(handlers.contains("let value = (2.0_f64).powf(value);"));
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
fn native_server_launcher_lowers_query_param_float_arithmetic_response_body() {
    let src = r"@server {
  @listen 8080
  @route GET /search {
    @respond 200 {
      total: (@query.amount as float) * (@query.quantity as float),
      ratio: 100.0 / (@query.parts as float)
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
    assert_eq!(response.body_query_params.len(), 2);
    assert_eq!(response.body_query_params[0].field, "total");
    assert_eq!(response.body_query_params[0].param, "amount");
    assert_eq!(
        response.body_query_params[0].value_kind,
        "query_param_float"
    );
    assert_eq!(response.body_query_params[0].op.as_deref(), Some("mul"));
    assert_eq!(
        response.body_query_params[0].operand_kind.as_deref(),
        Some("query_param_float")
    );
    assert_eq!(
        response.body_query_params[0].operand_name.as_deref(),
        Some("quantity")
    );
    assert_eq!(response.body_query_params[1].field, "ratio");
    assert_eq!(response.body_query_params[1].param, "parts");
    assert_eq!(response.body_query_params[1].op.as_deref(), Some("rdiv"));
    assert_eq!(
        response.body_query_params[1].operand_json.as_deref(),
        Some("100.0")
    );
    assert!(handlers.contains("routes::orv_native_query_value(route_match, \"amount\")"));
    assert!(handlers.contains("routes::orv_native_query_value(route_match, \"quantity\")"));
    assert!(handlers.contains("routes::orv_native_query_value(route_match, \"parts\")"));
    assert!(handlers.contains(".trim().parse::<f64>()"));
    assert!(handlers.contains("let value = value * operand;"));
    assert!(handlers.contains("let value = 100.0 / value;"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_query_param_bool_cast_response_body() {
    let src = r"@server {
  @listen 8080
  @route GET /search {
    @respond 200 { includeArchived: @query.includeArchived as bool }
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
    assert_eq!(response.body_query_params[0].field, "includeArchived");
    assert_eq!(response.body_query_params[0].param, "includeArchived");
    assert_eq!(response.body_query_params[0].value_kind, "query_param_bool");
    assert!(handlers.contains(
            "match routes::orv_native_query_value(route_match, \"includeArchived\").unwrap_or(\"\").trim()"
        ));
    assert!(handlers.contains(r#""true" => body.push_str("true")"#));
    assert!(handlers.contains(r#""false" => body.push_str("false")"#));
    assert!(!handlers.contains(
            "orv_native_push_json_string(routes::orv_native_query_value(route_match, \"includeArchived\")"
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
fn native_server_launcher_lowers_if_else_response_route() {
    let src = r#"@server {
  @listen 8080
  @route POST /inventory {
    if (@body.quantity as int) <= (@body.stock as int) {
      @respond 201 { accepted: true, quantity: @body.quantity as int }
    } else {
      @respond 409 { err: "out_of_stock" }
    }
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
    let condition = artifact.routes[0].responses[0]
        .condition
        .as_ref()
        .expect("guard condition");
    assert_eq!(condition.kind, "request_body_field_int_le");
    assert_eq!(condition.name, "quantity");
    assert_eq!(
        condition.operand_kind.as_deref(),
        Some("request_body_field_int")
    );
    assert_eq!(condition.operand_name.as_deref(), Some("stock"));
    assert!(artifact.routes[0].responses[1].condition.is_none());
    assert!(handlers.contains(
            "routes::orv_native_body_field_value(route_match, \"quantity\").unwrap_or(\"\").trim().parse::<i64>()"
        ));
    assert!(handlers.contains(
            "routes::orv_native_body_field_value(route_match, \"stock\").unwrap_or(\"\").trim().parse::<i64>()"
        ));
    assert!(handlers.contains("status: 201"));
    assert!(handlers.contains("status: 409"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_else_if_response_route() {
    let src = r#"@server {
  @listen 8080
  @route POST /inventory {
    if (@body.quantity as int) <= 0 {
      @respond 400 { err: "bad_quantity" }
    } else if (@body.quantity as int) <= (@body.stock as int) {
      @respond 201 { accepted: true, quantity: @body.quantity as int }
    } else {
      @respond 409 { err: "out_of_stock" }
    }
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

    assert_eq!(artifact.routes[0].responses.len(), 3);
    let invalid_condition = artifact.routes[0].responses[0]
        .condition
        .as_ref()
        .expect("invalid quantity condition");
    assert_eq!(invalid_condition.kind, "request_body_field_int_le");
    assert_eq!(invalid_condition.name, "quantity");
    assert_eq!(invalid_condition.value, "0");
    let stock_condition = artifact.routes[0].responses[1]
        .condition
        .as_ref()
        .expect("stock condition");
    assert_eq!(stock_condition.kind, "request_body_field_int_le");
    assert_eq!(stock_condition.name, "quantity");
    assert_eq!(
        stock_condition.operand_kind.as_deref(),
        Some("request_body_field_int")
    );
    assert_eq!(stock_condition.operand_name.as_deref(), Some("stock"));
    assert!(artifact.routes[0].responses[2].condition.is_none());
    assert!(handlers.contains("status: 400"));
    assert!(handlers.contains("status: 201"));
    assert!(handlers.contains("status: 409"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_block_wrapped_else_if_response_route() {
    let src = r#"@server {
  @listen 8080
  @route POST /inventory {
    if (@body.quantity as int) <= 0 {
      @respond 400 { err: "bad_quantity" }
    } else {
      if (@body.quantity as int) <= (@body.stock as int) {
        @respond 201 { accepted: true, quantity: @body.quantity as int }
      } else {
        @respond 409 { err: "out_of_stock" }
      }
    }
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

    assert_eq!(artifact.routes[0].responses.len(), 3);
    assert_eq!(
        artifact.routes[0].responses[0]
            .condition
            .as_ref()
            .map(|condition| condition.value.as_str()),
        Some("0")
    );
    assert_eq!(
        artifact.routes[0].responses[1]
            .condition
            .as_ref()
            .and_then(|condition| condition.operand_name.as_deref()),
        Some("stock")
    );
    assert!(artifact.routes[0].responses[2].condition.is_none());
    assert!(handlers.contains("status: 400"));
    assert!(handlers.contains("status: 201"));
    assert!(handlers.contains("status: 409"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_else_if_then_final_response_route() {
    let src = r#"@server {
  @listen 8080
  @route POST /inventory {
    if (@body.quantity as int) <= 0 {
      @respond 400 { err: "bad_quantity" }
    } else if (@body.quantity as int) <= (@body.stock as int) {
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

    assert_eq!(artifact.routes[0].responses.len(), 3);
    assert_eq!(
        artifact.routes[0].responses[0]
            .condition
            .as_ref()
            .map(|condition| condition.value.as_str()),
        Some("0")
    );
    assert_eq!(
        artifact.routes[0].responses[1]
            .condition
            .as_ref()
            .and_then(|condition| condition.operand_name.as_deref()),
        Some("stock")
    );
    assert!(artifact.routes[0].responses[2].condition.is_none());
    assert!(handlers.contains("status: 400"));
    assert!(handlers.contains("status: 201"));
    assert!(handlers.contains("status: 409"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_block_wrapped_else_if_then_final_response_route() {
    let src = r#"@server {
  @listen 8080
  @route POST /inventory {
    if (@body.quantity as int) <= 0 {
      @respond 400 { err: "bad_quantity" }
    } else {
      if (@body.quantity as int) <= (@body.stock as int) {
        @respond 201 { accepted: true, quantity: @body.quantity as int }
      }
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

    assert_eq!(artifact.routes[0].responses.len(), 3);
    assert_eq!(
        artifact.routes[0].responses[0]
            .condition
            .as_ref()
            .map(|condition| condition.value.as_str()),
        Some("0")
    );
    assert_eq!(
        artifact.routes[0].responses[1]
            .condition
            .as_ref()
            .and_then(|condition| condition.operand_name.as_deref()),
        Some("stock")
    );
    assert!(artifact.routes[0].responses[2].condition.is_none());
    assert!(handlers.contains("status: 400"));
    assert!(handlers.contains("status: 201"));
    assert!(handlers.contains("status: 409"));
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
fn native_server_launcher_lowers_request_body_bool_comparison_guard() {
    let src = r#"@server {
  @listen 8080
  @route POST /members {
    if (@body.subscribed as bool) == true {
      @respond 201 { subscribed: @body.subscribed as bool }
    }
    @respond 400 { err: "not_subscribed" }
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
    assert_eq!(condition.kind, "request_body_field_bool_eq");
    assert_eq!(condition.name, "subscribed");
    assert_eq!(condition.value, "true");
    assert!(artifact.routes[0].responses[1].condition.is_none());
    assert!(handlers.contains(
            "match routes::orv_native_body_field_value(route_match, \"subscribed\").unwrap_or(\"\").trim()"
        ));
    assert!(handlers.contains(r#""true" => true == true"#));
    assert!(handlers.contains(r#""false" => false == true"#));
    assert!(handlers.contains("status: 201"));
    assert!(handlers.contains("status: 400"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_request_body_bool_truthy_guard() {
    let src = r#"@server {
  @listen 8080
  @route POST /members {
    if (@body.subscribed as bool) {
      @respond 201 { subscribed: @body.subscribed as bool }
    }
    @respond 400 { err: "not_subscribed" }
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
    assert_eq!(condition.kind, "request_body_field_bool_eq");
    assert_eq!(condition.name, "subscribed");
    assert_eq!(condition.value, "true");
    assert!(artifact.routes[0].responses[1].condition.is_none());
    assert!(handlers.contains(
            "match routes::orv_native_body_field_value(route_match, \"subscribed\").unwrap_or(\"\").trim()"
        ));
    assert!(handlers.contains(r#""true" => true == true"#));
    assert!(handlers.contains(r#""false" => false == true"#));
    assert!(handlers.contains("status: 201"));
    assert!(handlers.contains("status: 400"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_request_body_bool_negated_guard() {
    let src = r#"@server {
  @listen 8080
  @route POST /members {
    if !(@body.subscribed as bool) {
      @respond 400 { err: "not_subscribed" }
    }
    @respond 201 { subscribed: @body.subscribed as bool }
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
    assert_eq!(condition.kind, "request_body_field_bool_ne");
    assert_eq!(condition.name, "subscribed");
    assert_eq!(condition.value, "true");
    assert!(artifact.routes[0].responses[1].condition.is_none());
    assert!(handlers.contains(
            "match routes::orv_native_body_field_value(route_match, \"subscribed\").unwrap_or(\"\").trim()"
        ));
    assert!(handlers.contains(r#""true" => true != true"#));
    assert!(handlers.contains(r#""false" => false != true"#));
    assert!(handlers.contains("status: 400"));
    assert!(handlers.contains("status: 201"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_request_body_query_bool_and_guard() {
    let src = r#"@server {
  @listen 8080
  @route POST /members {
    if (@body.subscribed as bool) && (@query.verified as bool) {
      @respond 201 { eligible: true }
    }
    @respond 403 { err: "not_eligible" }
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
    assert_eq!(condition.kind, "request_body_field_bool_and");
    assert_eq!(condition.name, "subscribed");
    assert_eq!(condition.operand_kind.as_deref(), Some("query_param_bool"));
    assert_eq!(condition.operand_name.as_deref(), Some("verified"));
    assert!(artifact.routes[0].responses[1].condition.is_none());
    assert!(handlers.contains(
            "match (routes::orv_native_body_field_value(route_match, \"subscribed\").unwrap_or(\"\").trim(), routes::orv_native_query_value(route_match, \"verified\").unwrap_or(\"\").trim())"
        ));
    assert!(handlers.contains(r#"("true", "true") => true && true"#));
    assert!(handlers.contains(r#"("true", "false") => true && false"#));
    assert!(handlers.contains(r#"("false", "true") => false && true"#));
    assert!(handlers.contains(r#"("false", "false") => false && false"#));
    assert!(handlers.contains("status: 201"));
    assert!(handlers.contains("status: 403"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_request_body_query_bool_or_guard() {
    let src = r#"@server {
  @listen 8080
  @route POST /members {
    if (@body.subscribed as bool) || (@query.override as bool) {
      @respond 201 { eligible: true }
    }
    @respond 403 { err: "not_eligible" }
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
    assert_eq!(condition.kind, "request_body_field_bool_or");
    assert_eq!(condition.name, "subscribed");
    assert_eq!(condition.operand_kind.as_deref(), Some("query_param_bool"));
    assert_eq!(condition.operand_name.as_deref(), Some("override"));
    assert!(artifact.routes[0].responses[1].condition.is_none());
    assert!(handlers.contains(
            "match (routes::orv_native_body_field_value(route_match, \"subscribed\").unwrap_or(\"\").trim(), routes::orv_native_query_value(route_match, \"override\").unwrap_or(\"\").trim())"
        ));
    assert!(handlers.contains(r#"("true", "true") => true || true"#));
    assert!(handlers.contains(r#"("true", "false") => true || false"#));
    assert!(handlers.contains(r#"("false", "true") => false || true"#));
    assert!(handlers.contains(r#"("false", "false") => false || false"#));
    assert!(handlers.contains("status: 201"));
    assert!(handlers.contains("status: 403"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_negated_request_body_query_bool_and_guard() {
    let src = r#"@server {
  @listen 8080
  @route POST /members {
    if !(@body.suspended as bool) && (@query.verified as bool) {
      @respond 201 { eligible: true }
    }
    @respond 403 { err: "not_eligible" }
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
    assert_eq!(condition.kind, "request_body_field_bool_not_and");
    assert_eq!(condition.name, "suspended");
    assert_eq!(condition.operand_kind.as_deref(), Some("query_param_bool"));
    assert_eq!(condition.operand_name.as_deref(), Some("verified"));
    assert!(artifact.routes[0].responses[1].condition.is_none());
    assert!(handlers.contains(
            "match (routes::orv_native_body_field_value(route_match, \"suspended\").unwrap_or(\"\").trim(), routes::orv_native_query_value(route_match, \"verified\").unwrap_or(\"\").trim())"
        ));
    assert!(handlers.contains(r#"("true", "true") => !true && true"#));
    assert!(handlers.contains(r#"("true", "false") => !true && false"#));
    assert!(handlers.contains(r#"("false", "true") => !false && true"#));
    assert!(handlers.contains(r#"("false", "false") => !false && false"#));
    assert!(handlers.contains("status: 201"));
    assert!(handlers.contains("status: 403"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_negated_request_body_query_bool_or_guard() {
    let src = r#"@server {
  @listen 8080
  @route POST /members {
    if !(@body.suspended as bool) || (@query.override as bool) {
      @respond 201 { eligible: true }
    }
    @respond 403 { err: "not_eligible" }
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
    assert_eq!(condition.kind, "request_body_field_bool_not_or");
    assert_eq!(condition.name, "suspended");
    assert_eq!(condition.operand_kind.as_deref(), Some("query_param_bool"));
    assert_eq!(condition.operand_name.as_deref(), Some("override"));
    assert!(artifact.routes[0].responses[1].condition.is_none());
    assert!(handlers.contains(
            "match (routes::orv_native_body_field_value(route_match, \"suspended\").unwrap_or(\"\").trim(), routes::orv_native_query_value(route_match, \"override\").unwrap_or(\"\").trim())"
        ));
    assert!(handlers.contains(r#"("true", "true") => !true || true"#));
    assert!(handlers.contains(r#"("true", "false") => !true || false"#));
    assert!(handlers.contains(r#"("false", "true") => !false || true"#));
    assert!(handlers.contains(r#"("false", "false") => !false || false"#));
    assert!(handlers.contains("status: 201"));
    assert!(handlers.contains("status: 403"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_request_body_negated_query_bool_and_guard() {
    let src = r#"@server {
  @listen 8080
  @route POST /members {
    if (@body.subscribed as bool) && !(@query.blocked as bool) {
      @respond 201 { eligible: true }
    }
    @respond 403 { err: "not_eligible" }
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
    assert_eq!(condition.kind, "request_body_field_bool_and_not");
    assert_eq!(condition.name, "subscribed");
    assert_eq!(condition.operand_kind.as_deref(), Some("query_param_bool"));
    assert_eq!(condition.operand_name.as_deref(), Some("blocked"));
    assert!(artifact.routes[0].responses[1].condition.is_none());
    assert!(handlers.contains(
            "match (routes::orv_native_body_field_value(route_match, \"subscribed\").unwrap_or(\"\").trim(), routes::orv_native_query_value(route_match, \"blocked\").unwrap_or(\"\").trim())"
        ));
    assert!(handlers.contains(r#"("true", "true") => true && !true"#));
    assert!(handlers.contains(r#"("true", "false") => true && !false"#));
    assert!(handlers.contains(r#"("false", "true") => false && !true"#));
    assert!(handlers.contains(r#"("false", "false") => false && !false"#));
    assert!(handlers.contains("status: 201"));
    assert!(handlers.contains("status: 403"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_request_body_negated_query_bool_or_guard() {
    let src = r#"@server {
  @listen 8080
  @route POST /members {
    if (@body.subscribed as bool) || !(@query.blocked as bool) {
      @respond 201 { eligible: true }
    }
    @respond 403 { err: "not_eligible" }
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
    assert_eq!(condition.kind, "request_body_field_bool_or_not");
    assert_eq!(condition.name, "subscribed");
    assert_eq!(condition.operand_kind.as_deref(), Some("query_param_bool"));
    assert_eq!(condition.operand_name.as_deref(), Some("blocked"));
    assert!(artifact.routes[0].responses[1].condition.is_none());
    assert!(handlers.contains(
            "match (routes::orv_native_body_field_value(route_match, \"subscribed\").unwrap_or(\"\").trim(), routes::orv_native_query_value(route_match, \"blocked\").unwrap_or(\"\").trim())"
        ));
    assert!(handlers.contains(r#"("true", "true") => true || !true"#));
    assert!(handlers.contains(r#"("true", "false") => true || !false"#));
    assert!(handlers.contains(r#"("false", "true") => false || !true"#));
    assert!(handlers.contains(r#"("false", "false") => false || !false"#));
    assert!(handlers.contains("status: 201"));
    assert!(handlers.contains("status: 403"));
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
fn native_server_launcher_lowers_request_body_int_scaled_comparison_guard() {
    let src = r#"@server {
  @listen 8080
  @route POST /orders {
    if (@body.quantity as int) <= ((@body.stock as int) * 10) {
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
    assert_eq!(condition.operand_scale_json.as_deref(), Some("10"));
    assert!(artifact.routes[0].responses[1].condition.is_none());
    assert!(handlers.contains(
            "routes::orv_native_body_field_value(route_match, \"quantity\").unwrap_or(\"\").trim().parse::<i64>()"
        ));
    assert!(handlers.contains(
            "routes::orv_native_body_field_value(route_match, \"stock\").unwrap_or(\"\").trim().parse::<i64>()"
        ));
    assert!(handlers.contains("operand.checked_mul(10)"));
    assert!(handlers.contains("Some(operand) => value <= operand"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_request_body_int_product_comparison_guard() {
    let src = r#"@server {
  @listen 8080
  @route POST /orders {
    if (@body.total as int) <= ((@body.quantity as int) * (@body.unit_price as int)) {
      @respond 201 { accepted: true, total: @body.total as int }
    }
    @respond 409 { err: "over_total" }
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
    assert_eq!(condition.name, "total");
    assert_eq!(condition.operand_name.as_deref(), Some("quantity"));
    assert_eq!(
        condition.operand_kind.as_deref(),
        Some("request_body_field_int")
    );
    assert_eq!(
        condition.secondary_operand_kind.as_deref(),
        Some("request_body_field_int")
    );
    assert_eq!(
        condition.secondary_operand_name.as_deref(),
        Some("unit_price")
    );
    assert!(artifact.routes[0].responses[1].condition.is_none());
    assert!(handlers.contains("product_left.checked_mul(product_right)"));
    assert!(handlers.contains("Some(operand) => value <= operand"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_request_body_int_scaled_product_comparison_guard() {
    let src = r#"@server {
  @listen 8080
  @route POST /orders {
    if (@body.total as int) <= (((@body.quantity as int) * (@body.unit_price as int)) * 100) {
      @respond 201 { accepted: true, total: @body.total as int }
    }
    @respond 409 { err: "over_total" }
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
    assert_eq!(condition.name, "total");
    assert_eq!(condition.operand_name.as_deref(), Some("quantity"));
    assert_eq!(
        condition.operand_kind.as_deref(),
        Some("request_body_field_int")
    );
    assert_eq!(condition.operand_scale_json.as_deref(), Some("100"));
    assert_eq!(
        condition.secondary_operand_kind.as_deref(),
        Some("request_body_field_int")
    );
    assert_eq!(
        condition.secondary_operand_name.as_deref(),
        Some("unit_price")
    );
    assert!(artifact.routes[0].responses[1].condition.is_none());
    assert!(handlers.contains("partial_product.checked_mul(100)"));
    assert!(handlers.contains("Some(operand) => value <= operand"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_left_scaled_product_int_comparison_guard() {
    let src = r#"@server {
  @listen 8080
  @route POST /orders {
    if (((@body.quantity as int) * (@body.unit_price as int)) * 100) <= (@body.total as int) {
      @respond 201 { accepted: true, total: @body.total as int }
    }
    @respond 409 { err: "over_total" }
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
    assert_eq!(condition.kind, "request_body_field_int_ge");
    assert_eq!(condition.name, "total");
    assert_eq!(condition.operand_name.as_deref(), Some("quantity"));
    assert_eq!(condition.operand_scale_json.as_deref(), Some("100"));
    assert_eq!(
        condition.secondary_operand_name.as_deref(),
        Some("unit_price")
    );
    assert!(handlers.contains("partial_product.checked_mul(100)"));
    assert!(handlers.contains("Some(operand) => value >= operand"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_request_body_int_triple_product_comparison_guard() {
    let src = r#"@server {
  @listen 8080
  @route POST /orders {
    if (@body.total as int) <= (((@body.quantity as int) * (@body.unit_price as int)) * (@body.bundle_count as int)) {
      @respond 201 { accepted: true, total: @body.total as int }
    }
    @respond 409 { err: "over_total" }
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
    assert_eq!(condition.name, "total");
    assert_eq!(condition.value, NATIVE_CONDITION_TRIPLE_PRODUCT);
    assert_eq!(condition.operand_name.as_deref(), Some("quantity"));
    assert_eq!(
        condition.operand_kind.as_deref(),
        Some("request_body_field_int")
    );
    assert_eq!(
        condition.secondary_operand_name.as_deref(),
        Some("unit_price")
    );
    assert_eq!(
        condition.tertiary_operand_name.as_deref(),
        Some("bundle_count")
    );
    assert!(artifact.routes[0].responses[1].condition.is_none());
    assert!(handlers.contains("partial_product.checked_mul(third_product)"));
    assert!(handlers.contains("Some(triple_product) => value <= triple_product"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_product_product_int_comparison_guard() {
    let src = r#"@server {
  @listen 8080
  @route POST /orders {
    if ((@body.quantity as int) * (@body.unit_price as int)) <= ((@body.stock as int) * (@body.reserve_price as int)) {
      @respond 201 { accepted: true, quantity: @body.quantity as int }
    }
    @respond 409 { err: "over_total" }
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
    assert_eq!(condition.operand_name.as_deref(), Some("unit_price"));
    assert_eq!(
        condition.operand_kind.as_deref(),
        Some("request_body_field_int")
    );
    assert_eq!(
        condition.secondary_operand_kind.as_deref(),
        Some("request_body_field_int")
    );
    assert_eq!(condition.secondary_operand_name.as_deref(), Some("stock"));
    assert_eq!(
        condition.tertiary_operand_kind.as_deref(),
        Some("request_body_field_int")
    );
    assert_eq!(
        condition.tertiary_operand_name.as_deref(),
        Some("reserve_price")
    );
    assert!(artifact.routes[0].responses[1].condition.is_none());
    assert!(handlers.contains("value.checked_mul(left_right)"));
    assert!(handlers.contains("right_left.checked_mul(right_right)"));
    assert!(handlers.contains("Some(right_product) => left_product <= right_product"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_product_static_int_comparison_guard() {
    let src = r#"@server {
  @listen 8080
  @route POST /orders {
    if ((@body.quantity as int) * (@body.unit_price as int)) <= 1000 {
      @respond 201 { accepted: true, quantity: @body.quantity as int }
    }
    @respond 409 { err: "over_total" }
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
    assert_eq!(condition.value, "1000");
    assert_eq!(condition.operand_name.as_deref(), Some("unit_price"));
    assert_eq!(
        condition.operand_kind.as_deref(),
        Some("request_body_field_int")
    );
    assert!(condition.secondary_operand_kind.is_none());
    assert!(condition.secondary_operand_name.is_none());
    assert!(artifact.routes[0].responses[1].condition.is_none());
    assert!(handlers.contains("value.checked_mul(product_right)"));
    assert!(handlers.contains("Some(product) => product <= 1000"));
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
fn native_server_launcher_lowers_product_static_float_comparison_guard() {
    let src = r#"@server {
  @listen 8080
  @route POST /payments {
    if ((@body.price as float) * (@body.quantity as float)) <= 40.0 {
      @respond 201 { accepted: true, amount: @body.price as float }
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
    assert_eq!(condition.name, "price");
    assert_eq!(condition.value, "40.0");
    assert_eq!(condition.operand_name.as_deref(), Some("quantity"));
    assert_eq!(
        condition.operand_kind.as_deref(),
        Some("request_body_field_float")
    );
    assert!(condition.secondary_operand_kind.is_none());
    assert!(condition.secondary_operand_name.is_none());
    assert!(artifact.routes[0].responses[1].condition.is_none());
    assert!(handlers.contains("let product = value * product_right;"));
    assert!(handlers.contains("product.is_finite() && product <= 40.0"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_request_body_float_triple_product_comparison_guard() {
    let src = r#"@server {
  @listen 8080
  @route POST /payments {
    if (@body.total as float) <= (((@body.price as float) * (@body.quantity as float)) * (@body.multiplier as float)) {
      @respond 201 { accepted: true, amount: @body.total as float }
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
    assert_eq!(condition.name, "total");
    assert_eq!(condition.value, NATIVE_CONDITION_TRIPLE_PRODUCT);
    assert_eq!(condition.operand_name.as_deref(), Some("price"));
    assert_eq!(
        condition.operand_kind.as_deref(),
        Some("request_body_field_float")
    );
    assert_eq!(
        condition.secondary_operand_name.as_deref(),
        Some("quantity")
    );
    assert_eq!(
        condition.tertiary_operand_name.as_deref(),
        Some("multiplier")
    );
    assert!(artifact.routes[0].responses[1].condition.is_none());
    assert!(
        handlers.contains("let triple_product = first_product * second_product * third_product;")
    );
    assert!(handlers.contains("value <= triple_product"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_request_body_float_scaled_product_comparison_guard() {
    let src = r#"@server {
  @listen 8080
  @route POST /payments {
    if (@body.total as float) <= (((@body.price as float) * (@body.quantity as float)) * 0.5) {
      @respond 201 { accepted: true, amount: @body.total as float }
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
    assert_eq!(condition.name, "total");
    assert_eq!(condition.operand_name.as_deref(), Some("price"));
    assert_eq!(
        condition.operand_kind.as_deref(),
        Some("request_body_field_float")
    );
    assert_eq!(condition.operand_scale_json.as_deref(), Some("0.5"));
    assert_eq!(
        condition.secondary_operand_kind.as_deref(),
        Some("request_body_field_float")
    );
    assert_eq!(
        condition.secondary_operand_name.as_deref(),
        Some("quantity")
    );
    assert!(artifact.routes[0].responses[1].condition.is_none());
    assert!(handlers.contains("let operand = partial_product * 0.5;"));
    assert!(handlers.contains("value <= operand"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
}

#[test]
fn native_server_launcher_lowers_product_product_float_comparison_guard() {
    let src = r#"@server {
  @listen 8080
  @route POST /payments {
    if ((@body.price as float) * (@body.quantity as float)) <= ((@body.limit_price as float) * (@body.limit_units as float)) {
      @respond 201 { accepted: true, amount: @body.price as float }
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
    assert_eq!(condition.name, "price");
    assert_eq!(condition.operand_name.as_deref(), Some("quantity"));
    assert_eq!(
        condition.operand_kind.as_deref(),
        Some("request_body_field_float")
    );
    assert_eq!(
        condition.secondary_operand_kind.as_deref(),
        Some("request_body_field_float")
    );
    assert_eq!(
        condition.secondary_operand_name.as_deref(),
        Some("limit_price")
    );
    assert_eq!(
        condition.tertiary_operand_kind.as_deref(),
        Some("request_body_field_float")
    );
    assert_eq!(
        condition.tertiary_operand_name.as_deref(),
        Some("limit_units")
    );
    assert!(artifact.routes[0].responses[1].condition.is_none());
    assert!(handlers.contains("let left_product = value * left_right;"));
    assert!(handlers.contains("let right_product = right_left * right_right;"));
    assert!(handlers.contains("left_product <= right_product"));
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
    assert!(handlers
        .contains("routes::orv_native_query_value(route_match, \"mode\") != Some(\"compact\")"));
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
    assert!(source.contains("pub policies: &'static [OrvNativeRoutePolicy]"));
    assert!(source.contains("pub struct OrvNativeRoutePolicy"));
    assert!(source.contains("pub const ORV_NATIVE_ROUTES"));
    assert!(source.contains("OrvNativeRoute { method: \"GET\", path: \"/ping\", origin_id:"));
    assert!(source.contains("pub fn orv_native_match_route("));
    assert!(source.contains("orv_native_route_path_params(route.path, path)"));
    assert!(source.contains(&format!("origin_id: \"{route_origin}\"")));
    assert!(source.contains(&format!(
        "response_origin_ids: &[{}]",
        native::rust_string_literal(&artifact.routes[0].response_origin_ids[0])
    )));
    assert!(source.contains("pub const ORV_NATIVE_ROUTE_COUNT: usize = ORV_NATIVE_ROUTES.len();"));
}

#[test]
fn native_server_routes_source_declares_route_policy_table() {
    let src = r#"@server {
  @listen 8080
  @route POST /checkout {
    @csrf
    @respond 201 { ok: true }
  }
}"#;
    let program = lower(src);
    let map = origin_map(&program);
    let manifest = build_manifest("server.orv", &map);
    let artifact =
        server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);
    let source = native_server_routes_source(&artifact);
    let csrf_origin = artifact.routes[0]
        .policies
        .iter()
        .find(|policy| policy.kind == "csrf")
        .and_then(|policy| policy.origin_id.as_deref())
        .expect("csrf policy origin");

    assert!(source.contains("pub struct OrvNativeRoutePolicy"));
    assert!(source.contains("policies: &[OrvNativeRoutePolicy"));
    assert!(source.contains("kind: \"csrf\""));
    assert!(source.contains(&format!("origin_id: Some(\"{csrf_origin}\")")));
    assert!(source.contains("required: Some(true)"));
    assert!(source.contains("kind: \"rate_limit\""));
    assert!(source.contains("limit: Some(10)"));
    assert!(source.contains("window_seconds: Some(60)"));
}

#[test]
fn native_server_routes_source_declares_explicit_rate_limit_policy_fields() {
    let src = r#"@server {
  @listen 8080
  @route POST /limited {
    @rateLimit key=@body.memberId limit=2 window="1m"
    @respond 201 { ok: true }
  }
}"#;
    let program = lower(src);
    let map = origin_map(&program);
    let manifest = build_manifest("server.orv", &map);
    let artifact =
        server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);
    let source = native_server_routes_source(&artifact);

    assert!(source.contains("pub struct OrvNativeRoutePolicy"));
    assert!(source.contains("kind: \"rate_limit\""));
    assert!(source.contains("origin_id: Some(\"ori_"));
    assert!(source.contains("key: Some(\"@body.memberId\")"));
    assert!(source.contains("exempt: None"));
    assert!(source.contains("limit: Some(2)"));
    assert!(source.contains("window_seconds: Some(60)"));
}

#[test]
fn native_server_routes_source_declares_csrf_exemption_policy_fields() {
    let src = r#"@server {
  @listen 8080
  @route POST /webhooks/custom {
    @csrf exempt
    @respond 200 { ok: true }
  }
}"#;
    let program = lower(src);
    let map = origin_map(&program);
    let manifest = build_manifest("server.orv", &map);
    let artifact =
        server_runtime_artifact_with_program(&manifest, &map, &program, [("server.orv", src)]);
    let source = native_server_routes_source(&artifact);

    assert!(source.contains("kind: \"csrf\""));
    assert!(source.contains("origin_id: Some(\"ori_"));
    assert!(source.contains("required: Some(false)"));
    assert!(source.contains("exempt: Some(true)"));
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
    assert!(source.contains("fn orv_native_route_path_params(pattern: &'static str, path: &str)"));
    assert!(source.contains("orv_native_match_route_segment(pattern_segment"));
    assert!(source.contains("fn orv_native_route_param_segment(segment: &str)"));
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
    assert!(source.contains("name: name.to_string()"));
    assert!(source.contains("value: value.to_string()"));
    assert!(source.contains("path_segment.strip_suffix(suffix)"));
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
    assert!(source
        .contains("response_origin_id: route_match.route.response_origin_ids.first().copied()"));
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
    assert!(source.contains(r#"const ORV_SERVER_ARTIFACT: &str = "server/app.orv-runtime.json";"#));
    assert!(source.contains(r#"const ORV_NATIVE_SERVER_PLAN: &str = "server/native-server.json";"#));
    assert!(source.contains("routes::ORV_NATIVE_ROUTE_COUNT"));
    assert!(source.contains(r#"routes::orv_native_match_route("__orv_probe__", "__orv_probe__")"#));
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
    @respond 201 { received: (@body.id as int) + ((((@body.bonus as int) * (@body.scale as int)) * (@body.extra as int)) * (@body.more as int)) }
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
