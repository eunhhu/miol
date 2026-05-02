use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

struct DapServer {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl Drop for DapServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn temp_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    std::env::temp_dir().join(format!("orv-cli-{name}-{}-{nanos}", std::process::id()))
}

fn free_port() -> u16 {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind free port");
    listener.local_addr().expect("local addr").port()
}

fn start_dap() -> DapServer {
    let mut child = Command::new(env!("CARGO_BIN_EXE_orv"))
        .args(["dap", "serve", "--stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn dap server");
    let stdin = child.stdin.take().expect("dap stdin");
    let stdout = BufReader::new(child.stdout.take().expect("dap stdout"));
    DapServer {
        child,
        stdin,
        stdout,
    }
}

fn dap_response(server: &mut DapServer, request: &serde_json::Value) -> serde_json::Value {
    let request_seq = request["seq"].as_u64().expect("request seq");
    let body = serde_json::to_vec(request).expect("serialize request");
    write!(server.stdin, "Content-Length: {}\r\n\r\n", body.len()).expect("write header");
    server.stdin.write_all(&body).expect("write body");
    server.stdin.flush().expect("flush request");

    loop {
        let frame = read_dap_frame(&mut server.stdout);
        if frame["type"] == "response" && frame["request_seq"] == request_seq {
            return frame;
        }
    }
}

fn read_dap_frame(stdout: &mut BufReader<ChildStdout>) -> serde_json::Value {
    let mut content_length = None;
    loop {
        let mut line = String::new();
        stdout.read_line(&mut line).expect("read DAP header");
        let header = line.trim_end_matches('\n').trim_end_matches('\r');
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            if name.eq_ignore_ascii_case("Content-Length") {
                content_length = Some(value.trim().parse::<usize>().expect("content length"));
            }
        }
    }
    let length = content_length.expect("content length header");
    let mut body = vec![0_u8; length];
    stdout.read_exact(&mut body).expect("read DAP body");
    serde_json::from_slice(&body).expect("parse DAP frame")
}

fn wait_for_http_ok(port: u16) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut last_error = String::new();
    while Instant::now() < deadline {
        match http_get(port) {
            Ok(response) if response.contains("200 OK") && response.contains(r#"{"ok":true}"#) => {
                return response;
            }
            Ok(response) => last_error = response,
            Err(err) => last_error = err,
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("server did not answer /ping: {last_error}");
}

fn wait_for_http_response(port: u16, path: &str, expected_body: &[&str]) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut last_error = String::new();
    while Instant::now() < deadline {
        match http_get_path(port, path) {
            Ok(response)
                if response.contains("200 OK")
                    && expected_body
                        .iter()
                        .all(|expected| response.contains(expected)) =>
            {
                return response;
            }
            Ok(response) => last_error = response,
            Err(err) => last_error = err,
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("server did not answer {path}: {last_error}");
}

fn http_get(port: u16) -> Result<String, String> {
    http_get_path(port, "/ping")
}

fn http_get_path(port: u16, path: &str) -> Result<String, String> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(1)))
        .map_err(|e| e.to_string())?;
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
    )
    .map_err(|e| e.to_string())?;
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|e| e.to_string())?;
    Ok(response)
}

