use super::*;

fn workspace_path(parts: &[&str]) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("../..");
    for part in parts {
        path.push(part);
    }
    path
}

fn orv_files_under(parts: &[&str]) -> Vec<PathBuf> {
    let root = workspace_path(parts);
    let mut files = Vec::new();
    collect_orv_files(&root, &mut files);
    files.sort();
    files
}

fn collect_orv_files(root: &Path, out: &mut Vec<PathBuf>) {
    for entry in
        std::fs::read_dir(root).unwrap_or_else(|e| panic!("failed to read {}: {e}", root.display()))
    {
        let path = entry.expect("fixture dir entry").path();
        if path.is_dir() {
            collect_orv_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "orv") {
            out.push(path);
        }
    }
}

fn temp_output_dir(name: &str) -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after unix epoch")
        .as_nanos();
    let mut path = std::env::temp_dir();
    path.push(format!("orv-cli-{name}-{}-{unique}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    path
}

fn adapter_values_without_source_origin_ids(value: &serde_json::Value) -> serde_json::Value {
    let mut value = value.clone();
    for adapter in value.as_array_mut().expect("adapter array") {
        adapter
            .as_object_mut()
            .expect("adapter object")
            .remove("source_origin_id");
        adapter
            .as_object_mut()
            .expect("adapter object")
            .remove("source_origin_ids");
    }
    value
}

fn corrupt_origin_entry_kind_and_graph(build_dir: &Path, origin_id: &str, kind: &str, name: &str) {
    let origin_map_path = build_dir.join("origin-map.json");
    let mut origin_map = read_json_value(&origin_map_path).expect("origin map");
    let entry = origin_map["entries"]
        .as_array_mut()
        .expect("origin entries")
        .iter_mut()
        .find(|entry| entry["id"] == origin_id)
        .expect("origin entry");
    entry["kind"] = serde_json::json!(kind);
    entry["name"] = serde_json::json!(name);
    write_json(&origin_map_path, &origin_map).expect("write corrupt origin map");

    let graph_path = build_dir.join("project-graph.json");
    let mut graph = read_json_value(&graph_path).expect("project graph");
    graph["semantic"]["origin_map"] = origin_map;
    write_json(&graph_path, &graph).expect("write corrupt graph origin map");
}

fn workspace_build_fixture(name: &str) -> PathBuf {
    let root = temp_output_dir(name);
    std::fs::create_dir_all(root.join("apps/web/src")).expect("create web src");
    std::fs::create_dir_all(root.join("shared/models/src")).expect("create models src");
    std::fs::write(
        root.join("orv.toml"),
        r#"[workspace]
resolver = "2"
members = ["apps/web", "shared/models"]
"#,
    )
    .expect("write root manifest");
    std::fs::write(
        root.join("apps/web/orv.toml"),
        r#"[project]
name = "web"
version = "0.1.0"
entry = "src/main.orv"

[dependencies]
models = { path = "../../shared/models", version = "0.1.0" }
"#,
    )
    .expect("write web manifest");
    std::fs::write(
        root.join("shared/models/orv.toml"),
        r#"[project]
name = "models"
version = "0.1.0"
entry = "src/main.orv"
"#,
    )
    .expect("write models manifest");
    std::fs::write(
        root.join("apps/web/src/main.orv"),
        r#"@out @html { @body { @h1 "Web" } }"#,
    )
    .expect("write web source");
    std::fs::write(
        root.join("shared/models/src/main.orv"),
        r#"@out @html { @body { @h1 "Models" } }"#,
    )
    .expect("write models source");
    root
}

fn send_raw_http(address: &str, path: &str) -> String {
    let mut last_error = None;
    for _ in 0..20 {
        match send_raw_http_once(address, path) {
            Ok(response) if !response.is_empty() => return response,
            Ok(_) => last_error = Some("empty response".to_string()),
            Err(err) => last_error = Some(err.to_string()),
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!(
        "read http response: {}",
        last_error.unwrap_or_else(|| "no response".to_string())
    );
}

fn send_raw_http_once(address: &str, path: &str) -> std::io::Result<String> {
    let mut stream = std::net::TcpStream::connect(address)?;
    std::io::Write::write_all(
        &mut stream,
        format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").as_bytes(),
    )?;
    let mut response = String::new();
    std::io::Read::read_to_string(&mut stream, &mut response)?;
    Ok(response)
}

fn send_raw_http_json_post(address: &str, path: &str, body: &str) -> String {
    let mut last_error = None;
    for _ in 0..20 {
        match send_raw_http_json_post_once(address, path, body) {
            Ok(response) if !response.is_empty() => return response,
            Ok(_) => last_error = Some("empty response".to_string()),
            Err(err) => last_error = Some(err.to_string()),
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!(
        "read http response: {}",
        last_error.unwrap_or_else(|| "no response".to_string())
    );
}

fn send_raw_http_json_post_once(address: &str, path: &str, body: &str) -> std::io::Result<String> {
    let mut stream = std::net::TcpStream::connect(address)?;
    std::io::Write::write_all(
            &mut stream,
            format!(
                "POST {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        )?;
    let mut response = String::new();
    std::io::Read::read_to_string(&mut stream, &mut response)?;
    Ok(response)
}

struct ChildGuard(std::process::Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn_one_shot_http_json(path: &'static str, body: Vec<u8>) -> (String, JoinHandle<()>) {
    spawn_one_shot_http_json_with_optional_auth(path, body, None)
}

fn spawn_one_shot_http_json_with_auth(
    path: &'static str,
    body: Vec<u8>,
    expected_authorization: &'static str,
) -> (String, JoinHandle<()>) {
    spawn_one_shot_http_json_with_optional_auth(path, body, Some(expected_authorization))
}

fn spawn_one_shot_http_json_with_optional_auth(
    path: &'static str,
    body: Vec<u8>,
    expected_authorization: Option<&'static str>,
) -> (String, JoinHandle<()>) {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind registry");
    let addr = listener.local_addr().expect("registry address");
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept registry request");
        let mut request = Vec::new();
        let mut buffer = [0_u8; 512];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") && request.len() < 4096 {
            let read =
                std::io::Read::read(&mut stream, &mut buffer).expect("read registry request");
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
        }
        let request = String::from_utf8_lossy(&request);
        assert!(
            request.starts_with(&format!("GET {path} HTTP/1.1")),
            "{request}"
        );
        if let Some(expected_authorization) = expected_authorization {
            assert!(
                request
                    .lines()
                    .any(|line| line == format!("Authorization: {expected_authorization}")),
                "{request}"
            );
        }
        let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
        std::io::Write::write_all(&mut stream, response.as_bytes())
            .expect("write registry response head");
        std::io::Write::write_all(&mut stream, &body).expect("write registry response body");
    });
    (format!("http://{addr}"), handle)
}

fn dap_test_request(
    session: &mut DapSession,
    seq: u64,
    command: &str,
    arguments: serde_json::Value,
) -> serde_json::Value {
    let mut request = serde_json::json!({
        "seq": seq,
        "type": "request",
        "command": command,
    });
    request["arguments"] = arguments;
    session
        .message_response(&request)
        .unwrap_or_else(|| panic!("{command} response"))
}

fn prod_server_source(name: &str) -> (PathBuf, PathBuf) {
    let dir = temp_output_dir(name);
    std::fs::create_dir_all(&dir).expect("create prod source dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        "@server { @listen 8080 @route GET /ping { @respond 200 { ok: true } } }\n",
    )
    .expect("write prod source");
    (dir, path)
}

fn env_prod_server_source(name: &str) -> (PathBuf, PathBuf) {
    let dir = temp_output_dir(name);
    std::fs::create_dir_all(&dir).expect("create env prod source dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        r#"@server {
  @listen int.from(@env.PORT ?? "8080")
  @route GET /ping { @respond 200 { ok: true } }
}
"#,
    )
    .expect("write env prod source");
    (dir, path)
}

fn json_routes_include(routes: &serde_json::Value, method: &str, path: &str) -> bool {
    routes.as_array().is_some_and(|routes| {
        routes
            .iter()
            .any(|route| route["method"] == method && route["path"] == path)
    })
}

fn json_route<'a>(
    routes: &'a serde_json::Value,
    method: &str,
    path: &str,
) -> Option<&'a serde_json::Value> {
    routes.as_array()?.iter().find(|route| {
        route["method"] == serde_json::json!(method) && route["path"] == serde_json::json!(path)
    })
}

fn native_routes_source_includes(source: &str, method: &str, path: &str) -> bool {
    source.contains(&format!(
        "OrvNativeRoute {{ method: {method:?}, path: {path:?},"
    ))
}

fn protocol_frames(output: &str) -> Vec<serde_json::Value> {
    let mut offset = 0;
    let mut frames = Vec::new();
    while offset < output.len() {
        let tail = &output[offset..];
        let (headers, _) = tail
            .split_once("\r\n\r\n")
            .expect("content-length response frame");
        let content_length = headers
            .lines()
            .find_map(|line| {
                line.strip_prefix("Content-Length: ")
                    .and_then(|value| value.parse::<usize>().ok())
            })
            .expect("content length header");
        let body_start = offset + headers.len() + "\r\n\r\n".len();
        let body_end = body_start + content_length;
        let body = output.get(body_start..body_end).expect("complete body");
        frames.push(serde_json::from_str(body).expect("response json"));
        offset = body_end;
    }
    frames
}

fn protocol_request_frame(body: &serde_json::Value) -> String {
    let body = body.to_string();
    format!("Content-Length: {}\r\n\r\n{}", body.len(), body)
}

#[test]
fn check_accepts_all_e2e_fixtures() {
    let files = orv_files_under(&["fixtures", "e2e"]);
    assert!(!files.is_empty(), "expected e2e fixtures");
    for file in files {
        cmd_check(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
    }
}

#[test]
fn check_accepts_plan_and_default_fixtures() {
    let mut files = orv_files_under(&["fixtures", "plan"]);
    files.push(workspace_path(&["fixtures", "default-syntax.orv"]));
    assert!(!files.is_empty(), "expected plan fixtures");
    for file in files {
        cmd_check(&file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
    }
}

#[test]
fn check_accepts_orv_toml_project_entry() {
    let dir = temp_output_dir("project-manifest-check");
    let src = dir.join("src");
    std::fs::create_dir_all(&src).expect("create src dir");
    let entry = src.join("main.orv");
    std::fs::write(&entry, "@out \"manifest check\"\n").expect("write entry");
    let manifest = dir.join("orv.toml");
    std::fs::write(
        &manifest,
        r#"[project]
name = "manifest-demo"
entry = "src/main.orv"
"#,
    )
    .expect("write manifest");

    cmd_check(&manifest).expect("manifest check");

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn graph_subcommand_is_accepted() {
    let parsed = Cli::try_parse_from(["orv", "graph", "fixtures/e2e/hello.orv"]);
    if let Err(err) = parsed {
        panic!("{}", err.render());
    }
}

#[test]
fn graph_view_subcommand_is_accepted() {
    let parsed = Cli::try_parse_from([
        "orv",
        "graph",
        "fixtures/e2e/hello.orv",
        "--view",
        "--out",
        "target/orv-graph-view",
    ]);
    if let Err(err) = parsed {
        panic!("{}", err.render());
    }
}

#[test]
fn init_subcommand_is_accepted() {
    let parsed = Cli::try_parse_from(["orv", "init", "target/new-shop", "--name", "new-shop"]);
    if let Err(err) = parsed {
        panic!("{}", err.render());
    }
}

#[test]
fn test_subcommand_is_accepted() {
    let parsed = Cli::try_parse_from(["orv", "test", "src/models", "--filter", "user"]);
    if let Err(err) = parsed {
        panic!("{}", err.render());
    }
}

#[test]
fn test_list_flag_is_accepted() {
    let parsed = Cli::try_parse_from(["orv", "test", "--list", "src/models"]);
    let cli = match parsed {
        Ok(cli) => cli,
        Err(err) => panic!("{}", err.render()),
    };
    match cli.command {
        Command::Test { path, filter, list } => {
            assert_eq!(path, PathBuf::from("src/models"));
            assert_eq!(filter, None);
            assert!(list);
        }
        _ => panic!("unexpected command"),
    }
}

#[test]
fn test_list_json_discovers_filtered_tests_without_running_them() {
    let dir = temp_output_dir("test-runner-list");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("checkout_test.orv");
    std::fs::write(
        &source,
        r#"test "checkout shows cart" {
  assert true
}

test "checkout failing runtime body" {
  assert false
}
"#,
    )
    .expect("write test source");

    let value = orv_test_list_json(&dir, Some("shows")).expect("test list");
    let tests = value["tests"].as_array().expect("tests array");

    assert_eq!(value["schema_version"], 1);
    assert_eq!(tests.len(), 1);
    assert_eq!(tests[0]["name"], "checkout shows cart");
    assert_eq!(tests[0]["path"], source.display().to_string());
    assert_eq!(tests[0]["line"], 1);
    assert_eq!(tests[0]["column"], 1);
    assert_eq!(tests[0]["span"]["start"], 0);
    assert!(tests[0]["span"]["end"].as_u64().is_some_and(|end| end > 0));
    assert_eq!(tests[0]["range"]["start"]["line"], 0);
    assert_eq!(tests[0]["range"]["start"]["character"], 0);
    assert_eq!(tests[0]["range"]["end"]["line"], 2);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn test_summary_discovers_and_runs_matching_tests() {
    let dir = temp_output_dir("test-runner-pass");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("math_test.orv");
    std::fs::write(
        &source,
        r#"test "math adds" {
  assert 1 + 2 == 3
}
"#,
    )
    .expect("write test source");

    let summary = orv_test_summary(&dir, Some("math")).expect("test summary");

    assert_eq!(summary.selected, 1);
    assert_eq!(summary.passed, 1);
    assert_eq!(summary.failed, 0);
    assert!(summary.files.iter().any(|file| file == &source));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn test_summary_runs_only_matching_test_blocks() {
    let dir = temp_output_dir("test-runner-filter-isolation");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("checkout_test.orv");
    std::fs::write(
        &source,
        r#"test "checkout only" {
  assert true
}

test "checkout excluded failure" {
  assert false
}
"#,
    )
    .expect("write test source");

    let summary = orv_test_summary(&dir, Some("only")).expect("test summary");

    assert_eq!(summary.selected, 1);
    assert_eq!(summary.passed, 1);
    assert_eq!(summary.failed, 0);
    assert_eq!(summary.files, vec![source.clone()]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn test_summary_reports_runtime_failures() {
    let dir = temp_output_dir("test-runner-fail");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("math_test.orv");
    std::fs::write(
        &source,
        r#"test "math fails" {
  assert 1 + 2 == 4
}
"#,
    )
    .expect("write test source");

    let err = orv_test_summary(&dir, None).expect_err("failing test should fail");

    assert!(err.to_string().contains("math_test.orv"));
    assert!(err.to_string().contains("assertion failed"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn init_writes_project_manifest_and_entry() {
    let dir = temp_output_dir("init-project");

    cmd_init(&dir, Some("starter-shop"), InitTemplate::Basic).expect("init project");

    let manifest = dir.join("orv.toml");
    let entry = dir.join("src").join("main.orv");
    assert!(manifest.is_file(), "missing {}", manifest.display());
    assert!(entry.is_file(), "missing {}", entry.display());
    let manifest_text = std::fs::read_to_string(&manifest).expect("manifest text");
    assert!(manifest_text.contains("name = \"starter-shop\""));
    assert!(manifest_text.contains("entry = \"src/main.orv\""));
    cmd_check(&manifest).expect("check manifest project");
    cmd_check(&dir).expect("check project directory");
    let out = dir.join("dist");
    cmd_build(&dir, &out).expect("build project directory");
    assert!(out.join("pages").join("index.html").is_file());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn init_accepts_shop_template_flag() {
    let parsed = Cli::try_parse_from(["orv", "init", "target/new-shop", "--template", "shop"]);
    if let Err(err) = parsed {
        panic!("{}", err.render());
    }
}

#[test]
fn init_shop_template_scaffolds_shopping_routes() {
    let dir = temp_output_dir("init-shop-template");

    cmd_init(&dir, Some("starter-shop"), InitTemplate::Shop).expect("init shop project");

    let entry = dir.join("src").join("main.orv");
    let source = std::fs::read_to_string(&entry).expect("entry source");
    assert!(source.contains("@listen 8080"));
    assert!(source.contains(
        r#"let shopdb = @db.connect(@env.SHOP_DATABASE_URL ?? "sqlite://data/shop.sqlite")"#
    ));
    assert!(source.contains("@design"));
    assert!(source.contains("@colors"));
    assert!(source.contains(r##"primary: "#315c5a""##));
    assert!(source.contains("@spacing"));
    assert!(source.contains("@typography"));
    assert!(source.contains("@design.colors.surface"));
    assert!(source.contains("@design.spacing.lg"));
    assert!(source.contains("@design.typography.fontFamily"));
    assert!(source.contains("@route GET / {\n"));
    assert!(source.contains("@serve @html"));
    assert!(source.contains("@a href=\"/catalog\" \"Shop catalog\""));
    assert!(source.contains("@route GET /catalog"));
    assert!(source.contains("Shop Catalog"));
    assert!(source.contains("@a href=\"/cart\" \"Cart\""));
    assert!(source.contains("@form action=\"/cart/items\" method=post"));
    assert!(source.contains("@route GET /cart"));
    assert!(source.contains("@route POST /cart/items"));
    assert!(source.contains("@a href=\"/account/sessions\" \"My sessions\""));
    assert!(source.contains("@route GET /account/sessions"));
    assert!(source.contains("Account Sessions"));
    assert!(source.contains("@a href=\"/admin\" \"Admin dashboard\""));
    assert!(source.contains("@route GET /admin"));
    assert!(source.contains("@Auth required role=\"admin\""));
    assert!(source.matches("@Auth required role=\"admin\"").count() >= 8);
    assert!(source.contains(r#"handle: "admin""#));
    assert!(source.contains(r#"email: "admin@example.test""#));
    assert!(source.contains("Operations dashboard"));
    assert!(source.contains("@a href=\"/admin/summary\" \"Operations summary\""));
    assert!(source.contains("@route GET /admin/summary"));
    assert!(source.contains("@a href=\"/admin/catalog\" \"Catalog read model\""));
    assert!(source.contains("@route GET /admin/catalog"));
    assert!(source.contains("@a href=\"/admin/orders\" \"Order read model\""));
    assert!(source.contains("@route GET /admin/orders"));
    assert!(source.contains("@a href=\"/admin/payments\" \"Payment read model\""));
    assert!(source.contains("@route GET /admin/payments"));
    assert!(source.contains("@a href=\"/admin/shipments\" \"Shipment read model\""));
    assert!(source.contains("@route GET /admin/shipments"));
    assert!(source.contains("@a href=\"/admin/webhooks\" \"Webhook read model\""));
    assert!(source.contains("@route GET /admin/webhooks"));
    assert!(source.contains("@a href=\"/admin/audit\" \"Audit read model\""));
    assert!(source.contains("@route GET /admin/audit"));
    assert!(source.contains(r#"shopdb.count("Product", {})"#));
    assert!(source.contains(r#"shopdb.count("WebhookEvent", {})"#));
    assert!(source.contains(r#"shopdb.count("AuditEvent", {})"#));
    assert!(source.contains(r#"shopdb.findAll("Order", {})"#));
    assert!(source.contains(r#"shopdb.findAll("Payment", {})"#));
    assert!(source.contains(r#"shopdb.findAll("Shipment", {})"#));
    assert!(source.contains(r#"shopdb.findAll("WebhookEvent", {})"#));
    assert!(source.contains(r#"shopdb.findAll("AuditEvent", {})"#));
    assert!(source.contains("@form action=\"/products\" method=post"));
    assert!(source.contains("badge: string(trim, min=1)"));
    assert!(source.contains("@input type=text name=badge value=\"New arrival\" required"));
    assert!(source.contains("badge: @body.badge"));
    assert!(source.contains("{product.badge}"));
    assert!(source.contains("@input type=number name=stock required"));
    assert!(source.contains("@form action=\"/checkout\" method=post"));
    assert!(source.contains("@input type=password name=password required"));
    assert!(source.contains("@input type=hidden name=_csrf value=\"orv-reference-csrf\""));
    assert!(source.matches("@csrf").count() >= 8);
    assert!(source.contains("struct ProductInput"));
    assert!(source.contains("struct CheckoutInput"));
    assert!(source.contains("@body: ProductInput"));
    assert!(source.contains("@body: MemberSignupInput"));
    assert!(source.contains("@body: MemberLoginInput"));
    assert!(source.contains("@body: CartItemInput"));
    assert!(source.contains("@body: OrderInput"));
    assert!(source.contains("@body: CheckoutInput"));
    assert!(source.contains("@body: PaymentInput"));
    assert!(source.contains("@body: ShipmentInput"));
    assert!(source.contains("@route POST /checkout"));
    assert!(source.contains("One-step checkout"));
    assert!(source.contains("@route POST /members"));
    assert!(source.contains(r#"role: "member""#));
    assert!(source.contains("hash.password(@body.password)"));
    assert!(source.contains("hash.verify(@body.password, member.passwordHash)"));
    assert!(source.contains("admin-reference-password"));
    assert!(source.contains("passwordHash: passwordHash"));
    assert!(source.contains("@form action=\"/members/login\" method=post"));
    assert!(source.contains("@route POST /members/login"));
    assert!(source.contains(r#"shopdb.create("Session""#));
    assert!(source.contains(r#"role: member.role ?? "member""#));
    assert!(source.contains("@route POST /payments"));
    assert!(source.contains("@route POST /webhooks/stripe"));
    assert!(source.contains(r#"@header["stripe-signature"]"#));
    assert!(source.contains("payments.verifyWebhook"));
    assert!(source.contains("let eventId = @body.id"));
    assert!(source.contains(r#"shopdb.find("WebhookEvent""#));
    assert!(source.contains("duplicate: true"));
    assert!(source.contains("let mut reconciledPayment = void"));
    assert!(source.contains(r#"let reconcileOrderId = @body["orderId"]"#));
    assert!(source.contains(r#"let reconcilePaymentStatus = @body["paymentStatus"]"#));
    assert!(source.contains(r#"let reconcileOrderStatus = @body["orderStatus"]"#));
    assert!(source.contains(r#"shopdb.update("Payment", { orderId: reconciledOrderId }"#));
    assert!(source.contains(r#"shopdb.update("Order", { id: reconciledOrderId }"#));
    assert!(source.contains("reconciledPayment: reconciledPayment"));
    assert!(source.contains(r#"shopdb.create("WebhookEvent""#));
    assert!(source.contains(r#"shopdb.create("AuditEvent""#));
    assert!(source.contains("checkout.complete"));
    assert!(source.contains("payment.capture"));
    assert!(source.contains("shipment.book"));
    assert!(source.contains("webhook.received"));
    assert!(source.contains("@route POST /shipments"));
    assert!(source
        .contains(r#"@payment.connect(@env.PAYMENT_ADAPTER_URL ?? "file://data/payments.jsonl")"#));
    assert!(source.contains(
        r#"@shipping.connect(@env.SHIPPING_ADAPTER_URL ?? "file://data/shipments.jsonl")"#
    ));
    cmd_check(&dir).expect("check shop project");
    let out = dir.join("dist");
    cmd_build_with_profile(&dir, &out, BuildProfile::Production).expect("build shop project");
    assert!(out.join("server").join("app.orv-runtime.json").is_file());
    assert!(out.join("deploy").join("manifest.json").is_file());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn init_shop_template_writes_deploy_guide() {
    let dir = temp_output_dir("init-shop-guide");

    cmd_init(&dir, Some("starter-shop"), InitTemplate::Shop).expect("init shop project");

    let guide = std::fs::read_to_string(dir.join("README.md")).expect("shop README");
    assert!(guide.contains("starter-shop"));
    assert!(guide.contains("orv check ."));
    assert!(guide.contains("orv build . --prod --out dist"));
    assert!(guide.contains("orv verify-build dist"));
    assert!(guide.contains("orv deploy-env-check dist"));
    assert!(guide.contains("orv benchmark-report dist"));
    assert!(guide.contains("orv benchmark-report dist --require-pass"));
    assert!(guide.contains("keeps the local reference server in the foreground"));
    assert!(guide.contains("sh dist/deploy/smoke-test.sh"));
    assert!(guide.contains("deploy/README.md"));
    assert!(guide.contains("deploy/compose.yaml"));
    assert!(guide.contains("deploy/env.example"));
    assert!(guide.contains("deploy/db-adapters.json"));
    assert!(guide.contains("deploy/commerce-adapters.json"));
    assert!(guide.contains("deploy/preflight.json"));
    assert!(guide.contains("deploy/benchmark-evidence.json"));
    assert!(guide.contains("deploy/smoke-output.txt"));
    assert!(guide.contains("- `pass_marker`"));
    assert!(guide.contains("- `dap_source_bundle`"));
    assert!(guide.contains("- `trace_stream_requested`"));
    assert!(guide.contains("5-hour shop benchmark"));
    assert!(guide.contains("deploy/smoke-test.sh"));
    assert!(guide.contains("server/native-server.json"));
    assert!(guide.contains("server/native/Cargo.toml"));
    assert!(guide.contains("server/native/main.rs"));
    assert!(guide.contains("server/native/routes.rs"));
    assert!(guide.contains("server/native/router.rs"));
    assert!(guide.contains("server/native/handlers.rs"));
    assert!(guide.contains("cd dist"));
    assert!(guide.contains("PORT=8080 docker compose -f deploy/compose.yaml up --build -d"));
    assert!(guide.contains("./deploy/smoke-test.sh"));
    assert!(guide.contains("cargo build --manifest-path dist/server/native/Cargo.toml --release"));
    assert!(
        guide.contains("ORV_BUILD_DIR=dist ./dist/server/native/target/release/orv-native-server")
    );
    assert!(guide.contains("The generated launcher path can infer `dist`"));
    assert!(guide.contains("Persistent database: `data/shop.sqlite`"));
    assert!(guide.contains("SHOP_DATABASE_URL"));
    assert!(guide.contains("Commerce records: `data/payments.jsonl`, `data/shipments.jsonl`"));
    assert!(guide.contains("PAYMENT_ADAPTER_URL"));
    assert!(guide.contains("SHIPPING_ADAPTER_URL"));
    assert!(guide.contains("provider-mode adapters"));
    assert!(guide.contains("stripe://"));
    assert!(guide.contains("carrier://"));
    assert!(guide.contains("STRIPE_SECRET_KEY"));
    assert!(guide.contains("STRIPE_WEBHOOK_SECRET"));
    assert!(guide.contains("STRIPE_WEBHOOK_SECRET_PREVIOUS"));
    assert!(guide.contains("CARRIER_API_KEY"));
    assert!(guide.contains("CARRIER_WEBHOOK_SECRET"));
    assert!(guide.contains("Compose mounts `data/` into `/app/data`"));
    assert!(guide.contains("Back up `data/shop.sqlite` and commerce record logs"));
    assert!(guide.contains("Browser home"));
    assert!(guide.contains("http://localhost:8080/"));
    assert!(guide.contains("Theme tokens"));
    assert!(guide.contains("@design"));
    assert!(guide.contains("@colors"));
    assert!(guide.contains("@spacing"));
    assert!(guide.contains("@typography"));
    assert!(guide.contains("Product field edits"));
    assert!(guide.contains("ProductInput.badge"));
    assert!(guide.contains("/admin/catalog"));
    assert!(guide.contains("Admin dashboard: http://localhost:8080/admin"));
    assert!(guide.contains("@Auth required role=\"admin\""));
    assert!(guide.contains("admin@example.test"));
    assert!(guide.contains("Argon2"));
    assert!(guide.contains("hash.password"));
    assert!(guide.contains("hash.verify"));
    assert!(guide.contains("never persists plaintext passwords"));
    assert!(guide.contains("orv_session"));
    assert!(guide.contains("orv_session_role"));
    assert!(guide.contains("HttpOnly"));
    assert!(guide.contains("SameSite=Lax"));
    assert!(guide.contains("Secure"));
    assert!(guide.contains("@session required"));
    assert!(guide.contains("@session.id"));
    assert!(guide.contains("@csrf"));
    assert!(guide.contains("orv_csrf"));
    assert!(guide.contains("GET /"));
    assert!(guide.contains("GET /catalog"));
    assert!(guide.contains("GET /cart"));
    assert!(guide.contains("GET /account/sessions"));
    assert!(guide.contains("GET /admin"));
    assert!(guide.contains("GET /admin/catalog"));
    assert!(guide.contains("GET /admin/summary"));
    assert!(guide.contains("GET /admin/orders"));
    assert!(guide.contains("GET /admin/payments"));
    assert!(guide.contains("GET /admin/shipments"));
    assert!(guide.contains("GET /admin/webhooks"));
    assert!(guide.contains("GET /admin/audit"));
    assert!(guide.contains("POST /members"));
    assert!(guide.contains("POST /members/login"));
    assert!(guide.contains("POST /checkout"));
    assert!(guide.contains("POST /cart/items"));
    assert!(guide.contains("POST /payments"));
    assert!(guide.contains("POST /webhooks/stripe"));
    assert!(guide.contains("Stripe webhook"));
    assert!(guide.contains("POST /shipments"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn init_shop_template_prod_artifacts_keep_full_service_routes() {
    let dir = temp_output_dir("init-shop-prod-routes");

    cmd_init(&dir, Some("starter-shop"), InitTemplate::Shop).expect("init shop project");
    let out = dir.join("dist");
    cmd_build_with_profile(&dir, &out, BuildProfile::Production).expect("build shop project");

    let manifest = read_json_value(&out.join("build-manifest.json")).expect("manifest");
    let deploy = read_json_value(&out.join("deploy").join("manifest.json")).expect("deploy");
    let runtime =
        read_json_value(&out.join("server").join("app.orv-runtime.json")).expect("runtime");
    let container = read_json_value(&out.join("deploy").join("container.json")).expect("container");
    let compose =
        std::fs::read_to_string(out.join("deploy").join("compose.yaml")).expect("compose");
    let env_example =
        std::fs::read_to_string(out.join("deploy").join("env.example")).expect("env example");
    let commerce_adapters = read_json_value(&out.join("deploy").join("commerce-adapters.json"))
        .expect("commerce adapters");
    let preflight = read_json_value(&out.join("deploy").join("preflight.json")).expect("preflight");
    let benchmark_evidence = read_json_value(&out.join("deploy").join("benchmark-evidence.json"))
        .expect("benchmark evidence");
    let smoke_test =
        std::fs::read_to_string(out.join("deploy").join("smoke-test.sh")).expect("smoke test");
    let native_routes =
        std::fs::read_to_string(out.join("server").join("native").join("routes.rs"))
            .expect("native routes source");
    for (method, path) in [
        ("GET", "/"),
        ("GET", "/catalog"),
        ("GET", "/cart"),
        ("GET", "/account/sessions"),
        ("GET", "/admin"),
        ("GET", "/admin/catalog"),
        ("GET", "/admin/summary"),
        ("GET", "/admin/orders"),
        ("GET", "/admin/payments"),
        ("GET", "/admin/shipments"),
        ("GET", "/admin/webhooks"),
        ("GET", "/admin/audit"),
        ("GET", "/products/:sku"),
        ("GET", "/members/:handle"),
        ("GET", "/orders/:customer"),
        ("POST", "/checkout"),
        ("POST", "/cart/items"),
        ("POST", "/members"),
        ("POST", "/members/login"),
        ("POST", "/payments"),
        ("POST", "/webhooks/stripe"),
        ("POST", "/shipments"),
        ("GET", "/shipments/:orderId"),
    ] {
        assert!(json_routes_include(&runtime["routes"], method, path));
        assert!(json_routes_include(
            &deploy["server"]["routes"],
            method,
            path
        ));
        assert!(native_routes_source_includes(&native_routes, method, path));
    }
    for feature in [
        "auth_roles",
        "csrf_protection",
        "payment_adapter",
        "rate_limit",
        "session_cookies",
        "shipping_adapter",
    ] {
        assert!(manifest["capabilities"]["runtime_features"]
            .as_array()
            .expect("manifest runtime features")
            .iter()
            .any(|item| item == feature));
        assert!(runtime["runtime_features"]
            .as_array()
            .expect("runtime features")
            .iter()
            .any(|item| item == feature));
        assert!(deploy["server"]["runtime_features"]
            .as_array()
            .expect("deploy runtime features")
            .iter()
            .any(|item| item == feature));
    }
    let admin_route = json_route(&runtime["routes"], "GET", "/admin").expect("admin route");
    assert!(admin_route["policies"]
        .as_array()
        .expect("admin policies")
        .iter()
        .any(|policy| policy["kind"] == "auth"
            && policy["role"] == "admin"
            && policy["required"] == true
            && policy["origin_id"]
                .as_str()
                .is_some_and(|origin_id| origin_id.starts_with("ori_"))));
    let account_sessions_route =
        json_route(&runtime["routes"], "GET", "/account/sessions").expect("sessions route");
    assert!(account_sessions_route["policies"]
        .as_array()
        .expect("session policies")
        .iter()
        .any(|policy| policy["kind"] == "session"
            && policy["required"] == true
            && policy["origin_id"]
                .as_str()
                .is_some_and(|origin_id| origin_id.starts_with("ori_"))));
    let checkout_route =
        json_route(&preflight["routes"], "POST", "/checkout").expect("checkout route");
    assert!(checkout_route["policies"]
        .as_array()
        .expect("checkout policies")
        .iter()
        .any(|policy| policy["kind"] == "csrf"
            && policy["required"] == true
            && policy["origin_id"]
                .as_str()
                .is_some_and(|origin_id| origin_id.starts_with("ori_"))));
    assert!(checkout_route["policies"]
        .as_array()
        .expect("checkout policies")
        .iter()
        .any(|policy| policy["kind"] == "rate_limit"
            && policy["limit"] == 10
            && policy["window_seconds"] == 60));
    assert_eq!(
        deploy["server"]["native_routes_source"],
        serde_json::json!("server/native/routes.rs")
    );
    assert_eq!(
        deploy["server"]["native_router_source"],
        serde_json::json!("server/native/router.rs")
    );
    assert_eq!(
        deploy["server"]["native_handlers_source"],
        serde_json::json!("server/native/handlers.rs")
    );
    assert!(native_routes.contains("pub fn orv_native_match_route("));
    assert!(native_routes.contains("pub struct OrvNativeRouteMatch"));
    assert!(native_routes.contains("pub struct OrvNativeParam"));
    assert!(native_routes.contains("pub struct OrvNativeRoutePolicy"));
    assert!(native_routes.contains("pub policies: &'static [OrvNativeRoutePolicy]"));
    assert!(native_routes.contains("kind: \"auth\""));
    assert!(native_routes.contains("role: Some(\"admin\")"));
    assert!(native_routes.contains("kind: \"csrf\""));
    assert!(native_routes.contains("kind: \"rate_limit\""));
    assert!(native_routes.contains("limit: Some(10)"));
    assert!(native_routes.contains("window_seconds: Some(60)"));
    assert!(native_routes.contains("orv_native_route_path_params(route.path, path)"));
    assert!(native_routes.contains("orv_native_match_route_segment(pattern_segment"));
    assert!(native_routes.contains("fn orv_native_route_param_segment(segment: &str)"));
    assert_eq!(
        deploy["server"]["persistence"]["db_paths"][0],
        serde_json::json!("data/shop.sqlite")
    );
    assert_eq!(
        deploy["server"]["persistence"]["db_env"],
        serde_json::json!([
            {
                "env": "SHOP_DATABASE_URL",
                "default": "sqlite://data/shop.sqlite"
            }
        ])
    );
    assert_eq!(
        deploy["server"]["persistence"]["record_paths"],
        serde_json::json!(["data/payments.jsonl", "data/shipments.jsonl"])
    );
    assert_eq!(
        deploy["server"]["commerce_adapters"],
        serde_json::json!("deploy/commerce-adapters.json")
    );
    assert_eq!(
        deploy["server"]["smoke_test"],
        serde_json::json!("deploy/smoke-test.sh")
    );
    assert_eq!(
        deploy["server"]["smoke_output"],
        serde_json::json!("deploy/smoke-output.txt")
    );
    assert_eq!(
        deploy["server"]["preflight"],
        serde_json::json!("deploy/preflight.json")
    );
    assert_eq!(
        deploy["server"]["benchmark_evidence"],
        serde_json::json!("deploy/benchmark-evidence.json")
    );
    assert_eq!(
        deploy["server"]["persistence"]["commerce_env"],
        serde_json::json!([
            {
                "env": "PAYMENT_ADAPTER_URL",
                "default": "file://data/payments.jsonl"
            },
            {
                "env": "SHIPPING_ADAPTER_URL",
                "default": "file://data/shipments.jsonl"
            }
        ])
    );
    assert_eq!(
        adapter_values_without_source_origin_ids(&commerce_adapters["adapters"]),
        serde_json::json!([
            {
                "kind": "payment",
                "mode": "file",
                "env": "PAYMENT_ADAPTER_URL",
                "default": "file://data/payments.jsonl",
                "endpoint": null,
                "record_path": "data/payments.jsonl",
                "request": {
                    "method": "POST",
                    "content_type": "application/json",
                    "kind": "payment.capture",
                    "body": {
                        "kind": "payment.capture",
                        "payload": "payment capture payload"
                    }
                }
            },
            {
                "kind": "payment",
                "mode": "provider",
                "env": null,
                "default": null,
                "endpoint": null,
                "record_path": null,
                "request": {
                    "method": "POST",
                    "content_type": "application/json",
                    "kind": "payment.capture",
                    "body": {
                        "kind": "payment.capture",
                        "payload": "payment capture payload"
                    }
                },
                "provider": "stripe",
                "provider_env": [
                    {
                        "env": "STRIPE_WEBHOOK_SECRET",
                        "required": false,
                        "purpose": "webhook_signature"
                    },
                    {
                        "env": "STRIPE_WEBHOOK_SECRET_PREVIOUS",
                        "required": false,
                        "purpose": "webhook_signature_previous"
                    }
                ]
            },
            {
                "kind": "shipping",
                "mode": "file",
                "env": "SHIPPING_ADAPTER_URL",
                "default": "file://data/shipments.jsonl",
                "endpoint": null,
                "record_path": "data/shipments.jsonl",
                "request": {
                    "method": "POST",
                    "content_type": "application/json",
                    "kind": "shipping.booking",
                    "body": {
                        "kind": "shipping.booking",
                        "payload": "shipping booking payload"
                    }
                }
            }
        ])
    );
    assert!(commerce_adapters["adapters"]
        .as_array()
        .expect("commerce adapters")
        .iter()
        .all(|adapter| adapter["source_origin_id"]
            .as_str()
            .is_some_and(|origin_id| origin_id.starts_with("ori_"))));
    assert_eq!(
        container["persistence"]["volumes"][0]["host"],
        serde_json::json!("data")
    );
    assert_eq!(
        container["persistence"]["volumes"][0]["container"],
        serde_json::json!("/app/data")
    );
    assert!(compose.contains("../data:/app/data"));
    assert!(
        compose.contains(r#"SHOP_DATABASE_URL: "${SHOP_DATABASE_URL:-sqlite://data/shop.sqlite}""#)
    );
    assert!(compose
        .contains(r#"PAYMENT_ADAPTER_URL: "${PAYMENT_ADAPTER_URL:-file://data/payments.jsonl}""#));
    assert!(compose.contains(
        r#"SHIPPING_ADAPTER_URL: "${SHIPPING_ADAPTER_URL:-file://data/shipments.jsonl}""#
    ));
    assert!(env_example.contains("PORT=8080"));
    assert!(env_example.contains("SHOP_DATABASE_URL=sqlite://data/shop.sqlite"));
    assert!(env_example.contains("PAYMENT_ADAPTER_URL=file://data/payments.jsonl"));
    assert!(env_example.contains("SHIPPING_ADAPTER_URL=file://data/shipments.jsonl"));
    assert!(env_example.contains("STRIPE_WEBHOOK_SECRET="));
    assert!(env_example.contains("STRIPE_WEBHOOK_SECRET_PREVIOUS="));
    assert_eq!(preflight["schema_version"], serde_json::json!(1));
    assert_eq!(preflight["kind"], serde_json::json!("orv.deploy.preflight"));
    assert_eq!(
        preflight["commands"]["verify_build"],
        serde_json::json!("orv verify-build .")
    );
    assert_eq!(
        preflight["commands"]["env_check"],
        serde_json::json!("orv deploy-env-check .")
    );
    assert_eq!(
        preflight["commands"]["smoke_test"],
        serde_json::json!("./deploy/smoke-test.sh")
    );
    assert_eq!(
        preflight["commands"]["editor_run_debug"],
        serde_json::json!("orv editor run-debug . --control next")
    );
    assert_eq!(
        preflight["commands"]["benchmark_report"],
        serde_json::json!("orv benchmark-report .")
    );
    assert_eq!(
        preflight["commands"]["benchmark_report_require_pass"],
        serde_json::json!("orv benchmark-report . --require-pass")
    );
    assert_eq!(
        preflight["commands"]["trace_run_build"],
        serde_json::json!("orv run-build . --trace deploy/request-trace.json")
    );
    assert_eq!(
        preflight["commands"]["trace_stream_smoke"],
        serde_json::json!("ORV_SMOKE_TRACE_STREAM=1 ./deploy/smoke-test.sh")
    );
    assert_eq!(
        preflight["artifacts"]["commerce_adapters"],
        serde_json::json!("deploy/commerce-adapters.json")
    );
    assert_eq!(
        preflight["artifacts"]["source_bundle"],
        serde_json::json!(SOURCE_BUNDLE_PATH)
    );
    assert_eq!(
        preflight["artifacts"]["project_graph"],
        serde_json::json!("project-graph.json")
    );
    assert_eq!(
        preflight["artifacts"]["origin_map"],
        serde_json::json!("origin-map.json")
    );
    assert_eq!(
        preflight["artifacts"]["build_manifest"],
        serde_json::json!("build-manifest.json")
    );
    assert_eq!(
        preflight["artifacts"]["bundle_plan"],
        serde_json::json!("bundle-plan.json")
    );
    assert_eq!(
        preflight["security_features"],
        serde_json::json!([
            "auth_roles",
            "csrf_protection",
            "rate_limit",
            "session_cookies"
        ])
    );
    assert_eq!(preflight["benchmark"]["kind"], "orv.benchmark.shop_5h");
    assert_eq!(preflight["benchmark"]["max_elapsed_minutes"], 300);
    assert_eq!(
        preflight["artifacts"]["benchmark_evidence"],
        serde_json::json!("deploy/benchmark-evidence.json")
    );
    assert_eq!(
        preflight["artifacts"]["smoke_output"],
        serde_json::json!("deploy/smoke-output.txt")
    );
    assert!(preflight["benchmark"]["success_criteria"]
        .as_array()
        .expect("benchmark success criteria")
        .iter()
        .any(|criterion| criterion
            .as_str()
            .is_some_and(|value| value.contains("complete checkout"))));
    assert!(preflight["benchmark"]["data_to_record"]
        .as_array()
        .expect("benchmark data")
        .iter()
        .any(|item| item == "smoke-test output"));
    assert_eq!(
        benchmark_evidence["kind"],
        serde_json::json!("orv.benchmark.shop_5h.evidence")
    );
    assert_eq!(benchmark_evidence["benchmark"], preflight["benchmark"]);
    assert_eq!(benchmark_evidence["commands"], preflight["commands"]);
    assert_eq!(benchmark_evidence["artifacts"], preflight["artifacts"]);
    assert_eq!(
        benchmark_evidence["task_entries"]
            .as_array()
            .expect("benchmark evidence task entries")
            .len(),
        10
    );
    assert_eq!(
        benchmark_evidence["data"]["smoke_test_output"],
        serde_json::Value::Null
    );
    assert_eq!(
        benchmark_evidence["data"]["smoke_test_required_markers"],
        serde_json::json!(deploy_benchmark::SMOKE_REQUIRED_MARKERS)
    );
    assert!(preflight["optional_env"]
        .as_array()
        .expect("optional preflight env")
        .iter()
        .any(|env| env["env"] == "SHOP_DATABASE_URL"
            && env["default"] == "sqlite://data/shop.sqlite"));
    assert!(preflight["optional_env"]
        .as_array()
        .expect("optional preflight env")
        .iter()
        .any(|env| env["env"] == "STRIPE_WEBHOOK_SECRET"
            && env["provider"] == "stripe"
            && env["required"] == false));
    assert!(smoke_test.contains(r#"BASE_URL="${ORV_BASE_URL:-http://127.0.0.1:8080}""#));
    assert!(smoke_test.contains(r#"ORV_BIN="${ORV_BIN:-orv}""#));
    assert!(smoke_test.contains("command -v curl"));
    assert!(smoke_test.contains("orv deploy smoke test requires curl"));
    assert!(smoke_test.contains("orv deploy smoke test requires orv"));
    assert!(smoke_test.contains("orv_smoke_reveal_contains()"));
    assert!(smoke_test.contains("orv_smoke_editor_reveal_contains()"));
    assert!(smoke_test.contains("orv_smoke_lsp_reveal_contains()"));
    assert!(smoke_test.contains("orv_smoke_dap_summary_contains()"));
    assert!(smoke_test.contains("lsp reveal"));
    assert!(smoke_test.contains("editor run-debug . --control next"));
    assert!(smoke_test.contains("orv_smoke_trace_stream()"));
    assert!(smoke_test.contains("ORV_SMOKE_TRACE_STREAM"));
    assert!(smoke_test.contains("editor trace-stream"));
    assert!(smoke_test.contains("orv deploy smoke test failed: live trace stream"));
    assert!(smoke_test.contains("orv_smoke_graph_contract()"));
    assert!(smoke_test.contains("\norv_smoke_graph_contract\n"));
    assert!(smoke_test.contains(r#""$ORV_BIN" verify-build ."#));
    assert!(smoke_test.contains("source-bundle.json"));
    assert!(smoke_test.contains(
        r#"orv_smoke_dap_summary_contains "dap source bundle panel" '"source_bundle": {'"#
    ));
    assert!(smoke_test.contains(
            r#"orv_smoke_dap_summary_contains "dap source bundle panel path" '"path": "./source-bundle.json"'"#
        ));
    assert!(smoke_test.contains(
        r#"orv_smoke_dap_summary_contains "dap source bundle panel file count" '"fileCount": 1'"#
    ));
    assert!(smoke_test
        .contains(r#"orv_smoke_dap_summary_contains "dap source bundle panel hash" '"hash":'"#));
    assert!(smoke_test.contains("project-graph.json"));
    assert!(smoke_test.contains("origin-map.json"));
    assert!(smoke_test.contains("orv_smoke_curl()"));
    assert!(smoke_test.contains("orv_smoke_origin_header()"));
    assert!(smoke_test.contains("orv_smoke_response_origin_header()"));
    assert!(smoke_test.contains("orv_smoke_curl_origin()"));
    assert!(smoke_test.contains("orv_smoke_curl_origin_response()"));
    assert!(smoke_test.contains("orv_smoke_fetch()"));
    assert!(smoke_test.contains("orv_smoke_fetch_origin()"));
    assert!(smoke_test.contains("orv_smoke_fetch_capture_origin()"));
    assert!(smoke_test.contains("orv_smoke_body_contains()"));
    assert!(smoke_test.contains("orv_smoke_cookie_from_headers()"));
    assert!(smoke_test.contains("orv deploy smoke test failed: %s"));
    assert!(smoke_test.contains(r#"READY_PATH="/health""#));
    assert!(smoke_test.contains("for attempt in 1 2 3 4 5"));
    assert!(smoke_test.contains("sleep 1"));
    assert!(smoke_test.contains(r#"ORV_SMOKE_ORIGIN_GET_HEALTH="ori_"#));
    assert!(smoke_test.contains(r#"ORV_SMOKE_RESPONSE_ORIGIN_GET_HEALTH="ori_"#));
    assert!(smoke_test.contains(
            r#"orv_smoke_curl_origin_response "GET /health" "$ORV_SMOKE_ORIGIN_GET_HEALTH" "$ORV_SMOKE_RESPONSE_ORIGIN_GET_HEALTH" "$BASE_URL/health""#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_fetch_capture_origin "GET / home" "$SMOKE_HOME_BODY" "$SMOKE_HEADERS" "$ORV_SMOKE_ORIGIN_GET_ROOT" "$BASE_URL/""#
        ));
    assert!(smoke_test
        .contains(r#"orv_smoke_body_contains "home title" "$SMOKE_HOME_BODY" 'Miol Shop'"#));
    assert!(smoke_test.contains(
            r#"orv_smoke_body_contains "home copy" "$SMOKE_HOME_BODY" 'Catalog, member signup, payment capture, and shipment booking are ready.'"#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_body_contains "home theme surface" "$SMOKE_HOME_BODY" 'background-color: #f8fafc'"#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_body_contains "home theme typography" "$SMOKE_HOME_BODY" 'font-family: Inter, system-ui, sans-serif'"#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_reveal_contains "reveal GET / source" "$ORV_SMOKE_ORIGIN_GET_ROOT" '@route GET /'"#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_reveal_contains "reveal GET / production" "$ORV_SMOKE_ORIGIN_GET_ROOT" '"path": "/"'"#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_reveal_contains "reveal GET / native target summary" "$ORV_SMOKE_ORIGIN_GET_ROOT" '"native_server_target_count": 1'"#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_editor_reveal_contains "editor reveal GET / native route summary" "$ORV_SMOKE_ORIGIN_GET_ROOT" '"native_server_route_count": 1'"#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_lsp_reveal_contains "lsp reveal GET / native target summary" "$ORV_SMOKE_ORIGIN_GET_ROOT" '"native_server_target_count": 1'"#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_reveal_contains "reveal GET /health response source" "$ORV_SMOKE_RESPONSE_ORIGIN_GET_HEALTH" '@respond'"#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_reveal_contains "reveal GET /health response production" "$ORV_SMOKE_RESPONSE_ORIGIN_GET_HEALTH" '"response_origin_dispatch": true'"#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_lsp_reveal_contains "lsp reveal GET /health response origin" "$ORV_SMOKE_RESPONSE_ORIGIN_GET_HEALTH" '"name": "respond"'"#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_lsp_reveal_contains "lsp reveal GET /health response production" "$ORV_SMOKE_RESPONSE_ORIGIN_GET_HEALTH" '"response_origin_dispatch": true'"#
        ));
    assert!(smoke_test.contains(r#"ORV_SMOKE_DB_CONNECT_ORIGIN="ori_"#));
    assert!(smoke_test.contains(
            r#"orv_smoke_reveal_contains "reveal DB source" "$ORV_SMOKE_DB_CONNECT_ORIGIN" '@db.connect'"#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_reveal_contains "reveal DB preflight" "$ORV_SMOKE_DB_CONNECT_ORIGIN" '"preflight"'"#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_reveal_contains "reveal DB smoke summary" "$ORV_SMOKE_DB_CONNECT_ORIGIN" '"smoke_test_summary"'"#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_reveal_contains "reveal DB smoke summary count" "$ORV_SMOKE_DB_CONNECT_ORIGIN" '"preflight_smoke_summary_missing_count"'"#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_reveal_contains "reveal DB sqlite path" "$ORV_SMOKE_DB_CONNECT_ORIGIN" 'sqlite://data/shop.sqlite'"#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_editor_reveal_contains "editor reveal DB smoke summary" "$ORV_SMOKE_DB_CONNECT_ORIGIN" '"smoke_test_summary"'"#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_editor_reveal_contains "editor reveal DB smoke summary count" "$ORV_SMOKE_DB_CONNECT_ORIGIN" '"preflight_smoke_summary_missing_count"'"#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_lsp_reveal_contains "lsp reveal DB origin" "$ORV_SMOKE_DB_CONNECT_ORIGIN" '@db.connect'"#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_lsp_reveal_contains "lsp reveal DB smoke summary" "$ORV_SMOKE_DB_CONNECT_ORIGIN" '"smoke_test_summary"'"#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_lsp_reveal_contains "lsp reveal DB smoke summary count" "$ORV_SMOKE_DB_CONNECT_ORIGIN" '"preflight_smoke_summary_missing_count"'"#
        ));
    assert!(smoke_test.contains(r#"ORV_SMOKE_PAYMENT_CONNECT_ORIGIN="ori_"#));
    assert!(smoke_test.contains(r#"ORV_SMOKE_SHIPPING_CONNECT_ORIGIN="ori_"#));
    assert!(smoke_test.contains(
            r#"orv_smoke_reveal_contains "reveal payment source" "$ORV_SMOKE_PAYMENT_CONNECT_ORIGIN" '@payment.connect'"#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_reveal_contains "reveal payment match" "$ORV_SMOKE_PAYMENT_CONNECT_ORIGIN" '"matched": true'"#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_reveal_contains "reveal payment record path" "$ORV_SMOKE_PAYMENT_CONNECT_ORIGIN" 'file://data/payments.jsonl'"#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_reveal_contains "reveal payment request kind" "$ORV_SMOKE_PAYMENT_CONNECT_ORIGIN" 'payment.capture'"#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_lsp_reveal_contains "lsp reveal payment match" "$ORV_SMOKE_PAYMENT_CONNECT_ORIGIN" '"matched": true'"#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_reveal_contains "reveal shipping source" "$ORV_SMOKE_SHIPPING_CONNECT_ORIGIN" '@shipping.connect'"#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_reveal_contains "reveal shipping match" "$ORV_SMOKE_SHIPPING_CONNECT_ORIGIN" '"matched": true'"#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_reveal_contains "reveal shipping record path" "$ORV_SMOKE_SHIPPING_CONNECT_ORIGIN" 'file://data/shipments.jsonl'"#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_reveal_contains "reveal shipping request kind" "$ORV_SMOKE_SHIPPING_CONNECT_ORIGIN" 'shipping.booking'"#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_lsp_reveal_contains "lsp reveal shipping match" "$ORV_SMOKE_SHIPPING_CONNECT_ORIGIN" '"matched": true'"#
        ));
    assert!(smoke_test
        .contains("CSRF_COOKIE=\"$(orv_smoke_cookie_from_headers orv_csrf \"$SMOKE_HEADERS\")\""));
    assert!(smoke_test.contains(r#"-H "x-csrf-token: ${CSRF_TOKEN}""#));
    assert!(smoke_test
            .contains(r#"orv_smoke_curl_origin "POST /products" "$ORV_SMOKE_ORIGIN_POST_PRODUCTS" -X POST "$BASE_URL/products""#));
    assert!(smoke_test.contains(r#"SMOKE_SKU="orv-smoke-sku-${SMOKE_ID}""#));
    assert!(smoke_test.contains(r#"SMOKE_SKU_SECOND="orv-smoke-sku-${SMOKE_ID}-2""#));
    assert!(smoke_test.contains(r#"SMOKE_SKU_THIRD="orv-smoke-sku-${SMOKE_ID}-3""#));
    assert!(smoke_test.contains(r#"SMOKE_BADGE="orv-smoke-badge-${SMOKE_ID}""#));
    assert!(smoke_test.contains(r#"SMOKE_BADGE_SECOND="orv-smoke-badge-${SMOKE_ID}-2""#));
    assert!(smoke_test.contains(r#"SMOKE_BADGE_THIRD="orv-smoke-badge-${SMOKE_ID}-3""#));
    assert!(smoke_test.contains(
        r#"orv_smoke_curl_origin "POST /products second" "$ORV_SMOKE_ORIGIN_POST_PRODUCTS""#
    ));
    assert!(smoke_test.contains(
        r#"orv_smoke_curl_origin "POST /products third" "$ORV_SMOKE_ORIGIN_POST_PRODUCTS""#
    ));
    assert!(smoke_test
            .contains(r#"orv_smoke_curl_origin "POST /members" "$ORV_SMOKE_ORIGIN_POST_MEMBERS" -X POST "$BASE_URL/members""#));
    assert!(smoke_test.contains(
            r#"orv_smoke_curl_capture_origin "POST /members/login smoke" "$SMOKE_MEMBER_HEADERS" "$ORV_SMOKE_ORIGIN_POST_MEMBERS_LOGIN""#
        ));
    assert!(smoke_test.contains("MEMBER_SESSION_COOKIE=\"$(orv_smoke_cookie_from_headers orv_session \"$SMOKE_MEMBER_HEADERS\")\""));
    assert!(smoke_test.contains(
            r#"orv_smoke_curl_origin "GET /account/sessions" "$ORV_SMOKE_ORIGIN_GET_ACCOUNT_SESSIONS" -H "cookie: ${MEMBER_SESSION_COOKIE}" "$BASE_URL/account/sessions""#
        ));
    assert!(smoke_test.contains(r#"SMOKE_HANDLE="orv-smoke-${SMOKE_ID}""#));
    assert!(smoke_test.contains(r#"SMOKE_PASSWORD="orv-smoke-password-${SMOKE_ID}""#));
    assert!(smoke_test.contains(r#"\"password\":\"${SMOKE_PASSWORD}\""#));
    assert!(smoke_test.contains(
            r#"orv_smoke_curl_origin "POST /cart/items" "$ORV_SMOKE_ORIGIN_POST_CART_ITEMS" -X POST "$BASE_URL/cart/items""#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_fetch_origin "POST /checkout" "$SMOKE_CHECKOUT_BODY" "$ORV_SMOKE_ORIGIN_POST_CHECKOUT" -X POST "$BASE_URL/checkout""#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_body_contains "checkout shipped order" "$SMOKE_CHECKOUT_BODY" '"status":"shipped"'"#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_body_contains "checkout captured payment" "$SMOKE_CHECKOUT_BODY" '"status":"captured"'"#
        ));
    assert!(smoke_test.contains(
        r#"orv_smoke_body_contains "checkout shipment tracking" "$SMOKE_CHECKOUT_BODY" 'TRK-LOCAL'"#
    ));
    assert!(smoke_test.contains(
            r#"orv_smoke_curl_capture_origin "POST /members/login admin" "$SMOKE_ADMIN_HEADERS" "$ORV_SMOKE_ORIGIN_POST_MEMBERS_LOGIN""#
        ));
    assert!(smoke_test.contains("ADMIN_SESSION_COOKIE=\"$(orv_smoke_cookie_from_headers orv_session \"$SMOKE_ADMIN_HEADERS\")\""));
    assert!(smoke_test.contains("ADMIN_ROLE_COOKIE=\"$(orv_smoke_cookie_from_headers orv_session_role \"$SMOKE_ADMIN_HEADERS\")\""));
    assert!(smoke_test.contains(
            r#"orv_smoke_curl_origin "GET /admin/summary" "$ORV_SMOKE_ORIGIN_GET_ADMIN_SUMMARY" -H "cookie: ${ADMIN_SESSION_COOKIE}; ${ADMIN_ROLE_COOKIE}" "$BASE_URL/admin/summary""#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_fetch_origin "GET /admin dashboard content" "$SMOKE_ADMIN_BODY" "$ORV_SMOKE_ORIGIN_GET_ADMIN" -H "cookie: ${ADMIN_SESSION_COOKIE}; ${ADMIN_ROLE_COOKIE}" "$BASE_URL/admin""#
        ));
    assert!(smoke_test.contains(
        r#"orv_smoke_body_contains "admin dashboard title" "$SMOKE_ADMIN_BODY" 'Miol Shop Admin'"#
    ));
    assert!(smoke_test.contains(
            r#"orv_smoke_body_contains "admin dashboard summary link" "$SMOKE_ADMIN_BODY" '/admin/summary'"#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_body_contains "admin dashboard webhook link" "$SMOKE_ADMIN_BODY" '/admin/webhooks'"#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_body_contains "admin dashboard sqlite storage" "$SMOKE_ADMIN_BODY" 'data/shop.sqlite'"#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_body_contains "admin summary webhook events" "$SMOKE_ADMIN_SUMMARY_BODY" '"webhookEvents"'"#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_body_contains "admin summary audit events" "$SMOKE_ADMIN_SUMMARY_BODY" '"auditEvents"'"#
        ));
    assert!(smoke_test.contains(
        r#"orv_smoke_body_contains "catalog smoke product" "$SMOKE_CATALOG_BODY" "$SMOKE_SKU""#
    ));
    assert!(smoke_test.contains(
            r#"orv_smoke_body_contains "catalog second smoke product" "$SMOKE_CATALOG_BODY" "$SMOKE_SKU_SECOND""#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_body_contains "catalog third smoke product" "$SMOKE_CATALOG_BODY" "$SMOKE_SKU_THIRD""#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_body_contains "catalog smoke product field" "$SMOKE_CATALOG_BODY" "$SMOKE_BADGE""#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_body_contains "catalog second smoke product field" "$SMOKE_CATALOG_BODY" "$SMOKE_BADGE_SECOND""#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_body_contains "catalog third smoke product field" "$SMOKE_CATALOG_BODY" "$SMOKE_BADGE_THIRD""#
        ));
    assert!(smoke_test
        .contains(r#"orv_smoke_body_contains "cart smoke item" "$SMOKE_CART_BODY" "$SMOKE_SKU""#));
    assert!(smoke_test.contains(
        r#"orv_smoke_body_contains "account smoke session" "$SMOKE_ACCOUNT_BODY" "$SMOKE_HANDLE""#
    ));
    assert!(smoke_test.contains(
            r#"orv_smoke_body_contains "admin catalog smoke product" "$SMOKE_ADMIN_CATALOG_BODY" "$SMOKE_SKU""#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_body_contains "admin catalog second smoke product" "$SMOKE_ADMIN_CATALOG_BODY" "$SMOKE_SKU_SECOND""#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_body_contains "admin catalog third smoke product" "$SMOKE_ADMIN_CATALOG_BODY" "$SMOKE_SKU_THIRD""#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_body_contains "admin catalog smoke product field" "$SMOKE_ADMIN_CATALOG_BODY" "$SMOKE_BADGE""#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_body_contains "admin catalog second smoke product field" "$SMOKE_ADMIN_CATALOG_BODY" "$SMOKE_BADGE_SECOND""#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_body_contains "admin catalog third smoke product field" "$SMOKE_ADMIN_CATALOG_BODY" "$SMOKE_BADGE_THIRD""#
        ));
    assert!(smoke_test.contains(
        r#"orv_smoke_body_contains "admin orders shipped" "$SMOKE_ADMIN_ORDERS_BODY" 'shipped'"#
    ));
    assert!(smoke_test.contains(
            r#"orv_smoke_body_contains "admin payments captured" "$SMOKE_ADMIN_PAYMENTS_BODY" 'captured'"#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_body_contains "admin shipments tracking" "$SMOKE_ADMIN_SHIPMENTS_BODY" 'TRK-LOCAL'"#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_fetch_origin "GET /admin/webhooks content" "$SMOKE_ADMIN_WEBHOOKS_BODY" "$ORV_SMOKE_ORIGIN_GET_ADMIN_WEBHOOKS" -H "cookie: ${ADMIN_SESSION_COOKIE}; ${ADMIN_ROLE_COOKIE}" "$BASE_URL/admin/webhooks""#
        ));
    assert!(smoke_test.contains(
        r#"orv_smoke_body_contains "admin webhooks title" "$SMOKE_ADMIN_WEBHOOKS_BODY" 'Webhooks'"#
    ));
    assert!(smoke_test.contains(
            r#"orv_smoke_body_contains "admin audit checkout" "$SMOKE_ADMIN_AUDIT_BODY" 'checkout.complete'"#
        ));
    let runbook =
        std::fs::read_to_string(out.join("deploy").join("README.md")).expect("deploy runbook");
    assert!(runbook.contains("deploy/env.example"));
    assert!(runbook.contains("deploy/commerce-adapters.json"));
    assert!(runbook.contains("deploy/smoke-test.sh"));
    assert!(runbook.contains("deploy/smoke-output.txt"));
    assert!(runbook.contains("deploy/preflight.json"));
    assert!(runbook.contains("deploy/benchmark-evidence.json"));
    assert!(runbook.contains("## Benchmark Evidence"));
    assert!(runbook.contains("./deploy/smoke-test.sh"));
    assert!(runbook.contains("ORV_SMOKE_TRACE_STREAM=1 ./deploy/smoke-test.sh"));
    assert!(runbook.contains("orv verify-build ."));
    assert!(runbook.contains("orv editor run-debug . --control next"));
    assert!(runbook.contains("orv benchmark-report ."));
    assert!(runbook.contains("orv benchmark-report . --require-pass"));
    assert!(
        runbook.contains("- DB adapter env: SHOP_DATABASE_URL default sqlite://data/shop.sqlite")
    );
    assert!(runbook.contains("- Record log: data/payments.jsonl"));
    assert!(runbook.contains("- Record log: data/shipments.jsonl"));
    assert!(runbook.contains(
        "- Commerce adapter env: PAYMENT_ADAPTER_URL default file://data/payments.jsonl"
    ));
    assert!(runbook.contains(
        "- Commerce adapter env: SHIPPING_ADAPTER_URL default file://data/shipments.jsonl"
    ));
    assert!(runbook.contains(
        "- Commerce provider env: payment stripe STRIPE_WEBHOOK_SECRET optional webhook_signature"
    ));
    cmd_verify_build(&out).expect("verify shop prod build");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_snapshot_subcommand_is_accepted() {
    let parsed = Cli::try_parse_from(["orv", "lsp", "snapshot", "fixtures/e2e/hello.orv"]);
    if let Err(err) = parsed {
        panic!("{}", err.render());
    }
}

#[test]
fn lsp_reveal_subcommand_is_accepted() {
    let parsed = Cli::try_parse_from([
        "orv",
        "lsp",
        "reveal",
        "target/orv-build-test",
        "route:GET_/ping:abc123",
    ]);
    if let Err(err) = parsed {
        panic!("{}", err.render());
    }
}

#[test]
fn lsp_serve_stdio_subcommand_is_accepted() {
    let parsed = Cli::try_parse_from(["orv", "lsp", "serve", "--stdio"]);
    if let Err(err) = parsed {
        panic!("{}", err.render());
    }
}

#[test]
fn dap_serve_stdio_subcommand_is_accepted() {
    let parsed = Cli::try_parse_from(["orv", "dap", "serve", "--stdio"]);
    if let Err(err) = parsed {
        panic!("{}", err.render());
    }
}

#[test]
fn build_subcommand_is_accepted() {
    let parsed = Cli::try_parse_from([
        "orv",
        "build",
        "fixtures/e2e/hello.orv",
        "--out",
        "target/orv-build-test",
    ]);
    if let Err(err) = parsed {
        panic!("{}", err.render());
    }
}

#[test]
fn db_plan_subcommand_is_accepted() {
    let parsed = Cli::try_parse_from(["orv", "db", "plan", "fixtures/e2e/hello.orv"]);
    if let Err(err) = parsed {
        panic!("{}", err.render());
    }
}

#[test]
fn db_apply_subcommand_is_accepted() {
    let parsed = Cli::try_parse_from([
        "orv",
        "db",
        "apply",
        "fixtures/e2e/hello.orv",
        "--schema",
        "target/orv-db-schema.json",
        "--history",
        "target/orv-db-history.json",
    ]);
    if let Err(err) = parsed {
        panic!("{}", err.render());
    }
}

#[test]
fn db_migrate_subcommand_is_accepted() {
    let parsed = Cli::try_parse_from([
        "orv",
        "db",
        "migrate",
        "fixtures/e2e/hello.orv",
        "--schema",
        "target/orv-db-schema.json",
        "--history",
        "target/orv-db-history.json",
    ]);
    if let Err(err) = parsed {
        panic!("{}", err.render());
    }
}

#[test]
fn db_rollback_subcommand_is_accepted() {
    let parsed = Cli::try_parse_from([
        "orv",
        "db",
        "rollback",
        "--schema",
        "target/orv-db-schema.json",
    ]);
    if let Err(err) = parsed {
        panic!("{}", err.render());
    }
}

#[test]
fn db_plan_reports_added_nullable_field_from_applied_snapshot() {
    let dir = temp_output_dir("db-plan");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        r#"struct User {
  id: int
  email: string
  avatar: string?
}"#,
    )
    .expect("write source");
    let applied = dir.join("applied-schema.json");
    std::fs::write(
        &applied,
        r#"{
  "schema_version": 1,
  "structs": {
    "User": {
      "fields": {
        "id": { "type": "int", "optional": false },
        "email": { "type": "string", "optional": false }
      }
    }
  }
}"#,
    )
    .expect("write applied schema");

    let plan = db_plan_json(&source, Some(&applied)).expect("db plan");

    let actions = plan["actions"].as_array().expect("actions array");
    assert!(actions.iter().any(|action| {
        action["kind"] == "add_field"
            && action["struct"] == "User"
            && action["field"] == "avatar"
            && action["type"] == "string?"
            && action["optional"] == true
    }));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_snapshot_includes_diagnostics_graph_and_document_symbols() {
    let dir = temp_output_dir("lsp-snapshot");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        r#"struct User {
  id: int
}

function greet(user: User): string -> "hello"
"#,
    )
    .expect("write source");

    let snapshot = lsp_snapshot_json(&source).expect("lsp snapshot");

    assert_eq!(snapshot["schema_version"], 1);
    assert_eq!(
        snapshot["diagnostics"]
            .as_array()
            .expect("diagnostics")
            .len(),
        0
    );
    assert!(snapshot["project_graph"]["nodes"]
        .as_array()
        .expect("nodes")
        .iter()
        .any(|node| node["kind"] == "struct" && node["name"] == "User"));
    let symbols = snapshot["document_symbols"]
        .as_array()
        .expect("document symbols");
    let user = symbols
        .iter()
        .find(|symbol| symbol["name"] == "User")
        .expect("User symbol");
    assert_eq!(user["kind"], "Struct");
    assert_eq!(user["range"]["start"]["line"], 0);
    assert!(symbols
        .iter()
        .any(|symbol| symbol["name"] == "greet" && symbol["kind"] == "Function"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_initialize_returns_server_capabilities() {
    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 7,
        "method": "initialize",
        "params": {},
    }));

    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 7);
    assert_eq!(response["result"]["serverInfo"]["name"], "orv-lsp");
    let capabilities = &response["result"]["capabilities"];
    assert_eq!(capabilities["textDocumentSync"]["openClose"], true);
    assert_eq!(capabilities["textDocumentSync"]["change"], 1);
    assert_eq!(
        capabilities["textDocumentSync"]["save"]["includeText"],
        true
    );
    for provider in [
        "documentSymbolProvider",
        "foldingRangeProvider",
        "selectionRangeProvider",
        "definitionProvider",
        "declarationProvider",
        "typeDefinitionProvider",
        "implementationProvider",
        "typeHierarchyProvider",
        "callHierarchyProvider",
        "monikerProvider",
        "colorProvider",
        "linkedEditingRangeProvider",
        "referencesProvider",
        "documentHighlightProvider",
        "workspaceSymbolProvider",
        "hoverProvider",
        "inlayHintProvider",
        "documentFormattingProvider",
        "documentRangeFormattingProvider",
    ] {
        assert_eq!(capabilities[provider], true, "{provider}");
    }
    assert_eq!(
        capabilities["documentOnTypeFormattingProvider"]["firstTriggerCharacter"],
        "}"
    );
    assert!(
        capabilities["documentOnTypeFormattingProvider"]["moreTriggerCharacter"]
            .as_array()
            .expect("on type trigger characters")
            .iter()
            .any(|trigger| trigger == "\n")
    );
    assert_eq!(
        capabilities["documentLinkProvider"]["resolveProvider"],
        false
    );
    assert_eq!(capabilities["semanticTokensProvider"]["full"], true);
    assert_eq!(
        capabilities["semanticTokensProvider"]["legend"]["tokenTypes"][1],
        "type"
    );
    assert_eq!(capabilities["codeLensProvider"]["resolveProvider"], false);
    assert_eq!(
        capabilities["codeActionProvider"]["codeActionKinds"][0],
        "quickfix"
    );
    assert_eq!(
        capabilities["executeCommandProvider"]["commands"][0],
        "orv.revealSourceNode"
    );
    assert_eq!(capabilities["renameProvider"]["prepareProvider"], true);
    assert_eq!(
        capabilities["completionProvider"]["triggerCharacters"][0],
        "@"
    );
    assert_eq!(
        capabilities["signatureHelpProvider"]["triggerCharacters"][0],
        "("
    );
    assert_eq!(
        capabilities["diagnosticProvider"]["workspaceDiagnostics"],
        true
    );
}

#[test]
fn lsp_shutdown_returns_null_result() {
    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 8,
        "method": "shutdown",
    }));

    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 8);
    assert!(response.get("error").is_none());
    assert!(response
        .get("result")
        .is_some_and(serde_json::Value::is_null));
}

#[test]
fn lsp_unknown_method_returns_method_not_found_with_method_name() {
    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": "request-9",
        "method": "workspace/configuration",
    }));

    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], "request-9");
    assert_eq!(response["error"]["code"], -32601);
    assert_eq!(
        response["error"]["data"]["method"],
        "workspace/configuration"
    );
}

#[test]
fn lsp_stdio_serves_content_length_initialize_frame() {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 10,
        "method": "initialize",
        "params": {},
    })
    .to_string();
    let input = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);

    let output = lsp_stdio_response(&input).expect("stdio response");
    let (_, response_body) = output
        .split_once("\r\n\r\n")
        .expect("content-length response frame");
    let response: serde_json::Value = serde_json::from_str(response_body).expect("response json");

    assert!(output.starts_with("Content-Length: "));
    assert_eq!(response["id"], 10);
    assert_eq!(response["result"]["serverInfo"]["name"], "orv-lsp");
}

#[test]
fn lsp_stdio_ignores_notifications_without_id() {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "initialized",
        "params": {},
    })
    .to_string();
    let input = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);

    let output = lsp_stdio_response(&input).expect("stdio response");

    assert_eq!(output, "");
}

#[test]
fn dap_initialize_returns_debug_capabilities() {
    let response = dap_protocol_response(&serde_json::json!({
        "seq": 1,
        "type": "request",
        "command": "initialize",
        "arguments": {},
    }));

    assert_eq!(response["type"], "response");
    assert_eq!(response["request_seq"], 1);
    assert_eq!(response["command"], "initialize");
    assert_eq!(response["success"], true);
    assert_eq!(response["body"]["supportsConfigurationDoneRequest"], true);
    assert_eq!(response["body"]["supportsTerminateRequest"], true);
    assert_eq!(response["body"]["supportsTerminateThreadsRequest"], true);
    assert_eq!(response["body"]["supportsLoadedSourcesRequest"], true);
    assert_eq!(response["body"]["supportsEvaluateForHovers"], true);
    assert_eq!(response["body"]["supportsCompletionsRequest"], true);
    assert_eq!(response["body"]["supportsBreakpointLocationsRequest"], true);
    assert_eq!(response["body"]["supportsConditionalBreakpoints"], true);
    assert_eq!(response["body"]["supportsHitConditionalBreakpoints"], true);
    assert_eq!(response["body"]["supportsFunctionBreakpoints"], true);
    assert_eq!(response["body"]["supportsDataBreakpoints"], true);
    assert_eq!(response["body"]["supportsExceptionInfoRequest"], true);
    assert_eq!(response["body"]["supportsRestartRequest"], true);
    assert_eq!(response["body"]["supportsSetVariable"], true);
    assert_eq!(response["body"]["supportsSetExpression"], true);
    assert_eq!(response["body"]["supportsModulesRequest"], true);
    assert_eq!(response["body"]["supportsGotoTargetsRequest"], true);
    assert_eq!(response["body"]["supportsStepBack"], true);
    assert_eq!(response["body"]["supportsStepInTargetsRequest"], true);
    assert_eq!(response["body"]["supportsRestartFrame"], true);
    assert_eq!(response["body"]["supportsPauseRequest"], true);
    assert_eq!(response["body"]["supportsCancelRequest"], true);
    assert_eq!(response["body"]["supportsInstructionBreakpoints"], true);
    assert_eq!(response["body"]["supportsDisassembleRequest"], true);
    assert_eq!(response["body"]["supportsReadMemoryRequest"], true);
    assert_eq!(response["body"]["supportsOrvRuntimeAttach"], true);
    assert_eq!(response["body"]["supportsOrvRuntimeTracePath"], true);
    assert_eq!(response["body"]["supportsOrvSourceBundleLaunch"], true);
}

#[test]
fn dap_cancel_request_is_accepted() {
    let response = dap_protocol_response(&serde_json::json!({
        "seq": 66,
        "type": "request",
        "command": "cancel",
        "arguments": {
            "requestId": 1,
            "progressId": "orv-progress",
        },
    }));

    assert_eq!(response["type"], "response");
    assert_eq!(response["request_seq"], 66);
    assert_eq!(response["command"], "cancel");
    assert_eq!(response["success"], true);
}

#[test]
fn dap_set_instruction_breakpoints_requires_launch_for_verification() {
    let response = dap_protocol_response(&serde_json::json!({
        "seq": 77,
        "type": "request",
        "command": "setInstructionBreakpoints",
        "arguments": {
            "breakpoints": [
                {
                    "instructionReference": "orv:entry:0",
                    "offset": 4,
                }
            ],
        },
    }));

    assert_eq!(response["type"], "response");
    assert_eq!(response["request_seq"], 77);
    assert_eq!(response["command"], "setInstructionBreakpoints");
    assert_eq!(response["success"], true);
    let breakpoint = &response["body"]["breakpoints"][0];
    assert_eq!(breakpoint["verified"], false);
    assert_eq!(breakpoint["instructionReference"], "orv:entry:0");
    assert_eq!(breakpoint["offset"], 4);
    assert_eq!(
        breakpoint["message"],
        "launch is required before verifying ORV instruction breakpoints"
    );
}

#[test]
fn dap_instruction_breakpoint_stops_continue_at_frame() {
    let dir = temp_output_dir("dap-instruction-breakpoint");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        "let first: int = 1\nlet second: int = 2\nlet third: int = 3\n",
    )
    .expect("write source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 82,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    let set_instruction_breakpoints = session
        .message_response(&serde_json::json!({
            "seq": 83,
            "type": "request",
            "command": "setInstructionBreakpoints",
            "arguments": {
                "breakpoints": [
                    {
                        "instructionReference": "orv:frame:2",
                        "offset": 0,
                    }
                ],
            },
        }))
        .expect("setInstructionBreakpoints response");
    let continue_response = session
        .message_response(&serde_json::json!({
            "seq": 84,
            "type": "request",
            "command": "continue",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("continue response");
    let events = session.drain_pending_events();
    let stack = session
        .message_response(&serde_json::json!({
            "seq": 85,
            "type": "request",
            "command": "stackTrace",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("stack response");

    assert_eq!(
        set_instruction_breakpoints["body"]["breakpoints"][0]["verified"],
        true
    );
    assert_eq!(
        set_instruction_breakpoints["body"]["breakpoints"][0]["instructionReference"],
        "orv:frame:2"
    );
    assert_eq!(continue_response["success"], true, "{continue_response}");
    assert_eq!(stack["body"]["stackFrames"][0]["line"], 2);
    assert!(events.iter().any(|event| {
        event["type"] == "event"
            && event["event"] == "stopped"
            && event["body"]["reason"] == "instruction breakpoint"
    }));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_disassemble_returns_source_frame_pseudo_instructions() {
    let dir = temp_output_dir("dap-disassemble");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    let source_text = "let first: int = 1\nlet second: int = 2\n";
    std::fs::write(&source, source_text).expect("write source");
    let canonical_source = std::fs::canonicalize(&source).expect("canonical source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 78,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    let response = session
        .message_response(&serde_json::json!({
            "seq": 79,
            "type": "request",
            "command": "disassemble",
            "arguments": {
                "memoryReference": "orv:frame:1",
                "instructionOffset": 0,
                "instructionCount": 2,
            },
        }))
        .expect("disassemble response");

    assert_eq!(response["type"], "response");
    assert_eq!(response["request_seq"], 79);
    assert_eq!(response["command"], "disassemble");
    assert_eq!(response["success"], true, "{response}");
    let instructions = response["body"]["instructions"]
        .as_array()
        .expect("instructions");
    assert_eq!(instructions.len(), 2);
    assert_eq!(instructions[0]["address"], "orv:frame:1");
    assert_eq!(instructions[0]["instruction"], "orv entry line 1");
    assert_eq!(
        instructions[0]["location"]["path"],
        canonical_source.display().to_string()
    );
    assert_eq!(
        instructions[0]["location"]["checksums"][0]["algorithm"],
        serde_json::json!("SHA256")
    );
    assert_eq!(
        instructions[0]["location"]["checksums"][0]["checksum"],
        serde_json::json!(sha256_hex(source_text.as_bytes()))
    );
    assert_eq!(instructions[0]["line"], 1);
    assert_eq!(instructions[1]["address"], "orv:frame:2");
    assert_eq!(instructions[1]["instruction"], "orv entry line 2");
    assert_eq!(instructions[1]["line"], 2);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_read_memory_returns_base64_source_frame_bytes() {
    let dir = temp_output_dir("dap-read-memory");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "let first: int = 1\nlet second: int = 2\n").expect("write source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 80,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    let response = session
        .message_response(&serde_json::json!({
            "seq": 81,
            "type": "request",
            "command": "readMemory",
            "arguments": {
                "memoryReference": "orv:frame:1",
                "offset": 4,
                "count": 5,
            },
        }))
        .expect("readMemory response");

    assert_eq!(response["type"], "response");
    assert_eq!(response["request_seq"], 81);
    assert_eq!(response["command"], "readMemory");
    assert_eq!(response["success"], true, "{response}");
    assert_eq!(response["body"]["address"], "orv:frame:1");
    assert_eq!(response["body"]["data"], "Zmlyc3Q=");
    assert_eq!(response["body"]["unreadableBytes"], 0);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_set_exception_breakpoints_accepts_orv_filters() {
    let mut session = DapSession::default();

    let response = session
        .message_response(&serde_json::json!({
            "seq": 67,
            "type": "request",
            "command": "setExceptionBreakpoints",
            "arguments": {
                "filters": ["orv.diagnostics", "orv.runtime"],
            },
        }))
        .expect("setExceptionBreakpoints response");

    assert_eq!(response["success"], true, "{response}");
    assert_eq!(response["command"], "setExceptionBreakpoints");
    assert_eq!(
        response["body"]["breakpoints"]
            .as_array()
            .expect("breakpoints")
            .len(),
        2
    );
    assert_eq!(response["body"]["breakpoints"][0]["verified"], true);
    assert_eq!(
        response["body"]["breakpoints"][0]["filter"],
        "orv.diagnostics"
    );
    assert_eq!(response["body"]["breakpoints"][1]["verified"], true);
    assert_eq!(response["body"]["breakpoints"][1]["filter"], "orv.runtime");
}

#[test]
fn dap_set_exception_breakpoints_empty_filters_disable_diagnostic_stop_reason() {
    let dir = temp_output_dir("dap-exception-filters-empty");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "let bad: int = \"wrong\"\n").expect("write source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 68,
            "type": "request",
            "command": "setExceptionBreakpoints",
            "arguments": {
                "filters": [],
            },
        }))
        .expect("setExceptionBreakpoints response");
    session
        .message_response(&serde_json::json!({
            "seq": 69,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    session
        .message_response(&serde_json::json!({
            "seq": 70,
            "type": "request",
            "command": "configurationDone",
            "arguments": {},
        }))
        .expect("configurationDone response");
    let events = session.drain_pending_events();

    assert!(events
        .iter()
        .any(|event| { event["event"] == "stopped" && event["body"]["reason"] == "entry" }));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_set_breakpoints_accepts_loaded_source_reference() {
    let dir = temp_output_dir("dap-set-breakpoints-source-ref");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "let answer: int = 42\n").expect("write source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 7,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    let response = session
        .message_response(&serde_json::json!({
            "seq": 8,
            "type": "request",
            "command": "setBreakpoints",
            "arguments": {
                "source": {
                    "sourceReference": 1,
                },
                "breakpoints": [
                    {
                        "line": 1,
                    },
                ],
            },
        }))
        .expect("setBreakpoints response");

    assert_eq!(response["success"], true, "{response}");
    assert_eq!(response["body"]["breakpoints"][0]["verified"], true);
    assert_eq!(response["body"]["breakpoints"][0]["line"], 1);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_stdio_serves_content_length_initialize_frame() {
    let body = serde_json::json!({
        "seq": 1,
        "type": "request",
        "command": "initialize",
        "arguments": {},
    })
    .to_string();
    let input = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);

    let output = dap_stdio_response(&input).expect("stdio response");
    let frames = protocol_frames(&output);
    let response = &frames[0];

    assert!(output.starts_with("Content-Length: "));
    assert_eq!(response["type"], "response");
    assert_eq!(response["command"], "initialize");
    assert_eq!(response["success"], true);
}

#[test]
fn dap_stdio_emits_initialized_event_after_initialize() {
    let body = serde_json::json!({
        "seq": 1,
        "type": "request",
        "command": "initialize",
        "arguments": {},
    })
    .to_string();
    let input = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);

    let output = dap_stdio_response(&input).expect("stdio response");
    let frames = protocol_frames(&output);

    assert_eq!(frames.len(), 2, "{output}");
    assert_eq!(frames[0]["type"], "response");
    assert_eq!(frames[0]["command"], "initialize");
    assert_eq!(frames[1]["type"], "event");
    assert_eq!(frames[1]["event"], "initialized");
}

#[test]
fn dap_stdio_emits_stopped_event_after_configuration_done() {
    let dir = temp_output_dir("dap-stopped-event");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "let answer: int = 42\n").expect("write source");
    let input = [
        protocol_request_frame(&serde_json::json!({
            "seq": 1,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        })),
        protocol_request_frame(&serde_json::json!({
            "seq": 2,
            "type": "request",
            "command": "configurationDone",
            "arguments": {},
        })),
    ]
    .join("");

    let output = dap_stdio_response(&input).expect("stdio response");
    let frames = protocol_frames(&output);
    let stopped = frames
        .iter()
        .find(|frame| frame["type"] == "event" && frame["event"] == "stopped")
        .expect("stopped event");

    assert_eq!(stopped["body"]["reason"], "entry");
    assert_eq!(stopped["body"]["threadId"], 1);
    assert_eq!(stopped["body"]["allThreadsStopped"], false);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_stdio_emits_continued_and_terminated_events_after_continue() {
    let dir = temp_output_dir("dap-continue-events");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "let answer: int = 42\n").expect("write source");
    let input = [
        protocol_request_frame(&serde_json::json!({
            "seq": 1,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        })),
        protocol_request_frame(&serde_json::json!({
            "seq": 2,
            "type": "request",
            "command": "continue",
            "arguments": {
                "threadId": 1,
            },
        })),
    ]
    .join("");

    let output = dap_stdio_response(&input).expect("stdio response");
    let frames = protocol_frames(&output);
    let continued = frames
        .iter()
        .find(|frame| frame["type"] == "event" && frame["event"] == "continued")
        .expect("continued event");
    let terminated = frames
        .iter()
        .find(|frame| frame["type"] == "event" && frame["event"] == "terminated")
        .expect("terminated event");

    assert_eq!(continued["body"]["threadId"], 1);
    assert_eq!(continued["body"]["allThreadsContinued"], false);
    assert_eq!(terminated["body"], serde_json::json!({}));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_continue_terminates_session_state() {
    let dir = temp_output_dir("dap-continue-terminates-state");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "let answer: int = 42\n").expect("write source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 71,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    let continue_response = session
        .message_response(&serde_json::json!({
            "seq": 72,
            "type": "request",
            "command": "continue",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("continue response");
    let stack = session
        .message_response(&serde_json::json!({
            "seq": 73,
            "type": "request",
            "command": "stackTrace",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("stack response");

    assert_eq!(continue_response["success"], true, "{continue_response}");
    assert_eq!(stack["success"], false, "{stack}");
    assert!(stack["message"]
        .as_str()
        .is_some_and(|message| message.contains("launch is required")));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_debug_control_rejects_unknown_thread_id() {
    let dir = temp_output_dir("dap-debug-control-thread");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "let first: int = 1\nlet second: int = 2\n").expect("write source");

    for command in ["continue", "next", "stepIn", "pause"] {
        let mut session = DapSession::default();
        session
            .message_response(&serde_json::json!({
                "seq": 57,
                "type": "request",
                "command": "launch",
                "arguments": {
                    "program": format!("file://{}", source.display()),
                },
            }))
            .expect("launch response");
        let response = session
            .message_response(&serde_json::json!({
                "seq": 58,
                "type": "request",
                "command": command,
                "arguments": {
                    "threadId": 99,
                },
            }))
            .expect("debug control response");

        assert_eq!(response["success"], false, "{command}: {response}");
        assert!(response["message"]
            .as_str()
            .is_some_and(|message| { message.contains("unknown ORV thread id 99") }));
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_continue_stops_at_next_verified_breakpoint_frame() {
    let dir = temp_output_dir("dap-continue-breakpoint-frame");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        "let first: int = 1\nlet middle: int = 2\nlet last: int = 3\n",
    )
    .expect("write source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 158,
            "type": "request",
            "command": "setBreakpoints",
            "arguments": {
                "source": {
                    "path": source.display().to_string(),
                },
                "breakpoints": [
                    { "line": 1 },
                    { "line": 3 },
                ],
            },
        }))
        .expect("breakpoints response");
    session
        .message_response(&serde_json::json!({
            "seq": 159,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    let first_stack = session
        .message_response(&serde_json::json!({
            "seq": 160,
            "type": "request",
            "command": "stackTrace",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("first stack response");
    session
        .message_response(&serde_json::json!({
            "seq": 161,
            "type": "request",
            "command": "continue",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("continue response");
    let events = session.drain_pending_events();
    let second_stack = session
        .message_response(&serde_json::json!({
            "seq": 162,
            "type": "request",
            "command": "stackTrace",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("second stack response");

    assert_eq!(first_stack["body"]["stackFrames"][0]["line"], 1);
    assert_eq!(second_stack["body"]["stackFrames"][0]["line"], 3);
    assert!(events.iter().any(|event| {
        event["type"] == "event" && event["event"] == "continued" && event["body"]["threadId"] == 1
    }));
    assert!(events.iter().any(|event| {
        event["type"] == "event"
            && event["event"] == "stopped"
            && event["body"]["reason"] == "breakpoint"
            && event["body"]["threadId"] == 1
    }));
    assert!(!events
        .iter()
        .any(|event| event["type"] == "event" && event["event"] == "terminated"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_logpoint_outputs_without_stopping() {
    let dir = temp_output_dir("dap-logpoint");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        "let first: int = 1\nlet second: int = 2\nlet third: int = 3\n",
    )
    .expect("write source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 164,
            "type": "request",
            "command": "setBreakpoints",
            "arguments": {
                "source": {
                    "path": source.display().to_string(),
                },
                "breakpoints": [
                    {
                        "line": 2,
                        "logMessage": "middle reached",
                    },
                ],
            },
        }))
        .expect("breakpoints response");
    session
        .message_response(&serde_json::json!({
            "seq": 165,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    let first_stack = session
        .message_response(&serde_json::json!({
            "seq": 166,
            "type": "request",
            "command": "stackTrace",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("first stack response");
    session.drain_pending_events();
    let continue_response = session
        .message_response(&serde_json::json!({
            "seq": 167,
            "type": "request",
            "command": "continue",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("continue response");
    let events = session.drain_pending_events();

    assert_eq!(first_stack["body"]["stackFrames"][0]["line"], 1);
    assert_eq!(continue_response["success"], true, "{continue_response}");
    assert!(events.iter().any(|event| {
        event["type"] == "event"
            && event["event"] == "output"
            && event["body"]["category"] == "console"
            && event["body"]["output"] == "middle reached\n"
    }));
    assert!(!events.iter().any(|event| {
        event["type"] == "event"
            && event["event"] == "stopped"
            && event["body"]["reason"] == "breakpoint"
    }));
    assert!(events
        .iter()
        .any(|event| event["type"] == "event" && event["event"] == "terminated"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_conditional_breakpoint_skips_false_condition_frame() {
    let dir = temp_output_dir("dap-conditional-breakpoint");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        "let mut total: int = 1\ntotal = total + 4\ntotal = total + 4\n",
    )
    .expect("write source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 204,
            "type": "request",
            "command": "setBreakpoints",
            "arguments": {
                "source": {
                    "path": source.display().to_string(),
                },
                "breakpoints": [
                    {
                        "line": 2,
                        "condition": "total == 9",
                    },
                    {
                        "line": 3,
                        "condition": "total == 9",
                    },
                ],
            },
        }))
        .expect("setBreakpoints response");
    session
        .message_response(&serde_json::json!({
            "seq": 205,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    let stack = session
        .message_response(&serde_json::json!({
            "seq": 206,
            "type": "request",
            "command": "stackTrace",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("stack response");

    assert_eq!(stack["success"], true, "{stack}");
    assert_eq!(stack["body"]["stackFrames"][0]["line"], 3);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_hit_condition_breakpoint_stops_on_requested_hit() {
    let dir = temp_output_dir("dap-hit-condition-breakpoint");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        r"function bump(value: int): int -> {
  let result: int = value + 1
  result
}
let first: int = bump(0)
let second: int = bump(1)
",
    )
    .expect("write source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 207,
            "type": "request",
            "command": "setBreakpoints",
            "arguments": {
                "source": {
                    "path": source.display().to_string(),
                },
                "breakpoints": [
                    {
                        "line": 2,
                        "hitCondition": "2",
                    },
                ],
            },
        }))
        .expect("setBreakpoints response");
    session
        .message_response(&serde_json::json!({
            "seq": 208,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    let locals = session
        .message_response(&serde_json::json!({
            "seq": 209,
            "type": "request",
            "command": "variables",
            "arguments": {
                "variablesReference": 2,
            },
        }))
        .expect("locals response");

    let vars = locals["body"]["variables"].as_array().expect("locals");
    assert!(
        vars.iter()
            .any(|var| var["name"] == "result" && var["value"] == "2"),
        "{locals}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_reverse_continue_stops_at_previous_verified_breakpoint_frame() {
    let dir = temp_output_dir("dap-reverse-continue");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        "let first: int = 1\nlet middle: int = 2\nlet last: int = 3\n",
    )
    .expect("write source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 181,
            "type": "request",
            "command": "setBreakpoints",
            "arguments": {
                "source": {
                    "path": source.display().to_string(),
                },
                "breakpoints": [
                    { "line": 1 },
                    { "line": 3 },
                ],
            },
        }))
        .expect("breakpoints response");
    session
        .message_response(&serde_json::json!({
            "seq": 182,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    session
        .message_response(&serde_json::json!({
            "seq": 183,
            "type": "request",
            "command": "continue",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("continue response");
    let _ = session.drain_pending_events();
    let reverse = session
        .message_response(&serde_json::json!({
            "seq": 184,
            "type": "request",
            "command": "reverseContinue",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("reverseContinue response");
    let events = session.drain_pending_events();
    let stack = session
        .message_response(&serde_json::json!({
            "seq": 185,
            "type": "request",
            "command": "stackTrace",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("stack response");

    assert_eq!(reverse["success"], true, "{reverse}");
    assert_eq!(stack["body"]["stackFrames"][0]["line"], 1);
    assert!(events.iter().any(|event| {
        event["type"] == "event"
            && event["event"] == "stopped"
            && event["body"]["reason"] == "breakpoint"
    }));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_function_breakpoint_stops_inside_named_function() {
    let dir = temp_output_dir("dap-function-breakpoint");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        r"function add(a: int, b: int): int -> {
  let result: int = a + b
  result
}
let total: int = add(2, 3)
",
    )
    .expect("write source");
    let mut session = DapSession::default();

    let breakpoints = session
        .message_response(&serde_json::json!({
            "seq": 190,
            "type": "request",
            "command": "setFunctionBreakpoints",
            "arguments": {
                "breakpoints": [
                    { "name": "add" },
                ],
            },
        }))
        .expect("setFunctionBreakpoints response");
    session
        .message_response(&serde_json::json!({
            "seq": 191,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    session
        .message_response(&serde_json::json!({
            "seq": 192,
            "type": "request",
            "command": "configurationDone",
            "arguments": {},
        }))
        .expect("configurationDone response");
    let events = session.drain_pending_events();
    let stack = session
        .message_response(&serde_json::json!({
            "seq": 193,
            "type": "request",
            "command": "stackTrace",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("stack response");

    assert_eq!(breakpoints["success"], true, "{breakpoints}");
    assert_eq!(breakpoints["body"]["breakpoints"][0]["verified"], true);
    assert_eq!(stack["success"], true, "{stack}");
    assert_eq!(stack["body"]["stackFrames"][0]["name"], "add");
    assert_eq!(stack["body"]["stackFrames"][0]["line"], 2);
    assert!(events.iter().any(|event| {
        event["type"] == "event"
            && event["event"] == "stopped"
            && event["body"]["reason"] == "function breakpoint"
    }));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_stack_trace_honors_start_frame_and_levels() {
    let dir = temp_output_dir("dap-stack-trace-paging");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        r"function add(a: int, b: int): int -> {
  let result: int = a + b
  result
}
let total: int = add(2, 3)
",
    )
    .expect("write source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 204,
            "type": "request",
            "command": "setFunctionBreakpoints",
            "arguments": {
                "breakpoints": [
                    { "name": "add" },
                ],
            },
        }))
        .expect("setFunctionBreakpoints response");
    session
        .message_response(&serde_json::json!({
            "seq": 205,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    let stack = session
        .message_response(&serde_json::json!({
            "seq": 206,
            "type": "request",
            "command": "stackTrace",
            "arguments": {
                "threadId": 1,
                "startFrame": 1,
                "levels": 1,
            },
        }))
        .expect("stack response");

    assert_eq!(stack["success"], true, "{stack}");
    assert_eq!(stack["body"]["totalFrames"], 2);
    let frames = stack["body"]["stackFrames"]
        .as_array()
        .expect("stack frames");
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0]["name"], "orv entry");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_continue_stops_at_next_function_breakpoint_frame() {
    let dir = temp_output_dir("dap-continue-function-breakpoint");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        r"let first: int = 1
function add(a: int, b: int): int -> {
  let result: int = a + b
  result
}
let total: int = add(2, 3)
",
    )
    .expect("write source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 194,
            "type": "request",
            "command": "setBreakpoints",
            "arguments": {
                "source": {
                    "path": source.display().to_string(),
                },
                "breakpoints": [
                    { "line": 1 },
                ],
            },
        }))
        .expect("setBreakpoints response");
    session
        .message_response(&serde_json::json!({
            "seq": 195,
            "type": "request",
            "command": "setFunctionBreakpoints",
            "arguments": {
                "breakpoints": [
                    { "name": "add" },
                ],
            },
        }))
        .expect("setFunctionBreakpoints response");
    session
        .message_response(&serde_json::json!({
            "seq": 196,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    session
        .message_response(&serde_json::json!({
            "seq": 197,
            "type": "request",
            "command": "continue",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("continue response");
    let events = session.drain_pending_events();
    let stack = session
        .message_response(&serde_json::json!({
            "seq": 198,
            "type": "request",
            "command": "stackTrace",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("stack response");

    assert_eq!(stack["success"], true, "{stack}");
    assert_eq!(stack["body"]["stackFrames"][0]["name"], "add");
    assert_eq!(stack["body"]["stackFrames"][0]["line"], 3);
    assert!(events.iter().any(|event| {
        event["type"] == "event"
            && event["event"] == "stopped"
            && event["body"]["reason"] == "function breakpoint"
    }));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_data_breakpoint_stops_when_local_changes() {
    let dir = temp_output_dir("dap-data-breakpoint");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "let mut total: int = 1\ntotal = total + 4\n").expect("write source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 199,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    let info = session
        .message_response(&serde_json::json!({
            "seq": 200,
            "type": "request",
            "command": "dataBreakpointInfo",
            "arguments": {
                "variablesReference": 2,
                "name": "total",
            },
        }))
        .expect("dataBreakpointInfo response");
    let data_id = info["body"]["dataId"].as_str().expect("data id");
    let set_data = session
        .message_response(&serde_json::json!({
            "seq": 201,
            "type": "request",
            "command": "setDataBreakpoints",
            "arguments": {
                "breakpoints": [
                    {
                        "dataId": data_id,
                        "accessType": "write",
                    },
                ],
            },
        }))
        .expect("setDataBreakpoints response");
    session
        .message_response(&serde_json::json!({
            "seq": 202,
            "type": "request",
            "command": "continue",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("continue response");
    let events = session.drain_pending_events();
    let stack = session
        .message_response(&serde_json::json!({
            "seq": 203,
            "type": "request",
            "command": "stackTrace",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("stack response");

    assert_eq!(info["success"], true, "{info}");
    assert_eq!(info["body"]["dataId"], "local:total");
    assert_eq!(set_data["success"], true, "{set_data}");
    assert_eq!(set_data["body"]["breakpoints"][0]["verified"], true);
    assert_eq!(stack["success"], true, "{stack}");
    assert_eq!(stack["body"]["stackFrames"][0]["line"], 2);
    assert!(events.iter().any(|event| {
        event["type"] == "event"
            && event["event"] == "stopped"
            && event["body"]["reason"] == "data breakpoint"
    }));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_stdio_emits_output_event_for_reference_stdout_after_launch() {
    let dir = temp_output_dir("dap-output-event");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "@out \"debug-ready\"\n").expect("write source");
    let input = protocol_request_frame(&serde_json::json!({
        "seq": 55,
        "type": "request",
        "command": "launch",
        "arguments": {
            "program": format!("file://{}", source.display()),
        },
    }));

    let output = dap_stdio_response(&input).expect("stdio response");
    let frames = protocol_frames(&output);
    let output_event = frames
        .iter()
        .find(|frame| frame["type"] == "event" && frame["event"] == "output")
        .expect("output event");

    assert_eq!(output_event["body"]["category"], "stdout");
    assert_eq!(output_event["body"]["output"], "debug-ready\n");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_stdio_emits_stderr_output_event_for_runtime_error_after_launch() {
    let dir = temp_output_dir("dap-error-output-event");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "throw \"panic!\"\n").expect("write source");
    let input = protocol_request_frame(&serde_json::json!({
        "seq": 56,
        "type": "request",
        "command": "launch",
        "arguments": {
            "program": format!("file://{}", source.display()),
        },
    }));

    let output = dap_stdio_response(&input).expect("stdio response");
    let frames = protocol_frames(&output);
    let output_event = frames
        .iter()
        .find(|frame| frame["type"] == "event" && frame["event"] == "output")
        .expect("output event");

    assert_eq!(frames[0]["body"]["runtime"]["status"], "error");
    assert_eq!(output_event["body"]["category"], "stderr");
    assert!(output_event["body"]["output"]
        .as_str()
        .is_some_and(|output| output.contains("panic!")));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_next_advances_to_next_executable_line_and_queues_stopped_event() {
    let dir = temp_output_dir("dap-next-line");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "let first: int = 1\n\nlet second: int = 2\n").expect("write source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 48,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    let first_stack = session
        .message_response(&serde_json::json!({
            "seq": 49,
            "type": "request",
            "command": "stackTrace",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("first stack response");
    let next = session
        .message_response(&serde_json::json!({
            "seq": 50,
            "type": "request",
            "command": "next",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("next response");
    let events = session.drain_pending_events();
    let second_stack = session
        .message_response(&serde_json::json!({
            "seq": 51,
            "type": "request",
            "command": "stackTrace",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("second stack response");

    assert_eq!(first_stack["body"]["stackFrames"][0]["line"], 1);
    assert_eq!(next["success"], true, "{next}");
    assert_eq!(next["body"], serde_json::json!({}));
    assert_eq!(second_stack["body"]["stackFrames"][0]["line"], 3);
    assert!(events.iter().any(|event| {
        event["type"] == "event"
            && event["event"] == "stopped"
            && event["body"]["reason"] == "step"
            && event["body"]["threadId"] == 1
    }));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_live_launch_defers_output_until_next_step() {
    let dir = temp_output_dir("dap-live-launch");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "let first: int = 1\n@out \"second\"\n").expect("write source");
    let mut session = DapSession::default();

    let launch = session
        .message_response(&serde_json::json!({
            "seq": 208,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
                "live": true,
            },
        }))
        .expect("launch response");
    let launch_events = session.drain_pending_events();
    let first_stack = session
        .message_response(&serde_json::json!({
            "seq": 209,
            "type": "request",
            "command": "stackTrace",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("first stack response");
    let next = session
        .message_response(&serde_json::json!({
            "seq": 210,
            "type": "request",
            "command": "next",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("next response");
    let next_events = session.drain_pending_events();

    assert_eq!(launch["success"], true, "{launch}");
    assert_eq!(launch["body"]["runtime"]["status"], "running");
    assert_eq!(launch["body"]["runtime"]["stdout"], "");
    assert!(launch_events
        .iter()
        .all(|event| { event["event"] != "output" || event["body"]["output"] != "second\n" }));
    assert_eq!(first_stack["body"]["stackFrames"][0]["line"], 1);
    assert_eq!(next["success"], true, "{next}");
    assert!(next_events.iter().any(|event| {
        event["type"] == "event"
            && event["event"] == "output"
            && event["body"]["category"] == "stdout"
            && event["body"]["output"] == "second\n"
    }));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_live_continue_stops_at_breakpoint_before_program_end() {
    let dir = temp_output_dir("dap-live-continue-breakpoint");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        "let first: int = 1\n@out \"middle\"\nlet third: int = 3\nlet done: int = 4\n",
    )
    .expect("write source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 211,
            "type": "request",
            "command": "setBreakpoints",
            "arguments": {
                "source": {
                    "path": source.display().to_string(),
                },
                "breakpoints": [
                    {
                        "line": 3,
                    },
                ],
            },
        }))
        .expect("setBreakpoints response");
    let launch = session
        .message_response(&serde_json::json!({
            "seq": 212,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
                "live": true,
            },
        }))
        .expect("launch response");
    let _ = session.drain_pending_events();
    let continue_response = session
        .message_response(&serde_json::json!({
            "seq": 213,
            "type": "request",
            "command": "continue",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("continue response");
    let events = session.drain_pending_events();
    let stack = session
        .message_response(&serde_json::json!({
            "seq": 214,
            "type": "request",
            "command": "stackTrace",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("stack response");

    assert_eq!(launch["body"]["runtime"]["status"], "running");
    assert_eq!(continue_response["success"], true, "{continue_response}");
    assert_eq!(stack["success"], true, "{stack}");
    assert_eq!(stack["body"]["stackFrames"][0]["line"], 3);
    assert!(events.iter().any(|event| {
        event["type"] == "event"
            && event["event"] == "stopped"
            && event["body"]["reason"] == "breakpoint"
    }));
    assert!(events.iter().all(|event| event["event"] != "terminated"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_live_step_in_rejects_target_id() {
    let dir = temp_output_dir("dap-live-step-in-target");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "let first: int = 1\nlet second: int = 2\n").expect("write source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 218,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
                "live": true,
            },
        }))
        .expect("launch response");
    let step_in = session
        .message_response(&serde_json::json!({
            "seq": 219,
            "type": "request",
            "command": "stepIn",
            "arguments": {
                "threadId": 1,
                "targetId": 1_000_000,
            },
        }))
        .expect("stepIn response");
    let stack = session
        .message_response(&serde_json::json!({
            "seq": 220,
            "type": "request",
            "command": "stackTrace",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("stack response");

    assert_eq!(step_in["success"], false, "{step_in}");
    assert!(step_in["message"]
        .as_str()
        .is_some_and(|message| message.contains("targetId is unavailable in live debug mode")));
    assert_eq!(stack["body"]["stackFrames"][0]["line"], 1);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_long_running_server_state_uses_server_frame_without_runtime() {
    let dir = temp_output_dir("dap-long-running-server-state");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        r"@server {
  @listen 0
  @route GET /ping { @respond 200 { ok: true } }
}
",
    )
    .expect("write source");
    let loaded = orv_project::load_project(&source).expect("load project");
    let resolved = orv_resolve::resolve(&loaded.program);
    let lowered = orv_analyzer::lower_with_diagnostics(&loaded.program, &resolved);
    let sources = loaded
        .files
        .iter()
        .enumerate()
        .map(|(index, file)| dap_source_info(file, u64::try_from(index + 1).unwrap_or(u64::MAX)))
        .collect::<Vec<_>>();

    let (runtime, frames) =
        dap_long_running_runtime_state(&lowered.program, &loaded.files, &sources);

    assert!(dap_program_has_long_running_runtime(&lowered.program));
    assert_eq!(runtime.status, "paused");
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].line, 1);
    assert_eq!(frames[0].stack[0].name, "server runtime");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_launch_server_program_reports_paused_long_running_runtime() {
    let dir = temp_output_dir("dap-server-long-running-launch");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        r"@server {
  @listen 0
  @route GET /ping { @respond 200 { ok: true } }
}
",
    )
    .expect("write source");
    let mut session = DapSession::default();

    let launch = session
        .message_response(&serde_json::json!({
            "seq": 221,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    let stack = session
        .message_response(&serde_json::json!({
            "seq": 222,
            "type": "request",
            "command": "stackTrace",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("stack response");

    assert_eq!(launch["success"], true, "{launch}");
    assert_eq!(launch["body"]["runtime"]["status"], "paused");
    assert_eq!(launch["body"]["runtime"]["async"]["route_count"], 1);
    assert_eq!(
        launch["body"]["runtime"]["async"]["routes"][0]["method"],
        "GET"
    );
    assert_eq!(
        launch["body"]["runtime"]["async"]["routes"][0]["path"],
        "/ping"
    );
    assert_eq!(stack["success"], true, "{stack}");
    assert_eq!(stack["body"]["stackFrames"][0]["line"], 1);
    assert_eq!(stack["body"]["stackFrames"][0]["name"], "server runtime");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_long_running_continue_and_pause_queue_events() {
    let dir = temp_output_dir("dap-server-long-running-pause");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        r"@server {
  @listen 0
  @route GET /ping { @respond 200 { ok: true } }
}
",
    )
    .expect("write source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 223,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    let _ = session.drain_pending_events();
    let continue_response = session
        .message_response(&serde_json::json!({
            "seq": 224,
            "type": "request",
            "command": "continue",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("continue response");
    let continue_events = session.drain_pending_events();
    let pause = session
        .message_response(&serde_json::json!({
            "seq": 225,
            "type": "request",
            "command": "pause",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("pause response");
    let pause_events = session.drain_pending_events();

    assert_eq!(continue_response["success"], true, "{continue_response}");
    assert!(continue_events.iter().any(|event| {
        event["type"] == "event" && event["event"] == "continued" && event["body"]["threadId"] == 1
    }));
    assert_eq!(pause["success"], true, "{pause}");
    assert!(pause_events.iter().any(|event| {
        event["type"] == "event"
            && event["event"] == "stopped"
            && event["body"]["reason"] == "pause"
            && event["body"]["threadId"] == 1
    }));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_long_running_exposes_async_pause_resume_state() {
    let dir = temp_output_dir("dap-server-async-state");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        "@server { @listen 0 @route GET /ping { @respond 200 { ok: true } } }\n",
    )
    .expect("write source");
    let mut session = DapSession::default();

    let launch = session
        .message_response(&serde_json::json!({
            "seq": 226,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    session
        .message_response(&serde_json::json!({
            "seq": 227,
            "type": "request",
            "command": "continue",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("continue response");
    session
        .message_response(&serde_json::json!({
            "seq": 228,
            "type": "request",
            "command": "pause",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("pause response");
    let variables = session
        .message_response(&serde_json::json!({
            "seq": 229,
            "type": "request",
            "command": "variables",
            "arguments": {
                "variablesReference": 1,
            },
        }))
        .expect("variables response");
    let async_state = session
        .message_response(&serde_json::json!({
            "seq": 230,
            "type": "request",
            "command": "evaluate",
            "arguments": {
                "expression": "runtimeAsyncState",
            },
        }))
        .expect("evaluate response");
    let completions = session
        .message_response(&serde_json::json!({
            "seq": 231,
            "type": "request",
            "command": "completions",
            "arguments": {
                "text": "runtime",
                "column": 8,
                "line": 1,
            },
        }))
        .expect("completions response");

    assert_eq!(launch["success"], true, "{launch}");
    assert_eq!(launch["body"]["runtime"]["async"]["kind"], "server");
    assert_eq!(launch["body"]["runtime"]["async"]["state"], "paused");
    assert!(variables["body"]["variables"]
        .as_array()
        .expect("variables")
        .iter()
        .any(|variable| variable["name"] == "runtimeAsyncState" && variable["value"] == "paused"));
    assert!(variables["body"]["variables"]
        .as_array()
        .expect("variables")
        .iter()
        .any(|variable| variable["name"] == "runtimeResumeCount" && variable["value"] == "1"));
    assert!(variables["body"]["variables"]
        .as_array()
        .expect("variables")
        .iter()
        .any(|variable| variable["name"] == "runtimePauseCount" && variable["value"] == "1"));
    assert_eq!(async_state["success"], true, "{async_state}");
    assert_eq!(async_state["body"]["result"], "paused");
    assert!(completions["body"]["targets"]
        .as_array()
        .expect("completion targets")
        .iter()
        .any(|target| target["label"] == "runtimeAsyncState" && target["type"] == "property"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_long_running_exposes_async_route_inventory() {
    let dir = temp_output_dir("dap-server-async-routes");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        "@server { @listen 0 @route GET /ping { @respond 200 { ok: true } } }\n",
    )
    .expect("write source");
    let mut session = DapSession::default();

    let launch = session
        .message_response(&serde_json::json!({
            "seq": 232,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    let variables = session
        .message_response(&serde_json::json!({
            "seq": 233,
            "type": "request",
            "command": "variables",
            "arguments": {
                "variablesReference": 1,
            },
        }))
        .expect("variables response");
    let routes = session
        .message_response(&serde_json::json!({
            "seq": 234,
            "type": "request",
            "command": "evaluate",
            "arguments": {
                "expression": "runtimeRoutes",
            },
        }))
        .expect("route evaluate response");
    let completions = session
        .message_response(&serde_json::json!({
            "seq": 235,
            "type": "request",
            "command": "completions",
            "arguments": {
                "text": "runtimeR",
                "column": 9,
                "line": 1,
            },
        }))
        .expect("completions response");

    assert_eq!(launch["body"]["runtime"]["async"]["route_count"], 1);
    assert_eq!(
        launch["body"]["runtime"]["async"]["routes"][0]["path"],
        "/ping"
    );
    assert!(variables["body"]["variables"]
        .as_array()
        .expect("variables")
        .iter()
        .any(|variable| variable["name"] == "runtimeRouteCount" && variable["value"] == "1"));
    assert_eq!(routes["success"], true, "{routes}");
    assert_eq!(routes["body"]["result"], "GET /ping");
    assert!(completions["body"]["targets"]
        .as_array()
        .expect("completion targets")
        .iter()
        .any(|target| target["label"] == "runtimeRoutes" && target["type"] == "property"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_attach_request_enables_runtime_transport() {
    let dir = temp_output_dir("dap-attach-runtime");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        "@server { @listen 0 @route GET /ping { @respond 200 { ok: true } } }\n",
    )
    .expect("write source");
    let mut session = DapSession::default();

    let attach = session
        .message_response(&serde_json::json!({
            "seq": 236,
            "type": "request",
            "command": "attach",
            "arguments": {
                "program": format!("file://{}", source.display()),
                "attachRuntimeMode": "inProcess",
            },
        }))
        .expect("attach response");
    assert_eq!(attach["type"], "response");
    assert_eq!(attach["command"], "attach");
    assert_eq!(attach["success"], true, "{attach}");
    assert_eq!(
        attach["body"]["runtime"]["async"]["transport"]["kind"],
        "in-process"
    );
    assert_eq!(
        attach["body"]["runtime"]["async"]["transport"]["state"],
        "detached"
    );
    session
        .message_response(&serde_json::json!({
            "seq": 237,
            "type": "request",
            "command": "continue",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("continue response");
    let address = session
        .launched
        .as_ref()
        .and_then(|launched| launched.async_runtime.as_ref())
        .and_then(|runtime| runtime.transport.as_ref())
        .and_then(|transport| transport.address.clone())
        .expect("in-process runtime address");

    let response = send_raw_http(&address, "/ping");

    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_in_process_runtime_exposes_request_trace_json() {
    let dir = temp_output_dir("dap-runtime-request-trace");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        "@server { @listen 0 @route GET /ping { @respond 200 { ok: true } } }\n",
    )
    .expect("write source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 236,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
                "attachRuntime": true,
                "attachRuntimeMode": "inProcess",
            },
        }))
        .expect("launch response");
    session
        .message_response(&serde_json::json!({
            "seq": 237,
            "type": "request",
            "command": "continue",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("continue response");
    let address = session
        .launched
        .as_ref()
        .and_then(|launched| launched.async_runtime.as_ref())
        .and_then(|runtime| runtime.transport.as_ref())
        .and_then(|transport| transport.address.clone())
        .expect("in-process runtime address");

    let response = send_raw_http(&address, "/ping");
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");

    let variables = session
        .message_response(&serde_json::json!({
            "seq": 238,
            "type": "request",
            "command": "variables",
            "arguments": {
                "variablesReference": 1,
            },
        }))
        .expect("variables response");
    let trace = session
        .message_response(&serde_json::json!({
            "seq": 239,
            "type": "request",
            "command": "evaluate",
            "arguments": {
                "expression": "runtimeRequestTrace",
            },
        }))
        .expect("trace evaluate response");
    let completions = session
        .message_response(&serde_json::json!({
            "seq": 240,
            "type": "request",
            "command": "completions",
            "arguments": {
                "text": "runtimeRequestT",
                "column": 16,
                "line": 1,
            },
        }))
        .expect("completions response");

    assert!(variables["body"]["variables"]
        .as_array()
        .expect("variables")
        .iter()
        .any(|variable| variable["name"] == "runtimeRequestTrace" && variable["type"] == "json"));
    assert_eq!(trace["success"], true, "{trace}");
    let trace_json: serde_json::Value =
        serde_json::from_str(trace["body"]["result"].as_str().expect("trace json string"))
            .expect("trace json");
    assert_eq!(trace_json["schema_version"], 1);
    assert_eq!(trace_json["kind"], "orv.production.trace");
    assert_eq!(trace_json["frames"][0]["method"], "GET");
    assert_eq!(trace_json["frames"][0]["path"], "/ping");
    assert_eq!(trace_json["frames"][0]["status"], 200);
    assert!(trace_json["frames"][0]["route_origin_id"]
        .as_str()
        .is_some_and(|origin| origin.starts_with("ori_")));
    assert!(completions["body"]["targets"]
        .as_array()
        .expect("completion targets")
        .iter()
        .any(|target| target["label"] == "runtimeRequestTrace"));
    drop(session);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_in_process_runtime_flushes_request_trace_path_on_pause() {
    let dir = temp_output_dir("dap-runtime-request-trace-path");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    let trace_path = dir.join("trace").join("requests.json");
    std::fs::write(
        &source,
        "@server { @listen 0 @route GET /ping { @respond 200 { ok: true } } }\n",
    )
    .expect("write source");
    let mut session = DapSession::default();

    let launch = dap_test_request(
        &mut session,
        241,
        "launch",
        serde_json::json!({
            "program": format!("file://{}", source.display()),
            "attachRuntime": true,
            "attachRuntimeMode": "inProcess",
            "runtimeRequestTracePath": trace_path.display().to_string(),
        }),
    );
    dap_test_request(
        &mut session,
        242,
        "continue",
        serde_json::json!({ "threadId": 1 }),
    );
    let address = session
        .launched
        .as_ref()
        .and_then(|launched| launched.async_runtime.as_ref())
        .and_then(|runtime| runtime.transport.as_ref())
        .and_then(|transport| transport.address.clone())
        .expect("in-process runtime address");

    let response = send_raw_http(&address, "/ping");
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let pause = dap_test_request(
        &mut session,
        243,
        "pause",
        serde_json::json!({ "threadId": 1 }),
    );

    assert_eq!(launch["success"], true, "{launch}");
    assert_eq!(pause["success"], true, "{pause}");
    let trace = read_json_value(&trace_path).expect("trace file");
    assert_eq!(trace["schema_version"], 1);
    assert_eq!(trace["kind"], "orv.production.trace");
    assert_eq!(trace["frames"][0]["method"], "GET");
    assert_eq!(trace["frames"][0]["path"], "/ping");
    assert_eq!(trace["frames"][0]["status"], 200);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_in_process_runtime_exposes_request_trace_path_expression() {
    let dir = temp_output_dir("dap-runtime-request-trace-path-expression");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    let trace_path = dir.join("trace").join("requests.json");
    std::fs::write(
        &source,
        "@server { @listen 0 @route GET /ping { @respond 200 { ok: true } } }\n",
    )
    .expect("write source");
    let mut session = DapSession::default();

    let launch = dap_test_request(
        &mut session,
        247,
        "launch",
        serde_json::json!({
            "program": format!("file://{}", source.display()),
            "attachRuntime": true,
            "attachRuntimeMode": "inProcess",
            "runtimeRequestTracePath": trace_path.display().to_string(),
        }),
    );
    let variables = dap_test_request(
        &mut session,
        248,
        "variables",
        serde_json::json!({ "variablesReference": 1 }),
    );
    let trace_path_value = dap_test_request(
        &mut session,
        249,
        "evaluate",
        serde_json::json!({ "expression": "runtimeRequestTracePath" }),
    );
    let completions = dap_test_request(
        &mut session,
        250,
        "completions",
        serde_json::json!({
            "text": "runtimeRequestTraceP",
            "column": 21,
            "line": 1,
        }),
    );

    assert_eq!(launch["success"], true, "{launch}");
    assert!(variables["body"]["variables"]
        .as_array()
        .expect("variables")
        .iter()
        .any(|variable| variable["name"] == "runtimeRequestTracePath"
            && variable["value"] == trace_path.display().to_string()));
    assert_eq!(trace_path_value["success"], true, "{trace_path_value}");
    assert_eq!(
        trace_path_value["body"]["result"],
        trace_path.display().to_string()
    );
    assert!(completions["body"]["targets"]
        .as_array()
        .expect("completion targets")
        .iter()
        .any(|target| target["label"] == "runtimeRequestTracePath"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_long_running_exposes_async_listen_endpoint() {
    let dir = temp_output_dir("dap-server-async-listen");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        "@server { @listen 8080 @route GET /ping { @respond 200 { ok: true } } }\n",
    )
    .expect("write source");
    let mut session = DapSession::default();

    let launch = session
        .message_response(&serde_json::json!({
            "seq": 236,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    let variables = session
        .message_response(&serde_json::json!({
            "seq": 237,
            "type": "request",
            "command": "variables",
            "arguments": {
                "variablesReference": 1,
            },
        }))
        .expect("variables response");
    let listen = session
        .message_response(&serde_json::json!({
            "seq": 238,
            "type": "request",
            "command": "evaluate",
            "arguments": {
                "expression": "runtimeListen",
            },
        }))
        .expect("listen evaluate response");
    let completions = session
        .message_response(&serde_json::json!({
            "seq": 239,
            "type": "request",
            "command": "completions",
            "arguments": {
                "text": "runtimeL",
                "column": 9,
                "line": 1,
            },
        }))
        .expect("completions response");

    assert_eq!(
        launch["body"]["runtime"]["async"]["listen"]["kind"],
        "static"
    );
    assert_eq!(launch["body"]["runtime"]["async"]["listen"]["port"], 8080);
    assert!(variables["body"]["variables"]
        .as_array()
        .expect("variables")
        .iter()
        .any(|variable| variable["name"] == "runtimeListen" && variable["value"] == "8080"));
    assert_eq!(listen["success"], true, "{listen}");
    assert_eq!(listen["body"]["result"], "8080");
    assert!(completions["body"]["targets"]
        .as_array()
        .expect("completion targets")
        .iter()
        .any(|target| target["label"] == "runtimeListen" && target["type"] == "property"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_long_running_exposes_env_listen_endpoint() {
    let dir = temp_output_dir("dap-server-env-listen");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        r#"@server {
  @listen int.from(@env.PORT ?? "8080")
  @route GET /ping { @respond 200 { ok: true } }
}
"#,
    )
    .expect("write source");
    let mut session = DapSession::default();

    let launch = session
        .message_response(&serde_json::json!({
            "seq": 240,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    let listen = session
        .message_response(&serde_json::json!({
            "seq": 241,
            "type": "request",
            "command": "evaluate",
            "arguments": {
                "expression": "runtimeListen",
            },
        }))
        .expect("listen evaluate response");

    assert_eq!(launch["body"]["runtime"]["async"]["listen"]["kind"], "env");
    assert_eq!(
        launch["body"]["runtime"]["async"]["listen"]["variable"],
        "PORT"
    );
    assert_eq!(
        launch["body"]["runtime"]["async"]["listen"]["default_port"],
        8080
    );
    assert_eq!(listen["success"], true, "{listen}");
    assert_eq!(listen["body"]["result"], "PORT default 8080");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_step_back_moves_to_previous_runtime_frame() {
    let dir = temp_output_dir("dap-step-back");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "let first: int = 1\nlet second: int = 2\n").expect("write source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 186,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    session
        .message_response(&serde_json::json!({
            "seq": 187,
            "type": "request",
            "command": "next",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("next response");
    let _ = session.drain_pending_events();
    let step_back = session
        .message_response(&serde_json::json!({
            "seq": 188,
            "type": "request",
            "command": "stepBack",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("stepBack response");
    let events = session.drain_pending_events();
    let stack = session
        .message_response(&serde_json::json!({
            "seq": 189,
            "type": "request",
            "command": "stackTrace",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("stack response");

    assert_eq!(step_back["success"], true, "{step_back}");
    assert_eq!(stack["body"]["stackFrames"][0]["line"], 1);
    assert!(events.iter().any(|event| {
        event["type"] == "event" && event["event"] == "stopped" && event["body"]["reason"] == "step"
    }));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_step_out_leaves_current_function_frame() {
    let dir = temp_output_dir("dap-step-out");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        r"function add(a: int, b: int): int -> {
  let result: int = a + b
  result
}
let total: int = add(2, 3)
let done: int = total
",
    )
    .expect("write source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 190,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    session
        .message_response(&serde_json::json!({
            "seq": 191,
            "type": "request",
            "command": "stepIn",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("stepIn response");
    let inside_stack = session
        .message_response(&serde_json::json!({
            "seq": 192,
            "type": "request",
            "command": "stackTrace",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("inside stack response");
    let step_out = session
        .message_response(&serde_json::json!({
            "seq": 193,
            "type": "request",
            "command": "stepOut",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("stepOut response");
    let events = session.drain_pending_events();
    let outside_stack = session
        .message_response(&serde_json::json!({
            "seq": 194,
            "type": "request",
            "command": "stackTrace",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("outside stack response");

    assert_eq!(inside_stack["body"]["stackFrames"][0]["name"], "add");
    assert_eq!(inside_stack["body"]["stackFrames"][0]["line"], 2);
    assert_eq!(step_out["success"], true, "{step_out}");
    assert_eq!(outside_stack["body"]["stackFrames"][0]["name"], "orv entry");
    assert_eq!(outside_stack["body"]["stackFrames"][0]["line"], 5);
    assert!(events.iter().any(|event| {
        event["type"] == "event" && event["event"] == "stopped" && event["body"]["reason"] == "step"
    }));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_next_steps_over_function_call_frames() {
    let dir = temp_output_dir("dap-next-step-over");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        r"function add(a: int, b: int): int -> {
  let result: int = a + b
  result
}
let total: int = add(2, 3)
let done: int = total
",
    )
    .expect("write source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 195,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    let next = session
        .message_response(&serde_json::json!({
            "seq": 196,
            "type": "request",
            "command": "next",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("next response");
    let stack = session
        .message_response(&serde_json::json!({
            "seq": 197,
            "type": "request",
            "command": "stackTrace",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("stack response");

    assert_eq!(next["success"], true, "{next}");
    assert_eq!(stack["body"]["stackFrames"][0]["name"], "orv entry");
    assert_eq!(stack["body"]["stackFrames"][0]["line"], 5);
    assert_eq!(stack["body"]["totalFrames"], 1);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_step_in_targets_enter_selected_function_frame() {
    let dir = temp_output_dir("dap-step-in-targets");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    let source_text = r"function add(a: int, b: int): int -> {
  let result: int = a + b
  result
}
let total: int = add(2, 3)
";
    std::fs::write(&source, source_text).expect("write source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 198,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    let targets = session
        .message_response(&serde_json::json!({
            "seq": 199,
            "type": "request",
            "command": "stepInTargets",
            "arguments": {
                "frameId": 1,
            },
        }))
        .expect("stepInTargets response");
    let add_target = targets["body"]["targets"]
        .as_array()
        .expect("targets")
        .iter()
        .find(|target| target["label"] == "add")
        .expect("add target");
    let target_id = add_target["id"].as_u64().expect("add target id");
    let step_in = session
        .message_response(&serde_json::json!({
            "seq": 200,
            "type": "request",
            "command": "stepIn",
            "arguments": {
                "threadId": 1,
                "targetId": target_id,
            },
        }))
        .expect("stepIn response");
    let events = session.drain_pending_events();
    let stack = session
        .message_response(&serde_json::json!({
            "seq": 201,
            "type": "request",
            "command": "stackTrace",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("stack response");
    let caller_frame_id = stack["body"]["stackFrames"]
        .as_array()
        .expect("stack frames")
        .get(1)
        .and_then(|frame| frame["id"].as_u64())
        .expect("caller frame id");
    let caller_scopes = session
        .message_response(&serde_json::json!({
            "seq": 202,
            "type": "request",
            "command": "scopes",
            "arguments": {
                "frameId": caller_frame_id,
            },
        }))
        .expect("caller scopes response");
    let caller_targets = session
        .message_response(&serde_json::json!({
            "seq": 203,
            "type": "request",
            "command": "stepInTargets",
            "arguments": {
                "frameId": caller_frame_id,
            },
        }))
        .expect("caller stepInTargets response");

    assert_eq!(targets["success"], true, "{targets}");
    assert_eq!(
        add_target["source"]["checksums"][0]["checksum"],
        serde_json::json!(sha256_hex(source_text.as_bytes()))
    );
    assert_eq!(step_in["success"], true, "{step_in}");
    assert_eq!(stack["body"]["stackFrames"][0]["name"], "add");
    assert_eq!(stack["body"]["stackFrames"][0]["line"], 2);
    assert_eq!(
        stack["body"]["stackFrames"][0]["source"]["checksums"][0]["checksum"],
        serde_json::json!(sha256_hex(source_text.as_bytes()))
    );
    assert_eq!(caller_scopes["success"], true, "{caller_scopes}");
    assert_eq!(
        caller_scopes["body"]["scopes"][0]["variablesReference"],
        serde_json::json!(0)
    );
    assert_eq!(
        caller_scopes["body"]["scopes"][0]["source"]["checksums"][0]["checksum"],
        serde_json::json!(sha256_hex(source_text.as_bytes()))
    );
    assert_eq!(caller_targets["success"], true, "{caller_targets}");
    assert_eq!(caller_targets["body"]["targets"], serde_json::json!([]));
    assert!(events.iter().any(|event| {
        event["type"] == "event" && event["event"] == "stopped" && event["body"]["reason"] == "step"
    }));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_restart_frame_rewinds_current_function_frame() {
    let dir = temp_output_dir("dap-restart-frame");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        r"function add(a: int, b: int): int -> {
  let result: int = a + b
  result
}
let total: int = add(2, 3)
",
    )
    .expect("write source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 202,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    session
        .message_response(&serde_json::json!({
            "seq": 203,
            "type": "request",
            "command": "stepIn",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("first stepIn response");
    session
        .message_response(&serde_json::json!({
            "seq": 204,
            "type": "request",
            "command": "stepIn",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("second stepIn response");
    let before = session
        .message_response(&serde_json::json!({
            "seq": 205,
            "type": "request",
            "command": "stackTrace",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("before stack response");
    let restart_frame = session
        .message_response(&serde_json::json!({
            "seq": 206,
            "type": "request",
            "command": "restartFrame",
            "arguments": {
                "frameId": 1,
            },
        }))
        .expect("restartFrame response");
    let events = session.drain_pending_events();
    let after = session
        .message_response(&serde_json::json!({
            "seq": 207,
            "type": "request",
            "command": "stackTrace",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("after stack response");

    assert_eq!(before["body"]["stackFrames"][0]["name"], "add");
    assert_eq!(before["body"]["stackFrames"][0]["line"], 3);
    assert_eq!(restart_frame["success"], true, "{restart_frame}");
    assert_eq!(after["body"]["stackFrames"][0]["name"], "add");
    assert_eq!(after["body"]["stackFrames"][0]["line"], 2);
    assert!(events.iter().any(|event| {
        event["type"] == "event"
            && event["event"] == "stopped"
            && event["body"]["reason"] == "restart"
    }));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_restart_frame_accepts_reported_entry_frame_id() {
    let dir = temp_output_dir("dap-restart-entry-frame");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        r"function add(a: int, b: int): int -> {
  let result: int = a + b
  result
}
let total: int = add(2, 3)
",
    )
    .expect("write source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 216,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    session
        .message_response(&serde_json::json!({
            "seq": 217,
            "type": "request",
            "command": "stepIn",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("first stepIn response");
    session
        .message_response(&serde_json::json!({
            "seq": 218,
            "type": "request",
            "command": "stepIn",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("second stepIn response");
    let before = session
        .message_response(&serde_json::json!({
            "seq": 219,
            "type": "request",
            "command": "stackTrace",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("before stack response");
    let entry_frame_id = before["body"]["stackFrames"]
        .as_array()
        .expect("stack frames")
        .iter()
        .find(|frame| frame["name"] == "orv entry")
        .and_then(|frame| frame["id"].as_u64())
        .expect("entry frame id");
    let restart_frame = session
        .message_response(&serde_json::json!({
            "seq": 220,
            "type": "request",
            "command": "restartFrame",
            "arguments": {
                "frameId": entry_frame_id,
            },
        }))
        .expect("restartFrame response");
    let after = session
        .message_response(&serde_json::json!({
            "seq": 221,
            "type": "request",
            "command": "stackTrace",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("after stack response");

    assert_eq!(restart_frame["success"], true, "{restart_frame}");
    assert_eq!(after["body"]["stackFrames"][0]["name"], "orv entry");
    assert_eq!(after["body"]["stackFrames"][0]["line"], 1);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_next_queues_output_for_reached_runtime_frame() {
    let dir = temp_output_dir("dap-next-output-frame");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "let first: int = 1\n@out \"second\"\n").expect("write source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 166,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    assert!(session.drain_pending_events().is_empty());
    session
        .message_response(&serde_json::json!({
            "seq": 167,
            "type": "request",
            "command": "next",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("next response");
    let events = session.drain_pending_events();

    assert!(events.iter().any(|event| {
        event["type"] == "event"
            && event["event"] == "output"
            && event["body"]["category"] == "stdout"
            && event["body"]["output"] == "second\n"
    }));
    assert!(events
        .iter()
        .any(|event| event["type"] == "event" && event["event"] == "stopped"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_stack_trace_names_runtime_function_frame() {
    let dir = temp_output_dir("dap-function-stack-frame");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        r"function add(a: int, b: int): int -> {
  let result: int = a + b
  result
}
let total: int = add(2, 3)
",
    )
    .expect("write source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 163,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    session
        .message_response(&serde_json::json!({
            "seq": 164,
            "type": "request",
            "command": "stepIn",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("stepIn response");
    let stack = session
        .message_response(&serde_json::json!({
            "seq": 165,
            "type": "request",
            "command": "stackTrace",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("stack response");

    assert_eq!(stack["success"], true, "{stack}");
    assert_eq!(stack["body"]["stackFrames"][0]["name"], "add");
    assert_eq!(stack["body"]["stackFrames"][0]["line"], 2);
    assert_eq!(stack["body"]["stackFrames"][1]["name"], "orv entry");
    assert_eq!(stack["body"]["totalFrames"], 2);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_next_after_last_executable_line_terminates_session() {
    let dir = temp_output_dir("dap-next-terminate");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "let only: int = 1\n").expect("write source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 68,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    let next = session
        .message_response(&serde_json::json!({
            "seq": 69,
            "type": "request",
            "command": "next",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("next response");
    let events = session.drain_pending_events();
    let stack = session
        .message_response(&serde_json::json!({
            "seq": 70,
            "type": "request",
            "command": "stackTrace",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("stack response");

    assert_eq!(next["success"], true, "{next}");
    assert!(events
        .iter()
        .any(|event| { event["type"] == "event" && event["event"] == "terminated" }));
    assert_eq!(stack["success"], false, "{stack}");
    assert!(stack["message"]
        .as_str()
        .is_some_and(|message| message.contains("launch is required")));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_pause_keeps_current_line_and_queues_pause_stopped_event() {
    let dir = temp_output_dir("dap-pause-event");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "let answer: int = 42\n").expect("write source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 52,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    let pause = session
        .message_response(&serde_json::json!({
            "seq": 53,
            "type": "request",
            "command": "pause",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("pause response");
    let events = session.drain_pending_events();
    let stack = session
        .message_response(&serde_json::json!({
            "seq": 54,
            "type": "request",
            "command": "stackTrace",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("stack response");

    assert_eq!(pause["success"], true, "{pause}");
    assert_eq!(stack["body"]["stackFrames"][0]["line"], 1);
    assert!(events.iter().any(|event| {
        event["type"] == "event"
            && event["event"] == "stopped"
            && event["body"]["reason"] == "pause"
            && event["body"]["threadId"] == 1
    }));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_terminate_threads_clears_launch_and_queues_terminated_event() {
    let dir = temp_output_dir("dap-terminate-threads");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "let answer: int = 42\n").expect("write source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 183,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    let terminate_threads = session
        .message_response(&serde_json::json!({
            "seq": 184,
            "type": "request",
            "command": "terminateThreads",
            "arguments": {
                "threadIds": [1],
            },
        }))
        .expect("terminateThreads response");
    let events = session.drain_pending_events();
    let stack = session
        .message_response(&serde_json::json!({
            "seq": 185,
            "type": "request",
            "command": "stackTrace",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("stack response");

    assert_eq!(terminate_threads["success"], true, "{terminate_threads}");
    assert!(events
        .iter()
        .any(|event| { event["type"] == "event" && event["event"] == "terminated" }));
    assert_eq!(stack["success"], false, "{stack}");
    assert!(stack["message"]
        .as_str()
        .is_some_and(|message| message.contains("launch is required")));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_restart_reloads_current_program_and_resets_stopped_line() {
    let dir = temp_output_dir("dap-restart");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "let first: int = 1\nlet second: int = 2\n").expect("write source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 78,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    session
        .message_response(&serde_json::json!({
            "seq": 79,
            "type": "request",
            "command": "next",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("next response");
    let moved_stack = session
        .message_response(&serde_json::json!({
            "seq": 80,
            "type": "request",
            "command": "stackTrace",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("moved stack response");
    let restart = session
        .message_response(&serde_json::json!({
            "seq": 81,
            "type": "request",
            "command": "restart",
            "arguments": {},
        }))
        .expect("restart response");
    let restarted_stack = session
        .message_response(&serde_json::json!({
            "seq": 82,
            "type": "request",
            "command": "stackTrace",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("restarted stack response");

    assert_eq!(moved_stack["body"]["stackFrames"][0]["line"], 2);
    assert_eq!(restart["success"], true, "{restart}");
    assert_eq!(restarted_stack["body"]["stackFrames"][0]["line"], 1);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_restart_preserves_live_launch_mode() {
    let dir = temp_output_dir("dap-restart-live");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "let first: int = 1\n@out \"after\"\n").expect("write source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 215,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
                "live": true,
            },
        }))
        .expect("launch response");
    let _ = session.drain_pending_events();
    let restart = session
        .message_response(&serde_json::json!({
            "seq": 216,
            "type": "request",
            "command": "restart",
            "arguments": {},
        }))
        .expect("restart response");
    let restart_events = session.drain_pending_events();
    let restarted_stack = session
        .message_response(&serde_json::json!({
            "seq": 217,
            "type": "request",
            "command": "stackTrace",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("restarted stack response");

    assert_eq!(restart["success"], true, "{restart}");
    assert_eq!(restart["body"]["runtime"]["status"], "running");
    assert_eq!(restart["body"]["runtime"]["stdout"], "");
    assert_eq!(restarted_stack["body"]["stackFrames"][0]["line"], 1);
    assert!(restart_events
        .iter()
        .all(|event| { event["event"] != "output" || event["body"]["output"] != "after\n" }));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_launch_threads_and_stacktrace_use_entry_source() {
    let dir = temp_output_dir("dap-launch");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "let answer: int = 42\n").expect("write source");
    let canonical_source = std::fs::canonicalize(&source).expect("canonical source");
    let mut session = DapSession::default();

    let launch = session
        .message_response(&serde_json::json!({
            "seq": 2,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    let threads = session
        .message_response(&serde_json::json!({
            "seq": 3,
            "type": "request",
            "command": "threads",
        }))
        .expect("threads response");
    let stack = session
        .message_response(&serde_json::json!({
            "seq": 4,
            "type": "request",
            "command": "stackTrace",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("stack response");

    assert_eq!(launch["success"], true, "{launch}");
    assert_eq!(launch["body"]["projectGraphNodes"], 1);
    assert_eq!(threads["body"]["threads"][0]["id"], 1);
    assert_eq!(stack["success"], true, "{stack}");
    assert_eq!(stack["body"]["totalFrames"], 1);
    let frame = &stack["body"]["stackFrames"][0];
    assert_eq!(frame["id"], 1);
    assert_eq!(frame["line"], 1);
    assert_eq!(frame["column"], 1);
    assert_eq!(
        frame["source"]["path"],
        canonical_source.display().to_string()
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_loaded_sources_returns_project_files_after_launch() {
    let dir = temp_output_dir("dap-loaded-sources");
    let models = dir.join("models");
    std::fs::create_dir_all(&models).expect("create models dir");
    let source = dir.join("app.orv");
    let imported = models.join("user.orv");
    let source_text = "import models.user.User\nlet u: User = { id: 1 }\n";
    let imported_source = "pub struct User { id: int }\n";
    std::fs::write(&source, source_text).expect("write source");
    std::fs::write(&imported, imported_source).expect("write imported");
    let mut session = DapSession::default();

    let launch = session
        .message_response(&serde_json::json!({
            "seq": 30,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    let loaded = session
        .message_response(&serde_json::json!({
            "seq": 31,
            "type": "request",
            "command": "loadedSources",
            "arguments": {},
        }))
        .expect("loadedSources response");

    assert_eq!(launch["success"], true, "{launch}");
    assert_eq!(loaded["success"], true, "{loaded}");
    let sources = loaded["body"]["sources"].as_array().expect("sources");
    assert!(sources
        .iter()
        .any(|item| item["name"] == "app.orv" && item["path"].as_str().is_some()));
    let imported_item = sources
        .iter()
        .find(|item| item["name"] == "user.orv" && item["path"].as_str().is_some())
        .expect("imported source");
    assert_eq!(
        imported_item["checksums"][0]["algorithm"],
        serde_json::json!("SHA256")
    );
    assert_eq!(
        imported_item["checksums"][0]["checksum"],
        serde_json::json!(sha256_hex(imported_source.as_bytes()))
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_modules_returns_project_sources_after_launch() {
    let dir = temp_output_dir("dap-modules");
    let models = dir.join("models");
    std::fs::create_dir_all(&models).expect("create models dir");
    let source = dir.join("app.orv");
    let imported = models.join("user.orv");
    std::fs::write(
        &source,
        "import models.user.User\nlet u: User = { id: 1 }\n",
    )
    .expect("write source");
    std::fs::write(&imported, "pub struct User { id: int }\n").expect("write imported");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 175,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    let modules = session
        .message_response(&serde_json::json!({
            "seq": 176,
            "type": "request",
            "command": "modules",
            "arguments": {
                "startModule": 0,
                "moduleCount": 1,
            },
        }))
        .expect("modules response");

    assert_eq!(modules["success"], true, "{modules}");
    assert_eq!(modules["body"]["totalModules"], 2);
    let items = modules["body"]["modules"].as_array().expect("modules");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["name"], "app.orv");
    assert_eq!(items[0]["id"], 1);
    assert_eq!(items[0]["isUserCode"], true);
    assert!(items[0]["path"].as_str().is_some());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_breakpoint_locations_return_project_graph_lines() {
    let dir = temp_output_dir("dap-breakpoint-locations");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        r#"struct User { id: int }

function greet(user: User): string -> "hello"
"#,
    )
    .expect("write source");
    let mut session = DapSession::default();

    let response = session
        .message_response(&serde_json::json!({
            "seq": 51,
            "type": "request",
            "command": "breakpointLocations",
            "arguments": {
                "source": {
                    "path": format!("file://{}", source.display()),
                },
                "line": 1,
                "endLine": 3,
            },
        }))
        .expect("breakpointLocations response");

    assert_eq!(response["success"], true, "{response}");
    let breakpoints = response["body"]["breakpoints"]
        .as_array()
        .expect("breakpoint locations");
    assert!(breakpoints
        .iter()
        .any(|breakpoint| breakpoint["line"] == 1 && breakpoint["column"] == 1));
    assert!(breakpoints
        .iter()
        .any(|breakpoint| breakpoint["line"] == 3 && breakpoint["column"] == 1));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_goto_targets_and_goto_move_to_executable_frame() {
    let dir = temp_output_dir("dap-goto");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "let first: int = 1\n\nlet third: int = 3\n").expect("write source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 177,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    let targets = session
        .message_response(&serde_json::json!({
            "seq": 178,
            "type": "request",
            "command": "gotoTargets",
            "arguments": {
                "source": {
                    "path": source.display().to_string(),
                },
                "line": 1,
                "endLine": 3,
            },
        }))
        .expect("gotoTargets response");
    assert_eq!(targets["success"], true, "{targets}");
    let target_id = targets["body"]["targets"]
        .as_array()
        .expect("targets")
        .iter()
        .find(|target| target["line"] == 3)
        .and_then(|target| target["id"].as_u64())
        .expect("line 3 target");
    let goto = session
        .message_response(&serde_json::json!({
            "seq": 179,
            "type": "request",
            "command": "goto",
            "arguments": {
                "threadId": 1,
                "targetId": target_id,
            },
        }))
        .expect("goto response");
    let events = session.drain_pending_events();
    let stack = session
        .message_response(&serde_json::json!({
            "seq": 180,
            "type": "request",
            "command": "stackTrace",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("stack response");

    let target_lines = targets["body"]["targets"]
        .as_array()
        .expect("targets")
        .iter()
        .map(|target| target["line"].as_u64().expect("line"))
        .collect::<Vec<_>>();
    assert_eq!(target_lines, vec![1, 3]);
    assert_eq!(goto["success"], true, "{goto}");
    assert_eq!(stack["body"]["stackFrames"][0]["line"], 3);
    assert!(events.iter().any(|event| {
        event["type"] == "event" && event["event"] == "stopped" && event["body"]["reason"] == "goto"
    }));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_exception_info_returns_launch_runtime_status() {
    let dir = temp_output_dir("dap-exception-info");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "let bad: int = \"wrong\"\n").expect("write source");
    let mut session = DapSession::default();

    let launch = session
        .message_response(&serde_json::json!({
            "seq": 52,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    let exception = session
        .message_response(&serde_json::json!({
            "seq": 53,
            "type": "request",
            "command": "exceptionInfo",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("exceptionInfo response");

    assert_eq!(launch["success"], true, "{launch}");
    assert_eq!(launch["body"]["runtime"]["status"], "diagnostics");
    assert_eq!(exception["success"], true, "{exception}");
    assert_eq!(exception["body"]["exceptionId"], "orv.diagnostics");
    assert_eq!(exception["body"]["description"], "diagnostics present");
    assert_eq!(exception["body"]["breakMode"], "always");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_source_returns_loaded_file_content_after_launch() {
    let dir = temp_output_dir("dap-source");
    let models = dir.join("models");
    std::fs::create_dir_all(&models).expect("create models dir");
    let source = dir.join("app.orv");
    let imported = models.join("user.orv");
    let imported_source = "pub struct User { id: int }\n";
    std::fs::write(
        &source,
        "import models.user.User\nlet u: User = { id: 1 }\n",
    )
    .expect("write source");
    std::fs::write(&imported, imported_source).expect("write imported");
    let canonical_imported = std::fs::canonicalize(&imported).expect("canonical imported");
    let mut session = DapSession::default();

    let launch = session
        .message_response(&serde_json::json!({
            "seq": 32,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    let source_response = session
        .message_response(&serde_json::json!({
            "seq": 33,
            "type": "request",
            "command": "source",
            "arguments": {
                "source": {
                    "path": canonical_imported.display().to_string(),
                },
            },
        }))
        .expect("source response");

    assert_eq!(launch["success"], true, "{launch}");
    assert_eq!(source_response["success"], true, "{source_response}");
    assert_eq!(source_response["body"]["content"], imported_source);
    assert_eq!(source_response["body"]["mimeType"], "text/x-orv");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_source_returns_content_by_loaded_source_reference() {
    let dir = temp_output_dir("dap-source-reference");
    let models = dir.join("models");
    std::fs::create_dir_all(&models).expect("create models dir");
    let source = dir.join("app.orv");
    let imported = models.join("user.orv");
    let imported_source = "pub struct User { id: int }\n";
    std::fs::write(
        &source,
        "import models.user.User\nlet u: User = { id: 1 }\n",
    )
    .expect("write source");
    std::fs::write(&imported, imported_source).expect("write imported");
    let mut session = DapSession::default();

    let launch = session
        .message_response(&serde_json::json!({
            "seq": 34,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    let loaded = session
        .message_response(&serde_json::json!({
            "seq": 35,
            "type": "request",
            "command": "loadedSources",
            "arguments": {},
        }))
        .expect("loadedSources response");
    let user_reference = loaded["body"]["sources"]
        .as_array()
        .expect("sources")
        .iter()
        .find(|item| item["name"] == "user.orv")
        .and_then(|item| item["sourceReference"].as_u64())
        .expect("user source reference");
    std::fs::remove_file(&imported).expect("remove imported after launch");
    let source_response = session
        .message_response(&serde_json::json!({
            "seq": 36,
            "type": "request",
            "command": "source",
            "arguments": {
                "sourceReference": user_reference,
            },
        }))
        .expect("source response");

    assert_eq!(launch["success"], true, "{launch}");
    assert!(user_reference > 0);
    assert_eq!(source_response["success"], true, "{source_response}");
    assert_eq!(source_response["body"]["content"], imported_source);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_launch_source_bundle_rehydrates_source_when_original_file_is_missing() {
    let dir = temp_output_dir("dap-source-bundle-launch");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    let source_text = "let answer: int = 42\n@out answer\n";
    std::fs::write(&source, source_text).expect("write source");
    let build_out = dir.join("dist");
    cmd_build_with_profile(&source, &build_out, BuildProfile::Production).expect("prod build");
    std::fs::remove_file(&source).expect("remove original source");
    let source_bundle_path = build_out.join(SOURCE_BUNDLE_PATH);
    assert_eq!(
        dap_launch_source_bundle_path(&serde_json::json!({
            "arguments": {
                "sourceBundle": source_bundle_path.display().to_string(),
            },
        }))
        .expect("camel sourceBundle path"),
        Some(source_bundle_path.clone())
    );
    assert_eq!(
        dap_launch_source_bundle_path(&serde_json::json!({
            "arguments": {
                "source_bundle": source_bundle_path.display().to_string(),
            },
        }))
        .expect("snake source_bundle path"),
        Some(source_bundle_path.clone())
    );
    let mut session = DapSession::default();
    let source_bundle_value =
        read_json_value(&source_bundle_path).expect("source bundle json value");
    let expected_source_bundle_hash =
        stable_json_hash(&source_bundle_value).expect("source bundle hash");

    let launch = session
        .message_response(&serde_json::json!({
            "seq": 37,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
                "sourceBundle": source_bundle_path.display().to_string(),
            },
        }))
        .expect("launch response");
    let loaded = session
        .message_response(&serde_json::json!({
            "seq": 38,
            "type": "request",
            "command": "loadedSources",
            "arguments": {},
        }))
        .expect("loadedSources response");
    let source_reference = loaded["body"]["sources"]
        .as_array()
        .expect("loaded sources")
        .iter()
        .find(|item| item["name"] == "app.orv")
        .and_then(|item| item["sourceReference"].as_u64())
        .expect("source reference");
    let source_response = session
        .message_response(&serde_json::json!({
            "seq": 39,
            "type": "request",
            "command": "source",
            "arguments": {
                "sourceReference": source_reference,
            },
        }))
        .expect("source response");
    let restart = session
        .message_response(&serde_json::json!({
            "seq": 40,
            "type": "request",
            "command": "restart",
            "arguments": {},
        }))
        .expect("restart response");

    assert_eq!(launch["success"], true, "{launch}");
    assert!(
        launch["body"]["projectGraphNodes"]
            .as_u64()
            .is_some_and(|count| count > 0),
        "{launch}"
    );
    assert_eq!(
        launch["body"]["sourceBundle"]["path"],
        source_bundle_path.display().to_string()
    );
    assert_eq!(
        launch["body"]["sourceBundle"]["entry"],
        source_bundle_value["entry"]
    );
    assert_eq!(launch["body"]["sourceBundle"]["fileCount"], 1);
    assert_eq!(
        launch["body"]["sourceBundle"]["hash"],
        expected_source_bundle_hash
    );
    assert_eq!(source_response["success"], true, "{source_response}");
    assert_eq!(source_response["body"]["content"], source_text);
    assert_eq!(restart["success"], true, "{restart}");
    assert_eq!(
        restart["body"]["sourceBundle"]["path"],
        source_bundle_path.display().to_string()
    );
    assert_eq!(
        restart["body"]["sourceBundle"]["hash"],
        expected_source_bundle_hash
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_set_breakpoints_and_stacktrace_use_verified_breakpoint_line() {
    let dir = temp_output_dir("dap-breakpoints");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "let first: int = 1\nlet second: int = 2\n").expect("write source");
    let mut session = DapSession::default();

    let breakpoints = session
        .message_response(&serde_json::json!({
            "seq": 5,
            "type": "request",
            "command": "setBreakpoints",
            "arguments": {
                "source": {
                    "path": source.display().to_string(),
                },
                "breakpoints": [
                    { "line": 2 }
                ],
            },
        }))
        .expect("breakpoints response");
    let launch = session
        .message_response(&serde_json::json!({
            "seq": 6,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    let stack = session
        .message_response(&serde_json::json!({
            "seq": 7,
            "type": "request",
            "command": "stackTrace",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("stack response");

    assert_eq!(breakpoints["success"], true, "{breakpoints}");
    assert_eq!(breakpoints["body"]["breakpoints"][0]["verified"], true);
    assert_eq!(breakpoints["body"]["breakpoints"][0]["line"], 2);
    assert_eq!(launch["success"], true, "{launch}");
    assert_eq!(stack["body"]["stackFrames"][0]["line"], 2);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_set_breakpoints_rejects_non_executable_lines() {
    let dir = temp_output_dir("dap-breakpoint-verify");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "let first: int = 1\n\nlet second: int = 2\n").expect("write source");
    let mut session = DapSession::default();

    let breakpoints = session
        .message_response(&serde_json::json!({
            "seq": 47,
            "type": "request",
            "command": "setBreakpoints",
            "arguments": {
                "source": {
                    "path": source.display().to_string(),
                },
                "breakpoints": [
                    { "line": 2 },
                    { "line": 3 }
                ],
            },
        }))
        .expect("breakpoints response");

    assert_eq!(breakpoints["success"], true, "{breakpoints}");
    assert_eq!(breakpoints["body"]["breakpoints"][0]["verified"], false);
    assert_eq!(
        breakpoints["body"]["breakpoints"][0]["message"],
        "no executable ORV node on this line"
    );
    assert_eq!(breakpoints["body"]["breakpoints"][1]["verified"], true);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_scopes_and_variables_expose_project_launch_state() {
    let dir = temp_output_dir("dap-variables");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    let source_text = "let answer: int = 42\n";
    std::fs::write(&source, source_text).expect("write source");
    let canonical_source = std::fs::canonicalize(&source).expect("canonical source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 8,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    let scopes = session
        .message_response(&serde_json::json!({
            "seq": 9,
            "type": "request",
            "command": "scopes",
            "arguments": {
                "frameId": 1,
            },
        }))
        .expect("scopes response");
    let variables = session
        .message_response(&serde_json::json!({
            "seq": 10,
            "type": "request",
            "command": "variables",
            "arguments": {
                "variablesReference": 1,
            },
        }))
        .expect("variables response");

    assert_eq!(scopes["success"], true, "{scopes}");
    assert_eq!(scopes["body"]["scopes"][0]["name"], "Project");
    assert_eq!(scopes["body"]["scopes"][0]["variablesReference"], 1);
    assert_eq!(scopes["body"]["scopes"][0]["namedVariables"], 6);
    assert_eq!(scopes["body"]["scopes"][1]["name"], "Locals");
    assert_eq!(
        scopes["body"]["scopes"][0]["source"]["checksums"][0]["algorithm"],
        serde_json::json!("SHA256")
    );
    assert_eq!(
        scopes["body"]["scopes"][0]["source"]["checksums"][0]["checksum"],
        serde_json::json!(sha256_hex(source_text.as_bytes()))
    );
    assert!(scopes["body"]["scopes"][1]["namedVariables"]
        .as_u64()
        .is_some_and(|count| count >= 1));
    let vars = variables["body"]["variables"]
        .as_array()
        .expect("variables");
    assert_eq!(
        scopes["body"]["scopes"][0]["namedVariables"],
        serde_json::json!(vars.len())
    );
    assert!(vars.iter().any(|var| {
        var["name"] == "entry" && var["value"] == canonical_source.display().to_string()
    }));
    assert!(vars
        .iter()
        .any(|var| var["name"] == "projectGraphNodes" && var["value"] == "1"));
    assert!(vars
        .iter()
        .any(|var| var["name"] == "diagnostics" && var["value"] == "0"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_scopes_rejects_unknown_frame_id() {
    let dir = temp_output_dir("dap-scopes-frame-id");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "let answer: int = 42\n").expect("write source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 214,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    let response = session
        .message_response(&serde_json::json!({
            "seq": 215,
            "type": "request",
            "command": "scopes",
            "arguments": {
                "frameId": 99,
            },
        }))
        .expect("scopes response");

    assert_eq!(response["success"], false, "{response}");
    assert!(response["message"]
        .as_str()
        .is_some_and(|message| message.contains("unknown ORV frameId 99")));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_variables_expose_top_level_locals() {
    let dir = temp_output_dir("dap-locals");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        "let answer: int = 42\nconst greeting = \"hello\"\nlet ready = true\n",
    )
    .expect("write source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 41,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    let scopes = session
        .message_response(&serde_json::json!({
            "seq": 42,
            "type": "request",
            "command": "scopes",
            "arguments": {
                "frameId": 1,
            },
        }))
        .expect("scopes response");
    let locals_ref = scopes["body"]["scopes"]
        .as_array()
        .expect("scopes")
        .iter()
        .find(|scope| scope["name"] == "Locals")
        .and_then(|scope| scope["variablesReference"].as_u64())
        .expect("locals scope");
    session
        .message_response(&serde_json::json!({
            "seq": 43,
            "type": "request",
            "command": "next",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("first next response");
    session
        .message_response(&serde_json::json!({
            "seq": 44,
            "type": "request",
            "command": "next",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("second next response");
    let locals = session
        .message_response(&serde_json::json!({
            "seq": 45,
            "type": "request",
            "command": "variables",
            "arguments": {
                "variablesReference": locals_ref,
            },
        }))
        .expect("locals response");

    assert_eq!(locals_ref, 2);
    assert_eq!(locals["success"], true, "{locals}");
    let vars = locals["body"]["variables"].as_array().expect("locals");
    assert!(vars
        .iter()
        .any(|var| var["name"] == "answer" && var["value"] == "42" && var["type"] == "int"));
    assert!(vars.iter().any(|var| {
        var["name"] == "greeting" && var["value"] == "\"hello\"" && var["type"] == "string"
    }));
    assert!(vars
        .iter()
        .any(|var| var["name"] == "ready" && var["value"] == "true" && var["type"] == "bool"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_variables_honor_start_and_count() {
    let dir = temp_output_dir("dap-variables-paging");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        "let answer: int = 42\nconst greeting = \"hello\"\nlet ready = true\n",
    )
    .expect("write source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 207,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    session
        .message_response(&serde_json::json!({
            "seq": 208,
            "type": "request",
            "command": "next",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("first next response");
    session
        .message_response(&serde_json::json!({
            "seq": 209,
            "type": "request",
            "command": "next",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("second next response");
    let locals = session
        .message_response(&serde_json::json!({
            "seq": 210,
            "type": "request",
            "command": "variables",
            "arguments": {
                "variablesReference": 2,
                "start": 1,
                "count": 1,
            },
        }))
        .expect("locals response");

    assert_eq!(locals["success"], true, "{locals}");
    let vars = locals["body"]["variables"].as_array().expect("locals");
    assert_eq!(vars.len(), 1);
    assert_eq!(vars[0]["name"], "greeting");
    assert_eq!(vars[0]["value"], "\"hello\"");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_variables_honor_named_and_indexed_filters() {
    let dir = temp_output_dir("dap-variables-filter");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "let answer: int = 42\n").expect("write source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 211,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    let named = session
        .message_response(&serde_json::json!({
            "seq": 212,
            "type": "request",
            "command": "variables",
            "arguments": {
                "variablesReference": 2,
                "filter": "named",
            },
        }))
        .expect("named locals response");
    let indexed = session
        .message_response(&serde_json::json!({
            "seq": 213,
            "type": "request",
            "command": "variables",
            "arguments": {
                "variablesReference": 2,
                "filter": "indexed",
            },
        }))
        .expect("indexed locals response");

    assert_eq!(named["success"], true, "{named}");
    assert_eq!(indexed["success"], true, "{indexed}");
    assert!(named["body"]["variables"]
        .as_array()
        .expect("named locals")
        .iter()
        .any(|var| var["name"] == "answer"));
    assert!(indexed["body"]["variables"]
        .as_array()
        .expect("indexed locals")
        .is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_set_variable_updates_current_local_and_evaluate() {
    let dir = temp_output_dir("dap-set-variable");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "let answer: int = 42\n").expect("write source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 168,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    let set_variable = session
        .message_response(&serde_json::json!({
            "seq": 169,
            "type": "request",
            "command": "setVariable",
            "arguments": {
                "variablesReference": 2,
                "name": "answer",
                "value": "99",
            },
        }))
        .expect("setVariable response");
    let locals = session
        .message_response(&serde_json::json!({
            "seq": 170,
            "type": "request",
            "command": "variables",
            "arguments": {
                "variablesReference": 2,
            },
        }))
        .expect("locals response");
    let evaluate = session
        .message_response(&serde_json::json!({
            "seq": 171,
            "type": "request",
            "command": "evaluate",
            "arguments": {
                "expression": "answer",
                "context": "repl",
            },
        }))
        .expect("evaluate response");

    assert_eq!(set_variable["success"], true, "{set_variable}");
    assert_eq!(set_variable["body"]["value"], "99");
    assert_eq!(set_variable["body"]["type"], "int");
    let vars = locals["body"]["variables"].as_array().expect("locals");
    assert!(vars
        .iter()
        .any(|var| var["name"] == "answer" && var["value"] == "99" && var["type"] == "int"));
    assert_eq!(evaluate["body"]["result"], "99");
    assert_eq!(evaluate["body"]["type"], "int");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_set_expression_updates_current_local() {
    let dir = temp_output_dir("dap-set-expression");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "let name = \"Ada\"\n").expect("write source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 172,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    let set_expression = session
        .message_response(&serde_json::json!({
            "seq": 173,
            "type": "request",
            "command": "setExpression",
            "arguments": {
                "expression": "name",
                "value": "\"Grace\"",
                "frameId": 1,
            },
        }))
        .expect("setExpression response");
    let evaluate = session
        .message_response(&serde_json::json!({
            "seq": 174,
            "type": "request",
            "command": "evaluate",
            "arguments": {
                "expression": "name",
                "context": "repl",
            },
        }))
        .expect("evaluate response");

    assert_eq!(set_expression["success"], true, "{set_expression}");
    assert_eq!(set_expression["body"]["value"], "\"Grace\"");
    assert_eq!(set_expression["body"]["type"], "string");
    assert_eq!(evaluate["body"]["result"], "\"Grace\"");
    assert_eq!(evaluate["body"]["type"], "string");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_evaluate_and_completions_include_top_level_locals() {
    let dir = temp_output_dir("dap-local-evaluate");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "let answer: int = 42\n").expect("write source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 44,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    let evaluate = session
        .message_response(&serde_json::json!({
            "seq": 45,
            "type": "request",
            "command": "evaluate",
            "arguments": {
                "expression": "answer",
                "context": "repl",
            },
        }))
        .expect("evaluate response");
    let completions = session
        .message_response(&serde_json::json!({
            "seq": 46,
            "type": "request",
            "command": "completions",
            "arguments": {
                "text": "ans",
                "column": 4,
                "line": 1,
            },
        }))
        .expect("completions response");

    assert_eq!(evaluate["success"], true, "{evaluate}");
    assert_eq!(evaluate["body"]["result"], "42");
    assert_eq!(evaluate["body"]["type"], "int");
    let targets = completions["body"]["targets"]
        .as_array()
        .expect("completion targets");
    assert!(targets
        .iter()
        .any(|target| target["label"] == "answer" && target["type"] == "variable"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_locals_use_runtime_values_from_function_calls() {
    let dir = temp_output_dir("dap-runtime-call-locals");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        "function add(a: int, b: int): int -> a + b\nlet total: int = add(2, 3)\n",
    )
    .expect("write source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 151,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    session
        .message_response(&serde_json::json!({
            "seq": 152,
            "type": "request",
            "command": "next",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("next response");
    let locals = session
        .message_response(&serde_json::json!({
            "seq": 153,
            "type": "request",
            "command": "variables",
            "arguments": {
                "variablesReference": 2,
            },
        }))
        .expect("locals response");
    let evaluate = session
        .message_response(&serde_json::json!({
            "seq": 154,
            "type": "request",
            "command": "evaluate",
            "arguments": {
                "expression": "total",
                "context": "repl",
            },
        }))
        .expect("evaluate response");

    let vars = locals["body"]["variables"].as_array().expect("locals");
    assert!(vars
        .iter()
        .any(|var| var["name"] == "total" && var["value"] == "5" && var["type"] == "int"));
    assert_eq!(evaluate["success"], true, "{evaluate}");
    assert_eq!(evaluate["body"]["result"], "5");
    assert_eq!(evaluate["body"]["type"], "int");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_locals_reflect_runtime_reassignment_after_step() {
    let dir = temp_output_dir("dap-runtime-assign-locals");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "let mut total: int = 1\ntotal = total + 4\n").expect("write source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 155,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    session
        .message_response(&serde_json::json!({
            "seq": 156,
            "type": "request",
            "command": "next",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("next response");
    let locals = session
        .message_response(&serde_json::json!({
            "seq": 157,
            "type": "request",
            "command": "variables",
            "arguments": {
                "variablesReference": 2,
            },
        }))
        .expect("locals response");

    let vars = locals["body"]["variables"].as_array().expect("locals");
    assert!(vars
        .iter()
        .any(|var| { var["name"] == "total" && var["value"] == "5" && var["type"] == "int" }));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_variables_include_reference_runtime_output() {
    let dir = temp_output_dir("dap-runtime-output");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "@out \"debug-ready\"\n").expect("write source");
    let mut session = DapSession::default();

    let launch = session
        .message_response(&serde_json::json!({
            "seq": 11,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    let variables = session
        .message_response(&serde_json::json!({
            "seq": 12,
            "type": "request",
            "command": "variables",
            "arguments": {
                "variablesReference": 1,
            },
        }))
        .expect("variables response");

    assert_eq!(launch["success"], true, "{launch}");
    assert_eq!(launch["body"]["runtime"]["status"], "ok");
    assert_eq!(launch["body"]["runtime"]["stdout"], "debug-ready\n");
    let vars = variables["body"]["variables"]
        .as_array()
        .expect("variables");
    assert!(vars
        .iter()
        .any(|var| var["name"] == "runtimeStatus" && var["value"] == "ok"));
    assert!(vars
        .iter()
        .any(|var| var["name"] == "stdout" && var["value"] == "debug-ready\n"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_evaluate_returns_project_runtime_values() {
    let dir = temp_output_dir("dap-evaluate");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "@out \"eval-ready\"\n").expect("write source");
    let mut session = DapSession::default();

    let launch = session
        .message_response(&serde_json::json!({
            "seq": 37,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    let evaluate = session
        .message_response(&serde_json::json!({
            "seq": 38,
            "type": "request",
            "command": "evaluate",
            "arguments": {
                "expression": "stdout",
                "context": "repl",
            },
        }))
        .expect("evaluate response");

    assert_eq!(launch["success"], true, "{launch}");
    assert_eq!(evaluate["success"], true, "{evaluate}");
    assert_eq!(evaluate["body"]["result"], "eval-ready\n");
    assert_eq!(evaluate["body"]["type"], "string");
    assert_eq!(evaluate["body"]["variablesReference"], 0);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_completions_returns_evaluable_project_values() {
    let dir = temp_output_dir("dap-completions");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "@out \"complete-ready\"\n").expect("write source");
    let mut session = DapSession::default();

    let launch = session
        .message_response(&serde_json::json!({
            "seq": 39,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    let completions = session
        .message_response(&serde_json::json!({
            "seq": 40,
            "type": "request",
            "command": "completions",
            "arguments": {
                "text": "std",
                "column": 4,
                "line": 1,
            },
        }))
        .expect("completions response");

    assert_eq!(launch["success"], true, "{launch}");
    assert_eq!(completions["success"], true, "{completions}");
    let targets = completions["body"]["targets"]
        .as_array()
        .expect("completion targets");
    assert!(targets
        .iter()
        .any(|target| target["label"] == "stdout" && target["type"] == "property"));
    assert!(targets.iter().all(|target| target["label"]
        .as_str()
        .is_some_and(|label| label.starts_with("std"))));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_locals_follow_current_stopped_line() {
    let dir = temp_output_dir("dap-line-locals");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "let first: int = 1\nlet second: int = 2\n").expect("write source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 57,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    let scopes = session
        .message_response(&serde_json::json!({
            "seq": 58,
            "type": "request",
            "command": "scopes",
            "arguments": {
                "frameId": 1,
            },
        }))
        .expect("scopes response");
    let locals_ref = scopes["body"]["scopes"]
        .as_array()
        .expect("scopes")
        .iter()
        .find(|scope| scope["name"] == "Locals")
        .and_then(|scope| scope["variablesReference"].as_u64())
        .expect("locals scope");
    let first_locals = session
        .message_response(&serde_json::json!({
            "seq": 59,
            "type": "request",
            "command": "variables",
            "arguments": {
                "variablesReference": locals_ref,
            },
        }))
        .expect("first locals response");
    session
        .message_response(&serde_json::json!({
            "seq": 60,
            "type": "request",
            "command": "next",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("next response");
    let second_locals = session
        .message_response(&serde_json::json!({
            "seq": 61,
            "type": "request",
            "command": "variables",
            "arguments": {
                "variablesReference": locals_ref,
            },
        }))
        .expect("second locals response");

    let first_vars = first_locals["body"]["variables"]
        .as_array()
        .expect("first locals");
    assert!(first_vars.iter().any(|var| var["name"] == "first"));
    assert!(!first_vars.iter().any(|var| var["name"] == "second"));
    let second_vars = second_locals["body"]["variables"]
        .as_array()
        .expect("second locals");
    assert!(second_vars.iter().any(|var| var["name"] == "first"));
    assert!(second_vars.iter().any(|var| var["name"] == "second"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_locals_evaluate_pure_top_level_expressions() {
    let dir = temp_output_dir("dap-expression-locals");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        "let base: int = 2\nlet doubled: int = base * 2 + 1\n",
    )
    .expect("write source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 62,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    session
        .message_response(&serde_json::json!({
            "seq": 63,
            "type": "request",
            "command": "next",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("next response");
    let scopes = session
        .message_response(&serde_json::json!({
            "seq": 64,
            "type": "request",
            "command": "scopes",
            "arguments": {
                "frameId": 1,
            },
        }))
        .expect("scopes response");
    let locals_ref = scopes["body"]["scopes"]
        .as_array()
        .expect("scopes")
        .iter()
        .find(|scope| scope["name"] == "Locals")
        .and_then(|scope| scope["variablesReference"].as_u64())
        .expect("locals scope");
    let locals = session
        .message_response(&serde_json::json!({
            "seq": 65,
            "type": "request",
            "command": "variables",
            "arguments": {
                "variablesReference": locals_ref,
            },
        }))
        .expect("locals response");
    let evaluate = session
        .message_response(&serde_json::json!({
            "seq": 66,
            "type": "request",
            "command": "evaluate",
            "arguments": {
                "expression": "doubled",
                "context": "repl",
            },
        }))
        .expect("evaluate response");

    let vars = locals["body"]["variables"].as_array().expect("locals");
    assert!(vars
        .iter()
        .any(|var| var["name"] == "doubled" && var["value"] == "5" && var["type"] == "int"));
    assert_eq!(evaluate["success"], true, "{evaluate}");
    assert_eq!(evaluate["body"]["result"], "5");
    assert_eq!(evaluate["body"]["type"], "int");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_locals_evaluate_array_and_object_initializers() {
    let dir = temp_output_dir("dap-compound-locals");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        "let xs = [1, 2, 3]\nlet user = { id: 1, name: \"Ada\" }\n",
    )
    .expect("write source");
    let mut session = DapSession::default();

    session
        .message_response(&serde_json::json!({
            "seq": 74,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
            },
        }))
        .expect("launch response");
    session
        .message_response(&serde_json::json!({
            "seq": 75,
            "type": "request",
            "command": "next",
            "arguments": {
                "threadId": 1,
            },
        }))
        .expect("next response");
    let scopes = session
        .message_response(&serde_json::json!({
            "seq": 76,
            "type": "request",
            "command": "scopes",
            "arguments": {
                "frameId": 1,
            },
        }))
        .expect("scopes response");
    let locals_ref = scopes["body"]["scopes"]
        .as_array()
        .expect("scopes")
        .iter()
        .find(|scope| scope["name"] == "Locals")
        .and_then(|scope| scope["variablesReference"].as_u64())
        .expect("locals scope");
    let locals = session
        .message_response(&serde_json::json!({
            "seq": 77,
            "type": "request",
            "command": "variables",
            "arguments": {
                "variablesReference": locals_ref,
            },
        }))
        .expect("locals response");

    let vars = locals["body"]["variables"].as_array().expect("locals");
    assert!(vars
        .iter()
        .any(|var| var["name"] == "xs" && var["value"] == "[1, 2, 3]" && var["type"] == "array"));
    assert!(vars.iter().any(|var| {
        var["name"] == "user"
            && var["value"] == "{ id: 1, name: \"Ada\" }"
            && var["type"] == "object"
    }));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_stdio_document_symbol_returns_symbols_for_file_uri() {
    let dir = temp_output_dir("lsp-document-symbol");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        r#"struct User {
  id: int
}

function greet(user: User): string -> "hello"
"#,
    )
    .expect("write source");
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 11,
        "method": "textDocument/documentSymbol",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
        },
    })
    .to_string();
    let input = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);

    let output = lsp_stdio_response(&input).expect("stdio response");
    let (_, response_body) = output
        .split_once("\r\n\r\n")
        .expect("content-length response frame");
    let response: serde_json::Value = serde_json::from_str(response_body).expect("response json");
    let symbols = response["result"].as_array().expect("document symbols");

    assert_eq!(response["id"], 11);
    assert!(response.get("error").is_none());
    assert!(symbols
        .iter()
        .any(|symbol| symbol["name"] == "User" && symbol["kind"] == 23));
    assert!(symbols
        .iter()
        .any(|symbol| symbol["name"] == "greet" && symbol["kind"] == 12));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_document_symbol_accepts_percent_encoded_file_uri() {
    let dir = temp_output_dir("lsp-document-symbol-space");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app space.orv");
    std::fs::write(&source, "struct User { id: int }\n").expect("write source");
    let uri = format!("file://{}", source.display()).replace(' ', "%20");

    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 12,
        "method": "textDocument/documentSymbol",
        "params": {
            "textDocument": {
                "uri": uri,
            },
        },
    }));

    assert!(response.get("error").is_none(), "{response}");
    assert!(response["result"]
        .as_array()
        .expect("document symbols")
        .iter()
        .any(|symbol| symbol["name"] == "User"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_text_document_diagnostic_returns_full_report_for_file_uri() {
    let dir = temp_output_dir("lsp-diagnostic");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "let bad: int = \"wrong\"\n").expect("write source");
    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 13,
        "method": "textDocument/diagnostic",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
        },
    }));

    assert_eq!(response["id"], 13);
    assert!(response.get("error").is_none(), "{response}");
    assert_eq!(response["result"]["kind"], "full");
    let items = response["result"]["items"]
        .as_array()
        .expect("diagnostic items");
    assert!(items.iter().any(|item| {
        item["severity"] == 1
            && item["message"]
                .as_str()
                .is_some_and(|message| message.contains("type mismatch"))
    }));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_code_action_returns_reveal_action_for_diagnostic_range() {
    let dir = temp_output_dir("lsp-code-action");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "let bad: int = \"wrong\"\n").expect("write source");
    let canonical_source = std::fs::canonicalize(&source).expect("canonical source");

    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 32,
        "method": "textDocument/codeAction",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
            "range": {
                "start": { "line": 0, "character": 0 },
                "end": { "line": 0, "character": 25 },
            },
            "context": {
                "diagnostics": [],
            },
        },
    }));

    assert_eq!(response["id"], 32);
    assert!(response.get("error").is_none(), "{response}");
    let actions = response["result"].as_array().expect("code actions");
    let action = actions
        .iter()
        .find(|action| {
            action["title"]
                .as_str()
                .is_some_and(|title| title.contains("type mismatch"))
        })
        .expect("diagnostic reveal action");
    assert_eq!(action["kind"], "quickfix");
    assert_eq!(action["command"]["command"], "orv.revealDiagnostic");
    assert_eq!(action["diagnostics"][0]["source"], "orv");
    assert_eq!(
        action["command"]["arguments"][0],
        format!("file://{}", canonical_source.display())
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_code_action_inserts_default_route_method_and_path() {
    let dir = temp_output_dir("lsp-code-action-route-method");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "@server {\n  @route {\n}\n").expect("write source");
    let uri = format!("file://{}", source.display());
    let canonical_uri = format!(
        "file://{}",
        std::fs::canonicalize(&source)
            .expect("canonical source")
            .display()
    );

    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 33,
        "method": "textDocument/codeAction",
        "params": {
            "textDocument": {
                "uri": uri,
            },
            "range": {
                "start": { "line": 1, "character": 2 },
                "end": { "line": 1, "character": 10 },
            },
            "context": {
                "diagnostics": [],
            },
        },
    }));

    assert_eq!(response["id"], 33);
    assert!(response.get("error").is_none(), "{response}");
    let actions = response["result"].as_array().expect("code actions");
    let action = actions
        .iter()
        .find(|action| action["title"] == "Insert default GET route head")
        .expect("route method quickfix");
    assert_eq!(action["kind"], "quickfix");
    assert_eq!(action["diagnostics"][0]["code"], "syntax/route-method");
    let change = &action["edit"]["changes"][canonical_uri.as_str()][0];
    assert_eq!(change["newText"], "GET /path ");
    assert_eq!(change["range"]["start"]["line"], 1);
    assert_eq!(change["range"]["start"]["character"], 9);
    assert_eq!(change["range"]["end"], change["range"]["start"]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_code_action_inserts_default_route_path() {
    let dir = temp_output_dir("lsp-code-action-route-path");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "@server {\n  @route GET {\n}\n").expect("write source");
    let uri = format!("file://{}", source.display());
    let canonical_uri = format!(
        "file://{}",
        std::fs::canonicalize(&source)
            .expect("canonical source")
            .display()
    );

    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 34,
        "method": "textDocument/codeAction",
        "params": {
            "textDocument": {
                "uri": uri,
            },
            "range": {
                "start": { "line": 1, "character": 2 },
                "end": { "line": 1, "character": 14 },
            },
            "context": {
                "diagnostics": [],
            },
        },
    }));

    assert_eq!(response["id"], 34);
    assert!(response.get("error").is_none(), "{response}");
    let actions = response["result"].as_array().expect("code actions");
    let action = actions
        .iter()
        .find(|action| action["title"] == "Insert default route path")
        .expect("route path quickfix");
    assert_eq!(action["kind"], "quickfix");
    assert_eq!(action["diagnostics"][0]["code"], "syntax/route-path");
    let change = &action["edit"]["changes"][canonical_uri.as_str()][0];
    assert_eq!(change["newText"], "/path ");
    assert_eq!(change["range"]["start"]["line"], 1);
    assert_eq!(change["range"]["start"]["character"], 13);
    assert_eq!(change["range"]["end"], change["range"]["start"]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_document_link_returns_import_targets() {
    let dir = temp_output_dir("lsp-document-link");
    let models = dir.join("models");
    std::fs::create_dir_all(&models).expect("create models dir");
    let source = dir.join("app.orv");
    let imported = models.join("user.orv");
    std::fs::write(&source, "import models.user.User\nlet ok: int = 1\n").expect("write source");
    std::fs::write(&imported, "pub struct User { id: int }\n").expect("write imported");
    let canonical_imported = std::fs::canonicalize(&imported).expect("canonical imported");

    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 24,
        "method": "textDocument/documentLink",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
        },
    }));

    assert_eq!(response["id"], 24);
    assert!(response.get("error").is_none(), "{response}");
    let links = response["result"].as_array().expect("document links");
    let link = links
        .iter()
        .find(|link| link["target"] == format!("file://{}", canonical_imported.display()))
        .expect("import document link");
    assert_eq!(link["range"]["start"]["line"], 0);
    assert_eq!(link["range"]["start"]["character"], 0);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_folding_range_returns_multiline_declarations() {
    let dir = temp_output_dir("lsp-folding-range");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        r#"struct User {
  id: int
  email: string
}

function greet(user: User): string -> {
  "hello"
}
"#,
    )
    .expect("write source");

    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 25,
        "method": "textDocument/foldingRange",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
        },
    }));

    assert_eq!(response["id"], 25);
    assert!(response.get("error").is_none(), "{response}");
    let ranges = response["result"].as_array().expect("folding ranges");
    assert!(ranges.iter().any(|range| {
        range["startLine"] == 0 && range["endLine"].as_u64().is_some_and(|line| line >= 3)
    }));
    assert!(ranges.iter().any(|range| {
        range["startLine"] == 5 && range["endLine"].as_u64().is_some_and(|line| line >= 7)
    }));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_selection_range_returns_structural_parent_range() {
    let dir = temp_output_dir("lsp-selection-range");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        r#"struct User {
  id: int
  email: string
}

function greet(user: User): string -> {
  "hello"
}
"#,
    )
    .expect("write source");

    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 26,
        "method": "textDocument/selectionRange",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
            "positions": [
                {
                    "line": 1,
                    "character": 4,
                },
            ],
        },
    }));

    assert_eq!(response["id"], 26);
    assert!(response.get("error").is_none(), "{response}");
    let selections = response["result"].as_array().expect("selection ranges");
    assert_eq!(selections.len(), 1);
    let selection = &selections[0];
    assert_eq!(selection["range"]["start"]["line"], 0);
    assert_eq!(selection["range"]["start"]["character"], 0);
    assert!(selection["range"]["end"]["line"]
        .as_u64()
        .is_some_and(|line| line >= 3));
    assert!(selection
        .get("parent")
        .is_none_or(serde_json::Value::is_null));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_prepare_rename_returns_identifier_range_and_placeholder() {
    let dir = temp_output_dir("lsp-prepare-rename");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "struct User { id: int }\n").expect("write source");

    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 27,
        "method": "textDocument/prepareRename",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
            "position": {
                "line": 0,
                "character": 8,
            },
        },
    }));

    assert_eq!(response["id"], 27);
    assert!(response.get("error").is_none(), "{response}");
    assert_eq!(response["result"]["placeholder"], "User");
    assert_eq!(response["result"]["range"]["start"]["line"], 0);
    assert_eq!(response["result"]["range"]["start"]["character"], 7);
    assert_eq!(response["result"]["range"]["end"]["character"], 11);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_prepare_rename_rejects_language_tokens_and_builtin_directives() {
    let dir = temp_output_dir("lsp-prepare-rename-language-token");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
            &source,
            "struct User { id: int }\n@server {\n  @route GET /ping {\n    @respond 200 \"ok\"\n  }\n}\n",
        )
        .expect("write source");

    let keyword_response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 29,
        "method": "textDocument/prepareRename",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
            "position": {
                "line": 0,
                "character": 1,
            },
        },
    }));
    let route_response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 30,
        "method": "textDocument/prepareRename",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
            "position": {
                "line": 2,
                "character": 4,
            },
        },
    }));

    assert_eq!(keyword_response["id"], 29);
    assert!(
        keyword_response.get("error").is_none(),
        "{keyword_response}"
    );
    assert!(keyword_response["result"].is_null());
    assert_eq!(route_response["id"], 30);
    assert!(route_response.get("error").is_none(), "{route_response}");
    assert!(route_response["result"].is_null());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_prepare_rename_rejects_domain_field_names() {
    let dir = temp_output_dir("lsp-prepare-rename-domain-field");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    let source_text = r#"@server {
  @route POST /checkout {
    let sku = @body.sku
  }
}
"#;
    std::fs::write(&source, source_text).expect("write source");
    let body_line = source_text.lines().nth(2).expect("body line");
    let character = body_line.rfind("sku").expect("body field");
    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 32,
        "method": "textDocument/prepareRename",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
            "position": {
                "line": 2,
                "character": character,
            },
        },
    }));

    assert_eq!(response["id"], 32);
    assert!(response.get("error").is_none(), "{response}");
    assert!(response["result"].is_null());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_rename_returns_workspace_edit_for_project_references() {
    let dir = temp_output_dir("lsp-rename");
    let models = dir.join("models");
    std::fs::create_dir_all(&models).expect("create models dir");
    let source = dir.join("app.orv");
    let imported = models.join("user.orv");
    std::fs::write(
        &source,
        "import models.user.User\nlet u: User = { id: 1 }\n",
    )
    .expect("write source");
    std::fs::write(&imported, "pub struct User { id: int }\n").expect("write imported");
    let canonical_source = std::fs::canonicalize(&source).expect("canonical source");
    let canonical_imported = std::fs::canonicalize(&imported).expect("canonical imported");

    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 28,
        "method": "textDocument/rename",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
            "position": {
                "line": 1,
                "character": 8,
            },
            "newName": "Account",
        },
    }));

    assert_eq!(response["id"], 28);
    assert!(response.get("error").is_none(), "{response}");
    let changes = response["result"]["changes"].as_object().expect("changes");
    let source_uri = format!("file://{}", canonical_source.display());
    let imported_uri = format!("file://{}", canonical_imported.display());
    let source_edits = changes
        .get(&source_uri)
        .and_then(serde_json::Value::as_array)
        .expect("source edits");
    let imported_edits = changes
        .get(&imported_uri)
        .and_then(serde_json::Value::as_array)
        .expect("imported edits");
    assert!(
        source_edits
            .iter()
            .filter(|edit| edit["newText"] == "Account")
            .count()
            >= 2
    );
    assert!(imported_edits
        .iter()
        .any(|edit| edit["newText"] == "Account"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_rename_rejects_keyword_new_name() {
    let dir = temp_output_dir("lsp-rename-keyword-new-name");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "struct User { id: int }\n").expect("write source");

    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 31,
        "method": "textDocument/rename",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
            "position": {
                "line": 0,
                "character": 8,
            },
            "newName": "struct",
        },
    }));

    assert_eq!(response["id"], 31);
    assert_eq!(response["error"]["code"], -32602);
    assert!(response["error"]["message"]
        .as_str()
        .is_some_and(|message| message.contains("non-keyword identifier")));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_document_highlight_returns_current_file_identifier_occurrences() {
    let dir = temp_output_dir("lsp-document-highlight");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        r"struct User { id: int }

let u: User = { id: 1 }
let v: User = u
",
    )
    .expect("write source");

    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 29,
        "method": "textDocument/documentHighlight",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
            "position": {
                "line": 2,
                "character": 8,
            },
        },
    }));

    assert_eq!(response["id"], 29);
    assert!(response.get("error").is_none(), "{response}");
    let highlights = response["result"].as_array().expect("highlights");
    assert_eq!(highlights.len(), 3);
    assert!(highlights
        .iter()
        .any(|highlight| highlight["range"]["start"]["line"] == 0));
    assert!(highlights
        .iter()
        .any(|highlight| highlight["range"]["start"]["line"] == 2));
    assert!(highlights
        .iter()
        .any(|highlight| highlight["range"]["start"]["line"] == 3));
    assert!(highlights.iter().all(|highlight| highlight["kind"] == 1));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_document_highlight_returns_domain_field_occurrences() {
    let dir = temp_output_dir("lsp-document-highlight-domain-field");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    let source_text = r#"@server {
  @route POST /checkout {
    let sku = @body.sku
    let label = sku
    let again = @body.sku
  }
}
"#;
    std::fs::write(&source, source_text).expect("write source");
    let first_body_line = source_text.lines().nth(2).expect("first body line");
    let second_body_line = source_text.lines().nth(4).expect("second body line");
    let first_character = first_body_line.rfind("sku").expect("first body field");
    let second_character = second_body_line.rfind("sku").expect("second body field");

    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 31,
        "method": "textDocument/documentHighlight",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
            "position": {
                "line": 2,
                "character": first_character,
            },
        },
    }));

    assert_eq!(response["id"], 31);
    assert!(response.get("error").is_none(), "{response}");
    let highlights = response["result"].as_array().expect("highlights");
    assert_eq!(highlights.len(), 2);
    assert!(highlights.iter().any(|highlight| {
        highlight["range"]["start"]["line"] == 2
            && highlight["range"]["start"]["character"] == first_character
    }));
    assert!(highlights.iter().any(|highlight| {
        highlight["range"]["start"]["line"] == 4
            && highlight["range"]["start"]["character"] == second_character
    }));
    assert!(highlights.iter().all(|highlight| highlight["kind"] == 1));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_document_highlight_ignores_language_keywords() {
    let dir = temp_output_dir("lsp-document-highlight-keyword");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "let total = 1\nlet next = total + 1\n").expect("write source");

    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 30,
        "method": "textDocument/documentHighlight",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
            "position": {
                "line": 0,
                "character": 1,
            },
        },
    }));

    assert_eq!(response["id"], 30);
    assert!(response.get("error").is_none(), "{response}");
    assert!(response["result"]
        .as_array()
        .expect("highlight result")
        .is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_semantic_tokens_returns_project_graph_declaration_tokens() {
    let dir = temp_output_dir("lsp-semantic-tokens");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        r#"struct User { id: int }

function greet(user: User): string -> "hello"
"#,
    )
    .expect("write source");

    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 30,
        "method": "textDocument/semanticTokens/full",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
        },
    }));

    assert_eq!(response["id"], 30);
    assert!(response.get("error").is_none(), "{response}");
    let data = response["result"]["data"]
        .as_array()
        .expect("semantic token data");
    assert_eq!(data.len() % 5, 0);
    let tokens: Vec<Vec<u64>> = data
        .chunks(5)
        .map(|chunk| {
            chunk
                .iter()
                .map(|value| value.as_u64().expect("semantic token integer"))
                .collect()
        })
        .collect();
    assert!(tokens
        .iter()
        .any(|token| token.as_slice() == [0, 7, 4, 1, 1]));
    assert!(tokens
        .iter()
        .any(|token| token.as_slice() == [2, 9, 5, 2, 1]));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_code_lens_returns_project_graph_reveal_commands() {
    let dir = temp_output_dir("lsp-code-lens");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        r#"struct User { id: int }

function greet(user: User): string -> "hello"
"#,
    )
    .expect("write source");

    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 31,
        "method": "textDocument/codeLens",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
        },
    }));

    assert_eq!(response["id"], 31);
    assert!(response.get("error").is_none(), "{response}");
    let lenses = response["result"].as_array().expect("code lenses");
    let user_lens = lenses
        .iter()
        .find(|lens| lens["command"]["arguments"][1] == "User")
        .expect("User code lens");
    assert_eq!(user_lens["range"]["start"]["line"], 0);
    assert_eq!(user_lens["command"]["command"], "orv.revealSourceNode");
    assert_eq!(user_lens["command"]["title"], "Reveal Struct User");
    assert!(lenses
        .iter()
        .any(|lens| lens["command"]["arguments"][1] == "greet"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_execute_command_reveals_project_graph_source_node() {
    let dir = temp_output_dir("lsp-execute-command");
    let src = dir.join("src");
    std::fs::create_dir_all(&src).expect("create src dir");
    let source = src.join("main.orv");
    std::fs::write(
        dir.join("orv.toml"),
        r#"[project]
name = "execute-command"
entry = "src/main.orv"
"#,
    )
    .expect("write manifest");
    std::fs::write(&source, "struct User { id: int }\n").expect("write source");
    let canonical_source = std::fs::canonicalize(&source).expect("canonical source");
    let mut session = LspSession::default();

    let initialize = session.jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 33,
        "method": "initialize",
        "params": {
            "rootUri": format!("file://{}", dir.display()),
        },
    }));
    let lenses = session.jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 34,
        "method": "textDocument/codeLens",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
        },
    }));
    let user_lens = lenses["result"]
        .as_array()
        .expect("code lenses")
        .iter()
        .find(|lens| lens["command"]["arguments"][1] == "User")
        .expect("User code lens")
        .clone();
    let execute = session.jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 35,
        "method": "workspace/executeCommand",
        "params": {
            "command": user_lens["command"]["command"],
            "arguments": user_lens["command"]["arguments"],
        },
    }));

    assert!(initialize.get("error").is_none(), "{initialize}");
    assert!(lenses.get("error").is_none(), "{lenses}");
    assert_eq!(execute["id"], 35);
    assert!(execute.get("error").is_none(), "{execute}");
    assert_eq!(execute["result"]["name"], "User");
    assert_eq!(execute["result"]["kind"], "Struct");
    assert_eq!(
        execute["result"]["source_node"],
        user_lens["command"]["arguments"][0]
    );
    assert_eq!(
        execute["result"]["location"]["uri"],
        format!("file://{}", canonical_source.display())
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_workspace_diagnostic_returns_imported_file_diagnostics() {
    let dir = temp_output_dir("lsp-workspace-diagnostic");
    let src = dir.join("src");
    let models = src.join("models");
    std::fs::create_dir_all(&models).expect("create models dir");
    let entry = src.join("main.orv");
    let imported = models.join("user.orv");
    std::fs::write(
        dir.join("orv.toml"),
        r#"[project]
name = "workspace-diagnostic"
entry = "src/main.orv"
"#,
    )
    .expect("write manifest");
    std::fs::write(&entry, "import models.user.User\nlet ok: int = 1\n").expect("write entry");
    std::fs::write(
        &imported,
        "pub struct User { id: int }\nlet bad: int = \"wrong\"\n",
    )
    .expect("write imported");
    let canonical_imported = std::fs::canonicalize(&imported).expect("canonical imported");
    let mut session = LspSession::default();

    let initialize = session.jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 22,
        "method": "initialize",
        "params": {
            "rootUri": format!("file://{}", dir.display()),
        },
    }));
    let response = session.jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 23,
        "method": "workspace/diagnostic",
        "params": {
            "previousResultIds": [],
        },
    }));

    assert!(initialize.get("error").is_none(), "{initialize}");
    assert_eq!(response["id"], 23);
    assert!(response.get("error").is_none(), "{response}");
    let items = response["result"]["items"]
        .as_array()
        .expect("workspace diagnostic items");
    let imported_report = items
        .iter()
        .find(|item| item["uri"] == format!("file://{}", canonical_imported.display()))
        .expect("imported diagnostic report");
    let diagnostics = imported_report["items"]
        .as_array()
        .expect("imported diagnostics");
    assert!(diagnostics.iter().any(|item| {
        item["message"]
            .as_str()
            .is_some_and(|message| message.contains("type mismatch"))
    }));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_definition_returns_symbol_declaration_location() {
    let dir = temp_output_dir("lsp-definition");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        r"struct User {
  id: int
}

let u: User = { id: 1 }
",
    )
    .expect("write source");
    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 16,
        "method": "textDocument/definition",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
            "position": {
                "line": 4,
                "character": 8,
            },
        },
    }));

    assert_eq!(response["id"], 16);
    assert!(response.get("error").is_none(), "{response}");
    let canonical_source = std::fs::canonicalize(&source).expect("canonical source");
    assert_eq!(
        response["result"]["uri"],
        format!("file://{}", canonical_source.display())
    );
    assert_eq!(response["result"]["range"]["start"]["line"], 0);
    assert_eq!(response["result"]["range"]["start"]["character"], 0);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_declaration_returns_symbol_declaration_location() {
    let dir = temp_output_dir("lsp-declaration");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    let source_text =
        "function greet(name: string): string -> name\nlet message: string = greet(\"Ada\")\n";
    std::fs::write(&source, source_text).expect("write source");
    let call_line = source_text.lines().nth(1).expect("call line");
    let call_character = call_line.find("greet").expect("call name");
    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 20,
        "method": "textDocument/declaration",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
            "position": {
                "line": 1,
                "character": call_character,
            },
        },
    }));

    assert_eq!(response["id"], 20);
    assert!(response.get("error").is_none(), "{response}");
    let canonical_source = std::fs::canonicalize(&source).expect("canonical source");
    assert_eq!(
        response["result"]["uri"],
        format!("file://{}", canonical_source.display())
    );
    assert_eq!(response["result"]["range"]["start"]["line"], 0);
    assert_eq!(response["result"]["range"]["start"]["character"], 0);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_type_definition_returns_type_declaration_location() {
    let dir = temp_output_dir("lsp-type-definition");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    let source_text = r"struct User {
  id: int
}

let u: User = { id: 1 }
";
    std::fs::write(&source, source_text).expect("write source");
    let binding_line = source_text.lines().nth(4).expect("binding line");
    let type_character = binding_line.find("User").expect("type name");
    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 21,
        "method": "textDocument/typeDefinition",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
            "position": {
                "line": 4,
                "character": type_character,
            },
        },
    }));

    assert_eq!(response["id"], 21);
    assert!(response.get("error").is_none(), "{response}");
    let canonical_source = std::fs::canonicalize(&source).expect("canonical source");
    assert_eq!(
        response["result"]["uri"],
        format!("file://{}", canonical_source.display())
    );
    assert_eq!(response["result"]["range"]["start"]["line"], 0);
    assert_eq!(response["result"]["range"]["start"]["character"], 0);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_implementation_returns_concrete_symbol_location() {
    let dir = temp_output_dir("lsp-implementation");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    let source_text =
        "function greet(name: string): string -> name\nlet message: string = greet(\"Ada\")\n";
    std::fs::write(&source, source_text).expect("write source");
    let call_line = source_text.lines().nth(1).expect("call line");
    let call_character = call_line.find("greet").expect("call name");
    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 27,
        "method": "textDocument/implementation",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
            "position": {
                "line": 1,
                "character": call_character,
            },
        },
    }));

    assert_eq!(response["id"], 27);
    assert!(response.get("error").is_none(), "{response}");
    let canonical_source = std::fs::canonicalize(&source).expect("canonical source");
    assert_eq!(
        response["result"]["uri"],
        format!("file://{}", canonical_source.display())
    );
    assert_eq!(response["result"]["range"]["start"]["line"], 0);
    assert_eq!(response["result"]["range"]["start"]["character"], 0);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_prepare_type_hierarchy_returns_type_item() {
    let dir = temp_output_dir("lsp-type-hierarchy-prepare");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    let source_text = "struct User {\n  id: int\n}\n\nlet u: User = { id: 1 }\n";
    std::fs::write(&source, source_text).expect("write source");
    let binding_line = source_text.lines().nth(4).expect("binding line");
    let type_character = binding_line.find("User").expect("type name");
    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 28,
        "method": "textDocument/prepareTypeHierarchy",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
            "position": {
                "line": 4,
                "character": type_character,
            },
        },
    }));

    assert_eq!(response["id"], 28);
    assert!(response.get("error").is_none(), "{response}");
    let items = response["result"].as_array().expect("type hierarchy items");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["name"], "User");
    assert_eq!(items[0]["kind"], 23);
    assert_eq!(items[0]["selectionRange"]["start"]["line"], 0);
    assert_eq!(items[0]["selectionRange"]["start"]["character"], 7);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_type_hierarchy_supertypes_and_subtypes_are_empty_without_inheritance() {
    let dir = temp_output_dir("lsp-type-hierarchy-empty");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    let source_text = "struct User {\n  id: int\n}\n";
    std::fs::write(&source, source_text).expect("write source");
    let canonical_source = std::fs::canonicalize(&source).expect("canonical source");
    let item = serde_json::json!({
        "name": "User",
        "kind": 23,
        "uri": format!("file://{}", canonical_source.display()),
        "range": {
            "start": { "line": 0, "character": 0 },
            "end": { "line": 2, "character": 1 },
        },
        "selectionRange": {
            "start": { "line": 0, "character": 7 },
            "end": { "line": 0, "character": 11 },
        },
    });
    let supertypes = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 29,
        "method": "typeHierarchy/supertypes",
        "params": {
            "item": item,
        },
    }));
    let subtypes = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 30,
        "method": "typeHierarchy/subtypes",
        "params": {
            "item": item,
        },
    }));

    assert_eq!(supertypes["id"], 29);
    assert!(supertypes.get("error").is_none(), "{supertypes}");
    assert_eq!(
        supertypes["result"].as_array().expect("supertypes").len(),
        0
    );
    assert_eq!(subtypes["id"], 30);
    assert!(subtypes.get("error").is_none(), "{subtypes}");
    assert_eq!(subtypes["result"].as_array().expect("subtypes").len(), 0);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_moniker_returns_project_symbol_identifier() {
    let dir = temp_output_dir("lsp-moniker");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    let source_text =
        "struct User {\n  id: int\n}\n\nfunction greet(user: User): string -> \"hello\"\n";
    std::fs::write(&source, source_text).expect("write source");
    let function_line = source_text.lines().nth(4).expect("function line");
    let function_character = function_line.find("greet").expect("function name");
    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 31,
        "method": "textDocument/moniker",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
            "position": {
                "line": 4,
                "character": function_character,
            },
        },
    }));

    assert_eq!(response["id"], 31);
    assert!(response.get("error").is_none(), "{response}");
    let monikers = response["result"].as_array().expect("monikers");
    assert_eq!(monikers.len(), 1);
    assert_eq!(monikers[0]["scheme"], "orv");
    assert_eq!(monikers[0]["identifier"], "function:greet");
    assert_eq!(monikers[0]["unique"], "project");
    assert_eq!(monikers[0]["kind"], "export");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_document_color_returns_hex_literal_ranges() {
    let dir = temp_output_dir("lsp-document-color");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    let source_text = "let accent = \"#336699\"\n";
    std::fs::write(&source, source_text).expect("write source");
    let color_character = source_text.find("#336699").expect("color literal");
    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 32,
        "method": "textDocument/documentColor",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
        },
    }));

    assert_eq!(response["id"], 32);
    assert!(response.get("error").is_none(), "{response}");
    let colors = response["result"].as_array().expect("document colors");
    assert_eq!(colors.len(), 1);
    assert_eq!(colors[0]["range"]["start"]["character"], color_character);
    assert_eq!(colors[0]["color"]["red"], 0.2);
    assert_eq!(colors[0]["color"]["green"], 0.4);
    assert_eq!(colors[0]["color"]["blue"], 0.6);
    assert_eq!(colors[0]["color"]["alpha"], 1.0);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_color_presentation_returns_hex_text_edit() {
    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 33,
        "method": "textDocument/colorPresentation",
        "params": {
            "textDocument": {
                "uri": "file:///tmp/app.orv",
            },
            "color": {
                "red": 0.2,
                "green": 0.4,
                "blue": 0.6,
                "alpha": 1.0,
            },
            "range": {
                "start": { "line": 0, "character": 14 },
                "end": { "line": 0, "character": 21 },
            },
        },
    }));

    assert_eq!(response["id"], 33);
    assert!(response.get("error").is_none(), "{response}");
    let presentations = response["result"].as_array().expect("color presentations");
    assert_eq!(presentations.len(), 1);
    assert_eq!(presentations[0]["label"], "#336699");
    assert_eq!(presentations[0]["textEdit"]["newText"], "#336699");
}

#[test]
fn lsp_linked_editing_range_returns_identifier_ranges() {
    let dir = temp_output_dir("lsp-linked-editing-range");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    let source_text = "let total = 1\nlet next = total + 1\n";
    std::fs::write(&source, source_text).expect("write source");
    let use_line = source_text.lines().nth(1).expect("use line");
    let use_character = use_line.find("total").expect("identifier use");
    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 34,
        "method": "textDocument/linkedEditingRange",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
            "position": {
                "line": 1,
                "character": use_character,
            },
        },
    }));

    assert_eq!(response["id"], 34);
    assert!(response.get("error").is_none(), "{response}");
    let result = response["result"]
        .as_object()
        .expect("linked editing result");
    let ranges = result["ranges"].as_array().expect("linked ranges");
    assert_eq!(ranges.len(), 2);
    assert_eq!(ranges[0]["start"]["line"], 0);
    assert_eq!(ranges[0]["start"]["character"], 4);
    assert_eq!(ranges[1]["start"]["line"], 1);
    assert_eq!(ranges[1]["start"]["character"], use_character);
    assert_eq!(result["wordPattern"], "[A-Za-z_][A-Za-z0-9_]*");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_linked_editing_range_ignores_builtin_directives() {
    let dir = temp_output_dir("lsp-linked-editing-range-directive");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "@server {\n  @route GET /ping {\n  }\n}\n").expect("write source");
    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 35,
        "method": "textDocument/linkedEditingRange",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
            "position": {
                "line": 1,
                "character": 4,
            },
        },
    }));

    assert_eq!(response["id"], 35);
    assert!(response.get("error").is_none(), "{response}");
    assert!(response["result"].is_null());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_prepare_call_hierarchy_returns_function_item() {
    let dir = temp_output_dir("lsp-call-hierarchy-prepare");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    let source_text = "function discount(price: int): int -> price\nfunction total(price: int): int -> discount(price)\nlet value: int = total(10)\n";
    std::fs::write(&source, source_text).expect("write source");
    let total_line = source_text.lines().nth(1).expect("total line");
    let total_character = total_line.find("total").expect("total name");
    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 24,
        "method": "textDocument/prepareCallHierarchy",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
            "position": {
                "line": 1,
                "character": total_character,
            },
        },
    }));

    assert_eq!(response["id"], 24);
    assert!(response.get("error").is_none(), "{response}");
    let items = response["result"].as_array().expect("call hierarchy items");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["name"], "total");
    assert_eq!(items[0]["kind"], 12);
    assert_eq!(
        items[0]["uri"],
        format!(
            "file://{}",
            std::fs::canonicalize(&source)
                .expect("canonical source")
                .display()
        )
    );
    assert_eq!(items[0]["selectionRange"]["start"]["line"], 1);
    assert_eq!(
        items[0]["selectionRange"]["start"]["character"],
        total_character
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_call_hierarchy_outgoing_returns_direct_calls() {
    let dir = temp_output_dir("lsp-call-hierarchy-outgoing");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    let source_text = "function discount(price: int): int -> price\nfunction total(price: int): int -> discount(price)\nlet value: int = total(10)\n";
    std::fs::write(&source, source_text).expect("write source");
    let canonical_source = std::fs::canonicalize(&source).expect("canonical source");
    let total_line = source_text.lines().nth(1).expect("total line");
    let call_character = total_line.find("discount").expect("discount call");
    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 25,
        "method": "callHierarchy/outgoingCalls",
        "params": {
            "item": {
                "name": "total",
                "kind": 12,
                "uri": format!("file://{}", canonical_source.display()),
                "range": {
                    "start": { "line": 1, "character": 0 },
                    "end": { "line": 1, "character": total_line.len() },
                },
                "selectionRange": {
                    "start": { "line": 1, "character": total_line.find("total").expect("total name") },
                    "end": { "line": 1, "character": total_line.find("total").expect("total name") + "total".len() },
                },
            },
        },
    }));

    assert_eq!(response["id"], 25);
    assert!(response.get("error").is_none(), "{response}");
    let calls = response["result"].as_array().expect("outgoing calls");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["to"]["name"], "discount");
    assert_eq!(calls[0]["to"]["kind"], 12);
    assert_eq!(calls[0]["fromRanges"][0]["start"]["line"], 1);
    assert_eq!(
        calls[0]["fromRanges"][0]["start"]["character"],
        call_character
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_call_hierarchy_incoming_returns_direct_callers() {
    let dir = temp_output_dir("lsp-call-hierarchy-incoming");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    let source_text = "function discount(price: int): int -> price\nfunction total(price: int): int -> discount(price)\nlet value: int = total(10)\n";
    std::fs::write(&source, source_text).expect("write source");
    let canonical_source = std::fs::canonicalize(&source).expect("canonical source");
    let discount_line = source_text.lines().next().expect("discount line");
    let total_line = source_text.lines().nth(1).expect("total line");
    let call_character = total_line.find("discount").expect("discount call");
    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 26,
        "method": "callHierarchy/incomingCalls",
        "params": {
            "item": {
                "name": "discount",
                "kind": 12,
                "uri": format!("file://{}", canonical_source.display()),
                "range": {
                    "start": { "line": 0, "character": 0 },
                    "end": { "line": 0, "character": discount_line.len() },
                },
                "selectionRange": {
                    "start": { "line": 0, "character": discount_line.find("discount").expect("discount name") },
                    "end": { "line": 0, "character": discount_line.find("discount").expect("discount name") + "discount".len() },
                },
            },
        },
    }));

    assert_eq!(response["id"], 26);
    assert!(response.get("error").is_none(), "{response}");
    let calls = response["result"].as_array().expect("incoming calls");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["from"]["name"], "total");
    assert_eq!(calls[0]["from"]["kind"], 12);
    assert_eq!(calls[0]["fromRanges"][0]["start"]["line"], 1);
    assert_eq!(
        calls[0]["fromRanges"][0]["start"]["character"],
        call_character
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_hover_returns_symbol_summary() {
    let dir = temp_output_dir("lsp-hover");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        r"struct User {
  id: int
}

let u: User = { id: 1 }
",
    )
    .expect("write source");
    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 17,
        "method": "textDocument/hover",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
            "position": {
                "line": 4,
                "character": 8,
            },
        },
    }));

    assert_eq!(response["id"], 17);
    assert!(response.get("error").is_none(), "{response}");
    assert_eq!(response["result"]["contents"]["kind"], "markdown");
    assert_eq!(response["result"]["contents"]["value"], "**Struct** `User`");
    assert_eq!(response["result"]["range"]["start"]["line"], 0);
    assert_eq!(response["result"]["range"]["start"]["character"], 0);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_hover_returns_request_body_field_summary() {
    let dir = temp_output_dir("lsp-hover-body-field");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    let source_text = r#"@server {
  @route POST /checkout {
    let sku = @body.sku
  }
}
"#;
    std::fs::write(&source, source_text).expect("write source");
    let body_line = source_text.lines().nth(2).expect("body line");
    let character = body_line.rfind("sku").expect("body field");
    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 18,
        "method": "textDocument/hover",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
            "position": {
                "line": 2,
                "character": character,
            },
        },
    }));

    assert_eq!(response["id"], 18);
    assert!(response.get("error").is_none(), "{response}");
    assert_eq!(response["result"]["contents"]["kind"], "markdown");
    assert_eq!(
        response["result"]["contents"]["value"],
        "**Request body field** `sku`"
    );
    assert_eq!(response["result"]["range"]["start"]["line"], 2);
    assert_eq!(response["result"]["range"]["start"]["character"], character);
    assert_eq!(
        response["result"]["range"]["end"]["character"],
        character + "sku".len()
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_hover_returns_env_value_summary() {
    let dir = temp_output_dir("lsp-hover-env-field");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    let source_text = r#"@server {
  let db = @db.connect(@env.SHOP_DATABASE_URL ?? "sqlite://data/shop.sqlite")
}
"#;
    std::fs::write(&source, source_text).expect("write source");
    let env_line = source_text.lines().nth(1).expect("env line");
    let character = env_line.find("SHOP_DATABASE_URL").expect("env field name");
    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 19,
        "method": "textDocument/hover",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
            "position": {
                "line": 1,
                "character": character,
            },
        },
    }));

    assert_eq!(response["id"], 19);
    assert!(response.get("error").is_none(), "{response}");
    assert_eq!(response["result"]["contents"]["kind"], "markdown");
    assert_eq!(
        response["result"]["contents"]["value"],
        "**Environment value** `SHOP_DATABASE_URL`"
    );
    assert_eq!(response["result"]["range"]["start"]["line"], 1);
    assert_eq!(response["result"]["range"]["start"]["character"], character);
    assert_eq!(
        response["result"]["range"]["end"]["character"],
        character + "SHOP_DATABASE_URL".len()
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_signature_help_returns_function_parameters() {
    let dir = temp_output_dir("lsp-signature-help");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    let source_text =
        "function add(left: int, right: int): int -> left + right\nlet total: int = add(1, 2)\n";
    std::fs::write(&source, source_text).expect("write source");
    let call_line = source_text.lines().nth(1).expect("call line");
    let character = call_line.find('2').expect("second argument");
    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 18,
        "method": "textDocument/signatureHelp",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
            "position": {
                "line": 1,
                "character": character,
            },
        },
    }));

    assert_eq!(response["id"], 18);
    assert!(response.get("error").is_none(), "{response}");
    assert_eq!(response["result"]["activeSignature"], 0);
    assert_eq!(response["result"]["activeParameter"], 1);
    let signature = &response["result"]["signatures"][0];
    assert_eq!(signature["label"], "add(left: int, right: int): int");
    assert_eq!(signature["parameters"][0]["label"], "left: int");
    assert_eq!(signature["parameters"][1]["label"], "right: int");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_inlay_hint_returns_function_parameter_labels() {
    let dir = temp_output_dir("lsp-inlay-hint");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    let source_text =
        "function add(left: int, right: int): int -> left + right\nlet total: int = add(1, 2)\n";
    std::fs::write(&source, source_text).expect("write source");
    let call_line = source_text.lines().nth(1).expect("call line");
    let first_arg = call_line.find('1').expect("first argument");
    let second_arg = call_line.find('2').expect("second argument");
    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 19,
        "method": "textDocument/inlayHint",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
            "range": {
                "start": { "line": 0, "character": 0 },
                "end": { "line": 1, "character": call_line.len() },
            },
        },
    }));

    assert_eq!(response["id"], 19);
    assert!(response.get("error").is_none(), "{response}");
    let hints = response["result"].as_array().expect("inlay hints");
    assert!(hints.iter().any(|hint| {
        hint["label"] == "left:"
            && hint["kind"] == 2
            && hint["position"]["line"] == 1
            && hint["position"]["character"] == first_arg
    }));
    assert!(hints.iter().any(|hint| {
        hint["label"] == "right:"
            && hint["kind"] == 2
            && hint["position"]["line"] == 1
            && hint["position"]["character"] == second_arg
    }));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_formatting_returns_full_document_text_edit_for_unsaved_content() {
    let dir = temp_output_dir("lsp-formatting");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    let uri = format!("file://{}", source.display());
    let mut session = LspSession::default();
    session.handle_notification(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {
                    "uri": uri,
                    "languageId": "orv",
                    "version": 1,
                    "text": "@server {\n@listen 8080  \n@route GET /ping {\n@respond 200 {\nok: true\n}\n}\n}\n",
                },
            },
        }));

    let response = session.jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 44,
        "method": "textDocument/formatting",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
            "options": {
                "tabSize": 2,
                "insertSpaces": true,
            },
        },
    }));

    assert_eq!(response["id"], 44);
    assert!(response.get("error").is_none(), "{response}");
    let edits = response["result"].as_array().expect("format edits");
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0]["range"]["start"]["line"], 0);
    assert_eq!(edits[0]["range"]["start"]["character"], 0);
    assert_eq!(edits[0]["range"]["end"]["line"], 8);
    assert_eq!(edits[0]["range"]["end"]["character"], 0);
    assert_eq!(
            edits[0]["newText"],
            "@server {\n  @listen 8080\n  @route GET /ping {\n    @respond 200 {\n      ok: true\n    }\n  }\n}\n"
        );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_range_formatting_uses_surrounding_indent_context() {
    let dir = temp_output_dir("lsp-range-formatting");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    let uri = format!("file://{}", source.display());
    let mut session = LspSession::default();
    session.handle_notification(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {
                    "uri": uri,
                    "languageId": "orv",
                    "version": 1,
                    "text": "@server {\n@listen 8080\n@route GET /ping {\n@respond 200 {\nok: true\n}\n}\n}\n",
                },
            },
        }));

    let response = session.jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 45,
        "method": "textDocument/rangeFormatting",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
            "range": {
                "start": { "line": 2, "character": 0 },
                "end": { "line": 6, "character": 1 },
            },
            "options": {
                "tabSize": 2,
                "insertSpaces": true,
            },
        },
    }));

    assert_eq!(response["id"], 45);
    assert!(response.get("error").is_none(), "{response}");
    let edits = response["result"].as_array().expect("format edits");
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0]["range"]["start"]["line"], 2);
    assert_eq!(edits[0]["range"]["start"]["character"], 0);
    assert_eq!(edits[0]["range"]["end"]["line"], 7);
    assert_eq!(edits[0]["range"]["end"]["character"], 0);
    assert_eq!(
        edits[0]["newText"],
        "  @route GET /ping {\n    @respond 200 {\n      ok: true\n    }\n  }\n"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_on_type_formatting_indents_new_current_line() {
    let dir = temp_output_dir("lsp-on-type-formatting-newline");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    let uri = format!("file://{}", source.display());
    let mut session = LspSession::default();
    session.handle_notification(&serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {
            "textDocument": {
                "uri": uri,
                "languageId": "orv",
                "version": 1,
                "text": "@server {\n",
            },
        },
    }));

    let response = session.jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 46,
        "method": "textDocument/onTypeFormatting",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
            "position": { "line": 1, "character": 0 },
            "ch": "\n",
            "options": {
                "tabSize": 2,
                "insertSpaces": true,
            },
        },
    }));

    assert_eq!(response["id"], 46);
    assert!(response.get("error").is_none(), "{response}");
    let edits = response["result"].as_array().expect("format edits");
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0]["range"]["start"]["line"], 1);
    assert_eq!(edits[0]["range"]["start"]["character"], 0);
    assert_eq!(edits[0]["newText"], "  ");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_on_type_formatting_aligns_closing_brace_line() {
    let dir = temp_output_dir("lsp-on-type-formatting-brace");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    let uri = format!("file://{}", source.display());
    let mut session = LspSession::default();
    session.handle_notification(&serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {
                    "uri": uri,
                    "languageId": "orv",
                    "version": 1,
                    "text": "@server {\n  @route GET /ping {\n    @respond 200 {\n      ok: true\n}\n  }\n}\n",
                },
            },
        }));

    let response = session.jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 47,
        "method": "textDocument/onTypeFormatting",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
            "position": { "line": 4, "character": 1 },
            "ch": "}",
            "options": {
                "tabSize": 2,
                "insertSpaces": true,
            },
        },
    }));

    assert_eq!(response["id"], 47);
    assert!(response.get("error").is_none(), "{response}");
    let edits = response["result"].as_array().expect("format edits");
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0]["range"]["start"]["line"], 4);
    assert_eq!(edits[0]["range"]["end"]["line"], 5);
    assert_eq!(edits[0]["newText"], "    }\n");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_references_returns_identifier_locations() {
    let dir = temp_output_dir("lsp-references");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        r#"struct User {
  id: int
}

function greet(user: User): string -> "hello"

let u: User = { id: 1 }
"#,
    )
    .expect("write source");
    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 19,
        "method": "textDocument/references",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
            "position": {
                "line": 6,
                "character": 8,
            },
        },
    }));

    assert_eq!(response["id"], 19);
    assert!(response.get("error").is_none(), "{response}");
    let locations = response["result"].as_array().expect("reference locations");
    assert!(locations.iter().any(|location| {
        location["range"]["start"]["line"] == 0 && location["range"]["start"]["character"] == 7
    }));
    assert!(locations.iter().any(|location| {
        location["range"]["start"]["line"] == 4 && location["range"]["start"]["character"] == 21
    }));
    assert!(locations.iter().any(|location| {
        location["range"]["start"]["line"] == 6 && location["range"]["start"]["character"] == 7
    }));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_references_returns_domain_field_locations() {
    let dir = temp_output_dir("lsp-references-domain-field");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    let source_text = r#"@server {
  @route POST /checkout {
    let sku = @body.sku
    let label = sku
    let again = @body.sku
  }
}
"#;
    std::fs::write(&source, source_text).expect("write source");
    let first_body_line = source_text.lines().nth(2).expect("first body line");
    let second_body_line = source_text.lines().nth(4).expect("second body line");
    let first_character = first_body_line.rfind("sku").expect("first body field");
    let second_character = second_body_line.rfind("sku").expect("second body field");
    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 21,
        "method": "textDocument/references",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
            "position": {
                "line": 2,
                "character": first_character,
            },
        },
    }));

    assert_eq!(response["id"], 21);
    assert!(response.get("error").is_none(), "{response}");
    let locations = response["result"].as_array().expect("reference locations");
    assert_eq!(locations.len(), 2);
    assert!(locations.iter().any(|location| {
        location["range"]["start"]["line"] == 2
            && location["range"]["start"]["character"] == first_character
    }));
    assert!(locations.iter().any(|location| {
        location["range"]["start"]["line"] == 4
            && location["range"]["start"]["character"] == second_character
    }));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_references_ignore_language_keywords() {
    let dir = temp_output_dir("lsp-references-keyword");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        "struct User { id: int }\nstruct Post { id: int }\n",
    )
    .expect("write source");
    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 20,
        "method": "textDocument/references",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
            "position": {
                "line": 0,
                "character": 1,
            },
        },
    }));

    assert_eq!(response["id"], 20);
    assert!(response.get("error").is_none(), "{response}");
    assert!(response["result"]
        .as_array()
        .expect("reference result")
        .is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_completion_returns_project_symbols() {
    let dir = temp_output_dir("lsp-completion");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        r#"struct User {
  id: int
}

function greet(user: User): string -> "hello"

@server {
  @route GET /ping {
    @respond 200 "ok"
  }
}
"#,
    )
    .expect("write source");
    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 18,
        "method": "textDocument/completion",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
            "position": {
                "line": 5,
                "character": 0,
            },
        },
    }));

    assert_eq!(response["id"], 18);
    assert!(response.get("error").is_none(), "{response}");
    assert_eq!(response["result"]["isIncomplete"], false);
    let items = response["result"]["items"]
        .as_array()
        .expect("completion items");
    assert!(items
        .iter()
        .any(|item| item["label"] == "User" && item["kind"] == 22));
    assert!(items
        .iter()
        .any(|item| item["label"] == "greet" && item["kind"] == 3));
    assert!(items
        .iter()
        .any(|item| item["label"] == "route" && item["kind"] == 23));
    assert!(items
        .iter()
        .any(|item| item["label"] == "function" && item["kind"] == 15));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_completion_returns_directive_snippets_at_at_prefix() {
    let dir = temp_output_dir("lsp-completion-directives");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        r#"@server {
  @
}
"#,
    )
    .expect("write source");
    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 19,
        "method": "textDocument/completion",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
            "position": {
                "line": 1,
                "character": 3,
            },
        },
    }));

    assert_eq!(response["id"], 19);
    assert!(response.get("error").is_none(), "{response}");
    let items = response["result"]["items"]
        .as_array()
        .expect("completion items");
    let route = items
        .iter()
        .find(|item| item["label"] == "@route")
        .expect("@route completion");
    assert_eq!(route["kind"], 15);
    assert_eq!(route["insertTextFormat"], 2);
    assert_eq!(route["insertText"], "@route ${1:GET} ${2:/path} {\n  $0\n}");
    assert!(items
        .iter()
        .any(|item| item["label"] == "@payment.connect" && item["kind"] == 15));
    assert!(items
        .iter()
        .any(|item| item["label"] == "@shipping.connect" && item["kind"] == 15));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_completion_returns_route_methods_inside_route_head() {
    let dir = temp_output_dir("lsp-completion-route-methods");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "@server {\n  @route \n}\n").expect("write source");
    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 20,
        "method": "textDocument/completion",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
            "position": {
                "line": 1,
                "character": 9,
            },
        },
    }));

    assert_eq!(response["id"], 20);
    assert!(response.get("error").is_none(), "{response}");
    let items = response["result"]["items"]
        .as_array()
        .expect("completion items");
    assert!(items
        .iter()
        .any(|item| item["label"] == "GET" && item["kind"] == 14));
    assert!(items
        .iter()
        .any(|item| item["label"] == "POST" && item["kind"] == 14));
    assert!(!items.iter().any(|item| item["label"] == "@route"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_completion_returns_domain_field_names_after_dot() {
    let dir = temp_output_dir("lsp-completion-domain-fields");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        r#"@server {
  let db = @db.connect(@env.SHOP_DATABASE_URL ?? "sqlite://data/shop.sqlite")
  @route POST /checkout {
    let sku = @body.sku
    let quantity = @body.quantity
    let id = @param.orderId
    let page = @query.page
    let next = @body.
  }
}
"#,
    )
    .expect("write source");
    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 21,
        "method": "textDocument/completion",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
            "position": {
                "line": 7,
                "character": 21,
            },
        },
    }));

    assert_eq!(response["id"], 21);
    assert!(response.get("error").is_none(), "{response}");
    let items = response["result"]["items"]
        .as_array()
        .expect("completion items");
    assert!(items
        .iter()
        .any(|item| item["label"] == "sku" && item["kind"] == 10));
    assert!(items
        .iter()
        .any(|item| item["label"] == "quantity" && item["kind"] == 10));
    assert!(!items.iter().any(|item| item["label"] == "@route"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_completion_returns_route_path_params_after_param_dot() {
    let dir = temp_output_dir("lsp-completion-route-param-fields");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        r#"@server {
  @route GET /orders/:orderId/items/:itemId {
    let current = @param.
  }
}
"#,
    )
    .expect("write source");
    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 22,
        "method": "textDocument/completion",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
            "position": {
                "line": 2,
                "character": 25,
            },
        },
    }));

    assert_eq!(response["id"], 22);
    assert!(response.get("error").is_none(), "{response}");
    let items = response["result"]["items"]
        .as_array()
        .expect("completion items");
    assert!(items
        .iter()
        .any(|item| item["label"] == "orderId" && item["kind"] == 10));
    assert!(items
        .iter()
        .any(|item| item["label"] == "itemId" && item["kind"] == 10));
    assert!(!items.iter().any(|item| item["label"] == "@param"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_completion_returns_env_names_after_dot() {
    let dir = temp_output_dir("lsp-completion-env-fields");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        r#"@server {
  let db = @db.connect(@env.SHOP_DATABASE_URL ?? "sqlite://data/shop.sqlite")
  let payments = @payment.connect(@env.PAYMENT_ADAPTER_URL ?? "file://data/payments.jsonl")
  let current = @env.
}
"#,
    )
    .expect("write source");
    let response = lsp_jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 22,
        "method": "textDocument/completion",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
            "position": {
                "line": 3,
                "character": 21,
            },
        },
    }));

    assert_eq!(response["id"], 22);
    assert!(response.get("error").is_none(), "{response}");
    let items = response["result"]["items"]
        .as_array()
        .expect("completion items");
    assert!(items
        .iter()
        .any(|item| item["label"] == "SHOP_DATABASE_URL" && item["kind"] == 21));
    assert!(items
        .iter()
        .any(|item| item["label"] == "PAYMENT_ADAPTER_URL" && item["kind"] == 21));
    assert!(!items.iter().any(|item| item["label"] == "@env"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_workspace_symbol_returns_matching_project_symbols() {
    let dir = temp_output_dir("lsp-workspace-symbol");
    let src = dir.join("src");
    let models = src.join("models");
    std::fs::create_dir_all(&models).expect("create models dir");
    let entry = src.join("main.orv");
    let imported = models.join("user.orv");
    std::fs::write(
        dir.join("orv.toml"),
        r#"[project]
name = "workspace-symbol"
entry = "src/main.orv"
"#,
    )
    .expect("write manifest");
    std::fs::write(
        &entry,
        "import models.user.User\nfunction checkout(user: User): string -> \"ok\"\n",
    )
    .expect("write entry");
    std::fs::write(&imported, "pub struct User { id: int }\n").expect("write imported");
    let canonical_imported = std::fs::canonicalize(&imported).expect("canonical imported");
    let mut session = LspSession::default();

    let initialize = session.jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 20,
        "method": "initialize",
        "params": {
            "rootUri": format!("file://{}", dir.display()),
        },
    }));
    let response = session.jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 21,
        "method": "workspace/symbol",
        "params": {
            "query": "User",
        },
    }));

    assert!(initialize.get("error").is_none(), "{initialize}");
    assert_eq!(response["id"], 21);
    assert!(response.get("error").is_none(), "{response}");
    let symbols = response["result"].as_array().expect("workspace symbols");
    let user = symbols
        .iter()
        .find(|symbol| symbol["name"] == "User")
        .expect("User workspace symbol");
    assert_eq!(user["kind"], 23);
    assert_eq!(
        user["location"]["uri"],
        format!("file://{}", canonical_imported.display())
    );
    assert!(symbols.iter().all(|symbol| symbol["name"]
        .as_str()
        .is_some_and(|name| name.contains("User"))));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_stdio_document_symbol_uses_did_open_unsaved_content() {
    let dir = temp_output_dir("lsp-did-open-symbol");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("unsaved.orv");
    let uri = format!("file://{}", source.display());
    let did_open = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {
            "textDocument": {
                "uri": uri,
                "languageId": "orv",
                "version": 1,
                "text": "struct Draft { id: int }\n",
            },
        },
    })
    .to_string();
    let document_symbol = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 14,
        "method": "textDocument/documentSymbol",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
        },
    })
    .to_string();
    let input = format!(
        "Content-Length: {}\r\n\r\n{}Content-Length: {}\r\n\r\n{}",
        did_open.len(),
        did_open,
        document_symbol.len(),
        document_symbol
    );

    let output = lsp_stdio_response(&input).expect("stdio response");
    let (_, response_body) = output
        .split_once("\r\n\r\n")
        .expect("content-length response frame");
    let response: serde_json::Value = serde_json::from_str(response_body).expect("response json");

    assert_eq!(response["id"], 14);
    assert!(response.get("error").is_none(), "{response}");
    assert!(response["result"]
        .as_array()
        .expect("document symbols")
        .iter()
        .any(|symbol| symbol["name"] == "Draft"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_stdio_document_symbol_uses_did_change_unsaved_content() {
    let dir = temp_output_dir("lsp-did-change-symbol");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("unsaved.orv");
    let uri = format!("file://{}", source.display());
    let did_open = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {
            "textDocument": {
                "uri": uri,
                "languageId": "orv",
                "version": 1,
                "text": "struct Draft { id: int }\n",
            },
        },
    })
    .to_string();
    let did_change = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didChange",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
                "version": 2,
            },
            "contentChanges": [
                { "text": "struct Changed { id: int }\n" }
            ],
        },
    })
    .to_string();
    let document_symbol = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 15,
        "method": "textDocument/documentSymbol",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
        },
    })
    .to_string();
    let input = format!(
        "Content-Length: {}\r\n\r\n{}Content-Length: {}\r\n\r\n{}Content-Length: {}\r\n\r\n{}",
        did_open.len(),
        did_open,
        did_change.len(),
        did_change,
        document_symbol.len(),
        document_symbol
    );

    let output = lsp_stdio_response(&input).expect("stdio response");
    let (_, response_body) = output
        .split_once("\r\n\r\n")
        .expect("content-length response frame");
    let response: serde_json::Value = serde_json::from_str(response_body).expect("response json");
    let symbols = response["result"].as_array().expect("document symbols");

    assert_eq!(response["id"], 15);
    assert!(response.get("error").is_none(), "{response}");
    assert!(symbols.iter().any(|symbol| symbol["name"] == "Changed"));
    assert!(!symbols.iter().any(|symbol| symbol["name"] == "Draft"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_did_close_drops_unsaved_content() {
    let dir = temp_output_dir("lsp-did-close-symbol");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "struct Disk { id: int }\n").expect("write source");
    let uri = format!("file://{}", source.display());
    let mut session = LspSession::default();
    session.handle_notification(&serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {
            "textDocument": {
                "uri": uri,
                "languageId": "orv",
                "version": 1,
                "text": "struct Draft { id: int }\n",
            },
        },
    }));
    session.handle_notification(&serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didClose",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
        },
    }));

    let response = session.jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 16,
        "method": "textDocument/documentSymbol",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
        },
    }));
    let symbols = response["result"].as_array().expect("document symbols");

    assert_eq!(response["id"], 16);
    assert!(response.get("error").is_none(), "{response}");
    assert!(symbols.iter().any(|symbol| symbol["name"] == "Disk"));
    assert!(!symbols.iter().any(|symbol| symbol["name"] == "Draft"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_did_save_with_text_updates_unsaved_content() {
    let dir = temp_output_dir("lsp-did-save-text-symbol");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "struct Disk { id: int }\n").expect("write source");
    let uri = format!("file://{}", source.display());
    let mut session = LspSession::default();
    session.handle_notification(&serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {
            "textDocument": {
                "uri": uri,
                "languageId": "orv",
                "version": 1,
                "text": "struct Draft { id: int }\n",
            },
        },
    }));
    session.handle_notification(&serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didSave",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
            "text": "struct Saved { id: int }\n",
        },
    }));

    let response = session.jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 17,
        "method": "textDocument/documentSymbol",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
        },
    }));
    let symbols = response["result"].as_array().expect("document symbols");

    assert_eq!(response["id"], 17);
    assert!(response.get("error").is_none(), "{response}");
    assert!(symbols.iter().any(|symbol| symbol["name"] == "Saved"));
    assert!(!symbols.iter().any(|symbol| symbol["name"] == "Draft"));
    assert!(!symbols.iter().any(|symbol| symbol["name"] == "Disk"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_did_save_without_text_returns_to_disk_content() {
    let dir = temp_output_dir("lsp-did-save-no-text-symbol");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(&source, "struct Disk { id: int }\n").expect("write source");
    let uri = format!("file://{}", source.display());
    let mut session = LspSession::default();
    session.handle_notification(&serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didOpen",
        "params": {
            "textDocument": {
                "uri": uri,
                "languageId": "orv",
                "version": 1,
                "text": "struct Draft { id: int }\n",
            },
        },
    }));
    session.handle_notification(&serde_json::json!({
        "jsonrpc": "2.0",
        "method": "textDocument/didSave",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
        },
    }));

    let response = session.jsonrpc_response(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 18,
        "method": "textDocument/documentSymbol",
        "params": {
            "textDocument": {
                "uri": format!("file://{}", source.display()),
            },
        },
    }));
    let symbols = response["result"].as_array().expect("document symbols");

    assert_eq!(response["id"], 18);
    assert!(response.get("error").is_none(), "{response}");
    assert!(symbols.iter().any(|symbol| symbol["name"] == "Disk"));
    assert!(!symbols.iter().any(|symbol| symbol["name"] == "Draft"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn db_apply_writes_current_schema_snapshot() {
    let dir = temp_output_dir("db-apply");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        r#"struct User {
  id: int
  email: string
}"#,
    )
    .expect("write source");
    let schema = dir.join("schema.json");

    cmd_db_apply(&source, &schema).expect("apply schema");

    let written = read_json_value(&schema).expect("read schema");
    assert_eq!(written["schema_version"], 1);
    assert_eq!(
        written["structs"]["User"]["fields"]["email"]["type"],
        "string"
    );
    let plan = db_plan_json(&source, Some(&schema)).expect("db plan after apply");
    assert_eq!(plan["actions"].as_array().expect("actions").len(), 0);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn db_apply_appends_migration_history_when_requested() {
    let dir = temp_output_dir("db-history");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let first_source = dir.join("first.orv");
    std::fs::write(
        &first_source,
        r#"struct User {
  id: int
  email: string
}"#,
    )
    .expect("write first source");
    let second_source = dir.join("second.orv");
    std::fs::write(
        &second_source,
        r#"struct User {
  id: int
  email: string
  avatar: string?
}"#,
    )
    .expect("write second source");
    let schema = dir.join("schema.json");
    let history = dir.join("history.json");

    cmd_db_apply_with_history(&first_source, &schema, Some(&history)).expect("apply first schema");
    cmd_db_apply_with_history(&second_source, &schema, Some(&history))
        .expect("apply second schema");

    let history = read_json_value(&history).expect("read history");
    assert_eq!(history["schema_version"], 1);
    let entries = history["entries"].as_array().expect("history entries");
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["actions"].as_array().expect("actions").len(), 1);
    assert!(entries[1]["actions"]
        .as_array()
        .expect("actions")
        .iter()
        .any(|action| action["kind"] == "add_field" && action["field"] == "avatar"));
    assert_ne!(entries[0]["schema_hash"], entries[1]["schema_hash"]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn db_migrate_applies_schema_and_history() {
    let dir = temp_output_dir("db-migrate");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        r#"struct Order {
  id: int
  total: int
}"#,
    )
    .expect("write source");
    let schema = dir.join("schema.json");
    let history = dir.join("history.json");

    cmd_db_migrate(&source, &schema, Some(&history)).expect("migrate schema");

    let written = read_json_value(&schema).expect("read schema");
    assert_eq!(
        written["structs"]["Order"]["fields"]["total"]["type"],
        "int"
    );
    let history = read_json_value(&history).expect("read history");
    assert_eq!(
        history["entries"]
            .as_array()
            .expect("history entries")
            .len(),
        1
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn db_rollback_restores_previous_schema_snapshot() {
    let dir = temp_output_dir("db-rollback");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let original_source = dir.join("original.orv");
    std::fs::write(
        &original_source,
        r#"struct User {
  id: int
  email: string
}"#,
    )
    .expect("write original source");
    let changed_source = dir.join("changed.orv");
    std::fs::write(
        &changed_source,
        r#"struct User {
  id: int
  email: string
  avatar: string?
}"#,
    )
    .expect("write changed source");
    let schema = dir.join("schema.json");

    cmd_db_apply(&original_source, &schema).expect("apply original schema");
    cmd_db_apply(&changed_source, &schema).expect("apply changed schema");
    assert!(
        read_json_value(&schema).expect("read changed schema")["structs"]["User"]["fields"]
            .as_object()
            .expect("fields")
            .contains_key("avatar")
    );

    cmd_db_rollback(&schema).expect("rollback schema");

    let restored = read_json_value(&schema).expect("read restored schema");
    assert!(!restored["structs"]["User"]["fields"]
        .as_object()
        .expect("fields")
        .contains_key("avatar"));
    let plan = db_plan_json(&original_source, Some(&schema)).expect("plan after rollback");
    assert_eq!(plan["actions"].as_array().expect("actions").len(), 0);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn db_verify_accepts_current_schema_snapshot() {
    let dir = temp_output_dir("db-verify-current");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        r#"struct User {
  id: int
  email: string
}"#,
    )
    .expect("write source");
    let schema = dir.join("schema.json");

    cmd_db_apply(&source, &schema).expect("apply schema");

    cmd_db_verify(&source, &schema).expect("verify current schema");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn db_verify_subcommand_is_accepted() {
    let parsed = Cli::try_parse_from([
        "orv",
        "db",
        "verify",
        "fixtures/e2e/hello.orv",
        "--schema",
        "target/schema.json",
    ]);
    if let Err(err) = parsed {
        panic!("{}", err.render());
    }
}

#[test]
fn db_verify_rejects_schema_drift() {
    let dir = temp_output_dir("db-verify-drift");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let original = dir.join("original.orv");
    std::fs::write(
        &original,
        r#"struct User {
  id: int
  email: string
}"#,
    )
    .expect("write original");
    let changed = dir.join("changed.orv");
    std::fs::write(
        &changed,
        r#"struct User {
  id: int
  email: string
  avatar: string?
}"#,
    )
    .expect("write changed");
    let schema = dir.join("schema.json");

    cmd_db_apply(&original, &schema).expect("apply schema");

    let err = cmd_db_verify(&changed, &schema).expect_err("schema drift");
    assert!(
        err.to_string().contains("db schema drift: 1 action(s)"),
        "{err}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn db_squash_writes_compacted_history_actions() {
    let dir = temp_output_dir("db-squash");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let first_source = dir.join("first.orv");
    std::fs::write(
        &first_source,
        r#"struct User {
  id: int
  email: string
}"#,
    )
    .expect("write first");
    let second_source = dir.join("second.orv");
    std::fs::write(
        &second_source,
        r#"struct User {
  id: int
  email: string
  avatar: string?
}"#,
    )
    .expect("write second");
    let schema = dir.join("schema.json");
    let history = dir.join("history.json");
    let squashed = dir.join("squashed.json");

    cmd_db_apply_with_history(&first_source, &schema, Some(&history)).expect("apply first schema");
    cmd_db_apply_with_history(&second_source, &schema, Some(&history))
        .expect("apply second schema");

    cmd_db_squash(&history, &squashed).expect("squash history");

    let value = read_json_value(&squashed).expect("read squashed");
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["entries"], 2);
    assert!(value["schema_hash"].as_str().expect("schema hash").len() >= 16);
    assert!(value["actions"]
        .as_array()
        .expect("actions")
        .iter()
        .any(|action| action["kind"] == "add_field" && action["field"] == "avatar"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn db_squash_subcommand_is_accepted() {
    let parsed = Cli::try_parse_from([
        "orv",
        "db",
        "squash",
        "--history",
        "target/history.json",
        "--out",
        "target/squashed.json",
    ]);
    if let Err(err) = parsed {
        panic!("{}", err.render());
    }
}

#[test]
fn db_recover_archive_subcommand_is_accepted() {
    let parsed = Cli::try_parse_from([
        "orv",
        "db",
        "recover",
        "--archive",
        "target/archive.json",
        "--out",
        "target/data.json",
        "--until-record",
        "1",
    ]);
    if let Err(err) = parsed {
        panic!("{}", err.render());
    }
}

#[test]
fn db_restore_wal_subcommand_is_accepted() {
    let parsed = Cli::try_parse_from([
        "orv",
        "db",
        "restore",
        "--wal",
        "target/db.wal.jsonl",
        "--data",
        "target/data.json",
        "--at",
        "2023-11-14T22:13:20Z",
    ]);
    if let Err(err) = parsed {
        panic!("{}", err.render());
    }
}

#[test]
fn db_restore_raw_wal_replays_point_in_time_snapshot() {
    let dir = temp_output_dir("db-restore-raw-wal");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let wal = dir.join("db.wal.jsonl");
    let data = dir.join("data.json");
    std::fs::write(
            &wal,
            concat!(
                "{\"schema_version\":1,\"op\":\"create\",\"table\":\"users\",\"data\":{\"name\":\"Ada\"},\"ts_unix_ms\":1700000000000}\n",
                "{\"schema_version\":1,\"op\":\"create\",\"table\":\"users\",\"data\":{\"name\":\"Grace\"},\"ts_unix_ms\":1700000001000}\n",
            ),
        )
        .expect("write wal");
    std::fs::write(
        &data,
        serde_json::json!({
            "schema_version": 1,
            "tables": {
                "users": {
                    "next_id": 1,
                    "rows": [{ "id": 1, "name": "stale" }]
                }
            }
        })
        .to_string(),
    )
    .expect("write stale data");

    cmd_db_restore_from_inputs(None, Some(&wal), None, Some("2023-11-14T22:13:20Z"), &data)
        .expect("restore raw wal");

    let snapshot = read_json_value(&data).expect("read restored data");
    let rows = snapshot["tables"]["users"]["rows"]
        .as_array()
        .expect("users rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "Ada");
    let rollback = read_json_value(&rollback_schema_path(&data)).expect("read rollback");
    assert_eq!(rollback["tables"]["users"]["rows"][0]["name"], "stale");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn db_recover_archive_rejects_wal_hash_mismatch() {
    let dir = temp_output_dir("db-recover-archive-hash");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let wal = dir.join("db.wal.jsonl");
    let archive = dir.join("archive.json");
    let target_dir = dir.join("archive-target");
    let out = dir.join("data.json");
    let mut db = orv_runtime::db::InMemoryDb::load_wal(&wal).expect("open wal");
    db.create_logged(
        "users",
        vec![(
            "name".to_string(),
            orv_runtime::Value::Str("Ada".to_string()),
        )],
    )
    .expect("create user");
    cmd_db_archive(
        &wal,
        &archive,
        Some(&format!("file://{}", target_dir.display())),
    )
    .expect("archive wal");
    let archived_wal = db_archive_manifest_wal_path(&archive).expect("archive wal path");
    let tampered = std::fs::read_to_string(&archived_wal)
        .expect("read archived wal")
        .replace("Ada", "Eve");
    std::fs::write(&archived_wal, tampered).expect("tamper archived wal");

    let err = cmd_db_recover_from_inputs(None, Some(&archive), &out, None, None, None)
        .expect_err("tampered archive recover");

    assert!(err.to_string().contains("db archive WAL hash mismatch"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn db_recover_archive_uses_archived_wal_target() {
    let dir = temp_output_dir("db-recover-archive-target");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let wal = dir.join("db.wal.jsonl");
    let archive = dir.join("archive.json");
    let target_dir = dir.join("archive-target");
    let out = dir.join("data.json");
    let mut db = orv_runtime::db::InMemoryDb::load_wal(&wal).expect("open wal");
    db.create_logged(
        "users",
        vec![(
            "name".to_string(),
            orv_runtime::Value::Str("Ada".to_string()),
        )],
    )
    .expect("create first user");
    db.create_logged(
        "users",
        vec![(
            "name".to_string(),
            orv_runtime::Value::Str("Grace".to_string()),
        )],
    )
    .expect("create second user");
    cmd_db_archive(
        &wal,
        &archive,
        Some(&format!("file://{}", target_dir.display())),
    )
    .expect("archive wal");
    std::fs::remove_file(&wal).expect("remove original wal");

    cmd_db_recover_from_inputs(None, Some(&archive), &out, Some(1), None, None)
        .expect("recover from archive");

    let snapshot = read_json_value(&out).expect("snapshot");
    let rows = snapshot["tables"]["users"]["rows"]
        .as_array()
        .expect("users rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["name"], "Ada");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn verify_artifact_subcommand_is_accepted() {
    let parsed = Cli::try_parse_from([
        "orv",
        "verify-artifact",
        "target/orv-build-test/server/app.orv-runtime.json",
    ]);
    if let Err(err) = parsed {
        panic!("{}", err.render());
    }
}

#[test]
fn check_artifact_subcommand_is_accepted() {
    let parsed = Cli::try_parse_from([
        "orv",
        "check-artifact",
        "target/orv-build-test/server/app.orv-runtime.json",
    ]);
    if let Err(err) = parsed {
        panic!("{}", err.render());
    }
}

#[test]
fn check_build_subcommand_is_accepted() {
    let parsed = Cli::try_parse_from(["orv", "check-build", "target/orv-build-test"]);
    if let Err(err) = parsed {
        panic!("{}", err.render());
    }
}

#[test]
fn run_artifact_subcommand_is_accepted() {
    let parsed = Cli::try_parse_from([
        "orv",
        "run-artifact",
        "target/orv-build-test/server/app.orv-runtime.json",
    ]);
    if let Err(err) = parsed {
        panic!("{}", err.render());
    }
}

#[test]
fn run_artifact_trace_option_is_accepted() {
    let parsed = Cli::try_parse_from([
        "orv",
        "run-artifact",
        "target/orv-build-test/server/app.orv-runtime.json",
        "--trace",
        "target/orv-request-trace.json",
    ])
    .unwrap_or_else(|err| panic!("{}", err.render()));
    let Command::RunArtifact { trace, .. } = parsed.command else {
        panic!("expected run-artifact command");
    };
    assert_eq!(trace, Some(PathBuf::from("target/orv-request-trace.json")));
}

#[test]
fn run_build_subcommand_is_accepted() {
    let parsed = Cli::try_parse_from(["orv", "run-build", "target/orv-build-test"]);
    if let Err(err) = parsed {
        panic!("{}", err.render());
    }
}

#[test]
fn run_build_trace_option_is_accepted() {
    let parsed = Cli::try_parse_from([
        "orv",
        "run-build",
        "target/orv-build-test",
        "--trace",
        "target/orv-request-trace.json",
    ])
    .unwrap_or_else(|err| panic!("{}", err.render()));
    let Command::RunBuild { trace, .. } = parsed.command else {
        panic!("expected run-build command");
    };
    assert_eq!(trace, Some(PathBuf::from("target/orv-request-trace.json")));
}

#[test]
fn dev_subcommand_is_accepted() {
    let parsed =
        Cli::try_parse_from(["orv", "dev", "src/main.orv", "--out", "target/orv-dev-test"]);
    if let Err(err) = parsed {
        panic!("{}", err.render());
    }
}

#[test]
fn dev_hmr_subcommand_is_accepted() {
    let parsed = Cli::try_parse_from(["orv", "dev", "src/main.orv", "--hmr"]);
    if let Err(err) = parsed {
        panic!("{}", err.render());
    }
}

#[test]
fn dev_watch_subcommand_is_accepted() {
    let parsed = Cli::try_parse_from(["orv", "dev", "src/main.orv", "--watch"]);
    if let Err(err) = parsed {
        panic!("{}", err.render());
    }
}

#[test]
fn dev_watch_loop_subcommand_is_accepted() {
    let parsed = Cli::try_parse_from([
        "orv",
        "dev",
        "src/main.orv",
        "--watch-loop",
        "--watch-iterations",
        "1",
    ]);
    if let Err(err) = parsed {
        panic!("{}", err.render());
    }
}

#[test]
fn dev_hmr_serve_subcommand_is_accepted() {
    let parsed = Cli::try_parse_from([
        "orv",
        "dev",
        "src/main.orv",
        "--hmr",
        "--serve",
        "--serve-port",
        "0",
        "--watch-iterations",
        "1",
    ])
    .unwrap_or_else(|err| panic!("{}", err.render()));
    let Command::Dev {
        serve, serve_port, ..
    } = parsed.command
    else {
        panic!("expected dev command");
    };
    assert!(serve);
    assert_eq!(serve_port, 0);
}

#[test]
fn lock_subcommand_is_accepted() {
    let parsed = Cli::try_parse_from(["orv", "lock", "demo", "--check"])
        .unwrap_or_else(|err| panic!("{}", err.render()));
    let Command::Lock { dir, check } = parsed.command else {
        panic!("expected lock command");
    };
    assert_eq!(dir, PathBuf::from("demo"));
    assert!(check);
}

#[test]
fn fetch_subcommand_is_accepted() {
    let parsed = Cli::try_parse_from(["orv", "fetch", "demo", "--out", "target/orv-deps"])
        .unwrap_or_else(|err| panic!("{}", err.render()));
    let Command::Fetch { dir, out } = parsed.command else {
        panic!("expected fetch command");
    };
    assert_eq!(dir, PathBuf::from("demo"));
    assert_eq!(out, PathBuf::from("target/orv-deps"));
}

#[test]
fn add_and_remove_subcommands_are_accepted() {
    let parsed = Cli::try_parse_from([
        "orv",
        "add",
        "auth",
        "1.2.3",
        "--manifest",
        "demo",
        "--dev",
        "--registry",
        "https://registry.orv.dev",
    ])
    .unwrap_or_else(|err| panic!("{}", err.render()));
    let Command::Add {
        pkg,
        version,
        manifest,
        dev,
        path,
        registry,
    } = parsed.command
    else {
        panic!("expected add command");
    };
    assert_eq!(pkg, "auth");
    assert_eq!(version.as_deref(), Some("1.2.3"));
    assert_eq!(manifest, PathBuf::from("demo"));
    assert!(dev);
    assert!(path.is_none());
    assert_eq!(registry.as_deref(), Some("https://registry.orv.dev"));

    let parsed = Cli::try_parse_from(["orv", "remove", "auth", "--manifest", "demo"])
        .unwrap_or_else(|err| panic!("{}", err.render()));
    let Command::Remove { pkg, manifest, dev } = parsed.command else {
        panic!("expected remove command");
    };
    assert_eq!(pkg, "auth");
    assert_eq!(manifest, PathBuf::from("demo"));
    assert!(!dev);
}

#[test]
fn workspace_new_subcommand_is_accepted() {
    let parsed = Cli::try_parse_from([
        "orv",
        "workspace",
        "new",
        "apps/web",
        "--root",
        "demo",
        "--name",
        "web",
    ])
    .unwrap_or_else(|err| panic!("{}", err.render()));
    let Command::Workspace { command } = parsed.command else {
        panic!("expected workspace command");
    };
    let WorkspaceCommand::New {
        member,
        root,
        name,
        template,
    } = command
    else {
        panic!("expected workspace new command");
    };
    assert_eq!(member, PathBuf::from("apps/web"));
    assert_eq!(root, PathBuf::from("demo"));
    assert_eq!(name.as_deref(), Some("web"));
    assert_eq!(template, InitTemplate::Basic);
}

#[test]
fn workspace_graph_subcommand_is_accepted() {
    let parsed = Cli::try_parse_from([
        "orv",
        "workspace",
        "graph",
        "demo",
        "--view",
        "--out",
        "target/orv-workspace-view",
    ])
    .unwrap_or_else(|err| panic!("{}", err.render()));
    let Command::Workspace { command } = parsed.command else {
        panic!("expected workspace command");
    };
    let WorkspaceCommand::Graph { root, view, out } = command else {
        panic!("expected workspace graph command");
    };
    assert_eq!(root, PathBuf::from("demo"));
    assert!(view);
    assert_eq!(out, Some(PathBuf::from("target/orv-workspace-view")));
}

#[test]
fn workspace_lock_subcommand_is_accepted() {
    let parsed = Cli::try_parse_from([
        "orv",
        "workspace",
        "lock",
        "demo",
        "--out",
        "target/orv-workspace-lock",
    ])
    .unwrap_or_else(|err| panic!("{}", err.render()));
    let Command::Workspace { command } = parsed.command else {
        panic!("expected workspace command");
    };
    let WorkspaceCommand::Lock { root, out } = command else {
        panic!("expected workspace lock command");
    };
    assert_eq!(root, PathBuf::from("demo"));
    assert_eq!(out, PathBuf::from("target/orv-workspace-lock"));
}

#[test]
fn workspace_fetch_subcommand_is_accepted() {
    let parsed = Cli::try_parse_from([
        "orv",
        "workspace",
        "fetch",
        "demo",
        "--out",
        "target/orv-workspace-deps",
    ])
    .unwrap_or_else(|err| panic!("{}", err.render()));
    let Command::Workspace { command } = parsed.command else {
        panic!("expected workspace command");
    };
    let WorkspaceCommand::Fetch { root, out } = command else {
        panic!("expected workspace fetch command");
    };
    assert_eq!(root, PathBuf::from("demo"));
    assert_eq!(out, PathBuf::from("target/orv-workspace-deps"));
}

#[test]
fn workspace_build_subcommand_is_accepted() {
    let parsed = Cli::try_parse_from([
        "orv",
        "workspace",
        "build",
        "demo",
        "--out",
        "target/orv-workspace-build",
        "--prod",
        "--incremental",
    ])
    .unwrap_or_else(|err| panic!("{}", err.render()));
    let Command::Workspace { command } = parsed.command else {
        panic!("expected workspace command");
    };
    let WorkspaceCommand::Build {
        root,
        out,
        prod,
        incremental,
    } = command
    else {
        panic!("expected workspace build command");
    };
    assert_eq!(root, PathBuf::from("demo"));
    assert_eq!(out, PathBuf::from("target/orv-workspace-build"));
    assert!(prod);
    assert!(incremental);
}

#[test]
fn reveal_subcommand_is_accepted() {
    let parsed = Cli::try_parse_from([
        "orv",
        "reveal",
        "target/orv-build-test",
        "route:GET_/ping:abc123",
    ]);
    if let Err(err) = parsed {
        panic!("{}", err.render());
    }
}

#[test]
fn editor_snapshot_subcommand_is_accepted() {
    let parsed = Cli::try_parse_from(["orv", "editor", "snapshot", "src/main.orv"]);
    if let Err(err) = parsed {
        panic!("{}", err.render());
    }
}

#[test]
fn editor_reveal_subcommand_is_accepted() {
    let parsed = Cli::try_parse_from(["orv", "editor", "reveal", "dist", "ori_1"]);
    if let Err(err) = parsed {
        panic!("{}", err.render());
    }
}

#[test]
fn editor_runtime_subcommand_is_accepted() {
    let parsed = Cli::try_parse_from(["orv", "editor", "runtime", "src/main.orv"]);
    if let Err(err) = parsed {
        panic!("{}", err.render());
    }
}

#[test]
fn editor_export_subcommand_is_accepted() {
    let parsed = Cli::try_parse_from([
        "orv",
        "editor",
        "export",
        "src/main.orv",
        "--out",
        "target/orv-editor",
    ]);
    if let Err(err) = parsed {
        panic!("{}", err.render());
    }
}

#[test]
fn editor_export_trace_options_are_accepted() {
    let parsed = Cli::try_parse_from([
        "orv",
        "editor",
        "export",
        "src/main.orv",
        "--out",
        "target/orv-editor",
        "--build",
        "target/orv-build",
        "--trace",
        "target/orv-trace.json",
    ]);
    if let Err(err) = parsed {
        panic!("{}", err.render());
    }
}

#[test]
fn editor_debug_subcommand_is_accepted() {
    let parsed = Cli::try_parse_from([
        "orv",
        "editor",
        "debug",
        "fixtures/e2e/hello.orv",
        "--control",
        "next",
    ]);
    if let Err(err) = parsed {
        panic!("{}", err.render());
    }
}

#[test]
fn editor_debug_subcommand_accepts_control_sequence() {
    let parsed = Cli::try_parse_from([
        "orv",
        "editor",
        "debug",
        "fixtures/e2e/hello.orv",
        "--control",
        "next",
        "--control",
        "next",
    ]);
    if let Err(err) = parsed {
        panic!("{}", err.render());
    }
}

#[test]
fn editor_debug_subcommand_accepts_watch_expression() {
    let parsed = Cli::try_parse_from([
        "orv",
        "editor",
        "debug",
        "fixtures/e2e/hello.orv",
        "--watch-expression",
        "runtimeStatus",
    ])
    .unwrap_or_else(|err| panic!("{}", err.render()));
    let Command::Editor {
        command: EditorCommand::Debug {
            watch_expressions, ..
        },
    } = parsed.command
    else {
        panic!("expected editor debug command");
    };
    assert_eq!(watch_expressions, vec!["runtimeStatus".to_string()]);
}

#[test]
fn editor_debug_subcommand_accepts_function_breakpoint() {
    let parsed = Cli::try_parse_from([
        "orv",
        "editor",
        "debug",
        "fixtures/e2e/hello.orv",
        "--function-breakpoint",
        "add",
    ])
    .unwrap_or_else(|err| panic!("{}", err.render()));
    let Command::Editor {
        command: EditorCommand::Debug {
            function_breakpoints,
            ..
        },
    } = parsed.command
    else {
        panic!("expected editor debug command");
    };
    assert_eq!(function_breakpoints, vec!["add".to_string()]);
}

#[test]
fn editor_debug_subcommand_accepts_data_breakpoint() {
    let parsed = Cli::try_parse_from([
        "orv",
        "editor",
        "debug",
        "fixtures/e2e/hello.orv",
        "--data-breakpoint",
        "total",
    ])
    .unwrap_or_else(|err| panic!("{}", err.render()));
    let Command::Editor {
        command: EditorCommand::Debug {
            data_breakpoints, ..
        },
    } = parsed.command
    else {
        panic!("expected editor debug command");
    };
    assert_eq!(data_breakpoints, vec!["total".to_string()]);
}

#[test]
fn editor_debug_subcommand_accepts_exception_filter() {
    let parsed = Cli::try_parse_from([
        "orv",
        "editor",
        "debug",
        "fixtures/e2e/hello.orv",
        "--exception-filter",
        "orv.runtime",
    ])
    .unwrap_or_else(|err| panic!("{}", err.render()));
    let Command::Editor {
        command: EditorCommand::Debug {
            exception_filters, ..
        },
    } = parsed.command
    else {
        panic!("expected editor debug command");
    };
    assert_eq!(exception_filters, vec!["orv.runtime".to_string()]);
}

#[test]
fn editor_run_debug_subcommand_accepts_exported_state() {
    let parsed = Cli::try_parse_from([
        "orv",
        "editor",
        "run-debug",
        "target/orv-editor/state.json",
        "--control",
        "next",
        "--control",
        "step-in",
        "--watch-expression",
        "stdout",
    ]);
    if let Err(err) = parsed {
        panic!("{}", err.render());
    }
}

#[test]
fn editor_trace_subcommand_is_accepted() {
    let parsed = Cli::try_parse_from([
        "orv",
        "editor",
        "trace",
        "target/orv-build",
        "--trace",
        "target/orv-trace.json",
    ]);
    if let Err(err) = parsed {
        panic!("{}", err.render());
    }
}

#[test]
fn editor_trace_stream_subcommand_accepts_event_stream_snapshot() {
    let parsed = Cli::try_parse_from([
        "orv",
        "editor",
        "trace-stream",
        "target/orv-build",
        "--events",
        "target/orv-build/trace-events.sse",
    ]);
    if let Err(err) = parsed {
        panic!("{}", err.render());
    }
}

#[test]
fn verify_build_subcommand_is_accepted() {
    let parsed = Cli::try_parse_from(["orv", "verify-build", "target/orv-build-test"]);
    if let Err(err) = parsed {
        panic!("{}", err.render());
    }
}

#[test]
fn deploy_env_check_subcommand_is_accepted() {
    let parsed = Cli::try_parse_from(["orv", "deploy-env-check", "target/orv-build-test"]);
    if let Err(err) = parsed {
        panic!("{}", err.render());
    }
}

#[test]
fn build_prod_subcommand_flag_is_accepted() {
    let parsed = Cli::try_parse_from([
        "orv",
        "build",
        "fixtures/e2e/hello.orv",
        "--out",
        "target/orv-prod-build-test",
        "--prod",
    ]);
    if let Err(err) = parsed {
        panic!("{}", err.render());
    }
}

#[test]
fn build_writes_manifest_origin_map_and_project_graph() {
    let path = workspace_path(&["fixtures", "e2e", "hello.orv"]);
    let out = temp_output_dir("build-artifacts");

    cmd_build(&path, &out).expect("build artifacts");

    let manifest_path = out.join("build-manifest.json");
    let origin_map_path = out.join("origin-map.json");
    let bundle_plan_path = out.join("bundle-plan.json");
    let server_artifact_path = out.join("server").join("app.orv-runtime.json");
    let server_launch_path = out.join("server").join("launch.json");
    let native_server_plan_path = out.join("server").join("native-server.json");
    let native_server_package_path = out.join("server").join("native").join("Cargo.toml");
    let native_server_source_path = out.join("server").join("native").join("main.rs");
    let native_server_routes_path = out.join("server").join("native").join("routes.rs");
    let native_server_router_path = out.join("server").join("native").join("router.rs");
    let native_server_handlers_path = out.join("server").join("native").join("handlers.rs");
    let graph_path = out.join("project-graph.json");
    let source_bundle_path = out.join("source-bundle.json");
    assert!(
        manifest_path.is_file(),
        "missing {}",
        manifest_path.display()
    );
    assert!(
        origin_map_path.is_file(),
        "missing {}",
        origin_map_path.display()
    );
    assert!(
        bundle_plan_path.is_file(),
        "missing {}",
        bundle_plan_path.display()
    );
    assert!(
        server_artifact_path.is_file(),
        "missing {}",
        server_artifact_path.display()
    );
    assert!(
        server_launch_path.is_file(),
        "missing {}",
        server_launch_path.display()
    );
    assert!(
        native_server_plan_path.is_file(),
        "missing {}",
        native_server_plan_path.display()
    );
    assert!(
        native_server_source_path.is_file(),
        "missing {}",
        native_server_source_path.display()
    );
    assert!(
        native_server_routes_path.is_file(),
        "missing {}",
        native_server_routes_path.display()
    );
    assert!(
        native_server_router_path.is_file(),
        "missing {}",
        native_server_router_path.display()
    );
    assert!(
        native_server_handlers_path.is_file(),
        "missing {}",
        native_server_handlers_path.display()
    );
    assert!(
        native_server_package_path.is_file(),
        "missing {}",
        native_server_package_path.display()
    );
    assert!(graph_path.is_file(), "missing {}", graph_path.display());
    assert!(
        source_bundle_path.is_file(),
        "missing {}",
        source_bundle_path.display()
    );

    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest_path).expect("manifest"))
            .expect("manifest json");
    assert_eq!(manifest["schema_version"], 1);
    assert_eq!(manifest["entry"], path.display().to_string());
    assert_eq!(manifest["runtime"], "reference-interpreter");
    let runtime_features = manifest["capabilities"]["runtime_features"]
        .as_array()
        .expect("runtime features array");
    assert!(runtime_features
        .iter()
        .any(|feature| feature == "http_server"));
    assert!(runtime_features.iter().any(|feature| feature == "router"));
    assert!(manifest["artifacts"]
        .as_array()
        .expect("artifacts array")
        .iter()
        .any(|artifact| artifact["kind"] == "origin_map" && artifact["path"] == "origin-map.json"));
    assert!(manifest["artifacts"]
        .as_array()
        .expect("artifacts array")
        .iter()
        .any(
            |artifact| artifact["kind"] == "bundle_plan" && artifact["path"] == "bundle-plan.json"
        ));
    assert!(manifest["artifacts"]
        .as_array()
        .expect("artifacts array")
        .iter()
        .any(|artifact| artifact["kind"] == "project_graph"
            && artifact["path"] == "project-graph.json"));
    assert!(manifest["artifacts"]
        .as_array()
        .expect("artifacts array")
        .iter()
        .any(|artifact| artifact["kind"] == "source_bundle"
            && artifact["path"] == "source-bundle.json"));
    assert!(manifest["artifacts"]
        .as_array()
        .expect("artifacts array")
        .iter()
        .any(|artifact| artifact["kind"] == "server_runtime"
            && artifact["path"] == "server/app.orv-runtime.json"));
    assert!(manifest["artifacts"]
        .as_array()
        .expect("artifacts array")
        .iter()
        .any(|artifact| artifact["kind"] == "server_launcher"
            && artifact["path"] == "server/launch.json"));
    assert!(manifest["artifacts"]
        .as_array()
        .expect("artifacts array")
        .iter()
        .any(|artifact| artifact["kind"] == "native_server_plan"
            && artifact["path"] == "server/native-server.json"));
    assert!(manifest["artifacts"]
        .as_array()
        .expect("artifacts array")
        .iter()
        .any(
            |artifact| artifact["kind"] == "native_server_launcher_source"
                && artifact["path"] == "server/native/main.rs"
        ));
    assert!(manifest["artifacts"]
        .as_array()
        .expect("artifacts array")
        .iter()
        .any(|artifact| artifact["kind"] == "native_server_routes_source"
            && artifact["path"] == "server/native/routes.rs"));
    assert!(manifest["artifacts"]
        .as_array()
        .expect("artifacts array")
        .iter()
        .any(|artifact| artifact["kind"] == "native_server_router_source"
            && artifact["path"] == "server/native/router.rs"));
    assert!(manifest["artifacts"]
        .as_array()
        .expect("artifacts array")
        .iter()
        .any(
            |artifact| artifact["kind"] == "native_server_handlers_source"
                && artifact["path"] == "server/native/handlers.rs"
        ));
    assert!(manifest["artifacts"]
        .as_array()
        .expect("artifacts array")
        .iter()
        .any(
            |artifact| artifact["kind"] == "native_server_launcher_package"
                && artifact["path"] == "server/native/Cargo.toml"
        ));
    let source_bundle: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&source_bundle_path).expect("source bundle"))
            .expect("source bundle json");
    assert_eq!(source_bundle["schema_version"], 1);
    assert!(source_bundle["files"]
        .as_array()
        .expect("source files")
        .iter()
        .any(|file| file["source"]
            .as_str()
            .is_some_and(|source| source.contains("@route GET /ping"))));
    let plan: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&bundle_plan_path).expect("plan"))
            .expect("bundle plan json");
    assert_eq!(plan["schema_version"], 1);
    assert!(plan["bundles"]
        .as_array()
        .expect("bundles array")
        .iter()
        .any(|bundle| bundle["kind"] == "server_runtime"
            && bundle["path"] == "server/app.orv-runtime.json"));
    assert!(plan["bundles"]
        .as_array()
        .expect("bundles array")
        .iter()
        .any(
            |bundle| bundle["kind"] == "server_launcher" && bundle["path"] == "server/launch.json"
        ));
    assert!(plan["bundles"]
        .as_array()
        .expect("bundles array")
        .iter()
        .any(|bundle| bundle["kind"] == "native_server_plan"
            && bundle["path"] == "server/native-server.json"));
    assert!(plan["bundles"]
        .as_array()
        .expect("bundles array")
        .iter()
        .any(|bundle| bundle["kind"] == "native_server_launcher_source"
            && bundle["path"] == "server/native/main.rs"));
    assert!(plan["bundles"]
        .as_array()
        .expect("bundles array")
        .iter()
        .any(|bundle| bundle["kind"] == "native_server_routes_source"
            && bundle["path"] == "server/native/routes.rs"));
    assert!(plan["bundles"]
        .as_array()
        .expect("bundles array")
        .iter()
        .any(|bundle| bundle["kind"] == "native_server_router_source"
            && bundle["path"] == "server/native/router.rs"));
    assert!(plan["bundles"]
        .as_array()
        .expect("bundles array")
        .iter()
        .any(|bundle| bundle["kind"] == "native_server_handlers_source"
            && bundle["path"] == "server/native/handlers.rs"));
    assert!(plan["bundles"]
        .as_array()
        .expect("bundles array")
        .iter()
        .any(|bundle| bundle["kind"] == "native_server_launcher_package"
            && bundle["path"] == "server/native/Cargo.toml"));
    let server_artifact: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&server_artifact_path).expect("server artifact"),
    )
    .expect("server artifact json");
    assert_eq!(server_artifact["schema_version"], 1);
    assert_eq!(server_artifact["runtime"], "reference-interpreter");
    assert_eq!(server_artifact["listen"]["port"], 0);
    assert!(server_artifact["listen"]["origin_id"]
        .as_str()
        .is_some_and(|origin| origin.starts_with("ori_")));
    assert!(server_artifact["routes"]
        .as_array()
        .expect("routes array")
        .iter()
        .any(|route| route["method"] == "GET" && route["path"] == "/ping"));
    assert!(server_artifact["routes"][0]["response_origin_ids"]
        .as_array()
        .expect("route response origins")
        .iter()
        .any(|origin| origin
            .as_str()
            .is_some_and(|origin| origin.starts_with("ori_"))));
    assert!(server_artifact["source_bundle"]["files"]
        .as_array()
        .expect("source bundle files")
        .iter()
        .any(|file| file["source"]
            .as_str()
            .is_some_and(|source| source.contains("@route GET /ping"))
            && file["content_hash"]
                .as_str()
                .is_some_and(|hash| hash.starts_with("fnv1a64:"))));
    let launch: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&server_launch_path).expect("server launch artifact"),
    )
    .expect("server launch json");
    assert_eq!(launch["schema_version"], 1);
    assert_eq!(launch["runtime"], "reference-interpreter");
    assert_eq!(launch["artifact"], "server/app.orv-runtime.json");
    assert_eq!(launch["protocol"], "http1");
    assert_eq!(launch["listen"], server_artifact["listen"]);
    assert_eq!(launch["command"][0], "orv");
    assert_eq!(launch["command"][1], "run-artifact");
    assert_eq!(launch["command"][2], "server/app.orv-runtime.json");
    assert!(launch["routes"]
        .as_array()
        .expect("launch routes")
        .iter()
        .any(|route| route["method"] == "GET" && route["path"] == "/ping"));
    let native_plan: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&native_server_plan_path).expect("native server plan"),
    )
    .expect("native server plan json");
    assert_eq!(native_plan["schema_version"], 1);
    assert_eq!(native_plan["kind"], "native_server_plan");
    assert_eq!(native_plan["status"], "direct_http");
    assert_eq!(native_plan["artifact"], "server/app.orv-runtime.json");
    assert_eq!(native_plan["launcher"], "server/launch.json");
    assert_eq!(native_plan["source"], "server/native/main.rs");
    assert_eq!(native_plan["routes_source"], "server/native/routes.rs");
    assert_eq!(native_plan["router_source"], "server/native/router.rs");
    assert_eq!(native_plan["handlers_source"], "server/native/handlers.rs");
    assert_eq!(native_plan["package"], "server/native/Cargo.toml");
    assert_eq!(native_plan["runtime"], "reference-interpreter");
    assert_eq!(native_plan["target"]["kind"], "server_binary");
    assert_eq!(native_plan["target"]["path"], "server/app");
    assert_eq!(native_plan["target"]["protocol"], "http1");
    assert_eq!(
        native_plan["commands"]["build"],
        serde_json::json!([
            "cargo",
            "build",
            "--manifest-path",
            "server/native/Cargo.toml",
            "--release"
        ])
    );
    assert_eq!(native_plan["commands"]["run"]["env"]["ORV_BUILD_DIR"], ".");
    assert_eq!(
        native_plan["commands"]["run"]["command"],
        serde_json::json!(["./server/native/target/release/orv-native-server"])
    );
    assert_eq!(native_plan["listen"], server_artifact["listen"]);
    assert!(json_routes_include(&native_plan["routes"], "GET", "/ping"));
    assert!(!native_plan["blocked_by"]
        .as_array()
        .expect("blocked_by")
        .iter()
        .any(|item| item == "native-codegen"));
    assert!(!native_plan["blocked_by"]
        .as_array()
        .expect("blocked_by")
        .iter()
        .any(|item| item == "native-runtime-image"));
    let native_source = std::fs::read_to_string(&native_server_source_path).expect("native source");
    assert!(native_source.contains("const ORV_SERVER_ARTIFACT"));
    assert!(native_source.contains("server/app.orv-runtime.json"));
    assert!(native_source.contains("build_dir.join(ORV_NATIVE_SERVER_PLAN)"));
    assert!(native_source.contains("fn orv_build_dir() -> std::path::PathBuf"));
    assert!(native_source.contains("std::env::current_exe()"));
    assert!(native_source.contains("native_plan.is_file()"));
    assert!(native_source.contains("build_dir.join(ORV_SERVER_ARTIFACT)"));
    assert!(native_source.contains("artifact.is_file()"));
    assert!(native_source.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(native_source.contains("std::net::TcpListener::bind(orv_native_listen_address())"));
    assert!(native_source.contains("router::orv_native_dispatch_with_request("));
    assert!(native_source.contains("request.body"));
    assert!(native_source.contains("fn orv_native_http_response("));
    assert!(!native_source.contains("Command::new(\"orv\")"));
    assert!(!native_source.contains(".arg(\"run-artifact\")"));
    assert!(native_source.contains("mod routes;"));
    assert!(native_source.contains("mod router;"));
    assert!(native_source.contains("mod handlers;"));
    assert!(native_source.contains("routes::ORV_NATIVE_ROUTE_COUNT"));
    assert!(native_source
        .contains(r#"routes::orv_native_match_route("__orv_probe__", "__orv_probe__")"#));
    assert!(native_source.contains("router::ORV_NATIVE_HANDLER_COUNT"));
    assert!(
        native_source.contains(r#"router::orv_native_dispatch("__orv_probe__", "__orv_probe__")"#)
    );
    assert!(native_source.contains("handlers::ORV_NATIVE_HANDLER_COUNT"));
    let native_route_table_source =
        std::fs::read_to_string(&native_server_routes_path).expect("native routes source");
    let route_origin = server_artifact["routes"][0]["origin_id"]
        .as_str()
        .expect("route origin id");
    let response_origin = server_artifact["routes"][0]["response_origin_ids"][0]
        .as_str()
        .expect("response origin id");
    assert!(native_route_table_source.contains("pub struct OrvNativeRoute"));
    assert!(native_route_table_source.contains("pub response_origin_ids: &'static [&'static str]"));
    assert!(native_route_table_source.contains("pub const ORV_NATIVE_ROUTES"));
    assert!(native_route_table_source.contains("method: \"GET\""));
    assert!(native_route_table_source.contains("path: \"/ping\""));
    assert!(native_route_table_source.contains("pub fn orv_native_match_route("));
    assert!(native_route_table_source.contains("orv_native_route_path_params(route.path, path)"));
    assert!(native_route_table_source.contains(&format!("origin_id: \"{route_origin}\"")));
    assert!(native_route_table_source
        .contains(&format!("response_origin_ids: &[\"{response_origin}\"]")));
    assert!(native_route_table_source
        .contains("pub const ORV_NATIVE_ROUTE_COUNT: usize = ORV_NATIVE_ROUTES.len();"));
    let native_router_source_text =
        std::fs::read_to_string(&native_server_router_path).expect("native router source");
    assert!(native_router_source_text.contains("use crate::{handlers, routes};"));
    assert!(native_router_source_text.contains("pub struct OrvNativeDispatch"));
    assert!(native_router_source_text.contains("pub const ORV_NATIVE_HANDLER_COUNT"));
    assert!(native_router_source_text.contains("pub fn orv_native_dispatch("));
    assert!(native_router_source_text.contains("routes::orv_native_match_route(method, path)"));
    assert!(native_router_source_text.contains("handlers::orv_native_handle_route(&route_match)"));
    assert!(native_router_source_text.contains("pub response_origin_id: Option<&'static str>"));
    assert!(native_router_source_text.contains("response_origin_id: response.response_origin_id"));
    assert!(native_router_source_text.contains("status: 404"));
    let native_handlers_source_text =
        std::fs::read_to_string(&native_server_handlers_path).expect("native handlers source");
    assert!(native_handlers_source_text.contains("use crate::routes;"));
    assert!(native_handlers_source_text.contains("pub struct OrvNativeHandlerDescriptor"));
    assert!(native_handlers_source_text.contains("pub struct OrvNativeHandlerResponse"));
    assert!(native_handlers_source_text.contains("pub const ORV_NATIVE_HANDLERS"));
    assert!(native_handlers_source_text.contains("pub const ORV_NATIVE_HANDLER_COUNT"));
    assert!(native_handlers_source_text.contains("pub fn orv_native_handle_route("));
    assert!(native_handlers_source_text.contains(&format!("route_origin_id: \"{route_origin}\"")));
    assert!(native_handlers_source_text
        .contains(&format!("response_origin_ids: &[\"{response_origin}\"]")));
    assert!(native_handlers_source_text.contains("response_origin_id: Some("));
    assert!(native_handlers_source_text.contains("status: 200"));
    assert!(native_handlers_source_text.contains(r#"body: "{\"ok\":true,\"msg\":\"pong\"}""#));
    assert!(!native_handlers_source_text.contains("native route body lowering pending"));
    let native_package =
        std::fs::read_to_string(&native_server_package_path).expect("native package");
    assert!(native_package.contains("name = \"orv-native-server\""));
    assert!(native_package.contains("path = \"main.rs\""));

    cmd_verify_build(&out).expect("verify build artifacts");

    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn build_writes_native_runtime_image_plan_contract() {
    let path = workspace_path(&["fixtures", "e2e", "hello.orv"]);
    let out = temp_output_dir("native-runtime-image-plan");

    cmd_build(&path, &out).expect("build artifacts");

    let image_plan_path = out.join("server").join("runtime-image.json");
    assert!(
        image_plan_path.is_file(),
        "missing {}",
        image_plan_path.display()
    );
    let image_plan = read_json_value(&image_plan_path).expect("runtime image plan");
    let server_artifact =
        read_json_value(&out.join(SERVER_ARTIFACT_PATH)).expect("server artifact");
    let native_plan =
        read_json_value(&out.join(NATIVE_SERVER_PLAN_PATH)).expect("native server plan");
    assert_manifest_artifact(
        &out.join("build-manifest.json"),
        "native_runtime_image_plan",
        "server/runtime-image.json",
    );
    assert_bundle_target(
        &out.join("bundle-plan.json"),
        "native_runtime_image_plan",
        "server/runtime-image.json",
    );
    assert_manifest_artifact(
        &out.join("build-manifest.json"),
        "native_runtime_image_dockerfile",
        NATIVE_RUNTIME_IMAGE_DOCKERFILE_PATH,
    );
    assert_bundle_target(
        &out.join("bundle-plan.json"),
        "native_runtime_image_dockerfile",
        NATIVE_RUNTIME_IMAGE_DOCKERFILE_PATH,
    );
    assert_eq!(
        native_plan["runtime_image_plan"],
        "server/runtime-image.json"
    );
    assert_eq!(image_plan["kind"], "native_runtime_image_plan");
    assert_eq!(image_plan["status"], "image_planned");
    assert_eq!(image_plan["artifact"], SERVER_ARTIFACT_PATH);
    assert_eq!(image_plan["native_plan"], NATIVE_SERVER_PLAN_PATH);
    assert_eq!(image_plan["runtime"], server_artifact["runtime"]);
    assert_eq!(
        image_plan["reference_image"],
        "ghcr.io/orv-lang/orv-reference:latest"
    );
    assert_eq!(image_plan["target"]["kind"], "oci_image");
    assert_eq!(image_plan["target"]["binary"], NATIVE_SERVER_BINARY_PATH);
    assert_eq!(
        image_plan["dockerfile"],
        NATIVE_RUNTIME_IMAGE_DOCKERFILE_PATH
    );
    assert_eq!(
        image_plan["commands"]["build"],
        serde_json::json!([
            "docker",
            "build",
            "-f",
            NATIVE_RUNTIME_IMAGE_DOCKERFILE_PATH,
            "-t",
            NATIVE_RUNTIME_IMAGE_NAME,
            "."
        ])
    );
    assert_eq!(image_plan["routes"], server_artifact["routes"]);
    assert!(!image_plan["blocked_by"]
        .as_array()
        .expect("blocked_by")
        .iter()
        .any(|item| item == "native-codegen"));
    assert!(!image_plan["blocked_by"]
        .as_array()
        .expect("blocked_by")
        .iter()
        .any(|item| item == "native-runtime-image"));
    let dockerfile = std::fs::read_to_string(out.join(NATIVE_RUNTIME_IMAGE_DOCKERFILE_PATH))
        .expect("native runtime image Dockerfile");
    assert!(dockerfile.contains("FROM rust:"));
    assert!(
        dockerfile.contains("cargo build --manifest-path /work/server/native/Cargo.toml --release")
    );
    assert!(dockerfile.contains("COPY . /app"));
    assert!(dockerfile.contains(
        "COPY --from=build /work/server/native/target/release/orv-native-server /app/server/app"
    ));
    assert!(dockerfile.contains("ENV ORV_BUILD_DIR=/app"));
    assert!(dockerfile.contains("ENTRYPOINT [\"/app/server/app\"]"));

    cmd_verify_build(&out).expect("verify build artifacts");
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn build_writes_native_server_routes_source_contract() {
    let path = workspace_path(&["fixtures", "e2e", "hello.orv"]);
    let out = temp_output_dir("native-server-routes-source");

    cmd_build(&path, &out).expect("build artifacts");

    let routes_source_path = out.join("server").join("native").join("routes.rs");
    assert!(
        routes_source_path.is_file(),
        "missing {}",
        routes_source_path.display()
    );
    assert_manifest_artifact(
        &out.join("build-manifest.json"),
        "native_server_routes_source",
        "server/native/routes.rs",
    );
    assert_bundle_target(
        &out.join("bundle-plan.json"),
        "native_server_routes_source",
        "server/native/routes.rs",
    );
    let native_plan =
        read_json_value(&out.join(NATIVE_SERVER_PLAN_PATH)).expect("native server plan");
    let server_artifact =
        read_json_value(&out.join(SERVER_ARTIFACT_PATH)).expect("server artifact");
    let source = std::fs::read_to_string(&routes_source_path).expect("routes source");
    let route_origin = server_artifact["routes"][0]["origin_id"]
        .as_str()
        .expect("route origin id");
    let response_origin = server_artifact["routes"][0]["response_origin_ids"][0]
        .as_str()
        .expect("response origin id");

    assert_eq!(native_plan["routes_source"], "server/native/routes.rs");
    assert!(source.contains("pub struct OrvNativeRoute"));
    assert!(source.contains("pub response_origin_ids: &'static [&'static str]"));
    assert!(source.contains("pub policies: &'static [OrvNativeRoutePolicy]"));
    assert!(source.contains("pub struct OrvNativeRoutePolicy"));
    assert!(source.contains("pub const ORV_NATIVE_ROUTES"));
    assert!(source.contains("OrvNativeRoute {"));
    assert!(source.contains("method: \"GET\""));
    assert!(source.contains("path: \"/ping\""));
    assert!(source.contains("pub fn orv_native_match_route("));
    assert!(source.contains("orv_native_route_path_params(route.path, path)"));
    assert!(source.contains(&format!("origin_id: \"{route_origin}\"")));
    assert!(source.contains(&format!("response_origin_ids: &[\"{response_origin}\"]")));
    assert!(source.contains("policies: &[]"));
    assert!(source.contains("pub const ORV_NATIVE_ROUTE_COUNT: usize = ORV_NATIVE_ROUTES.len();"));

    cmd_verify_build(&out).expect("verify build artifacts");
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn build_writes_native_server_router_source_contract() {
    let path = workspace_path(&["fixtures", "e2e", "hello.orv"]);
    let out = temp_output_dir("native-server-router-source");

    cmd_build(&path, &out).expect("build artifacts");

    let router_source_path = out.join("server").join("native").join("router.rs");
    assert!(
        router_source_path.is_file(),
        "missing {}",
        router_source_path.display()
    );
    assert_manifest_artifact(
        &out.join("build-manifest.json"),
        "native_server_router_source",
        "server/native/router.rs",
    );
    assert_bundle_target(
        &out.join("bundle-plan.json"),
        "native_server_router_source",
        "server/native/router.rs",
    );
    let native_plan =
        read_json_value(&out.join(NATIVE_SERVER_PLAN_PATH)).expect("native server plan");
    let source = std::fs::read_to_string(&router_source_path).expect("router source");

    assert_eq!(native_plan["router_source"], "server/native/router.rs");
    assert!(source.contains("use crate::{handlers, routes};"));
    assert!(source.contains("pub struct OrvNativeDispatch"));
    assert!(source.contains("pub const ORV_NATIVE_HANDLER_COUNT"));
    assert!(source.contains("pub fn orv_native_dispatch("));
    assert!(source.contains("routes::orv_native_match_route(method, path)"));
    assert!(source.contains("handlers::orv_native_handle_route(&route_match)"));
    assert!(source.contains("origin_id: response.origin_id"));
    assert!(source.contains("response_origin_id: response.response_origin_id"));
    assert!(source.contains("params: response.params"));
    assert!(source.contains("status: 404"));

    cmd_verify_build(&out).expect("verify build artifacts");
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn build_writes_native_server_handler_source_contract() {
    let path = workspace_path(&["fixtures", "e2e", "hello.orv"]);
    let out = temp_output_dir("native-server-handler-source");

    cmd_build(&path, &out).expect("build artifacts");

    let handlers_source_path = out.join("server").join("native").join("handlers.rs");
    assert!(
        handlers_source_path.is_file(),
        "missing {}",
        handlers_source_path.display()
    );
    assert_manifest_artifact(
        &out.join("build-manifest.json"),
        "native_server_handlers_source",
        "server/native/handlers.rs",
    );
    assert_bundle_target(
        &out.join("bundle-plan.json"),
        "native_server_handlers_source",
        "server/native/handlers.rs",
    );
    let native_plan =
        read_json_value(&out.join(NATIVE_SERVER_PLAN_PATH)).expect("native server plan");
    let server_artifact =
        read_json_value(&out.join(SERVER_ARTIFACT_PATH)).expect("server artifact");
    let response_origin = server_artifact["routes"][0]["response_origin_ids"][0]
        .as_str()
        .expect("response origin id");
    let source = std::fs::read_to_string(&handlers_source_path).expect("handlers source");

    assert_eq!(native_plan["handlers_source"], "server/native/handlers.rs");
    assert!(source.contains("use crate::routes;"));
    assert!(source.contains("pub struct OrvNativeHandlerResponse"));
    assert!(source
        .contains("pub const ORV_NATIVE_HANDLER_COUNT: usize = routes::ORV_NATIVE_ROUTE_COUNT;"));
    assert!(source.contains("pub fn orv_native_handle_route("));
    assert!(source.contains("response_origin_id: Some("));
    assert!(source.contains(response_origin));
    assert!(source.contains("status: 200"));
    assert!(source.contains(r#"body: "{\"ok\":true,\"msg\":\"pong\"}""#));
    assert!(!source.contains("native route body lowering pending"));

    cmd_verify_build(&out).expect("verify build artifacts");
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn build_lowers_static_response_body_into_native_handler_source() {
    let path = workspace_path(&["fixtures", "e2e", "hello.orv"]);
    let out = temp_output_dir("native-static-response-handler");

    cmd_build(&path, &out).expect("build artifacts");

    let server_artifact =
        read_json_value(&out.join(SERVER_ARTIFACT_PATH)).expect("server artifact");
    let response = &server_artifact["routes"][0]["responses"][0];
    let handlers_source_path = out.join("server").join("native").join("handlers.rs");
    let source = std::fs::read_to_string(&handlers_source_path).expect("handlers source");

    assert_eq!(response["status"], 200);
    assert_eq!(response["body_kind"], "static_json");
    assert_eq!(response["body_json"], r#"{"ok":true,"msg":"pong"}"#);
    assert!(source.contains("status: 200"));
    assert!(source.contains(r#"body: "{\"ok\":true,\"msg\":\"pong\"}""#));
    assert!(!source.contains("native route body lowering pending"));

    cmd_verify_build(&out).expect("verify build artifacts");
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn build_lowers_route_param_response_into_native_handler_source() {
    let dir = temp_output_dir("native-route-param-response-source");
    std::fs::create_dir_all(&dir).expect("create source dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        r"@server {
  @listen 8080
  @route GET /users/:id {
    @respond 200 { id: @param.id }
  }
}
",
    )
    .expect("write source");
    let out = temp_output_dir("native-route-param-response-build");

    cmd_build(&path, &out).expect("build artifacts");

    let server_artifact =
        read_json_value(&out.join(SERVER_ARTIFACT_PATH)).expect("server artifact");
    let response = &server_artifact["routes"][0]["responses"][0];
    let handlers_source_path = out.join("server").join("native").join("handlers.rs");
    let handlers = std::fs::read_to_string(&handlers_source_path).expect("handlers source");
    let launcher = std::fs::read_to_string(out.join("server").join("native").join("main.rs"))
        .expect("native launcher");

    assert_eq!(response["status"], 200);
    assert_eq!(response["body_kind"], "route_param_json");
    assert_eq!(response["body_route_params"][0]["field"], "id");
    assert_eq!(response["body_route_params"][0]["param"], "id");
    assert!(handlers.contains("routes::orv_native_param_value(route_match, \"id\")"));
    assert!(handlers.contains("orv_native_push_json_string("));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
    cmd_verify_build(&out).expect("verify route param native build");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = std::process::Command::new(cargo)
        .arg("check")
        .arg("--manifest-path")
        .arg(out.join("server").join("native").join("Cargo.toml"))
        .arg("--color")
        .arg("never")
        .output()
        .expect("cargo check route param native launcher");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "route param native launcher cargo check failed:\n{stderr}"
    );
    assert!(
        !stderr.contains("warning:"),
        "route param native launcher cargo check should be warning-free:\n{stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn build_lowers_query_param_response_into_native_handler_source() {
    let dir = temp_output_dir("native-query-param-response-source");
    std::fs::create_dir_all(&dir).expect("create source dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        r"@server {
  @listen 8080
  @route GET /search {
    @respond 200 { q: @query.q }
  }
}
",
    )
    .expect("write source");
    let out = temp_output_dir("native-query-param-response-build");

    cmd_build(&path, &out).expect("build artifacts");

    let server_artifact =
        read_json_value(&out.join(SERVER_ARTIFACT_PATH)).expect("server artifact");
    let response = &server_artifact["routes"][0]["responses"][0];
    let routes = std::fs::read_to_string(out.join("server").join("native").join("routes.rs"))
        .expect("routes source");
    let handlers = std::fs::read_to_string(out.join("server").join("native").join("handlers.rs"))
        .expect("handlers source");
    let launcher = std::fs::read_to_string(out.join("server").join("native").join("main.rs"))
        .expect("native launcher");

    assert_eq!(response["status"], 200);
    assert_eq!(response["body_kind"], "query_param_json");
    assert_eq!(response["body_query_params"][0]["field"], "q");
    assert_eq!(response["body_query_params"][0]["param"], "q");
    assert!(routes.contains("pub query: Vec<OrvNativeParam>"));
    assert!(routes.contains("pub fn orv_native_query_value<'a>("));
    assert!(handlers.contains("routes::orv_native_query_value(route_match, \"q\")"));
    assert!(handlers.contains("orv_native_push_json_string("));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("orv_native_parse_query(query)"));
    assert!(launcher.contains("router::orv_native_dispatch_with_request("));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
    cmd_verify_build(&out).expect("verify query param native build");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = std::process::Command::new(cargo)
        .arg("check")
        .arg("--manifest-path")
        .arg(out.join("server").join("native").join("Cargo.toml"))
        .arg("--color")
        .arg("never")
        .output()
        .expect("cargo check query param native launcher");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "query param native launcher cargo check failed:\n{stderr}"
    );
    assert!(
        !stderr.contains("warning:"),
        "query param native launcher cargo check should be warning-free:\n{stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn build_lowers_request_body_response_into_native_handler_source() {
    let dir = temp_output_dir("native-request-body-response-source");
    std::fs::create_dir_all(&dir).expect("create source dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        r"@server {
  @listen 8080
  @route POST /echo {
    @respond 201 { received: @body }
  }
}
",
    )
    .expect("write source");
    let out = temp_output_dir("native-request-body-response-build");

    cmd_build(&path, &out).expect("build artifacts");

    let server_artifact =
        read_json_value(&out.join(SERVER_ARTIFACT_PATH)).expect("server artifact");
    let response = &server_artifact["routes"][0]["responses"][0];
    let routes = std::fs::read_to_string(out.join("server").join("native").join("routes.rs"))
        .expect("routes source");
    let handlers = std::fs::read_to_string(out.join("server").join("native").join("handlers.rs"))
        .expect("handlers source");
    let launcher = std::fs::read_to_string(out.join("server").join("native").join("main.rs"))
        .expect("native launcher");

    assert_eq!(response["status"], 201);
    assert_eq!(response["body_kind"], "request_body_json");
    assert_eq!(response["body_request_json"][0]["field"], "received");
    assert!(routes.contains("pub body: String"));
    assert!(routes.contains("pub fn orv_native_body_json("));
    assert!(handlers.contains("routes::orv_native_body_json(route_match).unwrap_or(\"null\")"));
    assert!(handlers.contains("body.push_str(\"\\\"received\\\":\");"));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("body: String"));
    assert!(launcher.contains("orv_native_content_length("));
    assert!(launcher.contains("router::orv_native_dispatch_with_request("));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
    cmd_verify_build(&out).expect("verify request body native build");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = std::process::Command::new(cargo)
        .arg("check")
        .arg("--manifest-path")
        .arg(out.join("server").join("native").join("Cargo.toml"))
        .arg("--color")
        .arg("never")
        .output()
        .expect("cargo check request body native launcher");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "request body native launcher cargo check failed:\n{stderr}"
    );
    assert!(
        !stderr.contains("warning:"),
        "request body native launcher cargo check should be warning-free:\n{stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn build_lowers_request_body_field_response_into_native_handler_source() {
    let dir = temp_output_dir("native-request-body-field-response-source");
    std::fs::create_dir_all(&dir).expect("create source dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        r"@server {
  @listen 8080
  @route POST /members {
    @respond 201 { handle: @body.handle, email: @body.email }
  }
}
",
    )
    .expect("write source");
    let out = temp_output_dir("native-request-body-field-response-build");

    cmd_build(&path, &out).expect("build artifacts");

    let server_artifact =
        read_json_value(&out.join(SERVER_ARTIFACT_PATH)).expect("server artifact");
    let response = &server_artifact["routes"][0]["responses"][0];
    let routes = std::fs::read_to_string(out.join("server").join("native").join("routes.rs"))
        .expect("routes source");
    let handlers = std::fs::read_to_string(out.join("server").join("native").join("handlers.rs"))
        .expect("handlers source");
    let launcher = std::fs::read_to_string(out.join("server").join("native").join("main.rs"))
        .expect("native launcher");

    assert_eq!(response["status"], 201);
    assert_eq!(response["body_kind"], "request_body_field_json");
    assert_eq!(response["body_request_fields"][0]["field"], "handle");
    assert_eq!(response["body_request_fields"][0]["name"], "handle");
    assert_eq!(response["body_request_fields"][1]["field"], "email");
    assert_eq!(response["body_request_fields"][1]["name"], "email");
    assert!(routes.contains("pub body_fields: Vec<OrvNativeParam>"));
    assert!(routes.contains("pub fn orv_native_body_field_value<'a>("));
    assert!(handlers.contains("routes::orv_native_body_field_value(route_match, \"handle\")"));
    assert!(handlers.contains("routes::orv_native_body_field_value(route_match, \"email\")"));
    assert!(handlers.contains("orv_native_push_json_string("));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("orv_native_parse_body_fields("));
    assert!(launcher.contains("orv_native_parse_json_object_fields("));
    assert!(launcher.contains("orv_native_parse_query(&body)"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
    cmd_verify_build(&out).expect("verify request body field native build");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = std::process::Command::new(cargo)
        .arg("check")
        .arg("--manifest-path")
        .arg(out.join("server").join("native").join("Cargo.toml"))
        .arg("--color")
        .arg("never")
        .output()
        .expect("cargo check request body field native launcher");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "request body field native launcher cargo check failed:\n{stderr}"
    );
    assert!(
        !stderr.contains("warning:"),
        "request body field native launcher cargo check should be warning-free:\n{stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn build_lowers_mixed_static_and_request_body_field_response_into_native_handler_source() {
    let dir = temp_output_dir("native-mixed-body-field-response-source");
    std::fs::create_dir_all(&dir).expect("create source dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        r#"@server {
  @listen 8080
  @route POST /orders {
    @respond 404 { err: "product_not_found", sku: @body.sku }
  }
}
"#,
    )
    .expect("write source");
    let out = temp_output_dir("native-mixed-body-field-response-build");

    cmd_build(&path, &out).expect("build artifacts");

    let server_artifact =
        read_json_value(&out.join(SERVER_ARTIFACT_PATH)).expect("server artifact");
    let response = &server_artifact["routes"][0]["responses"][0];
    let handlers = std::fs::read_to_string(out.join("server").join("native").join("handlers.rs"))
        .expect("handlers source");
    let launcher = std::fs::read_to_string(out.join("server").join("native").join("main.rs"))
        .expect("native launcher");

    assert_eq!(response["status"], 404);
    assert_eq!(response["body_kind"], "mixed_json");
    assert_eq!(response["body_object_fields"][0]["field"], "err");
    assert_eq!(
        response["body_object_fields"][0]["value_kind"],
        "static_json"
    );
    assert_eq!(
        response["body_object_fields"][0]["value_json"],
        r#""product_not_found""#
    );
    assert_eq!(response["body_object_fields"][1]["field"], "sku");
    assert_eq!(
        response["body_object_fields"][1]["value_kind"],
        "request_body_field"
    );
    assert_eq!(response["body_object_fields"][1]["name"], "sku");
    assert!(handlers.contains("body.push_str(\"\\\"err\\\":\");"));
    assert!(handlers.contains("body.push_str(\"\\\"product_not_found\\\"\");"));
    assert!(handlers.contains("routes::orv_native_body_field_value(route_match, \"sku\")"));
    assert!(handlers.contains("orv_native_push_json_string("));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
    cmd_verify_build(&out).expect("verify mixed native build");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = std::process::Command::new(cargo)
        .arg("check")
        .arg("--manifest-path")
        .arg(out.join("server").join("native").join("Cargo.toml"))
        .arg("--color")
        .arg("never")
        .output()
        .expect("cargo check mixed native launcher");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "mixed native launcher cargo check failed:\n{stderr}"
    );
    assert!(
        !stderr.contains("warning:"),
        "mixed native launcher cargo check should be warning-free:\n{stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&out);
}

const MIXED_DYNAMIC_RESPONSE_SOURCE: &str = r#"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { sku: @body.sku, coupon: @query.coupon }
  }
  @route POST /sessions {
    @respond 201 { matches: @body.token == @query.token }
  }
  @route POST /labels {
    @respond 201 { label: @body.first + @query.suffix }
  }
  @route POST /sku-labels {
    @respond 201 { label: "sku-{@body.sku}-v1" }
  }
  @route POST /joined-labels {
    @respond 201 { label: "{@body.first}-{@query.suffix}" }
  }
  @route POST /quantities {
    @respond 201 { next: 1 + (@body.quantity as int) }
  }
}
"#;

#[test]
fn build_lowers_mixed_dynamic_response_into_native_handler_source() {
    let dir = temp_output_dir("native-mixed-dynamic-response-source");
    std::fs::create_dir_all(&dir).expect("create source dir");
    let path = dir.join("app.orv");
    std::fs::write(&path, MIXED_DYNAMIC_RESPONSE_SOURCE).expect("write source");
    let out = temp_output_dir("native-mixed-dynamic-response-build");

    cmd_build(&path, &out).expect("build artifacts");

    let server_artifact =
        read_json_value(&out.join(SERVER_ARTIFACT_PATH)).expect("server artifact");
    let response = &server_artifact["routes"][0]["responses"][0];
    let sku_label_response = &server_artifact["routes"][3]["responses"][0];
    let joined_label_response = &server_artifact["routes"][4]["responses"][0];
    let handlers = std::fs::read_to_string(out.join("server").join("native").join("handlers.rs"))
        .expect("handlers source");
    let launcher = std::fs::read_to_string(out.join("server").join("native").join("main.rs"))
        .expect("native launcher");

    assert_eq!(response["status"], 201);
    assert_eq!(response["body_kind"], "mixed_json");
    assert_eq!(response["body_object_fields"][0]["field"], "sku");
    assert_eq!(
        response["body_object_fields"][0]["value_kind"],
        "request_body_field"
    );
    assert_eq!(response["body_object_fields"][0]["name"], "sku");
    assert_eq!(response["body_object_fields"][1]["field"], "coupon");
    assert_eq!(
        response["body_object_fields"][1]["value_kind"],
        "query_param"
    );
    assert_eq!(response["body_object_fields"][1]["name"], "coupon");
    assert_eq!(sku_label_response["body_kind"], "request_body_field_json");
    assert_eq!(
        sku_label_response["body_request_fields"][0]["op"],
        "concat_affix"
    );
    assert_eq!(
        sku_label_response["body_request_fields"][0]["operand_json"],
        "4:sku--v1"
    );
    assert_eq!(
        joined_label_response["body_kind"],
        "request_body_field_json"
    );
    assert_eq!(
        joined_label_response["body_request_fields"][0]["op"],
        "concat_join"
    );
    assert_eq!(
        joined_label_response["body_request_fields"][0]["operand_json"],
        "-"
    );
    assert_eq!(
        joined_label_response["body_request_fields"][0]["operand_kind"],
        "query_param"
    );
    assert!(handlers.contains("routes::orv_native_body_field_value(route_match, \"sku\")"));
    assert!(handlers.contains("routes::orv_native_query_value(route_match, \"coupon\")"));
    assert!(handlers.contains("value.push_str(operand)"));
    assert!(handlers.contains("let mut value = String::from(\"sku-\")"));
    assert!(handlers.contains("value.push_str(\"-v1\")"));
    assert!(handlers.contains("value.push_str(\"-\")"));
    assert!(handlers.contains("match value.checked_add(1)"));
    assert!(handlers.contains("orv_native_push_json_string("));
    assert!(!handlers.contains("native route body lowering pending"));
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
    cmd_verify_build(&out).expect("verify mixed dynamic native build");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = std::process::Command::new(cargo)
        .arg("check")
        .arg("--manifest-path")
        .arg(out.join("server").join("native").join("Cargo.toml"))
        .arg("--color")
        .arg("never")
        .output()
        .expect("cargo check mixed dynamic native launcher");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "mixed dynamic native launcher cargo check failed:\n{stderr}"
    );
    assert!(
        !stderr.contains("warning:"),
        "mixed dynamic native launcher cargo check should be warning-free:\n{stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn build_lowers_static_left_ordered_arithmetic_response_into_native_handler_source() {
    let dir = temp_output_dir("native-static-left-ordered-response-source");
    std::fs::create_dir_all(&dir).expect("create source dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        r"@server {
  @listen 8080
  @route POST /int/unit {
    @respond 201 { unit: 100 / (@body.parts as int) }
  }
  @route POST /int/remainder {
    @respond 201 { remainder: 10 % (@body.parts as int) }
  }
  @route POST /float/ratio {
    @respond 201 { ratio: 100.0 / (@body.amount as float) }
  }
  @route POST /float/remainder {
    @respond 201 { remainder: 10.5 % (@body.amount as float) }
  }
  @route POST /int/power {
    @respond 201 { total: 2 ** (@body.exp as int) }
  }
  @route POST /float/power {
    @respond 201 { total: 2.0 ** (@body.exp as float) }
  }
}
",
    )
    .expect("write source");
    let out = temp_output_dir("native-static-left-ordered-response-build");

    cmd_build(&path, &out).expect("build artifacts");

    let server_artifact =
        read_json_value(&out.join(SERVER_ARTIFACT_PATH)).expect("server artifact");
    let handlers = std::fs::read_to_string(out.join("server").join("native").join("handlers.rs"))
        .expect("handlers source");
    let int_unit = &server_artifact["routes"][0]["responses"][0]["body_request_fields"][0];
    let int_remainder = &server_artifact["routes"][1]["responses"][0]["body_request_fields"][0];
    let float_ratio = &server_artifact["routes"][2]["responses"][0]["body_request_fields"][0];
    let float_remainder = &server_artifact["routes"][3]["responses"][0]["body_request_fields"][0];
    let int_power = &server_artifact["routes"][4]["responses"][0]["body_request_fields"][0];
    let float_power = &server_artifact["routes"][5]["responses"][0]["body_request_fields"][0];

    assert_eq!(int_unit["op"], "rdiv");
    assert_eq!(int_remainder["op"], "rrem");
    assert_eq!(float_ratio["op"], "rdiv");
    assert_eq!(float_remainder["op"], "rrem");
    assert_eq!(int_power["op"], "rpow");
    assert_eq!(float_power["op"], "rpow");
    assert!(handlers.contains("100_i64.checked_div(value)"));
    assert!(handlers.contains("10_i64.checked_rem(value)"));
    assert!(handlers.contains("let value = 100.0 / value;"));
    assert!(handlers.contains("let value = 10.5 % value;"));
    assert!(handlers.contains("2_i64.checked_pow(u32::try_from(value).unwrap_or(0))"));
    assert!(handlers.contains("let value = (2.0_f64).powf(value);"));
    assert!(!handlers.contains("native route body lowering pending"));
    cmd_verify_build(&out).expect("verify static-left ordered native build");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = std::process::Command::new(cargo)
        .arg("check")
        .arg("--manifest-path")
        .arg(out.join("server").join("native").join("Cargo.toml"))
        .arg("--color")
        .arg("never")
        .output()
        .expect("cargo check static-left ordered native launcher");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "static-left ordered native launcher cargo check failed:\n{stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn generated_native_server_serves_mixed_static_and_request_body_field_response() {
    let dir = temp_output_dir("native-mixed-body-field-server-source");
    std::fs::create_dir_all(&dir).expect("create source dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        r#"@server {
  @listen 8080
  @route POST /orders {
    @respond 404 { err: "product_not_found", sku: @body.sku }
  }
}
"#,
    )
    .expect("write source");
    let out = temp_output_dir("native-mixed-body-field-server-build");

    cmd_build(&path, &out).expect("build artifacts");
    cmd_verify_build(&out).expect("verify mixed native server build");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = std::process::Command::new(cargo)
        .arg("build")
        .arg("--manifest-path")
        .arg(out.join("server").join("native").join("Cargo.toml"))
        .arg("--release")
        .arg("--color")
        .arg("never")
        .output()
        .expect("cargo build mixed native server");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "mixed native server cargo build failed:\n{stderr}"
    );

    let binary = out
        .join("server")
        .join("native")
        .join("target")
        .join("release")
        .join("orv-native-server");
    let mut child = std::process::Command::new(&binary)
        .env("ORV_BUILD_DIR", &out)
        .env("ORV_HOST", "127.0.0.1")
        .env("ORV_PORT", "0")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn generated native server");
    let stderr = child.stderr.take().expect("native server stderr");
    let child = ChildGuard(child);
    let mut stderr = std::io::BufReader::new(stderr);
    let mut line = String::new();
    std::io::BufRead::read_line(&mut stderr, &mut line).expect("native server listen line");
    let address = line
        .trim()
        .strip_prefix("orv native server listening on ")
        .expect("native listen address");

    let response = send_raw_http_json_post(address, "/orders", r#"{"sku":"sku-1"}"#);

    assert!(response.starts_with("HTTP/1.1 404"));
    assert!(response.contains("content-type: application/json"));
    assert!(response.contains(r#"{"err":"product_not_found","sku":"sku-1"}"#));

    drop(child);
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&out);
}

const MIXED_DYNAMIC_SERVER_SOURCE: &str = r#"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { sku: @body.sku, coupon: @query.coupon }
  }
  @route POST /sessions {
    @respond 201 { matches: @body.token == @query.token }
  }
  @route POST /labels {
    @respond 201 { label: @body.first + @query.suffix }
  }
  @route POST /sku-labels {
    @respond 201 { label: "sku-{@body.sku}-v1" }
  }
  @route POST /joined-labels {
    @respond 201 { label: "{@body.first}-{@query.suffix}" }
  }
  @route POST /quantities {
    @respond 201 { next: 1 + (@body.quantity as int) }
  }
  @route POST /quantity-doubles {
    @respond 201 { doubled: 2 * (@body.quantity as int) }
  }
  @route POST /quantity-limits {
    @respond 201 { below_limit: 10 > (@body.quantity as int) }
  }
}
"#;

#[test]
fn generated_native_server_serves_mixed_dynamic_response() {
    let dir = temp_output_dir("native-mixed-dynamic-server-source");
    std::fs::create_dir_all(&dir).expect("create source dir");
    let path = dir.join("app.orv");
    std::fs::write(&path, MIXED_DYNAMIC_SERVER_SOURCE).expect("write source");
    let out = temp_output_dir("native-mixed-dynamic-server-build");

    cmd_build(&path, &out).expect("build artifacts");
    cmd_verify_build(&out).expect("verify mixed dynamic native server build");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = std::process::Command::new(cargo)
        .arg("build")
        .arg("--manifest-path")
        .arg(out.join("server").join("native").join("Cargo.toml"))
        .arg("--release")
        .arg("--color")
        .arg("never")
        .output()
        .expect("cargo build mixed dynamic native server");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "mixed dynamic native server cargo build failed:\n{stderr}"
    );

    let binary = out
        .join("server")
        .join("native")
        .join("target")
        .join("release")
        .join("orv-native-server");
    let mut child = std::process::Command::new(&binary)
        .env("ORV_BUILD_DIR", &out)
        .env("ORV_HOST", "127.0.0.1")
        .env("ORV_PORT", "0")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn generated native server");
    let stderr = child.stderr.take().expect("native server stderr");
    let child = ChildGuard(child);
    let mut stderr = std::io::BufReader::new(stderr);
    let mut line = String::new();
    std::io::BufRead::read_line(&mut stderr, &mut line).expect("native server listen line");
    let address = line
        .trim()
        .strip_prefix("orv native server listening on ")
        .expect("native listen address");

    let response = send_raw_http_json_post(address, "/orders?coupon=SAVE10", r#"{"sku":"sku-1"}"#);
    let session_response =
        send_raw_http_json_post(address, "/sessions?token=abc", r#"{"token":"abc"}"#);
    let label_response =
        send_raw_http_json_post(address, "/labels?suffix=-pro", r#"{"first":"orv"}"#);
    let sku_label_response = send_raw_http_json_post(address, "/sku-labels", r#"{"sku":"A1"}"#);
    let joined_label_response =
        send_raw_http_json_post(address, "/joined-labels?suffix=pro", r#"{"first":"orv"}"#);
    let quantity_response = send_raw_http_json_post(address, "/quantities", r#"{"quantity":"7"}"#);
    let doubled_response =
        send_raw_http_json_post(address, "/quantity-doubles", r#"{"quantity":"7"}"#);
    let limit_response =
        send_raw_http_json_post(address, "/quantity-limits", r#"{"quantity":"7"}"#);

    assert!(response.starts_with("HTTP/1.1 201"));
    assert!(response.contains("content-type: application/json"));
    assert!(response.contains(r#"{"sku":"sku-1","coupon":"SAVE10"}"#));
    assert!(session_response.starts_with("HTTP/1.1 201"));
    assert!(session_response.contains(r#"{"matches":true}"#));
    assert!(label_response.contains(r#"{"label":"orv-pro"}"#));
    assert!(sku_label_response.contains(r#"{"label":"sku-A1-v1"}"#));
    assert!(joined_label_response.contains(r#"{"label":"orv-pro"}"#));
    assert!(quantity_response.contains(r#"{"next":8}"#));
    assert!(doubled_response.contains(r#"{"doubled":14}"#));
    assert!(limit_response.contains(r#"{"below_limit":true}"#));

    drop(child);
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn generated_native_server_serves_guarded_multi_response_route() {
    let dir = temp_output_dir("native-guarded-multi-response-server-source");
    std::fs::create_dir_all(&dir).expect("create source dir");
    let path = dir.join("app.orv");
    std::fs::write(
            &path,
            r#"@server {
  @listen 8080
  @route POST /orders {
    if @body.sku == "" {
      @respond 400 { err: "missing_sku" }
    }
    @respond 201 { sku: @body.sku }
  }
  @route POST /orders-bonus {
    if @body.sku == "" {
      @respond 400 { err: "missing_sku" }
    }
    @respond 201 { quantity: (@body.quantity as int) + ((@body.bonus as int) * 2) }
  }
  @route POST /orders-bonus-left {
    @respond 201 { quantity: ((@body.bonus as int) * 2) + (@body.quantity as int) }
  }
  @route POST /orders-bonus-delta {
    @respond 201 { quantity: ((@body.bonus as int) * 2) - (@body.quantity as int) }
  }
  @route POST /members {
    if @body.password != @body.confirm {
      @respond 400 { err: "password_mismatch" }
    }
    @respond 201 { email: @body.email }
  }
  @route POST /sessions {
    if @body.token == @query.token {
      @respond 201 { ok: true }
    }
    @respond 401 { err: "token_mismatch" }
  }
  @route POST /quantity {
    if (@body.quantity as int) > 0 {
      @respond 201 { accepted: true, quantity: @body.quantity as int }
    }
    @respond 400 { err: "bad_quantity" }
  }
  @route POST /inventory {
    if (@body.quantity as int) <= (@body.stock as int) {
      @respond 201 { accepted: true, quantity: @body.quantity as int }
    }
    @respond 409 { err: "out_of_stock" }
  }
  @route POST /inventory-bulk {
    if (@body.quantity as int) <= ((@body.stock as int) * 10) {
      @respond 201 { accepted: true, quantity: @body.quantity as int }
    }
    @respond 409 { err: "out_of_stock" }
  }
  @route POST /inventory-value {
    if (@body.total as int) <= ((@body.quantity as int) * (@body.unit_price as int)) {
      @respond 201 { accepted: true, total: @body.total as int }
    }
    @respond 409 { err: "over_total" }
  }
  @route POST /inventory-value-scaled {
    if (@body.total as int) <= (((@body.quantity as int) * (@body.unit_price as int)) * 100) {
      @respond 201 { accepted: true, total: @body.total as int }
    }
    @respond 409 { err: "over_total" }
  }
  @route POST /inventory-value-static {
    if ((@body.quantity as int) * (@body.unit_price as int)) <= 1000 {
      @respond 201 { accepted: true, quantity: @body.quantity as int }
    }
    @respond 409 { err: "over_total" }
  }
  @route POST /inventory-value-product {
    if ((@body.quantity as int) * (@body.unit_price as int)) <= ((@body.stock as int) * (@body.reserve_price as int)) {
      @respond 201 { accepted: true, quantity: @body.quantity as int }
    }
    @respond 409 { err: "over_total" }
  }
  @route POST /ifelse-inventory {
    if (@body.quantity as int) <= (@body.stock as int) {
      @respond 201 { accepted: true, quantity: @body.quantity as int }
    } else {
      @respond 409 { err: "out_of_stock" }
    }
  }
  @route POST /tiered-inventory {
    if (@body.quantity as int) <= 0 {
      @respond 400 { err: "bad_quantity" }
    } else if (@body.quantity as int) <= (@body.stock as int) {
      @respond 201 { accepted: true, quantity: @body.quantity as int }
    } else {
      @respond 409 { err: "out_of_stock" }
    }
  }
  @route POST /tiered-block-inventory {
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
  @route POST /tiered-fallback-inventory {
    if (@body.quantity as int) <= 0 {
      @respond 400 { err: "bad_quantity" }
    } else if (@body.quantity as int) <= (@body.stock as int) {
      @respond 201 { accepted: true, quantity: @body.quantity as int }
    }
    @respond 409 { err: "out_of_stock" }
  }
  @route POST /amount {
    if (@body.amount as float) > 0.0 {
      @respond 201 { accepted: true, amount: @body.amount as float }
    }
    @respond 400 { err: "bad_amount" }
  }
  @route POST /limit {
    if (@body.amount as float) <= (@query.limit as float) {
      @respond 201 { accepted: true, amount: @body.amount as float }
    }
    @respond 409 { err: "amount_over_limit" }
  }
  @route POST /limit-product {
    if ((@body.price as float) * (@body.quantity as float)) <= ((@body.limit_price as float) * (@body.limit_units as float)) {
      @respond 201 { accepted: true, amount: @body.price as float }
    }
    @respond 409 { err: "amount_over_limit" }
  }
  @route GET /catalog/:kind {
    if @param.kind == "sale" {
      @respond 200 { kind: @param.kind }
    }
    @respond 200 { kind: "regular" }
  }
  @route GET /search {
    if @query.mode != "compact" {
      @respond 200 { mode: @query.mode }
    }
    @respond 200 { mode: "compact" }
  }
}
"#,
        )
        .expect("write source");
    let out = temp_output_dir("native-guarded-multi-response-server-build");

    cmd_build(&path, &out).expect("build artifacts");
    cmd_verify_build(&out).expect("verify guarded native server build");
    let launcher = std::fs::read_to_string(out.join("server").join("native").join("main.rs"))
        .expect("native launcher");
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = std::process::Command::new(cargo)
        .arg("build")
        .arg("--manifest-path")
        .arg(out.join("server").join("native").join("Cargo.toml"))
        .arg("--release")
        .arg("--color")
        .arg("never")
        .output()
        .expect("cargo build guarded native server");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "guarded native server cargo build failed:\n{stderr}"
    );

    let binary = out
        .join("server")
        .join("native")
        .join("target")
        .join("release")
        .join("orv-native-server");
    let mut child = std::process::Command::new(&binary)
        .env("ORV_BUILD_DIR", &out)
        .env("ORV_HOST", "127.0.0.1")
        .env("ORV_PORT", "0")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn generated guarded native server");
    let stderr = child.stderr.take().expect("native server stderr");
    let child = ChildGuard(child);
    let mut stderr = std::io::BufReader::new(stderr);
    let mut line = String::new();
    std::io::BufRead::read_line(&mut stderr, &mut line).expect("native server listen line");
    let address = line
        .trim()
        .strip_prefix("orv native server listening on ")
        .expect("native listen address");

    let missing = send_raw_http_json_post(address, "/orders", r#"{"sku":""}"#);
    let created = send_raw_http_json_post(address, "/orders", r#"{"sku":"sku-7"}"#);
    let missing_bonus = send_raw_http_json_post(address, "/orders-bonus", r#"{"sku":""}"#);
    let created_bonus = send_raw_http_json_post(
        address,
        "/orders-bonus",
        r#"{"sku":"sku-7","quantity":"7","bonus":"2"}"#,
    );
    let created_bonus_left = send_raw_http_json_post(
        address,
        "/orders-bonus-left",
        r#"{"quantity":"7","bonus":"2"}"#,
    );
    let created_bonus_delta = send_raw_http_json_post(
        address,
        "/orders-bonus-delta",
        r#"{"quantity":"5","bonus":"8"}"#,
    );
    let mismatch = send_raw_http_json_post(
        address,
        "/members",
        r#"{"email":"a@orv.dev","password":"one","confirm":"two"}"#,
    );
    let member = send_raw_http_json_post(
        address,
        "/members",
        r#"{"email":"a@orv.dev","password":"same","confirm":"same"}"#,
    );
    let session = send_raw_http_json_post(address, "/sessions?token=abc", r#"{"token":"abc"}"#);
    let rejected_session =
        send_raw_http_json_post(address, "/sessions?token=abc", r#"{"token":"xyz"}"#);
    let accepted_quantity = send_raw_http_json_post(address, "/quantity", r#"{"quantity":"3"}"#);
    let rejected_quantity = send_raw_http_json_post(address, "/quantity", r#"{"quantity":"0"}"#);
    let accepted_inventory =
        send_raw_http_json_post(address, "/inventory", r#"{"quantity":"3","stock":"5"}"#);
    let rejected_inventory =
        send_raw_http_json_post(address, "/inventory", r#"{"quantity":"7","stock":"5"}"#);
    let accepted_bulk_inventory = send_raw_http_json_post(
        address,
        "/inventory-bulk",
        r#"{"quantity":"30","stock":"5"}"#,
    );
    let rejected_bulk_inventory = send_raw_http_json_post(
        address,
        "/inventory-bulk",
        r#"{"quantity":"51","stock":"5"}"#,
    );
    let accepted_value_inventory = send_raw_http_json_post(
        address,
        "/inventory-value",
        r#"{"total":"875","quantity":"7","unit_price":"125"}"#,
    );
    let rejected_value_inventory = send_raw_http_json_post(
        address,
        "/inventory-value",
        r#"{"total":"901","quantity":"7","unit_price":"125"}"#,
    );
    let accepted_static_value_inventory = send_raw_http_json_post(
        address,
        "/inventory-value-static",
        r#"{"quantity":"7","unit_price":"125"}"#,
    );
    let rejected_static_value_inventory = send_raw_http_json_post(
        address,
        "/inventory-value-static",
        r#"{"quantity":"9","unit_price":"125"}"#,
    );
    let accepted_product_value_inventory = send_raw_http_json_post(
        address,
        "/inventory-value-product",
        r#"{"quantity":"7","unit_price":"125","stock":"8","reserve_price":"125"}"#,
    );
    let rejected_product_value_inventory = send_raw_http_json_post(
        address,
        "/inventory-value-product",
        r#"{"quantity":"9","unit_price":"125","stock":"8","reserve_price":"125"}"#,
    );
    let accepted_scaled_product_value_inventory = send_raw_http_json_post(
        address,
        "/inventory-value-scaled",
        r#"{"total":"87500","quantity":"7","unit_price":"125"}"#,
    );
    let rejected_scaled_product_value_inventory = send_raw_http_json_post(
        address,
        "/inventory-value-scaled",
        r#"{"total":"87501","quantity":"7","unit_price":"125"}"#,
    );
    let accepted_ifelse_inventory = send_raw_http_json_post(
        address,
        "/ifelse-inventory",
        r#"{"quantity":"3","stock":"5"}"#,
    );
    let rejected_ifelse_inventory = send_raw_http_json_post(
        address,
        "/ifelse-inventory",
        r#"{"quantity":"7","stock":"5"}"#,
    );
    let invalid_tiered_inventory = send_raw_http_json_post(
        address,
        "/tiered-inventory",
        r#"{"quantity":"0","stock":"5"}"#,
    );
    let accepted_tiered_inventory = send_raw_http_json_post(
        address,
        "/tiered-inventory",
        r#"{"quantity":"3","stock":"5"}"#,
    );
    let rejected_tiered_inventory = send_raw_http_json_post(
        address,
        "/tiered-inventory",
        r#"{"quantity":"7","stock":"5"}"#,
    );
    let accepted_tiered_block_inventory = send_raw_http_json_post(
        address,
        "/tiered-block-inventory",
        r#"{"quantity":"3","stock":"5"}"#,
    );
    let rejected_tiered_block_inventory = send_raw_http_json_post(
        address,
        "/tiered-block-inventory",
        r#"{"quantity":"7","stock":"5"}"#,
    );
    let invalid_tiered_fallback_inventory = send_raw_http_json_post(
        address,
        "/tiered-fallback-inventory",
        r#"{"quantity":"0","stock":"5"}"#,
    );
    let accepted_tiered_fallback_inventory = send_raw_http_json_post(
        address,
        "/tiered-fallback-inventory",
        r#"{"quantity":"3","stock":"5"}"#,
    );
    let rejected_tiered_fallback_inventory = send_raw_http_json_post(
        address,
        "/tiered-fallback-inventory",
        r#"{"quantity":"7","stock":"5"}"#,
    );
    let accepted_amount = send_raw_http_json_post(address, "/amount", r#"{"amount":"12.5"}"#);
    let rejected_amount = send_raw_http_json_post(address, "/amount", r#"{"amount":"0.0"}"#);
    let accepted_limit =
        send_raw_http_json_post(address, "/limit?limit=20.0", r#"{"amount":"12.5"}"#);
    let rejected_limit =
        send_raw_http_json_post(address, "/limit?limit=10.0", r#"{"amount":"12.5"}"#);
    let accepted_product_limit = send_raw_http_json_post(
        address,
        "/limit-product",
        r#"{"price":"12.5","quantity":"3","limit_price":"20.0","limit_units":"2"}"#,
    );
    let rejected_product_limit = send_raw_http_json_post(
        address,
        "/limit-product",
        r#"{"price":"12.5","quantity":"4","limit_price":"12.5","limit_units":"3"}"#,
    );
    let sale = send_raw_http(address, "/catalog/sale");
    let regular = send_raw_http(address, "/catalog/full");
    let expanded = send_raw_http(address, "/search?mode=expanded");
    let compact = send_raw_http(address, "/search?mode=compact");

    assert!(missing.starts_with("HTTP/1.1 400"));
    assert!(missing.contains(r#"{"err":"missing_sku"}"#));
    assert!(created.starts_with("HTTP/1.1 201"));
    assert!(created.contains(r#"{"sku":"sku-7"}"#));
    assert!(missing_bonus.starts_with("HTTP/1.1 400"));
    assert!(missing_bonus.contains(r#"{"err":"missing_sku"}"#));
    assert!(created_bonus.starts_with("HTTP/1.1 201"));
    assert!(created_bonus.contains(r#"{"quantity":11}"#));
    assert!(created_bonus_left.starts_with("HTTP/1.1 201"));
    assert!(created_bonus_left.contains(r#"{"quantity":11}"#));
    assert!(created_bonus_delta.starts_with("HTTP/1.1 201"));
    assert!(created_bonus_delta.contains(r#"{"quantity":11}"#));
    assert!(mismatch.starts_with("HTTP/1.1 400"));
    assert!(mismatch.contains(r#"{"err":"password_mismatch"}"#));
    assert!(member.starts_with("HTTP/1.1 201"));
    assert!(member.contains(r#"{"email":"a@orv.dev"}"#));
    assert!(session.starts_with("HTTP/1.1 201"));
    assert!(session.contains(r#"{"ok":true}"#));
    assert!(rejected_session.starts_with("HTTP/1.1 401"));
    assert!(rejected_session.contains(r#"{"err":"token_mismatch"}"#));
    assert!(accepted_quantity.starts_with("HTTP/1.1 201"));
    assert!(accepted_quantity.contains(r#"{"accepted":true,"quantity":3}"#));
    assert!(rejected_quantity.starts_with("HTTP/1.1 400"));
    assert!(rejected_quantity.contains(r#"{"err":"bad_quantity"}"#));
    assert!(accepted_inventory.starts_with("HTTP/1.1 201"));
    assert!(accepted_inventory.contains(r#"{"accepted":true,"quantity":3}"#));
    assert!(rejected_inventory.starts_with("HTTP/1.1 409"));
    assert!(rejected_inventory.contains(r#"{"err":"out_of_stock"}"#));
    assert!(accepted_bulk_inventory.starts_with("HTTP/1.1 201"));
    assert!(accepted_bulk_inventory.contains(r#"{"accepted":true,"quantity":30}"#));
    assert!(rejected_bulk_inventory.starts_with("HTTP/1.1 409"));
    assert!(rejected_bulk_inventory.contains(r#"{"err":"out_of_stock"}"#));
    assert!(accepted_value_inventory.starts_with("HTTP/1.1 201"));
    assert!(accepted_value_inventory.contains(r#"{"accepted":true,"total":875}"#));
    assert!(rejected_value_inventory.starts_with("HTTP/1.1 409"));
    assert!(rejected_value_inventory.contains(r#"{"err":"over_total"}"#));
    assert!(accepted_static_value_inventory.starts_with("HTTP/1.1 201"));
    assert!(accepted_static_value_inventory.contains(r#"{"accepted":true,"quantity":7}"#));
    assert!(rejected_static_value_inventory.starts_with("HTTP/1.1 409"));
    assert!(rejected_static_value_inventory.contains(r#"{"err":"over_total"}"#));
    assert!(accepted_product_value_inventory.starts_with("HTTP/1.1 201"));
    assert!(accepted_product_value_inventory.contains(r#"{"accepted":true,"quantity":7}"#));
    assert!(rejected_product_value_inventory.starts_with("HTTP/1.1 409"));
    assert!(rejected_product_value_inventory.contains(r#"{"err":"over_total"}"#));
    assert!(accepted_scaled_product_value_inventory.starts_with("HTTP/1.1 201"));
    assert!(accepted_scaled_product_value_inventory.contains(r#"{"accepted":true,"total":87500}"#));
    assert!(rejected_scaled_product_value_inventory.starts_with("HTTP/1.1 409"));
    assert!(rejected_scaled_product_value_inventory.contains(r#"{"err":"over_total"}"#));
    assert!(accepted_ifelse_inventory.starts_with("HTTP/1.1 201"));
    assert!(accepted_ifelse_inventory.contains(r#"{"accepted":true,"quantity":3}"#));
    assert!(rejected_ifelse_inventory.starts_with("HTTP/1.1 409"));
    assert!(rejected_ifelse_inventory.contains(r#"{"err":"out_of_stock"}"#));
    assert!(invalid_tiered_inventory.starts_with("HTTP/1.1 400"));
    assert!(invalid_tiered_inventory.contains(r#"{"err":"bad_quantity"}"#));
    assert!(accepted_tiered_inventory.starts_with("HTTP/1.1 201"));
    assert!(accepted_tiered_inventory.contains(r#"{"accepted":true,"quantity":3}"#));
    assert!(rejected_tiered_inventory.starts_with("HTTP/1.1 409"));
    assert!(rejected_tiered_inventory.contains(r#"{"err":"out_of_stock"}"#));
    assert!(accepted_tiered_block_inventory.starts_with("HTTP/1.1 201"));
    assert!(accepted_tiered_block_inventory.contains(r#"{"accepted":true,"quantity":3}"#));
    assert!(rejected_tiered_block_inventory.starts_with("HTTP/1.1 409"));
    assert!(rejected_tiered_block_inventory.contains(r#"{"err":"out_of_stock"}"#));
    assert!(invalid_tiered_fallback_inventory.starts_with("HTTP/1.1 400"));
    assert!(invalid_tiered_fallback_inventory.contains(r#"{"err":"bad_quantity"}"#));
    assert!(accepted_tiered_fallback_inventory.starts_with("HTTP/1.1 201"));
    assert!(accepted_tiered_fallback_inventory.contains(r#"{"accepted":true,"quantity":3}"#));
    assert!(rejected_tiered_fallback_inventory.starts_with("HTTP/1.1 409"));
    assert!(rejected_tiered_fallback_inventory.contains(r#"{"err":"out_of_stock"}"#));
    assert!(accepted_amount.starts_with("HTTP/1.1 201"));
    assert!(accepted_amount.contains(r#"{"accepted":true,"amount":12.5}"#));
    assert!(rejected_amount.starts_with("HTTP/1.1 400"));
    assert!(rejected_amount.contains(r#"{"err":"bad_amount"}"#));
    assert!(accepted_limit.starts_with("HTTP/1.1 201"));
    assert!(accepted_limit.contains(r#"{"accepted":true,"amount":12.5}"#));
    assert!(rejected_limit.starts_with("HTTP/1.1 409"));
    assert!(rejected_limit.contains(r#"{"err":"amount_over_limit"}"#));
    assert!(accepted_product_limit.starts_with("HTTP/1.1 201"));
    assert!(accepted_product_limit.contains(r#"{"accepted":true,"amount":12.5}"#));
    assert!(rejected_product_limit.starts_with("HTTP/1.1 409"));
    assert!(rejected_product_limit.contains(r#"{"err":"amount_over_limit"}"#));
    assert!(sale.starts_with("HTTP/1.1 200"));
    assert!(sale.contains(r#"{"kind":"sale"}"#));
    assert!(regular.starts_with("HTTP/1.1 200"));
    assert!(regular.contains(r#"{"kind":"regular"}"#));
    assert!(expanded.starts_with("HTTP/1.1 200"));
    assert!(expanded.contains(r#"{"mode":"expanded"}"#));
    assert!(compact.starts_with("HTTP/1.1 200"));
    assert!(compact.contains(r#"{"mode":"compact"}"#));

    drop(child);
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn generated_native_server_serves_request_body_int_cast_response() {
    let dir = temp_output_dir("native-request-body-int-cast-server-source");
    std::fs::create_dir_all(&dir).expect("create source dir");
    let path = dir.join("app.orv");
    std::fs::write(
            &path,
            r#"@server {
  @listen 8080
  @route POST /orders {
    @respond 201 { quantity: @body.quantity as int }
  }
  @route POST /orders/next {
    @respond 201 { quantity: (@body.quantity as int) + 1 }
  }
  @route POST /orders/remaining {
    @respond 201 { remaining: 10 - (@body.quantity as int) }
  }
  @route POST /orders/neg {
    @respond 201 { quantity: -(@body.quantity as int) }
  }
  @route POST /orders/cents {
    @respond 201 { cents: (@body.quantity as int) * 100 }
  }
  @route POST /orders/cents-total {
    @respond 201 { cents: (@body.quantity as int) * ((@body.unit_price as int) * 100) }
  }
  @route POST /orders/total {
    @respond 201 { total: (@body.quantity as int) * (@body.unit_price as int) }
  }
  @route POST /orders/total-with-fee {
    @respond 201 { total: (@body.fee as int) + ((@body.quantity as int) * (@body.unit_price as int)) }
  }
  @route POST /orders/scaled-product-fee {
    @respond 201 { total: (@body.base as int) + (((@body.quantity as int) * (@body.unit_price as int)) * 100) }
  }
  @route POST /orders/product-plus-static-fee {
    @respond 201 { total: ((@body.quantity as int) * (@body.unit_price as int)) + 25 }
  }
  @route POST /orders/product-plus-product-fee {
    @respond 201 { total: ((@body.quantity as int) * (@body.unit_price as int)) + ((@body.fee_units as int) * (@body.fee_value as int)) }
  }
  @route POST /orders/triple-product-fee {
    @respond 201 { total: (@body.base as int) + (((@body.quantity as int) * (@body.unit_price as int)) * (@body.bundle_count as int)) }
  }
  @route POST /orders/static-minus-product {
    @respond 201 { remaining: 1000 - ((@body.quantity as int) * (@body.unit_price as int)) }
  }
  @route POST /orders/bundles {
    @respond 201 { bundles: (@body.total as int) / ((@body.quantity as int) * (@body.unit_price as int)) }
  }
  @route POST /orders/remainder-product-left {
    @respond 201 { remainder: ((@body.quantity as int) * (@body.unit_price as int)) % (@body.total as int) }
  }
  @route POST /orders/power {
    @respond 201 { total: (@body.quantity as int) ** (@body.bonus as int) }
  }
  @route POST /orders/power-invalid {
    @respond 201 { total: (@body.quantity as int) ** -1 }
  }
  @route POST /orders/due {
    @respond 201 { due: (@body.total as int) - (@body.discount as int) }
  }
  @route POST /orders/share {
    @respond 201 { share: (@body.total as int) / (@body.parts as int) }
  }
  @route POST /orders/unit-bundle {
    @respond 201 { unit: (@body.total as int) / ((@body.parts as int) * 100) }
  }
  @route POST /orders/unit-bundle-left {
    @respond 201 { unit: ((@body.total as int) * 100) / (@body.parts as int) }
  }
  @route POST /orders/remainder {
    @respond 201 { remainder: (@body.total as int) % (@body.parts as int) }
  }
  @route POST /orders/remainder-scaled {
    @respond 201 { remainder: (@body.total as int) % ((@body.parts as int) * 10) }
  }
  @route POST /orders/remainder-scaled-left {
    @respond 201 { remainder: ((@body.total as int) * 10) % (@body.parts as int) }
  }
  @route POST /orders/available {
    @respond 201 { available: (@body.quantity as int) <= (@body.stock as int) }
  }
  @route POST /orders/available-bulk {
    @respond 201 { available: (@body.quantity as int) <= ((@body.stock as int) * 10) }
  }
  @route POST /orders/covered-min {
    @respond 201 { covered: ((@body.minimum as int) * 100) <= (@body.total as int) }
  }
  @route POST /orders/covered-total {
    @respond 201 { covered: (@body.total as int) <= ((@body.quantity as int) * (@body.unit_price as int)) }
  }
  @route POST /orders/product-covered-static {
    @respond 201 { covered: ((@body.quantity as int) * (@body.unit_price as int)) <= 1000 }
  }
  @route POST /orders/product-covered-product {
    @respond 201 { covered: ((@body.quantity as int) * (@body.unit_price as int)) <= ((@body.stock as int) * (@body.reserve_price as int)) }
  }
}
"#,
        )
        .expect("write source");
    let out = temp_output_dir("native-request-body-int-cast-server-build");

    cmd_build(&path, &out).expect("build artifacts");
    cmd_verify_build(&out).expect("verify int cast native server build");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = std::process::Command::new(cargo)
        .arg("build")
        .arg("--manifest-path")
        .arg(out.join("server").join("native").join("Cargo.toml"))
        .arg("--release")
        .arg("--color")
        .arg("never")
        .output()
        .expect("cargo build int cast native server");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "int cast native server cargo build failed:\n{stderr}"
    );

    let binary = out
        .join("server")
        .join("native")
        .join("target")
        .join("release")
        .join("orv-native-server");
    let mut child = std::process::Command::new(&binary)
        .env("ORV_BUILD_DIR", &out)
        .env("ORV_HOST", "127.0.0.1")
        .env("ORV_PORT", "0")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn generated int cast native server");
    let stderr = child.stderr.take().expect("native server stderr");
    let child = ChildGuard(child);
    let mut stderr = std::io::BufReader::new(stderr);
    let mut line = String::new();
    std::io::BufRead::read_line(&mut stderr, &mut line).expect("native server listen line");
    let address = line
        .trim()
        .strip_prefix("orv native server listening on ")
        .expect("native listen address");

    let response = send_raw_http_json_post(address, "/orders", r#"{"quantity":"7"}"#);
    let next_response = send_raw_http_json_post(address, "/orders/next", r#"{"quantity":"7"}"#);
    let remaining_response =
        send_raw_http_json_post(address, "/orders/remaining", r#"{"quantity":"7"}"#);
    let neg_response = send_raw_http_json_post(address, "/orders/neg", r#"{"quantity":"7"}"#);
    let cents_response = send_raw_http_json_post(address, "/orders/cents", r#"{"quantity":"7"}"#);
    let cents_total_response = send_raw_http_json_post(
        address,
        "/orders/cents-total",
        r#"{"quantity":"2","unit_price":"125"}"#,
    );
    let total_response = send_raw_http_json_post(
        address,
        "/orders/total",
        r#"{"quantity":"7","unit_price":"125"}"#,
    );
    let total_with_fee_response = send_raw_http_json_post(
        address,
        "/orders/total-with-fee",
        r#"{"fee":"25","quantity":"7","unit_price":"125"}"#,
    );
    let scaled_product_fee_response = send_raw_http_json_post(
        address,
        "/orders/scaled-product-fee",
        r#"{"base":"25","quantity":"7","unit_price":"125"}"#,
    );
    let product_plus_static_fee_response = send_raw_http_json_post(
        address,
        "/orders/product-plus-static-fee",
        r#"{"quantity":"7","unit_price":"125"}"#,
    );
    let product_plus_product_fee_response = send_raw_http_json_post(
        address,
        "/orders/product-plus-product-fee",
        r#"{"quantity":"7","unit_price":"125","fee_units":"2","fee_value":"50"}"#,
    );
    let triple_product_fee_response = send_raw_http_json_post(
        address,
        "/orders/triple-product-fee",
        r#"{"base":"25","quantity":"7","unit_price":"125","bundle_count":"2"}"#,
    );
    let static_minus_product_response = send_raw_http_json_post(
        address,
        "/orders/static-minus-product",
        r#"{"quantity":"7","unit_price":"125"}"#,
    );
    let bundles_response = send_raw_http_json_post(
        address,
        "/orders/bundles",
        r#"{"total":"1750","quantity":"7","unit_price":"125"}"#,
    );
    let remainder_product_left_response = send_raw_http_json_post(
        address,
        "/orders/remainder-product-left",
        r#"{"quantity":"7","unit_price":"125","total":"400"}"#,
    );
    let power_response =
        send_raw_http_json_post(address, "/orders/power", r#"{"quantity":"2","bonus":"6"}"#);
    let invalid_power_response =
        send_raw_http_json_post(address, "/orders/power-invalid", r#"{"quantity":"2"}"#);
    let due_response = send_raw_http_json_post(
        address,
        "/orders/due",
        r#"{"total":"875","discount":"125"}"#,
    );
    let share_response =
        send_raw_http_json_post(address, "/orders/share", r#"{"total":"875","parts":"7"}"#);
    let unit_bundle_response = send_raw_http_json_post(
        address,
        "/orders/unit-bundle",
        r#"{"total":"1000","parts":"2"}"#,
    );
    let unit_bundle_left_response = send_raw_http_json_post(
        address,
        "/orders/unit-bundle-left",
        r#"{"total":"5","parts":"2"}"#,
    );
    let remainder_response = send_raw_http_json_post(
        address,
        "/orders/remainder",
        r#"{"total":"875","parts":"6"}"#,
    );
    let remainder_scaled_response = send_raw_http_json_post(
        address,
        "/orders/remainder-scaled",
        r#"{"total":"101","parts":"3"}"#,
    );
    let remainder_scaled_left_response = send_raw_http_json_post(
        address,
        "/orders/remainder-scaled-left",
        r#"{"total":"3","parts":"7"}"#,
    );
    let available_response = send_raw_http_json_post(
        address,
        "/orders/available",
        r#"{"quantity":"7","stock":"10"}"#,
    );
    let available_bulk_response = send_raw_http_json_post(
        address,
        "/orders/available-bulk",
        r#"{"quantity":"70","stock":"7"}"#,
    );
    let covered_min_response = send_raw_http_json_post(
        address,
        "/orders/covered-min",
        r#"{"minimum":"10","total":"1000"}"#,
    );
    let covered_total_response = send_raw_http_json_post(
        address,
        "/orders/covered-total",
        r#"{"total":"875","quantity":"7","unit_price":"125"}"#,
    );
    let product_covered_static_response = send_raw_http_json_post(
        address,
        "/orders/product-covered-static",
        r#"{"quantity":"7","unit_price":"125"}"#,
    );
    let product_covered_product_response = send_raw_http_json_post(
        address,
        "/orders/product-covered-product",
        r#"{"quantity":"7","unit_price":"125","stock":"8","reserve_price":"125"}"#,
    );

    assert!(response.starts_with("HTTP/1.1 201"));
    assert!(response.contains(r#"{"quantity":7}"#));
    assert!(next_response.starts_with("HTTP/1.1 201"));
    assert!(next_response.contains(r#"{"quantity":8}"#));
    assert!(remaining_response.starts_with("HTTP/1.1 201"));
    assert!(remaining_response.contains(r#"{"remaining":3}"#));
    assert!(neg_response.starts_with("HTTP/1.1 201"));
    assert!(neg_response.contains(r#"{"quantity":-7}"#));
    assert!(cents_response.starts_with("HTTP/1.1 201"));
    assert!(cents_response.contains(r#"{"cents":700}"#));
    assert!(cents_total_response.starts_with("HTTP/1.1 201"));
    assert!(cents_total_response.contains(r#"{"cents":25000}"#));
    assert!(total_response.starts_with("HTTP/1.1 201"));
    assert!(total_response.contains(r#"{"total":875}"#));
    assert!(total_with_fee_response.starts_with("HTTP/1.1 201"));
    assert!(total_with_fee_response.contains(r#"{"total":900}"#));
    assert!(scaled_product_fee_response.starts_with("HTTP/1.1 201"));
    assert!(scaled_product_fee_response.contains(r#"{"total":87525}"#));
    assert!(product_plus_static_fee_response.starts_with("HTTP/1.1 201"));
    assert!(product_plus_static_fee_response.contains(r#"{"total":900}"#));
    assert!(product_plus_product_fee_response.starts_with("HTTP/1.1 201"));
    assert!(product_plus_product_fee_response.contains(r#"{"total":975}"#));
    assert!(triple_product_fee_response.starts_with("HTTP/1.1 201"));
    assert!(triple_product_fee_response.contains(r#"{"total":1775}"#));
    assert!(static_minus_product_response.starts_with("HTTP/1.1 201"));
    assert!(static_minus_product_response.contains(r#"{"remaining":125}"#));
    assert!(bundles_response.starts_with("HTTP/1.1 201"));
    assert!(bundles_response.contains(r#"{"bundles":2}"#));
    assert!(remainder_product_left_response.starts_with("HTTP/1.1 201"));
    assert!(remainder_product_left_response.contains(r#"{"remainder":75}"#));
    assert!(power_response.starts_with("HTTP/1.1 201"));
    assert!(power_response.contains(r#"{"total":64}"#));
    assert!(invalid_power_response.starts_with("HTTP/1.1 500"));
    assert!(
        invalid_power_response.contains(r#"{"error":"native request body int arithmetic failed"}"#)
    );
    assert!(due_response.starts_with("HTTP/1.1 201"));
    assert!(due_response.contains(r#"{"due":750}"#));
    assert!(share_response.starts_with("HTTP/1.1 201"));
    assert!(share_response.contains(r#"{"share":125}"#));
    assert!(unit_bundle_response.starts_with("HTTP/1.1 201"));
    assert!(unit_bundle_response.contains(r#"{"unit":5}"#));
    assert!(unit_bundle_left_response.starts_with("HTTP/1.1 201"));
    assert!(unit_bundle_left_response.contains(r#"{"unit":250}"#));
    assert!(remainder_response.starts_with("HTTP/1.1 201"));
    assert!(remainder_response.contains(r#"{"remainder":5}"#));
    assert!(remainder_scaled_response.starts_with("HTTP/1.1 201"));
    assert!(remainder_scaled_response.contains(r#"{"remainder":11}"#));
    assert!(remainder_scaled_left_response.starts_with("HTTP/1.1 201"));
    assert!(remainder_scaled_left_response.contains(r#"{"remainder":2}"#));
    assert!(available_response.starts_with("HTTP/1.1 201"));
    assert!(available_response.contains(r#"{"available":true}"#));
    assert!(available_bulk_response.starts_with("HTTP/1.1 201"));
    assert!(available_bulk_response.contains(r#"{"available":true}"#));
    assert!(covered_min_response.starts_with("HTTP/1.1 201"));
    assert!(covered_min_response.contains(r#"{"covered":true}"#));
    assert!(covered_total_response.starts_with("HTTP/1.1 201"));
    assert!(covered_total_response.contains(r#"{"covered":true}"#));
    assert!(product_covered_static_response.starts_with("HTTP/1.1 201"));
    assert!(product_covered_static_response.contains(r#"{"covered":true}"#));
    assert!(product_covered_product_response.starts_with("HTTP/1.1 201"));
    assert!(product_covered_product_response.contains(r#"{"covered":true}"#));

    drop(child);
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn generated_native_server_serves_request_body_float_cast_response() {
    let dir = temp_output_dir("native-request-body-float-cast-server-source");
    std::fs::create_dir_all(&dir).expect("create source dir");
    let path = dir.join("app.orv");
    std::fs::write(
            &path,
            r#"@server {
  @listen 8080
  @route POST /payments {
    @respond 201 { amount: @body.amount as float }
  }
  @route POST /payments/refund {
    @respond 201 { amount: -(@body.amount as float) }
  }
  @route POST /payments/remaining {
    @respond 201 { remaining: 100.5 - (@body.amount as float) }
  }
  @route POST /payments/total {
    @respond 201 { total: (@body.price as float) * (@body.quantity as float) }
  }
  @route POST /payments/total-plus-fee {
    @respond 201 { total: ((@body.price as float) * (@body.quantity as float)) + 1.25 }
  }
  @route POST /payments/scaled-product-fee {
    @respond 201 { total: (@body.base as float) + (((@body.price as float) * (@body.quantity as float)) * 0.5) }
  }
  @route POST /payments/power {
    @respond 201 { total: (@body.base as float) ** (@body.exp as float) }
  }
  @route POST /payments/under-limit {
    @respond 201 { under_limit: (@body.amount as float) <= (@query.limit as float) }
  }
  @route POST /payments/product-under-static-limit {
    @respond 201 { under_limit: ((@body.price as float) * (@body.quantity as float)) <= 40.0 }
  }
  @route POST /payments/product-plus-product-fee {
    @respond 201 { total: ((@body.price as float) * (@body.quantity as float)) + ((@body.fee as float) * (@body.fee_units as float)) }
  }
  @route POST /payments/triple-product-fee {
    @respond 201 { total: (@body.base as float) + (((@body.price as float) * (@body.quantity as float)) * (@body.multiplier as float)) }
  }
  @route POST /payments/product-under-product-limit {
    @respond 201 { under_limit: ((@body.price as float) * (@body.quantity as float)) <= ((@body.limit_price as float) * (@body.limit_units as float)) }
  }
}
"#,
        )
        .expect("write source");
    let out = temp_output_dir("native-request-body-float-cast-server-build");

    cmd_build(&path, &out).expect("build artifacts");
    cmd_verify_build(&out).expect("verify float cast native server build");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = std::process::Command::new(cargo)
        .arg("build")
        .arg("--manifest-path")
        .arg(out.join("server").join("native").join("Cargo.toml"))
        .arg("--release")
        .arg("--color")
        .arg("never")
        .output()
        .expect("cargo build float cast native server");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "float cast native server cargo build failed:\n{stderr}"
    );

    let binary = out
        .join("server")
        .join("native")
        .join("target")
        .join("release")
        .join("orv-native-server");
    let mut child = std::process::Command::new(&binary)
        .env("ORV_BUILD_DIR", &out)
        .env("ORV_HOST", "127.0.0.1")
        .env("ORV_PORT", "0")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn generated float cast native server");
    let stderr = child.stderr.take().expect("native server stderr");
    let child = ChildGuard(child);
    let mut stderr = std::io::BufReader::new(stderr);
    let mut line = String::new();
    std::io::BufRead::read_line(&mut stderr, &mut line).expect("native server listen line");
    let address = line
        .trim()
        .strip_prefix("orv native server listening on ")
        .expect("native listen address");

    let response = send_raw_http_json_post(address, "/payments", r#"{"amount":"12.5"}"#);
    let refund_response =
        send_raw_http_json_post(address, "/payments/refund", r#"{"amount":"12.5"}"#);
    let remaining_response =
        send_raw_http_json_post(address, "/payments/remaining", r#"{"amount":"12.5"}"#);
    let total_response = send_raw_http_json_post(
        address,
        "/payments/total",
        r#"{"price":"12.5","quantity":"3"}"#,
    );
    let total_plus_fee_response = send_raw_http_json_post(
        address,
        "/payments/total-plus-fee",
        r#"{"price":"12.5","quantity":"3"}"#,
    );
    let scaled_product_fee_response = send_raw_http_json_post(
        address,
        "/payments/scaled-product-fee",
        r#"{"base":"1.25","price":"12.5","quantity":"3"}"#,
    );
    let power_response =
        send_raw_http_json_post(address, "/payments/power", r#"{"base":"2.5","exp":"2.0"}"#);
    let under_limit_response = send_raw_http_json_post(
        address,
        "/payments/under-limit?limit=20.0",
        r#"{"amount":"12.5"}"#,
    );
    let product_under_static_limit_response = send_raw_http_json_post(
        address,
        "/payments/product-under-static-limit",
        r#"{"price":"12.5","quantity":"3"}"#,
    );
    let product_plus_product_fee_response = send_raw_http_json_post(
        address,
        "/payments/product-plus-product-fee",
        r#"{"price":"12.5","quantity":"3","fee":"1.25","fee_units":"2"}"#,
    );
    let triple_product_fee_response = send_raw_http_json_post(
        address,
        "/payments/triple-product-fee",
        r#"{"base":"1.25","price":"12.5","quantity":"3","multiplier":"2.0"}"#,
    );
    let product_under_product_limit_response = send_raw_http_json_post(
        address,
        "/payments/product-under-product-limit",
        r#"{"price":"12.5","quantity":"3","limit_price":"20.0","limit_units":"2"}"#,
    );

    let assert_created = |response: &str, body: &str| {
        assert!(response.starts_with("HTTP/1.1 201"));
        assert!(response.contains(body));
    };
    assert_created(&response, r#"{"amount":12.5}"#);
    assert_created(&refund_response, r#"{"amount":-12.5}"#);
    assert_created(&remaining_response, r#"{"remaining":88}"#);
    assert_created(&total_response, r#"{"total":37.5}"#);
    assert_created(&total_plus_fee_response, r#"{"total":38.75}"#);
    assert_created(&scaled_product_fee_response, r#"{"total":20}"#);
    assert_created(&power_response, r#"{"total":6.25}"#);
    assert_created(&under_limit_response, r#"{"under_limit":true}"#);
    assert_created(
        &product_under_static_limit_response,
        r#"{"under_limit":true}"#,
    );
    assert_created(&product_plus_product_fee_response, r#"{"total":40}"#);
    assert_created(&triple_product_fee_response, r#"{"total":76.25}"#);
    assert_created(
        &product_under_product_limit_response,
        r#"{"under_limit":true}"#,
    );

    drop(child);
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn generated_native_server_serves_route_and_query_numeric_cast_responses() {
    let dir = temp_output_dir("native-param-query-cast-server-source");
    std::fs::create_dir_all(&dir).expect("create source dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        r#"@server {
  @listen 8080
  @route GET /products/:id.json {
    @respond 200 { id: @param.id as int }
  }
  @route GET /products/:id {
    @respond 200 { id: @param.id as int }
  }
  @route GET /products/:id/math {
    @respond 200 {
      prev: (@param.id as int) - 1,
      doubled: (@param.id as int) * 2,
      half: (@param.id as int) / 2,
      parity: (@param.id as int) % 2
    }
  }
  @route GET /products/:id/shift/:offset {
    @respond 200 { shifted: (@param.id as int) + (@param.offset as int) }
  }
  @route GET /products/:price/float-math/:tax {
    @respond 200 {
      discounted: (@param.price as float) * 0.5,
      taxed: (@param.price as float) + (@param.tax as float)
    }
  }
  @route GET /products/:id/mixed {
    @respond 200 {
      kind: "calc",
      next_id: (@param.id as int) + 1,
      prev_page: (@query.page as int) - 1
    }
  }
  @route GET /search {
    @respond 200 { page: @query.page as float }
  }
  @route GET /search/next {
    @respond 200 { next: (@query.page as int) + 1 }
  }
  @route GET /search/step {
    @respond 200 { next: (@query.page as int) + (@query.step as int) }
  }
  @route GET /search/math {
    @respond 200 {
      prev: (@query.page as int) - 1,
      doubled: (@query.page as int) * 2,
      half: (@query.page as int) / 2,
      parity: (@query.page as int) % 2
    }
  }
  @route GET /search/float-total {
    @respond 200 { total: (@query.amount as float) * (@query.quantity as float) }
  }
  @route GET /search/float-ratio {
    @respond 200 { ratio: 100.0 / (@query.parts as float) }
  }
}
"#,
    )
    .expect("write source");
    let out = temp_output_dir("native-param-query-cast-server-build");

    cmd_build(&path, &out).expect("build artifacts");
    cmd_verify_build(&out).expect("verify param/query cast native server build");
    let launcher = std::fs::read_to_string(out.join("server").join("native").join("main.rs"))
        .expect("native launcher");
    assert!(launcher.contains("fn orv_native_serve() -> std::io::Result<()>"));
    assert!(!launcher.contains(r#"std::process::Command::new("orv")"#));
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = std::process::Command::new(cargo)
        .arg("build")
        .arg("--manifest-path")
        .arg(out.join("server").join("native").join("Cargo.toml"))
        .arg("--release")
        .arg("--color")
        .arg("never")
        .output()
        .expect("cargo build param/query cast native server");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "param/query cast native server cargo build failed:\n{stderr}"
    );

    let binary = out
        .join("server")
        .join("native")
        .join("target")
        .join("release")
        .join("orv-native-server");
    let mut child = std::process::Command::new(&binary)
        .env("ORV_BUILD_DIR", &out)
        .env("ORV_HOST", "127.0.0.1")
        .env("ORV_PORT", "0")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn generated param/query cast native server");
    let stderr = child.stderr.take().expect("native server stderr");
    let child = ChildGuard(child);
    let mut stderr = std::io::BufReader::new(stderr);
    let mut line = String::new();
    std::io::BufRead::read_line(&mut stderr, &mut line).expect("native server listen line");
    let address = line
        .trim()
        .strip_prefix("orv native server listening on ")
        .expect("native listen address");

    let route_response = send_raw_http(address, "/products/42");
    let route_suffix_response = send_raw_http(address, "/products/42.json");
    let route_math_response = send_raw_http(address, "/products/13/math");
    let route_shift_response = send_raw_http(address, "/products/13/shift/4");
    let route_float_response = send_raw_http(address, "/products/12.5/float-math/1.25");
    let route_mixed_response = send_raw_http(address, "/products/41/mixed?page=13");
    let query_response = send_raw_http(address, "/search?page=12.5");
    let next_response = send_raw_http(address, "/search/next?page=12");
    let step_response = send_raw_http(address, "/search/step?page=12&step=3");
    let math_response = send_raw_http(address, "/search/math?page=13");
    let float_total_response = send_raw_http(address, "/search/float-total?amount=12.5&quantity=3");
    let float_ratio_response = send_raw_http(address, "/search/float-ratio?parts=4");

    assert!(route_response.starts_with("HTTP/1.1 200"));
    assert!(route_response.contains(r#"{"id":42}"#));
    assert!(route_suffix_response.starts_with("HTTP/1.1 200"));
    assert!(route_suffix_response.contains(r#"{"id":42}"#));
    assert!(route_math_response.starts_with("HTTP/1.1 200"));
    assert!(route_math_response.contains(r#"{"prev":12,"doubled":26,"half":6,"parity":1}"#));
    assert!(route_shift_response.starts_with("HTTP/1.1 200"));
    assert!(route_shift_response.contains(r#"{"shifted":17}"#));
    assert!(route_float_response.starts_with("HTTP/1.1 200"));
    assert!(route_float_response.contains(r#"{"discounted":6.25,"taxed":13.75}"#));
    assert!(route_mixed_response.starts_with("HTTP/1.1 200"));
    assert!(route_mixed_response.contains(r#"{"kind":"calc","next_id":42,"prev_page":12}"#));
    assert!(query_response.starts_with("HTTP/1.1 200"));
    assert!(query_response.contains(r#"{"page":12.5}"#));
    assert!(next_response.starts_with("HTTP/1.1 200"));
    assert!(next_response.contains(r#"{"next":13}"#));
    assert!(step_response.starts_with("HTTP/1.1 200"));
    assert!(step_response.contains(r#"{"next":15}"#));
    assert!(math_response.starts_with("HTTP/1.1 200"));
    assert!(math_response.contains(r#"{"prev":12,"doubled":26,"half":6,"parity":1}"#));
    assert!(float_total_response.starts_with("HTTP/1.1 200"));
    assert!(float_total_response.contains(r#"{"total":37.5}"#));
    assert!(float_ratio_response.starts_with("HTTP/1.1 200"));
    assert!(float_ratio_response.contains(r#"{"ratio":25}"#));

    drop(child);
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn build_writes_cargo_checkable_native_launcher_package() {
    let path = workspace_path(&["fixtures", "e2e", "hello.orv"]);
    let out = temp_output_dir("native-server-cargo-check");

    cmd_build(&path, &out).expect("build artifacts");

    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = std::process::Command::new(cargo)
        .arg("check")
        .arg("--manifest-path")
        .arg(out.join("server").join("native").join("Cargo.toml"))
        .arg("--color")
        .arg("never")
        .output()
        .expect("cargo check native launcher");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "native launcher cargo check failed:\n{stderr}"
    );
    assert!(
        !stderr.contains("warning:"),
        "native launcher cargo check should be warning-free:\n{stderr}"
    );

    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn build_uses_reference_native_launcher_for_dynamic_handlers() {
    let dir = temp_output_dir("native-server-dynamic-fallback-source");
    std::fs::create_dir_all(&dir).expect("create source dir");
    let path = dir.join("app.orv");
    std::fs::write(
            &path,
            r"@server {
  @listen 8080
  @route POST /echo {
    @respond 201 { received: (@body.id as int) + ((((@body.bonus as int) * (@body.scale as int)) * (@body.extra as int)) * (@body.more as int)) }
  }
}
",
        )
        .expect("write source");
    let out = temp_output_dir("native-server-dynamic-fallback");

    cmd_build(&path, &out).expect("build artifacts");

    let source = std::fs::read_to_string(out.join("server").join("native").join("main.rs"))
        .expect("native source");
    let native_plan = read_json_value(&out.join(NATIVE_SERVER_PLAN_PATH)).expect("native plan");
    let image_plan =
        read_json_value(&out.join(NATIVE_RUNTIME_IMAGE_PLAN_PATH)).expect("image plan");
    assert_eq!(native_plan["status"], "planned");
    assert!(native_plan["blocked_by"]
        .as_array()
        .expect("blocked_by")
        .iter()
        .any(|item| item == "native-codegen"));
    assert_eq!(image_plan["status"], "planned");
    assert!(image_plan["blocked_by"]
        .as_array()
        .expect("blocked_by")
        .iter()
        .any(|item| item == "native-codegen"));
    assert!(source.contains("fn orv_native_reference_bridge("));
    assert!(source.contains(r#"std::process::Command::new("orv")"#));
    assert!(source.contains(r#".arg("run-artifact")"#));
    assert!(!source.contains("fn orv_native_serve() -> std::io::Result<()>"));
    cmd_verify_build(&out).expect("verify dynamic fallback build");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = std::process::Command::new(cargo)
        .arg("check")
        .arg("--manifest-path")
        .arg(out.join("server").join("native").join("Cargo.toml"))
        .arg("--color")
        .arg("never")
        .output()
        .expect("cargo check dynamic fallback native launcher");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "dynamic fallback native launcher cargo check failed:\n{stderr}"
    );

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&out);
}

fn assert_manifest_artifact(path: &Path, kind: &str, artifact_path: &str) {
    let manifest = read_json_value(path).expect("build manifest");
    assert!(
        manifest["artifacts"]
            .as_array()
            .expect("manifest artifacts")
            .iter()
            .any(|artifact| artifact["kind"] == kind && artifact["path"] == artifact_path),
        "missing manifest artifact {kind}"
    );
}

fn assert_bundle_target(path: &Path, kind: &str, target_path: &str) {
    let plan = read_json_value(path).expect("bundle plan");
    assert!(
        plan["bundles"]
            .as_array()
            .expect("bundle targets")
            .iter()
            .any(|bundle| bundle["kind"] == kind && bundle["path"] == target_path),
        "missing bundle target {kind}"
    );
}

fn assert_client_loader_contract(loader: &str) {
    for expected in [
        "ORV_CLIENT_BOOTSTRAP",
        "sourceBundleUrl",
        "../source-bundle.json",
        "sourceBundleHash",
        "sourceFileCount",
        "manifestUrl",
        "loadClientManifest",
        "client manifest hash mismatch",
        "validateWasmBundle",
        "client wasm hash mismatch",
        "reactivePlanUrl",
        "loadReactivePlan",
        "embeddedReactivePlan",
        "embeddedReactivePlanHash",
        "loadEmbeddedReactivePlan",
        "validateReactivePlan",
        "client embedded reactive plan hash mismatch",
        "validateReactiveBindings",
        "client reactive plan hash mismatch",
        "client reactive plan initial_render binding mismatch",
        "client reactive plan signal_state binding mismatch",
        "client reactive plan signal_text binding mismatch",
        "client reactive plan signal_attr binding mismatch",
        "client reactive plan signal_event binding mismatch",
        "renderSignalTextBinding",
        "text_template",
        "renderSignalTextCondition",
        "text_condition",
        "signalTextBindingStateKeys",
        "signalTextBindingCursorKey",
        "state_keys",
        "renderSignalAttrBinding",
        "attr_template",
        "signalAttrBindingStateKeys",
        "signalAttrBindingCursorKey",
        "renderSignalAttrCondition",
        "attr_condition",
        "compareSignalAttrCondition",
        "decodeSignalConditionOperand",
        "createReactiveState",
        "bindReactiveDom",
        "bindReactiveAttrs",
        "bindReactiveEvents",
        "applySignalAction",
        "assign_add",
        "assign_sub",
        "assign_toggle",
        "assign_event_target_checked",
        "assign_event_target_value",
        "assign_event_target_value_float",
        "assign_event_target_value_int",
        "setSignal",
        "loadSourceBundle",
        "sourceFileCount",
        "fnv1a64",
        "source bundle hash mismatch",
        "runtimeFeatures",
        "WebAssembly.instantiate",
        "validateInitialRender",
        "initial_render",
        "client initial render hash mismatch",
        "orv_start",
        "orv_render_ptr",
        "orv_render_len",
        "TextDecoder",
        "#orv-root",
        "initialRenderMountHtml",
        "DOMParser",
        "root.innerHTML",
        "app.wasm",
        "orvReactiveSignals",
        "orvReactiveBindings",
        "orvReactiveDomBindings",
        "orvReactiveAttrBindings",
        "orvReactiveEventBindings",
        "__ORV_CLIENT_REACTIVE_STATE__",
        "__ORV_SET_SIGNAL__",
    ] {
        assert!(
            loader.contains(expected),
            "missing loader snippet {expected}"
        );
    }
}

fn client_loader_bootstrap_json(loader: &str) -> serde_json::Value {
    let start_marker = "Object.freeze(";
    let start = loader.find(start_marker).expect("bootstrap start") + start_marker.len();
    let end = loader[start..]
        .find(");\n\nconst manifestUrl")
        .expect("bootstrap end")
        + start;
    serde_json::from_str(&loader[start..end]).expect("bootstrap json")
}

#[test]
fn build_prod_writes_deploy_manifest_and_server_entrypoint() {
    let (src_dir, path) = prod_server_source("build-prod-source");
    let out = temp_output_dir("build-prod-artifacts");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");

    let deploy_manifest_path = out.join("deploy").join("manifest.json");
    let deploy_container_path = out.join("deploy").join("container.json");
    let deploy_dockerfile_path = out.join("deploy").join("Dockerfile");
    let deploy_compose_path = out.join("deploy").join("compose.yaml");
    let deploy_env_example_path = out.join("deploy").join("env.example");
    let deploy_runbook_path = out.join("deploy").join("README.md");
    let deploy_routes_path = out.join("deploy").join("routes.json");
    let deploy_smoke_test_path = out.join("deploy").join("smoke-test.sh");
    let deploy_preflight_path = out.join("deploy").join("preflight.json");
    let deploy_benchmark_evidence_path = out.join("deploy").join("benchmark-evidence.json");
    let server_entrypoint_path = out.join("deploy").join("server.sh");
    let native_server_plan_path = out.join("server").join("native-server.json");
    assert!(
        deploy_manifest_path.is_file(),
        "missing {}",
        deploy_manifest_path.display()
    );
    assert!(
        deploy_container_path.is_file(),
        "missing {}",
        deploy_container_path.display()
    );
    assert!(
        deploy_dockerfile_path.is_file(),
        "missing {}",
        deploy_dockerfile_path.display()
    );
    assert!(
        deploy_compose_path.is_file(),
        "missing {}",
        deploy_compose_path.display()
    );
    assert!(
        deploy_env_example_path.is_file(),
        "missing {}",
        deploy_env_example_path.display()
    );
    assert!(
        deploy_runbook_path.is_file(),
        "missing {}",
        deploy_runbook_path.display()
    );
    assert!(
        deploy_routes_path.is_file(),
        "missing {}",
        deploy_routes_path.display()
    );
    assert!(
        deploy_smoke_test_path.is_file(),
        "missing {}",
        deploy_smoke_test_path.display()
    );
    assert!(
        deploy_preflight_path.is_file(),
        "missing {}",
        deploy_preflight_path.display()
    );
    assert!(
        deploy_benchmark_evidence_path.is_file(),
        "missing {}",
        deploy_benchmark_evidence_path.display()
    );
    assert!(
        server_entrypoint_path.is_file(),
        "missing {}",
        server_entrypoint_path.display()
    );
    assert!(
        native_server_plan_path.is_file(),
        "missing {}",
        native_server_plan_path.display()
    );
    let deploy = read_json_value(&deploy_manifest_path).expect("deploy manifest");
    assert_eq!(deploy["schema_version"], 1);
    assert_eq!(deploy["profile"], "prod");
    assert_eq!(deploy["entry"], path.display().to_string());
    assert_eq!(deploy["source_bundle"], "source-bundle.json");
    assert_eq!(deploy["server"]["artifact"], "server/app.orv-runtime.json");
    assert_eq!(deploy["server"]["entrypoint"], "deploy/server.sh");
    assert_eq!(deploy["server"]["container"], "deploy/container.json");
    assert_eq!(deploy["server"]["dockerfile"], "deploy/Dockerfile");
    assert_eq!(deploy["server"]["compose"], "deploy/compose.yaml");
    assert_eq!(deploy["server"]["env_example"], "deploy/env.example");
    assert_eq!(deploy["server"]["runbook"], "deploy/README.md");
    assert_eq!(deploy["server"]["smoke_test"], "deploy/smoke-test.sh");
    assert_eq!(deploy["server"]["smoke_output"], "deploy/smoke-output.txt");
    assert_eq!(deploy["server"]["preflight"], "deploy/preflight.json");
    assert_eq!(
        deploy["server"]["benchmark_evidence"],
        "deploy/benchmark-evidence.json"
    );
    assert_eq!(deploy["server"]["native_plan"], "server/native-server.json");
    assert_eq!(
        deploy["server"]["native_runtime_image_plan"],
        "server/runtime-image.json"
    );
    assert_eq!(
        deploy["server"]["native_routes_source"],
        "server/native/routes.rs"
    );
    assert_eq!(
        deploy["server"]["native_router_source"],
        "server/native/router.rs"
    );
    assert_eq!(
        deploy["server"]["native_handlers_source"],
        "server/native/handlers.rs"
    );
    assert_eq!(
        deploy["server"]["runtime_image"],
        "ghcr.io/orv-lang/orv-reference:latest"
    );
    assert_eq!(deploy["server"]["listen"]["port"], 8080);
    assert!(deploy["server"]["routes"]
        .as_array()
        .expect("server routes")
        .iter()
        .any(|route| route["method"] == "GET" && route["path"] == "/ping"));
    assert_eq!(deploy["server"]["routes_artifact"], "deploy/routes.json");
    let container = read_json_value(&deploy_container_path).expect("deploy container");
    assert_eq!(container["schema_version"], 1);
    assert_eq!(container["kind"], "reference-server-container");
    assert_eq!(container["artifact"], "server/app.orv-runtime.json");
    assert_eq!(container["entrypoint"], "deploy/server.sh");
    assert_eq!(container["routes_artifact"], "deploy/routes.json");
    assert_eq!(container["dockerfile"], "deploy/Dockerfile");
    assert_eq!(container["runtime"], "reference-interpreter");
    assert_eq!(
        container["runtime_image"],
        deploy["server"]["runtime_image"]
    );
    assert_eq!(container["protocol"], "http1");
    assert_eq!(container["listen"], deploy["server"]["listen"]);
    assert_eq!(container["ports"][0]["container"], 8080);
    assert_eq!(container["ports"][0]["protocol"], "tcp");
    assert_eq!(container["command"][0], "./deploy/server.sh");
    let dockerfile = std::fs::read_to_string(&deploy_dockerfile_path).expect("Dockerfile");
    assert!(dockerfile.contains("ARG ORV_RUNTIME_IMAGE=ghcr.io/orv-lang/orv-reference:latest"));
    assert!(dockerfile.contains("FROM ${ORV_RUNTIME_IMAGE}"));
    assert!(dockerfile.contains("COPY . /app"));
    assert!(dockerfile.contains("EXPOSE 8080"));
    assert!(dockerfile.contains(r#"ENTRYPOINT ["./deploy/server.sh"]"#));
    let compose = std::fs::read_to_string(&deploy_compose_path).expect("compose");
    assert!(compose.contains("dockerfile: deploy/Dockerfile"));
    assert!(compose.contains("ORV_RUNTIME_IMAGE: ghcr.io/orv-lang/orv-reference:latest"));
    assert!(compose.contains(r#""8080:8080""#));
    assert!(compose.contains(r#"PORT: "8080""#));
    let env_example = std::fs::read_to_string(&deploy_env_example_path).expect("env example");
    assert!(env_example.contains("PORT=8080"));
    let runbook = std::fs::read_to_string(&deploy_runbook_path).expect("deploy runbook");
    assert!(runbook.contains("docker compose -f deploy/compose.yaml up --build -d"));
    assert!(runbook.contains("deploy/env.example"));
    assert!(runbook.contains("PORT=8080"));
    assert!(runbook.contains("cargo build --manifest-path server/native/Cargo.toml --release"));
    assert!(runbook.contains("ORV_BUILD_DIR=. ./server/native/target/release/orv-native-server"));
    assert!(
        runbook.contains("docker build -f server/native/Dockerfile -t orv-native-server:latest .")
    );
    assert!(runbook.contains("ORV_BUILD_DIR is an explicit override"));
    assert!(runbook.contains("./deploy/server.sh --trace deploy/request-trace.json"));
    assert!(runbook.contains("./deploy/smoke-test.sh"));
    assert!(runbook.contains("deploy/smoke-output.txt"));
    assert!(runbook.contains("deploy/preflight.json"));
    assert!(runbook.contains("deploy/benchmark-evidence.json"));
    assert!(runbook.contains("## Benchmark Evidence"));
    assert!(runbook.contains("## Smoke Output Markers"));
    assert!(runbook.contains("- `pass_marker`"));
    assert!(runbook.contains("- `dap_source_bundle`"));
    assert!(runbook.contains("- `trace_stream_requested`"));
    assert!(runbook.contains("orv verify-build ."));
    assert!(runbook.contains("orv deploy-env-check ."));
    assert!(runbook.contains("orv editor run-debug . --control next"));
    assert!(runbook.contains("orv benchmark-report ."));
    assert!(runbook.contains("orv benchmark-report . --require-pass"));
    assert!(runbook.contains("/__orv/trace/events"));
    assert!(runbook.contains("orv editor trace . --trace deploy/request-trace.json"));
    assert!(runbook.contains("ORV_SMOKE_TRACE_STREAM=1 ./deploy/smoke-test.sh"));
    assert!(runbook.contains("- GET /ping"));
    let routes = read_json_value(&deploy_routes_path).expect("deploy routes");
    assert_eq!(routes["schema_version"], 1);
    assert_eq!(routes["artifact"], "server/app.orv-runtime.json");
    assert!(json_routes_include(&routes["routes"], "GET", "/ping"));
    let smoke_test = std::fs::read_to_string(&deploy_smoke_test_path).expect("smoke test");
    assert!(smoke_test.contains(r#"BASE_URL="${ORV_BASE_URL:-http://127.0.0.1:8080}""#));
    assert!(smoke_test.contains("command -v curl"));
    assert!(smoke_test.contains("orv deploy smoke test requires curl"));
    assert!(
        smoke_test.contains(r#"ORV_SMOKE_OUTPUT="${ORV_SMOKE_OUTPUT:-deploy/smoke-output.txt}""#)
    );
    assert!(smoke_test.contains(r#"> "$ORV_SMOKE_OUTPUT""#));
    assert!(smoke_test.contains("orv_smoke_write_output()"));
    assert!(smoke_test.contains("\norv_smoke_write_output\n"));
    assert!(smoke_test.contains("graph_contract=verified"));
    assert!(smoke_test.contains("dap_summary=verified"));
    assert!(smoke_test.contains("dap_source_bundle=verified"));
    assert!(smoke_test.contains(
        r#"orv_smoke_dap_summary_contains "dap source bundle panel" '"source_bundle": {'"#
    ));
    assert!(smoke_test.contains(
            r#"orv_smoke_dap_summary_contains "dap source bundle panel path" '"path": "./source-bundle.json"'"#
        ));
    assert!(smoke_test.contains(
        r#"orv_smoke_dap_summary_contains "dap source bundle panel file count" '"fileCount": 1'"#
    ));
    assert!(smoke_test
        .contains(r#"orv_smoke_dap_summary_contains "dap source bundle panel hash" '"hash":'"#));
    assert!(smoke_test.contains("orv_smoke_dap_summary_capture()"));
    assert!(smoke_test.contains("orv_smoke_dap_summary_cleanup()"));
    assert!(smoke_test.contains("\norv_smoke_dap_summary_cleanup\n"));
    assert!(smoke_test.contains(
        r#"orv_smoke_dap_summary_contains "dap smoke required markers" '"smoke_test_required_markers": ['"#
    ));
    assert!(smoke_test.contains(
        r#"orv_smoke_dap_summary_contains "dap smoke summary required markers" '"required_markers": ['"#
    ));
    assert!(smoke_test.contains(
        r#"orv_smoke_dap_summary_contains "dap smoke marker dap source bundle" '"dap_source_bundle"'"#
    ));
    assert!(smoke_test.contains("server_routes=1"));
    assert!(smoke_test.contains("trace_stream_requested=%s"));
    assert!(smoke_test.contains("orv_smoke_reveal_contains()"));
    assert!(smoke_test.contains("orv_smoke_editor_reveal_contains()"));
    assert!(smoke_test.contains("orv_smoke_lsp_reveal_contains()"));
    assert!(smoke_test.contains("lsp reveal"));
    assert!(smoke_test.contains("orv_smoke_trace_stream()"));
    assert!(smoke_test.contains("ORV_SMOKE_TRACE_STREAM"));
    assert!(smoke_test.contains("editor trace-stream"));
    assert!(smoke_test.contains("orv_smoke_curl()"));
    assert!(smoke_test.contains("orv_smoke_origin_header()"));
    assert!(smoke_test.contains("orv_smoke_response_origin_header()"));
    assert!(smoke_test.contains("orv_smoke_curl_origin()"));
    assert!(smoke_test.contains("orv_smoke_curl_origin_response()"));
    assert!(smoke_test.contains("orv deploy smoke test failed: %s"));
    assert!(smoke_test.contains(r#"READY_PATH="/ping""#));
    assert!(smoke_test.contains("for attempt in 1 2 3 4 5"));
    assert!(smoke_test.contains("sleep 1"));
    assert!(smoke_test.contains(r#"ORV_SMOKE_ORIGIN_GET_PING="ori_"#));
    assert!(smoke_test.contains(r#"ORV_SMOKE_RESPONSE_ORIGIN_GET_PING="ori_"#));
    assert!(smoke_test.contains(
            r#"orv_smoke_curl_origin_response "GET /ping" "$ORV_SMOKE_ORIGIN_GET_PING" "$ORV_SMOKE_RESPONSE_ORIGIN_GET_PING" "$BASE_URL/ping""#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_reveal_contains "reveal GET /ping response source" "$ORV_SMOKE_RESPONSE_ORIGIN_GET_PING" '@respond'"#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_reveal_contains "reveal GET /ping response production" "$ORV_SMOKE_RESPONSE_ORIGIN_GET_PING" '"response_origin_dispatch": true'"#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_reveal_contains "reveal GET /ping native target summary" "$ORV_SMOKE_ORIGIN_GET_PING" '"native_server_target_count": 1'"#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_editor_reveal_contains "editor reveal GET /ping native route summary" "$ORV_SMOKE_ORIGIN_GET_PING" '"native_server_route_count": 1'"#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_lsp_reveal_contains "lsp reveal GET /ping native target summary" "$ORV_SMOKE_ORIGIN_GET_PING" '"native_server_target_count": 1'"#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_dap_summary_contains "dap native target summary" '"native_server_target_count": 1'"#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_dap_summary_contains "dap native route summary" '"native_server_route_count": 1'"#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_lsp_reveal_contains "lsp reveal GET /ping response origin" "$ORV_SMOKE_RESPONSE_ORIGIN_GET_PING" '"name": "respond"'"#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_lsp_reveal_contains "lsp reveal GET /ping response production" "$ORV_SMOKE_RESPONSE_ORIGIN_GET_PING" '"response_origin_dispatch": true'"#
        ));
    assert!(smoke_test.contains(
        r#"orv_smoke_reveal_contains "reveal smoke required markers" "$ORV_SMOKE_ORIGIN_GET_PING" '"smoke_test_required_markers": ['"#
    ));
    assert!(smoke_test.contains(
        r#"orv_smoke_reveal_contains "reveal smoke summary required markers" "$ORV_SMOKE_ORIGIN_GET_PING" '"required_markers": ['"#
    ));
    assert!(smoke_test.contains(
        r#"orv_smoke_reveal_contains "reveal smoke marker dap source bundle" "$ORV_SMOKE_ORIGIN_GET_PING" '"dap_source_bundle"'"#
    ));
    assert!(smoke_test.contains(
        r#"orv_smoke_editor_reveal_contains "editor reveal smoke required markers" "$ORV_SMOKE_ORIGIN_GET_PING" '"smoke_test_required_markers": ['"#
    ));
    assert!(smoke_test.contains(
        r#"orv_smoke_lsp_reveal_contains "lsp reveal smoke required markers" "$ORV_SMOKE_ORIGIN_GET_PING" '"smoke_test_required_markers": ['"#
    ));
    let preflight = read_json_value(&deploy_preflight_path).expect("deploy preflight");
    assert_eq!(preflight["schema_version"], 1);
    assert_eq!(preflight["kind"], "orv.deploy.preflight");
    assert_eq!(preflight["artifact"], "server/app.orv-runtime.json");
    assert_eq!(preflight["artifacts"]["smoke_test"], "deploy/smoke-test.sh");
    assert_eq!(
        preflight["artifacts"]["smoke_output"],
        "deploy/smoke-output.txt"
    );
    assert_eq!(preflight["artifacts"]["preflight"], "deploy/preflight.json");
    assert_eq!(
        preflight["artifacts"]["benchmark_evidence"],
        "deploy/benchmark-evidence.json"
    );
    assert_eq!(preflight["artifacts"]["source_bundle"], SOURCE_BUNDLE_PATH);
    assert_eq!(
        preflight["artifacts"]["project_graph"],
        "project-graph.json"
    );
    assert_eq!(preflight["artifacts"]["origin_map"], "origin-map.json");
    assert_eq!(
        preflight["artifacts"]["build_manifest"],
        "build-manifest.json"
    );
    assert_eq!(preflight["artifacts"]["bundle_plan"], "bundle-plan.json");
    assert_eq!(preflight["commands"]["verify_build"], "orv verify-build .");
    assert_eq!(preflight["commands"]["env_check"], "orv deploy-env-check .");
    assert_eq!(preflight["commands"]["run_build"], "orv run-build .");
    assert_eq!(
        preflight["commands"]["trace_run_build"],
        "orv run-build . --trace deploy/request-trace.json"
    );
    assert_eq!(
        preflight["commands"]["smoke_test"],
        "./deploy/smoke-test.sh"
    );
    assert_eq!(
        preflight["commands"]["editor_run_debug"],
        "orv editor run-debug . --control next"
    );
    assert_eq!(
        preflight["commands"]["benchmark_report"],
        "orv benchmark-report ."
    );
    assert_eq!(
        preflight["commands"]["benchmark_report_require_pass"],
        "orv benchmark-report . --require-pass"
    );
    assert_eq!(
        preflight["commands"]["trace_stream_smoke"],
        "ORV_SMOKE_TRACE_STREAM=1 ./deploy/smoke-test.sh"
    );
    assert_eq!(
        preflight["commands"]["compose_up"],
        "docker compose -f deploy/compose.yaml up --build -d"
    );
    assert_eq!(preflight["listen"], deploy["server"]["listen"]);
    assert_eq!(preflight["routes"], deploy["server"]["routes"]);
    assert_eq!(
        preflight["runtime_features"],
        deploy["server"]["runtime_features"]
    );
    let evidence = read_json_value(&deploy_benchmark_evidence_path).expect("benchmark evidence");
    assert_eq!(evidence["schema_version"], 1);
    assert_eq!(evidence["kind"], "orv.benchmark.shop_5h.evidence");
    assert_eq!(evidence["preflight"], "deploy/preflight.json");
    assert!(evidence["preflight_hash"].as_str().is_some());
    assert_eq!(evidence["benchmark"], preflight["benchmark"]);
    assert_eq!(evidence["commands"], preflight["commands"]);
    assert_eq!(evidence["artifacts"], preflight["artifacts"]);
    assert_eq!(evidence["recording_status"], "not_recorded");
    assert_eq!(
        evidence["task_entries"]
            .as_array()
            .expect("benchmark tasks")
            .len(),
        10
    );
    assert_eq!(
        evidence["data"]["elapsed_time_per_task"],
        "task_entries[*].elapsed_minutes"
    );
    assert!(evidence["data"]
        .as_object()
        .expect("benchmark data")
        .contains_key("smoke_test_output"));
    let script = std::fs::read_to_string(&server_entrypoint_path).expect("server entrypoint");
    assert!(script.contains("orv run-artifact"));

    cmd_verify_build(&out).expect("verify prod build");
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn build_prod_runbook_documents_client_bundle_contract() {
    let dir = temp_output_dir("build-prod-client-runbook-source");
    std::fs::create_dir_all(&dir).expect("create temp root");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        r#"@server {
  @listen 8080
  @route GET /ping {
    @respond 200 { ok: true }
  }
}

let sig count: int = 0
@out @html { @body { @p count } }
"#,
    )
    .expect("write source");
    let out = temp_output_dir("build-prod-client-runbook");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let runbook_path = out.join("deploy").join("README.md");
    let runbook = std::fs::read_to_string(&runbook_path).expect("deploy runbook");

    assert!(runbook.contains("## Client Bundle"));
    assert!(runbook.contains("- Client manifest: client/manifest.json"));
    assert!(runbook.contains("- Client reactive plan: client/reactive-plan.json"));
    assert!(runbook.contains("- Client page: pages/index.html"));
    assert!(runbook.contains("- Client loader: client/app.js"));
    assert!(runbook.contains("- Client WASM: client/app.wasm"));
    assert!(runbook.contains("- Client runtime: client_wasm"));
    assert!(runbook.contains("signal_text"));
    assert!(runbook.contains("dynamic-client-codegen"));
    cmd_verify_build(&out).expect("verify client runbook");

    write_text(
        &runbook_path,
        &runbook.replace("signal_text", "signal_slot"),
    )
    .expect("write corrupt runbook");
    let err = cmd_verify_build(&out).expect_err("client runbook mismatch");
    assert!(
        err.to_string()
            .contains("deploy runbook must document client capability surface signal_text"),
        "{err}"
    );
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(out);
}

#[test]
fn build_prod_smoke_test_documents_client_bundle_contract() {
    let dir = temp_output_dir("build-prod-client-smoke-source");
    std::fs::create_dir_all(&dir).expect("create temp root");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        r#"@server {
  @listen 8080
  @route GET /ping {
    @respond 200 { ok: true }
  }
}

let sig count: int = 0
@out @html { @body { @p count } }
"#,
    )
    .expect("write source");
    let out = temp_output_dir("build-prod-client-smoke");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let smoke_path = out.join("deploy").join("smoke-test.sh");
    let smoke = std::fs::read_to_string(&smoke_path).expect("deploy smoke test");

    assert!(smoke.contains("ORV_SMOKE_BUILD_DIR="));
    assert!(smoke.contains(r#"cd "$ORV_SMOKE_BUILD_DIR""#));
    assert!(smoke.contains("orv_smoke_file()"));
    assert!(smoke.contains("orv_smoke_grep()"));
    assert!(smoke.contains("orv_smoke_write_output()"));
    assert!(smoke.contains("graph_contract=verified"));
    assert!(smoke.contains("dap_summary=verified"));
    assert!(smoke.contains("dap_source_bundle=verified"));
    assert!(smoke.contains(
        r#"orv_smoke_dap_summary_contains "dap source bundle panel" '"source_bundle": {'"#
    ));
    assert!(smoke.contains(
            r#"orv_smoke_dap_summary_contains "dap source bundle panel path" '"path": "./source-bundle.json"'"#
        ));
    assert!(smoke.contains(
        r#"orv_smoke_dap_summary_contains "dap source bundle panel file count" '"fileCount": 1'"#
    ));
    assert!(smoke
        .contains(r#"orv_smoke_dap_summary_contains "dap source bundle panel hash" '"hash":'"#));
    assert!(smoke.contains("server_routes=1"));
    assert!(smoke.contains("trace_stream_requested=%s"));
    assert!(smoke.contains(r#"orv_smoke_file "client/manifest.json""#));
    assert!(smoke.contains(r#"orv_smoke_file "client/reactive-plan.json""#));
    assert!(smoke.contains(r#"orv_smoke_file "pages/index.html""#));
    assert!(smoke.contains(r#"orv_smoke_file "client/app.js""#));
    assert!(smoke.contains(r#"orv_smoke_file "client/app.wasm""#));
    assert!(smoke.contains(
        r#"orv_smoke_grep "client page marker" "pages/index.html" 'data-orv-client="wasm"'"#
    ));
    assert!(smoke.contains(
            r#"orv_smoke_grep "client manifest reactive plan path" "client/manifest.json" '"reactive_plan": "client/reactive-plan.json"'"#
        ));
    assert!(smoke.contains("client_manifest=client/manifest.json"));
    assert!(smoke.contains("client_reactive_plan=client/reactive-plan.json"));
    assert!(smoke.contains("client_page=pages/index.html"));
    assert!(smoke.contains("client_loader=client/app.js"));
    assert!(smoke.contains("client_wasm=client/app.wasm"));
    assert!(smoke.contains(r#"ORV_SMOKE_CLIENT_ORIGIN="ori_"#));
    assert!(smoke.contains(
            r#"orv_smoke_grep "client manifest reactive plan hash" "client/manifest.json" '"reactive_plan_hash"'"#
        ));
    assert!(smoke.contains(
        r#"orv_smoke_grep "client manifest loader hash" "client/manifest.json" '"loader_hash"'"#
    ));
    assert!(smoke.contains(
        r#"orv_smoke_grep "client manifest wasm hash" "client/manifest.json" '"wasm_hash"'"#
    ));
    assert!(smoke.contains(
            r#"orv_smoke_grep "client manifest source bundle" "client/manifest.json" '"source_bundle": "source-bundle.json"'"#
        ));
    assert!(smoke.contains(
            r#"orv_smoke_grep "client manifest runtime" "client/manifest.json" '"runtime": "client_wasm"'"#
        ));
    assert!(smoke.contains(
        r#"orv_smoke_grep "client manifest capabilities" "client/manifest.json" '"capabilities"'"#
    ));
    assert!(smoke.contains(
            r#"orv_smoke_grep "client manifest capability surfaces" "client/manifest.json" '"surfaces"'"#
        ));
    assert!(smoke.contains(
        r#"orv_smoke_grep "client manifest event actions" "client/manifest.json" '"event_actions"'"#
    ));
    assert!(smoke.contains(
            r#"orv_smoke_grep "client reactive plan kind" "client/reactive-plan.json" '"kind": "orv.client.reactive_plan"'"#
        ));
    assert!(smoke.contains(
            r#"orv_smoke_grep "client reactive plan source bundle" "client/reactive-plan.json" '"source_bundle": "source-bundle.json"'"#
        ));
    assert!(smoke.contains(
            r#"orv_smoke_grep "client reactive plan blocked_by" "client/reactive-plan.json" '"blocked_by"'"#
        ));
    assert!(smoke.contains(
        r#"orv_smoke_grep "client loader bootstrap" "client/app.js" 'ORV_CLIENT_BOOTSTRAP'"#
    ));
    assert!(smoke.contains(
            r#"orv_smoke_grep "client loader embedded reactive plan" "client/app.js" 'embeddedReactivePlan'"#
        ));
    assert!(smoke.contains(
            r#"orv_smoke_grep "client loader embedded reactive plan hash" "client/app.js" 'embeddedReactivePlanHash'"#
        ));
    assert!(smoke.contains(
        r#"orv_smoke_grep "client loader source bundle hash" "client/app.js" 'sourceBundleHash'"#
    ));
    assert!(smoke
        .contains(r#"orv_smoke_grep "client loader wasm reference" "client/app.js" 'app.wasm'"#));
    assert!(smoke.contains(
        r#"orv_smoke_grep "client loader signal setter" "client/app.js" '__ORV_SET_SIGNAL__'"#
    ));
    assert!(smoke.contains(
            r#"orv_smoke_reveal_contains "reveal client target summary" "$ORV_SMOKE_CLIENT_ORIGIN" '"client_target_count": 5'"#
        ));
    assert!(smoke.contains(
            r#"orv_smoke_reveal_contains "reveal client manifest target" "$ORV_SMOKE_CLIENT_ORIGIN" '"path": "client/manifest.json"'"#
        ));
    assert!(smoke.contains(
            r#"orv_smoke_editor_reveal_contains "editor reveal client manifest summary" "$ORV_SMOKE_CLIENT_ORIGIN" '"client_manifest_count": 1'"#
        ));
    assert!(smoke.contains(
            r#"orv_smoke_lsp_reveal_contains "lsp reveal client capability summary" "$ORV_SMOKE_CLIENT_ORIGIN" '"client_capability_surface_count"'"#
        ));
    assert!(smoke.contains(
        r#"orv_smoke_dap_summary_contains "dap client target summary" '"client_target_count": 5'"#
    ));
    assert!(smoke.contains(
            r#"orv_smoke_dap_summary_contains "dap client manifest summary" '"client_manifest_count": 1'"#
        ));
    assert!(smoke.contains(
            r#"orv_smoke_dap_summary_contains "dap client capability summary" '"client_capability_surface_count"'"#
        ));
    cmd_verify_build(&out).expect("verify client smoke test");

    write_text(
        &smoke_path,
        &smoke.replace(
            r#""reveal client target summary""#,
            r#""reveal client summary""#,
        ),
    )
    .expect("write corrupt smoke test");
    let err = cmd_verify_build(&out).expect_err("client reveal smoke mismatch");
    assert!(
        err.to_string()
            .contains("deploy smoke test must include orv_smoke_reveal_contains"),
        "{err}"
    );
    write_text(&smoke_path, &smoke).expect("restore smoke test");

    write_text(
        &smoke_path,
        &smoke.replace("ORV_CLIENT_BOOTSTRAP", "ORV_CLIENT_BOOT"),
    )
    .expect("write corrupt smoke test");
    let err = cmd_verify_build(&out).expect_err("client smoke test mismatch");
    assert!(
        err.to_string()
            .contains(r#"deploy smoke test must include orv_smoke_grep "client loader bootstrap""#),
        "{err}"
    );
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(out);
}

#[test]
fn build_prod_mounts_file_db_connect_adapter_wal() {
    let dir = temp_output_dir("build-prod-file-db-connect-source");
    std::fs::create_dir_all(&dir).expect("create file db source dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        r#"@server {
  @listen 8080
  let appdb = @db.connect "file://data/app.wal.jsonl"
  @route GET /ping { @respond 200 { ok: true } }
}
"#,
    )
    .expect("write file db source");
    let out = temp_output_dir("build-prod-file-db-connect");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");

    let manifest = read_json_value(&out.join("build-manifest.json")).expect("manifest");
    let deploy = read_json_value(&out.join("deploy").join("manifest.json")).expect("deploy");
    let runtime =
        read_json_value(&out.join("server").join("app.orv-runtime.json")).expect("runtime");
    let container = read_json_value(&out.join("deploy").join("container.json")).expect("container");
    let compose =
        std::fs::read_to_string(out.join("deploy").join("compose.yaml")).expect("compose");
    let runbook = std::fs::read_to_string(out.join("deploy").join("README.md")).expect("runbook");

    assert_eq!(
        deploy["server"]["persistence"]["wal_paths"][0],
        serde_json::json!("data/app.wal.jsonl")
    );
    assert!(manifest["capabilities"]["runtime_features"]
        .as_array()
        .expect("runtime features")
        .iter()
        .any(|feature| feature == "db_adapter"));
    assert!(runtime["runtime_features"]
        .as_array()
        .expect("runtime features")
        .iter()
        .any(|feature| feature == "db_adapter"));
    assert!(deploy["server"]["runtime_features"]
        .as_array()
        .expect("runtime features")
        .iter()
        .any(|feature| feature == "db_adapter"));
    assert_eq!(
        container["persistence"]["volumes"][0]["host"],
        serde_json::json!("data")
    );
    assert!(compose.contains("../data:/app/data"));
    assert!(runbook.contains("- WAL: data/app.wal.jsonl"));
    assert!(runbook.contains("- Compose volume: ../data:/app/data"));
    cmd_verify_build(&out).expect("verify prod build");
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(out);
}

#[test]
fn build_prod_mounts_sqlite_db_connect_adapter_file() {
    let dir = temp_output_dir("build-prod-sqlite-db-connect-source");
    std::fs::create_dir_all(&dir).expect("create sqlite db source dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        r#"@server {
  @listen 8080
  let appdb = @db.connect "sqlite://data/app.sqlite"
  @route GET /ping { @respond 200 { ok: true } }
}
"#,
    )
    .expect("write sqlite db source");
    let out = temp_output_dir("build-prod-sqlite-db-connect");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");

    let deploy = read_json_value(&out.join("deploy").join("manifest.json")).expect("deploy");
    let container = read_json_value(&out.join("deploy").join("container.json")).expect("container");
    let compose =
        std::fs::read_to_string(out.join("deploy").join("compose.yaml")).expect("compose");
    let runbook = std::fs::read_to_string(out.join("deploy").join("README.md")).expect("runbook");

    assert_eq!(
        deploy["server"]["persistence"]["db_paths"],
        serde_json::json!(["data/app.sqlite"])
    );
    assert_eq!(
        container["persistence"]["volumes"][0]["host"],
        serde_json::json!("data")
    );
    assert!(compose.contains("../data:/app/data"));
    assert!(runbook.contains("- DB: data/app.sqlite"));
    cmd_verify_build(&out).expect("verify prod build");
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(out);
}

#[test]
fn build_prod_records_env_configured_sqlite_db_adapter() {
    let dir = temp_output_dir("build-prod-env-sqlite-db-connect-source");
    std::fs::create_dir_all(&dir).expect("create env sqlite db source dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        r#"@server {
  @listen 8080
  let appdb = @db.connect(@env.SHOP_DATABASE_URL ?? "sqlite://data/app.sqlite")
  @route GET /ping { @respond 200 { ok: true } }
}
"#,
    )
    .expect("write env sqlite db source");
    let out = temp_output_dir("build-prod-env-sqlite-db-connect");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");

    let deploy = read_json_value(&out.join("deploy").join("manifest.json")).expect("deploy");
    let container = read_json_value(&out.join("deploy").join("container.json")).expect("container");
    let compose =
        std::fs::read_to_string(out.join("deploy").join("compose.yaml")).expect("compose");
    let runbook = std::fs::read_to_string(out.join("deploy").join("README.md")).expect("runbook");

    assert_eq!(
        deploy["server"]["persistence"]["db_paths"],
        serde_json::json!(["data/app.sqlite"])
    );
    assert_eq!(
        deploy["server"]["persistence"]["db_env"],
        serde_json::json!([
            {
                "env": "SHOP_DATABASE_URL",
                "default": "sqlite://data/app.sqlite"
            }
        ])
    );
    assert_eq!(
        container["persistence"]["db_env"],
        deploy["server"]["persistence"]["db_env"]
    );
    assert!(compose.contains("../data:/app/data"));
    assert!(
        compose.contains(r#"SHOP_DATABASE_URL: "${SHOP_DATABASE_URL:-sqlite://data/app.sqlite}""#)
    );
    assert!(runbook.contains("- DB: data/app.sqlite"));
    assert!(
        runbook.contains("- DB adapter env: SHOP_DATABASE_URL default sqlite://data/app.sqlite")
    );
    cmd_verify_build(&out).expect("verify prod build");
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(out);
}

#[test]
fn build_prod_records_external_db_adapter_endpoints_without_volumes() {
    let dir = temp_output_dir("build-prod-external-db-connect-source");
    std::fs::create_dir_all(&dir).expect("create external db source dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        r#"@server {
  @listen 8080
  let analytics = @db.connect "postgres://db.internal/shop"
  let shopdb = @db.connect(@env.SHOP_DATABASE_URL ?? "mysql://db.internal/shop")
  @route GET /ping { @respond 200 { ok: true } }
}
"#,
    )
    .expect("write external db source");
    let out = temp_output_dir("build-prod-external-db-connect");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");

    let deploy = read_json_value(&out.join("deploy").join("manifest.json")).expect("deploy");
    let container = read_json_value(&out.join("deploy").join("container.json")).expect("container");
    let db_adapters_path = out.join("deploy").join("db-adapters.json");
    let db_adapters = read_json_value(&db_adapters_path).expect("db adapters");
    let compose =
        std::fs::read_to_string(out.join("deploy").join("compose.yaml")).expect("compose");
    let env_example =
        std::fs::read_to_string(out.join("deploy").join("env.example")).expect("env example");
    let smoke_test =
        std::fs::read_to_string(out.join("deploy").join("smoke-test.sh")).expect("smoke test");
    let preflight = read_json_value(&out.join("deploy").join("preflight.json")).expect("preflight");
    let runbook = std::fs::read_to_string(out.join("deploy").join("README.md")).expect("runbook");

    assert_eq!(
        deploy["server"]["persistence"]["db_endpoints"],
        serde_json::json!(["mysql://db.internal/shop", "postgres://db.internal/shop"])
    );
    assert_eq!(deploy["server"]["db_adapters"], "deploy/db-adapters.json");
    assert_eq!(db_adapters["schema_version"], 1);
    assert_eq!(db_adapters["artifact"], "server/app.orv-runtime.json");
    let adapters = db_adapters["adapters"].as_array().expect("db adapters");
    assert_eq!(adapters.len(), 2);
    assert!(adapters.iter().all(|adapter| adapter["source_origin_id"]
        .as_str()
        .is_some_and(|origin_id| origin_id.starts_with("ori_"))));
    assert_eq!(
        adapter_values_without_source_origin_ids(&db_adapters["adapters"]),
        serde_json::json!([
            {
                "kind": "db",
                "mode": "external",
                "provider": "mysql",
                "env": "SHOP_DATABASE_URL",
                "default": "mysql://db.internal/shop",
                "endpoint": "mysql://db.internal/shop",
                "adapter_status": "unsupported_runtime",
                "runtime": {
                    "status": "unsupported_runtime",
                    "query_methods": ["create", "find", "update", "delete", "transaction"]
                },
                "bridge": {
                    "contract": "http-json-v1",
                    "method": "POST",
                    "content_type": "application/json",
                    "query_methods": [
                        "create",
                        "find",
                        "findAll",
                        "update",
                        "delete",
                        "upsert",
                        "search",
                        "count",
                        "sum",
                        "transaction",
                        "schema"
                    ],
                    "body": {
                        "kind": "orv.db.adapter",
                        "contract": "http-json-v1",
                        "provider": "adapter provider",
                        "url": "adapter url",
                        "method": "db method",
                        "args": "runtime value array"
                    },
                    "retry": {
                        "attempts": 3,
                        "on": ["5xx", "connect_error", "read_error", "timeout"]
                    },
                    "env": [
                        {
                            "env": "ORV_DB_ADAPTER_MYSQL_ENDPOINT",
                            "required": true,
                            "purpose": "bridge_endpoint"
                        },
                        {
                            "env": "ORV_DB_ADAPTER_MYSQL_AUTH_TOKEN",
                            "required": false,
                            "purpose": "bridge_auth_token"
                        },
                        {
                            "env": "ORV_DB_ADAPTER_ENDPOINT",
                            "required": false,
                            "purpose": "bridge_endpoint_fallback"
                        },
                        {
                            "env": "ORV_DB_ADAPTER_AUTH_TOKEN",
                            "required": false,
                            "purpose": "bridge_auth_token_fallback"
                        }
                    ]
                }
            },
            {
                "kind": "db",
                "mode": "external",
                "provider": "postgres",
                "env": null,
                "default": null,
                "endpoint": "postgres://db.internal/shop",
                "adapter_status": "unsupported_runtime",
                "runtime": {
                    "status": "unsupported_runtime",
                    "query_methods": ["create", "find", "update", "delete", "transaction"]
                },
                "bridge": {
                    "contract": "http-json-v1",
                    "method": "POST",
                    "content_type": "application/json",
                    "query_methods": [
                        "create",
                        "find",
                        "findAll",
                        "update",
                        "delete",
                        "upsert",
                        "search",
                        "count",
                        "sum",
                        "transaction",
                        "schema"
                    ],
                    "body": {
                        "kind": "orv.db.adapter",
                        "contract": "http-json-v1",
                        "provider": "adapter provider",
                        "url": "adapter url",
                        "method": "db method",
                        "args": "runtime value array"
                    },
                    "retry": {
                        "attempts": 3,
                        "on": ["5xx", "connect_error", "read_error", "timeout"]
                    },
                    "env": [
                        {
                            "env": "ORV_DB_ADAPTER_POSTGRES_ENDPOINT",
                            "required": true,
                            "purpose": "bridge_endpoint"
                        },
                        {
                            "env": "ORV_DB_ADAPTER_POSTGRES_AUTH_TOKEN",
                            "required": false,
                            "purpose": "bridge_auth_token"
                        },
                        {
                            "env": "ORV_DB_ADAPTER_ENDPOINT",
                            "required": false,
                            "purpose": "bridge_endpoint_fallback"
                        },
                        {
                            "env": "ORV_DB_ADAPTER_AUTH_TOKEN",
                            "required": false,
                            "purpose": "bridge_auth_token_fallback"
                        }
                    ]
                }
            }
        ])
    );
    assert!(container["persistence"]["volumes"]
        .as_array()
        .expect("volumes")
        .is_empty());
    assert_eq!(
        container["persistence"]["db_endpoints"],
        deploy["server"]["persistence"]["db_endpoints"]
    );
    assert!(
        compose.contains(r#"SHOP_DATABASE_URL: "${SHOP_DATABASE_URL:-mysql://db.internal/shop}""#)
    );
    assert!(
        compose.contains(r#"ORV_DB_ADAPTER_MYSQL_ENDPOINT: "${ORV_DB_ADAPTER_MYSQL_ENDPOINT}""#)
    );
    assert!(compose
        .contains(r#"ORV_DB_ADAPTER_POSTGRES_ENDPOINT: "${ORV_DB_ADAPTER_POSTGRES_ENDPOINT}""#));
    assert!(compose.contains(r#"ORV_DB_ADAPTER_ENDPOINT: "${ORV_DB_ADAPTER_ENDPOINT}""#));
    assert!(env_example.contains("SHOP_DATABASE_URL=mysql://db.internal/shop"));
    assert!(env_example.contains("ORV_DB_ADAPTER_MYSQL_ENDPOINT="));
    assert!(env_example.contains("ORV_DB_ADAPTER_POSTGRES_ENDPOINT="));
    assert!(env_example.contains("ORV_DB_ADAPTER_ENDPOINT="));
    assert!(preflight["required_env"]
        .as_array()
        .expect("required preflight env")
        .iter()
        .any(|env| env["env"] == "ORV_DB_ADAPTER_MYSQL_ENDPOINT"
            && env["provider"] == "mysql"
            && env["purpose"] == "bridge_endpoint"));
    assert!(preflight["required_env"]
        .as_array()
        .expect("required preflight env")
        .iter()
        .any(|env| env["env"] == "ORV_DB_ADAPTER_POSTGRES_ENDPOINT"
            && env["provider"] == "postgres"
            && env["purpose"] == "bridge_endpoint"));
    assert!(runbook.contains("- DB endpoint: mysql://db.internal/shop"));
    assert!(runbook.contains("- DB endpoint: postgres://db.internal/shop"));
    assert!(
        runbook.contains("- DB adapter env: SHOP_DATABASE_URL default mysql://db.internal/shop")
    );
    assert!(runbook
        .contains("- DB bridge env: mysql ORV_DB_ADAPTER_MYSQL_ENDPOINT required bridge_endpoint"));
    assert!(runbook.contains(
        "- DB bridge env: postgres ORV_DB_ADAPTER_POSTGRES_ENDPOINT required bridge_endpoint"
    ));
    assert!(smoke_test.contains(r#"orv_smoke_file "deploy/db-adapters.json""#));
    assert!(smoke_test.contains(
            r#"orv_smoke_grep "db adapter bridge contract" "deploy/db-adapters.json" '"contract": "http-json-v1"'"#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_db_bridge_schema "mysql bridge" "${ORV_DB_ADAPTER_MYSQL_ENDPOINT:-${ORV_DB_ADAPTER_ENDPOINT:-}}" "mysql" "mysql://db.internal/shop" "${ORV_DB_ADAPTER_MYSQL_AUTH_TOKEN:-${ORV_DB_ADAPTER_AUTH_TOKEN:-}}""#
        ));
    assert!(smoke_test.contains(
            r#"orv_smoke_db_bridge_schema "postgres bridge" "${ORV_DB_ADAPTER_POSTGRES_ENDPOINT:-${ORV_DB_ADAPTER_ENDPOINT:-}}" "postgres" "postgres://db.internal/shop" "${ORV_DB_ADAPTER_POSTGRES_AUTH_TOKEN:-${ORV_DB_ADAPTER_AUTH_TOKEN:-}}""#
        ));
    assert!(runbook.contains("deploy/db-adapters.json"));
    cmd_verify_build(&out).expect("verify prod build");
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(out);
}

#[test]
fn build_prod_mounts_file_commerce_adapter_records() {
    let dir = temp_output_dir("build-prod-file-commerce-source");
    std::fs::create_dir_all(&dir).expect("create file commerce source dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        r#"@server {
  @listen 8080
  let payments = @payment.connect("file://records/payments.jsonl")
  let shipping = @shipping.connect("file://records/shipments.jsonl")
  @route POST /checkout {
    let captured = payments.capture({ orderId: "o_1", amount: 42, method: "card" })
    let booked = shipping.book({ orderId: "o_1", carrier: "post", address: "Seoul" })
    @respond 200 { payment: captured.status, shipment: booked.status }
  }
}
"#,
    )
    .expect("write file commerce source");
    let out = temp_output_dir("build-prod-file-commerce");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");

    let deploy = read_json_value(&out.join("deploy").join("manifest.json")).expect("deploy");
    let container = read_json_value(&out.join("deploy").join("container.json")).expect("container");
    let compose =
        std::fs::read_to_string(out.join("deploy").join("compose.yaml")).expect("compose");
    let runbook = std::fs::read_to_string(out.join("deploy").join("README.md")).expect("runbook");

    assert_eq!(
        deploy["server"]["persistence"]["record_paths"],
        serde_json::json!(["records/payments.jsonl", "records/shipments.jsonl"])
    );
    assert_eq!(
        container["persistence"]["volumes"][0]["host"],
        serde_json::json!("records")
    );
    assert!(compose.contains("../records:/app/records"));
    assert!(runbook.contains("- Record log: records/payments.jsonl"));
    assert!(runbook.contains("- Record log: records/shipments.jsonl"));
    cmd_verify_build(&out).expect("verify prod build");
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(out);
}

#[test]
fn build_prod_records_http_commerce_adapter_endpoints() {
    let dir = temp_output_dir("build-prod-http-commerce-source");
    std::fs::create_dir_all(&dir).expect("create http commerce source dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        r#"@server {
  @listen 8080
  let payments = @payment.connect("http://payments.internal/capture")
  let shipping = @shipping.connect("http://shipping.internal/book")
  @route POST /checkout {
    let captured = payments.capture({ orderId: "o_1", amount: 42, method: "card" })
    let booked = shipping.book({ orderId: "o_1", carrier: "post", address: "Seoul" })
    @respond 200 { payment: captured.status, shipment: booked.status }
  }
}
"#,
    )
    .expect("write http commerce source");
    let out = temp_output_dir("build-prod-http-commerce");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");

    let deploy = read_json_value(&out.join("deploy").join("manifest.json")).expect("deploy");
    let container = read_json_value(&out.join("deploy").join("container.json")).expect("container");
    let compose =
        std::fs::read_to_string(out.join("deploy").join("compose.yaml")).expect("compose");
    let runbook = std::fs::read_to_string(out.join("deploy").join("README.md")).expect("runbook");

    assert_eq!(
        deploy["server"]["persistence"]["commerce_endpoints"],
        serde_json::json!([
            "http://payments.internal/capture",
            "http://shipping.internal/book"
        ])
    );
    assert_eq!(
        container["persistence"]["commerce_endpoints"],
        deploy["server"]["persistence"]["commerce_endpoints"]
    );
    assert!(container["persistence"]["volumes"]
        .as_array()
        .expect("volumes")
        .is_empty());
    assert!(!compose.contains("../records:/app/records"));
    assert!(runbook.contains("- Commerce endpoint: http://payments.internal/capture"));
    assert!(runbook.contains("- Commerce endpoint: http://shipping.internal/book"));
    cmd_verify_build(&out).expect("verify prod build");
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(out);
}

#[test]
fn build_prod_records_env_configured_http_commerce_endpoints() {
    let dir = temp_output_dir("build-prod-env-http-commerce-source");
    std::fs::create_dir_all(&dir).expect("create env http commerce source dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        r#"@server {
  @listen 8080
  let payments = @payment.connect(@env.PAYMENT_ADAPTER_URL ?? "http://payments.internal/capture")
  let shipping = @shipping.connect(@env.SHIPPING_ADAPTER_URL ?? "http://shipping.internal/book")
  @route POST /checkout {
    let captured = payments.capture({ orderId: "o_1", amount: 42, method: "card" })
    let booked = shipping.book({ orderId: "o_1", carrier: "post", address: "Seoul" })
    @respond 200 { payment: captured.status, shipment: booked.status }
  }
}
"#,
    )
    .expect("write env http commerce source");
    let out = temp_output_dir("build-prod-env-http-commerce");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");

    let deploy = read_json_value(&out.join("deploy").join("manifest.json")).expect("deploy");
    let container = read_json_value(&out.join("deploy").join("container.json")).expect("container");
    let compose =
        std::fs::read_to_string(out.join("deploy").join("compose.yaml")).expect("compose");
    let commerce_adapters_path = out.join("deploy").join("commerce-adapters.json");
    let commerce_adapters = read_json_value(&commerce_adapters_path).expect("commerce adapters");
    let runbook = std::fs::read_to_string(out.join("deploy").join("README.md")).expect("runbook");

    assert_eq!(
        deploy["server"]["commerce_adapters"],
        "deploy/commerce-adapters.json"
    );
    assert_eq!(
        deploy["server"]["persistence"]["commerce_endpoints"],
        serde_json::json!([
            "http://payments.internal/capture",
            "http://shipping.internal/book"
        ])
    );
    assert_eq!(
        deploy["server"]["persistence"]["commerce_env"],
        serde_json::json!([
            {
                "env": "PAYMENT_ADAPTER_URL",
                "default": "http://payments.internal/capture"
            },
            {
                "env": "SHIPPING_ADAPTER_URL",
                "default": "http://shipping.internal/book"
            }
        ])
    );
    assert_eq!(
        container["persistence"]["commerce_env"],
        deploy["server"]["persistence"]["commerce_env"]
    );
    assert!(compose.contains(
        r#"PAYMENT_ADAPTER_URL: "${PAYMENT_ADAPTER_URL:-http://payments.internal/capture}""#
    ));
    assert!(compose.contains(
        r#"SHIPPING_ADAPTER_URL: "${SHIPPING_ADAPTER_URL:-http://shipping.internal/book}""#
    ));
    assert_eq!(commerce_adapters["schema_version"], 1);
    assert_eq!(commerce_adapters["artifact"], "server/app.orv-runtime.json");
    assert_eq!(
        adapter_values_without_source_origin_ids(&commerce_adapters["adapters"]),
        serde_json::json!([
            {
                "kind": "payment",
                "mode": "http",
                "env": "PAYMENT_ADAPTER_URL",
                "default": "http://payments.internal/capture",
                "endpoint": "http://payments.internal/capture",
                "record_path": null,
                "request": {
                    "method": "POST",
                    "content_type": "application/json",
                    "kind": "payment.capture",
                    "body": {
                        "kind": "payment.capture",
                        "payload": "payment capture payload"
                    }
                }
            },
            {
                "kind": "shipping",
                "mode": "http",
                "env": "SHIPPING_ADAPTER_URL",
                "default": "http://shipping.internal/book",
                "endpoint": "http://shipping.internal/book",
                "record_path": null,
                "request": {
                    "method": "POST",
                    "content_type": "application/json",
                    "kind": "shipping.booking",
                    "body": {
                        "kind": "shipping.booking",
                        "payload": "shipping booking payload"
                    }
                }
            }
        ])
    );
    assert!(commerce_adapters["adapters"]
        .as_array()
        .expect("commerce adapters")
        .iter()
        .all(|adapter| adapter["source_origin_id"]
            .as_str()
            .is_some_and(|origin_id| origin_id.starts_with("ori_"))));
    assert!(runbook.contains(
        "- Commerce adapter env: PAYMENT_ADAPTER_URL default http://payments.internal/capture"
    ));
    assert!(runbook.contains(
        "- Commerce adapter env: SHIPPING_ADAPTER_URL default http://shipping.internal/book"
    ));
    assert!(runbook.contains("deploy/commerce-adapters.json"));
    cmd_verify_build(&out).expect("verify prod build");
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(out);
}

#[test]
fn build_prod_records_provider_commerce_adapters() {
    let dir = temp_output_dir("build-prod-provider-commerce-source");
    std::fs::create_dir_all(&dir).expect("create provider commerce source dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        r#"@server {
  @listen 8080
  let payments = @payment.connect(@env.PAYMENT_ADAPTER_URL ?? "stripe://local")
  let shipping = @shipping.connect(@env.SHIPPING_ADAPTER_URL ?? "carrier://local")
  @route POST /checkout {
    let captured = payments.capture({ orderId: "o_1", amount: 42, method: "card" })
    let booked = shipping.book({ orderId: "o_1", carrier: "post", address: "Seoul" })
    @respond 200 { payment: captured.status, shipment: booked.status }
  }
}
"#,
    )
    .expect("write provider commerce source");
    let out = temp_output_dir("build-prod-provider-commerce");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");

    let deploy = read_json_value(&out.join("deploy").join("manifest.json")).expect("deploy");
    let container = read_json_value(&out.join("deploy").join("container.json")).expect("container");
    let compose =
        std::fs::read_to_string(out.join("deploy").join("compose.yaml")).expect("compose");
    let commerce_adapters = read_json_value(&out.join("deploy").join("commerce-adapters.json"))
        .expect("commerce adapters");
    let env_example =
        std::fs::read_to_string(out.join("deploy").join("env.example")).expect("env example");
    let runbook = std::fs::read_to_string(out.join("deploy").join("README.md")).expect("runbook");

    assert_eq!(
        deploy["server"]["persistence"]["commerce_endpoints"],
        serde_json::json!([])
    );
    assert!(container["persistence"]["volumes"]
        .as_array()
        .expect("volumes")
        .is_empty());
    assert!(compose.contains(r#"PAYMENT_ADAPTER_URL: "${PAYMENT_ADAPTER_URL:-stripe://local}""#));
    assert!(compose.contains(r#"SHIPPING_ADAPTER_URL: "${SHIPPING_ADAPTER_URL:-carrier://local}""#));
    assert!(compose.contains(r#"STRIPE_SECRET_KEY: "${STRIPE_SECRET_KEY}""#));
    assert!(compose.contains(r#"STRIPE_API_ENDPOINT: "${STRIPE_API_ENDPOINT}""#));
    assert!(compose.contains(r#"STRIPE_WEBHOOK_SECRET: "${STRIPE_WEBHOOK_SECRET}""#));
    assert!(
        compose.contains(r#"STRIPE_WEBHOOK_SECRET_PREVIOUS: "${STRIPE_WEBHOOK_SECRET_PREVIOUS}""#)
    );
    assert!(compose.contains(r#"CARRIER_API_KEY: "${CARRIER_API_KEY}""#));
    assert!(compose.contains(r#"CARRIER_API_ENDPOINT: "${CARRIER_API_ENDPOINT}""#));
    assert!(compose.contains(r#"CARRIER_WEBHOOK_SECRET: "${CARRIER_WEBHOOK_SECRET}""#));
    assert_eq!(
        adapter_values_without_source_origin_ids(&commerce_adapters["adapters"]),
        serde_json::json!([
            {
                "kind": "payment",
                "mode": "provider",
                "provider": "stripe",
                "env": "PAYMENT_ADAPTER_URL",
                "default": "stripe://local",
                "endpoint": null,
                "record_path": null,
                "provider_env": [
                    {
                        "env": "STRIPE_API_ENDPOINT",
                        "required": false,
                        "purpose": "api_endpoint"
                    },
                    {
                        "env": "STRIPE_SECRET_KEY",
                        "required": true,
                        "purpose": "api_secret"
                    },
                    {
                        "env": "STRIPE_WEBHOOK_SECRET",
                        "required": false,
                        "purpose": "webhook_signature"
                    },
                    {
                        "env": "STRIPE_WEBHOOK_SECRET_PREVIOUS",
                        "required": false,
                        "purpose": "webhook_signature_previous"
                    }
                ],
                "request": {
                    "method": "POST",
                    "content_type": "application/json",
                    "kind": "payment.capture",
                    "body": {
                        "kind": "payment.capture",
                        "payload": "payment capture payload"
                    }
                }
            },
            {
                "kind": "shipping",
                "mode": "provider",
                "provider": "carrier",
                "env": "SHIPPING_ADAPTER_URL",
                "default": "carrier://local",
                "endpoint": null,
                "record_path": null,
                "provider_env": [
                    {
                        "env": "CARRIER_API_ENDPOINT",
                        "required": false,
                        "purpose": "api_endpoint"
                    },
                    {
                        "env": "CARRIER_API_KEY",
                        "required": true,
                        "purpose": "api_key"
                    },
                    {
                        "env": "CARRIER_WEBHOOK_SECRET",
                        "required": false,
                        "purpose": "webhook_signature"
                    }
                ],
                "request": {
                    "method": "POST",
                    "content_type": "application/json",
                    "kind": "shipping.booking",
                    "body": {
                        "kind": "shipping.booking",
                        "payload": "shipping booking payload"
                    }
                }
            }
        ])
    );
    assert!(commerce_adapters["adapters"]
        .as_array()
        .expect("commerce adapters")
        .iter()
        .all(|adapter| adapter["source_origin_id"]
            .as_str()
            .is_some_and(|origin_id| origin_id.starts_with("ori_"))));
    assert!(env_example.contains("STRIPE_API_ENDPOINT="));
    assert!(env_example.contains("STRIPE_SECRET_KEY="));
    assert!(env_example.contains("STRIPE_WEBHOOK_SECRET="));
    assert!(env_example.contains("STRIPE_WEBHOOK_SECRET_PREVIOUS="));
    assert!(env_example.contains("CARRIER_API_ENDPOINT="));
    assert!(env_example.contains("CARRIER_API_KEY="));
    assert!(env_example.contains("CARRIER_WEBHOOK_SECRET="));
    assert!(runbook.contains("- Commerce adapter env: PAYMENT_ADAPTER_URL default stripe://local"));
    assert!(
        runbook.contains("- Commerce adapter env: SHIPPING_ADAPTER_URL default carrier://local")
    );
    assert!(runbook.contains(
        "- Commerce provider env: payment stripe STRIPE_API_ENDPOINT optional api_endpoint"
    ));
    assert!(runbook
        .contains("- Commerce provider env: payment stripe STRIPE_SECRET_KEY required api_secret"));
    assert!(runbook.contains(
        "- Commerce provider env: payment stripe STRIPE_WEBHOOK_SECRET optional webhook_signature"
    ));
    assert!(runbook.contains(
            "- Commerce provider env: payment stripe STRIPE_WEBHOOK_SECRET_PREVIOUS optional webhook_signature_previous"
        ));
    assert!(runbook.contains(
        "- Commerce provider env: shipping carrier CARRIER_API_ENDPOINT optional api_endpoint"
    ));
    assert!(runbook
        .contains("- Commerce provider env: shipping carrier CARRIER_API_KEY required api_key"));
    assert!(runbook.contains(
            "- Commerce provider env: shipping carrier CARRIER_WEBHOOK_SECRET optional webhook_signature"
        ));
    assert!(runbook.contains("orv deploy-env-check ."));
    cmd_verify_build(&out).expect("verify prod build");
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(out);
}

#[test]
fn deploy_env_check_reports_missing_required_provider_credentials() {
    let dir = temp_output_dir("deploy-env-check-provider-source");
    std::fs::create_dir_all(&dir).expect("create provider commerce source dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        r#"@server {
  @listen 8080
  let payments = @payment.connect(@env.PAYMENT_ADAPTER_URL ?? "stripe://local")
  let shipping = @shipping.connect(@env.SHIPPING_ADAPTER_URL ?? "carrier://local")
  @route POST /checkout {
    let captured = payments.capture({ orderId: "o_1", amount: 42, method: "card" })
    let booked = shipping.book({ orderId: "o_1", carrier: "post", address: "Seoul" })
    @respond 200 { payment: captured.status, shipment: booked.status }
  }
}
"#,
    )
    .expect("write provider commerce source");
    let out = temp_output_dir("deploy-env-check-provider");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");

    let err = deploy_env_check_with_lookup(&out, |_| None).expect_err("required envs are missing");
    let message = err.to_string();
    assert!(message.contains("STRIPE_SECRET_KEY"), "{message}");
    assert!(message.contains("CARRIER_API_KEY"), "{message}");

    deploy_env_check_with_lookup(&out, |env| match env {
        "STRIPE_SECRET_KEY" => Some("sk_test".to_string()),
        "CARRIER_API_KEY" => Some("carrier_key".to_string()),
        _ => None,
    })
    .expect("optional webhook envs may be absent");
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(out);
}

#[test]
fn deploy_env_check_reports_missing_required_db_adapter_env() {
    let dir = temp_output_dir("deploy-env-check-db-source");
    std::fs::create_dir_all(&dir).expect("create db adapter source dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        r#"@server {
  @listen 8080
  let shopdb = @db.connect(@env.SHOP_DATABASE_URL)
  @route GET /ping { @respond 200 { ok: true } }
}
"#,
    )
    .expect("write db adapter source");
    let out = temp_output_dir("deploy-env-check-db");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");

    let err = deploy_env_check_with_lookup(&out, |_| None).expect_err("required DB env missing");
    let message = err.to_string();
    assert!(message.contains("SHOP_DATABASE_URL"), "{message}");

    deploy_env_check_with_lookup(&out, |env| match env {
        "SHOP_DATABASE_URL" => Some("postgres://db.internal/shop".to_string()),
        _ => None,
    })
    .expect("configured DB env passes");
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(out);
}

#[test]
fn deploy_env_check_reports_missing_required_db_bridge_endpoint() {
    let dir = temp_output_dir("deploy-env-check-db-bridge-source");
    std::fs::create_dir_all(&dir).expect("create db bridge source dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        r#"@server {
  @listen 8080
  let shopdb = @db.connect "postgres://db.internal/shop"
  @route GET /ping { @respond 200 { ok: true } }
}
"#,
    )
    .expect("write db bridge source");
    let out = temp_output_dir("deploy-env-check-db-bridge");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");

    let err = deploy_env_check_with_lookup(&out, |_| None).expect_err("required DB bridge missing");
    let message = err.to_string();
    assert!(
        message.contains("ORV_DB_ADAPTER_POSTGRES_ENDPOINT"),
        "{message}"
    );

    deploy_env_check_with_lookup(&out, |env| match env {
        "ORV_DB_ADAPTER_ENDPOINT" => Some("http://db-adapter.internal/shared".to_string()),
        _ => None,
    })
    .expect("generic DB bridge endpoint fallback passes");

    deploy_env_check_with_lookup(&out, |env| match env {
        "ORV_DB_ADAPTER_POSTGRES_ENDPOINT" => {
            Some("http://db-adapter.internal/postgres".to_string())
        }
        _ => None,
    })
    .expect("configured DB bridge endpoint passes");
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(out);
}

#[test]
fn verify_build_rejects_deploy_commerce_adapter_mismatch() {
    let dir = temp_output_dir("deploy-commerce-adapters-source");
    std::fs::create_dir_all(&dir).expect("create commerce adapter source dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        r#"@server {
  @listen 8080
  let payments = @payment.connect(@env.PAYMENT_ADAPTER_URL ?? "http://payments.internal/capture")
  @route POST /checkout {
    let captured = payments.capture({ orderId: "o_1", amount: 42, method: "card" })
    @respond 200 { payment: captured.status }
  }
}
"#,
    )
    .expect("write commerce adapter source");
    let out = temp_output_dir("deploy-commerce-adapters-mismatch");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let adapters_path = out.join("deploy").join("commerce-adapters.json");
    let mut adapters = read_json_value(&adapters_path).expect("commerce adapters");
    adapters["adapters"][0]["endpoint"] = serde_json::json!("http://wrong.example/capture");
    write_json(&adapters_path, &adapters).expect("write corrupt commerce adapters");

    let err = cmd_verify_build(&out).expect_err("commerce adapter mismatch");

    assert!(err
        .to_string()
        .contains("deploy commerce adapters do not match runtime artifact persistence"));
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(out);
}

#[test]
fn verify_build_rejects_deploy_commerce_adapter_origin_drift_from_origin_map() {
    let dir = temp_output_dir("deploy-commerce-adapter-origin-source");
    std::fs::create_dir_all(&dir).expect("create commerce adapter origin source dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        r#"@server {
  @listen 8080
  let payments = @payment.connect(@env.PAYMENT_ADAPTER_URL ?? "http://payments.internal/capture")
  @route POST /checkout {
    let captured = payments.capture({ orderId: "o_1", amount: 42, method: "card" })
    @respond 200 { payment: captured.status }
  }
}
"#,
    )
    .expect("write commerce adapter origin source");
    let out = temp_output_dir("deploy-commerce-adapter-origin-mismatch");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let adapters_path = out.join("deploy").join("commerce-adapters.json");
    let adapters = read_json_value(&adapters_path).expect("commerce adapters");
    let origin_id = adapters["adapters"][0]["source_origin_id"]
        .as_str()
        .expect("commerce source origin")
        .to_string();
    corrupt_origin_entry_kind_and_graph(&out, &origin_id, "domain", "payment");

    let err = cmd_verify_build(&out).expect_err("commerce adapter origin mismatch");

    assert!(err
        .to_string()
        .contains("deploy commerce adapter payment source_origin_id"));
    assert!(err
        .to_string()
        .contains("must reference origin-map call @payment.connect"));
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(out);
}

#[test]
fn verify_build_rejects_deploy_db_adapter_mismatch() {
    let dir = temp_output_dir("deploy-db-adapters-source");
    std::fs::create_dir_all(&dir).expect("create db adapter source dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        r#"@server {
  @listen 8080
  let shopdb = @db.connect(@env.SHOP_DATABASE_URL ?? "postgres://db.internal/shop")
  @route GET /ping { @respond 200 { ok: true } }
}
"#,
    )
    .expect("write db adapter source");
    let out = temp_output_dir("deploy-db-adapters-mismatch");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let adapters_path = out.join("deploy").join("db-adapters.json");
    let mut adapters = read_json_value(&adapters_path).expect("db adapters");
    adapters["adapters"][0]["endpoint"] = serde_json::json!("postgres://wrong.example/shop");
    write_json(&adapters_path, &adapters).expect("write corrupt db adapters");

    let err = cmd_verify_build(&out).expect_err("db adapter mismatch");

    assert!(err
        .to_string()
        .contains("deploy DB adapters do not match runtime artifact persistence"));
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(out);
}

#[test]
fn verify_build_rejects_deploy_db_adapter_origin_drift_from_origin_map() {
    let dir = temp_output_dir("deploy-db-adapter-origin-source");
    std::fs::create_dir_all(&dir).expect("create db adapter origin source dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        r#"@server {
  @listen 8080
  let shopdb = @db.connect(@env.SHOP_DATABASE_URL ?? "postgres://db.internal/shop")
  @route GET /ping { @respond 200 { ok: true } }
}
"#,
    )
    .expect("write db adapter origin source");
    let out = temp_output_dir("deploy-db-adapter-origin-mismatch");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let adapters_path = out.join("deploy").join("db-adapters.json");
    let adapters = read_json_value(&adapters_path).expect("db adapters");
    let origin_id = adapters["adapters"][0]["source_origin_id"]
        .as_str()
        .expect("db source origin")
        .to_string();
    corrupt_origin_entry_kind_and_graph(&out, &origin_id, "domain", "db");

    let err = cmd_verify_build(&out).expect_err("db adapter origin mismatch");

    assert!(err
        .to_string()
        .contains("deploy DB adapter source_origin_id"));
    assert!(err
        .to_string()
        .contains("must reference origin-map call @db.connect"));
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(out);
}

#[test]
fn verify_build_rejects_deploy_smoke_test_path_mismatch() {
    let (src_dir, path) = prod_server_source("deploy-smoke-path-source");
    let out = temp_output_dir("deploy-smoke-path-mismatch");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let canonical_smoke_path = out.join("deploy").join("smoke-test.sh");
    let wrong_smoke_path = out.join("deploy").join("alternate-smoke.sh");
    std::fs::copy(&canonical_smoke_path, &wrong_smoke_path).expect("copy smoke test");
    let deploy_manifest_path = out.join("deploy").join("manifest.json");
    let mut deploy = read_json_value(&deploy_manifest_path).expect("deploy manifest");
    deploy["server"]["smoke_test"] = serde_json::json!("deploy/alternate-smoke.sh");
    write_json(&deploy_manifest_path, &deploy).expect("write corrupt deploy manifest");
    let runbook_path = out.join("deploy").join("README.md");
    let runbook = std::fs::read_to_string(&runbook_path).expect("deploy runbook");
    std::fs::write(
        &runbook_path,
        runbook.replace("deploy/smoke-test.sh", "deploy/alternate-smoke.sh"),
    )
    .expect("write corrupt deploy runbook");

    let err = cmd_verify_build(&out).expect_err("smoke test path mismatch");

    assert!(err
        .to_string()
        .contains("deploy server smoke_test must be deploy/smoke-test.sh"));
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(out);
}

#[test]
fn verify_build_rejects_deploy_smoke_graph_contract_missing() {
    let (src_dir, path) = prod_server_source("deploy-smoke-graph-source");
    let out = temp_output_dir("deploy-smoke-graph-missing");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let smoke_path = out.join("deploy").join("smoke-test.sh");
    let smoke = std::fs::read_to_string(&smoke_path).expect("smoke test");
    write_text(
        &smoke_path,
        &smoke.replace("\norv_smoke_graph_contract\n", "\n"),
    )
    .expect("write corrupt smoke test");

    let err = cmd_verify_build(&out).expect_err("smoke graph contract mismatch");

    assert!(err
        .to_string()
        .contains("deploy smoke test must verify the build graph contract"));
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(out);
}

#[test]
fn verify_build_rejects_deploy_smoke_dap_source_bundle_panel_missing() {
    let (src_dir, path) = prod_server_source("deploy-smoke-dap-source-bundle-source");
    let out = temp_output_dir("deploy-smoke-dap-source-bundle-missing");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let smoke_path = out.join("deploy").join("smoke-test.sh");
    let smoke = std::fs::read_to_string(&smoke_path).expect("smoke test");
    write_text(
            &smoke_path,
            &smoke.replace(
                r#"orv_smoke_dap_summary_contains "dap source bundle panel path" '"path": "./source-bundle.json"'
"#,
                "",
            ),
        )
        .expect("write corrupt smoke test");

    let err = cmd_verify_build(&out).expect_err("smoke DAP source bundle panel mismatch");

    assert!(err
        .to_string()
        .contains("deploy smoke test must verify the build graph contract"));
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(out);
}

#[test]
fn verify_build_rejects_deploy_smoke_dap_marker_contract_missing() {
    let (src_dir, path) = prod_server_source("deploy-smoke-dap-marker-contract-source");
    let out = temp_output_dir("deploy-smoke-dap-marker-contract-missing");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let smoke_path = out.join("deploy").join("smoke-test.sh");
    let smoke = std::fs::read_to_string(&smoke_path).expect("smoke test");
    write_text(
        &smoke_path,
        &smoke.replace(
            r#"orv_smoke_dap_summary_contains "dap smoke required markers" '"smoke_test_required_markers": ['
"#,
            "",
        ),
    )
    .expect("write corrupt smoke test");

    let err = cmd_verify_build(&out).expect_err("smoke DAP marker contract mismatch");

    assert!(
        err.to_string().contains(
            "deploy smoke test must verify smoke marker contract in DAP production context"
        ),
        "{err:?}"
    );
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(out);
}

#[test]
fn verify_build_rejects_deploy_smoke_reveal_marker_contract_missing() {
    let (src_dir, path) = prod_server_source("deploy-smoke-reveal-marker-contract-source");
    let out = temp_output_dir("deploy-smoke-reveal-marker-contract-missing");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let smoke_path = out.join("deploy").join("smoke-test.sh");
    let smoke = std::fs::read_to_string(&smoke_path).expect("smoke test");
    write_text(
        &smoke_path,
        &smoke.replace(
            r#"orv_smoke_reveal_contains "reveal smoke required markers" "$ORV_SMOKE_ORIGIN_GET_PING" '"smoke_test_required_markers": ['
"#,
            "",
        ),
    )
    .expect("write corrupt smoke test");

    let err = cmd_verify_build(&out).expect_err("smoke reveal marker contract mismatch");

    assert!(
        err.to_string()
            .contains("deploy smoke test must verify smoke marker contract across reveal surfaces"),
        "{err:?}"
    );
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(out);
}

#[test]
fn verify_build_rejects_deploy_smoke_output_contract_missing() {
    let (src_dir, path) = prod_server_source("deploy-smoke-output-source");
    let out = temp_output_dir("deploy-smoke-output-missing");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let smoke_path = out.join("deploy").join("smoke-test.sh");
    let smoke = std::fs::read_to_string(&smoke_path).expect("smoke test");
    write_text(
        &smoke_path,
        &smoke.replace(r#"> "$ORV_SMOKE_OUTPUT""#, r#"> /dev/null"#),
    )
    .expect("write corrupt smoke test");

    let err = cmd_verify_build(&out).expect_err("smoke output contract mismatch");

    assert!(
        err.to_string()
            .contains("deploy smoke test must write deploy smoke output artifact"),
        "{err:?}"
    );
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(out);
}

#[test]
fn verify_build_rejects_deploy_smoke_output_dap_marker_missing() {
    let (src_dir, path) = prod_server_source("deploy-smoke-output-dap-source");
    let out = temp_output_dir("deploy-smoke-output-dap-missing");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let smoke_path = out.join("deploy").join("smoke-test.sh");
    let smoke = std::fs::read_to_string(&smoke_path).expect("smoke test");
    write_text(
        &smoke_path,
        &smoke.replace("dap_summary=verified", "dap_summary=missing"),
    )
    .expect("write corrupt smoke test");

    let err = cmd_verify_build(&out).expect_err("smoke output DAP marker mismatch");

    assert!(
        err.to_string()
            .contains("deploy smoke test must write deploy smoke output artifact"),
        "{err:?}"
    );
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(out);
}

#[test]
fn verify_build_rejects_deploy_smoke_output_dap_source_bundle_marker_missing() {
    let (src_dir, path) = prod_server_source("deploy-smoke-output-dap-bundle-source");
    let out = temp_output_dir("deploy-smoke-output-dap-bundle-missing");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let smoke_path = out.join("deploy").join("smoke-test.sh");
    let smoke = std::fs::read_to_string(&smoke_path).expect("smoke test");
    write_text(
        &smoke_path,
        &smoke.replace("dap_source_bundle=verified", "dap_source_bundle=missing"),
    )
    .expect("write corrupt smoke test");

    let err = cmd_verify_build(&out).expect_err("smoke output DAP source bundle marker mismatch");

    assert!(
        err.to_string()
            .contains("deploy smoke test must write deploy smoke output artifact"),
        "{err:?}"
    );
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(out);
}

#[test]
fn verify_build_rejects_deploy_smoke_origin_assignment_mismatch() {
    let (src_dir, path) = prod_server_source("deploy-smoke-origin-source");
    let out = temp_output_dir("deploy-smoke-origin-mismatch");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let artifact = read_server_artifact(&out.join("server").join("app.orv-runtime.json"))
        .expect("server artifact");
    let route = artifact
        .routes
        .iter()
        .find(|route| route.method == "GET" && route.path == "/ping")
        .expect("GET /ping route");
    let smoke_path = out.join("deploy").join("smoke-test.sh");
    let smoke = std::fs::read_to_string(&smoke_path).expect("smoke test");
    let expected = format!(r#"ORV_SMOKE_ORIGIN_GET_PING="{}""#, route.origin_id);
    let smoke = smoke.replace(&expected, r#"ORV_SMOKE_ORIGIN_GET_PING="ori_wrong""#);
    write_text(&smoke_path, &smoke).expect("write corrupt smoke test");

    let err = cmd_verify_build(&out).expect_err("smoke origin mismatch");

    assert!(err
        .to_string()
        .contains("deploy smoke test must declare expected origin for GET /ping"));
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(out);
}

#[test]
fn verify_build_rejects_deploy_smoke_response_origin_assignment_mismatch() {
    let (src_dir, path) = prod_server_source("deploy-smoke-response-origin-source");
    let out = temp_output_dir("deploy-smoke-response-origin-mismatch");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let artifact = read_server_artifact(&out.join("server").join("app.orv-runtime.json"))
        .expect("server artifact");
    let route = artifact
        .routes
        .iter()
        .find(|route| route.method == "GET" && route.path == "/ping")
        .expect("GET /ping route");
    let response_origin = route
        .response_origin_ids
        .first()
        .expect("GET /ping response origin");
    let smoke_path = out.join("deploy").join("smoke-test.sh");
    let smoke = std::fs::read_to_string(&smoke_path).expect("smoke test");
    let expected = format!(r#"ORV_SMOKE_RESPONSE_ORIGIN_GET_PING="{response_origin}""#);
    let smoke = smoke.replace(
        &expected,
        r#"ORV_SMOKE_RESPONSE_ORIGIN_GET_PING="ori_wrong""#,
    );
    write_text(&smoke_path, &smoke).expect("write corrupt smoke test");

    let err = cmd_verify_build(&out).expect_err("smoke response origin mismatch");

    assert!(err
        .to_string()
        .contains("deploy smoke test must declare expected response origin for GET /ping"));
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(out);
}

#[cfg(unix)]
#[test]
fn verify_build_rejects_non_executable_deploy_smoke_test() {
    use std::os::unix::fs::PermissionsExt;

    let (src_dir, path) = prod_server_source("deploy-smoke-mode-source");
    let out = temp_output_dir("deploy-smoke-mode-mismatch");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let smoke_path = out.join("deploy").join("smoke-test.sh");
    let mut permissions = std::fs::metadata(&smoke_path)
        .expect("smoke metadata")
        .permissions();
    permissions.set_mode(0o644);
    std::fs::set_permissions(&smoke_path, permissions).expect("remove executable bit");

    let err = cmd_verify_build(&out).expect_err("smoke test mode mismatch");

    assert!(err
        .to_string()
        .contains("deploy smoke test must be executable"));
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(out);
}

#[cfg(unix)]
#[test]
fn verify_build_rejects_invalid_deploy_smoke_test_shell_syntax() {
    let (src_dir, path) = prod_server_source("deploy-smoke-syntax-source");
    let out = temp_output_dir("deploy-smoke-syntax-mismatch");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let smoke_path = out.join("deploy").join("smoke-test.sh");
    let mut smoke = std::fs::read_to_string(&smoke_path).expect("smoke test");
    smoke.push_str("\nif\n");
    std::fs::write(&smoke_path, smoke).expect("write corrupt smoke script");

    let err = cmd_verify_build(&out).expect_err("smoke shell syntax mismatch");

    assert!(err
        .to_string()
        .contains("deploy smoke test shell syntax invalid"));
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(out);
}

#[test]
fn build_prod_writes_env_listen_container_contract() {
    let (src_dir, path) = env_prod_server_source("build-prod-env-listen-source");
    let out = temp_output_dir("build-prod-env-listen");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");

    let deploy_manifest_path = out.join("deploy").join("manifest.json");
    let deploy_container_path = out.join("deploy").join("container.json");
    let deploy_dockerfile_path = out.join("deploy").join("Dockerfile");
    let deploy_compose_path = out.join("deploy").join("compose.yaml");
    let deploy_env_example_path = out.join("deploy").join("env.example");
    let deploy = read_json_value(&deploy_manifest_path).expect("deploy manifest");
    let container = read_json_value(&deploy_container_path).expect("deploy container");

    assert_eq!(deploy["server"]["listen"]["port"], serde_json::Value::Null);
    assert_eq!(deploy["server"]["listen"]["env"]["variable"], "PORT");
    assert_eq!(deploy["server"]["listen"]["env"]["default_port"], 8080);
    assert_eq!(container["listen"], deploy["server"]["listen"]);
    assert_eq!(container["ports"][0]["env"], "PORT");
    assert_eq!(container["ports"][0]["default"], 8080);
    assert_eq!(container["ports"][0]["protocol"], "tcp");
    let dockerfile = std::fs::read_to_string(&deploy_dockerfile_path).expect("Dockerfile");
    assert!(dockerfile.contains("EXPOSE 8080"));
    let compose = std::fs::read_to_string(&deploy_compose_path).expect("compose");
    assert!(compose.contains(r#""${PORT:-8080}:8080""#));
    assert!(compose.contains(r#"PORT: "${PORT:-8080}""#));
    let env_example = std::fs::read_to_string(&deploy_env_example_path).expect("env example");
    assert!(env_example.contains("PORT=8080"));

    cmd_verify_build(&out).expect("verify prod build");
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn build_prod_rejects_test_only_ephemeral_listen_port() {
    let path = workspace_path(&["fixtures", "e2e", "hello.orv"]);
    let out = temp_output_dir("build-prod-ephemeral-listen");

    let err = cmd_build_with_profile(&path, &out, BuildProfile::Production)
        .expect_err("ephemeral prod listen");

    assert!(err
        .to_string()
        .contains("prod server listen port must be 1..=65535"));
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_deploy_routes_mismatch() {
    let (src_dir, path) = prod_server_source("deploy-routes-source");
    let out = temp_output_dir("deploy-routes-mismatch");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let routes_path = out.join("deploy").join("routes.json");
    let mut routes = read_json_value(&routes_path).expect("routes");
    routes["routes"][0]["path"] = serde_json::json!("/wrong");
    write_json(&routes_path, &routes).expect("write corrupt routes");

    let err = cmd_verify_build(&out).expect_err("routes mismatch");

    assert!(err
        .to_string()
        .contains("deploy routes do not match runtime artifact"));
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_server_route_origin_missing_from_origin_map() {
    let (src_dir, path) = prod_server_source("server-route-origin-source");
    let out = temp_output_dir("server-route-origin-mismatch");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let artifact_path = out.join("server").join("app.orv-runtime.json");
    let mut artifact = read_json_value(&artifact_path).expect("server artifact");
    artifact["routes"][0]["origin_id"] = serde_json::json!("ori_missing_route");
    write_json(&artifact_path, &artifact).expect("write corrupt server artifact");

    let err = cmd_verify_build(&out).expect_err("route origin mismatch");

    assert!(err.to_string().contains(
        "server route GET /ping origin_id `ori_missing_route` not found in origin-map.json"
    ));
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_server_response_origin_drift_from_origin_map() {
    let (src_dir, path) = prod_server_source("server-response-origin-source");
    let out = temp_output_dir("server-response-origin-mismatch");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let artifact_path = out.join("server").join("app.orv-runtime.json");
    let mut artifact = read_json_value(&artifact_path).expect("server artifact");
    artifact["routes"][0]["response_origin_ids"][0] = serde_json::json!("ori_missing_response");
    write_json(&artifact_path, &artifact).expect("write corrupt server artifact");

    let err = cmd_verify_build(&out).expect_err("response origin mismatch");

    assert!(err.to_string().contains(
        "server route GET /ping response_origin_ids do not match origin-map contains edges"
    ));
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_server_policy_origin_drift_from_origin_map() {
    let dir = temp_output_dir("server-policy-origin-source");
    std::fs::create_dir_all(&dir).expect("create policy origin source dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        r#"@server {
  @listen 8080
  @route POST /checkout {
    @csrf
    @respond 201 { ok: true }
  }
}
"#,
    )
    .expect("write policy source");
    let out = temp_output_dir("server-policy-origin-mismatch");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let artifact_path = out.join("server").join("app.orv-runtime.json");
    let mut artifact = read_json_value(&artifact_path).expect("server artifact");
    artifact["routes"][0]["policies"][1]["origin_id"] = serde_json::json!("ori_missing_policy");
    write_json(&artifact_path, &artifact).expect("write corrupt server artifact");

    let err = cmd_verify_build(&out).expect_err("policy origin mismatch");

    assert!(err.to_string().contains(
            "server route POST /checkout policy `csrf` origin_id `ori_missing_policy` not found in origin-map.json"
        ));
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_server_source_bundle_drift() {
    let (src_dir, path) = prod_server_source("server-source-bundle-source");
    let out = temp_output_dir("server-source-bundle-mismatch");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let artifact_path = out.join("server").join("app.orv-runtime.json");
    let mut artifact = read_json_value(&artifact_path).expect("server artifact");
    let source_path = artifact["source_bundle"]["files"][0]["path"]
        .as_str()
        .expect("source path")
        .to_string();
    let tampered_source =
        "@server { @listen 8080 @route GET /wrong { @respond 200 { ok: true } } }\n";
    let tampered_bundle = orv_compiler::source_bundle_artifact(
        artifact["entry"].as_str().expect("entry"),
        [(source_path.as_str(), tampered_source)],
    );
    artifact["source_bundle"]["files"][0]["source"] = serde_json::json!(tampered_source);
    artifact["source_bundle"]["files"][0]["content_hash"] =
        serde_json::json!(tampered_bundle.files[0].content_hash.clone());
    write_json(&artifact_path, &artifact).expect("write corrupt server artifact");

    let err = cmd_verify_build(&out).expect_err("server source bundle mismatch");

    assert!(err
        .to_string()
        .contains("does not match build source-bundle artifact"));
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_project_graph_semantic_origin_drift() {
    let (src_dir, path) = prod_server_source("project-graph-origin-source");
    let out = temp_output_dir("project-graph-origin-mismatch");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let graph_path = out.join("project-graph.json");
    let mut graph = read_json_value(&graph_path).expect("project graph");
    graph["semantic"]["origin_map"]["entries"][0]["id"] = serde_json::json!("ori_wrong");
    write_json(&graph_path, &graph).expect("write corrupt project graph");

    let err = cmd_verify_build(&out).expect_err("project graph origin drift");

    assert!(err
        .to_string()
        .contains("project-graph.json semantic origin_map does not match origin-map.json"));
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_project_graph_source_file_drift() {
    let (src_dir, path) = prod_server_source("project-graph-file-source");
    let out = temp_output_dir("project-graph-file-mismatch");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let graph_path = out.join("project-graph.json");
    let mut graph = read_json_value(&graph_path).expect("project graph");
    graph["nodes"][0]["name"] = serde_json::json!("/tmp/wrong.orv");
    write_json(&graph_path, &graph).expect("write corrupt project graph");

    let err = cmd_verify_build(&out).expect_err("project graph source file drift");

    assert!(err
        .to_string()
        .contains("project-graph.json is missing source-bundle file node"));
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_project_graph_origin_link_drift() {
    let (src_dir, path) = prod_server_source("project-graph-link-source");
    let out = temp_output_dir("project-graph-link-mismatch");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let graph_path = out.join("project-graph.json");
    let mut graph = read_json_value(&graph_path).expect("project graph");
    graph["semantic"]["origin_links"] = serde_json::json!([]);
    write_json(&graph_path, &graph).expect("write corrupt project graph");

    let err = cmd_verify_build(&out).expect_err("project graph origin link drift");

    assert!(err.to_string().contains(
        "project-graph.json semantic origin_links do not match graph nodes and origin-map.json"
    ));
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_deploy_container_mismatch() {
    let (src_dir, path) = prod_server_source("deploy-container-source");
    let out = temp_output_dir("deploy-container-mismatch");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let container_path = out.join("deploy").join("container.json");
    let mut container = read_json_value(&container_path).expect("container");
    container["artifact"] = serde_json::json!("server/wrong.orv-runtime.json");
    write_json(&container_path, &container).expect("write corrupt container");

    let err = cmd_verify_build(&out).expect_err("container mismatch");

    assert!(err
        .to_string()
        .contains("deploy container artifact must be server/app.orv-runtime.json"));
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_deploy_container_runtime_image_mismatch() {
    let (src_dir, path) = prod_server_source("deploy-container-runtime-image-source");
    let out = temp_output_dir("deploy-container-runtime-image-mismatch");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let container_path = out.join("deploy").join("container.json");
    let mut container = read_json_value(&container_path).expect("container");
    container["runtime_image"] = serde_json::json!("example.invalid/orv:wrong");
    write_json(&container_path, &container).expect("write corrupt container");

    let err = cmd_verify_build(&out).expect_err("container runtime image mismatch");

    assert!(err
        .to_string()
        .contains("deploy container runtime_image must be"));
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_native_server_plan_mismatch() {
    let (src_dir, path) = prod_server_source("native-server-plan-source");
    let out = temp_output_dir("native-server-plan-mismatch");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let native_plan_path = out.join("server").join("native-server.json");
    let mut native_plan = read_json_value(&native_plan_path).expect("native server plan");
    native_plan["artifact"] = serde_json::json!("server/wrong.orv-runtime.json");
    write_json(&native_plan_path, &native_plan).expect("write corrupt native server plan");

    let err = cmd_verify_build(&out).expect_err("native server plan mismatch");

    assert!(err
        .to_string()
        .contains("native server plan artifact must be server/app.orv-runtime.json"));
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_native_server_plan_command_mismatch() {
    let (src_dir, path) = prod_server_source("native-server-plan-command-source");
    let out = temp_output_dir("native-server-plan-command-mismatch");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let native_plan_path = out.join("server").join("native-server.json");
    let mut native_plan = read_json_value(&native_plan_path).expect("native server plan");
    native_plan["commands"] = serde_json::json!({
        "build": [
            "wrong-cargo",
            "build",
            "--manifest-path",
            "server/native/Cargo.toml",
            "--release"
        ],
        "run": {
            "env": {
                "ORV_BUILD_DIR": "."
            },
            "command": [
                "./server/native/target/release/orv-native-server"
            ]
        }
    });
    write_json(&native_plan_path, &native_plan).expect("write corrupt native server plan");

    let err = cmd_verify_build(&out).expect_err("native server plan command mismatch");

    assert!(err
        .to_string()
        .contains("native server plan build command must match generated launcher package"));
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_native_server_launcher_source_mismatch() {
    let (src_dir, path) = prod_server_source("native-server-source-source");
    let out = temp_output_dir("native-server-source-mismatch");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let source_path = out.join("server").join("native").join("main.rs");
    let mut source = std::fs::read_to_string(&source_path).expect("native source");
    source = source.replace(
        "router::orv_native_dispatch_with_request(",
        "router::orv_native_dispatch(\"GET\", \"/wrong\")",
    );
    write_text(&source_path, &source).expect("write corrupt native source");

    let err = cmd_verify_build(&out).expect_err("native server source mismatch");

    assert!(err
        .to_string()
        .contains("native server launcher source must dispatch through generated router"));
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_native_server_launcher_compile_error() {
    let (src_dir, path) = prod_server_source("native-server-compile-source");
    let out = temp_output_dir("native-server-compile-mismatch");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let source_path = out.join("server").join("native").join("main.rs");
    let mut source = std::fs::read_to_string(&source_path).expect("native source");
    source.push_str("\nfn __orv_compile_error( {\n");
    write_text(&source_path, &source).expect("write corrupt native source");

    let err = cmd_verify_build(&out).expect_err("native server source compile mismatch");

    assert!(err
        .to_string()
        .contains("native server launcher source must match generated source"));
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_native_server_routes_source_mismatch() {
    let (src_dir, path) = prod_server_source("native-server-routes-source");
    let out = temp_output_dir("native-server-routes-source-mismatch");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let routes_path = out.join("server").join("native").join("routes.rs");
    let mut source = std::fs::read_to_string(&routes_path).expect("native routes source");
    source = source.replace("path: \"/ping\"", "path: \"/wrong\"");
    write_text(&routes_path, &source).expect("write corrupt native routes source");

    let err = cmd_verify_build(&out).expect_err("native routes source mismatch");

    assert!(err
        .to_string()
        .contains("native server routes source must match server runtime artifact"));
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_native_server_router_source_mismatch() {
    let (src_dir, path) = prod_server_source("native-server-router-source");
    let out = temp_output_dir("native-server-router-source-mismatch");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let router_path = out.join("server").join("native").join("router.rs");
    let mut source = std::fs::read_to_string(&router_path).expect("native router source");
    source = source.replace(
        "handlers::orv_native_handle_route(&route_match)",
        "handlers::orv_native_handle_missing_route(&route_match)",
    );
    write_text(&router_path, &source).expect("write corrupt native router source");

    let err = cmd_verify_build(&out).expect_err("native router source mismatch");

    assert!(err
        .to_string()
        .contains("native server router source must match generated source"));
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_native_server_launcher_package_mismatch() {
    let (src_dir, path) = prod_server_source("native-server-package-source");
    let out = temp_output_dir("native-server-package-mismatch");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let package_path = out.join("server").join("native").join("Cargo.toml");
    let mut package = std::fs::read_to_string(&package_path).expect("native package");
    package = package.replace("path = \"main.rs\"", "path = \"wrong.rs\"");
    write_text(&package_path, &package).expect("write corrupt native package");

    let err = cmd_verify_build(&out).expect_err("native server package mismatch");

    assert!(err
        .to_string()
        .contains("native server launcher package bin path must be main.rs"));
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_deploy_compose_port_mismatch() {
    let (src_dir, path) = prod_server_source("deploy-compose-port-source");
    let out = temp_output_dir("deploy-compose-port-mismatch");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let compose_path = out.join("deploy").join("compose.yaml");
    let mut compose = std::fs::read_to_string(&compose_path).expect("compose");
    compose = compose.replace(r#""8080:8080""#, r#""9090:9090""#);
    write_text(&compose_path, &compose).expect("write corrupt compose");

    let err = cmd_verify_build(&out).expect_err("compose port mismatch");

    assert!(err.to_string().contains("deploy compose must publish 8080"));
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_deploy_runbook_route_mismatch() {
    let (src_dir, path) = prod_server_source("deploy-runbook-route-source");
    let out = temp_output_dir("deploy-runbook-route-mismatch");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let runbook_path = out.join("deploy").join("README.md");
    let mut runbook = std::fs::read_to_string(&runbook_path).expect("runbook");
    runbook = runbook.replace("- GET /ping", "- GET /wrong");
    write_text(&runbook_path, &runbook).expect("write corrupt runbook");

    let err = cmd_verify_build(&out).expect_err("runbook route mismatch");

    assert!(err
        .to_string()
        .contains("deploy runbook must list route GET /ping"));
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_deploy_runbook_smoke_marker_mismatch() {
    let (src_dir, path) = prod_server_source("deploy-runbook-smoke-marker-source");
    let out = temp_output_dir("deploy-runbook-smoke-marker-mismatch");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let runbook_path = out.join("deploy").join("README.md");
    let mut runbook = std::fs::read_to_string(&runbook_path).expect("runbook");
    runbook = runbook.replace("- `dap_source_bundle`", "- `dap_source_bundle_missing`");
    write_text(&runbook_path, &runbook).expect("write corrupt runbook");

    let err = cmd_verify_build(&out).expect_err("runbook smoke marker mismatch");

    assert!(err
        .to_string()
        .contains("deploy runbook must document smoke output marker dap_source_bundle"));
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_deploy_preflight_runtime_feature_mismatch() {
    let (src_dir, path) = prod_server_source("deploy-preflight-runtime-feature-source");
    let out = temp_output_dir("deploy-preflight-runtime-feature-mismatch");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let preflight_path = out.join("deploy").join("preflight.json");
    let mut preflight = read_json_value(&preflight_path).expect("preflight");
    preflight["runtime_features"] = serde_json::json!(["http_server"]);
    write_json(&preflight_path, &preflight).expect("write corrupt preflight");

    let err = cmd_verify_build(&out).expect_err("preflight runtime feature mismatch");

    assert!(err
        .to_string()
        .contains("deploy preflight runtime_features do not match runtime artifact"));
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_deploy_preflight_benchmark_mismatch() {
    let (src_dir, path) = prod_server_source("deploy-preflight-benchmark-source");
    let out = temp_output_dir("deploy-preflight-benchmark-mismatch");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let preflight_path = out.join("deploy").join("preflight.json");
    let mut preflight = read_json_value(&preflight_path).expect("preflight");
    preflight["benchmark"]["max_elapsed_minutes"] = serde_json::json!(301);
    write_json(&preflight_path, &preflight).expect("write corrupt preflight");

    let err = cmd_verify_build(&out).expect_err("preflight benchmark mismatch");

    assert!(err
        .to_string()
        .contains("deploy preflight benchmark does not match 5-hour shop contract"));
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_deploy_benchmark_evidence_mismatch() {
    let (src_dir, path) = prod_server_source("deploy-benchmark-evidence-source");
    let out = temp_output_dir("deploy-benchmark-evidence-mismatch");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let evidence_path = out.join("deploy").join("benchmark-evidence.json");
    let mut evidence = read_json_value(&evidence_path).expect("benchmark evidence");
    evidence["benchmark"]["max_elapsed_minutes"] = serde_json::json!(301);
    write_json(&evidence_path, &evidence).expect("write corrupt benchmark evidence");

    let err = cmd_verify_build(&out).expect_err("benchmark evidence mismatch");

    assert!(err
        .to_string()
        .contains("deploy benchmark evidence benchmark does not match 5-hour shop contract"));
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_deploy_benchmark_evidence_smoke_marker_mismatch() {
    let (src_dir, path) = prod_server_source("deploy-benchmark-evidence-smoke-marker-source");
    let out = temp_output_dir("deploy-benchmark-evidence-smoke-marker-mismatch");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let evidence_path = out.join("deploy").join("benchmark-evidence.json");
    let mut evidence = read_json_value(&evidence_path).expect("benchmark evidence");
    evidence["data"]["smoke_test_required_markers"] =
        serde_json::json!(["pass_marker", "build_dir", "base_url"]);
    write_json(&evidence_path, &evidence).expect("write drifted benchmark evidence");

    let err = cmd_verify_build(&out).expect_err("benchmark evidence smoke marker mismatch");

    assert!(err
        .to_string()
        .contains("smoke_test_required_markers must match smoke output contract"));
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_accepts_recorded_deploy_benchmark_evidence_values() {
    let (src_dir, path) = prod_server_source("deploy-benchmark-evidence-recorded-source");
    let out = temp_output_dir("deploy-benchmark-evidence-recorded");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let evidence_path = out.join("deploy").join("benchmark-evidence.json");
    let mut evidence = read_json_value(&evidence_path).expect("benchmark evidence");
    evidence["recording_status"] = serde_json::json!("recorded");
    evidence["task_entries"][0]["elapsed_minutes"] = serde_json::json!(12.5);
    evidence["task_entries"][0]["status"] = serde_json::json!("recorded");
    evidence["task_entries"][0]["notes"] = serde_json::json!("first run completed");
    evidence["data"]["docs_help_lookups"] = serde_json::json!(3);
    evidence["data"]["compiler_runtime_errors"] = serde_json::json!(1);
    evidence["data"]["first_error_to_fix_minutes"] = serde_json::json!(4.5);
    evidence["data"]["manual_config_edits"] = serde_json::json!(["none"]);
    evidence["data"]["smoke_test_output"] = serde_json::json!("passed");
    evidence["data"]["participant_notes"] = serde_json::json!("sample");
    write_json(&evidence_path, &evidence).expect("write recorded benchmark evidence");

    cmd_verify_build(&out).expect("recorded benchmark evidence still verifies");
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn benchmark_report_marks_unrecorded_evidence_incomplete() {
    let (src_dir, path) = prod_server_source("benchmark-report-incomplete-source");
    let out = temp_output_dir("benchmark-report-incomplete");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");

    let report = benchmark_report_value(&out).expect("benchmark report");

    assert_eq!(report["kind"], "orv.benchmark.shop_5h.report");
    assert_eq!(report["status"], "incomplete");
    assert_eq!(report["contract_verified"], true);
    assert_eq!(report["evidence"], "deploy/benchmark-evidence.json");
    assert_eq!(report["preflight"], "deploy/preflight.json");
    assert_eq!(report["max_elapsed_minutes"], 300.0);
    assert_eq!(report["tasks"]["task_count"], 10);
    assert_eq!(report["tasks"]["recorded_task_count"], 0);
    assert_eq!(report["tasks"]["missing_task_count"], 10);
    assert!(report["data"]["missing_data"]
        .as_array()
        .expect("missing data")
        .iter()
        .any(|item| item == "docs_help_lookups"));
    assert!(report["data"]["missing_data"]
        .as_array()
        .expect("missing data")
        .iter()
        .any(|item| item == "smoke_test_output"));
    assert!(cmd_benchmark_report(&out, true)
        .expect_err("require pass rejects incomplete")
        .to_string()
        .contains("benchmark report status must be passed"));
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn benchmark_report_marks_recorded_evidence_passed() {
    let (src_dir, path) = prod_server_source("benchmark-report-passed-source");
    let out = temp_output_dir("benchmark-report-passed");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let evidence_path = out.join("deploy").join("benchmark-evidence.json");
    let mut evidence = read_json_value(&evidence_path).expect("benchmark evidence");
    evidence["recording_status"] = serde_json::json!("recorded");
    for entry in evidence["task_entries"]
        .as_array_mut()
        .expect("task entries")
    {
        entry["elapsed_minutes"] = serde_json::json!(10.0);
        entry["status"] = serde_json::json!("passed");
    }
    evidence["data"]["docs_help_lookups"] = serde_json::json!(2);
    evidence["data"]["compiler_runtime_errors"] = serde_json::json!(0);
    evidence["data"]["manual_config_edits"] = serde_json::json!([]);
    evidence["data"]["smoke_test_output"] = serde_json::json!(
            "orv deploy smoke test passed\nbuild_dir=/tmp/orv-build\nbase_url=http://127.0.0.1:8080\ngraph_contract=verified\ndap_summary=verified\ndap_source_bundle=verified\nserver_routes=1\ntrace_stream_requested=0\n"
        );
    evidence["data"]["participant_notes"] = serde_json::json!("no blockers");
    write_json(&evidence_path, &evidence).expect("write recorded benchmark evidence");

    let report = benchmark_report_value(&out).expect("benchmark report");

    assert_eq!(report["status"], "passed");
    assert_eq!(report["time_over_limit"], false);
    assert_eq!(report["total_elapsed_minutes"], 100.0);
    assert_eq!(report["tasks"]["recorded_task_count"], 10);
    assert_eq!(report["tasks"]["missing_task_count"], 0);
    assert_eq!(report["tasks"]["failed_task_count"], 0);
    assert_eq!(report["data"]["smoke_test_summary"]["passed_marker"], true);
    assert_eq!(
        report["data"]["smoke_test_required_markers"],
        serde_json::json!(deploy_benchmark::SMOKE_REQUIRED_MARKERS)
    );
    assert_eq!(
        report["data"]["smoke_test_summary"]["required_markers"],
        serde_json::json!(deploy_benchmark::SMOKE_REQUIRED_MARKERS)
    );
    assert_eq!(
        report["data"]["smoke_test_summary"]["graph_contract_verified"],
        true
    );
    assert_eq!(
        report["data"]["smoke_test_summary"]["dap_summary_verified"],
        true
    );
    assert_eq!(
        report["data"]["smoke_test_summary"]["dap_source_bundle_verified"],
        true
    );
    assert_eq!(report["data"]["smoke_test_summary"]["server_routes"], 1);
    assert_eq!(
        report["data"]["missing_data"]
            .as_array()
            .expect("missing data")
            .len(),
        0
    );
    cmd_benchmark_report(&out, true).expect("require pass accepts recorded evidence");
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn benchmark_report_marks_weak_smoke_output_incomplete() {
    let (src_dir, path) = prod_server_source("benchmark-report-weak-smoke-source");
    let out = temp_output_dir("benchmark-report-weak-smoke");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let evidence_path = out.join("deploy").join("benchmark-evidence.json");
    let mut evidence = read_json_value(&evidence_path).expect("benchmark evidence");
    evidence["recording_status"] = serde_json::json!("recorded");
    for entry in evidence["task_entries"]
        .as_array_mut()
        .expect("task entries")
    {
        entry["elapsed_minutes"] = serde_json::json!(10.0);
        entry["status"] = serde_json::json!("passed");
    }
    evidence["data"]["docs_help_lookups"] = serde_json::json!(2);
    evidence["data"]["compiler_runtime_errors"] = serde_json::json!(0);
    evidence["data"]["manual_config_edits"] = serde_json::json!([]);
    evidence["data"]["smoke_test_output"] = serde_json::json!("smoke passed");
    evidence["data"]["participant_notes"] = serde_json::json!("weak smoke output");
    write_json(&evidence_path, &evidence).expect("write recorded benchmark evidence");

    let report = benchmark_report_value(&out).expect("benchmark report");

    assert_eq!(report["status"], "incomplete");
    assert_eq!(report["data"]["smoke_test_summary"]["passed_marker"], false);
    assert!(report["data"]["missing_data"]
        .as_array()
        .expect("missing data")
        .iter()
        .any(|item| item == "smoke_test_output.graph_contract"));
    assert!(report["data"]["missing_data"]
        .as_array()
        .expect("missing data")
        .iter()
        .any(|item| item == "smoke_test_output.dap_summary"));
    assert!(report["data"]["missing_data"]
        .as_array()
        .expect("missing data")
        .iter()
        .any(|item| item == "smoke_test_output.dap_source_bundle"));
    assert!(cmd_benchmark_report(&out, true)
        .expect_err("require pass rejects weak smoke output")
        .to_string()
        .contains("benchmark report status must be passed"));
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn benchmark_report_uses_generated_smoke_output_artifact() {
    let (src_dir, path) = prod_server_source("benchmark-report-smoke-output-source");
    let out = temp_output_dir("benchmark-report-smoke-output");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let evidence_path = out.join("deploy").join("benchmark-evidence.json");
    let smoke_output_path = out.join("deploy").join("smoke-output.txt");
    let mut evidence = read_json_value(&evidence_path).expect("benchmark evidence");
    evidence["recording_status"] = serde_json::json!("recorded");
    for entry in evidence["task_entries"]
        .as_array_mut()
        .expect("task entries")
    {
        entry["elapsed_minutes"] = serde_json::json!(10.0);
        entry["status"] = serde_json::json!("passed");
    }
    evidence["data"]["docs_help_lookups"] = serde_json::json!(1);
    evidence["data"]["compiler_runtime_errors"] = serde_json::json!(0);
    evidence["data"]["manual_config_edits"] = serde_json::json!([]);
    evidence["data"]["participant_notes"] = serde_json::json!("smoke output from artifact");
    write_json(&evidence_path, &evidence).expect("write recorded benchmark evidence");
    std::fs::write(
            &smoke_output_path,
            "orv deploy smoke test passed\nbuild_dir=/tmp/orv-build\nbase_url=http://127.0.0.1:8080\ngraph_contract=verified\ndap_summary=verified\ndap_source_bundle=verified\nserver_routes=1\ntrace_stream_requested=1\n",
        )
        .expect("write smoke output");

    let report = benchmark_report_value(&out).expect("benchmark report");

    assert_eq!(report["status"], "passed");
    assert_eq!(
            report["data"]["smoke_test_output"],
            "orv deploy smoke test passed\nbuild_dir=/tmp/orv-build\nbase_url=http://127.0.0.1:8080\ngraph_contract=verified\ndap_summary=verified\ndap_source_bundle=verified\nserver_routes=1\ntrace_stream_requested=1\n"
        );
    assert_eq!(
        report["data"]["smoke_test_output_source"],
        "deploy/smoke-output.txt"
    );
    assert_eq!(
        report["data"]["smoke_test_summary"]["trace_stream_requested"],
        true
    );
    assert_eq!(
        report["data"]["smoke_test_summary"]["dap_summary_verified"],
        true
    );
    assert_eq!(
        report["data"]["smoke_test_summary"]["dap_source_bundle_verified"],
        true
    );
    assert_eq!(
        report["data"]["missing_data"]
            .as_array()
            .expect("missing data")
            .len(),
        0
    );
    cmd_benchmark_report(&out, true).expect("require pass accepts generated smoke output artifact");
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_deploy_preflight_smoke_command_mismatch() {
    let (src_dir, path) = prod_server_source("deploy-preflight-smoke-command-source");
    let out = temp_output_dir("deploy-preflight-smoke-command-mismatch");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let preflight_path = out.join("deploy").join("preflight.json");
    let mut preflight = read_json_value(&preflight_path).expect("preflight");
    preflight["commands"]["smoke_test"] = serde_json::json!("./deploy/other-smoke.sh");
    write_json(&preflight_path, &preflight).expect("write corrupt preflight");

    let err = cmd_verify_build(&out).expect_err("preflight smoke command mismatch");

    assert!(err
        .to_string()
        .contains("deploy preflight smoke_test command must be ./deploy/smoke-test.sh"));
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_deploy_preflight_benchmark_report_command_mismatch() {
    let (src_dir, path) = prod_server_source("deploy-preflight-benchmark-report-source");
    let out = temp_output_dir("deploy-preflight-benchmark-report-mismatch");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let preflight_path = out.join("deploy").join("preflight.json");
    let mut preflight = read_json_value(&preflight_path).expect("preflight");
    preflight["commands"]["benchmark_report"] = serde_json::json!("orv benchmark-report other");
    write_json(&preflight_path, &preflight).expect("write corrupt preflight");

    let err = cmd_verify_build(&out).expect_err("preflight benchmark-report mismatch");

    assert!(err
        .to_string()
        .contains("deploy preflight benchmark_report command"));
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_deploy_preflight_editor_run_debug_command_mismatch() {
    let (src_dir, path) = prod_server_source("deploy-preflight-run-debug-source");
    let out = temp_output_dir("deploy-preflight-run-debug-mismatch");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let preflight_path = out.join("deploy").join("preflight.json");
    let mut preflight = read_json_value(&preflight_path).expect("preflight");
    preflight["commands"]["editor_run_debug"] =
        serde_json::json!("orv editor run-debug other --control next");
    write_json(&preflight_path, &preflight).expect("write corrupt preflight");

    let err = cmd_verify_build(&out).expect_err("preflight editor run-debug mismatch");

    assert!(err
        .to_string()
        .contains("deploy preflight editor_run_debug command"));
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_deploy_preflight_trace_run_build_command_mismatch() {
    let (src_dir, path) = prod_server_source("deploy-preflight-trace-run-build-source");
    let out = temp_output_dir("deploy-preflight-trace-run-build-mismatch");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let preflight_path = out.join("deploy").join("preflight.json");
    let mut preflight = read_json_value(&preflight_path).expect("preflight");
    preflight["commands"]["trace_run_build"] =
        serde_json::json!("orv run-build . --trace other.json");
    write_json(&preflight_path, &preflight).expect("write corrupt preflight");

    let err = cmd_verify_build(&out).expect_err("preflight trace run-build mismatch");

    assert!(err
        .to_string()
        .contains("deploy preflight trace_run_build command"));
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_deploy_preflight_graph_artifact_mismatch() {
    let (src_dir, path) = prod_server_source("deploy-preflight-graph-artifact-source");
    let out = temp_output_dir("deploy-preflight-graph-artifact-mismatch");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let preflight_path = out.join("deploy").join("preflight.json");
    let mut preflight = read_json_value(&preflight_path).expect("preflight");
    preflight["artifacts"]["origin_map"] = serde_json::json!("wrong-origin-map.json");
    write_json(&preflight_path, &preflight).expect("write corrupt preflight");

    let err = cmd_verify_build(&out).expect_err("preflight graph artifact mismatch");

    assert!(err
        .to_string()
        .contains("deploy preflight artifact origin_map must be origin-map.json"));
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_deploy_container_listen_mismatch() {
    let (src_dir, path) = prod_server_source("deploy-container-listen-source");
    let out = temp_output_dir("deploy-container-listen-mismatch");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let container_path = out.join("deploy").join("container.json");
    let mut container = read_json_value(&container_path).expect("container");
    container["listen"] = serde_json::json!({
        "origin_id": "ori_wrong",
        "name": "port 9090",
        "port": 9090,
    });
    write_json(&container_path, &container).expect("write corrupt container");

    let err = cmd_verify_build(&out).expect_err("container listen mismatch");

    assert!(err
        .to_string()
        .contains("deploy container listen does not match runtime artifact"));
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_deploy_container_env_ports_mismatch() {
    let (src_dir, path) = env_prod_server_source("deploy-container-env-ports-source");
    let out = temp_output_dir("deploy-container-env-ports-mismatch");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let container_path = out.join("deploy").join("container.json");
    let mut container = read_json_value(&container_path).expect("container");
    container["ports"][0]["env"] = serde_json::json!("HTTP_PORT");
    write_json(&container_path, &container).expect("write corrupt container");

    let err = cmd_verify_build(&out).expect_err("container ports mismatch");

    assert!(err
        .to_string()
        .contains("deploy container ports do not match runtime artifact"));
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_server_launcher_listen_mismatch() {
    let path = workspace_path(&["fixtures", "e2e", "hello.orv"]);
    let out = temp_output_dir("server-launch-listen-mismatch");

    cmd_build(&path, &out).expect("build");
    let launch_path = out.join("server").join("launch.json");
    let mut launch = read_json_value(&launch_path).expect("launch");
    launch["listen"]["port"] = serde_json::json!(1234);
    write_json(&launch_path, &launch).expect("write corrupt launch");

    let err = cmd_verify_build(&out).expect_err("listen mismatch");

    assert!(err
        .to_string()
        .contains("server launcher listen does not match runtime artifact"));
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn build_accepts_orv_toml_project_entry() {
    let dir = temp_output_dir("project-manifest-build");
    let src = dir.join("src");
    std::fs::create_dir_all(&src).expect("create src dir");
    let entry = src.join("main.orv");
    std::fs::write(&entry, "@html { \"Manifest page\" }\n").expect("write entry");
    let manifest = dir.join("orv.toml");
    std::fs::write(
        &manifest,
        r#"[project]
name = "manifest-build"
entry = "src/main.orv"
"#,
    )
    .expect("write manifest");
    let out = dir.join("dist");

    cmd_build(&manifest, &out).expect("manifest build");

    let build_manifest = read_json_value(&out.join("build-manifest.json")).expect("manifest");
    assert_eq!(build_manifest["entry"], entry.display().to_string());
    assert!(
        out.join("pages").join("index.html").is_file(),
        "missing static page"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lock_writes_and_checks_deterministic_project_lockfile() {
    let dir = temp_output_dir("project-lock");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    std::fs::write(dir.join("src").join("main.orv"), "@out \"lock\"\n").expect("write entry");
    std::fs::write(
        dir.join("orv.toml"),
        r#"[project]
name = "shop"
version = "0.1.0"
entry = "src/main.orv"

[dependencies]
zeta = "2.0.0"
auth = { version = "1.2.3", registry = "https://registry.orv.dev" }
ui = { version = "0.1.0", path = "libs/ui" }

[dev-dependencies]
mock-server = "0.2.0"
"#,
    )
    .expect("write manifest");

    cmd_lock(&dir, false).expect("write lock");

    let lock = read_json_value(&dir.join("orv.lock")).expect("read lock");
    assert_eq!(lock["schema_version"], 1);
    assert_eq!(lock["kind"], "orv.lock");
    assert_eq!(lock["project"]["name"], "shop");
    assert_eq!(lock["project"]["version"], "0.1.0");
    assert_eq!(lock["project"]["entry"], "src/main.orv");
    assert_eq!(lock["dependencies"][0]["name"], "auth");
    assert_eq!(lock["dependencies"][0]["version"], "1.2.3");
    assert_eq!(lock["dependencies"][0]["source"], "registry");
    assert_eq!(
        lock["dependencies"][0]["registry"],
        "https://registry.orv.dev"
    );
    assert!(lock["dependencies"][0]["checksum"]
        .as_str()
        .is_some_and(|hash| hash.starts_with("fnv1a64:")));
    assert_eq!(lock["dependencies"][1]["name"], "ui");
    assert_eq!(lock["dependencies"][1]["source"], "path");
    assert_eq!(lock["dependencies"][1]["path"], "libs/ui");
    assert_eq!(lock["dependencies"][2]["name"], "zeta");
    assert_eq!(lock["dev_dependencies"][0]["name"], "mock-server");

    cmd_lock(&dir, true).expect("check lock");

    let mut stale = lock;
    stale["dependencies"][0]["version"] = serde_json::json!("9.9.9");
    write_json_atomic(&dir.join("orv.lock"), &stale).expect("write stale lock");
    let err = cmd_lock(&dir, true).expect_err("stale lock");
    assert!(err.to_string().contains("orv.lock is out of date"));

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn fetch_writes_dependency_source_bundles_from_lockfile() {
    let dir = temp_output_dir("project-fetch");
    std::fs::create_dir_all(dir.join("src")).expect("create project src");
    std::fs::create_dir_all(dir.join("libs/ui/src")).expect("create path dep src");
    std::fs::create_dir_all(dir.join("registry/auth/1.2.3/src")).expect("create registry dep src");
    std::fs::write(dir.join("src/main.orv"), "@out \"fetch\"\n").expect("write entry");
    std::fs::write(
        dir.join("orv.toml"),
        r#"[project]
name = "shop"
version = "0.1.0"
entry = "src/main.orv"

[dependencies]
auth = { version = "1.2.3", registry = "registry" }
ui = { version = "0.1.0", path = "libs/ui" }
"#,
    )
    .expect("write manifest");
    std::fs::write(
        dir.join("libs/ui/orv.toml"),
        r#"[project]
name = "ui"
version = "0.1.0"
entry = "src/main.orv"
"#,
    )
    .expect("write path dep manifest");
    std::fs::write(
        dir.join("libs/ui/src/main.orv"),
        r#"@out @html { @body { @p "UI" } }"#,
    )
    .expect("write path dep source");
    std::fs::write(
        dir.join("registry/auth/1.2.3/orv.toml"),
        r#"[project]
name = "auth"
version = "1.2.3"
entry = "src/main.orv"
"#,
    )
    .expect("write registry dep manifest");
    std::fs::write(
        dir.join("registry/auth/1.2.3/src/main.orv"),
        r#"@out @html { @body { @p "Auth" } }"#,
    )
    .expect("write registry dep source");
    cmd_lock(&dir, false).expect("write lock");

    let out = dir.join("target/orv-deps");
    cmd_fetch(&dir, &out).expect("fetch dependencies");

    assert!(out
        .join("packages/dependencies/auth/source-bundle.json")
        .is_file());
    assert!(out
        .join("packages/dependencies/ui/source-bundle.json")
        .is_file());
    let manifest = read_json_value(&out.join("deps-manifest.json")).expect("read manifest");
    assert_eq!(manifest["schema_version"], 1);
    assert_eq!(manifest["kind"], "orv.dependencies");
    assert_eq!(manifest["lockfile"], "orv.lock");
    assert_eq!(manifest["stats"]["package_count"], 2);
    assert!(manifest["packages"]
        .as_array()
        .expect("packages")
        .iter()
        .any(|package| package["name"] == "auth"
            && package["source"] == "registry"
            && package["source_bundle"] == "packages/dependencies/auth/source-bundle.json"
            && package["verified"] == true));
    assert!(manifest["packages"]
        .as_array()
        .expect("packages")
        .iter()
        .any(|package| package["name"] == "ui"
            && package["source"] == "path"
            && package["source_bundle"] == "packages/dependencies/ui/source-bundle.json"
            && package["verified"] == true));
    read_source_bundle_artifact(&out.join("packages/dependencies/auth/source-bundle.json"))
        .expect("auth source bundle");
    read_source_bundle_artifact(&out.join("packages/dependencies/ui/source-bundle.json"))
        .expect("ui source bundle");

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn fetch_downloads_dependency_source_bundle_from_http_registry() {
    let dir = temp_output_dir("project-fetch-http");
    std::fs::create_dir_all(dir.join("src")).expect("create project src");
    std::fs::write(dir.join("src/main.orv"), "@out \"fetch-http\"\n").expect("write entry");
    let bundle = orv_compiler::source_bundle_artifact(
        "registry/auth/1.2.3/src/main.orv",
        [(
            "registry/auth/1.2.3/src/main.orv",
            r#"@out @html { @body { @p "Auth" } }"#,
        )],
    );
    let body = serde_json::to_vec_pretty(&serde_json::to_value(&bundle).expect("bundle json"))
        .expect("bundle bytes");
    let (registry, handle) = spawn_one_shot_http_json("/auth/1.2.3/source-bundle.json", body);
    std::fs::write(
            dir.join("orv.toml"),
            format!(
                "[project]\nname = \"shop\"\nversion = \"0.1.0\"\nentry = \"src/main.orv\"\n\n[dependencies]\nauth = {{ version = \"1.2.3\", registry = \"{registry}\" }}\n"
            ),
        )
        .expect("write manifest");
    cmd_lock(&dir, false).expect("write lock");

    let out = dir.join("target/orv-deps");
    cmd_fetch(&dir, &out).expect("fetch dependencies");
    handle.join().expect("registry served request");

    let manifest = read_json_value(&out.join("deps-manifest.json")).expect("read manifest");
    assert!(manifest["packages"]
        .as_array()
        .expect("packages")
        .iter()
        .any(|package| package["name"] == "auth"
            && package["source"] == "registry"
            && package["resolved_url"] == format!("{registry}/auth/1.2.3/source-bundle.json")
            && package["source_bundle"] == "packages/dependencies/auth/source-bundle.json"));
    let downloaded =
        read_source_bundle_artifact(&out.join("packages/dependencies/auth/source-bundle.json"))
            .expect("downloaded source bundle");
    assert_eq!(downloaded.entry, "registry/auth/1.2.3/src/main.orv");

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn fetch_sends_bearer_token_for_authenticated_http_registry() {
    let dir = temp_output_dir("project-fetch-http-auth");
    std::fs::create_dir_all(dir.join("src")).expect("create project src");
    std::fs::write(dir.join("src/main.orv"), "@out \"fetch-http-auth\"\n").expect("write entry");
    let bundle = orv_compiler::source_bundle_artifact(
        "registry/auth/1.2.3/src/main.orv",
        [(
            "registry/auth/1.2.3/src/main.orv",
            r#"@out @html { @body { @p "Auth" } }"#,
        )],
    );
    let body = serde_json::to_vec_pretty(&serde_json::to_value(&bundle).expect("bundle json"))
        .expect("bundle bytes");
    let (registry, handle) = spawn_one_shot_http_json_with_auth(
        "/auth/1.2.3/source-bundle.json",
        body,
        "Bearer orv-test-token",
    );
    std::env::set_var("ORV_TEST_REGISTRY_TOKEN_AUTH_FETCH", "orv-test-token");
    std::fs::write(
            dir.join("orv.toml"),
            format!(
                "[project]\nname = \"shop\"\nversion = \"0.1.0\"\nentry = \"src/main.orv\"\n\n[dependencies]\nauth = {{ version = \"1.2.3\", registry = \"{registry}\", auth_token_env = \"ORV_TEST_REGISTRY_TOKEN_AUTH_FETCH\" }}\n"
            ),
        )
        .expect("write manifest");
    cmd_lock(&dir, false).expect("write lock");

    let out = dir.join("target/orv-deps");
    cmd_fetch(&dir, &out).expect("fetch dependencies");
    handle.join().expect("registry served request");
    std::env::remove_var("ORV_TEST_REGISTRY_TOKEN_AUTH_FETCH");

    let manifest = read_json_value(&out.join("deps-manifest.json")).expect("read manifest");
    assert!(manifest["packages"]
        .as_array()
        .expect("packages")
        .iter()
        .any(|package| package["name"] == "auth"
            && package["source"] == "registry"
            && package["auth_token_env"] == "ORV_TEST_REGISTRY_TOKEN_AUTH_FETCH"));

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lock_resolves_caret_version_from_local_registry_index() {
    let dir = temp_output_dir("project-lock-registry-index");
    std::fs::create_dir_all(dir.join("src")).expect("create project src");
    std::fs::create_dir_all(dir.join("registry/auth/1.2.0/src")).expect("create 1.2.0");
    std::fs::create_dir_all(dir.join("registry/auth/1.3.0/src")).expect("create 1.3.0");
    std::fs::create_dir_all(dir.join("registry/auth/2.0.0/src")).expect("create 2.0.0");
    std::fs::write(dir.join("src/main.orv"), "@out \"lock-index\"\n").expect("write entry");
    std::fs::write(
        dir.join("registry/auth/index.json"),
        r#"{"versions":["1.2.0","1.3.0","2.0.0"]}"#,
    )
    .expect("write index");
    std::fs::write(
        dir.join("orv.toml"),
        r#"[project]
name = "shop"
version = "0.1.0"
entry = "src/main.orv"

[dependencies]
auth = { version = "^1.2.0", registry = "registry" }
"#,
    )
    .expect("write manifest");

    cmd_lock(&dir, false).expect("write lock");

    let lock = read_json_value(&dir.join("orv.lock")).expect("read lock");
    assert_eq!(lock["dependencies"][0]["name"], "auth");
    assert_eq!(lock["dependencies"][0]["version"], "1.3.0");
    assert_eq!(lock["dependencies"][0]["requested_version"], "^1.2.0");

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lock_sends_bearer_token_for_authenticated_http_registry_index() {
    let dir = temp_output_dir("project-lock-http-auth-index");
    std::fs::create_dir_all(dir.join("src")).expect("create project src");
    std::fs::write(dir.join("src/main.orv"), "@out \"lock-http-auth\"\n").expect("write entry");
    let (registry, handle) = spawn_one_shot_http_json_with_auth(
        "/auth/index.json",
        br#"{"versions":["1.2.0","1.3.0"]}"#.to_vec(),
        "Bearer orv-index-token",
    );
    std::env::set_var("ORV_TEST_REGISTRY_TOKEN_AUTH_INDEX", "orv-index-token");
    std::fs::write(
            dir.join("orv.toml"),
            format!(
                "[project]\nname = \"shop\"\nversion = \"0.1.0\"\nentry = \"src/main.orv\"\n\n[dependencies]\nauth = {{ version = \"^1.2.0\", registry = \"{registry}\", auth_token_env = \"ORV_TEST_REGISTRY_TOKEN_AUTH_INDEX\" }}\n"
            ),
        )
        .expect("write manifest");

    cmd_lock(&dir, false).expect("write lock");
    handle.join().expect("registry served request");
    std::env::remove_var("ORV_TEST_REGISTRY_TOKEN_AUTH_INDEX");

    let lock = read_json_value(&dir.join("orv.lock")).expect("read lock");
    assert_eq!(lock["dependencies"][0]["name"], "auth");
    assert_eq!(lock["dependencies"][0]["version"], "1.3.0");
    assert_eq!(
        lock["dependencies"][0]["auth_token_env"],
        "ORV_TEST_REGISTRY_TOKEN_AUTH_INDEX"
    );
    assert_eq!(lock["dependencies"][0]["requested_version"], "^1.2.0");

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn registry_index_uses_https_transport_instead_of_roadmap_error() {
    let error = registry_index_versions(Path::new("."), "auth", "https://127.0.0.1:9", None)
        .expect_err("unreachable https registry");

    assert!(!error.to_string().contains("not implemented"), "{error}");
}

#[test]
fn registry_fetch_uses_https_transport_instead_of_roadmap_error() {
    let dependency = serde_json::json!({
        "name": "auth",
        "section": "dependencies",
        "source": "registry",
        "registry": "https://127.0.0.1:9",
        "version": "1.2.3",
        "checksum": "fnv1a64:test",
    });
    let Err(error) = registry_dependency_source(Path::new("."), &dependency) else {
        panic!("unreachable https registry unexpectedly succeeded");
    };

    assert!(!error.to_string().contains("not implemented"), "{error}");
}

#[test]
fn lock_resolves_tilde_version_from_local_registry_index() {
    let dir = temp_output_dir("project-lock-registry-tilde");
    std::fs::create_dir_all(dir.join("src")).expect("create project src");
    std::fs::create_dir_all(dir.join("registry/auth/1.2.0/src")).expect("create 1.2.0");
    std::fs::create_dir_all(dir.join("registry/auth/1.2.9/src")).expect("create 1.2.9");
    std::fs::create_dir_all(dir.join("registry/auth/1.3.0/src")).expect("create 1.3.0");
    std::fs::write(dir.join("src/main.orv"), "@out \"lock-tilde\"\n").expect("write entry");
    std::fs::write(
        dir.join("registry/auth/index.json"),
        r#"{"versions":["1.2.0","1.2.9","1.3.0"]}"#,
    )
    .expect("write index");
    std::fs::write(
        dir.join("orv.toml"),
        r#"[project]
name = "shop"
version = "0.1.0"
entry = "src/main.orv"

[dependencies]
auth = { version = "~1.2.0", registry = "registry" }
"#,
    )
    .expect("write manifest");

    cmd_lock(&dir, false).expect("write lock");

    let lock = read_json_value(&dir.join("orv.lock")).expect("read lock");
    assert_eq!(lock["dependencies"][0]["name"], "auth");
    assert_eq!(lock["dependencies"][0]["version"], "1.2.9");
    assert_eq!(lock["dependencies"][0]["requested_version"], "~1.2.0");

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lock_resolves_segment_wildcard_versions_from_local_registry_index() {
    let dir = temp_output_dir("project-lock-registry-wildcard");
    std::fs::create_dir_all(dir.join("src")).expect("create project src");
    std::fs::create_dir_all(dir.join("registry/auth/1.2.0/src")).expect("create auth 1.2.0");
    std::fs::create_dir_all(dir.join("registry/auth/1.2.9/src")).expect("create auth 1.2.9");
    std::fs::create_dir_all(dir.join("registry/auth/1.3.0/src")).expect("create auth 1.3.0");
    std::fs::create_dir_all(dir.join("registry/ui/1.0.0/src")).expect("create ui 1.0.0");
    std::fs::create_dir_all(dir.join("registry/ui/1.4.0/src")).expect("create ui 1.4.0");
    std::fs::create_dir_all(dir.join("registry/ui/2.0.0/src")).expect("create ui 2.0.0");
    std::fs::write(dir.join("src/main.orv"), "@out \"lock-wildcard\"\n").expect("write entry");
    std::fs::write(
        dir.join("registry/auth/index.json"),
        r#"{"versions":["1.2.0","1.2.9","1.3.0"]}"#,
    )
    .expect("write auth index");
    std::fs::write(
        dir.join("registry/ui/index.json"),
        r#"{"versions":["1.0.0","1.4.0","2.0.0"]}"#,
    )
    .expect("write ui index");
    std::fs::write(
        dir.join("orv.toml"),
        r#"[project]
name = "shop"
version = "0.1.0"
entry = "src/main.orv"

[dependencies]
auth = { version = "1.2.*", registry = "registry" }
ui = { version = "1.*", registry = "registry" }
"#,
    )
    .expect("write manifest");

    cmd_lock(&dir, false).expect("write lock");

    let lock = read_json_value(&dir.join("orv.lock")).expect("read lock");
    assert_eq!(lock["dependencies"][0]["name"], "auth");
    assert_eq!(lock["dependencies"][0]["version"], "1.2.9");
    assert_eq!(lock["dependencies"][0]["requested_version"], "1.2.*");
    assert_eq!(lock["dependencies"][1]["name"], "ui");
    assert_eq!(lock["dependencies"][1]["version"], "1.4.0");
    assert_eq!(lock["dependencies"][1]["requested_version"], "1.*");

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lock_resolves_compound_comparator_version_from_local_registry_index() {
    let dir = temp_output_dir("project-lock-registry-comparator");
    std::fs::create_dir_all(dir.join("src")).expect("create project src");
    std::fs::create_dir_all(dir.join("registry/auth/1.2.0/src")).expect("create 1.2.0");
    std::fs::create_dir_all(dir.join("registry/auth/1.9.0/src")).expect("create 1.9.0");
    std::fs::create_dir_all(dir.join("registry/auth/2.0.0/src")).expect("create 2.0.0");
    std::fs::write(dir.join("src/main.orv"), "@out \"lock-comparator\"\n").expect("write entry");
    std::fs::write(
        dir.join("registry/auth/index.json"),
        r#"{"versions":["1.2.0","1.9.0","2.0.0"]}"#,
    )
    .expect("write index");
    std::fs::write(
        dir.join("orv.toml"),
        r#"[project]
name = "shop"
version = "0.1.0"
entry = "src/main.orv"

[dependencies]
auth = { version = ">=1.2.0 <2.0.0", registry = "registry" }
"#,
    )
    .expect("write manifest");

    cmd_lock(&dir, false).expect("write lock");

    let lock = read_json_value(&dir.join("orv.lock")).expect("read lock");
    assert_eq!(lock["dependencies"][0]["name"], "auth");
    assert_eq!(lock["dependencies"][0]["version"], "1.9.0");
    assert_eq!(
        lock["dependencies"][0]["requested_version"],
        ">=1.2.0 <2.0.0"
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lock_preserves_exact_version_with_build_metadata() {
    let dir = temp_output_dir("project-lock-registry-build-metadata");
    std::fs::create_dir_all(dir.join("src")).expect("create project src");
    std::fs::write(dir.join("src/main.orv"), "@out \"lock-build\"\n").expect("write entry");
    std::fs::write(
        dir.join("orv.toml"),
        r#"[project]
name = "shop"
version = "0.1.0"
entry = "src/main.orv"

[dependencies]
auth = "1.2.3+build.7"
"#,
    )
    .expect("write manifest");

    cmd_lock(&dir, false).expect("write lock");

    let lock = read_json_value(&dir.join("orv.lock")).expect("read lock");
    assert_eq!(lock["dependencies"][0]["name"], "auth");
    assert_eq!(lock["dependencies"][0]["version"], "1.2.3+build.7");
    assert!(lock["dependencies"][0].get("requested_version").is_none());

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lock_resolves_prerelease_comparator_version_from_local_registry_index() {
    let dir = temp_output_dir("project-lock-registry-prerelease");
    std::fs::create_dir_all(dir.join("src")).expect("create project src");
    std::fs::create_dir_all(dir.join("registry/auth/1.2.0-alpha.1/src")).expect("create alpha.1");
    std::fs::create_dir_all(dir.join("registry/auth/1.2.0-alpha.2/src")).expect("create alpha.2");
    std::fs::create_dir_all(dir.join("registry/auth/1.2.0/src")).expect("create 1.2.0");
    std::fs::write(dir.join("src/main.orv"), "@out \"lock-prerelease\"\n").expect("write entry");
    std::fs::write(
        dir.join("registry/auth/index.json"),
        r#"{"versions":["1.2.0-alpha.1","1.2.0-alpha.2","1.2.0"]}"#,
    )
    .expect("write index");
    std::fs::write(
        dir.join("orv.toml"),
        r#"[project]
name = "shop"
version = "0.1.0"
entry = "src/main.orv"

[dependencies]
auth = { version = ">=1.2.0-alpha.1 <1.2.0", registry = "registry" }
"#,
    )
    .expect("write manifest");

    cmd_lock(&dir, false).expect("write lock");

    let lock = read_json_value(&dir.join("orv.lock")).expect("read lock");
    assert_eq!(lock["dependencies"][0]["name"], "auth");
    assert_eq!(lock["dependencies"][0]["version"], "1.2.0-alpha.2");
    assert_eq!(
        lock["dependencies"][0]["requested_version"],
        ">=1.2.0-alpha.1 <1.2.0"
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lock_resolves_disjunction_version_from_local_registry_index() {
    let dir = temp_output_dir("project-lock-registry-disjunction");
    std::fs::create_dir_all(dir.join("src")).expect("create project src");
    std::fs::create_dir_all(dir.join("registry/auth/1.2.4/src")).expect("create 1.2.4");
    std::fs::create_dir_all(dir.join("registry/auth/1.3.0/src")).expect("create 1.3.0");
    std::fs::create_dir_all(dir.join("registry/auth/2.1.0/src")).expect("create 2.1.0");
    std::fs::create_dir_all(dir.join("registry/auth/3.0.0/src")).expect("create 3.0.0");
    std::fs::write(dir.join("src/main.orv"), "@out \"lock-disjunction\"\n").expect("write entry");
    std::fs::write(
        dir.join("registry/auth/index.json"),
        r#"{"versions":["1.2.4","1.3.0","2.1.0","3.0.0"]}"#,
    )
    .expect("write index");
    std::fs::write(
        dir.join("orv.toml"),
        r#"[project]
name = "shop"
version = "0.1.0"
entry = "src/main.orv"

[dependencies]
auth = { version = ">=1.2.0 <1.3.0 || >=2.0.0 <3.0.0", registry = "registry" }
"#,
    )
    .expect("write manifest");

    cmd_lock(&dir, false).expect("write lock");

    let lock = read_json_value(&dir.join("orv.lock")).expect("read lock");
    assert_eq!(lock["dependencies"][0]["name"], "auth");
    assert_eq!(lock["dependencies"][0]["version"], "2.1.0");
    assert_eq!(
        lock["dependencies"][0]["requested_version"],
        ">=1.2.0 <1.3.0 || >=2.0.0 <3.0.0"
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn add_and_remove_update_manifest_and_lockfile() {
    let dir = temp_output_dir("project-add-remove");
    std::fs::create_dir_all(dir.join("src")).expect("create src");
    std::fs::write(dir.join("src").join("main.orv"), "@out \"deps\"\n").expect("write entry");
    std::fs::write(
        dir.join("orv.toml"),
        "[project]\nname = \"shop\"\nversion = \"0.1.0\"\nentry = \"src/main.orv\"\n",
    )
    .expect("write manifest");

    cmd_add_dependency(
        &dir,
        "auth",
        Some("1.2.3"),
        false,
        None,
        Some("https://registry.orv.dev"),
    )
    .expect("add registry dependency");
    cmd_add_dependency(
        &dir,
        "ui",
        Some("0.1.0"),
        true,
        Some(Path::new("libs/ui")),
        None,
    )
    .expect("add path dev dependency");

    let manifest = std::fs::read_to_string(dir.join("orv.toml")).expect("read manifest");
    let manifest = toml::from_str::<toml::Value>(&manifest).expect("parse manifest");
    assert_eq!(
        manifest["dependencies"]["auth"]["version"].as_str(),
        Some("1.2.3")
    );
    assert_eq!(
        manifest["dependencies"]["auth"]["registry"].as_str(),
        Some("https://registry.orv.dev")
    );
    assert_eq!(
        manifest["dev-dependencies"]["ui"]["path"].as_str(),
        Some("libs/ui")
    );

    let lock = read_json_value(&dir.join("orv.lock")).expect("read lock");
    assert_eq!(lock["dependencies"][0]["name"], "auth");
    assert_eq!(lock["dev_dependencies"][0]["name"], "ui");

    cmd_remove_dependency(&dir, "auth", false).expect("remove registry dependency");

    let manifest = std::fs::read_to_string(dir.join("orv.toml")).expect("read manifest");
    let manifest = toml::from_str::<toml::Value>(&manifest).expect("parse manifest");
    assert!(manifest
        .get("dependencies")
        .and_then(toml::Value::as_table)
        .is_none_or(toml::map::Map::is_empty));
    assert_eq!(
        manifest["dev-dependencies"]["ui"]["version"].as_str(),
        Some("0.1.0")
    );
    let lock = read_json_value(&dir.join("orv.lock")).expect("read lock");
    assert!(lock["dependencies"].as_array().is_some_and(Vec::is_empty));
    assert_eq!(lock["dev_dependencies"][0]["name"], "ui");

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn workspace_new_updates_root_manifest_and_creates_member_project() {
    let root = temp_output_dir("workspace-new");
    std::fs::create_dir_all(&root).expect("create workspace root");

    cmd_workspace_new(
        &root,
        Path::new("apps/web"),
        Some("web"),
        InitTemplate::Basic,
    )
    .expect("workspace new");

    let root_manifest = std::fs::read_to_string(root.join("orv.toml")).expect("read root manifest");
    let root_manifest = toml::from_str::<toml::Value>(&root_manifest).expect("parse root");
    assert_eq!(root_manifest["workspace"]["resolver"].as_str(), Some("2"));
    assert_eq!(
        root_manifest["workspace"]["members"][0].as_str(),
        Some("apps/web")
    );

    let member_manifest =
        std::fs::read_to_string(root.join("apps/web/orv.toml")).expect("read member manifest");
    let member_manifest = toml::from_str::<toml::Value>(&member_manifest).expect("parse member");
    assert_eq!(member_manifest["project"]["name"].as_str(), Some("web"));
    assert_eq!(
        member_manifest["project"]["entry"].as_str(),
        Some("src/main.orv")
    );
    assert!(root.join("apps/web/src/main.orv").is_file());

    cmd_workspace_new(
        &root,
        Path::new("shared/models"),
        Some("models"),
        InitTemplate::Basic,
    )
    .expect("workspace new second member");
    let root_manifest = std::fs::read_to_string(root.join("orv.toml")).expect("read root manifest");
    let root_manifest = toml::from_str::<toml::Value>(&root_manifest).expect("parse root");
    let members = root_manifest["workspace"]["members"]
        .as_array()
        .expect("members");
    assert_eq!(members.len(), 2);
    assert!(members
        .iter()
        .any(|member| member.as_str() == Some("apps/web")));
    assert!(members
        .iter()
        .any(|member| member.as_str() == Some("shared/models")));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn workspace_graph_merges_member_graphs_and_path_dependency_edges() {
    let root = temp_output_dir("workspace-graph");
    std::fs::create_dir_all(root.join("apps/web/src")).expect("create web src");
    std::fs::create_dir_all(root.join("shared/models/src")).expect("create models src");
    std::fs::write(
        root.join("orv.toml"),
        r#"[workspace]
resolver = "2"
members = ["apps/web", "shared/models"]
"#,
    )
    .expect("write root manifest");
    std::fs::write(
        root.join("apps/web/orv.toml"),
        r#"[project]
name = "web"
version = "0.1.0"
entry = "src/main.orv"

[dependencies]
models = { path = "../../shared/models", version = "0.1.0" }
"#,
    )
    .expect("write web manifest");
    std::fs::write(
        root.join("shared/models/orv.toml"),
        r#"[project]
name = "models"
version = "0.1.0"
entry = "src/main.orv"
"#,
    )
    .expect("write models manifest");
    std::fs::write(
        root.join("apps/web/src/main.orv"),
        "@server { @route GET / { @respond 200 { ok: true } } }\n",
    )
    .expect("write web source");
    std::fs::write(
        root.join("shared/models/src/main.orv"),
        "pub struct User { id: int, name: string }\n",
    )
    .expect("write models source");

    let graph = workspace_graph_json(&root).expect("workspace graph");

    assert_eq!(graph["schema_version"], 1);
    assert_eq!(graph["kind"], "orv.workspace.graph");
    assert_eq!(graph["resolver"], "2");
    assert_eq!(graph["stats"]["member_count"], 2);
    let members = graph["members"].as_array().expect("members");
    assert!(members
        .iter()
        .any(|member| member["path"] == "apps/web" && member["name"] == "web"));
    assert!(members
        .iter()
        .any(|member| member["path"] == "shared/models"
            && member["graph"]["nodes"]
                .as_array()
                .expect("nodes")
                .iter()
                .any(|node| node["kind"] == "struct" && node["name"] == "User")));
    assert!(graph["edges"]
        .as_array()
        .expect("workspace edges")
        .iter()
        .any(|edge| edge["kind"] == "path_dependency"
            && edge["from"] == "apps/web"
            && edge["to"] == "shared/models"
            && edge["package"] == "models"
            && edge["requested_version"] == "0.1.0"
            && edge["target_name"] == "models"
            && edge["target_version"] == "0.1.0"
            && edge["version_match"] == true));

    let out = root.join("target/orv-workspace");
    cmd_workspace_graph(&root, Some(&out), false).expect("write workspace graph");
    assert!(out.join("workspace-graph.json").is_file());
    let written = read_json_value(&out.join("workspace-graph.json")).expect("read written");
    assert_eq!(written["stats"]["member_count"], 2);

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn workspace_graph_view_writes_static_html_artifact() {
    let root = workspace_build_fixture("workspace-graph-view");
    let out = root.join("target/orv-workspace-view");

    cmd_workspace_graph(&root, Some(&out), true).expect("write workspace graph view");

    let graph = read_json_value(&out.join("workspace-graph.json")).expect("read graph");
    assert_eq!(graph["kind"], "orv.workspace.graph");
    let html = std::fs::read_to_string(out.join("index.html")).expect("workspace html");
    assert!(html.contains("ORV Workspace Graph"));
    assert!(html.contains("data-member-count=\"2\""));
    assert!(html.contains("workspace-graph.json"));
    assert!(html.contains("apps/web"));
    assert!(html.contains("shared/models"));
    assert!(html.contains("path_dependency"));
    assert!(html.contains("id=\"workspace-search\""));
    assert!(html.contains("data-workspace-member-row"));
    assert!(html.contains("data-workspace-edge-row"));
    assert!(html.contains("filterWorkspaceGraphRows"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn workspace_graph_rejects_member_path_dependency_version_mismatch() {
    let root = workspace_build_fixture("workspace-graph-version-mismatch");
    std::fs::write(
        root.join("apps/web/orv.toml"),
        r#"[project]
name = "web"
version = "0.1.0"
entry = "src/main.orv"

[dependencies]
models = { path = "../../shared/models", version = "2.0.0" }
"#,
    )
    .expect("write mismatched web manifest");

    let error = workspace_graph_json(&root).expect_err("version mismatch");
    assert!(error.to_string().contains(
            "workspace dependency apps/web -> shared/models requests `2.0.0` but target version is `0.1.0`"
        ));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn workspace_build_writes_member_builds_and_workspace_manifest() {
    let root = workspace_build_fixture("workspace-build");
    let out = root.join("target/orv-workspace-build");
    cmd_workspace_build(&root, &out, BuildProfile::Development, false).expect("workspace build");

    assert!(out.join("workspace-graph.json").is_file());
    assert!(out.join("members/apps/web/build-manifest.json").is_file());
    assert!(out
        .join("members/shared/models/build-manifest.json")
        .is_file());
    let manifest = read_json_value(&out.join("workspace-build.json")).expect("read manifest");
    assert_eq!(manifest["schema_version"], 1);
    assert_eq!(manifest["kind"], "orv.workspace.build");
    assert_eq!(manifest["profile"], "dev");
    assert_eq!(manifest["stats"]["member_count"], 2);
    assert_eq!(manifest["workspace_graph"], "workspace-graph.json");
    assert_eq!(
        manifest["build_order"],
        serde_json::json!(["shared/models", "apps/web"])
    );
    let member_paths = manifest["members"]
        .as_array()
        .expect("members")
        .iter()
        .map(|member| member["path"].as_str().expect("member path"))
        .collect::<Vec<_>>();
    assert_eq!(member_paths, ["shared/models", "apps/web"]);
    assert!(manifest["members"]
        .as_array()
        .expect("members")
        .iter()
        .any(|member| member["path"] == "apps/web"
            && member["build_dir"] == "members/apps/web"
            && member["manifest"] == "members/apps/web/build-manifest.json"));
    assert!(manifest["dependency_edges"]
        .as_array()
        .expect("dependency edges")
        .iter()
        .any(|edge| edge["kind"] == "path_dependency"
            && edge["from"] == "apps/web"
            && edge["to"] == "shared/models"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn workspace_lock_writes_member_locks_and_workspace_manifest() {
    let root = workspace_build_fixture("workspace-lock");
    let out = root.join("target/orv-workspace-lock");
    cmd_workspace_lock(&root, &out).expect("workspace lock");

    assert!(out.join("workspace-graph.json").is_file());
    assert!(out.join("workspace-lock.json").is_file());
    assert!(out.join("members/shared/models/orv.lock").is_file());
    assert!(out.join("members/apps/web/orv.lock").is_file());
    let manifest = read_json_value(&out.join("workspace-lock.json")).expect("read lock");
    assert_eq!(manifest["schema_version"], 1);
    assert_eq!(manifest["kind"], "orv.workspace.lock");
    assert_eq!(manifest["stats"]["member_count"], 2);
    assert_eq!(
        manifest["lock_order"],
        serde_json::json!(["shared/models", "apps/web"])
    );
    assert!(manifest["members"]
        .as_array()
        .expect("members")
        .iter()
        .any(|member| member["path"] == "apps/web"
            && member["lockfile"] == "members/apps/web/orv.lock"
            && member["dependencies"][0]["source"] == "path"
            && member["dependencies"][0]["path"] == "../../shared/models"));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn workspace_fetch_writes_member_dependency_caches() {
    let root = workspace_build_fixture("workspace-fetch");
    let out = root.join("target/orv-workspace-fetch");
    cmd_workspace_fetch(&root, &out).expect("workspace fetch");

    assert!(out.join("workspace-graph.json").is_file());
    assert!(out.join("workspace-lock.json").is_file());
    assert!(out.join("workspace-fetch.json").is_file());
    assert!(out
        .join("members/apps/web/deps/deps-manifest.json")
        .is_file());
    assert!(out
        .join("members/apps/web/deps/packages/dependencies/models/source-bundle.json")
        .is_file());
    assert!(out
        .join("members/shared/models/deps/deps-manifest.json")
        .is_file());
    let manifest = read_json_value(&out.join("workspace-fetch.json")).expect("read fetch");
    assert_eq!(manifest["schema_version"], 1);
    assert_eq!(manifest["kind"], "orv.workspace.dependencies");
    assert_eq!(manifest["stats"]["member_count"], 2);
    assert_eq!(manifest["stats"]["package_count"], 1);
    assert_eq!(
        manifest["fetch_order"],
        serde_json::json!(["shared/models", "apps/web"])
    );
    assert!(manifest["members"]
        .as_array()
        .expect("members")
        .iter()
        .any(|member| member["path"] == "apps/web"
            && member["deps_manifest"] == "members/apps/web/deps/deps-manifest.json"
            && member["package_count"] == 1));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn workspace_build_incremental_skips_unchanged_member_builds() {
    let root = workspace_build_fixture("workspace-build-incremental");
    let out = root.join("target/orv-workspace-build");
    cmd_workspace_build(&root, &out, BuildProfile::Development, false)
        .expect("initial workspace build");

    cmd_workspace_build(&root, &out, BuildProfile::Development, true)
        .expect("incremental workspace build");

    let manifest = read_json_value(&out.join("workspace-build.json")).expect("read manifest");
    assert_eq!(manifest["stats"]["built_count"], 0);
    assert_eq!(manifest["stats"]["skipped_count"], 2);
    assert!(manifest["members"]
        .as_array()
        .expect("members")
        .iter()
        .all(|member| member["status"] == "skipped"
            && member["input_hash"]
                .as_str()
                .is_some_and(|hash| hash.starts_with("fnv1a64:"))));

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn build_writes_static_html_page_for_html_only_entry() {
    let out = temp_output_dir("build-static-page");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        r#"@out @html { @body { @h1 "Home" @p "zero runtime" } }"#,
    )
    .expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");

    let page = build_out.join("pages").join("index.html");
    let html = std::fs::read_to_string(&page).expect("static page");
    assert_eq!(
        html,
        "<html><body><h1>Home</h1><p>zero runtime</p></body></html>"
    );
    let plan: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(build_out.join("bundle-plan.json")).expect("plan"),
    )
    .expect("bundle plan json");
    let static_bundle = plan["bundles"]
        .as_array()
        .expect("bundles array")
        .iter()
        .find(|bundle| bundle["kind"] == "static_page")
        .expect("static page bundle");
    assert_eq!(static_bundle["path"], "pages/index.html");
    assert_eq!(
        static_bundle["runtime_features"]
            .as_array()
            .expect("runtime features")
            .len(),
        0
    );
    assert!(!plan["bundles"]
        .as_array()
        .expect("bundles array")
        .iter()
        .any(|bundle| bundle["kind"] == "server_runtime"));

    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn client_wasm_i32_const_uses_signed_leb128_boundaries() {
    let mut body = Vec::new();
    push_wasm_const_i32_function(&mut body, 64);
    assert_eq!(body, [0x05, 0x00, 0x41, 0xc0, 0x00, 0x0b]);

    body.clear();
    push_wasm_const_i32_function(&mut body, 127);
    assert_eq!(body, [0x05, 0x00, 0x41, 0xff, 0x00, 0x0b]);

    body.clear();
    push_wasm_const_i32_function(&mut body, 128);
    assert_eq!(body, [0x05, 0x00, 0x41, 0x80, 0x01, 0x0b]);
}

#[test]
fn build_writes_client_wasm_for_signal_html_entry() {
    let out = temp_output_dir("build-client-wasm");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        "let sig count: int = 0\n@out @html { @body { @p count } }",
    )
    .expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");

    let manifest = read_json_value(&build_out.join("build-manifest.json")).expect("manifest");
    assert_eq!(manifest["capabilities"]["client_wasm"], true);
    assert!(manifest["capabilities"]["runtime_features"]
        .as_array()
        .expect("runtime features")
        .iter()
        .any(|feature| feature == "client_wasm"));
    let plan = read_json_value(&build_out.join("bundle-plan.json")).expect("plan");
    assert!(plan["bundles"]
        .as_array()
        .expect("bundles")
        .iter()
        .any(|bundle| bundle["kind"] == "client_wasm" && bundle["path"] == "client/app.wasm"));
    assert!(plan["bundles"]
        .as_array()
        .expect("bundles")
        .iter()
        .any(|bundle| bundle["kind"] == "client_js" && bundle["path"] == "client/app.js"));
    assert!(plan["bundles"]
        .as_array()
        .expect("bundles")
        .iter()
        .any(|bundle| bundle["kind"] == "client_page" && bundle["path"] == "pages/index.html"));
    assert!(!plan["bundles"]
        .as_array()
        .expect("bundles")
        .iter()
        .any(|bundle| bundle["kind"] == "static_page"));
    let wasm = std::fs::read(build_out.join("client").join("app.wasm")).expect("client wasm");
    assert_eq!(&wasm[..4], b"\0asm");
    let wasm_text = String::from_utf8_lossy(&wasm);
    assert!(wasm_text.contains("orv.client"));
    assert!(wasm_text.contains("source_bundle"));
    assert!(wasm_text.contains("orv_start"));
    let source_bundle =
        read_json_value(&build_out.join("source-bundle.json")).expect("source bundle");
    let expected_source_bundle_hash = stable_json_hash(&source_bundle).expect("source bundle hash");
    let wasm_metadata = client_wasm_custom_section_payload(&wasm)
        .expect("read wasm metadata")
        .expect("orv metadata section");
    let wasm_metadata: serde_json::Value =
        serde_json::from_slice(wasm_metadata).expect("wasm metadata json");
    assert_eq!(wasm_metadata["entry"], source_bundle["entry"]);
    assert_eq!(
        wasm_metadata["source_bundle_hash"],
        expected_source_bundle_hash
    );
    assert_eq!(wasm_metadata["initial_render"]["content_type"], "text/html");
    assert_eq!(wasm_metadata["initial_render"]["encoding"], "utf-8");
    assert!(wasm_metadata["initial_render"]["html_hash"]
        .as_str()
        .is_some_and(|hash| !hash.is_empty()));
    assert!(
        client_wasm_exports_function(&wasm, "orv_render_ptr").expect("render ptr export"),
        "client wasm must export render pointer"
    );
    assert!(
        client_wasm_exports_function(&wasm, "orv_render_len").expect("render len export"),
        "client wasm must export render length"
    );
    let loader =
        std::fs::read_to_string(build_out.join("client").join("app.js")).expect("client js");
    assert_client_loader_contract(&loader);
    let page =
        std::fs::read_to_string(build_out.join("pages").join("index.html")).expect("client page");
    assert!(page.contains("data-orv-client=\"wasm\""));
    assert!(page.contains("id=\"orv-root\""));
    assert!(page.contains("type=\"module\""));
    assert!(page.contains("../client/app.js"));
    cmd_verify_build(&build_out).expect("verify build");
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn build_prod_records_client_bootstrap_targets() {
    let out = temp_output_dir("build-prod-client");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        r"let sig count: int = 0
@out @html { @body { @p count } }",
    )
    .expect("write entry");
    let build_out = out.join("dist");

    cmd_build_with_profile(&entry, &build_out, BuildProfile::Production).expect("build prod");

    let deploy = read_json_value(&build_out.join("deploy").join("manifest.json")).expect("deploy");
    assert_eq!(deploy["client"]["manifest"], "client/manifest.json");
    assert_eq!(
        deploy["client"]["reactive_plan"],
        "client/reactive-plan.json"
    );
    assert_eq!(deploy["client"]["page"], "pages/index.html");
    assert_eq!(deploy["client"]["loader"], "client/app.js");
    assert_eq!(deploy["client"]["wasm"], "client/app.wasm");
    assert!(deploy["client"]["runtime_features"]
        .as_array()
        .expect("runtime features")
        .iter()
        .any(|feature| feature == "client_wasm"));
    assert_eq!(deploy["client"]["capabilities"]["runtime"], "client_wasm");
    assert_eq!(
        deploy["client"]["capabilities"]["bindings"]["signal_text"],
        1
    );
    assert!(deploy["client"]["blocked_by"]
        .as_array()
        .expect("blocked_by")
        .iter()
        .any(|item| item == "dynamic-client-codegen"));
    assert!(deploy["client"]["blockers"]
        .as_array()
        .expect("blockers")
        .iter()
        .any(|item| item["id"] == "dynamic-client-codegen"));
    cmd_verify_build(&build_out).expect("verify prod build");
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_deploy_client_capability_drift() {
    let out = temp_output_dir("verify-build-deploy-client-capabilities");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        "let sig count: int = 0\n@out @html { @body { @p count } }",
    )
    .expect("write entry");
    let build_out = out.join("dist");

    cmd_build_with_profile(&entry, &build_out, BuildProfile::Production).expect("build prod");
    let deploy_path = build_out.join("deploy").join("manifest.json");
    let mut deploy = read_json_value(&deploy_path).expect("deploy manifest");
    deploy["client"]["capabilities"]["bindings"]["signal_text"] = serde_json::json!(0);
    write_json(&deploy_path, &deploy).expect("write drifted deploy manifest");

    let err = cmd_verify_build(&build_out).expect_err("invalid deploy client capabilities");
    assert!(
        err.to_string()
            .contains("deploy client capabilities do not match client manifest"),
        "{err}"
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_deploy_client_reactive_plan_drift() {
    let out = temp_output_dir("verify-build-deploy-client-reactive-plan");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        "let sig count: int = 0\n@out @html { @body { @p count } }",
    )
    .expect("write entry");
    let build_out = out.join("dist");

    cmd_build_with_profile(&entry, &build_out, BuildProfile::Production).expect("build prod");
    let deploy_path = build_out.join("deploy").join("manifest.json");
    let mut deploy = read_json_value(&deploy_path).expect("deploy manifest");
    deploy["client"]["reactive_plan"] = serde_json::json!("client/other-plan.json");
    write_json(&deploy_path, &deploy).expect("write drifted deploy manifest");

    let err = cmd_verify_build(&build_out).expect_err("invalid deploy client reactive plan");
    assert!(
        err.to_string()
            .contains("deploy client reactive_plan does not match client manifest"),
        "{err}"
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_deploy_client_blocker_drift() {
    let out = temp_output_dir("verify-build-deploy-client-blockers");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        "let sig count: int = 0\n@out @html { @body { @p count } }",
    )
    .expect("write entry");
    let build_out = out.join("dist");

    cmd_build_with_profile(&entry, &build_out, BuildProfile::Production).expect("build prod");
    let deploy_path = build_out.join("deploy").join("manifest.json");
    let mut deploy = read_json_value(&deploy_path).expect("deploy manifest");
    deploy["client"]["blockers"] = serde_json::json!([]);
    write_json(&deploy_path, &deploy).expect("write drifted deploy manifest");

    let err = cmd_verify_build(&build_out).expect_err("invalid deploy client blockers");
    assert!(
        err.to_string()
            .contains("deploy client blockers do not match client manifest"),
        "{err}"
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_accepts_static_page_output() {
    let out = temp_output_dir("verify-build-static");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(&entry, r#"@out @html { @body { @h1 "Home" } }"#).expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");

    cmd_verify_build(&build_out).expect("verify build");
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn build_prod_records_static_page_target() {
    let out = temp_output_dir("build-prod-static-page");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(&entry, r#"@out @html { @body { @h1 "Home" } }"#).expect("write entry");
    let build_out = out.join("dist");

    cmd_build_with_profile(&entry, &build_out, BuildProfile::Production).expect("build prod");

    let deploy = read_json_value(&build_out.join("deploy").join("manifest.json")).expect("deploy");
    assert_eq!(deploy["static"]["path"], "pages/index.html");
    assert!(deploy["static"]["runtime_features"]
        .as_array()
        .expect("runtime features")
        .is_empty());
    assert_eq!(deploy["client"], serde_json::Value::Null);
    assert_eq!(deploy["server"], serde_json::Value::Null);
    cmd_verify_build(&build_out).expect("verify prod static build");
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_deploy_static_target_drift() {
    let out = temp_output_dir("verify-build-deploy-static-drift");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(&entry, r#"@out @html { @body { @h1 "Home" } }"#).expect("write entry");
    let build_out = out.join("dist");

    cmd_build_with_profile(&entry, &build_out, BuildProfile::Production).expect("build prod");
    let deploy_path = build_out.join("deploy").join("manifest.json");
    let mut deploy = read_json_value(&deploy_path).expect("deploy manifest");
    deploy["static"]["path"] = serde_json::json!(SOURCE_BUNDLE_PATH);
    write_json(&deploy_path, &deploy).expect("write drifted deploy manifest");

    let err = cmd_verify_build(&build_out).expect_err("invalid deploy static target");
    assert!(
        err.to_string()
            .contains("deploy static path does not match bundle static_page target"),
        "{err}"
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_missing_deploy_static_target_for_static_bundle() {
    let out = temp_output_dir("verify-build-deploy-static-missing");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(&entry, r#"@out @html { @body { @h1 "Home" } }"#).expect("write entry");
    let build_out = out.join("dist");

    cmd_build_with_profile(&entry, &build_out, BuildProfile::Production).expect("build prod");
    let deploy_path = build_out.join("deploy").join("manifest.json");
    let mut deploy = read_json_value(&deploy_path).expect("deploy manifest");
    deploy["static"] = serde_json::Value::Null;
    write_json(&deploy_path, &deploy).expect("write drifted deploy manifest");

    let err = cmd_verify_build(&build_out).expect_err("missing deploy static target");
    assert!(
        err.to_string()
            .contains("deploy static target missing for bundle static_page"),
        "{err}"
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_invalid_dev_hmr_session_manifest() {
    let out = temp_output_dir("verify-build-dev-hmr-session");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        "let sig count: int = 0\n@out @html { @body { @p count } }",
    )
    .expect("write entry");
    let build_out = out.join("dist");
    let mut stdout = Vec::new();

    dev_with_writer_with_options(&entry, &build_out, true, false, &mut stdout).expect("dev hmr");
    let session_path = build_out.join("dev").join("session.json");
    let mut session = read_json_value(&session_path).expect("dev session");
    session["watch"]["targets"] = serde_json::Value::Array(
        session["watch"]["targets"]
            .as_array()
            .expect("targets")
            .iter()
            .filter(|target| target["kind"] != "client_wasm")
            .cloned()
            .collect(),
    );
    write_json(&session_path, &session).expect("write corrupt dev session");

    let err = cmd_verify_build(&build_out).expect_err("invalid dev hmr session");

    assert!(err
        .to_string()
        .contains("dev session missing bundle target client_wasm:client/app.wasm"));
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_invalid_dev_hmr_transport_manifest() {
    let out = temp_output_dir("verify-build-dev-hmr-transport");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        "let sig count: int = 0\n@out @html { @body { @p count } }",
    )
    .expect("write entry");
    let build_out = out.join("dist");
    let mut stdout = Vec::new();

    dev_with_writer_with_options(&entry, &build_out, true, false, &mut stdout).expect("dev hmr");
    let transport_path = build_out.join("dev").join("transport.json");
    let mut transport = read_json_value(&transport_path).expect("dev hmr transport");
    transport["browser"]["client"] = serde_json::json!("tmp/hmr-client.js");
    write_json(&transport_path, &transport).expect("write corrupt dev hmr transport");

    let err = cmd_verify_build(&build_out).expect_err("invalid dev hmr transport");

    assert!(err
        .to_string()
        .contains("dev hmr transport browser client must be dev/hmr-client.js"));
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_invalid_dev_hmr_server_manifest() {
    let out = temp_output_dir("verify-build-dev-hmr-server");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        "let sig count: int = 0\n@out @html { @body { @p count } }",
    )
    .expect("write entry");
    let build_out = out.join("dist");
    let mut stdout = Vec::new();

    dev_with_writer_with_options(&entry, &build_out, true, true, &mut stdout)
        .expect("dev hmr watch");
    write_dev_watch_events(
        &build_out,
        true,
        1,
        &[dev_watch_loop_event(
            1,
            "initial",
            "build-verify-run",
            "ok",
            Some("sig"),
        )],
    )
    .expect("write events");
    write_dev_hmr_server_manifest(&build_out, "127.0.0.1:1234".parse().expect("addr"))
        .expect("server manifest");
    let server_path = build_out.join("dev").join("server.json");
    let mut server = read_json_value(&server_path).expect("dev hmr server");
    server["endpoints"]["events"] = serde_json::json!("/wrong");
    write_json(&server_path, &server).expect("write corrupt dev hmr server");

    let err = cmd_verify_build(&build_out).expect_err("invalid dev hmr server");

    assert!(err
        .to_string()
        .contains("dev hmr server events endpoint must be /__orv/hmr/events"));
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_invalid_dev_watch_session_manifest() {
    let out = temp_output_dir("verify-build-dev-watch-session");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(&entry, "@out @html { @body { @h1 \"Watch\" } }").expect("write entry");
    let build_out = out.join("dist");
    let mut stdout = Vec::new();

    dev_with_writer_with_options(&entry, &build_out, false, true, &mut stdout).expect("dev watch");
    let session_path = build_out.join("dev").join("watch.json");
    let mut session = read_json_value(&session_path).expect("dev watch session");
    session["loop"]["interval_ms"] = serde_json::json!(0);
    write_json(&session_path, &session).expect("write corrupt dev watch session");

    let err = cmd_verify_build(&build_out).expect_err("invalid dev watch session");

    assert!(err
        .to_string()
        .contains("dev watch session loop interval_ms must be positive"));
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_invalid_dev_watch_transport_path() {
    let out = temp_output_dir("verify-build-dev-watch-transport");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(&entry, "@out @html { @body { @h1 \"Watch\" } }").expect("write entry");
    let build_out = out.join("dist");
    let mut stdout = Vec::new();

    dev_with_writer_with_options(&entry, &build_out, false, true, &mut stdout).expect("dev watch");
    let session_path = build_out.join("dev").join("watch.json");
    let mut session = read_json_value(&session_path).expect("dev watch session");
    session["transport"]["path"] = serde_json::json!("tmp/watch.json");
    write_json(&session_path, &session).expect("write corrupt dev watch session");

    let err = cmd_verify_build(&build_out).expect_err("invalid dev watch transport");

    assert!(err
        .to_string()
        .contains("dev watch session transport path must be dev/watch.json"));
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_client_wasm_without_orv_custom_section() {
    let out = temp_output_dir("verify-build-client-wasm-section");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        r"let sig count: int = 0
@out @html { @body { @p count } }",
    )
    .expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");
    let mut wasm = WASM_MODULE_HEADER.to_vec();
    let mut custom_section = Vec::new();
    push_wasm_len(&mut custom_section, "not.orv".len());
    custom_section.extend_from_slice(b"not.orv");
    custom_section.extend_from_slice(br#"{"note":"orv.client source_bundle"}"#);
    wasm.push(0);
    push_wasm_len(&mut wasm, custom_section.len());
    wasm.extend(custom_section);
    std::fs::write(build_out.join("client").join("app.wasm"), wasm).expect("rewrite wasm");
    refresh_client_manifest_wasm_hash(&build_out);

    let err = cmd_verify_build(&build_out).expect_err("invalid client wasm");

    assert!(
        err.to_string().contains("ORV metadata"),
        "unexpected error: {err}"
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_client_wasm_without_start_export() {
    let out = temp_output_dir("verify-build-client-wasm-export");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        "let sig count: int = 0\n@out @html { @body { @p count } }",
    )
    .expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");
    let original_wasm =
        std::fs::read(build_out.join("client").join("app.wasm")).expect("client wasm");
    let original_metadata = client_wasm_custom_section_payload(&original_wasm)
        .expect("read wasm metadata")
        .expect("orv metadata section")
        .to_vec();
    let mut wasm = WASM_MODULE_HEADER.to_vec();
    let mut custom_section = Vec::new();
    push_wasm_len(&mut custom_section, CLIENT_WASM_CUSTOM_SECTION_NAME.len());
    custom_section.extend_from_slice(CLIENT_WASM_CUSTOM_SECTION_NAME.as_bytes());
    custom_section.extend_from_slice(&original_metadata);
    push_wasm_section(&mut wasm, 0, &custom_section);
    std::fs::write(build_out.join("client").join("app.wasm"), wasm).expect("rewrite wasm");
    refresh_client_manifest_wasm_hash(&build_out);

    let err = cmd_verify_build(&build_out).expect_err("invalid client wasm");

    assert!(
        err.to_string().contains("orv_start"),
        "unexpected error: {err}"
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_client_wasm_start_export_wrong_index() {
    let out = temp_output_dir("verify-build-client-wasm-start-index");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        "let sig count: int = 0\n@out @html { @body { @p count } }",
    )
    .expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");
    let wasm_path = build_out.join("client").join("app.wasm");
    let mut wasm = std::fs::read(&wasm_path).expect("client wasm");
    corrupt_generated_start_export_index(&mut wasm, 1);
    std::fs::write(&wasm_path, wasm).expect("rewrite wasm");
    refresh_client_manifest_wasm_hash(&build_out);

    let err = cmd_verify_build(&build_out).expect_err("invalid client wasm start index");

    assert!(
        err.to_string().contains("orv_start"),
        "unexpected error: {err}"
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_client_wasm_without_memory_export() {
    let out = temp_output_dir("verify-build-client-wasm-memory-export");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        "let sig count: int = 0\n@out @html { @body { @p count } }",
    )
    .expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");
    let wasm_path = build_out.join("client").join("app.wasm");
    let mut wasm = std::fs::read(&wasm_path).expect("client wasm");
    corrupt_generated_memory_export_kind(&mut wasm, 0);
    std::fs::write(&wasm_path, wasm).expect("rewrite wasm");
    refresh_client_manifest_wasm_hash(&build_out);

    let err = cmd_verify_build(&build_out).expect_err("invalid client wasm memory export");

    assert!(
        err.to_string().contains("memory"),
        "unexpected error: {err}"
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_client_wasm_memory_export_wrong_index() {
    let out = temp_output_dir("verify-build-client-wasm-memory-index");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        "let sig count: int = 0\n@out @html { @body { @p count } }",
    )
    .expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");
    let wasm_path = build_out.join("client").join("app.wasm");
    let mut wasm = std::fs::read(&wasm_path).expect("client wasm");
    corrupt_generated_memory_export_index(&mut wasm, 1);
    std::fs::write(&wasm_path, wasm).expect("rewrite wasm");
    refresh_client_manifest_wasm_hash(&build_out);

    let err = cmd_verify_build(&build_out).expect_err("invalid client wasm memory index");

    assert!(
        err.to_string().contains("memory 0"),
        "unexpected error: {err}"
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_client_wasm_initial_render_data_mismatch() {
    let out = temp_output_dir("verify-build-client-wasm-render-data");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        "let sig count: int = 0\n@out @html { @body { @p count } }",
    )
    .expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");
    let wasm_path = build_out.join("client").join("app.wasm");
    let mut wasm = std::fs::read(&wasm_path).expect("client wasm");
    let initial_html = b"<html><body><p>0</p></body></html>";
    let html_offset = wasm
        .windows(initial_html.len())
        .position(|window| window == initial_html)
        .expect("initial render data segment");
    let count_offset = html_offset + b"<html><body><p>".len();
    assert_eq!(wasm[count_offset], b'0');
    wasm[count_offset] = b'1';
    std::fs::write(&wasm_path, wasm).expect("rewrite wasm");
    refresh_client_manifest_wasm_hash(&build_out);

    let err = cmd_verify_build(&build_out).expect_err("invalid client wasm render data");

    assert!(
        err.to_string().contains("initial_render html_hash"),
        "unexpected error: {err}"
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_client_wasm_render_len_export_mismatch() {
    let out = temp_output_dir("verify-build-client-wasm-render-len");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        "let sig count: int = 0\n@out @html { @body { @p count } }",
    )
    .expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");
    let wasm_path = build_out.join("client").join("app.wasm");
    let mut wasm = std::fs::read(&wasm_path).expect("client wasm");
    corrupt_generated_render_len_const(&mut wasm, 0);
    std::fs::write(&wasm_path, wasm).expect("rewrite wasm");
    refresh_client_manifest_wasm_hash(&build_out);

    let err = cmd_verify_build(&build_out).expect_err("invalid client wasm render len");

    assert!(
        err.to_string().contains("orv_render_len"),
        "unexpected error: {err}"
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_client_manifest_wasm_hash_mismatch() {
    let out = temp_output_dir("verify-build-client-manifest-wasm-hash");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        "let sig count: int = 0\n@out @html { @body { @p count } }",
    )
    .expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");
    let manifest_path = build_out.join(CLIENT_MANIFEST_PATH);
    let mut manifest = read_json_value(&manifest_path).expect("client manifest");
    manifest["wasm_hash"] = serde_json::json!("fnv1a64:bad");
    write_json(&manifest_path, &manifest).expect("rewrite client manifest");

    let err = cmd_verify_build(&build_out).expect_err("invalid client manifest wasm hash");

    assert!(
        err.to_string().contains("wasm_hash"),
        "unexpected error: {err}"
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_client_manifest_loader_hash_mismatch() {
    let out = temp_output_dir("verify-build-client-manifest-loader-hash");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        "let sig count: int = 0\n@out @html { @body { @p count } }",
    )
    .expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");
    let manifest_path = build_out.join(CLIENT_MANIFEST_PATH);
    let mut manifest = read_json_value(&manifest_path).expect("client manifest");
    manifest["loader_hash"] = serde_json::json!("fnv1a64:bad");
    write_json(&manifest_path, &manifest).expect("rewrite client manifest");

    let err = cmd_verify_build(&build_out).expect_err("invalid client manifest loader hash");

    assert!(
        err.to_string().contains("loader_hash"),
        "unexpected error: {err}"
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_client_manifest_reactive_plan_hash_mismatch() {
    let out = temp_output_dir("verify-build-client-manifest-reactive-plan-hash");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        "let sig count: int = 0\n@out @html { @body { @p count } }",
    )
    .expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");
    let manifest_path = build_out.join(CLIENT_MANIFEST_PATH);
    let mut manifest = read_json_value(&manifest_path).expect("client manifest");
    manifest["reactive_plan_hash"] = serde_json::json!("fnv1a64:bad");
    write_json(&manifest_path, &manifest).expect("rewrite client manifest");

    let err = cmd_verify_build(&build_out).expect_err("invalid client manifest reactive plan hash");

    assert!(
        err.to_string().contains("reactive_plan_hash"),
        "unexpected error: {err}"
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_client_manifest_initial_render_mismatch() {
    let out = temp_output_dir("verify-build-client-manifest-render-mismatch");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        "let sig count: int = 0\n@out @html { @body { @p count } }",
    )
    .expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");
    let manifest_path = build_out.join(CLIENT_MANIFEST_PATH);
    let mut manifest = read_json_value(&manifest_path).expect("client manifest");
    manifest["initial_render"]["byte_length"] = serde_json::json!(0);
    write_json(&manifest_path, &manifest).expect("rewrite client manifest");

    let err = cmd_verify_build(&build_out).expect_err("invalid client manifest render");

    assert!(
        err.to_string().contains("initial_render"),
        "unexpected error: {err}"
    );
    let _ = std::fs::remove_dir_all(&out);
}

fn corrupt_generated_render_len_const(wasm: &mut [u8], replacement: u8) {
    let mut offset = WASM_MODULE_HEADER.len();
    while offset < wasm.len() {
        let section_id = wasm[offset];
        offset += 1;
        let section_len =
            read_wasm_u32_leb(wasm, &mut offset, wasm.len()).expect("section length") as usize;
        let section_end = offset + section_len;
        if section_id == 10 {
            let mut body_offset = offset;
            let function_count =
                read_wasm_u32_leb(wasm, &mut body_offset, section_end).expect("function count");
            assert_eq!(function_count, 3);
            for ordinal in 0..function_count {
                let body_len = read_wasm_u32_leb(wasm, &mut body_offset, section_end)
                    .expect("body len") as usize;
                let body_start = body_offset;
                let body_end = body_start + body_len;
                if ordinal == 2 {
                    assert_eq!(wasm[body_start], 0x00);
                    assert_eq!(wasm[body_start + 1], 0x41);
                    assert_eq!(wasm[body_end - 1], 0x0b);
                    wasm[body_start + 2] = replacement;
                    return;
                }
                body_offset = body_end;
            }
        }
        offset = section_end;
    }
    panic!("render_len function body not found");
}

fn corrupt_generated_memory_export_kind(wasm: &mut [u8], replacement: u8) {
    let Some(position) = wasm
        .windows(CLIENT_WASM_MEMORY_EXPORT.len())
        .rposition(|window| window == CLIENT_WASM_MEMORY_EXPORT.as_bytes())
    else {
        panic!("memory export name not found");
    };
    let kind_offset = position + CLIENT_WASM_MEMORY_EXPORT.len();
    assert_eq!(wasm[kind_offset], 2);
    wasm[kind_offset] = replacement;
}

fn corrupt_generated_start_export_index(wasm: &mut [u8], replacement: u8) {
    let Some(position) = wasm
        .windows(CLIENT_WASM_START_EXPORT.len())
        .rposition(|window| window == CLIENT_WASM_START_EXPORT.as_bytes())
    else {
        panic!("start export name not found");
    };
    let index_offset = position + CLIENT_WASM_START_EXPORT.len() + 1;
    assert_eq!(wasm[index_offset], 0);
    wasm[index_offset] = replacement;
}

fn corrupt_generated_memory_export_index(wasm: &mut [u8], replacement: u8) {
    let Some(position) = wasm
        .windows(CLIENT_WASM_MEMORY_EXPORT.len())
        .rposition(|window| window == CLIENT_WASM_MEMORY_EXPORT.as_bytes())
    else {
        panic!("memory export name not found");
    };
    let index_offset = position + CLIENT_WASM_MEMORY_EXPORT.len() + 1;
    assert_eq!(wasm[index_offset], 0);
    wasm[index_offset] = replacement;
}

fn refresh_client_manifest_wasm_hash(build_out: &Path) {
    let manifest_path = build_out.join(CLIENT_MANIFEST_PATH);
    let mut manifest = read_json_value(&manifest_path).expect("client manifest");
    let wasm_hash = file_content_hash(&build_out.join(CLIENT_WASM_PATH)).expect("client wasm hash");
    manifest["wasm_hash"] = serde_json::json!(wasm_hash);
    write_json(&manifest_path, &manifest).expect("rewrite client manifest wasm hash");
}

fn refresh_client_manifest_loader_hash(build_out: &Path) {
    let manifest_path = build_out.join(CLIENT_MANIFEST_PATH);
    let mut manifest = read_json_value(&manifest_path).expect("client manifest");
    let loader_hash =
        file_content_hash(&build_out.join(CLIENT_JS_PATH)).expect("client loader hash");
    manifest["loader_hash"] = serde_json::json!(loader_hash);
    write_json(&manifest_path, &manifest).expect("rewrite client manifest loader hash");
}

fn refresh_client_manifest_reactive_plan_hash(build_out: &Path) {
    let manifest_path = build_out.join(CLIENT_MANIFEST_PATH);
    let mut manifest = read_json_value(&manifest_path).expect("client manifest");
    let reactive_plan =
        read_json_value(&build_out.join(CLIENT_REACTIVE_PLAN_PATH)).expect("reactive plan");
    let reactive_plan_hash = stable_json_hash(&reactive_plan).expect("reactive plan hash");
    manifest["reactive_plan_hash"] = serde_json::json!(reactive_plan_hash);
    write_json(&manifest_path, &manifest).expect("rewrite client manifest reactive plan hash");
}

#[test]
fn verify_build_rejects_client_js_without_start_call() {
    let out = temp_output_dir("verify-build-client-js-start");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        "let sig count: int = 0\n@out @html { @body { @p count } }",
    )
    .expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");
    let loader_path = build_out.join("client").join("app.js");
    let loader = std::fs::read_to_string(&loader_path)
        .expect("client loader")
        .replace(
            r#"  if (typeof instance.exports.orv_start === "function") {
    instance.exports.orv_start();
  }
"#,
            "",
        );
    std::fs::write(&loader_path, loader).expect("rewrite loader");
    refresh_client_manifest_loader_hash(&build_out);

    let err = cmd_verify_build(&build_out).expect_err("invalid client loader");

    assert!(
        err.to_string().contains("orv_start"),
        "unexpected error: {err}"
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_client_js_without_source_bundle_hash_check() {
    let out = temp_output_dir("verify-build-client-js-source-hash");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        "let sig count: int = 0\n@out @html { @body { @p count } }",
    )
    .expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");
    let loader_path = build_out.join("client").join("app.js");
    let loader = std::fs::read_to_string(&loader_path)
        .expect("client loader")
        .replace("source bundle hash mismatch", "source bundle hash skipped");
    std::fs::write(&loader_path, loader).expect("rewrite loader");
    refresh_client_manifest_loader_hash(&build_out);

    let err = cmd_verify_build(&build_out).expect_err("invalid client loader");

    assert!(
        err.to_string().contains("source bundle hash"),
        "unexpected error: {err}"
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_client_js_without_manifest_contract_check() {
    let out = temp_output_dir("verify-build-client-js-manifest-contract");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        "let sig count: int = 0\n@out @html { @body { @p count } }",
    )
    .expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");
    let loader_path = build_out.join("client").join("app.js");
    let loader = std::fs::read_to_string(&loader_path)
        .expect("client loader")
        .replace("loadClientManifest", "loadClientContract");
    std::fs::write(&loader_path, loader).expect("rewrite loader");
    refresh_client_manifest_loader_hash(&build_out);

    let err = cmd_verify_build(&build_out).expect_err("invalid client loader");

    assert!(
        err.to_string().contains("client manifest"),
        "unexpected error: {err}"
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_client_js_without_reactive_plan_check() {
    let out = temp_output_dir("verify-build-client-js-reactive-plan");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        "let sig count: int = 0\n@out @html { @body { @p count } }",
    )
    .expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");
    let loader_path = build_out.join("client").join("app.js");
    let loader = std::fs::read_to_string(&loader_path)
        .expect("client loader")
        .replace("loadReactivePlan", "loadReactiveContract");
    std::fs::write(&loader_path, loader).expect("rewrite loader");
    refresh_client_manifest_loader_hash(&build_out);

    let err = cmd_verify_build(&build_out).expect_err("invalid client loader");

    assert!(
        err.to_string().contains("client reactive plan"),
        "unexpected error: {err}"
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_client_js_without_event_arithmetic_actions() {
    let out = temp_output_dir("verify-build-client-js-event-arithmetic");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
            &entry,
            "let sig count: int = 0\n@out @html { @body { @button onClick={count += 1} \"+\" @button onClick={count -= 1} \"-\" } }",
        )
        .expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");
    let loader_path = build_out.join("client").join("app.js");
    let loader = std::fs::read_to_string(&loader_path)
        .expect("client loader")
        .replace("assign_add", "assign_plus")
        .replace("assign_sub", "assign_minus");
    std::fs::write(&loader_path, loader).expect("rewrite loader");
    refresh_client_manifest_loader_hash(&build_out);

    let err = cmd_verify_build(&build_out).expect_err("invalid client loader");

    assert!(
        err.to_string().contains("client reactive plan"),
        "unexpected error: {err}"
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_client_js_without_initial_render_hash_check() {
    let out = temp_output_dir("verify-build-client-js-initial-render");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        "let sig count: int = 0\n@out @html { @body { @p count } }",
    )
    .expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");
    let loader_path = build_out.join("client").join("app.js");
    let loader = std::fs::read_to_string(&loader_path)
        .expect("client loader")
        .replace("validateInitialRender", "skipInitialRenderValidation");
    std::fs::write(&loader_path, loader).expect("rewrite loader");
    refresh_client_manifest_loader_hash(&build_out);

    let err = cmd_verify_build(&build_out).expect_err("invalid client loader");

    assert!(
        err.to_string().contains("initial render"),
        "unexpected error: {err}"
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_client_reactive_plan_without_initial_render_binding() {
    let out = temp_output_dir("verify-build-client-reactive-binding");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        "let sig count: int = 0\n@out @html { @body { @p count } }",
    )
    .expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");
    let plan_path = build_out.join(CLIENT_REACTIVE_PLAN_PATH);
    let mut plan = read_json_value(&plan_path).expect("reactive plan");
    plan["bindings"] = serde_json::json!([]);
    write_json(&plan_path, &plan).expect("write corrupt reactive plan");
    refresh_client_manifest_reactive_plan_hash(&build_out);

    let err = cmd_verify_build(&build_out).expect_err("invalid reactive plan");

    assert!(
        err.to_string().contains("initial_render binding"),
        "unexpected error: {err}"
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_client_reactive_plan_initial_render_mismatch() {
    let out = temp_output_dir("verify-build-client-reactive-render-mismatch");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        "let sig count: int = 0\n@out @html { @body { @p count } }",
    )
    .expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");
    let plan_path = build_out.join(CLIENT_REACTIVE_PLAN_PATH);
    let mut plan = read_json_value(&plan_path).expect("reactive plan");
    let binding = plan["bindings"]
        .as_array_mut()
        .expect("bindings")
        .iter_mut()
        .find(|binding| binding["kind"] == "initial_render")
        .expect("initial render binding");
    binding["byte_length"] = serde_json::json!(0);
    write_json(&plan_path, &plan).expect("write corrupt reactive plan");
    refresh_client_manifest_reactive_plan_hash(&build_out);

    let err = cmd_verify_build(&build_out).expect_err("invalid reactive plan");

    assert!(
        err.to_string().contains("initial_render binding"),
        "unexpected error: {err}"
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_client_reactive_plan_without_signal_state_binding() {
    let out = temp_output_dir("verify-build-client-reactive-state-binding");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        "let sig count: int = 0\n@out @html { @body { @p count } }",
    )
    .expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");
    let plan_path = build_out.join(CLIENT_REACTIVE_PLAN_PATH);
    let mut plan = read_json_value(&plan_path).expect("reactive plan");
    let bindings = plan["bindings"].as_array_mut().expect("bindings");
    bindings.retain(|binding| binding["kind"] != "signal_state");
    write_json(&plan_path, &plan).expect("write corrupt reactive plan");
    refresh_client_manifest_reactive_plan_hash(&build_out);

    let err = cmd_verify_build(&build_out).expect_err("invalid reactive plan");

    assert!(
        err.to_string().contains("signal_state binding"),
        "unexpected error: {err}"
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_client_reactive_plan_invalid_text_condition() {
    let out = temp_output_dir("verify-build-client-reactive-text-condition");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        r#"let sig count: int = 0
@out @html { @body { @p { count > 0 ? "has items" : "empty" } } }"#,
    )
    .expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");
    let plan_path = build_out.join(CLIENT_REACTIVE_PLAN_PATH);
    let mut plan = read_json_value(&plan_path).expect("reactive plan");
    let binding = plan["bindings"]
        .as_array_mut()
        .expect("bindings")
        .iter_mut()
        .find(|binding| binding["kind"] == "signal_text")
        .expect("signal text binding");
    binding["text_condition"]["truthy"] = serde_json::json!(true);
    write_json(&plan_path, &plan).expect("write corrupt reactive plan");
    refresh_client_manifest_reactive_plan_hash(&build_out);

    let err = cmd_verify_build(&build_out).expect_err("invalid reactive plan");

    assert!(
        err.to_string().contains("signal_text binding"),
        "unexpected error: {err}"
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_client_manifest_without_blocker_detail() {
    let out = temp_output_dir("verify-build-client-manifest-blocker-detail");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        "let sig count: int = 0\n@out @html { @body { @p count } }",
    )
    .expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");
    let manifest_path = build_out.join(CLIENT_MANIFEST_PATH);
    let mut manifest = read_json_value(&manifest_path).expect("client manifest");
    manifest["blockers"] = serde_json::json!([]);
    write_json(&manifest_path, &manifest).expect("write corrupt client manifest");

    let err = cmd_verify_build(&build_out).expect_err("invalid client manifest");

    assert!(
        err.to_string().contains(
            "client_manifest blockers must describe blocked_by entry dynamic-client-codegen"
        ),
        "unexpected error: {err}"
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_client_manifest_capability_drift() {
    let out = temp_output_dir("verify-build-client-manifest-capability-drift");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        "let sig count: int = 0\n@out @html { @body { @p count } }",
    )
    .expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");
    let manifest_path = build_out.join(CLIENT_MANIFEST_PATH);
    let mut manifest = read_json_value(&manifest_path).expect("client manifest");
    manifest["capabilities"]["bindings"]["signal_text"] = serde_json::json!(0);
    write_json(&manifest_path, &manifest).expect("write corrupt client manifest");

    let err = cmd_verify_build(&build_out).expect_err("invalid client manifest capabilities");

    assert!(
        err.to_string()
            .contains("client_manifest capabilities do not match reactive plan"),
        "unexpected error: {err}"
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_client_reactive_plan_without_blocker_detail() {
    let out = temp_output_dir("verify-build-client-reactive-plan-blocker-detail");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        "let sig count: int = 0\n@out @html { @body { @p count } }",
    )
    .expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");
    let plan_path = build_out.join(CLIENT_REACTIVE_PLAN_PATH);
    let mut plan = read_json_value(&plan_path).expect("reactive plan");
    plan["blockers"] = serde_json::json!([]);
    write_json(&plan_path, &plan).expect("write corrupt reactive plan");
    refresh_client_manifest_reactive_plan_hash(&build_out);

    let err = cmd_verify_build(&build_out).expect_err("invalid reactive plan");

    assert!(
        err.to_string().contains(
            "client_reactive_plan blockers must describe blocked_by entry reactive-dom-diff"
        ),
        "unexpected error: {err}"
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_build_rejects_missing_static_page_output() {
    let out = temp_output_dir("verify-build-missing-static");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(&entry, r#"@out @html { @body { @h1 "Home" } }"#).expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");
    std::fs::remove_file(build_out.join("pages").join("index.html")).expect("remove page");

    let err = cmd_verify_build(&build_out).expect_err("missing static page");

    let message = err.to_string();
    assert!(
        message.contains("missing bundle target static_page"),
        "unexpected error: {message}"
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn reveal_origin_links_static_html_to_page_output() {
    let out = temp_output_dir("reveal-static-html");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(&entry, r#"@out @html { @body { @h1 "Home" } }"#).expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");
    let origin_map: orv_compiler::OriginMap = serde_json::from_str(
        &std::fs::read_to_string(build_out.join("origin-map.json")).expect("origin map"),
    )
    .expect("origin map json");
    let html = origin_map
        .entries
        .iter()
        .find(|entry| entry.kind == "domain" && entry.name == "html")
        .expect("html origin");

    let reveal = reveal_origin_json(&build_out, &html.id).expect("reveal html origin");

    assert_eq!(reveal["origin"]["kind"], "domain");
    assert_eq!(reveal["origin"]["name"], "html");
    assert!(reveal["source"]["snippet"]
        .as_str()
        .is_some_and(|snippet| snippet.contains("@html")));
    let static_targets = reveal["production"]["static"]
        .as_array()
        .expect("static targets");
    assert!(static_targets.iter().any(|target| {
        target["kind"] == "static_page"
            && target["path"] == "pages/index.html"
            && target["exists"] == true
            && target["verified"] == true
            && target["runtime_features"]
                .as_array()
                .expect("runtime features")
                .is_empty()
    }));
    assert_eq!(reveal["production"]["summary"]["static_target_count"], 1);
    assert_eq!(reveal["production"]["summary"]["static_verified_count"], 1);
    let lsp_reveal = lsp_reveal_json(&build_out, &html.id).expect("lsp reveal html origin");
    assert_eq!(
        lsp_reveal["production"]["summary"]["static_target_count"],
        1
    );
    assert_eq!(
        lsp_reveal["production"]["summary"]["static_verified_count"],
        1
    );
    let editor_reveal =
        editor_reveal_json(&build_out, &html.id).expect("editor reveal html origin");
    assert_eq!(
        editor_reveal["production"]["summary"]["static_target_count"],
        1
    );
    assert_eq!(
        editor_reveal["production"]["summary"]["static_verified_count"],
        1
    );
    let _ = std::fs::remove_dir_all(out);
}

#[test]
fn reveal_origin_links_route_html_to_containing_route_output() {
    let dir = temp_output_dir("reveal-route-html-source");
    std::fs::create_dir_all(&dir).expect("create route html source dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        r#"@server {
  @listen 8080
  @route GET / {
    @serve @html {
      @body { @h1 "Home" }
    }
  }
}
"#,
    )
    .expect("write route html source");
    let out = temp_output_dir("reveal-route-html");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let origin_map: orv_compiler::OriginMap = serde_json::from_str(
        &std::fs::read_to_string(out.join("origin-map.json")).expect("origin map"),
    )
    .expect("origin map json");
    let html = origin_map
        .entries
        .iter()
        .find(|entry| entry.kind == "domain" && entry.name == "html")
        .expect("html origin");

    let reveal = reveal_origin_json(&out, &html.id).expect("reveal html origin");

    assert!(reveal["source"]["snippet"]
        .as_str()
        .is_some_and(|snippet| snippet.contains("@html")));
    let routes = reveal["production"]["routes"]
        .as_array()
        .expect("production routes");
    assert!(routes.iter().any(|route| {
        route["method"] == "GET"
            && route["path"] == "/"
            && route["match"] == "contains"
            && route["matched_origin_id"] == html.id
    }));
    let native_server = reveal["production"]["native_server"]
        .as_array()
        .expect("native server targets");
    assert!(native_server.iter().any(|target| {
        target["routes"]
            .as_array()
            .expect("native routes")
            .iter()
            .any(|route| route["method"] == "GET" && route["path"] == "/")
    }));
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(out);
}

#[test]
fn reveal_origin_links_build_artifact_back_to_source_and_route() {
    let path = workspace_path(&["fixtures", "e2e", "hello.orv"]);
    let out = temp_output_dir("reveal-origin");

    cmd_build(&path, &out).expect("build artifacts");
    let origin_map: orv_compiler::OriginMap = serde_json::from_str(
        &std::fs::read_to_string(out.join("origin-map.json")).expect("origin map"),
    )
    .expect("origin map json");
    let route = origin_map
        .entries
        .iter()
        .find(|entry| entry.kind == "route" && entry.name == "GET /ping")
        .expect("route origin");

    let reveal = reveal_origin_json(&out, &route.id).expect("reveal origin");

    assert_eq!(reveal["schema_version"], 1);
    assert_eq!(reveal["origin"]["id"], route.id);
    assert_eq!(reveal["origin"]["kind"], "route");
    assert_eq!(reveal["origin"]["name"], "GET /ping");
    let canonical_path = std::fs::canonicalize(&path).expect("canonical entry path");
    assert_eq!(
        reveal["source"]["path"],
        canonical_path.display().to_string()
    );
    assert!(reveal["source"]["snippet"]
        .as_str()
        .is_some_and(|snippet| snippet.contains("@route GET /ping")));
    assert_eq!(reveal["project_graph"]["kind"], "domain");
    assert_eq!(reveal["project_graph"]["name"], "route");
    assert!(reveal["production"]["routes"]
        .as_array()
        .expect("routes")
        .iter()
        .any(|route| route["method"] == "GET" && route["path"] == "/ping"));
    let native_server = reveal["production"]["native_server"]
        .as_array()
        .expect("native server targets");
    assert!(native_server.iter().any(|target| {
        target["kind"] == "native_server_plan"
            && target["path"] == "server/native-server.json"
            && target["status"] == "direct_http"
            && target["artifact"] == "server/app.orv-runtime.json"
            && target["target"]["path"] == "server/app"
            && target["routes_source"]["path"] == "server/native/routes.rs"
            && target["routes_source"]["exists"] == true
            && target["routes_source"]["route_count"] == 1
            && target["router_source"]["path"] == "server/native/router.rs"
            && target["router_source"]["exists"] == true
            && target["router_source"]["dispatch"] == true
            && target["router_source"]["handler_count_contract"] == true
            && target["router_source"]["response_origin_dispatch"] == true
            && target["handlers_source"]["path"] == "server/native/handlers.rs"
            && target["handlers_source"]["exists"] == true
            && target["handlers_source"]["handler_count_contract"] == true
            && target["handlers_source"]["body_lowering_placeholder"] == false
            && target["handlers_source"]["response_origin_dispatch"] == true
            && target["runtime_image"]["path"] == "server/runtime-image.json"
            && target["runtime_image"]["reference_image"] == "ghcr.io/orv-lang/orv-reference:latest"
            && target["runtime_image"]["target"]["image"] == "orv-native-server:latest"
            && target["commands"]["build"]
                == serde_json::json!([
                    "cargo",
                    "build",
                    "--manifest-path",
                    "server/native/Cargo.toml",
                    "--release"
                ])
            && target["commands"]["run"]["env"]["ORV_BUILD_DIR"] == "."
            && target["commands"]["run"]["command"]
                == serde_json::json!(["./server/native/target/release/orv-native-server"])
            && target["routes"]
                .as_array()
                .expect("native routes")
                .iter()
                .any(|route| route["method"] == "GET" && route["path"] == "/ping")
            && target["blocked_by"]
                .as_array()
                .expect("blocked_by")
                .iter()
                .all(|item| item != "native-codegen")
    }));
    assert_eq!(reveal["production"]["summary"]["route_target_count"], 1);
    assert_eq!(
        reveal["production"]["summary"]["native_server_target_count"],
        1
    );
    assert_eq!(
        reveal["production"]["summary"]["native_server_route_count"],
        1
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn reveal_origin_exposes_route_policy_contract() {
    let dir = temp_output_dir("reveal-route-policy-source");
    std::fs::create_dir_all(&dir).expect("create route policy reveal source dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        r#"@server {
  @listen 8080
  @route POST /checkout {
    @csrf
    @respond 201 { ok: true }
  }
}
"#,
    )
    .expect("write route policy reveal source");
    let out = temp_output_dir("reveal-route-policy");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let origin_map: orv_compiler::OriginMap = serde_json::from_str(
        &std::fs::read_to_string(out.join("origin-map.json")).expect("origin map"),
    )
    .expect("origin map json");
    let route = origin_map
        .entries
        .iter()
        .find(|entry| entry.kind == "route" && entry.name == "POST /checkout")
        .expect("checkout route origin");

    let reveal = reveal_origin_json(&out, &route.id).expect("reveal origin");
    let routes = reveal["production"]["routes"]
        .as_array()
        .expect("production routes");
    let route = routes
        .iter()
        .find(|route| route["method"] == "POST" && route["path"] == "/checkout")
        .expect("checkout production route");
    let policies = route["policies"].as_array().expect("route policies");

    assert!(policies.iter().any(|policy| policy["kind"] == "csrf"
        && policy["required"] == true
        && policy["origin_id"]
            .as_str()
            .is_some_and(|origin_id| origin_id.starts_with("ori_"))));
    assert!(policies.iter().any(|policy| policy["kind"] == "rate_limit"
        && policy["limit"] == 10
        && policy["window_seconds"] == 60));
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(out);
}

#[test]
fn reveal_origin_exposes_deploy_commerce_adapter_contract() {
    let dir = temp_output_dir("reveal-commerce-adapters-source");
    std::fs::create_dir_all(&dir).expect("create commerce reveal source dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        r#"@server {
  @listen 8080
  let payments = @payment.connect(@env.PAYMENT_ADAPTER_URL ?? "http://payments.internal/capture")
  @route POST /checkout {
    let captured = payments.capture({ orderId: "o_1", amount: 42, method: "card" })
    @respond 200 { payment: captured.status }
  }
}
"#,
    )
    .expect("write commerce reveal source");
    let out = temp_output_dir("reveal-commerce-adapters");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let origin_map: orv_compiler::OriginMap = serde_json::from_str(
        &std::fs::read_to_string(out.join("origin-map.json")).expect("origin map"),
    )
    .expect("origin map json");
    let route = origin_map
        .entries
        .iter()
        .find(|entry| entry.kind == "route" && entry.name == "POST /checkout")
        .expect("checkout route origin");

    let reveal = reveal_origin_json(&out, &route.id).expect("reveal origin");

    let commerce = reveal["production"]["commerce_adapters"]
        .as_array()
        .expect("commerce adapters");
    assert!(commerce.iter().any(|target| {
        target["path"] == "deploy/commerce-adapters.json"
            && target["exists"] == true
            && target["adapters"][0]["kind"] == "payment"
            && target["adapters"][0]["env"] == "PAYMENT_ADAPTER_URL"
            && target["adapters"][0]["endpoint"] == "http://payments.internal/capture"
            && target["adapters"][0]["request"]["kind"] == "payment.capture"
            && target["adapters"][0]["source_origin_id"]
                .as_str()
                .is_some_and(|origin_id| origin_id.starts_with("ori_"))
    }));
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(out);
}

#[test]
fn reveal_origin_links_commerce_connects_to_deploy_adapter_contract() {
    let dir = temp_output_dir("reveal-commerce-connect-origin-source");
    std::fs::create_dir_all(&dir).expect("create commerce connect reveal source dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        r#"@server {
  @listen 8080
  let payments = @payment.connect(@env.PAYMENT_ADAPTER_URL ?? "http://payments.internal/capture")
  let shipping = @shipping.connect(@env.SHIPPING_ADAPTER_URL ?? "http://shipping.internal/book")
  @route POST /checkout {
    let captured = payments.capture({ orderId: "o_1", amount: 42, method: "card" })
    let booked = shipping.book({ orderId: "o_1", carrier: "post", address: "Seoul" })
    @respond 200 { payment: captured.status, shipment: booked.status }
  }
}
"#,
    )
    .expect("write commerce connect reveal source");
    let out = temp_output_dir("reveal-commerce-connect-origin");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let origin_map: orv_compiler::OriginMap = serde_json::from_str(
        &std::fs::read_to_string(out.join("origin-map.json")).expect("origin map"),
    )
    .expect("origin map json");
    for (origin_name, kind, endpoint) in [
        (
            "@payment.connect",
            "payment",
            "http://payments.internal/capture",
        ),
        (
            "@shipping.connect",
            "shipping",
            "http://shipping.internal/book",
        ),
    ] {
        let origin = origin_map
            .entries
            .iter()
            .find(|entry| entry.kind == "call" && entry.name == origin_name)
            .expect("commerce connect origin");
        let reveal = reveal_origin_json(&out, &origin.id).expect("reveal commerce origin");
        let target = reveal["production"]["commerce_adapters"]
            .as_array()
            .expect("commerce adapters")
            .iter()
            .find(|target| target["path"] == "deploy/commerce-adapters.json")
            .expect("commerce adapter target")
            .clone();
        let matched = target["matched_adapters"]
            .as_array()
            .expect("matched commerce adapters");

        assert_eq!(target["matched"], true);
        assert_eq!(target["selected_origin_id"], origin.id);
        assert_eq!(target["matched_adapter_count"], 1);
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0]["source_origin_id"], origin.id);
        assert_eq!(matched[0]["matched_origin_id"], origin.id);
        assert_eq!(matched[0]["match"], "direct");
        assert_eq!(matched[0]["kind"], kind);
        assert_eq!(matched[0]["endpoint"], endpoint);
    }
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(out);
}

#[test]
fn reveal_origin_exposes_deploy_db_adapter_contract() {
    let dir = temp_output_dir("reveal-db-adapters-source");
    std::fs::create_dir_all(&dir).expect("create db reveal source dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        r#"@server {
  @listen 8080
  let shopdb = @db.connect(@env.SHOP_DATABASE_URL ?? "postgres://db.internal/shop")
  @route GET /ping { @respond 200 { ok: true } }
}
"#,
    )
    .expect("write db reveal source");
    let out = temp_output_dir("reveal-db-adapters");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let origin_map: orv_compiler::OriginMap = serde_json::from_str(
        &std::fs::read_to_string(out.join("origin-map.json")).expect("origin map"),
    )
    .expect("origin map json");
    let route = origin_map
        .entries
        .iter()
        .find(|entry| entry.kind == "route" && entry.name == "GET /ping")
        .expect("ping route origin");

    let reveal = reveal_origin_json(&out, &route.id).expect("reveal origin");

    let db_adapters = reveal["production"]["db_adapters"]
        .as_array()
        .expect("db adapters");
    assert!(db_adapters.iter().any(|target| {
        target["path"] == "deploy/db-adapters.json"
            && target["exists"] == true
            && target["adapters"][0]["kind"] == "db"
            && target["adapters"][0]["provider"] == "postgres"
            && target["adapters"][0]["env"] == "SHOP_DATABASE_URL"
            && target["adapters"][0]["endpoint"] == "postgres://db.internal/shop"
            && target["adapters"][0]["adapter_status"] == "unsupported_runtime"
            && target["adapters"][0]["source_origin_id"]
                .as_str()
                .is_some_and(|origin_id| origin_id.starts_with("ori_"))
    }));
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(out);
}

#[test]
fn reveal_origin_links_db_connect_to_deploy_adapter_contract() {
    let dir = temp_output_dir("reveal-db-connect-origin-source");
    std::fs::create_dir_all(&dir).expect("create db connect reveal source dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        r#"@server {
  @listen 8080
  let shopdb = @db.connect(@env.SHOP_DATABASE_URL ?? "postgres://db.internal/shop")
  @route GET /ping { @respond 200 { ok: true } }
}
"#,
    )
    .expect("write db connect reveal source");
    let out = temp_output_dir("reveal-db-connect-origin");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let origin_map: orv_compiler::OriginMap = serde_json::from_str(
        &std::fs::read_to_string(out.join("origin-map.json")).expect("origin map"),
    )
    .expect("origin map json");
    let db_connect = origin_map
        .entries
        .iter()
        .find(|entry| entry.kind == "call" && entry.name == "@db.connect")
        .expect("db connect origin");

    let reveal = reveal_origin_json(&out, &db_connect.id).expect("reveal db connect origin");
    let db_adapters = reveal["production"]["db_adapters"]
        .as_array()
        .expect("db adapters");
    let target = db_adapters
        .iter()
        .find(|target| target["path"] == "deploy/db-adapters.json")
        .expect("db adapter target");
    let matched = target["matched_adapters"]
        .as_array()
        .expect("matched db adapters");

    assert_eq!(target["matched"], true);
    assert_eq!(target["selected_origin_id"], db_connect.id);
    assert_eq!(target["matched_adapter_count"], 1);
    assert_eq!(matched.len(), 1);
    assert_eq!(matched[0]["source_origin_id"], db_connect.id);
    assert_eq!(matched[0]["matched_origin_id"], db_connect.id);
    assert_eq!(matched[0]["match"], "direct");
    assert_eq!(matched[0]["provider"], "postgres");
    assert_eq!(matched[0]["bridge"]["contract"], "http-json-v1");
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(out);
}

#[test]
fn reveal_origin_exposes_deploy_preflight_contract() {
    let dir = temp_output_dir("reveal-preflight-source");
    std::fs::create_dir_all(&dir).expect("create preflight reveal source dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        r#"@server {
  @listen 8080
  let shopdb = @db.connect(@env.SHOP_DATABASE_URL)
  @route GET /ping { @respond 200 { ok: true } }
}
"#,
    )
    .expect("write preflight reveal source");
    let out = temp_output_dir("reveal-preflight");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let origin_map: orv_compiler::OriginMap = serde_json::from_str(
        &std::fs::read_to_string(out.join("origin-map.json")).expect("origin map"),
    )
    .expect("origin map json");
    let route = origin_map
        .entries
        .iter()
        .find(|entry| entry.kind == "route" && entry.name == "GET /ping")
        .expect("ping route origin");

    let reveal = reveal_origin_json(&out, &route.id).expect("reveal origin");

    let preflight = reveal["production"]["preflight"]
        .as_array()
        .expect("preflight targets");
    assert!(preflight.iter().any(|target| {
        target["path"] == "deploy/preflight.json"
            && target["exists"] == true
            && target["commands"]["verify_build"] == "orv verify-build ."
            && target["commands"]["env_check"] == "orv deploy-env-check ."
            && target["commands"]["benchmark_report"] == "orv benchmark-report ."
            && target["commands"]["benchmark_report_require_pass"]
                == "orv benchmark-report . --require-pass"
            && target["artifacts"]["smoke_test"] == "deploy/smoke-test.sh"
            && target["artifacts"]["smoke_output"] == "deploy/smoke-output.txt"
            && target["artifacts"]["benchmark_evidence"] == "deploy/benchmark-evidence.json"
            && target["benchmark"]["kind"] == "orv.benchmark.shop_5h"
            && target["benchmark"]["max_elapsed_minutes"] == 300
            && target["benchmark_evidence"]["exists"] == true
            && target["benchmark_evidence"]["path"] == "deploy/benchmark-evidence.json"
            && target["benchmark_evidence"]["recording_status"] == "not_recorded"
            && target["benchmark_evidence"]["report_status"] == "incomplete"
            && target["benchmark_evidence"]["task_count"] == 10
            && target["benchmark_evidence"]["recorded_task_count"] == 0
            && target["benchmark_evidence"]["missing_task_count"] == 10
            && target["benchmark_evidence"]["missing_data_count"] == 3
            && target["benchmark_evidence"]["smoke_test_required_markers"]
                == serde_json::json!(deploy_benchmark::SMOKE_REQUIRED_MARKERS)
            && target["benchmark_evidence"]["smoke_test_summary"]["present"] == false
            && target["benchmark_evidence"]["smoke_test_summary"]["required_markers"]
                == serde_json::json!(deploy_benchmark::SMOKE_REQUIRED_MARKERS)
            && target["benchmark_evidence"]["smoke_test_output_source"].is_null()
            && target["benchmark_evidence"]["missing_data"]
                .as_array()
                .expect("missing data")
                .iter()
                .any(|item| item == "smoke_test_output")
            && target["routes"][0]["method"] == "GET"
            && target["routes"][0]["path"] == "/ping"
            && target["required_env"][0]["kind"] == "db"
            && target["required_env"][0]["env"] == "SHOP_DATABASE_URL"
            && target["required_env"][0]["required"] == true
    }));
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(out);
}

#[test]
fn reveal_origin_links_client_signal_to_client_bundle_targets() {
    let out = temp_output_dir("reveal-client-origin");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        "let sig count: int = 0\n@out @html { @body { @p count } }",
    )
    .expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");
    let origin_map: orv_compiler::OriginMap = serde_json::from_str(
        &std::fs::read_to_string(build_out.join("origin-map.json")).expect("origin map"),
    )
    .expect("origin map json");
    let signal = origin_map
        .entries
        .iter()
        .find(|entry| entry.kind == "signal" && entry.name == "count")
        .expect("signal origin");

    let reveal = reveal_origin_json(&build_out, &signal.id).expect("reveal origin");

    assert_eq!(reveal["origin"]["kind"], "signal");
    assert!(reveal["source"]["snippet"]
        .as_str()
        .is_some_and(|snippet| snippet.contains("let sig count")));
    let client = reveal["production"]["client"]
        .as_array()
        .expect("client targets");
    assert!(client.iter().any(|target| {
        target["kind"] == "client_manifest"
            && target["path"] == CLIENT_MANIFEST_PATH
            && target["source_bundle"] == SOURCE_BUNDLE_PATH
            && target["source_bundle_hash"]
                .as_str()
                .is_some_and(|hash| !hash.is_empty())
            && target["wasm_hash"]
                .as_str()
                .is_some_and(|hash| !hash.is_empty())
            && target["capabilities"]["runtime"] == "client_wasm"
            && target["capabilities"]["bindings"]["signal_text"] == 1
            && target["capabilities"]["surfaces"]
                .as_array()
                .expect("manifest capability surfaces")
                .iter()
                .any(|surface| surface == "signal_text")
            && target["blockers"]
                .as_array()
                .expect("manifest blockers")
                .iter()
                .any(|blocker| {
                    blocker["id"] == "dynamic-client-codegen"
                        && blocker["artifact"] == CLIENT_JS_PATH
                })
    }));
    assert!(client.iter().any(|target| {
        target["kind"] == "client_reactive_plan"
            && target["path"] == CLIENT_REACTIVE_PLAN_PATH
            && target["signal_count"] == 1
            && target["source_bundle_hash"]
                .as_str()
                .is_some_and(|hash| !hash.is_empty())
            && target["blockers"]
                .as_array()
                .expect("reactive blockers")
                .iter()
                .any(|blocker| {
                    blocker["id"] == "reactive-dom-diff"
                        && blocker["artifact"] == CLIENT_REACTIVE_PLAN_PATH
                })
    }));
    assert!(client
        .iter()
        .any(|target| target["kind"] == "client_page" && target["path"] == "pages/index.html"));
    assert!(client
        .iter()
        .any(|target| target["kind"] == "client_js" && target["path"] == "client/app.js"));
    assert!(client
        .iter()
        .any(|target| target["kind"] == "client_wasm" && target["path"] == "client/app.wasm"));
    assert!(reveal["production"]["routes"]
        .as_array()
        .expect("routes")
        .is_empty());
    assert_eq!(reveal["production"]["summary"]["client_target_count"], 5);
    assert_eq!(reveal["production"]["summary"]["client_manifest_count"], 1);
    assert!(
        reveal["production"]["summary"]["client_capability_surface_count"]
            .as_u64()
            .is_some_and(|count| count >= 2)
    );
    let lsp_reveal = lsp_reveal_json(&build_out, &signal.id).expect("lsp reveal");
    assert_eq!(
        lsp_reveal["production"]["summary"]["client_target_count"],
        5
    );
    assert_eq!(
        lsp_reveal["production"]["summary"]["client_manifest_count"],
        1
    );
    let editor_reveal = editor_reveal_json(&build_out, &signal.id).expect("editor reveal");
    assert_eq!(
        editor_reveal["production"]["summary"]["client_target_count"],
        5
    );
    assert_eq!(
        editor_reveal["production"]["summary"]["client_manifest_count"],
        1
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn build_writes_client_bundle_manifest_contract() {
    let out = temp_output_dir("client-bundle-manifest");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        "let sig count: int = 0\n@out @html { @body { @p count } }",
    )
    .expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");

    let manifest_path = build_out.join(CLIENT_MANIFEST_PATH);
    assert!(
        manifest_path.is_file(),
        "missing {}",
        manifest_path.display()
    );
    let client_manifest = read_json_value(&manifest_path).expect("client manifest");
    let source_bundle =
        read_json_value(&build_out.join("source-bundle.json")).expect("source bundle");
    let expected_source_hash = stable_json_hash(&source_bundle).expect("source hash");
    let expected_wasm_hash =
        file_content_hash(&build_out.join(CLIENT_WASM_PATH)).expect("wasm hash");
    let expected_loader_hash =
        file_content_hash(&build_out.join(CLIENT_JS_PATH)).expect("loader hash");
    let reactive_plan =
        read_json_value(&build_out.join(CLIENT_REACTIVE_PLAN_PATH)).expect("reactive plan");
    let expected_reactive_plan_hash = stable_json_hash(&reactive_plan).expect("reactive plan hash");
    assert_manifest_artifact(
        &build_out.join("build-manifest.json"),
        "client_manifest",
        CLIENT_MANIFEST_PATH,
    );
    assert_bundle_target(
        &build_out.join("bundle-plan.json"),
        "client_manifest",
        CLIENT_MANIFEST_PATH,
    );
    assert_eq!(client_manifest["kind"], "orv.client.bundle");
    assert_eq!(client_manifest["page"], "pages/index.html");
    assert_eq!(client_manifest["loader"], "client/app.js");
    assert_eq!(client_manifest["loader_hash"], expected_loader_hash);
    assert_eq!(
        client_manifest["reactive_plan_hash"],
        expected_reactive_plan_hash
    );
    assert_eq!(client_manifest["wasm"], "client/app.wasm");
    assert_eq!(client_manifest["wasm_hash"], expected_wasm_hash);
    assert_eq!(client_manifest["source_bundle"], "source-bundle.json");
    assert_eq!(client_manifest["source_bundle_hash"], expected_source_hash);
    assert_eq!(
        client_manifest["exports"]["start"],
        CLIENT_WASM_START_EXPORT
    );
    assert_eq!(
        client_manifest["exports"]["render_ptr"],
        CLIENT_WASM_RENDER_PTR_EXPORT
    );
    assert_eq!(
        client_manifest["exports"]["render_len"],
        CLIENT_WASM_RENDER_LEN_EXPORT
    );
    assert_eq!(client_manifest["capabilities"]["runtime"], "client_wasm");
    assert_eq!(
        client_manifest["capabilities"]["source"],
        CLIENT_REACTIVE_PLAN_PATH
    );
    assert_eq!(client_manifest["capabilities"]["signals"], 1);
    assert_eq!(
        client_manifest["capabilities"]["bindings"]["signal_state"],
        1
    );
    assert_eq!(
        client_manifest["capabilities"]["bindings"]["signal_text"],
        1
    );
    let capability_surfaces = client_manifest["capabilities"]["surfaces"]
        .as_array()
        .expect("capability surfaces");
    assert!(capability_surfaces
        .iter()
        .any(|surface| surface == "signal_text"));
    assert!(capability_surfaces
        .iter()
        .any(|surface| surface == "embedded_reactive_plan"));
    assert!(client_manifest["blocked_by"]
        .as_array()
        .expect("blocked_by")
        .iter()
        .any(|item| item == "dynamic-client-codegen"));
    assert!(client_manifest["blockers"]
        .as_array()
        .expect("blockers")
        .iter()
        .any(|item| item["id"] == "dynamic-client-codegen" && item["artifact"] == CLIENT_JS_PATH));

    cmd_verify_build(&build_out).expect("verify build artifacts");
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn build_writes_client_reactive_plan_contract() {
    let out = temp_output_dir("client-reactive-plan");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        "let sig count: int = 0\n@out @html { @body { @p count } }",
    )
    .expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");

    let reactive_path = build_out.join("client").join("reactive-plan.json");
    assert!(
        reactive_path.is_file(),
        "missing {}",
        reactive_path.display()
    );
    let reactive_plan = read_json_value(&reactive_path).expect("reactive plan");
    let source_bundle =
        read_json_value(&build_out.join("source-bundle.json")).expect("source bundle");
    let expected_source_hash = stable_json_hash(&source_bundle).expect("source hash");
    assert_manifest_artifact(
        &build_out.join("build-manifest.json"),
        "client_reactive_plan",
        "client/reactive-plan.json",
    );
    assert_bundle_target(
        &build_out.join("bundle-plan.json"),
        "client_reactive_plan",
        "client/reactive-plan.json",
    );
    assert_eq!(reactive_plan["kind"], "orv.client.reactive_plan");
    assert_eq!(reactive_plan["source_bundle"], SOURCE_BUNDLE_PATH);
    assert_eq!(reactive_plan["source_bundle_hash"], expected_source_hash);
    assert!(reactive_plan["signals"]
        .as_array()
        .expect("signals")
        .iter()
        .any(|signal| signal["name"] == "count"
            && signal["state_key"] == "count"
            && signal["initial_value"]["kind"] == "int"
            && signal["initial_value"]["value"] == "0"
            && signal["origin_id"]
                .as_str()
                .is_some_and(|id| !id.is_empty())));
    assert!(reactive_plan["bindings"]
        .as_array()
        .expect("bindings")
        .iter()
        .any(|binding| binding["kind"] == "initial_render"
            && binding["target"] == CLIENT_PAGE_PATH
            && binding["source"] == CLIENT_WASM_PATH));
    assert!(reactive_plan["bindings"]
        .as_array()
        .expect("bindings")
        .iter()
        .any(|binding| binding["kind"] == "signal_state"
            && binding["target"] == CLIENT_JS_PATH
            && binding["state_key"] == "count"
            && binding["source"].as_str().is_some_and(|id| !id.is_empty())));
    assert!(reactive_plan["bindings"]
        .as_array()
        .expect("bindings")
        .iter()
        .any(|binding| binding["kind"] == "signal_text"
            && binding["target"] == CLIENT_PAGE_PATH
            && binding["state_key"] == "count"
            && binding["selector"] == "p"
            && binding["source"].as_str().is_some_and(|id| !id.is_empty())));
    assert!(reactive_plan["blocked_by"]
        .as_array()
        .expect("blocked_by")
        .iter()
        .any(|item| item == "reactive-dom-diff"));
    assert!(!reactive_plan["blocked_by"]
        .as_array()
        .expect("blocked_by")
        .iter()
        .any(|item| item == "dynamic-client-codegen"));
    assert!(reactive_plan["blockers"]
        .as_array()
        .expect("blockers")
        .iter()
        .any(|item| item["id"] == "reactive-dom-diff"
            && item["artifact"] == CLIENT_REACTIVE_PLAN_PATH));
    let client_manifest =
        read_json_value(&build_out.join(CLIENT_MANIFEST_PATH)).expect("client manifest");
    assert_eq!(
        client_manifest["reactive_plan"],
        "client/reactive-plan.json"
    );
    let loader = std::fs::read_to_string(build_out.join(CLIENT_JS_PATH)).expect("client loader");
    let bootstrap = client_loader_bootstrap_json(&loader);
    assert_eq!(bootstrap["embeddedReactivePlan"], reactive_plan);
    assert_eq!(
        bootstrap["embeddedReactivePlanHash"],
        stable_json_hash(&reactive_plan).expect("reactive plan hash")
    );

    cmd_verify_build(&build_out).expect("verify build artifacts");
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn build_writes_client_signal_text_template_binding_contract() {
    let out = temp_output_dir("client-reactive-text-template-binding");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        r#"let sig count: int = 0
@out @html { @body { @p "count: {count}" @button onClick={count += 1} "+" } }"#,
    )
    .expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");

    let reactive_plan =
        read_json_value(&build_out.join(CLIENT_REACTIVE_PLAN_PATH)).expect("reactive plan");
    assert!(reactive_plan["bindings"]
        .as_array()
        .expect("bindings")
        .iter()
        .any(|binding| binding["kind"] == "signal_text"
            && binding["target"] == CLIENT_PAGE_PATH
            && binding["state_key"] == "count"
            && binding["selector"] == "p"
            && binding["text_template"]
                .as_array()
                .is_some_and(|segments| segments.iter().any(|segment| {
                    segment["kind"] == "signal" && segment["state_key"] == "count"
                }))
            && binding["source"].as_str().is_some_and(|id| !id.is_empty())));
    let loader = std::fs::read_to_string(build_out.join(CLIENT_JS_PATH)).expect("client loader");
    assert!(loader.contains("renderSignalTextBinding"));
    assert!(loader.contains("text_template"));
    cmd_verify_build(&build_out).expect("verify build artifacts");
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn build_writes_client_multi_signal_text_template_binding_contract() {
    let out = temp_output_dir("client-reactive-multi-signal-text-template-binding");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        r#"let sig label: string = "Items"
let sig count: int = 0
@out @html { @body { @p "{label}: {count}" } }"#,
    )
    .expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");

    let reactive_plan =
        read_json_value(&build_out.join(CLIENT_REACTIVE_PLAN_PATH)).expect("reactive plan");
    assert!(reactive_plan["bindings"]
        .as_array()
        .expect("bindings")
        .iter()
        .any(|binding| binding["kind"] == "signal_text"
            && binding["target"] == CLIENT_PAGE_PATH
            && binding["selector"] == "p"
            && binding["state_keys"] == serde_json::json!(["label", "count"])
            && binding["sources"].as_array().is_some_and(|sources| {
                sources.iter().any(|source| source["state_key"] == "label")
                    && sources.iter().any(|source| source["state_key"] == "count")
            })
            && binding["text_template"]
                == serde_json::json!([
                    {"kind": "signal", "state_key": "label"},
                    {"kind": "text", "value": ": "},
                    {"kind": "signal", "state_key": "count"},
                ])));
    let loader = std::fs::read_to_string(build_out.join(CLIENT_JS_PATH)).expect("client loader");
    assert!(loader.contains("signalTextBindingStateKeys"));
    assert!(loader.contains("state_keys"));
    cmd_verify_build(&build_out).expect("verify build artifacts");
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn build_writes_client_signal_text_condition_binding_contract() {
    let out = temp_output_dir("client-reactive-text-condition-binding");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        r#"let sig count: int = 0
@out @html { @body { @p { count > 0 ? "has items" : "empty" } } }"#,
    )
    .expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");

    let reactive_plan =
        read_json_value(&build_out.join(CLIENT_REACTIVE_PLAN_PATH)).expect("reactive plan");
    assert!(reactive_plan["bindings"]
        .as_array()
        .expect("bindings")
        .iter()
        .any(|binding| binding["kind"] == "signal_text"
            && binding["target"] == CLIENT_PAGE_PATH
            && binding["state_key"] == "count"
            && binding["selector"] == "p"
            && binding["text_condition"]["state_key"] == "count"
            && binding["text_condition"]["op"] == "gt"
            && binding["text_condition"]["rhs"]["kind"] == "int"
            && binding["text_condition"]["rhs"]["value"] == "0"
            && binding["text_condition"]["truthy"] == "has items"
            && binding["text_condition"]["falsy"] == "empty"
            && binding["source"].as_str().is_some_and(|id| !id.is_empty())));
    let loader = std::fs::read_to_string(build_out.join(CLIENT_JS_PATH)).expect("client loader");
    assert!(loader.contains("renderSignalTextCondition"));
    assert!(loader.contains("text_condition"));
    cmd_verify_build(&build_out).expect("verify build artifacts");
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn build_writes_client_duplicate_signal_slot_cursor_contract() {
    let out = temp_output_dir("client-reactive-duplicate-slot-cursors");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        r#"let sig first: string = "same"
let sig second: string = "same"
@out @html { @body {
  @p first
  @p second
  @input value={first}
  @input value={second}
} }"#,
    )
    .expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");

    let reactive_plan =
        read_json_value(&build_out.join(CLIENT_REACTIVE_PLAN_PATH)).expect("reactive plan");
    let text_bindings = reactive_plan["bindings"]
        .as_array()
        .expect("bindings")
        .iter()
        .filter(|binding| binding["kind"] == "signal_text")
        .collect::<Vec<_>>();
    assert_eq!(text_bindings.len(), 2);
    assert!(text_bindings
        .iter()
        .any(|binding| binding["state_key"] == "first" && binding["selector"] == "p"));
    assert!(text_bindings
        .iter()
        .any(|binding| binding["state_key"] == "second" && binding["selector"] == "p"));
    let attr_bindings = reactive_plan["bindings"]
        .as_array()
        .expect("bindings")
        .iter()
        .filter(|binding| binding["kind"] == "signal_attr")
        .collect::<Vec<_>>();
    assert_eq!(attr_bindings.len(), 2);
    assert!(attr_bindings.iter().any(|binding| {
        binding["state_key"] == "first"
            && binding["selector"] == "input"
            && binding["attr"] == "value"
    }));
    assert!(attr_bindings.iter().any(|binding| {
        binding["state_key"] == "second"
            && binding["selector"] == "input"
            && binding["attr"] == "value"
    }));
    let loader = std::fs::read_to_string(build_out.join(CLIENT_JS_PATH)).expect("client loader");
    assert!(loader.contains("signalTextBindingCursorKey"));
    assert!(loader.contains("signalAttrBindingCursorKey"));
    cmd_verify_build(&build_out).expect("verify build artifacts");
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn build_writes_client_signal_attr_binding_contract() {
    let out = temp_output_dir("client-reactive-attr-binding");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        "let sig input: string = \"hi\"\n@out @html { @body { @input value={input} } }",
    )
    .expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");

    let reactive_plan =
        read_json_value(&build_out.join(CLIENT_REACTIVE_PLAN_PATH)).expect("reactive plan");
    assert!(reactive_plan["bindings"]
        .as_array()
        .expect("bindings")
        .iter()
        .any(|binding| binding["kind"] == "signal_attr"
            && binding["target"] == CLIENT_PAGE_PATH
            && binding["state_key"] == "input"
            && binding["selector"] == "input"
            && binding["attr"] == "value"
            && binding["source"].as_str().is_some_and(|id| !id.is_empty())));
    let loader = std::fs::read_to_string(build_out.join(CLIENT_JS_PATH)).expect("client loader");
    assert_client_loader_contract(&loader);
    cmd_verify_build(&build_out).expect("verify build artifacts");
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn build_writes_client_signal_attr_template_binding_contract() {
    let out = temp_output_dir("client-reactive-attr-template-binding");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        r#"let sig input: string = "hi"
@out @html { @body { @input placeholder="{input}!" } }"#,
    )
    .expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");

    let reactive_plan =
        read_json_value(&build_out.join(CLIENT_REACTIVE_PLAN_PATH)).expect("reactive plan");
    assert!(reactive_plan["bindings"]
        .as_array()
        .expect("bindings")
        .iter()
        .any(|binding| binding["kind"] == "signal_attr"
            && binding["target"] == CLIENT_PAGE_PATH
            && binding["state_key"] == "input"
            && binding["selector"] == "input"
            && binding["attr"] == "placeholder"
            && binding["attr_template"]
                .as_array()
                .is_some_and(|segments| segments.iter().any(|segment| {
                    segment["kind"] == "signal" && segment["state_key"] == "input"
                }))
            && binding["source"].as_str().is_some_and(|id| !id.is_empty())));
    let loader = std::fs::read_to_string(build_out.join(CLIENT_JS_PATH)).expect("client loader");
    assert!(loader.contains("renderSignalAttrBinding"));
    assert!(loader.contains("attr_template"));
    cmd_verify_build(&build_out).expect("verify build artifacts");
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn build_writes_client_multi_signal_attr_template_binding_contract() {
    let out = temp_output_dir("client-reactive-multi-signal-attr-template-binding");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        r#"let sig prefix: string = "cart"
let sig count: int = 0
@out @html { @body { @input placeholder="{prefix}-{count}" } }"#,
    )
    .expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");

    let reactive_plan =
        read_json_value(&build_out.join(CLIENT_REACTIVE_PLAN_PATH)).expect("reactive plan");
    assert!(reactive_plan["bindings"]
        .as_array()
        .expect("bindings")
        .iter()
        .any(|binding| binding["kind"] == "signal_attr"
            && binding["target"] == CLIENT_PAGE_PATH
            && binding["selector"] == "input"
            && binding["attr"] == "placeholder"
            && binding["state_keys"] == serde_json::json!(["prefix", "count"])
            && binding["sources"].as_array().is_some_and(|sources| {
                sources.iter().any(|source| source["state_key"] == "prefix")
                    && sources.iter().any(|source| source["state_key"] == "count")
            })
            && binding["attr_template"]
                == serde_json::json!([
                    {"kind": "signal", "state_key": "prefix"},
                    {"kind": "text", "value": "-"},
                    {"kind": "signal", "state_key": "count"},
                ])));
    let loader = std::fs::read_to_string(build_out.join(CLIENT_JS_PATH)).expect("client loader");
    assert!(loader.contains("signalAttrBindingStateKeys"));
    assert!(loader.contains("state_keys"));
    cmd_verify_build(&build_out).expect("verify build artifacts");
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn build_writes_client_signal_attr_condition_binding_contract() {
    let out = temp_output_dir("client-reactive-attr-condition-binding");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        r#"let sig active: bool = false
@out @html { @body { @button class={active ? "enabled" : "disabled"} "Save" } }"#,
    )
    .expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");

    let reactive_plan =
        read_json_value(&build_out.join(CLIENT_REACTIVE_PLAN_PATH)).expect("reactive plan");
    assert!(reactive_plan["bindings"]
        .as_array()
        .expect("bindings")
        .iter()
        .any(|binding| binding["kind"] == "signal_attr"
            && binding["target"] == CLIENT_PAGE_PATH
            && binding["state_key"] == "active"
            && binding["selector"] == "button"
            && binding["attr"] == "class"
            && binding["attr_condition"]["state_key"] == "active"
            && binding["attr_condition"]["truthy"] == "enabled"
            && binding["attr_condition"]["falsy"] == "disabled"
            && binding["source"].as_str().is_some_and(|id| !id.is_empty())));
    let loader = std::fs::read_to_string(build_out.join(CLIENT_JS_PATH)).expect("client loader");
    assert!(loader.contains("renderSignalAttrCondition"));
    assert!(loader.contains("attr_condition"));
    cmd_verify_build(&build_out).expect("verify build artifacts");
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn build_writes_client_signal_attr_comparison_condition_binding_contract() {
    let out = temp_output_dir("client-reactive-attr-comparison-condition-binding");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        r#"let sig count: int = 0
@out @html { @body { @button class={count > 0 ? "enabled" : "disabled"} "Save" } }"#,
    )
    .expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");

    let reactive_plan =
        read_json_value(&build_out.join(CLIENT_REACTIVE_PLAN_PATH)).expect("reactive plan");
    assert!(reactive_plan["bindings"]
        .as_array()
        .expect("bindings")
        .iter()
        .any(|binding| binding["kind"] == "signal_attr"
            && binding["target"] == CLIENT_PAGE_PATH
            && binding["state_key"] == "count"
            && binding["selector"] == "button"
            && binding["attr"] == "class"
            && binding["attr_condition"]["state_key"] == "count"
            && binding["attr_condition"]["op"] == "gt"
            && binding["attr_condition"]["rhs"]["kind"] == "int"
            && binding["attr_condition"]["rhs"]["value"] == "0"
            && binding["attr_condition"]["truthy"] == "enabled"
            && binding["attr_condition"]["falsy"] == "disabled"
            && binding["source"].as_str().is_some_and(|id| !id.is_empty())));
    let loader = std::fs::read_to_string(build_out.join(CLIENT_JS_PATH)).expect("client loader");
    assert!(loader.contains("compareSignalAttrCondition"));
    assert!(loader.contains("decodeSignalConditionOperand"));
    cmd_verify_build(&build_out).expect("verify build artifacts");
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn build_writes_client_signal_event_binding_contract() {
    let out = temp_output_dir("client-reactive-event-binding");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
            &entry,
            "let sig count: int = 0\n@out @html { @body { @p count @button onClick={count += 1} \"+\" } }",
        )
        .expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");

    let reactive_plan =
        read_json_value(&build_out.join(CLIENT_REACTIVE_PLAN_PATH)).expect("reactive plan");
    assert!(reactive_plan["bindings"]
        .as_array()
        .expect("bindings")
        .iter()
        .any(|binding| binding["kind"] == "signal_event"
            && binding["target"] == CLIENT_PAGE_PATH
            && binding["state_key"] == "count"
            && binding["selector"] == "button"
            && binding["event"] == "click"
            && binding["action"]["kind"] == "assign_add"
            && binding["action"]["value"]["kind"] == "int"
            && binding["action"]["value"]["value"] == "1"
            && binding["source"].as_str().is_some_and(|id| !id.is_empty())));
    let loader = std::fs::read_to_string(build_out.join(CLIENT_JS_PATH)).expect("client loader");
    assert_client_loader_contract(&loader);
    cmd_verify_build(&build_out).expect("verify build artifacts");
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn build_writes_client_signal_event_toggle_binding_contract() {
    let out = temp_output_dir("client-reactive-event-toggle-binding");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
            &entry,
            "let sig muted: bool = false\n@out @html { @body { @button onClick={muted = !muted} \"mute\" } }",
        )
        .expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");

    let reactive_plan =
        read_json_value(&build_out.join(CLIENT_REACTIVE_PLAN_PATH)).expect("reactive plan");
    assert!(reactive_plan["bindings"]
        .as_array()
        .expect("bindings")
        .iter()
        .any(|binding| binding["kind"] == "signal_event"
            && binding["target"] == CLIENT_PAGE_PATH
            && binding["state_key"] == "muted"
            && binding["selector"] == "button"
            && binding["event"] == "click"
            && binding["action"]["kind"] == "assign_toggle"
            && binding["source"].as_str().is_some_and(|id| !id.is_empty())));
    let loader = std::fs::read_to_string(build_out.join(CLIENT_JS_PATH)).expect("client loader");
    assert!(loader.contains("assign_toggle"));
    cmd_verify_build(&build_out).expect("verify build artifacts");
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn build_writes_client_signal_event_input_value_binding_contract() {
    let out = temp_output_dir("client-reactive-event-input-value-binding");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        r#"let sig email: string = ""
@out @html { @body { @input value={email} onInput={(e) -> email = e.target.value} } }"#,
    )
    .expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");

    let reactive_plan =
        read_json_value(&build_out.join(CLIENT_REACTIVE_PLAN_PATH)).expect("reactive plan");
    assert!(reactive_plan["bindings"]
        .as_array()
        .expect("bindings")
        .iter()
        .any(|binding| binding["kind"] == "signal_event"
            && binding["target"] == CLIENT_PAGE_PATH
            && binding["state_key"] == "email"
            && binding["selector"] == "input"
            && binding["event"] == "input"
            && binding["action"]["kind"] == "assign_event_target_value"
            && binding["source"].as_str().is_some_and(|id| !id.is_empty())));
    let loader = std::fs::read_to_string(build_out.join(CLIENT_JS_PATH)).expect("client loader");
    assert!(loader.contains("assign_event_target_value"));
    cmd_verify_build(&build_out).expect("verify build artifacts");
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn build_writes_client_signal_event_input_checked_binding_contract() {
    let out = temp_output_dir("client-reactive-event-input-checked-binding");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
            &entry,
            r#"let sig accepted: bool = false
@out @html { @body { @input type="checkbox" checked={accepted} onChange={(e) -> accepted = e.target.checked} } }"#,
        )
        .expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");

    let reactive_plan =
        read_json_value(&build_out.join(CLIENT_REACTIVE_PLAN_PATH)).expect("reactive plan");
    assert!(reactive_plan["bindings"]
        .as_array()
        .expect("bindings")
        .iter()
        .any(|binding| binding["kind"] == "signal_event"
            && binding["target"] == CLIENT_PAGE_PATH
            && binding["state_key"] == "accepted"
            && binding["selector"] == "input"
            && binding["event"] == "change"
            && binding["action"]["kind"] == "assign_event_target_checked"
            && binding["source"].as_str().is_some_and(|id| !id.is_empty())));
    let loader = std::fs::read_to_string(build_out.join(CLIENT_JS_PATH)).expect("client loader");
    assert!(loader.contains("assign_event_target_checked"));
    cmd_verify_build(&build_out).expect("verify build artifacts");
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn build_writes_client_signal_event_numeric_input_value_binding_contract() {
    let out = temp_output_dir("client-reactive-event-numeric-input-binding");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        r#"let sig volume: float = 1.0
let sig quantity: int = 1
@out @html { @body {
  @input value={volume} onInput={(e) -> volume = float.from(e.target.value)}
  @input value={quantity} onInput={(e) -> quantity = int.from(e.target.value)}
} }"#,
    )
    .expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");

    let reactive_plan =
        read_json_value(&build_out.join(CLIENT_REACTIVE_PLAN_PATH)).expect("reactive plan");
    assert!(reactive_plan["bindings"]
        .as_array()
        .expect("bindings")
        .iter()
        .any(|binding| binding["kind"] == "signal_event"
            && binding["state_key"] == "volume"
            && binding["event"] == "input"
            && binding["action"]["kind"] == "assign_event_target_value_float"));
    assert!(reactive_plan["bindings"]
        .as_array()
        .expect("bindings")
        .iter()
        .any(|binding| binding["kind"] == "signal_event"
            && binding["state_key"] == "quantity"
            && binding["event"] == "input"
            && binding["action"]["kind"] == "assign_event_target_value_int"));
    let loader = std::fs::read_to_string(build_out.join(CLIENT_JS_PATH)).expect("client loader");
    assert!(loader.contains("assign_event_target_value_float"));
    assert!(loader.contains("assign_event_target_value_int"));
    cmd_verify_build(&build_out).expect("verify build artifacts");
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn reveal_origin_uses_build_source_bundle_when_original_client_source_is_missing() {
    let out = temp_output_dir("reveal-client-source-bundle");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        "let sig count: int = 0\n@out @html { @body { @p count } }",
    )
    .expect("write entry");
    let build_out = out.join("dist");

    cmd_build(&entry, &build_out).expect("build artifacts");
    let origin_map: orv_compiler::OriginMap = serde_json::from_str(
        &std::fs::read_to_string(build_out.join("origin-map.json")).expect("origin map"),
    )
    .expect("origin map json");
    let signal = origin_map
        .entries
        .iter()
        .find(|entry| entry.kind == "signal" && entry.name == "count")
        .expect("signal origin");
    std::fs::remove_file(&entry).expect("remove original source");

    let reveal = reveal_origin_json(&build_out, &signal.id).expect("reveal origin");

    assert!(reveal["source"]["snippet"]
        .as_str()
        .is_some_and(|snippet| snippet.contains("let sig count")));
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn lsp_reveal_returns_location_for_build_origin() {
    let dir = temp_output_dir("lsp-reveal");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        r"@server {
  @listen 0
  @route GET /ping {
    @respond 200 { ok: true }
  }
}",
    )
    .expect("write source");
    let out = dir.join("dist");

    cmd_build(&path, &out).expect("build artifacts");
    let origin_map: orv_compiler::OriginMap = serde_json::from_str(
        &std::fs::read_to_string(out.join("origin-map.json")).expect("origin map"),
    )
    .expect("origin map json");
    let route = origin_map
        .entries
        .iter()
        .find(|entry| entry.kind == "route" && entry.name == "GET /ping")
        .expect("route origin");

    let reveal = lsp_reveal_json(&out, &route.id).expect("lsp reveal");

    assert_eq!(reveal["schema_version"], 1);
    assert_eq!(reveal["origin"]["id"], route.id);
    let canonical_path = std::fs::canonicalize(&path).expect("canonical source path");
    assert_eq!(
        reveal["location"]["uri"],
        canonical_path.display().to_string()
    );
    assert_eq!(reveal["location"]["range"]["start"]["line"], 2);
    assert_eq!(reveal["location"]["range"]["start"]["character"], 2);
    assert!(reveal["production"]["routes"]
        .as_array()
        .expect("routes")
        .iter()
        .any(|route| route["method"] == "GET" && route["path"] == "/ping"));
    assert_eq!(reveal["production"]["summary"]["route_target_count"], 1);
    assert_eq!(
        reveal["production"]["summary"]["native_server_target_count"],
        1
    );
    assert_eq!(
        reveal["production"]["summary"]["native_server_route_count"],
        1
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_reveal_uses_build_source_bundle_when_original_source_is_missing() {
    let dir = temp_output_dir("lsp-reveal-source-bundle");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("page.orv");
    std::fs::write(
        &path,
        r#"let sig count: int = 0
@out @html { @body { @p count } }"#,
    )
    .expect("write source");
    let out = dir.join("dist");

    cmd_build(&path, &out).expect("build artifacts");
    let origin_map: orv_compiler::OriginMap = serde_json::from_str(
        &std::fs::read_to_string(out.join("origin-map.json")).expect("origin map"),
    )
    .expect("origin map json");
    let signal = origin_map
        .entries
        .iter()
        .find(|entry| entry.kind == "signal" && entry.name == "count")
        .expect("signal origin");
    std::fs::remove_file(&path).expect("remove source");

    let reveal = lsp_reveal_json(&out, &signal.id).expect("lsp reveal");

    assert_eq!(reveal["origin"]["kind"], "signal");
    assert_eq!(reveal["location"]["range"]["start"]["line"], 0);
    assert!(reveal["production"]["client"]
        .as_array()
        .expect("client targets")
        .iter()
        .any(|target| target["kind"] == "client_wasm"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn lsp_reveal_exposes_db_adapter_origin_match() {
    let dir = temp_output_dir("lsp-reveal-db-adapter");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        r#"@server {
  @listen 8080
  let shopdb = @db.connect(@env.SHOP_DATABASE_URL ?? "postgres://db.internal/shop")
  @route GET /ping { @respond 200 { ok: true } }
}
"#,
    )
    .expect("write source");
    let out = dir.join("dist");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let origin_map: orv_compiler::OriginMap = serde_json::from_str(
        &std::fs::read_to_string(out.join("origin-map.json")).expect("origin map"),
    )
    .expect("origin map json");
    let db_connect = origin_map
        .entries
        .iter()
        .find(|entry| entry.kind == "call" && entry.name == "@db.connect")
        .expect("db connect origin");

    let reveal = lsp_reveal_json(&out, &db_connect.id).expect("lsp reveal");
    let db_adapters = reveal["production"]["db_adapters"]
        .as_array()
        .expect("db adapters");
    let target = db_adapters
        .iter()
        .find(|target| target["path"] == "deploy/db-adapters.json")
        .expect("db adapter target");
    let matched = target["matched_adapters"]
        .as_array()
        .expect("matched db adapters");

    assert_eq!(reveal["origin"]["id"], db_connect.id);
    assert_eq!(
        reveal["production"]["graph_contract"]
            .as_array()
            .expect("graph contract")
            .len(),
        3
    );
    assert_eq!(reveal["production"]["summary"]["graph_contract_count"], 3);
    assert_eq!(reveal["production"]["summary"]["preflight_target_count"], 1);
    assert_eq!(
        reveal["production"]["summary"]["preflight_smoke_summary_present_count"],
        0
    );
    assert_eq!(
        reveal["production"]["summary"]["preflight_smoke_summary_missing_count"],
        1
    );
    assert_eq!(
        reveal["production"]["summary"]["preflight_smoke_summary_missing_marker_count"],
        0
    );
    assert_eq!(reveal["production"]["summary"]["db_target_count"], 1);
    assert_eq!(target["matched"], true);
    assert_eq!(target["selected_origin_id"], db_connect.id);
    assert_eq!(target["matched_adapter_count"], 1);
    assert_eq!(matched.len(), 1);
    assert_eq!(matched[0]["source_origin_id"], db_connect.id);
    assert_eq!(matched[0]["matched_origin_id"], db_connect.id);
    assert_eq!(matched[0]["match"], "direct");
    assert_eq!(matched[0]["provider"], "postgres");
    assert_eq!(matched[0]["bridge"]["contract"], "http-json-v1");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn editor_reveal_focuses_route_origin_for_native_navigation() {
    let dir = temp_output_dir("editor-reveal");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        r"@server {
  @listen 0
  @route GET /ping {
    @respond 200 { ok: true }
  }
}",
    )
    .expect("write source");
    let out = dir.join("dist");

    cmd_build(&path, &out).expect("build artifacts");
    let origin_map: orv_compiler::OriginMap = serde_json::from_str(
        &std::fs::read_to_string(out.join("origin-map.json")).expect("origin map"),
    )
    .expect("origin map json");
    let route = origin_map
        .entries
        .iter()
        .find(|entry| entry.kind == "route" && entry.name == "GET /ping")
        .expect("route origin");

    let reveal = editor_reveal_json(&out, &route.id).expect("editor reveal");

    assert_eq!(reveal["schema_version"], 1);
    assert_eq!(reveal["origin"]["id"], route.id);
    assert_eq!(reveal["focus"]["panel"], "routes");
    assert_eq!(reveal["focus"]["origin_id"], route.id);
    assert_eq!(reveal["source"]["location"]["range"]["start"]["line"], 2);
    assert!(reveal["source"]["snippet"]
        .as_str()
        .is_some_and(|snippet| snippet.contains("@route GET /ping")));
    assert!(reveal["production"]["routes"]
        .as_array()
        .expect("routes")
        .iter()
        .any(|route| route["method"] == "GET" && route["path"] == "/ping"));
    assert_eq!(reveal["production"]["summary"]["route_target_count"], 1);
    assert_eq!(
        reveal["production"]["summary"]["native_server_target_count"],
        1
    );
    assert_eq!(
        reveal["production"]["summary"]["native_server_route_count"],
        1
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn editor_reveal_exposes_commerce_adapter_origin_match() {
    let dir = temp_output_dir("editor-reveal-commerce-adapter");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        r#"@server {
  @listen 8080
  let payments = @payment.connect(@env.PAYMENT_ADAPTER_URL ?? "http://payments.internal/capture")
  @route POST /checkout {
    let captured = payments.capture({ orderId: "o_1", amount: 42, method: "card" })
    @respond 200 { payment: captured.status }
  }
}
"#,
    )
    .expect("write source");
    let out = dir.join("dist");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let origin_map: orv_compiler::OriginMap = serde_json::from_str(
        &std::fs::read_to_string(out.join("origin-map.json")).expect("origin map"),
    )
    .expect("origin map json");
    let payment_connect = origin_map
        .entries
        .iter()
        .find(|entry| entry.kind == "call" && entry.name == "@payment.connect")
        .expect("payment connect origin");

    let reveal = editor_reveal_json(&out, &payment_connect.id).expect("editor reveal");
    let commerce = reveal["production"]["commerce_adapters"]
        .as_array()
        .expect("commerce adapters");
    let target = commerce
        .iter()
        .find(|target| target["path"] == "deploy/commerce-adapters.json")
        .expect("commerce adapter target");
    let matched = target["matched_adapters"]
        .as_array()
        .expect("matched commerce adapters");

    assert_eq!(reveal["origin"]["id"], payment_connect.id);
    assert_eq!(reveal["production"]["summary"]["graph_contract_count"], 3);
    assert_eq!(
        reveal["production"]["summary"]["preflight_smoke_summary_missing_count"],
        1
    );
    assert_eq!(reveal["production"]["summary"]["commerce_target_count"], 1);
    assert_eq!(reveal["focus"]["origin_id"], payment_connect.id);
    assert_eq!(reveal["focus"]["panel"], "source");
    assert_eq!(target["matched"], true);
    assert_eq!(target["selected_origin_id"], payment_connect.id);
    assert_eq!(target["matched_adapter_count"], 1);
    assert_eq!(matched.len(), 1);
    assert_eq!(matched[0]["source_origin_id"], payment_connect.id);
    assert_eq!(matched[0]["matched_origin_id"], payment_connect.id);
    assert_eq!(matched[0]["match"], "direct");
    assert_eq!(matched[0]["kind"], "payment");
    assert_eq!(matched[0]["endpoint"], "http://payments.internal/capture");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn editor_trace_links_request_origin_to_source_navigation() {
    let dir = temp_output_dir("editor-trace");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        r#"@server {
  @listen 0
  @route GET /ping {
    @respond 200 { ok: true }
  }
}"#,
    )
    .expect("write source");
    let out = dir.join("dist");

    cmd_build(&path, &out).expect("build artifacts");
    let origin_map: orv_compiler::OriginMap = serde_json::from_str(
        &std::fs::read_to_string(out.join("origin-map.json")).expect("origin map"),
    )
    .expect("origin map json");
    let route = origin_map
        .entries
        .iter()
        .find(|entry| entry.kind == "route" && entry.name == "GET /ping")
        .expect("route origin");
    let trace_path = dir.join("production-trace.json");
    write_json(
        &trace_path,
        &serde_json::json!({
            "schema_version": 1,
            "kind": "orv.production.trace",
            "frames": [{
                "method": "GET",
                "path": "/ping",
                "status": 200,
                "route_origin_id": route.id,
            }],
        }),
    )
    .expect("write trace");

    let trace = editor_trace_json(&out, &trace_path).expect("editor trace");

    assert_eq!(trace["schema_version"], 1);
    assert_eq!(trace["kind"], "orv.editor.trace");
    assert_eq!(trace["trace"]["frame_count"], 1);
    assert_eq!(trace["live_refresh"]["strategy"], "trace-file-hash");
    assert_eq!(
        trace["live_refresh"]["watch"]["trace"]["path"],
        trace_path.display().to_string()
    );
    assert!(trace["live_refresh"]["watch"]["trace"]["content_hash"]
        .as_str()
        .is_some_and(|hash| hash.starts_with("fnv1a64:")));
    assert_eq!(trace["frames"][0]["request"]["method"], "GET");
    assert_eq!(trace["frames"][0]["request"]["path"], "/ping");
    assert_eq!(trace["frames"][0]["origin_id"], route.id);
    assert_eq!(trace["frames"][0]["navigation"]["focus"]["panel"], "routes");
    assert!(trace["frames"][0]["navigation"]["source"]["snippet"]
        .as_str()
        .is_some_and(|snippet| snippet.contains("@route GET /ping")));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn editor_trace_links_response_origin_to_source_navigation() {
    let dir = temp_output_dir("editor-trace-response");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        r#"@server {
  @listen 0
  @route GET /ping {
    @respond 200 { ok: true }
  }
}"#,
    )
    .expect("write source");
    let out = dir.join("dist");

    cmd_build(&path, &out).expect("build artifacts");
    let origin_map: orv_compiler::OriginMap = serde_json::from_str(
        &std::fs::read_to_string(out.join("origin-map.json")).expect("origin map"),
    )
    .expect("origin map json");
    let route = origin_map
        .entries
        .iter()
        .find(|entry| entry.kind == "route" && entry.name == "GET /ping")
        .expect("route origin");
    let response = origin_map
        .entries
        .iter()
        .find(|entry| entry.kind == "domain" && entry.name == "respond")
        .expect("response origin");
    let trace_path = dir.join("production-trace.json");
    write_json(
        &trace_path,
        &serde_json::json!({
            "schema_version": 1,
            "kind": "orv.production.trace",
            "frames": [{
                "method": "GET",
                "path": "/ping",
                "status": 200,
                "route_origin_id": route.id,
                "response_origin_id": response.id,
            }],
        }),
    )
    .expect("write trace");

    let trace = editor_trace_json(&out, &trace_path).expect("editor trace");

    assert_eq!(trace["frames"][0]["origin_id"], route.id);
    assert_eq!(trace["frames"][0]["response_origin_id"], response.id);
    assert_eq!(
        trace["frames"][0]["summary"]["response_origin_id"],
        response.id
    );
    assert!(trace["frames"][0]["navigation"]["source"]["snippet"]
        .as_str()
        .is_some_and(|snippet| snippet.contains("@route GET /ping")));
    assert!(
        trace["frames"][0]["response_navigation"]["source"]["snippet"]
            .as_str()
            .is_some_and(|snippet| snippet.contains("@respond 200"))
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn editor_trace_summarizes_request_statuses_for_panels() {
    let dir = temp_output_dir("editor-trace-status-summary");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let trace_path = dir.join("production-trace.json");
    write_json(
        &trace_path,
        &serde_json::json!({
            "schema_version": 1,
            "kind": "orv.production.trace",
            "frames": [
                { "method": "GET", "path": "/ok", "status": 200 },
                { "method": "GET", "path": "/missing", "status": 404 },
                { "method": "POST", "path": "/checkout", "status": 503 }
            ],
        }),
    )
    .expect("write trace");

    let trace = editor_trace_json(&dir, &trace_path).expect("editor trace");

    assert_eq!(trace["trace"]["status_counts"]["total"], 3);
    assert_eq!(trace["trace"]["status_counts"]["ok"], 1);
    assert_eq!(trace["trace"]["status_counts"]["client_error"], 1);
    assert_eq!(trace["trace"]["status_counts"]["server_error"], 1);
    assert_eq!(trace["frames"][0]["summary"]["label"], "GET /ok -> 200");
    assert_eq!(
        trace["frames"][1]["summary"]["status_class"],
        "client_error"
    );
    assert_eq!(
        trace["frames"][2]["summary"]["status_class"],
        "server_error"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn editor_snapshot_outputs_graph_backed_panels() {
    let dir = temp_output_dir("editor-snapshot");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        r#"struct User { id: int }
define Auth() -> { @out "auth" }
@server {
  @listen 8080
  @route GET /users/:id { @respond 200 { ok: true } }
}
"#,
    )
    .expect("write source");

    let snapshot = editor_snapshot_json(&path).expect("editor snapshot");

    assert_eq!(snapshot["schema_version"], 1);
    assert!(snapshot["panels"]["files"]
        .as_array()
        .expect("files")
        .iter()
        .any(|file| file["path"]
            .as_str()
            .is_some_and(|path| path.ends_with("app.orv"))));
    assert!(snapshot["panels"]["routes"]
        .as_array()
        .expect("routes")
        .iter()
        .any(|route| route["method"] == "GET" && route["path"] == "/users/:id"));
    assert!(snapshot["panels"]["schema"]
        .as_array()
        .expect("schema")
        .iter()
        .any(|item| item["kind"] == "struct" && item["name"] == "User"));
    assert!(snapshot["panels"]["domains"]
        .as_array()
        .expect("domains")
        .iter()
        .any(|item| item["kind"] == "define" && item["name"] == "Auth"));
    assert_eq!(snapshot["live_refresh"]["strategy"], "source-hash");
    assert!(snapshot["live_refresh"]["watch"]["sources"]
        .as_array()
        .expect("watch sources")
        .iter()
        .any(|source| source["path"]
            .as_str()
            .is_some_and(|path| path.ends_with("app.orv"))
            && source["content_hash"]
                .as_str()
                .is_some_and(|hash| hash.starts_with("fnv1a64:"))));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn editor_trace_exposes_live_trace_event_stream_transport() {
    let (src_dir, path) = prod_server_source("editor-trace-live-transport-source");
    let out = temp_output_dir("editor-trace-live-transport");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let trace_path = src_dir.join("production-trace.json");
    write_json(
        &trace_path,
        &serde_json::json!({
            "schema_version": 1,
            "kind": "orv.production.trace",
            "frames": [],
        }),
    )
    .expect("write trace");

    let trace = editor_trace_json(&out, &trace_path).expect("editor trace");

    assert_eq!(trace["live_refresh"]["transport"]["kind"], "event-source");
    assert_eq!(trace["live_refresh"]["transport"]["event"], "orv:trace");
    assert_eq!(
        trace["live_refresh"]["transport"]["url"],
        "http://127.0.0.1:8080/__orv/trace/events"
    );
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(out);
}

#[test]
fn editor_trace_stream_consumes_eventsource_trace_snapshot() {
    let (src_dir, path) = prod_server_source("editor-trace-stream-source");
    let out = temp_output_dir("editor-trace-stream");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let origin_map: orv_compiler::OriginMap = serde_json::from_str(
        &std::fs::read_to_string(out.join("origin-map.json")).expect("origin map"),
    )
    .expect("origin map json");
    let route = origin_map
        .entries
        .iter()
        .find(|entry| entry.kind == "route" && entry.name == "GET /ping")
        .expect("route origin");
    let payload = serde_json::json!({
        "schema_version": 1,
        "kind": "orv.production.trace",
        "frame_count": 1,
        "frames": [{
            "method": "GET",
            "path": "/ping",
            "status": 200,
            "route_origin_id": route.id,
        }],
    });
    let events_path = src_dir.join("trace-events.sse");
    std::fs::write(
        &events_path,
        format!(
            "event: message\ndata: {{\"kind\":\"heartbeat\"}}\n\nevent: orv:trace\ndata: {}\n\n",
            serde_json::to_string(&payload).expect("payload json")
        ),
    )
    .expect("write trace events");

    let stream = editor_trace_stream_json(&out, &events_path).expect("editor trace stream");

    assert_eq!(stream["kind"], "orv.editor.trace.stream");
    assert_eq!(stream["event_stream"]["content_type"], "text/event-stream");
    assert_eq!(stream["event_stream"]["event_count"], 2);
    assert_eq!(stream["event_stream"]["trace_event_count"], 1);
    assert_eq!(stream["events"][0]["event"], "orv:trace");
    assert_eq!(stream["latest"]["kind"], "orv.editor.trace");
    assert_eq!(
        stream["latest"]["live_refresh"]["strategy"],
        "event-source-snapshot"
    );
    assert_eq!(
        stream["latest"]["live_refresh"]["transport"]["url"],
        "http://127.0.0.1:8080/__orv/trace/events"
    );
    assert_eq!(stream["latest"]["frames"][0]["origin_id"], route.id);
    assert_eq!(
        stream["latest"]["frames"][0]["navigation"]["focus"]["panel"],
        "routes"
    );
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(out);
}

#[test]
fn editor_trace_stream_consumes_trace_frame_events() {
    let (src_dir, path) = prod_server_source("editor-trace-frame-stream-source");
    let out = temp_output_dir("editor-trace-frame-stream");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let origin_map: orv_compiler::OriginMap = serde_json::from_str(
        &std::fs::read_to_string(out.join("origin-map.json")).expect("origin map"),
    )
    .expect("origin map json");
    let route = origin_map
        .entries
        .iter()
        .find(|entry| entry.kind == "route" && entry.name == "GET /ping")
        .expect("route origin");
    let frame = serde_json::json!({
        "method": "GET",
        "path": "/ping",
        "status": 200,
        "route_origin_id": route.id,
    });
    let events_path = src_dir.join("trace-frame-events.sse");
    std::fs::write(
        &events_path,
        format!(
            "event: orv:trace.frame\ndata: {}\n\nevent: orv:trace.frame\ndata: {}\n\n",
            serde_json::to_string(&serde_json::json!({
                "schema_version": 1,
                "kind": "orv.production.trace.frame",
                "index": 0,
                "frame": frame,
            }))
            .expect("frame event"),
            serde_json::to_string(&serde_json::json!({
                "schema_version": 1,
                "kind": "orv.production.trace.frame",
                "index": 1,
                "frame": frame,
            }))
            .expect("frame event"),
        ),
    )
    .expect("write trace frame events");

    let stream = editor_trace_stream_json(&out, &events_path).expect("editor trace stream");

    assert_eq!(stream["event_stream"]["trace_event_count"], 0);
    assert_eq!(stream["event_stream"]["trace_frame_event_count"], 2);
    assert_eq!(stream["events"][0]["event"], "orv:trace.frame");
    assert_eq!(stream["latest"]["kind"], "orv.editor.trace");
    assert_eq!(stream["latest"]["trace"]["frame_count"], 2);
    assert_eq!(stream["latest"]["frames"][0]["origin_id"], route.id);
    assert_eq!(
        stream["latest"]["frames"][0]["navigation"]["focus"]["panel"],
        "routes"
    );
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(out);
}

#[test]
fn editor_runtime_outputs_reference_runtime_inspection_panel() {
    let dir = temp_output_dir("editor-runtime");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("app.orv");
    std::fs::write(&path, "@out \"editor-runtime-ready\"\n").expect("write source");

    let runtime = editor_runtime_json(&path).expect("editor runtime");

    assert_eq!(runtime["schema_version"], 1);
    assert_eq!(runtime["runtime"]["status"], "ok");
    assert_eq!(runtime["runtime"]["stdout"], "editor-runtime-ready\n");
    assert_eq!(runtime["panels"]["runtime"]["status"], "ok");
    assert_eq!(
        runtime["panels"]["runtime"]["stdout"],
        "editor-runtime-ready\n"
    );
    assert!(!runtime["frames"].as_array().expect("frames").is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn editor_export_writes_static_editor_shell_and_state() {
    let dir = temp_output_dir("editor-export");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        "struct User { id: int }\n@out \"editor-export-ready\"\n",
    )
    .expect("write source");
    let out = dir.join("editor");

    cmd_editor_export(&path, &out).expect("editor export");

    let html = std::fs::read_to_string(out.join("index.html")).expect("editor html");
    let runtime_panel =
        std::fs::read_to_string(out.join(EDITOR_RUNTIME_PANEL_HTML_PATH)).expect("runtime panel");
    let state = read_json_value(&out.join("state.json")).expect("editor state");
    let native_host =
        read_json_value(&out.join(EDITOR_NATIVE_HOST_MANIFEST_PATH)).expect("native host");
    assert!(html.contains("id=\"orv-editor\""));
    assert!(html.contains("id=\"routes-list\""));
    assert!(html.contains("renderEditorState"));
    assert!(html.contains("Routes"));
    assert!(html.contains("Runtime"));
    assert!(html.contains("Project Graph"));
    assert!(html.contains("id=\"editor-graph-view\""));
    assert_eq!(state["schema_version"], 1);
    assert_eq!(state["snapshot"]["schema_version"], 1);
    assert_eq!(state["snapshot"]["project_graph"]["schema_version"], 1);
    assert_eq!(state["runtime"]["runtime"]["status"], "ok");
    assert_eq!(
        state["runtime"]["runtime"]["stdout"],
        "editor-export-ready\n"
    );
    assert_eq!(native_host["runtime"]["status"], "ok");
    assert!(native_host["runtime"]["frame_count"]
        .as_u64()
        .is_some_and(|count| count > 0));
    assert!(runtime_panel.contains("Runtime Panel"));
    assert!(runtime_panel.contains("editor-export-ready"));
    assert_eq!(
        native_host["artifacts"]["runtime_panel_html"],
        EDITOR_RUNTIME_PANEL_HTML_PATH
    );
    assert_eq!(
        native_host["runtime"]["panel_html_path"],
        EDITOR_RUNTIME_PANEL_HTML_PATH
    );
    assert_eq!(
        native_host["runtime"]["panel_artifact"]["path"],
        EDITOR_RUNTIME_PANEL_HTML_PATH
    );
    assert_eq!(
        native_host["runtime"]["panel_artifact"]["kind"],
        "orv.editor.runtime.panel"
    );
    let panels = native_host["panels"]
        .as_array()
        .expect("native host panel inventory");
    assert!(panels.iter().any(|panel| {
        panel["name"] == "debug_result"
            && panel["artifact"]["path"] == EDITOR_DEBUG_SESSION_RESULT_PATH
    }));
    assert!(panels.iter().any(|panel| {
        panel["name"] == "runtime" && panel["artifact"]["path"] == EDITOR_RUNTIME_PANEL_HTML_PATH
    }));
    assert!(!panels.iter().any(|panel| panel["name"] == "production"));
    assert!(!panels.iter().any(|panel| panel["name"] == "trace"));
    assert_eq!(native_host["runtime"]["panel_contract"]["root"], "runtime");
    let runtime_sections = native_host["runtime"]["panel_contract"]["sections"]
        .as_array()
        .expect("runtime panel sections");
    assert!(runtime_sections
        .iter()
        .any(|section| section["name"] == "panel" && section["path"] == "runtime.panel"));
    assert!(runtime_sections
        .iter()
        .any(|section| section["name"] == "frames" && section["path"] == "runtime.frames"));
    assert!(runtime_sections
        .iter()
        .any(|section| section["name"] == "panel_artifact"
            && section["path"] == "runtime.panel_artifact"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn editor_export_renders_runtime_frame_inspector() {
    let dir = temp_output_dir("editor-export-runtime-frames");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        "let total: int = 41\nlet next: int = total + 1\n@out next\n",
    )
    .expect("write source");
    let out = dir.join("editor");

    cmd_editor_export(&path, &out).expect("editor export");

    let html = std::fs::read_to_string(out.join("index.html")).expect("editor html");
    let state = read_json_value(&out.join("state.json")).expect("editor state");
    assert!(html.contains("id=\"runtime-frame-list\""));
    assert!(html.contains("id=\"runtime-frame-detail\""));
    assert!(html.contains("renderRuntimeDetail"));
    assert!(html.contains("Runtime Frames"));
    let frames = state["runtime"]["frames"]
        .as_array()
        .expect("runtime frames");
    assert!(!frames.is_empty());
    assert!(frames.iter().any(|frame| {
        frame["locals"]
            .as_array()
            .is_some_and(|locals| locals.iter().any(|local| local["name"] == "next"))
    }));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn editor_export_embeds_dap_debug_wiring() {
    let dir = temp_output_dir("editor-export-debug");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("app.orv");
    std::fs::write(
            &path,
            "function helper(value: int): int -> {\n  value + 1\n}\nlet total: int = 41\nlet next: int = total + 1\n@out next\n",
        )
        .expect("write source");
    let out = dir.join("editor");

    cmd_editor_export(&path, &out).expect("editor export");

    let html = std::fs::read_to_string(out.join("index.html")).expect("editor html");
    let state = read_json_value(&out.join("state.json")).expect("editor state");
    assert_eq!(state["debug"]["schema_version"], 1);
    assert_eq!(state["debug"]["adapter"]["protocol"], "dap");
    assert_eq!(
        state["debug"]["adapter"]["command"],
        serde_json::json!(["orv", "dap", "serve", "--stdio"])
    );
    assert_eq!(
        state["debug"]["capabilities"]["supportsStepBack"],
        serde_json::json!(true)
    );
    assert_eq!(
        state["debug"]["capabilities"]["supportsLoadedSourcesRequest"],
        serde_json::json!(true)
    );
    assert_eq!(
        state["debug"]["capabilities"]["supportsStepInTargetsRequest"],
        serde_json::json!(true)
    );
    assert_eq!(
        state["debug"]["capabilities"]["supportsRestartFrame"],
        serde_json::json!(true)
    );
    assert_eq!(
        state["debug"]["session_runner"]["kind"],
        "orv.editor.debug.runner"
    );
    assert_eq!(
        state["debug"]["session_runner"]["command"],
        serde_json::json!([
            "orv",
            "editor",
            "run-debug",
            "debug/session-runner.json",
            "--control",
            "next"
        ])
    );
    assert_eq!(
        state["debug"]["session_runner"]["session"]["reuse_session"],
        true
    );
    assert_eq!(
        state["debug"]["session_runner"]["session"]["breakpoint_argument"],
        "--breakpoint"
    );
    assert_eq!(
        state["debug"]["session_runner"]["session"]["function_breakpoint_argument"],
        "--function-breakpoint"
    );
    assert_eq!(
        state["debug"]["session_runner"]["session"]["data_breakpoint_argument"],
        "--data-breakpoint"
    );
    assert_eq!(
        state["debug"]["session_runner"]["session"]["exception_filter_argument"],
        "--exception-filter"
    );
    assert_eq!(
        state["debug"]["session_runner"]["session"]["watch_expression_argument"],
        "--watch-expression"
    );
    assert!(state["debug"]["function_breakpoints"]
        .as_array()
        .expect("function breakpoints")
        .iter()
        .any(|breakpoint| {
            breakpoint["name"] == "helper"
                && breakpoint["request"]["command"] == "setFunctionBreakpoints"
                && breakpoint["runner_command"]
                    .as_array()
                    .is_some_and(|command| {
                        command.iter().any(|part| part == "--function-breakpoint")
                            && command.iter().any(|part| part == "helper")
                    })
        }));
    assert!(state["debug"]["data_breakpoints"]
        .as_array()
        .expect("data breakpoints")
        .iter()
        .any(|breakpoint| {
            breakpoint["name"] == "total"
                && breakpoint["info_request"]["command"] == "dataBreakpointInfo"
                && breakpoint["request"]["command"] == "setDataBreakpoints"
                && breakpoint["runner_command"]
                    .as_array()
                    .is_some_and(|command| {
                        command.iter().any(|part| part == "--data-breakpoint")
                            && command.iter().any(|part| part == "total")
                    })
        }));
    assert!(state["debug"]["exception_filters"]
        .as_array()
        .expect("exception filters")
        .iter()
        .any(|filter| {
            filter["filter"] == "orv.runtime"
                && filter["request"]["command"] == "setExceptionBreakpoints"
                && filter["runner_command"].as_array().is_some_and(|command| {
                    command.iter().any(|part| part == "--exception-filter")
                        && command.iter().any(|part| part == "orv.runtime")
                })
        }));
    assert_eq!(
        state["debug"]["session_runner"]["result"]["path"],
        EDITOR_DEBUG_SESSION_RESULT_PATH
    );
    assert_eq!(
        state["debug"]["result_artifact"]["path"],
        EDITOR_DEBUG_SESSION_RESULT_PATH
    );
    assert_eq!(
        state["debug"]["result_artifact"]["kind"],
        "orv.editor.debug.runner.result"
    );
    assert_eq!(
        state["debug"]["result_artifact"]["panel_contract"]["root"],
        "panels.debug"
    );
    assert_eq!(
        state["debug"]["source_inventory"]["kind"],
        "orv.editor.debug.source_inventory"
    );
    assert_eq!(state["debug"]["source_inventory"]["source_count"], 1);
    assert_eq!(
        state["debug"]["source_inventory"]["loaded_sources_request"]["command"],
        "loadedSources"
    );
    assert!(state["debug"]["source_inventory"]["sources"]
        .as_array()
        .expect("source inventory")
        .iter()
        .any(|source| {
            source["source"]["path"]
                .as_str()
                .is_some_and(|path| path.ends_with("app.orv"))
                && source["source"]["sourceReference"] == 1
                && source["request"]["command"] == "source"
                && source["request"]["arguments"]["sourceReference"] == 1
                && source["checksum"]["algorithm"] == "SHA256"
        }));
    assert!(
        state["debug"]["result_artifact"]["panel_contract"]["sections"]
            .as_array()
            .expect("result panel sections")
            .iter()
            .any(|section| {
                section["name"] == "session_summary"
                    && section["path"] == "panels.debug.session_summary"
            })
    );
    assert_editor_debug_runner_artifact(&out, &state);
    assert_editor_native_host_manifest(&out, &state);
    assert_editor_debug_configurations(&state);
    assert_editor_debug_breakpoint_sources(&state);
    assert_editor_debug_controls(&state);
    assert_editor_debug_html(&html);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn editor_export_debug_source_inventory_tracks_imports() {
    let dir = temp_output_dir("editor-export-debug-sources");
    let models = dir.join("models");
    std::fs::create_dir_all(&models).expect("create models dir");
    let path = dir.join("app.orv");
    let imported = models.join("user.orv");
    let imported_source = "pub struct User { id: int }\n";
    std::fs::write(
        &path,
        "import models.user.User\nlet user: User = { id: 1 }\n@out \"ok\"\n",
    )
    .expect("write source");
    std::fs::write(&imported, imported_source).expect("write imported source");
    let out = dir.join("editor");

    cmd_editor_export(&path, &out).expect("editor export");
    let state = read_json_value(&out.join("state.json")).expect("editor state");
    let native_host =
        read_json_value(&out.join(EDITOR_NATIVE_HOST_MANIFEST_PATH)).expect("native host");
    let run = editor_debug_runner_session_json(
        &out.join(EDITOR_DEBUG_SESSION_RUNNER_PATH),
        &[EditorDebugControl::Next],
        &[],
        &[],
        &[],
        &[],
        &[],
    )
    .expect("run debug source inventory");

    assert_eq!(state["debug"]["source_inventory"]["source_count"], 2);
    assert_eq!(
        native_host["debug"]["source_inventory"],
        state["debug"]["source_inventory"]
    );
    assert!(state["debug"]["source_inventory"]["sources"]
        .as_array()
        .expect("source inventory")
        .iter()
        .any(|source| {
            source["source"]["name"] == "user.orv"
                && source["checksum"]["value"]
                    == serde_json::json!(sha256_hex(imported_source.as_bytes()))
                && source["request"]["command"] == "source"
        }));
    assert!(run["debug"]["loaded_sources"]["sources"]
        .as_array()
        .expect("loaded sources")
        .iter()
        .any(|source| {
            source["name"] == "user.orv"
                && source["checksums"][0]["checksum"]
                    == serde_json::json!(sha256_hex(imported_source.as_bytes()))
        }));
    assert!(run["debug"]["source_snapshots"]
        .as_array()
        .expect("source snapshots")
        .iter()
        .any(|snapshot| {
            snapshot["source"]["name"] == "user.orv"
                && snapshot["response"]["success"] == true
                && snapshot["response"]["body"]["content"] == imported_source
        }));
    assert_eq!(run["panels"]["debug"]["loaded_source_count"], 2);
    assert_eq!(run["panels"]["debug"]["source_snapshot_count"], 2);
    assert!(run["panels"]["debug"]["source_snapshots"]
        .as_array()
        .expect("panel source snapshots")
        .iter()
        .any(|snapshot| snapshot["source"]["name"] == "user.orv"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn editor_export_with_build_carries_production_context_into_debug_runner() {
    let dir = temp_output_dir("editor-export-debug-production-context");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("app.orv");
    std::fs::write(&path, "let total: int = 41\n@out total\n").expect("write source");
    let build_out = dir.join("dist");
    let editor_out = dir.join("editor");

    cmd_build_with_profile(&path, &build_out, BuildProfile::Production).expect("prod build");
    cmd_editor_export_with_options(&path, &editor_out, Some(&build_out), None)
        .expect("editor export with build");

    let state = read_json_value(&editor_out.join("state.json")).expect("editor state");
    let runner =
        read_json_value(&editor_out.join(EDITOR_DEBUG_SESSION_RUNNER_PATH)).expect("runner");
    let native_host =
        read_json_value(&editor_out.join(EDITOR_NATIVE_HOST_MANIFEST_PATH)).expect("native host");
    let production_context = &state["debug"]["production_context"];

    assert_eq!(
        production_context["kind"],
        "orv.editor.debug.production_context"
    );
    assert_eq!(
        production_context["build_dir"],
        build_out.display().to_string()
    );
    assert_eq!(
        production_context["summary"]["graph_contract_count"],
        serde_json::json!(3)
    );
    assert_eq!(
        production_context["summary"]["source_bundle_file_count"],
        serde_json::json!(1)
    );
    assert_eq!(
        production_context["source_bundle"],
        build_out.join(SOURCE_BUNDLE_PATH).display().to_string()
    );
    assert!(production_context["graph_contract"]
        .as_array()
        .expect("graph contract")
        .iter()
        .any(|target| target["path"] == SOURCE_BUNDLE_PATH));
    assert_eq!(runner["production_context"], *production_context);
    assert_eq!(
        runner["source_bundle"],
        build_out.join(SOURCE_BUNDLE_PATH).display().to_string()
    );
    assert_eq!(
        native_host["debug"]["production_context"],
        *production_context
    );
    assert_eq!(native_host["capabilities"]["dap_production_context"], true);
    assert!(native_host["debug"]["panel_contract"]["sections"]
        .as_array()
        .expect("native host debug sections")
        .iter()
        .any(|section| section["name"] == "production_context"
            && section["path"] == "debug.production_context"));

    let run = editor_debug_runner_session_json(
        &editor_out.join(EDITOR_DEBUG_SESSION_RUNNER_PATH),
        &[EditorDebugControl::Next],
        &[],
        &[],
        &[],
        &[],
        &[],
    )
    .expect("run debug runner with production context");
    assert_eq!(run["production_context"], *production_context);
    assert_eq!(
        run["panels"]["debug"]["production_context"],
        *production_context
    );
    assert_eq!(
        run["panels"]["debug"]["production_summary"],
        production_context["summary"]
    );
    assert!(
        run["panels"]["debug"]["result_artifact"]["panel_contract"]["sections"]
            .as_array()
            .expect("debug result panel sections")
            .iter()
            .any(|section| section["name"] == "production_context"
                && section["path"] == "panels.debug.production_context")
    );
    assert!(
        run["panels"]["debug"]["result_artifact"]["panel_contract"]["sections"]
            .as_array()
            .expect("debug result panel sections")
            .iter()
            .any(|section| section["name"] == "production_summary"
                && section["path"] == "panels.debug.production_summary")
    );
    let result_html = editor_debug_runner_result_html(&run).expect("debug result html");
    assert!(result_html.contains("Production Summary"));
    assert!(result_html.contains("Production Context"));
    assert!(result_html.contains("source-bundle.json"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn editor_run_debug_result_summarizes_native_production_targets() {
    let dir = temp_output_dir("editor-run-debug-production-summary");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("server.orv");
    std::fs::write(
        &path,
        r"@server {
  @listen 8080
  @route GET /ping {
    @respond 200 { ok: true }
  }
}",
    )
    .expect("write source");
    let build_out = dir.join("dist");
    let editor_out = dir.join("editor");

    cmd_build_with_profile(&path, &build_out, BuildProfile::Production).expect("prod build");
    cmd_editor_export_with_options(&path, &editor_out, Some(&build_out), None)
        .expect("editor export with build");

    let run = editor_debug_runner_session_json(
        &editor_out.join(EDITOR_DEBUG_SESSION_RUNNER_PATH),
        &[EditorDebugControl::Next],
        &[],
        &[],
        &[],
        &[],
        &[],
    )
    .expect("run debug runner with production summary");
    assert_eq!(
        run["panels"]["debug"]["production_summary"]["native_server_target_count"],
        1
    );
    assert_eq!(
        run["panels"]["debug"]["production_summary"]["native_server_route_count"],
        1
    );
    assert_eq!(
        run["panels"]["debug"]["production_summary"]["preflight_target_count"],
        1
    );
    assert_eq!(
        run["panels"]["debug"]["production_context"]["preflight"][0]["benchmark_evidence"]
            ["smoke_test_required_markers"],
        serde_json::json!(deploy_benchmark::SMOKE_REQUIRED_MARKERS)
    );
    assert_eq!(
        run["panels"]["debug"]["production_context"]["preflight"][0]["benchmark_evidence"]
            ["smoke_test_summary"]["required_markers"],
        serde_json::json!(deploy_benchmark::SMOKE_REQUIRED_MARKERS)
    );
    let result_html = editor_debug_runner_result_html(&run).expect("debug result html");
    assert!(result_html.contains("Production Summary"));
    assert!(result_html.contains("native_server_target_count"));
    assert!(result_html.contains("native plans, 1 routes"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn editor_run_debug_result_summarizes_client_production_targets() {
    let dir = temp_output_dir("editor-run-debug-client-production-summary");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("page.orv");
    std::fs::write(
        &path,
        "let sig count: int = 0\n@out @html { @body { @p count } }",
    )
    .expect("write source");
    let build_out = dir.join("dist");
    let editor_out = dir.join("editor");

    cmd_build_with_profile(&path, &build_out, BuildProfile::Production).expect("prod build");
    cmd_editor_export_with_options(&path, &editor_out, Some(&build_out), None)
        .expect("editor export with build");

    let run = editor_debug_runner_session_json(
        &editor_out.join(EDITOR_DEBUG_SESSION_RUNNER_PATH),
        &[EditorDebugControl::Next],
        &[],
        &[],
        &[],
        &[],
        &[],
    )
    .expect("run debug runner with client production summary");
    assert_eq!(
        run["panels"]["debug"]["production_summary"]["client_target_count"],
        5
    );
    assert_eq!(
        run["panels"]["debug"]["production_summary"]["client_manifest_count"],
        1
    );
    assert!(
        run["panels"]["debug"]["production_summary"]["client_capability_surface_count"]
            .as_u64()
            .is_some_and(|count| count >= 2),
        "{run}"
    );
    let result_html = editor_debug_runner_result_html(&run).expect("debug result html");
    assert!(result_html.contains("Production Summary"));
    assert!(result_html.contains("client_target_count"));
    assert!(result_html.contains("client targets, 1 manifests"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn editor_run_debug_result_summarizes_static_production_targets() {
    let dir = temp_output_dir("editor-run-debug-static-production-summary");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("page.orv");
    std::fs::write(&path, r#"@out @html { @body { @h1 "Home" } }"#).expect("write source");
    let build_out = dir.join("dist");
    let editor_out = dir.join("editor");

    cmd_build_with_profile(&path, &build_out, BuildProfile::Production).expect("prod build");
    cmd_editor_export_with_options(&path, &editor_out, Some(&build_out), None)
        .expect("editor export with build");

    let run = editor_debug_runner_session_json(
        &editor_out.join(EDITOR_DEBUG_SESSION_RUNNER_PATH),
        &[EditorDebugControl::Next],
        &[],
        &[],
        &[],
        &[],
        &[],
    )
    .expect("run debug runner with static production summary");
    assert_eq!(
        run["panels"]["debug"]["production_summary"]["static_target_count"],
        1
    );
    assert_eq!(
        run["panels"]["debug"]["production_summary"]["static_verified_count"],
        1
    );
    let result_html = editor_debug_runner_result_html(&run).expect("debug result html");
    assert!(result_html.contains("Production Summary"));
    assert!(result_html.contains("static_target_count"));
    assert!(result_html.contains("1/1"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn editor_run_debug_build_dir_rehydrates_source_bundle_when_original_source_is_missing() {
    let dir = temp_output_dir("editor-run-debug-build-dir-source-bundle");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("page.orv");
    std::fs::write(&path, r#"@out @html { @body { @h1 "Home" } }"#).expect("write source");
    let build_out = dir.join("dist");

    cmd_build_with_profile(&path, &build_out, BuildProfile::Production).expect("prod build");
    let source_bundle_path = build_out.join(SOURCE_BUNDLE_PATH);
    let source_bundle_value = read_json_value(&source_bundle_path).expect("source bundle");
    let expected_source_bundle_hash =
        stable_json_hash(&source_bundle_value).expect("source bundle hash");
    std::fs::remove_file(&path).expect("remove original source");

    let run = editor_debug_runner_session_json(
        &build_out,
        &[EditorDebugControl::Next],
        &[],
        &[],
        &[],
        &[],
        &[],
    )
    .expect("run debug runner from build dir source bundle");
    assert_eq!(run["runner"]["kind"], "orv.editor.debug.runner");
    assert_eq!(
        run["runner"]["source_bundle"],
        source_bundle_path.display().to_string()
    );
    assert_eq!(
        run["debug"]["launch"]["body"]["sourceBundle"]["path"],
        source_bundle_path.display().to_string()
    );
    assert_eq!(
        run["debug"]["launch"]["body"]["sourceBundle"]["entry"],
        source_bundle_value["entry"]
    );
    assert_eq!(
        run["debug"]["launch"]["body"]["sourceBundle"]["fileCount"],
        1
    );
    assert_eq!(
        run["debug"]["launch"]["body"]["sourceBundle"]["hash"],
        expected_source_bundle_hash
    );
    assert_eq!(
        run["panels"]["debug"]["source_bundle"],
        run["debug"]["launch"]["body"]["sourceBundle"]
    );
    assert_eq!(
        run["panels"]["debug"]["session_summary"]["source_bundle"],
        run["debug"]["launch"]["body"]["sourceBundle"]
    );
    assert_eq!(
        run["panels"]["debug"]["production_summary"]["static_target_count"],
        1
    );
    assert_eq!(
        run["panels"]["debug"]["production_summary"]["static_verified_count"],
        1
    );
    assert!(run["debug"]["source_snapshots"]
        .as_array()
        .expect("source snapshots")
        .iter()
        .any(|snapshot| snapshot["response"]["body"]["content"]
            .as_str()
            .is_some_and(|content| content.contains("@html"))));

    cmd_editor_run_debug(
        &build_out,
        &[EditorDebugControl::Next],
        &[],
        &[],
        &[],
        &[],
        &[],
    )
    .expect("write build-dir debug result");
    let result =
        read_json_value(&build_out.join(EDITOR_DEBUG_SESSION_RESULT_PATH)).expect("result");
    assert_eq!(
        result["panels"]["debug"]["production_summary"]["static_target_count"],
        1
    );
    assert_eq!(
        result["panels"]["debug"]["source_bundle"]["hash"],
        expected_source_bundle_hash
    );
    assert!(build_out
        .join(EDITOR_DEBUG_SESSION_RESULT_HTML_PATH)
        .is_file());
    let result_html =
        std::fs::read_to_string(build_out.join(EDITOR_DEBUG_SESSION_RESULT_HTML_PATH))
            .expect("debug result html");
    assert!(result_html.contains("source_bundle"));
    assert!(result_html.contains("source-bundle.json"));
    let _ = std::fs::remove_dir_all(dir);
}

fn assert_editor_debug_runner_artifact(out: &Path, state: &serde_json::Value) {
    let runner =
        read_json_value(&out.join(EDITOR_DEBUG_SESSION_RUNNER_PATH)).expect("debug runner");
    assert_eq!(runner, state["debug"]["session_runner"]);
    assert_eq!(runner["result"]["path"], EDITOR_DEBUG_SESSION_RESULT_PATH);
    let run = editor_debug_runner_session_json(
        &out.join(EDITOR_DEBUG_SESSION_RUNNER_PATH),
        &[EditorDebugControl::Next],
        &[],
        &[],
        &[],
        &[],
        &[],
    )
    .expect("run standalone debug runner");
    assert_eq!(run["kind"], "orv.editor.debug.runner.result");
    assert_eq!(run["runner"], runner);
}

fn assert_editor_native_host_manifest(out: &Path, state: &serde_json::Value) {
    let native_host =
        read_json_value(&out.join(EDITOR_NATIVE_HOST_MANIFEST_PATH)).expect("native host");
    assert_eq!(native_host["kind"], "orv.editor.native_host");
    assert_eq!(native_host["artifacts"]["shell"], "index.html");
    assert_eq!(native_host["artifacts"]["state"], "state.json");
    assert_eq!(
        native_host["artifacts"]["debug_session_runner"],
        EDITOR_DEBUG_SESSION_RUNNER_PATH
    );
    assert_eq!(
        native_host["artifacts"]["debug_session_result"],
        EDITOR_DEBUG_SESSION_RESULT_PATH
    );
    assert_eq!(
        native_host["artifacts"]["debug_session_result_html"],
        EDITOR_DEBUG_SESSION_RESULT_HTML_PATH
    );
    assert_eq!(
        native_host["debug"]["adapter_command"],
        serde_json::json!(["orv", "dap", "serve", "--stdio"])
    );
    assert_eq!(
        native_host["debug"]["capabilities"],
        state["debug"]["capabilities"]
    );
    assert_eq!(
        native_host["debug"]["source_inventory"],
        state["debug"]["source_inventory"]
    );
    assert_eq!(native_host["debug"]["source_count"], 1);
    assert_eq!(native_host["capabilities"]["dap_sources"], true);
    assert_eq!(
        native_host["debug"]["runner_command"],
        state["debug"]["session_runner"]["command"]
    );
    assert_eq!(native_host["debug"]["breakpoint_argument"], "--breakpoint");
    assert_eq!(native_host["debug"]["breakpoint_format"], "<path>:<line>");
    assert_eq!(
        native_host["debug"]["function_breakpoint_argument"],
        "--function-breakpoint"
    );
    assert_eq!(
        native_host["debug"]["function_breakpoint_format"],
        "<function-name>"
    );
    assert_eq!(
        native_host["debug"]["data_breakpoint_argument"],
        "--data-breakpoint"
    );
    assert_eq!(
        native_host["debug"]["data_breakpoint_format"],
        "<local-name>"
    );
    assert_eq!(
        native_host["debug"]["exception_filter_argument"],
        "--exception-filter"
    );
    assert_eq!(
        native_host["debug"]["exception_filter_format"],
        "<orv.diagnostics|orv.runtime>"
    );
    assert_eq!(
        native_host["debug"]["watch_expression_argument"],
        "--watch-expression"
    );
    assert_eq!(
        native_host["debug"]["watch_expression_format"],
        "<expression>"
    );
    assert_eq!(
        native_host["debug"]["result_path"],
        EDITOR_DEBUG_SESSION_RESULT_PATH
    );
    assert_eq!(
        native_host["debug"]["result_kind"],
        "orv.editor.debug.runner.result"
    );
    assert_eq!(
        native_host["debug"]["result_artifact"],
        state["debug"]["result_artifact"]
    );
    assert_eq!(native_host["debug"]["panel_contract"]["root"], "debug");
    let debug_sections = native_host["debug"]["panel_contract"]["sections"]
        .as_array()
        .expect("native host debug panel sections");
    assert!(debug_sections
        .iter()
        .any(|section| section["name"] == "configurations"
            && section["path"] == "debug.configurations"));
    assert!(debug_sections.iter().any(|section| {
        section["name"] == "source_inventory" && section["path"] == "debug.source_inventory"
    }));
    assert!(debug_sections.iter().any(|section| {
        section["name"] == "control_commands" && section["path"] == "debug.control_commands"
    }));
    assert!(debug_sections.iter().any(|section| {
        section["name"] == "breakpoint_commands" && section["path"] == "debug.breakpoint_commands"
    }));
    assert!(debug_sections.iter().any(|section| {
        section["name"] == "function_breakpoint_commands"
            && section["path"] == "debug.function_breakpoint_commands"
    }));
    assert!(debug_sections.iter().any(|section| {
        section["name"] == "data_breakpoint_commands"
            && section["path"] == "debug.data_breakpoint_commands"
    }));
    assert!(debug_sections.iter().any(|section| {
        section["name"] == "exception_filter_commands"
            && section["path"] == "debug.exception_filter_commands"
    }));
    assert!(debug_sections.iter().any(|section| {
        section["name"] == "function_breakpoint_argument"
            && section["path"] == "debug.function_breakpoint_argument"
    }));
    assert!(debug_sections.iter().any(|section| {
        section["name"] == "data_breakpoint_argument"
            && section["path"] == "debug.data_breakpoint_argument"
    }));
    assert!(debug_sections.iter().any(|section| {
        section["name"] == "exception_filter_argument"
            && section["path"] == "debug.exception_filter_argument"
    }));
    assert!(debug_sections.iter().any(|section| {
        section["name"] == "watch_expression_argument"
            && section["path"] == "debug.watch_expression_argument"
    }));
    assert!(debug_sections.iter().any(|section| {
        section["name"] == "result_artifact" && section["path"] == "debug.result_artifact"
    }));
    assert!(
        native_host["debug"]["result_artifact"]["panel_contract"]["sections"]
            .as_array()
            .expect("native host result panel sections")
            .iter()
            .any(|section| section["name"] == "events" && section["path"] == "panels.debug.events")
    );
    assert!(
        native_host["debug"]["result_artifact"]["panel_contract"]["sections"]
            .as_array()
            .expect("native host result panel sections")
            .iter()
            .any(|section| section["name"] == "function_breakpoints"
                && section["path"] == "panels.debug.function_breakpoints")
    );
    assert!(
        native_host["debug"]["result_artifact"]["panel_contract"]["sections"]
            .as_array()
            .expect("native host result panel sections")
            .iter()
            .any(|section| section["name"] == "data_breakpoints"
                && section["path"] == "panels.debug.data_breakpoints")
    );
    assert!(
        native_host["debug"]["result_artifact"]["panel_contract"]["sections"]
            .as_array()
            .expect("native host result panel sections")
            .iter()
            .any(|section| section["name"] == "exception_filters"
                && section["path"] == "panels.debug.exception_filters")
    );
    assert!(
        native_host["debug"]["result_artifact"]["panel_contract"]["sections"]
            .as_array()
            .expect("native host result panel sections")
            .iter()
            .any(|section| section["name"] == "watch_expressions"
                && section["path"] == "panels.debug.watch_expressions")
    );
    assert!(
        native_host["debug"]["result_artifact"]["panel_contract"]["sections"]
            .as_array()
            .expect("native host result panel sections")
            .iter()
            .any(|section| section["name"] == "source_snapshots"
                && section["path"] == "panels.debug.source_snapshots")
    );
    assert_eq!(native_host["debug"]["configuration_count"], 3);
    let configurations = native_host["debug"]["configurations"]
        .as_array()
        .expect("native host debug configurations");
    assert!(configurations
        .iter()
        .any(|config| config["name"] == "Live Launch ORV" && config["live"] == true));
    assert!(configurations.iter().any(|config| {
        config["name"] == "Attach ORV Runtime"
            && config["request"] == "attach"
            && config["attachRuntimeMode"] == "inProcess"
    }));
    assert!(native_host["debug"]["breakpoint_count"]
        .as_u64()
        .is_some_and(|count| count > 0));
    assert!(native_host["debug"]["function_breakpoint_count"]
        .as_u64()
        .is_some_and(|count| count > 0));
    assert!(native_host["debug"]["data_breakpoint_count"]
        .as_u64()
        .is_some_and(|count| count > 0));
    assert!(native_host["debug"]["exception_filter_count"]
        .as_u64()
        .is_some_and(|count| count > 0));
    let control_commands = native_host["debug"]["control_commands"]
        .as_array()
        .expect("native host control commands");
    assert!(control_commands.iter().any(|command| {
        command["name"] == "Next"
            && command["command"]
                == serde_json::json!([
                    "orv",
                    "editor",
                    "run-debug",
                    "debug/session-runner.json",
                    "--control",
                    "next"
                ])
    }));
    assert!(control_commands.iter().any(|command| {
        command["name"] == "Step Back"
            && command["request"]
                == serde_json::json!({"command": "stepBack", "arguments": {"threadId": 1}})
    }));
    assert!(control_commands.iter().any(|command| {
        command["name"] == "Reverse Continue"
            && command["request"]
                == serde_json::json!({"command": "reverseContinue", "arguments": {"threadId": 1}})
    }));
    assert!(control_commands.iter().any(|command| {
        command["name"] == "Restart Frame"
            && command["request"]
                == serde_json::json!({"command": "restartFrame", "arguments": {"frameId": 1}})
    }));
    assert!(control_commands.iter().any(|command| {
        command["name"] == "Terminate"
            && command["request"] == serde_json::json!({"command": "terminate", "arguments": {}})
            && command["command"]
                .as_array()
                .is_some_and(|command| command.iter().any(|part| part == "terminate"))
    }));
    assert!(control_commands.iter().any(|command| {
            command["name"] == "Terminate Threads"
                && command["request"]
                    == serde_json::json!({"command": "terminateThreads", "arguments": {"threadIds": [1]}})
                && command["command"].as_array().is_some_and(|command| {
                    command.iter().any(|part| part == "terminate-threads")
                })
        }));
    assert!(control_commands.iter().any(|command| {
        command["name"] == "Step In Targets"
            && command["request"]
                == serde_json::json!({"command": "stepInTargets", "arguments": {"frameId": 1}})
    }));
    let breakpoint_commands = native_host["debug"]["breakpoint_commands"]
        .as_array()
        .expect("native host breakpoint commands");
    assert!(breakpoint_commands.iter().any(|breakpoint| {
        breakpoint["line"] == 1
            && breakpoint["source"]["path"]
                .as_str()
                .is_some_and(|path| path.ends_with("app.orv"))
            && breakpoint["request"]["command"] == "setBreakpoints"
            && breakpoint["command"].as_array().is_some_and(|command| {
                command.iter().any(|part| part == "--breakpoint")
                    && command.iter().any(|part| part == "continue")
            })
    }));
    let function_breakpoint_commands = native_host["debug"]["function_breakpoint_commands"]
        .as_array()
        .expect("native host function breakpoint commands");
    assert!(function_breakpoint_commands.iter().any(|breakpoint| {
        breakpoint["name"] == "helper"
            && breakpoint["request"]["command"] == "setFunctionBreakpoints"
            && breakpoint["command"].as_array().is_some_and(|command| {
                command.iter().any(|part| part == "--function-breakpoint")
                    && command.iter().any(|part| part == "helper")
            })
    }));
    let data_breakpoint_commands = native_host["debug"]["data_breakpoint_commands"]
        .as_array()
        .expect("native host data breakpoint commands");
    assert!(data_breakpoint_commands.iter().any(|breakpoint| {
        breakpoint["name"] == "total"
            && breakpoint["info_request"]["command"] == "dataBreakpointInfo"
            && breakpoint["request"]["command"] == "setDataBreakpoints"
            && breakpoint["command"].as_array().is_some_and(|command| {
                command.iter().any(|part| part == "--data-breakpoint")
                    && command.iter().any(|part| part == "total")
            })
    }));
    let exception_filter_commands = native_host["debug"]["exception_filter_commands"]
        .as_array()
        .expect("native host exception filter commands");
    assert!(exception_filter_commands.iter().any(|filter| {
        filter["filter"] == "orv.runtime"
            && filter["request"]["command"] == "setExceptionBreakpoints"
            && filter["command"].as_array().is_some_and(|command| {
                command.iter().any(|part| part == "--exception-filter")
                    && command.iter().any(|part| part == "orv.runtime")
            })
    }));
}

fn assert_editor_debug_configurations(state: &serde_json::Value) {
    assert!(state["debug"]["configurations"]
        .as_array()
        .expect("debug configurations")
        .iter()
        .any(|config| config["name"] == "Live Launch ORV" && config["live"] == true));
}

fn assert_editor_debug_breakpoint_sources(state: &serde_json::Value) {
    let breakpoint_sources = state["debug"]["breakpoint_sources"]
        .as_array()
        .expect("breakpoint sources");
    assert!(breakpoint_sources.iter().any(|source| {
        source["source"]["path"]
            .as_str()
            .is_some_and(|path| path.ends_with("app.orv"))
            && source["lines"]
                .as_array()
                .is_some_and(|lines| lines.iter().any(|line| line == 1))
    }));
    assert!(breakpoint_sources.iter().any(|source| {
        source["source"]["path"]
            .as_str()
            .is_some_and(|path| path.ends_with("app.orv"))
            && source["breakpoints"].as_array().is_some_and(|breakpoints| {
                breakpoints.iter().any(|breakpoint| {
                    breakpoint["line"] == 1
                        && breakpoint["request"]["command"] == "setBreakpoints"
                        && breakpoint["runner_command"]
                            .as_array()
                            .is_some_and(|command| {
                                command.iter().any(|part| part == "--breakpoint")
                                    && command.iter().any(|part| part == "continue")
                            })
                })
            })
    }));
}

fn assert_editor_debug_controls(state: &serde_json::Value) {
    let controls = state["debug"]["controls"]
        .as_array()
        .expect("debug controls");
    assert_editor_debug_control(
        controls,
        "Continue",
        &serde_json::json!({"command": "continue", "arguments": {"threadId": 1}}),
    );
    assert_editor_debug_control(
        controls,
        "Pause",
        &serde_json::json!({"command": "pause", "arguments": {"threadId": 1}}),
    );
    assert_editor_debug_control(
        controls,
        "Reverse Continue",
        &serde_json::json!({"command": "reverseContinue", "arguments": {"threadId": 1}}),
    );
    assert_editor_debug_control(
        controls,
        "Next",
        &serde_json::json!({"command": "next", "arguments": {"threadId": 1}}),
    );
    assert_editor_debug_control_runner_command(controls, "Next", "next");
    assert_editor_debug_control(
        controls,
        "Step Back",
        &serde_json::json!({"command": "stepBack", "arguments": {"threadId": 1}}),
    );
    assert_editor_debug_control(
        controls,
        "Step In",
        &serde_json::json!({"command": "stepIn", "arguments": {"threadId": 1}}),
    );
    assert_editor_debug_control(
        controls,
        "Step In Targets",
        &serde_json::json!({"command": "stepInTargets", "arguments": {"frameId": 1}}),
    );
    assert_editor_debug_control(
        controls,
        "Step Out",
        &serde_json::json!({"command": "stepOut", "arguments": {"threadId": 1}}),
    );
    assert_editor_debug_control(
        controls,
        "Restart Frame",
        &serde_json::json!({"command": "restartFrame", "arguments": {"frameId": 1}}),
    );
    assert_editor_debug_control(
        controls,
        "Restart",
        &serde_json::json!({"command": "restart", "arguments": {}}),
    );
    assert_editor_debug_control(
        controls,
        "Terminate",
        &serde_json::json!({"command": "terminate", "arguments": {}}),
    );
    assert_editor_debug_control_runner_command(controls, "Terminate", "terminate");
    assert_editor_debug_control(
        controls,
        "Terminate Threads",
        &serde_json::json!({"command": "terminateThreads", "arguments": {"threadIds": [1]}}),
    );
    assert_editor_debug_control_runner_command(controls, "Terminate Threads", "terminate-threads");
    assert_editor_debug_control(
        controls,
        "Disconnect",
        &serde_json::json!({"command": "disconnect", "arguments": {"terminateDebuggee": true}}),
    );
}

fn assert_editor_debug_html(html: &str) {
    assert!(html.contains("id=\"debug-config-list\""));
    assert!(html.contains("id=\"debug-control-list\""));
    assert!(html.contains("Debug Controls"));
    assert!(html.contains("DAP Capabilities"));
    assert!(html.contains("id=\"debug-breakpoint-list\""));
    assert!(html.contains("id=\"debug-function-breakpoint-list\""));
    assert!(html.contains("id=\"debug-data-breakpoint-list\""));
    assert!(html.contains("id=\"debug-exception-filter-list\""));
    assert!(html.contains("id=\"debug-capability-list\""));
    assert!(html.contains("id=\"debug-runner-detail\""));
    assert!(html.contains("id=\"debug-result-detail\""));
    assert!(html.contains("Runner Command"));
    assert!(html.contains("renderDebugRunner"));
    assert!(html.contains("renderDebugCapabilities"));
    assert!(html.contains("renderDebugResultArtifact"));
    assert!(html.contains("renderDebugDetail"));
    assert!(html.contains("renderDebugControlCommand"));
    assert!(html.contains("renderFunctionBreakpoints"));
    assert!(html.contains("renderDataBreakpoints"));
    assert!(html.contains("renderExceptionFilters"));
}

fn assert_editor_debug_control(
    controls: &[serde_json::Value],
    name: &str,
    request: &serde_json::Value,
) {
    assert!(
        controls
            .iter()
            .any(|control| control["name"] == name && control["request"] == *request),
        "missing debug control {name}"
    );
}

fn assert_editor_debug_control_runner_command(
    controls: &[serde_json::Value],
    name: &str,
    value: &str,
) {
    assert!(
        controls.iter().any(|control| {
            control["name"] == name
                && control["runner_command"]
                    == serde_json::json!([
                        "orv",
                        "editor",
                        "run-debug",
                        "debug/session-runner.json",
                        "--control",
                        value
                    ])
        }),
        "missing debug control runner command {name}"
    );
}

#[test]
fn editor_debug_control_uses_dap_stdio_transport() {
    let dir = temp_output_dir("editor-debug-control");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("app.orv");
    std::fs::write(&path, "let first: int = 1\nlet second: int = 2\n").expect("write source");

    let debug =
        editor_debug_session_json(&path, &[EditorDebugControl::Next], &[], &[], &[], &[], &[])
            .expect("editor debug session");

    assert_eq!(debug["kind"], "orv.editor.debug");
    assert_eq!(debug["adapter"]["protocol"], "dap");
    assert_eq!(debug["transport"]["framing"], "content-length");
    assert_eq!(debug["control"]["request"]["command"], "next");
    assert_eq!(debug["control"]["response"]["success"], true);
    assert_eq!(debug["stack"]["stackFrames"][0]["line"], 2);
    assert!(debug["frames"]
        .as_array()
        .expect("frames")
        .iter()
        .any(|frame| {
            frame["type"] == "event"
                && frame["event"] == "stopped"
                && frame["body"]["reason"] == "step"
        }));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn editor_debug_control_sequence_reuses_one_dap_session() {
    let dir = temp_output_dir("editor-debug-control-sequence");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        "let first: int = 1\nlet second: int = 2\nlet third: int = 3\n",
    )
    .expect("write source");

    let debug = editor_debug_session_json(
        &path,
        &[EditorDebugControl::Next, EditorDebugControl::Next],
        &[],
        &[],
        &[],
        &[],
        &[],
    )
    .expect("editor debug session");

    let controls = debug["controls"].as_array().expect("controls");
    assert_eq!(controls.len(), 2);
    assert!(controls
        .iter()
        .all(|control| control["response"]["success"] == true));
    assert_eq!(debug["transport"]["request_count"], 10);
    assert_eq!(debug["stack"]["stackFrames"][0]["line"], 3);
    assert!(debug["locals"]
        .as_array()
        .expect("locals")
        .iter()
        .any(|local| local["name"] == "third" && local["value"] == "3"));
    let step_stops = debug["frames"]
        .as_array()
        .expect("frames")
        .iter()
        .filter(|frame| {
            frame["type"] == "event"
                && frame["event"] == "stopped"
                && frame["body"]["reason"] == "step"
        })
        .count();
    assert!(step_stops >= 2, "{debug}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn editor_debug_terminate_threads_control_uses_dap_session() {
    let dir = temp_output_dir("editor-debug-terminate-threads");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("app.orv");
    std::fs::write(&path, "let answer: int = 42\n").expect("write source");

    let debug = editor_debug_session_json(
        &path,
        &[EditorDebugControl::TerminateThreads],
        &[],
        &[],
        &[],
        &[],
        &[],
    )
    .expect("editor debug session");

    assert_eq!(debug["control"]["request"]["command"], "terminateThreads");
    assert_eq!(debug["control"]["response"]["success"], true);
    assert!(debug["stack"]
        .as_object()
        .is_some_and(serde_json::Map::is_empty));
    assert!(debug["frames"]
        .as_array()
        .expect("frames")
        .iter()
        .any(|frame| frame["type"] == "event" && frame["event"] == "terminated"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn editor_debug_terminate_control_uses_dap_session() {
    let dir = temp_output_dir("editor-debug-terminate");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("app.orv");
    std::fs::write(&path, "let answer: int = 42\n").expect("write source");

    let debug = editor_debug_session_json(
        &path,
        &[EditorDebugControl::Terminate],
        &[],
        &[],
        &[],
        &[],
        &[],
    )
    .expect("editor debug session");

    assert_eq!(debug["control"]["request"]["command"], "terminate");
    assert_eq!(debug["control"]["response"]["success"], true);
    assert!(debug["stack"]
        .as_object()
        .is_some_and(serde_json::Map::is_empty));
    assert!(debug["frames"]
        .as_array()
        .expect("frames")
        .iter()
        .any(|frame| frame["type"] == "event" && frame["event"] == "terminated"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn editor_debug_breakpoint_argument_stops_continue_at_line() {
    let dir = temp_output_dir("editor-debug-breakpoint");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        "let first: int = 1\nlet second: int = 2\nlet third: int = 3\n",
    )
    .expect("write source");
    let breakpoint = EditorDebugBreakpoint {
        path: path.clone(),
        line: 3,
    };

    let debug = editor_debug_session_json(
        &path,
        &[EditorDebugControl::Continue],
        &[breakpoint],
        &[],
        &[],
        &[],
        &[],
    )
    .expect("editor debug session");

    assert_eq!(debug["transport"]["request_count"], 10);
    assert_eq!(
        debug["breakpoints"][0]["source"]["path"],
        path.display().to_string()
    );
    assert_eq!(debug["breakpoints"][0]["lines"], serde_json::json!([3]));
    assert_eq!(debug["breakpoints"][0]["response"]["success"], true);
    assert!(debug["breakpoints"][0]["response"]["body"]["breakpoints"]
        .as_array()
        .expect("breakpoints")
        .iter()
        .any(|breakpoint| breakpoint["verified"] == true && breakpoint["line"] == 3));
    assert_eq!(debug["control"]["request"]["command"], "continue");
    assert_eq!(debug["control"]["response"]["success"], true);
    assert_eq!(debug["stack"]["stackFrames"][0]["line"], 3);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn editor_debug_function_breakpoint_argument_stops_inside_function() {
    let dir = temp_output_dir("editor-debug-function-breakpoint");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("app.orv");
    std::fs::write(
            &path,
            "function add(a: int, b: int): int -> {\n  let result: int = a + b\n  result\n}\nlet total: int = add(2, 3)\n",
        )
        .expect("write source");
    let function_breakpoints = vec!["add".to_string()];

    let debug = editor_debug_session_json(
        &path,
        &[EditorDebugControl::Continue],
        &[],
        &function_breakpoints,
        &[],
        &[],
        &[],
    )
    .expect("editor debug session");

    assert_eq!(
        debug["function_breakpoints"][0]["request"]["command"],
        "setFunctionBreakpoints"
    );
    assert_eq!(
        debug["function_breakpoints"][0]["names"],
        serde_json::json!(["add"])
    );
    assert_eq!(
        debug["function_breakpoints"][0]["response"]["body"]["breakpoints"][0]["verified"],
        true
    );
    assert_eq!(debug["stack"]["stackFrames"][0]["name"], "add");
    assert!(debug["frames"]
        .as_array()
        .expect("frames")
        .iter()
        .any(|frame| {
            frame["type"] == "event"
                && frame["event"] == "stopped"
                && frame["body"]["reason"] == "function breakpoint"
        }));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn editor_debug_data_breakpoint_argument_stops_when_local_changes() {
    let dir = temp_output_dir("editor-debug-data-breakpoint");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("app.orv");
    std::fs::write(&path, "let mut total: int = 1\ntotal = total + 4\n").expect("write source");
    let data_breakpoints = vec!["total".to_string()];

    let debug = editor_debug_session_json(
        &path,
        &[EditorDebugControl::Continue],
        &[],
        &[],
        &data_breakpoints,
        &[],
        &[],
    )
    .expect("editor debug session");

    assert_eq!(
        debug["data_breakpoints"][0]["infos"][0]["request"]["command"],
        "dataBreakpointInfo"
    );
    assert_eq!(
        debug["data_breakpoints"][0]["request"]["command"],
        "setDataBreakpoints"
    );
    assert_eq!(
        debug["data_breakpoints"][0]["names"],
        serde_json::json!(["total"])
    );
    assert_eq!(
        debug["data_breakpoints"][0]["response"]["body"]["breakpoints"][0]["verified"],
        true
    );
    assert_eq!(debug["stack"]["stackFrames"][0]["line"], 2);
    assert!(debug["frames"]
        .as_array()
        .expect("frames")
        .iter()
        .any(|frame| {
            frame["type"] == "event"
                && frame["event"] == "stopped"
                && frame["body"]["reason"] == "data breakpoint"
        }));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn editor_debug_exception_filter_argument_configures_dap_session() {
    let dir = temp_output_dir("editor-debug-exception-filter");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("app.orv");
    std::fs::write(&path, "let answer: int = 42\n").expect("write source");
    let exception_filters = vec!["orv.runtime".to_string()];

    let debug = editor_debug_session_json(
        &path,
        &[EditorDebugControl::Next],
        &[],
        &[],
        &[],
        &exception_filters,
        &[],
    )
    .expect("editor debug session");

    assert_eq!(
        debug["exception_filters"][0]["request"]["command"],
        "setExceptionBreakpoints"
    );
    assert_eq!(
        debug["exception_filters"][0]["filters"],
        serde_json::json!(["orv.runtime"])
    );
    assert_eq!(debug["exception_filters"][0]["response"]["success"], true);
    assert_eq!(debug["control"]["response"]["success"], true);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn editor_run_debug_executes_exported_session_runner() {
    let dir = temp_output_dir("editor-run-debug");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        "let first: int = 1\nlet second: int = 2\nlet third: int = 3\n",
    )
    .expect("write source");
    let out = dir.join("editor");
    cmd_editor_export(&path, &out).expect("editor export");

    let run = editor_debug_runner_session_json(
        &out.join("state.json"),
        &[EditorDebugControl::Next, EditorDebugControl::Next],
        &[],
        &[],
        &[],
        &[],
        &[],
    )
    .expect("run exported debug runner");

    assert_eq!(run["kind"], "orv.editor.debug.runner.result");
    assert_eq!(run["runner"]["kind"], "orv.editor.debug.runner");
    assert_eq!(run["debug"]["transport"]["framing"], "content-length");
    assert_eq!(run["debug"]["transport"]["request_count"], 10);
    assert_eq!(run["debug"]["stack"]["stackFrames"][0]["line"], 3);
    assert!(run["debug"]["locals"]
        .as_array()
        .expect("locals")
        .iter()
        .any(|local| local["name"] == "third" && local["value"] == "3"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn editor_run_debug_executes_exported_runner_with_breakpoint() {
    let dir = temp_output_dir("editor-run-debug-breakpoint");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        "let first: int = 1\nlet second: int = 2\nlet third: int = 3\n",
    )
    .expect("write source");
    let out = dir.join("editor");
    cmd_editor_export(&path, &out).expect("editor export");
    let breakpoint = EditorDebugBreakpoint { path, line: 3 };

    let run = editor_debug_runner_session_json(
        &out.join("debug").join("session-runner.json"),
        &[EditorDebugControl::Continue],
        &[breakpoint],
        &[],
        &[],
        &[],
        &[],
    )
    .expect("run exported debug runner");

    assert_eq!(run["kind"], "orv.editor.debug.runner.result");
    assert_eq!(run["debug"]["breakpoints"][0]["response"]["success"], true);
    assert_eq!(run["debug"]["stack"]["stackFrames"][0]["line"], 3);
    assert!(run["panels"]["debug"]["controls"]
        .as_array()
        .expect("panel controls")
        .iter()
        .any(|control| control["name"] == "Continue"));
    assert!(run["panels"]["debug"]["breakpoints"]
        .as_array()
        .expect("panel breakpoints")
        .iter()
        .any(|breakpoint| {
            breakpoint["source"]["path"]
                .as_str()
                .is_some_and(|source| source.ends_with("app.orv"))
                && breakpoint["lines"]
                    .as_array()
                    .is_some_and(|lines| lines.iter().any(|line| line == 3))
                && breakpoint["response"]["success"] == true
        }));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn editor_run_debug_executes_exported_runner_with_data_breakpoint() {
    let dir = temp_output_dir("editor-run-debug-data-breakpoint");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("app.orv");
    std::fs::write(&path, "let mut total: int = 1\ntotal = total + 4\n").expect("write source");
    let out = dir.join("editor");
    cmd_editor_export(&path, &out).expect("editor export");
    let data_breakpoints = vec!["total".to_string()];

    let run = editor_debug_runner_session_json(
        &out.join("debug").join("session-runner.json"),
        &[EditorDebugControl::Continue],
        &[],
        &[],
        &data_breakpoints,
        &[],
        &[],
    )
    .expect("run exported debug runner");

    assert_eq!(run["kind"], "orv.editor.debug.runner.result");
    assert_eq!(
        run["debug"]["data_breakpoints"][0]["response"]["success"],
        true
    );
    assert_eq!(
        run["debug"]["data_breakpoints"][0]["response"]["body"]["breakpoints"][0]["verified"],
        true
    );
    assert_eq!(run["debug"]["stack"]["stackFrames"][0]["line"], 2);
    assert_eq!(run["panels"]["debug"]["data_breakpoint_count"], 1);
    assert!(run["panels"]["debug"]["data_breakpoints"]
        .as_array()
        .expect("panel data breakpoints")
        .iter()
        .any(|breakpoint| {
            breakpoint["names"]
                .as_array()
                .is_some_and(|names| names.iter().any(|name| name == "total"))
                && breakpoint["response"]["success"] == true
        }));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn editor_run_debug_writes_exported_runner_result_artifact() {
    let dir = temp_output_dir("editor-run-debug-result-artifact");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        "let first: int = 1\nlet second: int = 2\nlet third: int = 3\n",
    )
    .expect("write source");
    let out = dir.join("editor");
    cmd_editor_export(&path, &out).expect("editor export");
    let result_path = out.join(EDITOR_DEBUG_SESSION_RESULT_PATH);
    let result_html_path = out.join(EDITOR_DEBUG_SESSION_RESULT_HTML_PATH);
    assert!(!result_path.exists());
    assert!(!result_html_path.exists());

    cmd_editor_run_debug(
        &out.join(EDITOR_DEBUG_SESSION_RUNNER_PATH),
        &[EditorDebugControl::Next, EditorDebugControl::Next],
        &[],
        &[],
        &[],
        &[],
        &[],
    )
    .expect("run exported debug runner");

    let result = read_json_value(&result_path).expect("debug runner result artifact");
    assert_eq!(result["kind"], "orv.editor.debug.runner.result");
    assert_eq!(result["debug"]["stack"]["stackFrames"][0]["line"], 3);
    assert_eq!(
        result["runner"]["result"]["path"],
        EDITOR_DEBUG_SESSION_RESULT_PATH
    );
    let result_html =
        std::fs::read_to_string(result_html_path).expect("debug result html artifact");
    assert!(result_html.contains("id=\"orv-debug-result\""));
    assert!(result_html.contains("Selected Frame"));
    assert!(result_html.contains("Session Summary"));
    assert!(result_html.contains("Source Navigation"));
    assert!(result_html.contains("Stack Frames"));
    assert!(result_html.contains("Scopes"));
    assert!(result_html.contains("Locals"));
    assert!(result_html.contains("Project Variables"));
    assert!(result_html.contains("Executed Controls"));
    assert!(result_html.contains("Requested Breakpoints"));
    assert!(result_html.contains("Function Breakpoints"));
    assert!(result_html.contains("Data Breakpoints"));
    assert!(result_html.contains("Exception Filters"));
    assert!(result_html.contains("Watch Expressions"));
    assert!(result_html.contains("Stopped Events"));
    assert!(result_html.contains("All Events"));
    assert!(result_html.contains("initialized"));
    assert!(result_html.contains("line 3"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn editor_run_debug_writes_native_debug_result_panel_contract() {
    let dir = temp_output_dir("editor-run-debug-result-panel");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        "let first: int = 1\nlet second: int = first + 1\nlet third: int = second + 1\n",
    )
    .expect("write source");
    let out = dir.join("editor");
    cmd_editor_export(&path, &out).expect("editor export");
    let watch_expressions = vec!["third".to_string(), "runtimeStatus".to_string()];

    cmd_editor_run_debug(
        &out.join(EDITOR_DEBUG_SESSION_RUNNER_PATH),
        &[EditorDebugControl::Next, EditorDebugControl::Next],
        &[],
        &[],
        &[],
        &[],
        &watch_expressions,
    )
    .expect("run exported debug runner");

    let result =
        read_json_value(&out.join(EDITOR_DEBUG_SESSION_RESULT_PATH)).expect("debug result");
    assert_eq!(result["panels"]["debug"]["schema_version"], 1);
    assert_eq!(result["panels"]["debug"]["control_count"], 2);
    assert_eq!(result["panels"]["debug"]["breakpoint_count"], 0);
    assert_eq!(result["panels"]["debug"]["function_breakpoint_count"], 0);
    assert_eq!(result["panels"]["debug"]["data_breakpoint_count"], 0);
    assert_eq!(result["panels"]["debug"]["exception_filter_count"], 0);
    assert_eq!(result["panels"]["debug"]["watch_expression_count"], 2);
    let panel_controls = result["panels"]["debug"]["controls"]
        .as_array()
        .expect("panel controls");
    assert_eq!(panel_controls.len(), 2);
    assert_eq!(panel_controls[0]["name"], "Next");
    assert_eq!(panel_controls[1]["name"], "Next");
    assert!(result["runner"]["result"]["panel_contract"]["sections"]
        .as_array()
        .expect("panel sections")
        .iter()
        .any(|section| {
            section["name"] == "production_summary"
                && section["path"] == "panels.debug.production_summary"
        }));
    assert!(result["runner"]["result"]["panel_contract"]["sections"]
        .as_array()
        .expect("panel sections")
        .iter()
        .any(|section| {
            section["name"] == "source_navigation"
                && section["path"] == "panels.debug.source_navigation"
        }));
    assert!(result["runner"]["result"]["panel_contract"]["sections"]
        .as_array()
        .expect("panel sections")
        .iter()
        .any(|section| {
            section["name"] == "scopes" && section["path"] == "panels.debug.scopes"
        }));
    assert!(result["runner"]["result"]["panel_contract"]["sections"]
        .as_array()
        .expect("panel sections")
        .iter()
        .any(|section| {
            section["name"] == "controls" && section["path"] == "panels.debug.controls"
        }));
    assert!(result["runner"]["result"]["panel_contract"]["sections"]
        .as_array()
        .expect("panel sections")
        .iter()
        .any(|section| {
            section["name"] == "breakpoints" && section["path"] == "panels.debug.breakpoints"
        }));
    assert!(result["runner"]["result"]["panel_contract"]["sections"]
        .as_array()
        .expect("panel sections")
        .iter()
        .any(|section| {
            section["name"] == "function_breakpoints"
                && section["path"] == "panels.debug.function_breakpoints"
        }));
    assert!(result["runner"]["result"]["panel_contract"]["sections"]
        .as_array()
        .expect("panel sections")
        .iter()
        .any(|section| {
            section["name"] == "data_breakpoints"
                && section["path"] == "panels.debug.data_breakpoints"
        }));
    assert!(result["runner"]["result"]["panel_contract"]["sections"]
        .as_array()
        .expect("panel sections")
        .iter()
        .any(|section| {
            section["name"] == "exception_filters"
                && section["path"] == "panels.debug.exception_filters"
        }));
    assert!(result["runner"]["result"]["panel_contract"]["sections"]
        .as_array()
        .expect("panel sections")
        .iter()
        .any(|section| {
            section["name"] == "watch_expressions"
                && section["path"] == "panels.debug.watch_expressions"
        }));
    assert_eq!(
        result["panels"]["debug"]["session_summary"]["schema_version"],
        1
    );
    assert_eq!(
        result["panels"]["debug"]["session_summary"]["program"],
        path.display().to_string()
    );
    assert_eq!(
        result["panels"]["debug"]["session_summary"]["selected_line"],
        3
    );
    assert_eq!(
        result["panels"]["debug"]["session_summary"]["control_count"],
        2
    );
    assert_eq!(
        result["panels"]["debug"]["session_summary"]["function_breakpoint_count"],
        0
    );
    assert_eq!(
        result["panels"]["debug"]["session_summary"]["data_breakpoint_count"],
        0
    );
    assert_eq!(
        result["panels"]["debug"]["session_summary"]["exception_filter_count"],
        0
    );
    assert_eq!(
        result["panels"]["debug"]["session_summary"]["watch_expression_count"],
        2
    );
    assert!(result["panels"]["debug"]["session_summary"]["last_event"]
        .as_str()
        .is_some_and(|event| !event.is_empty()));
    assert_eq!(result["panels"]["debug"]["selected_frame"]["line"], 3);
    assert!(result["panels"]["debug"]["stack_frames"]
        .as_array()
        .expect("stack frames")
        .iter()
        .any(|frame| frame["line"] == 3));
    assert_eq!(
        result["panels"]["debug"]["source_navigation"]["selected"]["line"],
        3
    );
    assert!(
        result["panels"]["debug"]["source_navigation"]["selected"]["source"]["path"]
            .as_str()
            .is_some_and(|source| source.ends_with("app.orv"))
    );
    assert!(result["panels"]["debug"]["source_navigation"]["frames"]
        .as_array()
        .expect("source navigation frames")
        .iter()
        .any(|frame| frame["line"] == 3));
    assert!(result["panels"]["debug"]["scopes"]["scopes"]
        .as_array()
        .expect("scopes")
        .iter()
        .any(|scope| scope["name"] == "Locals" || scope["name"] == "Project"));
    assert!(result["panels"]["debug"]["locals"]
        .as_array()
        .expect("locals")
        .iter()
        .any(|local| local["name"] == "third" && local["value"] == "3"));
    let watch_panel = result["panels"]["debug"]["watch_expressions"]
        .as_array()
        .expect("watch expressions");
    assert_eq!(watch_panel.len(), 2);
    assert!(watch_panel.iter().any(|expression| {
        expression["expression"] == "third"
            && expression["response"]["success"] == true
            && expression["response"]["body"]["result"] == "3"
            && expression["response"]["body"]["type"] == "int"
    }));
    assert!(watch_panel.iter().any(|expression| {
        expression["expression"] == "runtimeStatus"
            && expression["response"]["success"] == true
            && expression["response"]["body"]["type"] == "string"
    }));
    assert!(result["panels"]["debug"]["project_variables"]
        .as_array()
        .expect("project variables")
        .iter()
        .any(|variable| variable["name"] == "stdout"));
    assert!(
        result["panels"]["debug"]["stopped_events"]
            .as_array()
            .expect("stopped events")
            .len()
            >= 2
    );
    assert!(
        result["panels"]["debug"]["event_count"]
            .as_u64()
            .is_some_and(|count| count >= 2),
        "{result}"
    );
    assert!(
        result["panels"]["debug"]["events"]
            .as_array()
            .expect("events")
            .iter()
            .any(|event| event["event"] == "stopped"),
        "{result}"
    );
    assert!(result["panels"]["debug"]["result_artifact"]["path"]
        .as_str()
        .is_some_and(|path| path.ends_with(EDITOR_DEBUG_SESSION_RESULT_PATH)));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn editor_export_embeds_trace_navigation_state() {
    let dir = temp_output_dir("editor-export-trace");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        r"@server {
  @listen 0
  @route GET /ping {
    @respond 200 { ok: true }
  }
}",
    )
    .expect("write source");
    let build_out = dir.join("dist");

    cmd_build(&path, &build_out).expect("build artifacts");
    let origin_map: orv_compiler::OriginMap = serde_json::from_str(
        &std::fs::read_to_string(build_out.join("origin-map.json")).expect("origin map"),
    )
    .expect("origin map json");
    let route = origin_map
        .entries
        .iter()
        .find(|entry| entry.kind == "route" && entry.name == "GET /ping")
        .expect("route origin");
    let trace_path = dir.join("production-trace.json");
    write_json(
        &trace_path,
        &serde_json::json!({
            "schema_version": 1,
            "kind": "orv.production.trace",
            "frames": [{
                "method": "GET",
                "path": "/ping",
                "status": 200,
                "route_origin_id": route.id,
            }],
        }),
    )
    .expect("write trace");
    let out = dir.join("editor");

    cmd_editor_export_with_options(&path, &out, Some(&build_out), Some(&trace_path))
        .expect("editor export with trace");

    let html = std::fs::read_to_string(out.join("index.html")).expect("editor html");
    let trace_panel =
        std::fs::read_to_string(out.join(EDITOR_TRACE_PANEL_HTML_PATH)).expect("trace panel");
    let state = read_json_value(&out.join("state.json")).expect("editor state");
    let native_host =
        read_json_value(&out.join(EDITOR_NATIVE_HOST_MANIFEST_PATH)).expect("native host");
    assert!(html.contains("Trace"));
    assert!(html.contains("id=\"trace-list\""));
    assert!(html.contains("id=\"trace-detail\""));
    assert!(html.contains("renderEditorState"));
    assert!(html.contains("renderTraceDetail"));
    assert_eq!(state["trace"]["kind"], "orv.editor.trace");
    assert_eq!(state["trace"]["frames"][0]["origin_id"], route.id);
    assert_eq!(
        state["trace"]["frames"][0]["navigation"]["focus"]["panel"],
        "routes"
    );
    assert!(trace_panel.contains("Trace Panel"));
    assert!(trace_panel.contains("GET /ping -> 200"));
    assert!(trace_panel.contains(route.id.as_str()));
    assert_eq!(
        native_host["artifacts"]["trace_panel_html"],
        EDITOR_TRACE_PANEL_HTML_PATH
    );
    assert_eq!(
        native_host["trace"]["panel_html_path"],
        EDITOR_TRACE_PANEL_HTML_PATH
    );
    assert_eq!(
        native_host["trace"]["panel_artifact"]["path"],
        EDITOR_TRACE_PANEL_HTML_PATH
    );
    assert_eq!(
        native_host["trace"]["panel_artifact"]["kind"],
        "orv.editor.trace.panel"
    );
    let panels = native_host["panels"]
        .as_array()
        .expect("native host panel inventory");
    assert!(panels.iter().any(|panel| {
        panel["name"] == "trace" && panel["artifact"]["path"] == EDITOR_TRACE_PANEL_HTML_PATH
    }));
    assert_eq!(native_host["capabilities"]["trace_navigation"], true);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn editor_export_native_host_includes_trace_transport_contract() {
    let (src_dir, path) = prod_server_source("editor-export-trace-transport-source");
    let build_out = temp_output_dir("editor-export-trace-transport-build");

    cmd_build_with_profile(&path, &build_out, BuildProfile::Production).expect("prod build");
    let trace_path = src_dir.join("production-trace.json");
    write_json(
        &trace_path,
        &serde_json::json!({
            "schema_version": 1,
            "kind": "orv.production.trace",
            "frames": [],
        }),
    )
    .expect("write trace");
    let editor_out = src_dir.join("editor");

    cmd_editor_export_with_options(&path, &editor_out, Some(&build_out), Some(&trace_path))
        .expect("editor export with trace transport");

    let html = std::fs::read_to_string(editor_out.join("index.html")).expect("editor html");
    let native_host =
        read_json_value(&editor_out.join(EDITOR_NATIVE_HOST_MANIFEST_PATH)).expect("native host");
    assert_eq!(native_host["trace"]["kind"], "orv.editor.native_host.trace");
    assert_eq!(
        native_host["trace"]["panel_html_path"],
        EDITOR_TRACE_PANEL_HTML_PATH
    );
    assert_eq!(
        native_host["trace"]["transport"]["url"],
        "http://127.0.0.1:8080/__orv/trace/events"
    );
    assert_eq!(
        native_host["trace"]["stream_runner"]["kind"],
        "orv.editor.native_host.trace_stream_runner"
    );
    assert_eq!(
        native_host["trace"]["stream_runner"]["event_stream"],
        "trace/events.sse"
    );
    assert_eq!(
        native_host["trace"]["stream_runner"]["command"],
        serde_json::json!([
            "orv",
            "editor",
            "trace-stream",
            build_out.display().to_string(),
            "--events",
            "trace/events.sse"
        ])
    );
    assert_eq!(native_host["trace"]["frame_count"], 0);
    assert!(html.contains("Trace Transport"));
    assert!(html.contains("id=\"trace-transport-detail\""));
    assert!(html.contains("Trace Stream Runner"));
    assert!(html.contains("id=\"trace-stream-runner-detail\""));
    assert!(html.contains("renderTraceTransport"));
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(build_out);
}

#[test]
fn editor_export_native_host_includes_trace_frame_navigation_inventory() {
    let (src_dir, path) = prod_server_source("editor-export-trace-frame-source");
    let build_out = temp_output_dir("editor-export-trace-frame-build");

    cmd_build_with_profile(&path, &build_out, BuildProfile::Production).expect("prod build");
    let origin_map: orv_compiler::OriginMap = serde_json::from_str(
        &std::fs::read_to_string(build_out.join("origin-map.json")).expect("origin map"),
    )
    .expect("origin map json");
    let route = origin_map
        .entries
        .iter()
        .find(|entry| entry.kind == "route" && entry.name == "GET /ping")
        .expect("route origin");
    let trace_path = src_dir.join("production-trace.json");
    write_json(
        &trace_path,
        &serde_json::json!({
            "schema_version": 1,
            "kind": "orv.production.trace",
            "frames": [
                {
                    "method": "GET",
                    "path": "/ping",
                    "status": 200,
                    "route_origin_id": route.id,
                },
                {
                    "method": "GET",
                    "path": "/missing",
                    "status": 404,
                },
            ],
        }),
    )
    .expect("write trace");
    let editor_out = src_dir.join("editor");

    cmd_editor_export_with_options(&path, &editor_out, Some(&build_out), Some(&trace_path))
        .expect("editor export with trace frame inventory");

    let native_host =
        read_json_value(&editor_out.join(EDITOR_NATIVE_HOST_MANIFEST_PATH)).expect("native host");
    let frames = native_host["trace"]["frames"]
        .as_array()
        .expect("native trace frames");
    assert_eq!(
        native_host["trace"]["summary"]["schema_version"],
        serde_json::json!(1)
    );
    assert_eq!(native_host["trace"]["summary"]["frame_count"], 2);
    assert_eq!(
        native_host["trace"]["summary"]["status_counts"]["client_error"],
        1
    );
    assert_eq!(
        native_host["trace"]["summary"]["first_request"]["label"],
        "GET /ping -> 200"
    );
    assert_eq!(
        native_host["trace"]["summary"]["last_request"]["label"],
        "GET /missing -> 404"
    );
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0]["index"], 0);
    assert_eq!(frames[0]["origin_id"], route.id);
    assert_eq!(frames[0]["summary"]["status_class"], "ok");
    assert_eq!(frames[0]["request"]["path"], "/ping");
    assert_eq!(frames[0]["navigation"]["focus"]["panel"], "routes");
    assert_eq!(frames[0]["source"], frames[0]["navigation"]["source"]);
    assert_eq!(
        frames[0]["production"],
        frames[0]["navigation"]["production"]
    );
    assert_eq!(
        frames[0]["reveal_command"],
        serde_json::json!([
            "orv",
            "editor",
            "reveal",
            build_out.display().to_string(),
            route.id
        ])
    );
    assert!(frames[0]["navigation"]["source"]["snippet"]
        .as_str()
        .is_some_and(|snippet| snippet.contains("@route GET /ping")));
    assert_eq!(frames[1]["summary"]["status_class"], "client_error");
    assert_eq!(frames[1]["navigation"], serde_json::Value::Null);
    assert_eq!(frames[1]["source"], serde_json::Value::Null);
    assert_eq!(frames[1]["production"], serde_json::Value::Null);
    assert_eq!(frames[1]["reveal_command"], serde_json::Value::Null);
    let filters = native_host["trace"]["status_filters"]
        .as_array()
        .expect("native trace status filters");
    assert!(filters
        .iter()
        .any(|filter| filter["name"] == "all" && filter["count"] == 2));
    assert!(filters
        .iter()
        .any(|filter| filter["name"] == "client_error" && filter["count"] == 1));
    assert_eq!(native_host["trace"]["panel_contract"]["root"], "trace");
    let sections = native_host["trace"]["panel_contract"]["sections"]
        .as_array()
        .expect("native trace panel sections");
    assert!(sections
        .iter()
        .any(|section| section["name"] == "summary" && section["path"] == "trace.summary"));
    assert!(sections
        .iter()
        .any(|section| section["name"] == "status_filters"
            && section["path"] == "trace.status_filters"));
    assert!(sections
        .iter()
        .any(|section| section["name"] == "frames" && section["path"] == "trace.frames"));
    assert!(sections
        .iter()
        .any(|section| section["name"] == "panel_artifact"
            && section["path"] == "trace.panel_artifact"));
    let _ = std::fs::remove_dir_all(src_dir);
    let _ = std::fs::remove_dir_all(build_out);
}

#[test]
fn editor_export_renders_trace_status_filters() {
    let state = serde_json::json!({
        "schema_version": 1,
        "snapshot": {
            "entry": { "path": "app.orv" },
            "panels": {
                "files": [],
                "routes": [],
                "schema": [],
                "domains": []
            },
            "diagnostics": []
        },
        "runtime": {
            "runtime": {
                "status": "ok",
                "stdout": ""
            }
        },
        "trace": {
            "trace": {
                "status_counts": {
                    "total": 3,
                    "ok": 1,
                    "redirect": 0,
                    "client_error": 1,
                    "server_error": 1,
                    "other": 0
                }
            },
            "frames": [
                { "summary": { "label": "GET /ok -> 200", "status_class": "ok" } },
                { "summary": { "label": "GET /missing -> 404", "status_class": "client_error" } },
                { "summary": { "label": "POST /checkout -> 503", "status_class": "server_error" } }
            ]
        }
    });

    let html = editor_export_html(&state).expect("editor html");

    assert!(html.contains("id=\"trace-status-summary\""));
    assert!(html.contains("data-trace-filter=\"client_error\""));
    assert!(html.contains("data-trace-filter=\"server_error\""));
    assert!(html.contains("filterTraceFrames"));
    assert!(html.contains("Client Err<b>1</b>"));
    assert!(html.contains("Server Err<b>1</b>"));
}

#[test]
fn editor_export_with_build_embeds_production_adapter_summary() {
    let dir = temp_output_dir("editor-export-production-source");
    std::fs::create_dir_all(&dir).expect("create editor export source dir");
    let path = dir.join("app.orv");
    std::fs::write(
        &path,
        r#"@server {
  @listen 8080
  let shopdb = @db.connect(@env.SHOP_DATABASE_URL ?? "postgres://db.internal/shop")
  let payments = @payment.connect(@env.PAYMENT_ADAPTER_URL ?? "http://payments.internal/capture")
  @route POST /checkout {
    @csrf
    let captured = payments.capture({ orderId: "o_1", amount: 42, method: "card" })
    @respond 200 { payment: captured.status }
  }
}
"#,
    )
    .expect("write editor export source");
    let out = temp_output_dir("editor-export-production");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let state =
        editor_export_state_json_with_trace(&path, Some(&out), None).expect("editor export state");
    let html = editor_export_html(&state).expect("editor html");

    let graph_contract = state["production"]["graph_contract"]
        .as_array()
        .expect("graph contract targets");
    let source_bundle_target = graph_contract
        .iter()
        .find(|target| target["kind"] == "source_bundle")
        .expect("source bundle target");
    let project_graph_target = graph_contract
        .iter()
        .find(|target| target["kind"] == "project_graph")
        .expect("project graph target");
    let origin_map_target = graph_contract
        .iter()
        .find(|target| target["kind"] == "origin_map")
        .expect("origin map target");
    assert_eq!(source_bundle_target["path"], SOURCE_BUNDLE_PATH);
    assert_eq!(source_bundle_target["exists"], true);
    assert_eq!(source_bundle_target["file_count"], 1);
    assert!(source_bundle_target["artifact_hash"].as_str().is_some());
    assert!(source_bundle_target["files"]
        .as_array()
        .expect("source bundle files")
        .iter()
        .any(|file| file["path"]
            .as_str()
            .is_some_and(|path| path.ends_with("app.orv"))
            && file["content_hash"].as_str().is_some()));
    assert_eq!(project_graph_target["path"], "project-graph.json");
    assert_eq!(project_graph_target["exists"], true);
    assert!(project_graph_target["node_count"]
        .as_u64()
        .is_some_and(|count| count > 0));
    assert!(project_graph_target["semantic_origin_count"]
        .as_u64()
        .is_some_and(|count| count > 0));
    assert_eq!(origin_map_target["path"], "origin-map.json");
    assert_eq!(origin_map_target["exists"], true);
    assert!(origin_map_target["entry_count"]
        .as_u64()
        .is_some_and(|count| count > 0));
    assert_eq!(
        state["production"]["db_adapters"][0]["path"],
        "deploy/db-adapters.json"
    );
    assert_eq!(
        state["production"]["commerce_adapters"][0]["path"],
        "deploy/commerce-adapters.json"
    );
    let db_origin_id = state["production"]["db_adapters"][0]["adapters"][0]["source_origin_id"]
        .as_str()
        .expect("db adapter source origin");
    let commerce_origin_id = state["production"]["commerce_adapters"][0]["adapters"][0]
        ["source_origin_id"]
        .as_str()
        .expect("commerce adapter source origin");
    assert_eq!(
        state["production"]["db_adapters"][0]["source_reveal_commands"][0]["source_origin_id"],
        db_origin_id
    );
    assert_eq!(
        state["production"]["db_adapters"][0]["source_reveal_commands"][0]["command"],
        serde_json::json!([
            "orv",
            "editor",
            "reveal",
            out.display().to_string(),
            db_origin_id
        ])
    );
    assert_eq!(
        state["production"]["commerce_adapters"][0]["source_reveal_commands"][0]
            ["source_origin_id"],
        commerce_origin_id
    );
    assert_eq!(
        state["production"]["commerce_adapters"][0]["source_reveal_commands"][0]["command"],
        serde_json::json!([
            "orv",
            "editor",
            "reveal",
            out.display().to_string(),
            commerce_origin_id
        ])
    );
    assert_eq!(
        state["production"]["preflight"][0]["path"],
        "deploy/preflight.json"
    );
    assert_eq!(
        state["production"]["preflight"][0]["commands"]["verify_build"],
        "orv verify-build ."
    );
    assert_eq!(
        state["production"]["preflight"][0]["commands"]["benchmark_report"],
        "orv benchmark-report ."
    );
    assert_eq!(
        state["production"]["preflight"][0]["commands"]["benchmark_report_require_pass"],
        "orv benchmark-report . --require-pass"
    );
    assert_eq!(
        state["production"]["preflight"][0]["artifacts"]["benchmark_evidence"],
        "deploy/benchmark-evidence.json"
    );
    assert_eq!(
        state["production"]["preflight"][0]["artifacts"]["smoke_output"],
        "deploy/smoke-output.txt"
    );
    assert_eq!(
        state["production"]["preflight"][0]["benchmark_evidence"]["recording_status"],
        "not_recorded"
    );
    assert_eq!(
        state["production"]["preflight"][0]["benchmark_evidence"]["report_status"],
        "incomplete"
    );
    assert_eq!(
        state["production"]["preflight"][0]["benchmark_evidence"]["missing_task_count"],
        10
    );
    assert_eq!(
        state["production"]["preflight"][0]["benchmark_evidence"]["missing_data_count"],
        3
    );
    assert_eq!(
        state["production"]["preflight"][0]["benchmark_evidence"]["smoke_test_required_markers"],
        serde_json::json!(deploy_benchmark::SMOKE_REQUIRED_MARKERS)
    );
    assert_eq!(
        state["production"]["preflight"][0]["benchmark_evidence"]["smoke_test_summary"]["present"],
        false
    );
    assert_eq!(
        state["production"]["preflight"][0]["benchmark_evidence"]["smoke_test_summary"]
            ["required_markers"],
        serde_json::json!(deploy_benchmark::SMOKE_REQUIRED_MARKERS)
    );
    assert_eq!(
        state["production"]["summary"]["schema_version"],
        serde_json::json!(1)
    );
    assert_eq!(state["production"]["summary"]["graph_contract_count"], 3);
    assert_eq!(
        state["production"]["native_server"][0]["path"],
        "server/native-server.json"
    );
    assert_eq!(
        state["production"]["summary"]["native_server_target_count"],
        1
    );
    assert_eq!(
        state["production"]["summary"]["native_server_route_count"],
        1
    );
    assert_eq!(state["production"]["summary"]["static_target_count"], 0);
    assert_eq!(state["production"]["summary"]["preflight_target_count"], 1);
    assert_eq!(
        state["production"]["summary"]["preflight_smoke_summary_present_count"],
        0
    );
    assert_eq!(
        state["production"]["summary"]["preflight_smoke_summary_missing_count"],
        1
    );
    assert_eq!(
        state["production"]["summary"]["preflight_smoke_summary_missing_marker_count"],
        0
    );
    assert!(
        state["production"]["preflight"][0]["benchmark_evidence"]["missing_data"]
            .as_array()
            .expect("missing data")
            .iter()
            .any(|item| item == "smoke_test_output")
    );
    let checkout_route = json_route(
        &state["production"]["preflight"][0]["routes"],
        "POST",
        "/checkout",
    )
    .expect("checkout route");
    assert!(checkout_route["policies"]
        .as_array()
        .expect("checkout policies")
        .iter()
        .any(|policy| policy["kind"] == "csrf"
            && policy["required"] == true
            && policy["origin_id"]
                .as_str()
                .is_some_and(|origin_id| origin_id.starts_with("ori_"))));
    assert!(checkout_route["policies"]
        .as_array()
        .expect("checkout policies")
        .iter()
        .any(|policy| policy["kind"] == "rate_limit"
            && policy["limit"] == 10
            && policy["window_seconds"] == 60));
    let native_host = editor_native_host_manifest_json(&path, &state);
    assert_eq!(
        native_host["production"]["db_adapters"][0]["path"],
        "deploy/db-adapters.json"
    );
    assert_eq!(
        native_host["production"]["commerce_adapters"][0]["path"],
        "deploy/commerce-adapters.json"
    );
    assert_eq!(
        native_host["production"]["db_adapters"][0]["source_reveal_commands"][0]
            ["source_origin_id"],
        db_origin_id
    );
    assert_eq!(
        native_host["production"]["db_adapters"][0]["source_reveal_commands"][0]["command"],
        serde_json::json!([
            "orv",
            "editor",
            "reveal",
            out.display().to_string(),
            db_origin_id
        ])
    );
    assert_eq!(
        native_host["production"]["commerce_adapters"][0]["source_reveal_commands"][0]
            ["source_origin_id"],
        commerce_origin_id
    );
    assert_eq!(
        native_host["production"]["commerce_adapters"][0]["source_reveal_commands"][0]["command"],
        serde_json::json!([
            "orv",
            "editor",
            "reveal",
            out.display().to_string(),
            commerce_origin_id
        ])
    );
    assert_eq!(
        native_host["production"]["preflight"][0]["path"],
        "deploy/preflight.json"
    );
    assert_eq!(
        native_host["production"]["graph_contract"],
        state["production"]["graph_contract"]
    );
    assert_eq!(
        native_host["production"]["summary"],
        state["production"]["summary"]
    );
    assert_eq!(
        native_host["production"]["summary"]["schema_version"],
        serde_json::json!(1)
    );
    assert_eq!(
        native_host["production"]["summary"]["graph_contract_count"],
        3
    );
    assert_eq!(
        native_host["production"]["summary"]["source_bundle_file_count"],
        1
    );
    assert!(
        native_host["production"]["summary"]["project_graph_node_count"]
            .as_u64()
            .is_some_and(|count| count > 0)
    );
    assert!(native_host["production"]["summary"]["origin_entry_count"]
        .as_u64()
        .is_some_and(|count| count > 0));
    assert_eq!(
        native_host["production"]["summary"]["preflight_target_count"],
        1
    );
    assert_eq!(
        native_host["production"]["summary"]["preflight_command_count"],
        12
    );
    assert_eq!(
        native_host["production"]["summary"]["preflight_route_count"],
        1
    );
    assert_eq!(
        native_host["production"]["summary"]["native_server_target_count"],
        1
    );
    assert_eq!(
        native_host["production"]["summary"]["native_server_route_count"],
        1
    );
    assert_eq!(
        native_host["production"]["summary"]["static_target_count"],
        0
    );
    assert_eq!(
        native_host["production"]["summary"]["route_policy_count"],
        2
    );
    assert_eq!(
        native_host["production"]["summary"]["route_policy_kind_counts"]["csrf"],
        1
    );
    assert_eq!(
        native_host["production"]["summary"]["route_policy_kind_counts"]["rate_limit"],
        1
    );
    assert_eq!(
        native_host["production"]["summary"]["preflight_optional_env_count"],
        5
    );
    assert_eq!(
        native_host["production"]["summary"]["preflight_smoke_summary_present_count"],
        0
    );
    assert_eq!(
        native_host["production"]["summary"]["preflight_smoke_summary_missing_count"],
        1
    );
    assert_eq!(
        native_host["production"]["summary"]["preflight_smoke_summary_missing_marker_count"],
        0
    );
    assert_eq!(native_host["production"]["summary"]["db_target_count"], 1);
    assert_eq!(
        native_host["production"]["summary"]["commerce_target_count"],
        1
    );
    assert!(
        native_host["production"]["summary"]["adapter_count"]
            .as_u64()
            .is_some_and(|count| count >= 2),
        "{native_host}"
    );
    assert_eq!(
        native_host["production"]["summary"]["missing_artifact_count"],
        0
    );
    assert_eq!(
        native_host["production"]["panel_contract"]["root"],
        "production"
    );
    let production_sections = native_host["production"]["panel_contract"]["sections"]
        .as_array()
        .expect("production panel sections");
    assert!(production_sections
        .iter()
        .any(|section| section["name"] == "summary" && section["path"] == "production.summary"));
    assert!(production_sections
        .iter()
        .any(|section| section["name"] == "graph_contract"
            && section["path"] == "production.graph_contract"));
    assert!(production_sections
        .iter()
        .any(|section| section["name"] == "db_adapters"
            && section["path"] == "production.db_adapters"));
    assert!(
        production_sections
            .iter()
            .any(|section| section["name"] == "preflight"
                && section["path"] == "production.preflight")
    );
    assert!(production_sections
        .iter()
        .any(|section| section["name"] == "native_server"
            && section["path"] == "production.native_server"));
    assert!(production_sections
        .iter()
        .any(|section| section["name"] == "static" && section["path"] == "production.static"));
    assert!(production_sections
        .iter()
        .any(|section| section["name"] == "route_policies"
            && section["path"] == "production.summary.route_policy_kind_counts"));
    assert!(production_sections
        .iter()
        .any(|section| section["name"] == "commerce_adapters"
            && section["path"] == "production.commerce_adapters"));
    assert!(production_sections
        .iter()
        .any(|section| section["name"] == "panel_artifact"
            && section["path"] == "production.panel_artifact"));
    assert_eq!(
        native_host["production"]["panel_html_path"],
        EDITOR_PRODUCTION_PANEL_HTML_PATH
    );
    assert_eq!(
        native_host["production"]["panel_artifact"]["path"],
        EDITOR_PRODUCTION_PANEL_HTML_PATH
    );
    assert_eq!(
        native_host["production"]["panel_artifact"]["kind"],
        "orv.editor.production.panel"
    );
    assert_eq!(native_host["capabilities"]["production_adapters"], true);
    assert_eq!(
        native_host["capabilities"]["production_graph_contract"],
        true
    );
    assert_eq!(native_host["capabilities"]["production_preflight"], true);
    assert_eq!(
        native_host["capabilities"]["production_route_policies"],
        true
    );
    assert!(html.contains("Production"));
    assert!(html.contains("Graph source_bundle source-bundle.json"));
    assert!(html.contains("Preflight"));
    assert!(html.contains("commands 12"));
    assert!(html.contains("route_policies 2"));
    assert!(html.contains("smoke_summary_present false"));
    assert!(html.contains("DB Adapters"));
    assert!(html.contains("Commerce Adapters"));
    assert!(html.contains("deploy/db-adapters.json"));
    let editor_out = dir.join("editor");
    cmd_editor_export_with_options(&path, &editor_out, Some(&out), None)
        .expect("editor export with production panel");
    let export_native_host =
        read_json_value(&editor_out.join(EDITOR_NATIVE_HOST_MANIFEST_PATH)).expect("native host");
    let production_panel =
        std::fs::read_to_string(editor_out.join(EDITOR_PRODUCTION_PANEL_HTML_PATH))
            .expect("production panel");
    assert_eq!(
        export_native_host["artifacts"]["production_panel_html"],
        EDITOR_PRODUCTION_PANEL_HTML_PATH
    );
    let export_panels = export_native_host["panels"]
        .as_array()
        .expect("native host panel inventory");
    assert!(export_panels.iter().any(|panel| {
        panel["name"] == "production"
            && panel["artifact"]["path"] == EDITOR_PRODUCTION_PANEL_HTML_PATH
    }));
    assert!(production_panel.contains("Production Panel"));
    assert!(production_panel.contains("Graph Contract"));
    assert!(production_panel.contains("source-bundle.json"));
    assert!(production_panel.contains("project-graph.json"));
    assert!(production_panel.contains("origin-map.json"));
    assert!(production_panel.contains("Native Server"));
    assert!(production_panel.contains("Native Plans</span><b>1</b>"));
    assert!(production_panel.contains("Native Routes</span><b>1</b>"));
    assert!(production_panel.contains("Static Pages"));
    assert!(production_panel.contains("Static Pages</span><b>0/0</b>"));
    assert!(production_panel.contains("Preflight"));
    assert!(production_panel.contains("\"benchmark_report\": \"orv benchmark-report .\""));
    assert!(production_panel
        .contains("\"benchmark_report_require_pass\": \"orv benchmark-report . --require-pass\""));
    assert!(production_panel.contains("\"report_status\": \"incomplete\""));
    assert!(production_panel.contains("\"missing_task_count\": 10"));
    assert!(production_panel.contains("\"smoke_test_required_markers\""));
    assert!(production_panel.contains("\"dap_source_bundle\""));
    assert!(production_panel.contains("\"smoke_test_summary\""));
    assert!(production_panel.contains("\"required_markers\""));
    assert!(production_panel.contains("\"present\": false"));
    assert!(production_panel.contains("\"preflight_smoke_summary_present_count\": 0"));
    assert!(production_panel.contains("\"preflight_smoke_summary_missing_count\": 1"));
    assert!(production_panel.contains("\"preflight_smoke_summary_missing_marker_count\": 0"));
    assert!(production_panel.contains("Smoke Summary</span><b>0/1</b>"));
    assert!(production_panel.contains("Smoke Gaps</span><b class=\"bad\">1</b>"));
    assert!(production_panel.contains("\"smoke_test_output\""));
    assert!(production_panel.contains("Preflight Commands</span><b>12</b>"));
    assert!(production_panel.contains("Route Policies"));
    assert!(production_panel.contains("Route Policy Summary"));
    assert!(production_panel.contains("\"csrf\": 1"));
    assert!(production_panel.contains("\"rate_limit\": 1"));
    assert!(production_panel.contains("DB Adapters"));
    assert!(production_panel.contains("Commerce Adapters"));
    assert!(production_panel.contains("deploy/preflight.json"));
    assert!(production_panel.contains("deploy/benchmark-evidence.json"));
    assert!(production_panel.contains("deploy/db-adapters.json"));
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(out);
}

#[test]
fn editor_export_with_build_embeds_production_client_capabilities() {
    let dir = temp_output_dir("editor-export-production-client-source");
    std::fs::create_dir_all(&dir).expect("create editor export client source dir");
    let path = dir.join("page.orv");
    std::fs::write(
        &path,
        "let sig count: int = 0\n@out @html { @body { @p count } }",
    )
    .expect("write editor export client source");
    let out = temp_output_dir("editor-export-production-client");

    cmd_build_with_profile(&path, &out, BuildProfile::Production).expect("prod build");
    let state =
        editor_export_state_json_with_trace(&path, Some(&out), None).expect("editor export state");
    let html = editor_export_html(&state).expect("editor html");
    let client_targets = state["production"]["client"]
        .as_array()
        .expect("production client targets");
    let client_manifest = client_targets
        .iter()
        .find(|target| target["kind"] == "client_manifest")
        .expect("client manifest target");

    assert_eq!(client_manifest["path"], CLIENT_MANIFEST_PATH);
    assert_eq!(
        client_manifest["capabilities"]["runtime"],
        serde_json::json!("client_wasm")
    );
    assert_eq!(
        client_manifest["capabilities"]["bindings"]["signal_text"],
        1
    );
    assert!(client_manifest["capabilities"]["surfaces"]
        .as_array()
        .expect("client capability surfaces")
        .iter()
        .any(|surface| surface == "signal_text"));

    let native_host = editor_native_host_manifest_json(&path, &state);
    assert_eq!(native_host["capabilities"]["client_bundles"], true);
    assert_eq!(
        native_host["production"]["summary"]["client_manifest_count"],
        1
    );
    assert!(
        native_host["production"]["summary"]["client_target_count"]
            .as_u64()
            .is_some_and(|count| count >= 5),
        "{native_host}"
    );
    assert!(
        native_host["production"]["summary"]["client_capability_surface_count"]
            .as_u64()
            .is_some_and(|count| count >= 2),
        "{native_host}"
    );
    let production_sections = native_host["production"]["panel_contract"]["sections"]
        .as_array()
        .expect("production panel sections");
    assert!(production_sections
        .iter()
        .any(|section| section["name"] == "client" && section["path"] == "production.client"));
    assert!(html.contains("Client Bundles"));
    assert!(html.contains("client/app.wasm"));

    let editor_out = dir.join("editor");
    cmd_editor_export_with_options(&path, &editor_out, Some(&out), None)
        .expect("editor export with production client");
    let export_native_host =
        read_json_value(&editor_out.join(EDITOR_NATIVE_HOST_MANIFEST_PATH)).expect("native host");
    let production_panel =
        std::fs::read_to_string(editor_out.join(EDITOR_PRODUCTION_PANEL_HTML_PATH))
            .expect("production panel");
    assert_eq!(export_native_host["capabilities"]["client_bundles"], true);
    assert!(production_panel.contains("Client Bundles"));
    assert!(production_panel.contains("signal_text"));
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(out);
}

#[test]
fn check_build_reanalyzes_source_bundle_without_original_source() {
    let dir = temp_output_dir("check-build-source-bundle");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let path = dir.join("page.orv");
    std::fs::write(
        &path,
        r#"let sig count: int = 0
@out @html { @body { @p count } }"#,
    )
    .expect("write source");
    let out = dir.join("dist");

    cmd_build(&path, &out).expect("build artifacts");
    std::fs::remove_file(&path).expect("remove source");

    cmd_check_build(&out).expect("check build");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn run_build_executes_server_launch_artifact_relative_to_build_dir() {
    let out = temp_output_dir("run-build");
    let artifact = out.join("server").join("app.orv-runtime.json");
    write_reference_artifact(&artifact, "artifact.orv", r#"@out "build ok""#);
    let launch = orv_compiler::ServerLaunchArtifact {
        schema_version: orv_compiler::SERVER_LAUNCH_ARTIFACT_VERSION,
        runtime: "reference-interpreter".to_string(),
        artifact: "server/app.orv-runtime.json".to_string(),
        command: vec![
            "orv".to_string(),
            "run-artifact".to_string(),
            "server/app.orv-runtime.json".to_string(),
        ],
        protocol: "http1".to_string(),
        routes: Vec::new(),
        listen: None,
    };
    write_json(
        &out.join("server").join("launch.json"),
        &serde_json::to_value(launch).expect("launch value"),
    )
    .expect("write launch");
    let mut stdout = Vec::new();

    run_build_with_writer(&out, &mut stdout).expect("run build");

    assert_eq!(
        String::from_utf8(stdout).expect("stdout utf-8"),
        "build ok\n"
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn run_build_resolves_relative_persistence_under_build_dir() {
    let out = temp_output_dir("run-build-persistence-cwd");
    let unique = std::process::id();
    let sqlite_name = format!("orv-run-build-cwd-{unique}.sqlite");
    let record_name = format!("orv-run-build-cwd-{unique}.jsonl");
    let cwd_data = std::env::current_dir().expect("cwd").join("data");
    let cwd_sqlite = cwd_data.join(&sqlite_name);
    let cwd_record = cwd_data.join(&record_name);
    let _ = std::fs::remove_file(&cwd_sqlite);
    let _ = std::fs::remove_file(&cwd_record);
    let source = format!(
        r#"let db = @db.connect "sqlite://data/{sqlite_name}"
await db.create("Item", {{ name: "ok" }})
let payments = @payment.connect("file://data/{record_name}")
payments.capture({{ orderId: 1, amount: 100, method: "card" }})
@out "ok""#
    );
    let artifact = out.join("server").join("app.orv-runtime.json");
    write_reference_artifact(&artifact, "artifact.orv", &source);
    let launch = orv_compiler::ServerLaunchArtifact {
        schema_version: orv_compiler::SERVER_LAUNCH_ARTIFACT_VERSION,
        runtime: "reference-interpreter".to_string(),
        artifact: "server/app.orv-runtime.json".to_string(),
        command: vec![
            "orv".to_string(),
            "run-artifact".to_string(),
            "server/app.orv-runtime.json".to_string(),
        ],
        protocol: "http1".to_string(),
        routes: Vec::new(),
        listen: None,
    };
    write_json(
        &out.join("server").join("launch.json"),
        &serde_json::to_value(launch).expect("launch value"),
    )
    .expect("write launch");
    let mut stdout = Vec::new();

    run_build_with_writer(&out, &mut stdout).expect("run build");

    assert_eq!(String::from_utf8(stdout).expect("stdout utf8"), "ok\n");
    assert!(out.join("data").join(&sqlite_name).is_file());
    assert!(out.join("data").join(&record_name).is_file());
    assert!(!cwd_sqlite.exists());
    assert!(!cwd_record.exists());
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn run_build_prints_zero_runtime_static_page() {
    let out = temp_output_dir("run-build-static");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(&entry, r#"@out @html { @body { @h1 "Static" } }"#).expect("write entry");
    let build_out = out.join("dist");
    cmd_build(&entry, &build_out).expect("build artifacts");
    let mut stdout = Vec::new();

    run_build_with_writer(&build_out, &mut stdout).expect("run build");

    assert_eq!(
        String::from_utf8(stdout).expect("stdout utf-8"),
        "<html><body><h1>Static</h1></body></html>"
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn run_build_prints_client_page_shell() {
    let out = temp_output_dir("run-build-client-page");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        "let sig count: int = 0\n@out @html { @body { @p count } }",
    )
    .expect("write entry");
    let build_out = out.join("dist");
    cmd_build(&entry, &build_out).expect("build artifacts");
    let mut stdout = Vec::new();

    run_build_with_writer(&build_out, &mut stdout).expect("run build");

    let html = String::from_utf8(stdout).expect("stdout utf-8");
    assert!(html.contains("data-orv-client=\"wasm\""));
    assert!(html.contains("../client/app.js"));
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn run_build_uses_bundle_plan_instead_of_stale_server_launcher() {
    let out = temp_output_dir("run-build-static-stale-server");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(&entry, r#"@out @html { @body { @h1 "Fresh" } }"#).expect("write entry");
    let build_out = out.join("dist");
    cmd_build(&entry, &build_out).expect("build artifacts");
    let stale_launch = build_out.join("server").join("launch.json");
    if let Some(parent) = stale_launch.parent() {
        std::fs::create_dir_all(parent).expect("create stale server dir");
    }
    std::fs::write(&stale_launch, "{ stale").expect("write stale launch");
    let mut stdout = Vec::new();

    run_build_with_writer(&build_out, &mut stdout).expect("run build");

    assert_eq!(
        String::from_utf8(stdout).expect("stdout utf-8"),
        "<html><body><h1>Fresh</h1></body></html>"
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn dev_builds_verifies_and_runs_static_page() {
    let out = temp_output_dir("dev-static");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(&entry, r#"@out @html { @body { @h1 "Dev" } }"#).expect("write entry");
    let build_out = out.join("dist");
    let mut stdout = Vec::new();

    dev_with_writer(&entry, &build_out, &mut stdout).expect("dev");

    assert!(build_out.join("build-manifest.json").is_file());
    assert!(build_out.join("bundle-plan.json").is_file());
    assert_eq!(
        String::from_utf8(stdout).expect("stdout utf-8"),
        "<html><body><h1>Dev</h1></body></html>"
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn dev_hmr_writes_session_manifest_for_client_page() {
    let out = temp_output_dir("dev-hmr-session");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        "let sig count: int = 0\n@out @html { @body { @p count } }",
    )
    .expect("write entry");
    let build_out = out.join("dist");
    let mut stdout = Vec::new();
    let canonical_entry = std::fs::canonicalize(&entry).expect("canonical entry");

    dev_with_writer_with_options(&entry, &build_out, true, false, &mut stdout).expect("dev hmr");

    let session =
        read_json_value(&build_out.join("dev").join("session.json")).expect("dev session");
    assert_eq!(session["schema_version"], 1);
    assert_eq!(session["mode"], "hmr");
    assert_eq!(session["source_bundle"], "source-bundle.json");
    assert_eq!(session["reload"]["strategy"], "hot-reload");
    assert_eq!(session["reload"]["fallback"], "full-reload");
    assert!(session["watch"]["sources"]
        .as_array()
        .expect("watch sources")
        .iter()
        .any(|source| {
            source["path"] == canonical_entry.display().to_string()
                && source["content_hash"]
                    .as_str()
                    .is_some_and(|hash| hash.starts_with("fnv1a64:"))
        }));
    assert!(session["watch"]["targets"]
        .as_array()
        .expect("watch targets")
        .iter()
        .any(|target| {
            target["kind"] == "client_wasm"
                && target["path"] == "client/app.wasm"
                && target["runtime_features"]
                    .as_array()
                    .expect("runtime features")
                    .iter()
                    .any(|feature| feature == "client_wasm")
        }));
    let transport =
        read_json_value(&build_out.join("dev").join("transport.json")).expect("hmr transport");
    assert_eq!(transport["schema_version"], 1);
    assert_eq!(transport["mode"], "hmr-transport");
    assert_eq!(transport["source_bundle"], "source-bundle.json");
    assert_eq!(transport["session"], "dev/session.json");
    assert_eq!(transport["browser"]["kind"], "event-source");
    assert_eq!(transport["browser"]["client"], "dev/hmr-client.js");
    assert_eq!(transport["browser"]["event_source"], "/__orv/hmr/events");
    assert_eq!(transport["browser"]["session"], "/__orv/hmr/session");
    assert_eq!(transport["server"]["kind"], "reference-dev");
    assert_eq!(transport["server"]["events"], "dev/events.json");
    let client =
        std::fs::read_to_string(build_out.join("dev").join("hmr-client.js")).expect("hmr client");
    assert!(client.contains("EventSource('/__orv/hmr/events')"));
    assert!(client.contains("window.location.reload()"));
    cmd_verify_build(&build_out).expect("verify dev hmr build");
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn dev_hmr_reference_server_serves_session_and_event_stream() {
    let out = temp_output_dir("dev-hmr-server");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(
        &entry,
        "let sig count: int = 0\n@out @html { @body { @p count } }",
    )
    .expect("write entry");
    let build_out = out.join("dist");
    let mut stdout = Vec::new();

    dev_with_writer_with_options(&entry, &build_out, true, true, &mut stdout)
        .expect("dev hmr watch");
    write_dev_watch_events(
        &build_out,
        true,
        1,
        &[dev_watch_loop_event(
            1,
            "initial",
            "build-verify-run",
            "ok",
            Some("sig"),
        )],
    )
    .expect("write hmr events");
    let server = spawn_dev_hmr_server(&build_out, 0).expect("spawn hmr server");
    let address = server.addr().to_string();

    let manifest =
        read_json_value(&build_out.join("dev").join("server.json")).expect("server manifest");
    assert_eq!(manifest["schema_version"], 1);
    assert_eq!(manifest["mode"], "hmr-server");
    assert_eq!(manifest["address"], address);
    assert_eq!(manifest["endpoints"]["session"], "/__orv/hmr/session");
    assert_eq!(manifest["endpoints"]["events"], "/__orv/hmr/events");

    let session_response = send_raw_http(&address, "/__orv/hmr/session");
    assert!(session_response.starts_with("HTTP/1.1 200 OK"));
    assert!(session_response.contains("Content-Type: application/json"));
    assert!(session_response.contains("\"mode\": \"hmr\""));

    let events_response = send_raw_http(&address, "/__orv/hmr/events");
    assert!(events_response.starts_with("HTTP/1.1 200 OK"));
    assert!(events_response.contains("Content-Type: text/event-stream"));
    assert!(events_response.contains("event: message"));
    assert!(events_response.contains("event: orv:reload"));
    assert!(events_response.contains("\"action\":\"build-verify-run\""));

    let missing_response = send_raw_http(&address, "/missing");
    assert!(missing_response.starts_with("HTTP/1.1 404 Not Found"));

    cmd_verify_build(&build_out).expect("verify dev hmr server build");
    drop(server);
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn dev_watch_writes_watch_session_manifest() {
    let out = temp_output_dir("dev-watch-session");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(&entry, "@out @html { @body { @h1 \"Watch\" } }").expect("write entry");
    let build_out = out.join("dist");
    let mut stdout = Vec::new();
    let canonical_entry = std::fs::canonicalize(&entry).expect("canonical entry");

    dev_with_writer_with_options(&entry, &build_out, false, true, &mut stdout).expect("dev watch");

    let watch = read_json_value(&build_out.join("dev").join("watch.json")).expect("watch session");
    assert_eq!(watch["schema_version"], 1);
    assert_eq!(watch["mode"], "watch");
    assert_eq!(watch["source_bundle"], "source-bundle.json");
    assert_eq!(watch["loop"]["strategy"], "poll");
    assert_eq!(watch["loop"]["run"], "build-verify-run");
    assert_eq!(watch["reload"]["strategy"], "full-reload");
    assert!(watch["watch"]["sources"]
        .as_array()
        .expect("watch sources")
        .iter()
        .any(|source| {
            source["path"] == canonical_entry.display().to_string()
                && source["content_hash"]
                    .as_str()
                    .is_some_and(|hash| hash.starts_with("fnv1a64:"))
        }));
    assert!(watch["watch"]["targets"]
        .as_array()
        .expect("watch targets")
        .iter()
        .any(|target| target["kind"] == "static_page" && target["path"] == "pages/index.html"));
    cmd_verify_build(&build_out).expect("verify dev watch build");
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn dev_watch_loop_writes_bounded_event_manifest() {
    let out = temp_output_dir("dev-watch-loop");
    std::fs::create_dir_all(&out).expect("create temp root");
    let entry = out.join("page.orv");
    std::fs::write(&entry, "@out @html { @body { @h1 \"Loop\" } }").expect("write entry");
    let build_out = out.join("dist");
    let mut stdout = Vec::new();

    dev_watch_loop_with_writer(&entry, &build_out, false, Some(2), 1, &mut stdout)
        .expect("dev watch loop");

    let events = read_json_value(&build_out.join("dev").join("events.json")).expect("watch events");
    assert_eq!(events["schema_version"], 1);
    assert_eq!(events["mode"], "watch-loop");
    assert_eq!(events["loop"]["strategy"], "poll");
    assert_eq!(events["loop"]["run"], "build-verify-run");
    assert_eq!(events["loop"]["interval_ms"], 1);
    assert_eq!(events["transport"]["path"], "dev/events.json");
    assert_eq!(events["events"][0]["iteration"], 1);
    assert_eq!(events["events"][0]["reason"], "initial");
    assert_eq!(events["events"][0]["action"], "build-verify-run");
    assert_eq!(events["events"][0]["status"], "ok");
    assert!(events["events"][0]["source_signature"]
        .as_str()
        .is_some_and(|signature| !signature.is_empty()));
    assert_eq!(events["events"][1]["iteration"], 2);
    assert_eq!(events["events"][1]["reason"], "unchanged");
    assert_eq!(events["events"][1]["action"], "skip");
    assert_eq!(events["events"][1]["status"], "ok");
    assert!(events["events"][1]["source_signature"].is_null());
    assert_eq!(
        String::from_utf8(stdout).expect("stdout utf-8"),
        "<html><body><h1>Loop</h1></body></html>"
    );
    cmd_verify_build(&build_out).expect("verify dev watch loop build");
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn verify_artifact_accepts_generated_server_runtime_artifact() {
    let path = workspace_path(&["fixtures", "e2e", "hello.orv"]);
    let out = temp_output_dir("verify-artifact");

    cmd_build(&path, &out).expect("build artifacts");
    let artifact = out.join("server").join("app.orv-runtime.json");

    cmd_verify_artifact(&artifact).expect("verify artifact");

    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn check_artifact_rehydrates_generated_server_runtime_artifact() {
    let path = workspace_path(&["fixtures", "e2e", "hello.orv"]);
    let out = temp_output_dir("check-artifact");

    cmd_build(&path, &out).expect("build artifacts");
    let artifact = out.join("server").join("app.orv-runtime.json");

    cmd_check_artifact(&artifact).expect("check artifact");

    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn run_artifact_rehydrates_and_runs_source_bundle() {
    let out = temp_output_dir("run-artifact");
    let artifact = out.join("app.orv-runtime.json");
    write_reference_artifact(&artifact, "artifact.orv", r#"@out "artifact ok""#);
    let mut stdout = Vec::new();

    run_artifact_with_writer(&artifact, &mut stdout).expect("run artifact");

    assert_eq!(
        String::from_utf8(stdout).expect("stdout utf-8"),
        "artifact ok\n"
    );
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn run_artifact_rehydrates_imported_source_bundle() {
    let out = temp_output_dir("run-artifact-import");
    let artifact = out.join("app.orv-runtime.json");
    write_reference_artifact_with_sources(
        &artifact,
        "main.orv",
        [
            (
                "main.orv",
                "import models.user.User\nlet u: User = { name: \"Ada\" }\n@out u.name",
            ),
            ("models/user.orv", "pub struct User { name: string }"),
        ],
    );
    let mut stdout = Vec::new();

    run_artifact_with_writer(&artifact, &mut stdout).expect("run artifact");

    assert_eq!(String::from_utf8(stdout).expect("stdout utf-8"), "Ada\n");
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn run_artifact_rejects_corrupt_source_bundle() {
    let out = temp_output_dir("run-artifact-corrupt");
    let artifact_path = out.join("app.orv-runtime.json");
    write_reference_artifact(&artifact_path, "artifact.orv", r#"@out "artifact ok""#);
    let mut artifact: orv_compiler::ServerRuntimeArtifact =
        serde_json::from_str(&std::fs::read_to_string(&artifact_path).expect("artifact json"))
            .expect("artifact");
    artifact.source_bundle.files[0].source = r#"@out "tampered""#.to_string();
    write_json(
        &artifact_path,
        &serde_json::to_value(artifact).expect("artifact value"),
    )
    .expect("write artifact");
    let mut stdout = Vec::new();

    let err = run_artifact_with_writer(&artifact_path, &mut stdout).expect_err("hash mismatch");

    assert!(err.to_string().contains("content hash mismatch"));
    assert!(stdout.is_empty());
    let _ = std::fs::remove_dir_all(&out);
}

fn write_reference_artifact(path: &Path, entry: &str, source: &str) {
    write_reference_artifact_with_sources(path, entry, [(entry, source)]);
}

fn write_reference_artifact_with_sources<'a>(
    path: &Path,
    entry: &str,
    sources: impl IntoIterator<Item = (&'a str, &'a str)>,
) {
    let manifest = orv_compiler::BuildManifest {
        schema_version: orv_compiler::BUILD_MANIFEST_VERSION,
        entry: entry.to_string(),
        runtime: "reference-interpreter".to_string(),
        artifacts: Vec::new(),
        capabilities: orv_compiler::BuildCapabilities {
            has_server: false,
            server_routes: 0,
            client_wasm: false,
            runtime_features: vec!["console_io".to_string()],
        },
    };
    let origin_map = orv_compiler::OriginMap {
        version: orv_compiler::ORIGIN_MAP_VERSION,
        entries: Vec::new(),
        edges: Vec::new(),
    };
    let artifact = orv_compiler::server_runtime_artifact(&manifest, &origin_map, sources);
    write_json(
        path,
        &serde_json::to_value(artifact).expect("artifact value"),
    )
    .expect("write artifact");
}

#[test]
fn graph_json_for_path_outputs_schema_nodes_and_edges() {
    let path = workspace_path(&["fixtures", "e2e", "hello.orv"]);
    let value = project_graph_json_for_path(&path).expect("graph json");

    assert_eq!(value["schema_version"], 1);
    let nodes = value["nodes"].as_array().expect("nodes array");
    let edges = value["edges"].as_array().expect("edges array");
    assert!(nodes.iter().any(|node| node["kind"] == "file"));
    assert!(nodes.iter().any(|node| node["kind"] == "domain"));
    assert!(edges.iter().any(|edge| edge["kind"] == "contains"));
    assert_eq!(value["stats"]["node_count"], nodes.len());
    assert_eq!(value["stats"]["edge_count"], edges.len());
    assert_eq!(value["stats"]["file_count"], 1);
    assert!(
        value["stats"]["max_semantic_contains_depth"]
            .as_u64()
            .expect("semantic depth")
            >= 2
    );
}

#[test]
fn graph_view_writes_static_html_artifact() {
    let path = workspace_path(&["fixtures", "e2e", "hello.orv"]);
    let out = temp_output_dir("graph-view");
    std::fs::create_dir_all(&out).expect("create temp root");
    let value = project_graph_json_for_path(&path).expect("graph json");

    write_project_graph_view(&out, &value).expect("graph view");

    let graph = read_json_value(&out.join("graph.json")).expect("graph artifact");
    assert_eq!(graph["schema_version"], 1);
    let html = std::fs::read_to_string(out.join("index.html")).expect("graph html");
    assert!(html.contains("ORV Project Graph"));
    assert!(html.contains("data-node-count=\""));
    assert!(html.contains("<svg role=\"img\""));
    assert!(html.contains("graph.json"));
    assert!(html.contains("GET /ping"));
    assert!(html.contains("id=\"graph-search\""));
    assert!(html.contains("id=\"graph-kind-filter\""));
    assert!(html.contains("data-graph-node-row"));
    assert!(html.contains("data-node-kind=\"domain\""));
    assert!(html.contains("filterProjectGraphRows"));
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn graph_json_for_path_includes_semantic_origin_map() {
    let path = workspace_path(&["fixtures", "e2e", "hello.orv"]);
    let value = project_graph_json_for_path(&path).expect("graph json");
    let entries = value["semantic"]["origin_map"]["entries"]
        .as_array()
        .expect("origin entries array");

    assert!(entries
        .iter()
        .any(|entry| entry["kind"] == "route" && entry["name"] == "GET /ping"));
    assert!(entries
        .iter()
        .any(|entry| entry["kind"] == "domain" && entry["name"] == "respond"));
}

#[test]
fn graph_json_links_semantic_origins_to_ast_nodes() {
    let path = workspace_path(&["fixtures", "e2e", "hello.orv"]);
    let value = project_graph_json_for_path(&path).expect("graph json");
    let nodes = value["nodes"].as_array().expect("nodes array");
    let route_node = nodes
        .iter()
        .find(|node| node["kind"] == "domain" && node["name"] == "route")
        .expect("route AST node");
    let route_origin = value["semantic"]["origin_map"]["entries"]
        .as_array()
        .expect("origin entries array")
        .iter()
        .find(|entry| entry["kind"] == "route" && entry["name"] == "GET /ping")
        .expect("route origin");
    let links = value["semantic"]["origin_links"]
        .as_array()
        .expect("origin links array");

    assert!(links.iter().any(|link| {
        link["kind"] == "source_node"
            && link["origin_id"] == route_origin["id"]
            && link["node_id"] == route_node["id"]
    }));
}

#[test]
fn graph_json_includes_semantic_origin_edges() {
    let path = workspace_path(&["fixtures", "e2e", "hello.orv"]);
    let value = project_graph_json_for_path(&path).expect("graph json");
    let entries = value["semantic"]["origin_map"]["entries"]
        .as_array()
        .expect("origin entries array");
    let server = entries
        .iter()
        .find(|entry| entry["kind"] == "domain" && entry["name"] == "server")
        .expect("server origin");
    let route = entries
        .iter()
        .find(|entry| entry["kind"] == "route" && entry["name"] == "GET /ping")
        .expect("route origin");
    let respond = entries
        .iter()
        .find(|entry| entry["kind"] == "domain" && entry["name"] == "respond")
        .expect("respond origin");
    let edges = value["semantic"]["origin_edges"]
        .as_array()
        .expect("origin edges array");

    assert!(edges.iter().any(|edge| {
        edge["kind"] == "contains" && edge["from"] == server["id"] && edge["to"] == route["id"]
    }));
    assert!(edges.iter().any(|edge| {
        edge["kind"] == "contains" && edge["from"] == route["id"] && edge["to"] == respond["id"]
    }));
}

#[test]
fn graph_json_exposes_call_edges_from_origin_map() {
    let path = workspace_path(&["fixtures", "plan", "01-basics.orv"]);
    let value = project_graph_json_for_path(&path).expect("graph json");
    let edges = value["semantic"]["origin_edges"]
        .as_array()
        .expect("origin edges array");

    assert!(edges.iter().any(|edge| edge["kind"] == "calls"));
}

#[test]
fn rendered_diagnostics_use_span_file_source() {
    let files = vec![
        orv_project::SourceFile {
            id: FileId(0),
            path: PathBuf::from("main.orv"),
            source: "import models.user.User\nlet u: User = { name: \"ok\" }\n".to_string(),
        },
        orv_project::SourceFile {
            id: FileId(1),
            path: PathBuf::from("models/user.orv"),
            source: "pub struct User { name: string }\nlet bad: int = \"wrong\"\n".to_string(),
        },
    ];
    let start = u32::try_from(files[1].source.find("\"wrong\"").unwrap()).expect("offset fits u32");
    let len = u32::try_from("\"wrong\"".len()).expect("length fits u32");
    let diag = orv_diagnostics::Diagnostic::error(
        "type mismatch: `bad` annotated as `int` but value has type `string`",
    )
    .with_primary(
        orv_diagnostics::Span::new(
            FileId(1),
            orv_diagnostics::ByteRange::new(start, start + len),
        ),
        "value has type `string`",
    );

    let rendered = render_diagnostics_for_test(&[diag], &files);
    assert!(rendered.contains("models/user.orv"), "{rendered}");
    assert!(rendered.contains("let bad: int = \"wrong\""), "{rendered}");
    assert!(
        !rendered.contains("let u: User = { name: \"ok\" }"),
        "{rendered}"
    );
}

#[test]
fn project_diagnostics_render_imported_file_source() {
    let dir = temp_output_dir("imported-diagnostic-source");
    let models = dir.join("models");
    std::fs::create_dir_all(&models).expect("create models dir");
    let entry = dir.join("main.orv");
    let imported = models.join("user.orv");
    std::fs::write(&entry, "import models.user.User\nlet ok: int = 1\n").expect("write entry");
    std::fs::write(
        &imported,
        "pub struct User { id: int }\nlet bad: int = \"wrong\"\n",
    )
    .expect("write imported");
    let loaded = orv_project::load_project(&entry).expect("load project");
    let resolved = orv_resolve::resolve(&loaded.program);
    let lowered = orv_analyzer::lower_with_diagnostics(&loaded.program, &resolved);
    let mut diagnostics = Vec::new();
    diagnostics.extend(loaded.diagnostics.clone());
    diagnostics.extend(resolved.diagnostics);
    diagnostics.extend(lowered.diagnostics);

    let rendered = render_diagnostics_for_test(&diagnostics, &loaded.files);

    assert!(rendered.contains("models/user.orv"), "{rendered}");
    assert!(rendered.contains("let bad: int = \"wrong\""), "{rendered}");
    assert!(!rendered.contains("let ok: int = 1"), "{rendered}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn project_diagnostics_report_unknown_route_param_source() {
    let dir = temp_output_dir("unknown-route-param-diagnostic-source");
    std::fs::create_dir_all(&dir).expect("create source dir");
    let entry = dir.join("main.orv");
    std::fs::write(
        &entry,
        r#"@server {
  @listen 8080
  @route GET /users/:id {
    @respond 200 { name: @param.name }
  }
}
"#,
    )
    .expect("write entry");
    let loaded = orv_project::load_project(&entry).expect("load project");
    let resolved = orv_resolve::resolve(&loaded.program);
    let lowered = orv_analyzer::lower_with_diagnostics(&loaded.program, &resolved);
    let rendered = render_diagnostics_for_test(&lowered.diagnostics, &loaded.files);

    assert!(
        rendered.contains("unknown route param `name`"),
        "{rendered}"
    );
    assert!(rendered.contains("declared route params: id"), "{rendered}");
    assert!(
        rendered.contains("@respond 200 { name: @param.name }"),
        "{rendered}"
    );
    let _ = std::fs::remove_dir_all(dir);
}
