use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn temp_dir(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    std::env::temp_dir().join(format!("orv-cli-{name}-{}-{nanos}", std::process::id()))
}

fn orv() -> Command {
    Command::new(env!("CARGO_BIN_EXE_orv"))
}

fn read_json(path: &Path) -> serde_json::Value {
    let source = std::fs::read_to_string(path).expect("read json");
    serde_json::from_str(&source).expect("parse json")
}

fn read_http_request(mut stream: TcpStream) -> (String, Vec<u8>) {
    let (_, path, body) = read_http_request_parts(&mut stream);
    write_http_response(&mut stream, "201 Created", "application/json", b"{}");
    (path, body)
}

fn read_http_request_parts(stream: &mut TcpStream) -> (String, String, Vec<u8>) {
    let (method, path, _, body) = read_http_request_parts_with_headers(stream);
    (method, path, body)
}

fn read_http_request_parts_with_headers(
    stream: &mut TcpStream,
) -> (String, String, String, Vec<u8>) {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("set read timeout");
    let mut headers = Vec::new();
    let mut byte = [0_u8; 1];
    while !headers.ends_with(b"\r\n\r\n") {
        stream.read_exact(&mut byte).expect("read request header");
        headers.push(byte[0]);
    }
    let headers = String::from_utf8(headers).expect("request headers utf-8");
    let mut request_parts = headers
        .lines()
        .next()
        .expect("request line")
        .split_whitespace();
    let request_method = request_parts.next().expect("request method").to_string();
    let request_path = request_parts.next().expect("request path").to_string();
    let content_length = headers
        .lines()
        .find_map(|line| line.strip_prefix("Content-Length: "))
        .map_or(0, |value| {
            value.parse::<usize>().expect("content length number")
        });
    let mut body = vec![0_u8; content_length];
    if content_length > 0 {
        stream.read_exact(&mut body).expect("read request body");
    }
    (request_method, request_path, headers, body)
}

fn write_http_response(stream: &mut TcpStream, status: &str, content_type: &str, body: &[u8]) {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .expect("write response headers");
    stream.write_all(body).expect("write response body");
}

fn accept_http_request_until(
    listener: &TcpListener,
    deadline: Instant,
    status: &str,
    content_type: &str,
    response_body: &[u8],
) -> Option<(String, String, String, Vec<u8>)> {
    loop {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let request = read_http_request_parts_with_headers(&mut stream);
                write_http_response(&mut stream, status, content_type, response_body);
                return Some(request);
            }
            Err(err) if err.kind() == ErrorKind::WouldBlock && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(err) if err.kind() == ErrorKind::WouldBlock => return None,
            Err(err) => panic!("accept http request: {err}"),
        }
    }
}

