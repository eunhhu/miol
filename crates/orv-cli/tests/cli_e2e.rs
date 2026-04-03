use std::path::PathBuf;
use std::process::Command;
use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("workspace root")
        .to_path_buf()
}

fn fixture_path(relative: &str) -> PathBuf {
    workspace_root().join(relative)
}

fn run_orv(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_orv"))
        .args(args)
        .current_dir(workspace_root())
        .output()
        .expect("orv CLI should run")
}

fn temp_dir(prefix: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should move forward")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("{prefix}-{unique}"));
    fs::create_dir_all(&path).expect("temp dir should be created");
    path
}

#[test]
fn check_hello_fixture_succeeds() {
    let fixture = fixture_path("fixtures/ok/hello.orv");
    let output = run_orv(&["check", fixture.to_str().expect("utf-8 path")]);
    assert!(output.status.success(), "{output:?}");

    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf-8");
    assert!(stdout.contains("check:"));
    assert!(stdout.contains("items"));
    assert!(stdout.contains("scopes"));
}

#[test]
fn dump_hir_counter_fixture_contains_lowered_scopes() {
    let fixture = fixture_path("fixtures/ok/counter.orv");
    let output = run_orv(&["dump", "hir", fixture.to_str().expect("utf-8 path")]);
    assert!(output.status.success(), "{output:?}");

    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf-8");
    assert!(stdout.contains("Define CounterPage scope#1"));
    assert!(stdout.contains("block scope#2"));
    assert!(stdout.contains("count@symbol#1"));
}

#[test]
fn check_unresolved_program_fails_with_diagnostic() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time should move forward")
        .as_nanos();
    let path = std::env::temp_dir().join(format!("orv-cli-e2e-{unique}.orv"));
    fs::write(&path, "function fail() -> missing\n").expect("temp source should be written");

    let output = run_orv(&["check", path.to_str().expect("utf-8 path")]);
    let _ = fs::remove_file(&path);
    assert!(!output.status.success(), "{output:?}");

    let stderr = String::from_utf8(output.stderr).expect("stderr should be utf-8");
    assert!(stderr.contains("error"));
    assert!(stderr.contains("unresolved name `missing`"));
}

#[test]
fn check_server_fixture_succeeds() {
    let fixture = fixture_path("fixtures/ok/server-basic.orv");
    let output = run_orv(&["check", fixture.to_str().expect("utf-8 path")]);
    assert!(output.status.success(), "{output:?}");

    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf-8");
    assert!(stdout.contains("check:"));
    assert!(stdout.contains("ok"));
}

#[test]
fn invalid_html_node_in_server_reports_domain_error() {
    let fixture = fixture_path("fixtures/err/domain-html-in-server.orv");
    let output = run_orv(&["check", fixture.to_str().expect("utf-8 path")]);
    assert!(!output.status.success(), "{output:?}");

    let stderr = String::from_utf8(output.stderr).expect("stderr should be utf-8");
    assert!(stderr.contains("node `@div` is not valid in @server context"));
}

#[test]
fn invalid_route_node_in_html_reports_domain_error() {
    let fixture = fixture_path("fixtures/err/domain-route-in-ui.orv");
    let output = run_orv(&["check", fixture.to_str().expect("utf-8 path")]);
    assert!(!output.status.success(), "{output:?}");

    let stderr = String::from_utf8(output.stderr).expect("stderr should be utf-8");
    assert!(stderr.contains("node `@route` is not valid in @html context"));
}

#[test]
fn run_server_fixture_executes_direct_adapter_path() {
    let fixture = fixture_path("fixtures/ok/server-basic.orv");
    let output = run_orv(&[
        "run",
        fixture.to_str().expect("utf-8 path"),
        "--method",
        "GET",
        "--path",
        "/api/health",
    ]);
    assert!(output.status.success(), "{output:?}");

    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf-8");
    assert!(stdout.contains("adapter: direct-match"));
    assert!(stdout.contains("status: 200"));
    assert!(stdout.contains("content-type: application/json"));
    assert!(stdout.contains(r#"body: {"status":"ok"}"#));
}

#[test]
fn build_server_fixture_emits_native_binary_that_runs() {
    let fixture = fixture_path("fixtures/ok/server-basic.orv");
    let output_dir = temp_dir("orv-build-e2e");
    let output = run_orv(&[
        "build",
        fixture.to_str().expect("utf-8 path"),
        "--output-dir",
        output_dir.to_str().expect("utf-8 path"),
    ]);
    assert!(output.status.success(), "{output:?}");

    let binary = output_dir.join(format!("orv-app{}", std::env::consts::EXE_SUFFIX));
    assert!(
        binary.exists(),
        "binary should exist at {}",
        binary.display()
    );
    assert!(output_dir.join("program.json").exists());
    assert!(output_dir.join("direct_adapter.rs").exists());

    let built = Command::new(&binary)
        .args(["GET", "/api/health"])
        .output()
        .expect("built adapter should run");
    assert!(built.status.success(), "{built:?}");

    let stdout = String::from_utf8(built.stdout).expect("stdout should be utf-8");
    assert!(stdout.contains("adapter: direct-match"));
    assert!(stdout.contains("status: 200"));
    assert!(stdout.contains(r#"body: {"status":"ok"}"#));

    let _ = fs::remove_dir_all(&output_dir);
}
