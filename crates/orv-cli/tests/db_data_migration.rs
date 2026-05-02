use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

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