#[test]
fn db_migrate_applies_data_snapshot_and_rollback() {
    let dir = temp_dir("db-data-migrate");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let old_source = dir.join("old.orv");
    std::fs::write(
        &old_source,
        r#"struct User {
  id: int
  email: string
  avatar: string?
}"#,
    )
    .expect("write old source");
    let new_source = dir.join("new.orv");
    std::fs::write(
        &new_source,
        r#"struct User {
  id: int
  email: string
  nickname: string?
}"#,
    )
    .expect("write new source");
    let schema = dir.join("schema.json");
    let history = dir.join("history.json");
    let data = dir.join("data.json");
    std::fs::write(
        &data,
        r#"{
  "schema_version": 1,
  "tables": {
    "User": {
      "next_id": 2,
      "rows": [
        {
          "id": 1,
          "email": "a@example.com",
          "avatar": "old.png"
        }
      ]
    }
  }
}"#,
    )
    .expect("write data");

    let apply = orv()
        .args(["db", "apply"])
        .arg(&old_source)
        .arg("--schema")
        .arg(&schema)
        .output()
        .expect("run db apply");
    assert!(
        apply.status.success(),
        "apply failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&apply.stdout),
        String::from_utf8_lossy(&apply.stderr)
    );

    let migrate = orv()
        .args(["db", "migrate"])
        .arg(&new_source)
        .arg("--schema")
        .arg(&schema)
        .arg("--history")
        .arg(&history)
        .arg("--data")
        .arg(&data)
        .output()
        .expect("run db migrate");
    assert!(
        migrate.status.success(),
        "migrate failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&migrate.stdout),
        String::from_utf8_lossy(&migrate.stderr)
    );

    let migrated = read_json(&data);
    let row = migrated["tables"]["User"]["rows"][0]
        .as_object()
        .expect("migrated row");
    assert_eq!(row.get("email"), Some(&serde_json::json!("a@example.com")));
    assert_eq!(row.get("nickname"), Some(&serde_json::Value::Null));
    assert!(!row.contains_key("avatar"));
    assert!(dir.join("data.json.rollback").is_file());

    let rollback = orv()
        .args(["db", "rollback"])
        .arg("--schema")
        .arg(&schema)
        .arg("--data")
        .arg(&data)
        .output()
        .expect("run db rollback");
    assert!(
        rollback.status.success(),
        "rollback failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&rollback.stdout),
        String::from_utf8_lossy(&rollback.stderr)
    );

    let restored = read_json(&data);
    let row = restored["tables"]["User"]["rows"][0]
        .as_object()
        .expect("restored row");
    assert_eq!(row.get("avatar"), Some(&serde_json::json!("old.png")));
    assert!(!row.contains_key("nickname"));
    assert!(!dir.join("data.json.rollback").exists());

    let restored_schema = read_json(&schema);
    let fields = restored_schema["structs"]["User"]["fields"]
        .as_object()
        .expect("schema fields");
    assert!(fields.contains_key("avatar"));
    assert!(!fields.contains_key("nickname"));

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn db_backup_and_restore_round_trips_data_snapshot() {
    let dir = temp_dir("db-backup-restore");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let data = dir.join("data.json");
    let backup = dir.join("backup.json");
    std::fs::write(
        &data,
        r#"{
  "schema_version": 1,
  "tables": {
    "User": {
      "next_id": 2,
      "rows": [
        {
          "id": 1,
          "email": "a@example.com"
        }
      ]
    }
  }
}"#,
    )
    .expect("write data");

    let backup_output = orv()
        .args(["db", "backup"])
        .arg("--data")
        .arg(&data)
        .arg("--out")
        .arg(&backup)
        .output()
        .expect("run db backup");
    assert!(
        backup_output.status.success(),
        "backup failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&backup_output.stdout),
        String::from_utf8_lossy(&backup_output.stderr)
    );

    let backup_json = read_json(&backup);
    assert_eq!(backup_json["schema_version"], 1);
    assert_eq!(
        backup_json["data"]["tables"]["User"]["rows"][0]["email"],
        "a@example.com"
    );
    assert!(backup_json["data_hash"].as_str().expect("data hash").len() >= 16);

    std::fs::write(
        &data,
        r#"{
  "schema_version": 1,
  "tables": {
    "User": {
      "next_id": 3,
      "rows": [
        {
          "id": 2,
          "email": "changed@example.com"
        }
      ]
    }
  }
}"#,
    )
    .expect("overwrite data");

    let restore_output = orv()
        .args(["db", "restore"])
        .arg("--backup")
        .arg(&backup)
        .arg("--data")
        .arg(&data)
        .output()
        .expect("run db restore");
    assert!(
        restore_output.status.success(),
        "restore failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&restore_output.stdout),
        String::from_utf8_lossy(&restore_output.stderr)
    );

    let restored = read_json(&data);
    assert_eq!(
        restored["tables"]["User"]["rows"][0]["email"],
        "a@example.com"
    );
    let rollback = read_json(&dir.join("data.json.rollback"));
    assert_eq!(
        rollback["tables"]["User"]["rows"][0]["email"],
        "changed@example.com"
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn db_recover_replays_wal_until_record_into_snapshot() {
    let dir = temp_dir("db-recover-wal");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let wal = dir.join("db.wal.jsonl");
    let data = dir.join("data.json");
    std::fs::write(
        &wal,
        r#"{"schema_version":1,"op":"create","table":"User","data":{"email":"a@example.com"}}
{"schema_version":1,"op":"create","table":"User","data":{"email":"b@example.com"}}
{"schema_version":1,"op":"create","table":"User","data":{"email":"c@example.com"}}
"#,
    )
    .expect("write wal");

    let recover = orv()
        .args(["db", "recover"])
        .arg("--wal")
        .arg(&wal)
        .arg("--out")
        .arg(&data)
        .arg("--until-record")
        .arg("2")
        .output()
        .expect("run db recover");
    assert!(
        recover.status.success(),
        "recover failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&recover.stdout),
        String::from_utf8_lossy(&recover.stderr)
    );

    let recovered = read_json(&data);
    let rows = recovered["tables"]["User"]["rows"]
        .as_array()
        .expect("recovered rows");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["email"], "a@example.com");
    assert_eq!(rows[1]["email"], "b@example.com");
    assert_eq!(recovered["tables"]["User"]["next_id"], 2);

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn db_recover_replays_wal_until_unix_ms_into_snapshot() {
    let dir = temp_dir("db-recover-wal-unix-ms");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let wal = dir.join("db.wal.jsonl");
    let data = dir.join("data.json");
    std::fs::write(
        &wal,
        r#"{"schema_version":1,"op":"create","ts_unix_ms":1000,"table":"User","data":{"email":"a@example.com"}}
{"schema_version":1,"op":"create","ts_unix_ms":2000,"table":"User","data":{"email":"b@example.com"}}
{"schema_version":1,"op":"create","ts_unix_ms":3000,"table":"User","data":{"email":"c@example.com"}}
"#,
    )
    .expect("write wal");

    let recover = orv()
        .args(["db", "recover"])
        .arg("--wal")
        .arg(&wal)
        .arg("--out")
        .arg(&data)
        .arg("--until-unix-ms")
        .arg("2000")
        .output()
        .expect("run db recover");
    assert!(
        recover.status.success(),
        "recover failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&recover.stdout),
        String::from_utf8_lossy(&recover.stderr)
    );

    let recovered = read_json(&data);
    let rows = recovered["tables"]["User"]["rows"]
        .as_array()
        .expect("recovered rows");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["email"], "a@example.com");
    assert_eq!(rows[1]["email"], "b@example.com");

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn db_recover_replays_wal_until_iso_time_into_snapshot() {
    let dir = temp_dir("db-recover-wal-iso-time");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let wal = dir.join("db.wal.jsonl");
    let data = dir.join("data.json");
    std::fs::write(
        &wal,
        r#"{"schema_version":1,"op":"create","ts_unix_ms":1000,"table":"User","data":{"email":"a@example.com"}}
{"schema_version":1,"op":"create","ts_unix_ms":2000,"table":"User","data":{"email":"b@example.com"}}
{"schema_version":1,"op":"create","ts_unix_ms":3000,"table":"User","data":{"email":"c@example.com"}}
"#,
    )
    .expect("write wal");

    let recover = orv()
        .args(["db", "recover"])
        .arg("--wal")
        .arg(&wal)
        .arg("--out")
        .arg(&data)
        .arg("--until-time")
        .arg("1970-01-01T00:00:02Z")
        .output()
        .expect("run db recover");
    assert!(
        recover.status.success(),
        "recover failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&recover.stdout),
        String::from_utf8_lossy(&recover.stderr)
    );

    let recovered = read_json(&data);
    let rows = recovered["tables"]["User"]["rows"]
        .as_array()
        .expect("recovered rows");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["email"], "a@example.com");
    assert_eq!(rows[1]["email"], "b@example.com");

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn db_recover_rejects_multiple_cutoffs() {
    let dir = temp_dir("db-recover-cutoff-conflict");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let wal = dir.join("db.wal.jsonl");
    let data = dir.join("data.json");
    std::fs::write(
        &wal,
        r#"{"schema_version":1,"op":"create","ts_unix_ms":1000,"table":"User","data":{"email":"a@example.com"}}
"#,
    )
    .expect("write wal");

    let recover = orv()
        .args(["db", "recover"])
        .arg("--wal")
        .arg(&wal)
        .arg("--out")
        .arg(&data)
        .arg("--until-record")
        .arg("1")
        .arg("--until-unix-ms")
        .arg("1000")
        .arg("--until-time")
        .arg("1970-01-01T00:00:01Z")
        .output()
        .expect("run db recover");

    assert!(
        !recover.status.success(),
        "recover unexpectedly succeeded\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&recover.stdout),
        String::from_utf8_lossy(&recover.stderr)
    );
    assert!(String::from_utf8_lossy(&recover.stderr).contains(
        "db recover accepts only one of --until-record, --until-unix-ms, or --until-time"
    ));
    assert!(!data.exists());

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn db_archive_writes_wal_manifest() {
    let dir = temp_dir("db-archive-wal");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let wal = dir.join("db.wal.jsonl");
    let archive = dir.join("archive.json");
    std::fs::write(
        &wal,
        r#"{"schema_version":1,"op":"create","ts_unix_ms":1000,"table":"User","data":{"email":"a@example.com"}}
{"schema_version":1,"op":"create","ts_unix_ms":2000,"table":"User","data":{"email":"b@example.com"}}
"#,
    )
    .expect("write wal");

    let output = orv()
        .args(["db", "archive"])
        .arg("--wal")
        .arg(&wal)
        .arg("--out")
        .arg(&archive)
        .output()
        .expect("run db archive");

    assert!(
        output.status.success(),
        "archive failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let manifest = read_json(&archive);
    assert_eq!(manifest["schema_version"], 1);
    assert_eq!(manifest["kind"], "orv.db.wal_archive");
    assert_eq!(manifest["wal"]["path"], wal.display().to_string());
    assert_eq!(manifest["wal"]["record_count"], 2);
    assert_eq!(manifest["wal"]["first_ts_unix_ms"], 1000);
    assert_eq!(manifest["wal"]["last_ts_unix_ms"], 2000);
    assert!(manifest["wal"]["hash"]
        .as_str()
        .is_some_and(|hash| hash.starts_with("fnv1a64:")));
    assert_eq!(manifest["records"][0]["record"], 1);
    assert_eq!(manifest["records"][0]["ts_unix_ms"], 1000);
    assert_eq!(manifest["records"][1]["record"], 2);
    assert_eq!(manifest["records"][1]["ts_unix_ms"], 2000);

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn db_archive_file_target_copies_wal_and_manifest() {
    let dir = temp_dir("db-archive-file-target");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let wal = dir.join("db.wal.jsonl");
    let archive = dir.join("archive.json");
    let target = dir.join("archive-target");
    std::fs::write(
        &wal,
        r#"{"schema_version":1,"op":"create","ts_unix_ms":1000,"table":"User","data":{"email":"a@example.com"}}
"#,
    )
    .expect("write wal");

    let output = orv()
        .args(["db", "archive"])
        .arg("--wal")
        .arg(&wal)
        .arg("--out")
        .arg(&archive)
        .arg("--target")
        .arg(format!("file://{}", target.display()))
        .output()
        .expect("run db archive");

    assert!(
        output.status.success(),
        "archive failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let uploaded_wal = target.join("db.wal.jsonl");
    let uploaded_manifest = target.join("archive.json");
    assert_eq!(
        std::fs::read_to_string(&uploaded_wal).expect("uploaded wal"),
        std::fs::read_to_string(&wal).expect("source wal")
    );
    let manifest = read_json(&archive);
    assert_eq!(manifest["target"]["kind"], "file");
    assert_eq!(
        manifest["target"]["uri"],
        format!("file://{}", target.display())
    );
    assert_eq!(
        manifest["target"]["wal"]["path"],
        format!("file://{}", uploaded_wal.display())
    );
    assert_eq!(
        manifest["target"]["manifest"]["path"],
        format!("file://{}", uploaded_manifest.display())
    );
    let uploaded = read_json(&uploaded_manifest);
    assert_eq!(uploaded["target"], manifest["target"]);

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn db_archive_http_target_uploads_wal_and_manifest() {
    let dir = temp_dir("db-archive-http-target");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let wal = dir.join("db.wal.jsonl");
    let archive = dir.join("archive.json");
    std::fs::write(
        &wal,
        r#"{"schema_version":1,"op":"create","ts_unix_ms":1000,"table":"User","data":{"email":"a@example.com"}}
"#,
    )
    .expect("write wal");
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind archive target");
    let address = listener.local_addr().expect("archive target address");
    let server = std::thread::spawn(move || {
        let mut requests = Vec::new();
        for _ in 0..2 {
            let (stream, _) = listener.accept().expect("accept archive upload");
            requests.push(read_http_request(stream));
        }
        requests
    });

    let target = format!("http://{address}/archive");
    let output = orv()
        .args(["db", "archive"])
        .arg("--wal")
        .arg(&wal)
        .arg("--out")
        .arg(&archive)
        .arg("--target")
        .arg(&target)
        .output()
        .expect("run db archive");

    assert!(
        output.status.success(),
        "archive failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let requests = server.join().expect("archive server finished");
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].0, "/archive/db.wal.jsonl");
    assert_eq!(
        String::from_utf8(requests[0].1.clone()).expect("uploaded wal utf-8"),
        std::fs::read_to_string(&wal).expect("source wal")
    );
    assert_eq!(requests[1].0, "/archive/archive.json");
    let manifest = read_json(&archive);
    assert_eq!(manifest["target"]["kind"], "http");
    assert_eq!(manifest["target"]["uri"], target);
    assert_eq!(
        manifest["target"]["wal"]["path"],
        format!("{target}/db.wal.jsonl")
    );
    assert_eq!(
        manifest["target"]["manifest"]["path"],
        format!("{target}/archive.json")
    );
    let uploaded_manifest: serde_json::Value =
        serde_json::from_slice(&requests[1].1).expect("uploaded manifest json");
    assert_eq!(uploaded_manifest["target"], manifest["target"]);

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn db_restore_archive_http_target_downloads_wal_when_source_is_missing() {
    let dir = temp_dir("db-restore-http-archive-target");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let wal = dir.join("db.wal.jsonl");
    let archive = dir.join("archive.json");
    let data = dir.join("data.json");
    let wal_body = concat!(
        "{\"schema_version\":1,\"op\":\"create\",\"ts_unix_ms\":1000,\"table\":\"User\",\"data\":{\"email\":\"a@example.com\"}}\n",
        "{\"schema_version\":1,\"op\":\"create\",\"ts_unix_ms\":2000,\"table\":\"User\",\"data\":{\"email\":\"b@example.com\"}}\n",
    );
    std::fs::write(&wal, wal_body).expect("write wal");
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind archive target");
    let address = listener.local_addr().expect("archive target address");
    let server_wal_body = wal_body.as_bytes().to_vec();
    let server = std::thread::spawn(move || {
        let (wal_upload, _) = listener.accept().expect("accept wal upload");
        let wal_upload = read_http_request(wal_upload);

        let (manifest_upload, _) = listener.accept().expect("accept manifest upload");
        let manifest_upload = read_http_request(manifest_upload);

        let (mut wal_download, _) = listener.accept().expect("accept wal download");
        let (method, path, body) = read_http_request_parts(&mut wal_download);
        assert_eq!(method, "GET");
        assert_eq!(path, "/archive/db.wal.jsonl");
        assert!(body.is_empty());
        write_http_response(
            &mut wal_download,
            "200 OK",
            "application/x-jsonlines",
            &server_wal_body,
        );
        (wal_upload, manifest_upload)
    });

    let target = format!("http://{address}/archive");
    let archive_output = orv()
        .args(["db", "archive"])
        .arg("--wal")
        .arg(&wal)
        .arg("--out")
        .arg(&archive)
        .arg("--target")
        .arg(&target)
        .output()
        .expect("run db archive");
    assert!(
        archive_output.status.success(),
        "archive failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&archive_output.stdout),
        String::from_utf8_lossy(&archive_output.stderr)
    );
    std::fs::remove_file(&wal).expect("remove source wal");

    let restore = orv()
        .args(["db", "restore"])
        .arg("--archive")
        .arg(&archive)
        .arg("--data")
        .arg(&data)
        .output()
        .expect("run db restore");

    assert!(
        restore.status.success(),
        "restore failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&restore.stdout),
        String::from_utf8_lossy(&restore.stderr)
    );
    let (wal_upload, manifest_upload) = server.join().expect("archive server finished");
    assert_eq!(wal_upload.0, "/archive/db.wal.jsonl");
    assert_eq!(
        String::from_utf8(wal_upload.1).expect("uploaded wal utf-8"),
        wal_body
    );
    assert_eq!(manifest_upload.0, "/archive/archive.json");
    let uploaded_manifest: serde_json::Value =
        serde_json::from_slice(&manifest_upload.1).expect("uploaded manifest json");
    assert_eq!(uploaded_manifest["target"]["kind"], "http");
    let restored = read_json(&data);
    let rows = restored["tables"]["User"]["rows"]
        .as_array()
        .expect("restored rows");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["email"], "a@example.com");
    assert_eq!(rows[1]["email"], "b@example.com");

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
#[allow(clippy::too_many_lines)]
fn db_archive_s3_target_uploads_and_restores_wal() {
    let dir = temp_dir("db-archive-s3-target");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let wal = dir.join("db.wal.jsonl");
    let archive = dir.join("archive.json");
    let data = dir.join("data.json");
    let wal_body = concat!(
        "{\"schema_version\":1,\"op\":\"create\",\"ts_unix_ms\":1000,\"table\":\"User\",\"data\":{\"email\":\"a@example.com\"}}\n",
        "{\"schema_version\":1,\"op\":\"create\",\"ts_unix_ms\":2000,\"table\":\"User\",\"data\":{\"email\":\"b@example.com\"}}\n",
    );
    std::fs::write(&wal, wal_body).expect("write wal");
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind s3 archive endpoint");
    listener
        .set_nonblocking(true)
        .expect("set nonblocking listener");
    let address = listener.local_addr().expect("s3 archive endpoint address");
    let server_wal_body = wal_body.as_bytes().to_vec();
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let wal_upload =
            accept_http_request_until(&listener, deadline, "200 OK", "application/json", b"{}")
                .expect("s3 wal upload");
        let manifest_upload =
            accept_http_request_until(&listener, deadline, "200 OK", "application/json", b"{}")
                .expect("s3 manifest upload");
        let wal_download = accept_http_request_until(
            &listener,
            deadline,
            "200 OK",
            "application/x-jsonlines",
            &server_wal_body,
        )
        .expect("s3 wal download");
        (wal_upload, manifest_upload, wal_download)
    });

    let endpoint = format!("http://{address}");
    let archive_output = orv()
        .args(["db", "archive"])
        .arg("--wal")
        .arg(&wal)
        .arg("--out")
        .arg(&archive)
        .arg("--target")
        .arg("s3://orv-backups/shop")
        .env("ORV_DB_ARCHIVE_S3_ENDPOINT", &endpoint)
        .env("ORV_DB_ARCHIVE_S3_AUTH_TOKEN", "orv-s3-test-token")
        .output()
        .expect("run db archive");
    std::fs::remove_file(&wal).expect("remove source wal");
    let restore = orv()
        .args(["db", "restore"])
        .arg("--archive")
        .arg(&archive)
        .arg("--data")
        .arg(&data)
        .env("ORV_DB_ARCHIVE_S3_ENDPOINT", &endpoint)
        .env("ORV_DB_ARCHIVE_S3_AUTH_TOKEN", "orv-s3-test-token")
        .output()
        .expect("run db restore");
    let (wal_upload, manifest_upload, wal_download) = server.join().expect("s3 server finished");

    assert!(
        archive_output.status.success(),
        "archive failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&archive_output.stdout),
        String::from_utf8_lossy(&archive_output.stderr)
    );
    assert!(
        restore.status.success(),
        "restore failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&restore.stdout),
        String::from_utf8_lossy(&restore.stderr)
    );
    assert_eq!(wal_upload.0, "PUT");
    assert_eq!(wal_upload.1, "/orv-backups/shop/db.wal.jsonl");
    assert!(
        wal_upload
            .2
            .lines()
            .any(|line| line == "Authorization: Bearer orv-s3-test-token"),
        "{}",
        wal_upload.2
    );
    assert_eq!(
        String::from_utf8(wal_upload.3).expect("uploaded wal utf-8"),
        wal_body
    );
    assert_eq!(manifest_upload.0, "PUT");
    assert_eq!(manifest_upload.1, "/orv-backups/shop/archive.json");
    assert!(
        manifest_upload
            .2
            .lines()
            .any(|line| line == "Authorization: Bearer orv-s3-test-token"),
        "{}",
        manifest_upload.2
    );
    let uploaded_manifest: serde_json::Value =
        serde_json::from_slice(&manifest_upload.3).expect("uploaded manifest json");
    assert_eq!(uploaded_manifest["target"]["kind"], "s3");
    assert_eq!(
        uploaded_manifest["target"]["auth_token_env"],
        "ORV_DB_ARCHIVE_S3_AUTH_TOKEN"
    );
    assert_eq!(
        uploaded_manifest["target"]["wal"]["path"],
        "s3://orv-backups/shop/db.wal.jsonl"
    );
    assert_eq!(wal_download.0, "GET");
    assert_eq!(wal_download.1, "/orv-backups/shop/db.wal.jsonl");
    assert!(
        wal_download
            .2
            .lines()
            .any(|line| line == "Authorization: Bearer orv-s3-test-token"),
        "{}",
        wal_download.2
    );
    let restored = read_json(&data);
    let rows = restored["tables"]["User"]["rows"]
        .as_array()
        .expect("restored rows");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["email"], "a@example.com");
    assert_eq!(rows[1]["email"], "b@example.com");

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn db_restore_archive_at_recovers_point_in_time_snapshot() {
    let dir = temp_dir("db-restore-archive-at");
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let wal = dir.join("db.wal.jsonl");
    let archive = dir.join("archive.json");
    let data = dir.join("data.json");
    let target = dir.join("archive-target");
    std::fs::write(
        &wal,
        r#"{"schema_version":1,"op":"create","ts_unix_ms":1000,"table":"User","data":{"email":"a@example.com"}}
{"schema_version":1,"op":"create","ts_unix_ms":2000,"table":"User","data":{"email":"b@example.com"}}
{"schema_version":1,"op":"create","ts_unix_ms":3000,"table":"User","data":{"email":"c@example.com"}}
"#,
    )
    .expect("write wal");

    let archive_output = orv()
        .args(["db", "archive"])
        .arg("--wal")
        .arg(&wal)
        .arg("--out")
        .arg(&archive)
        .arg("--target")
        .arg(format!("file://{}", target.display()))
        .output()
        .expect("run db archive");
    assert!(
        archive_output.status.success(),
        "archive failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&archive_output.stdout),
        String::from_utf8_lossy(&archive_output.stderr)
    );
    std::fs::remove_file(&wal).expect("remove source wal");

    let restore = orv()
        .args(["db", "restore"])
        .arg("--archive")
        .arg(&archive)
        .arg("--data")
        .arg(&data)
        .arg("--at")
        .arg("1970-01-01T00:00:02Z")
        .output()
        .expect("run db restore");
    assert!(
        restore.status.success(),
        "restore failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&restore.stdout),
        String::from_utf8_lossy(&restore.stderr)
    );

    let restored = read_json(&data);
    let rows = restored["tables"]["User"]["rows"]
        .as_array()
        .expect("restored rows");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["email"], "a@example.com");
    assert_eq!(rows[1]["email"], "b@example.com");
    assert!(!rows.iter().any(|row| row["email"] == "c@example.com"));

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn db_restore_archive_resolves_relative_wal_path_from_manifest_dir() {
    let dir = temp_dir("db-restore-archive-relative-wal");
    let caller = temp_dir("db-restore-archive-relative-caller");
    std::fs::create_dir_all(&dir).expect("create archive dir");
    std::fs::create_dir_all(&caller).expect("create caller dir");
    let wal = dir.join("db.wal.jsonl");
    let archive = dir.join("archive.json");
    let data = caller.join("data.json");
    std::fs::write(
        &wal,
        r#"{"schema_version":1,"op":"create","ts_unix_ms":1000,"table":"User","data":{"email":"a@example.com"}}
"#,
    )
    .expect("write wal");

    let archive_output = orv()
        .current_dir(&dir)
        .args(["db", "archive"])
        .arg("--wal")
        .arg("db.wal.jsonl")
        .arg("--out")
        .arg("archive.json")
        .output()
        .expect("run db archive");
    assert!(
        archive_output.status.success(),
        "archive failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&archive_output.stdout),
        String::from_utf8_lossy(&archive_output.stderr)
    );

    let restore = orv()
        .current_dir(&caller)
        .args(["db", "restore"])
        .arg("--archive")
        .arg(&archive)
        .arg("--data")
        .arg(&data)
        .output()
        .expect("run db restore");
    assert!(
        restore.status.success(),
        "restore failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&restore.stdout),
        String::from_utf8_lossy(&restore.stderr)
    );

    let restored = read_json(&data);
    assert_eq!(
        restored["tables"]["User"]["rows"][0]["email"],
        "a@example.com"
    );

    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(caller);
}