#[test]
fn dap_attach_runtime_continue_serves_http_and_pause_resumes_transport() {
    let dir = temp_dir("dap-async-transport");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let port = free_port();
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        format!(
            r"@server {{
  @listen {port}
  @route GET /ping {{ @respond 200 {{ ok: true }} }}
}}
"
        ),
    )
    .expect("write source");
    let mut dap = start_dap();

    let launch = dap_response(
        &mut dap,
        &serde_json::json!({
            "seq": 1,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
                "attachRuntime": true,
            },
        }),
    );
    assert_eq!(launch["success"], true, "{launch}");
    assert_eq!(
        launch["body"]["runtime"]["async"]["transport"]["state"],
        "detached"
    );

    let continued = dap_response(
        &mut dap,
        &serde_json::json!({
            "seq": 2,
            "type": "request",
            "command": "continue",
            "arguments": { "threadId": 1 },
        }),
    );
    assert_eq!(continued["success"], true, "{continued}");
    wait_for_http_ok(port);

    let pause = dap_response(
        &mut dap,
        &serde_json::json!({
            "seq": 3,
            "type": "request",
            "command": "pause",
            "arguments": { "threadId": 1 },
        }),
    );
    assert_eq!(pause["success"], true, "{pause}");
    let suspended = dap_response(
        &mut dap,
        &serde_json::json!({
            "seq": 4,
            "type": "request",
            "command": "evaluate",
            "arguments": { "expression": "runtimeTransport" },
        }),
    );
    assert_eq!(suspended["success"], true, "{suspended}");
    assert!(
        suspended["body"]["result"]
            .as_str()
            .expect("transport result")
            .starts_with("process suspended pid "),
        "{suspended}"
    );

    let resumed = dap_response(
        &mut dap,
        &serde_json::json!({
            "seq": 5,
            "type": "request",
            "command": "continue",
            "arguments": { "threadId": 1 },
        }),
    );
    assert_eq!(resumed["success"], true, "{resumed}");
    wait_for_http_ok(port);

    let terminated = dap_response(
        &mut dap,
        &serde_json::json!({
            "seq": 6,
            "type": "request",
            "command": "terminate",
            "arguments": {},
        }),
    );
    assert_eq!(terminated["success"], true, "{terminated}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_attach_runtime_in_process_reports_request_frames() {
    let dir = temp_dir("dap-in-process-request-frames");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let port = free_port();
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        format!(
            r"@server {{
  @listen {port}
  @route GET /users/:id {{ @respond 200 {{ id: @param.id, debug: @query.debug }} }}
}}
"
        ),
    )
    .expect("write source");
    let mut dap = start_dap();

    let launch = dap_response(
        &mut dap,
        &serde_json::json!({
            "seq": 21,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
                "attachRuntime": true,
                "attachRuntimeMode": "inProcess",
            },
        }),
    );
    assert_eq!(launch["success"], true, "{launch}");

    let continued = dap_response(
        &mut dap,
        &serde_json::json!({
            "seq": 22,
            "type": "request",
            "command": "continue",
            "arguments": { "threadId": 1 },
        }),
    );
    assert_eq!(continued["success"], true, "{continued}");
    wait_for_http_response(
        port,
        "/users/42?debug=true",
        &["\"id\":\"42\"", "\"debug\":\"true\""],
    );

    let request_count = dap_response(
        &mut dap,
        &serde_json::json!({
            "seq": 23,
            "type": "request",
            "command": "evaluate",
            "arguments": { "expression": "runtimeRequestCount" },
        }),
    );
    assert_eq!(request_count["success"], true, "{request_count}");
    assert_eq!(request_count["body"]["result"], "1");

    let last_request = dap_response(
        &mut dap,
        &serde_json::json!({
            "seq": 24,
            "type": "request",
            "command": "evaluate",
            "arguments": { "expression": "runtimeLastRequest" },
        }),
    );
    assert_eq!(last_request["success"], true, "{last_request}");
    assert_eq!(
        last_request["body"]["result"],
        "GET /users/42 -> 200 route GET /users/:id params id=42 query debug=true"
    );

    let request_frames = dap_response(
        &mut dap,
        &serde_json::json!({
            "seq": 25,
            "type": "request",
            "command": "evaluate",
            "arguments": { "expression": "runtimeRequestFrames" },
        }),
    );
    assert_eq!(request_frames["success"], true, "{request_frames}");
    assert_eq!(
        request_frames["body"]["result"],
        "#1 GET /users/42 -> 200 route GET /users/:id params id=42 query debug=true"
    );

    let terminated = dap_response(
        &mut dap,
        &serde_json::json!({
            "seq": 26,
            "type": "request",
            "command": "terminate",
            "arguments": {},
        }),
    );
    assert_eq!(terminated["success"], true, "{terminated}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn dap_attach_runtime_in_process_serves_http_and_reports_transport() {
    let dir = temp_dir("dap-in-process-transport");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let port = free_port();
    let source = dir.join("app.orv");
    std::fs::write(
        &source,
        format!(
            r"@server {{
  @listen {port}
  @route GET /ping {{ @respond 200 {{ ok: true }} }}
}}
"
        ),
    )
    .expect("write source");
    let mut dap = start_dap();

    let launch = dap_response(
        &mut dap,
        &serde_json::json!({
            "seq": 11,
            "type": "request",
            "command": "launch",
            "arguments": {
                "program": format!("file://{}", source.display()),
                "attachRuntime": true,
                "attachRuntimeMode": "inProcess",
            },
        }),
    );
    assert_eq!(launch["success"], true, "{launch}");
    assert_eq!(
        launch["body"]["runtime"]["async"]["transport"]["kind"],
        "in-process"
    );
    assert_eq!(
        launch["body"]["runtime"]["async"]["transport"]["state"],
        "detached"
    );

    let continued = dap_response(
        &mut dap,
        &serde_json::json!({
            "seq": 12,
            "type": "request",
            "command": "continue",
            "arguments": { "threadId": 1 },
        }),
    );
    assert_eq!(continued["success"], true, "{continued}");
    wait_for_http_ok(port);

    let transport = dap_response(
        &mut dap,
        &serde_json::json!({
            "seq": 13,
            "type": "request",
            "command": "evaluate",
            "arguments": { "expression": "runtimeTransport" },
        }),
    );
    assert_eq!(transport["success"], true, "{transport}");
    assert_eq!(
        transport["body"]["result"],
        format!("in-process running 127.0.0.1:{port}")
    );

    let terminated = dap_response(
        &mut dap,
        &serde_json::json!({
            "seq": 14,
            "type": "request",
            "command": "terminate",
            "arguments": {},
        }),
    );
    assert_eq!(terminated["success"], true, "{terminated}");
    let _ = std::fs::remove_dir_all(dir);
}
