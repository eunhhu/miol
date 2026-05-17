use std::path::{Path, PathBuf};
use std::time::Duration;

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

use super::{
    current_db_schema_snapshot, db_schema_diff_actions, dependency_cache_component,
    empty_db_schema_snapshot, fnv1a64, lsp_file_uri_for_path, lsp_file_uri_path, parse_http_url,
    read_json_value, rollback_schema_path, stable_json_hash, write_json_atomic,
};

type HmacSha256 = Hmac<Sha256>;
type ArchiveHttpHeaders = Vec<(String, String)>;

const DB_ARCHIVE_S3_ENDPOINT_ENV: &str = "ORV_DB_ARCHIVE_S3_ENDPOINT";
const DB_ARCHIVE_S3_AUTH_ENV: &str = "ORV_DB_ARCHIVE_S3_AUTH";
const DB_ARCHIVE_S3_AUTH_TOKEN_ENV: &str = "ORV_DB_ARCHIVE_S3_AUTH_TOKEN";
const DB_ARCHIVE_S3_AWS_ACCESS_KEY_ENV: &str = "AWS_ACCESS_KEY_ID";
const DB_ARCHIVE_S3_AWS_SECRET_KEY_ENV: &str = "AWS_SECRET_ACCESS_KEY";
const DB_ARCHIVE_S3_AWS_SESSION_TOKEN_ENV: &str = "AWS_SESSION_TOKEN";
const DB_ARCHIVE_S3_AWS_REGION_ENV: &str = "AWS_REGION";
const DB_ARCHIVE_S3_AWS_DEFAULT_REGION_ENV: &str = "AWS_DEFAULT_REGION";
const DB_ARCHIVE_HTTP_RETRY_ATTEMPTS_ENV: &str = "ORV_DB_ARCHIVE_HTTP_RETRY_ATTEMPTS";
const DB_ARCHIVE_HTTP_RETRY_DEFAULT_ATTEMPTS: usize = 3;
const DB_ARCHIVE_HTTP_RETRY_MAX_ATTEMPTS: usize = 8;

pub fn cmd_db_plan(path: &Path, applied: Option<&Path>) -> anyhow::Result<()> {
    let value = db_plan_json(path, applied)?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

pub fn cmd_db_verify(path: &Path, schema: &Path) -> anyhow::Result<()> {
    let plan = db_plan_json(path, Some(schema))?;
    let actions = plan
        .get("actions")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("db plan actions must be an array"))?;
    if !actions.is_empty() {
        anyhow::bail!("db schema drift: {} action(s)", actions.len());
    }
    println!("db schema: {} verified", schema.display());
    Ok(())
}

#[cfg(test)]
pub fn cmd_db_apply(path: &Path, schema: &Path) -> anyhow::Result<()> {
    cmd_db_apply_with_history(path, schema, None)
}

#[cfg(test)]
pub fn cmd_db_migrate(path: &Path, schema: &Path, history: Option<&Path>) -> anyhow::Result<()> {
    cmd_db_apply_with_history(path, schema, history)
}

pub fn cmd_db_migrate_with_data(
    path: &Path,
    schema: &Path,
    history: Option<&Path>,
    data: Option<&Path>,
) -> anyhow::Result<()> {
    cmd_db_apply_with_data(path, schema, history, data)
}

pub fn cmd_db_apply_with_history(
    path: &Path,
    schema: &Path,
    history: Option<&Path>,
) -> anyhow::Result<()> {
    cmd_db_apply_with_data(path, schema, history, None)
}

pub fn cmd_db_apply_with_data(
    path: &Path,
    schema: &Path,
    history: Option<&Path>,
    data: Option<&Path>,
) -> anyhow::Result<()> {
    let snapshot = current_db_schema_snapshot(path)?;
    let previous = if schema.is_file() {
        read_json_value(schema)?
    } else {
        empty_db_schema_snapshot()
    };
    let actions = db_schema_diff_actions(&previous, &snapshot);
    let migrated_data = if let Some(data) = data {
        Some(migrated_db_data_snapshot(data, &actions)?)
    } else {
        None
    };
    backup_schema_for_rollback(schema)?;
    if let Some(data) = data {
        backup_json_for_rollback(data)?;
    }
    write_json_atomic(schema, &snapshot)?;
    if let (Some(data), Some(migrated_data)) = (data, migrated_data.as_ref()) {
        write_json_atomic(data, migrated_data)?;
        println!("db data: {} migrated", data.display());
    }
    if let Some(history) = history {
        append_db_history(history, path, &snapshot, actions)?;
    }
    println!("db schema: {} applied", schema.display());
    Ok(())
}

#[cfg(test)]
pub fn cmd_db_rollback(schema: &Path) -> anyhow::Result<()> {
    cmd_db_rollback_with_data(schema, None)
}

pub fn cmd_db_rollback_with_data(schema: &Path, data: Option<&Path>) -> anyhow::Result<()> {
    let rollback = rollback_schema_path(schema);
    if !rollback.is_file() {
        anyhow::bail!("no rollback schema snapshot at {}", rollback.display());
    }
    let snapshot = read_json_value(&rollback)?;
    let data_snapshot = if let Some(data) = data {
        let rollback = rollback_schema_path(data);
        if !rollback.is_file() {
            anyhow::bail!("no rollback data snapshot at {}", rollback.display());
        }
        let snapshot = read_json_value(&rollback)?;
        Some((data, rollback, snapshot))
    } else {
        None
    };
    write_json_atomic(schema, &snapshot)?;
    std::fs::remove_file(&rollback)
        .map_err(|e| anyhow::anyhow!("failed to remove {}: {e}", rollback.display()))?;
    if let Some((data, rollback, snapshot)) = data_snapshot {
        write_json_atomic(data, &snapshot)?;
        std::fs::remove_file(&rollback)
            .map_err(|e| anyhow::anyhow!("failed to remove {}: {e}", rollback.display()))?;
        println!("db data: {} rolled back", data.display());
    }
    println!("db schema: {} rolled back", schema.display());
    Ok(())
}

fn backup_schema_for_rollback(schema: &Path) -> anyhow::Result<()> {
    backup_json_for_rollback(schema)
}

fn backup_json_for_rollback(path: &Path) -> anyhow::Result<()> {
    if path.is_file() {
        let current = read_json_value(path)?;
        write_json_atomic(&rollback_schema_path(path), &current)?;
    }
    Ok(())
}

pub fn cmd_db_backup(data: &Path, out: &Path) -> anyhow::Result<()> {
    let snapshot = read_json_value(data)?;
    validate_db_data_snapshot(&snapshot)?;
    let backup = serde_json::json!({
        "schema_version": 1,
        "source": data.display().to_string(),
        "data_hash": stable_json_hash(&snapshot)?,
        "data": snapshot,
    });
    write_json_atomic(out, &backup)?;
    println!("db backup: {} written", out.display());
    Ok(())
}

pub fn cmd_db_restore(backup: &Path, data: &Path) -> anyhow::Result<()> {
    let backup = read_json_value(backup)?;
    let version = backup
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow::anyhow!("db backup schema_version must be an integer"))?;
    if version != 1 {
        anyhow::bail!("unsupported db backup schema_version {version}");
    }
    let snapshot = backup
        .get("data")
        .ok_or_else(|| anyhow::anyhow!("db backup data snapshot is missing"))?;
    validate_db_data_snapshot(snapshot)?;
    backup_json_for_rollback(data)?;
    write_json_atomic(data, snapshot)?;
    println!("db data: {} restored", data.display());
    Ok(())
}

pub fn cmd_db_restore_from_inputs(
    backup: Option<&Path>,
    wal: Option<&Path>,
    archive: Option<&Path>,
    at: Option<&str>,
    data: &Path,
) -> anyhow::Result<()> {
    match (backup, wal, archive) {
        (Some(backup), None, None) => {
            if at.is_some() {
                anyhow::bail!("db restore --at requires --wal or --archive");
            }
            cmd_db_restore(backup, data)
        }
        (None, Some(wal), None) => {
            cmd_db_recover_from_inputs(Some(wal), None, data, None, None, at)
        }
        (None, None, Some(archive)) => {
            cmd_db_recover_from_inputs(None, Some(archive), data, None, None, at)
        }
        (None, None, None) => anyhow::bail!("db restore requires --backup, --wal, or --archive"),
        _ => anyhow::bail!("db restore accepts only one of --backup, --wal, or --archive"),
    }
}

pub fn cmd_db_recover(
    wal: &Path,
    out: &Path,
    until_record: Option<usize>,
    until_unix_ms: Option<u64>,
    until_time: Option<&str>,
) -> anyhow::Result<()> {
    let cutoff_count = usize::from(until_record.is_some())
        + usize::from(until_unix_ms.is_some())
        + usize::from(until_time.is_some());
    if cutoff_count > 1 {
        anyhow::bail!(
            "db recover accepts only one of --until-record, --until-unix-ms, or --until-time"
        );
    }
    let until_time_unix_ms = until_time.map(parse_db_recover_time_unix_ms).transpose()?;
    let timestamp_limit = until_unix_ms.or(until_time_unix_ms);
    let db = if timestamp_limit.is_some() {
        orv_runtime::db::InMemoryDb::load_wal_until_unix_ms(wal, timestamp_limit)
    } else {
        orv_runtime::db::InMemoryDb::load_wal_until_record(wal, until_record)
    }
    .map_err(|e| anyhow::anyhow!("db wal recover failed: {e}"))?;
    let snapshot = db.snapshot_json();
    validate_db_data_snapshot(&snapshot)?;
    backup_json_for_rollback(out)?;
    write_json_atomic(out, &snapshot)?;
    match (until_record, until_unix_ms, until_time) {
        (Some(limit), None, None) => println!(
            "db recover: {} written from {} through record {}",
            out.display(),
            wal.display(),
            limit
        ),
        (None, Some(limit), None) => println!(
            "db recover: {} written from {} through unix ms {}",
            out.display(),
            wal.display(),
            limit
        ),
        (None, None, Some(limit)) => println!(
            "db recover: {} written from {} through time {}",
            out.display(),
            wal.display(),
            limit
        ),
        (None, None, None) => println!(
            "db recover: {} written from {}",
            out.display(),
            wal.display()
        ),
        _ => unreachable!("validated mutually exclusive recover limits"),
    }
    Ok(())
}

pub fn cmd_db_recover_from_inputs(
    wal: Option<&Path>,
    archive: Option<&Path>,
    out: &Path,
    until_record: Option<usize>,
    until_unix_ms: Option<u64>,
    until_time: Option<&str>,
) -> anyhow::Result<()> {
    match (wal, archive) {
        (Some(wal), None) => cmd_db_recover(wal, out, until_record, until_unix_ms, until_time),
        (None, Some(archive)) => {
            let wal = db_archive_manifest_wal_path(archive)?;
            cmd_db_recover(&wal, out, until_record, until_unix_ms, until_time)
        }
        (Some(_), Some(_)) => anyhow::bail!("db recover accepts only one of --wal or --archive"),
        (None, None) => anyhow::bail!("db recover requires --wal or --archive"),
    }
}

pub fn cmd_db_archive(wal: &Path, out: &Path, target: Option<&str>) -> anyhow::Result<()> {
    let mut manifest = db_wal_archive_manifest(wal)?;
    let archive_target = target
        .map(|target| db_archive_target(target, wal, out))
        .transpose()?;
    if let Some(target) = &archive_target {
        manifest["target"] = db_archive_target_json(target);
    }
    write_json_atomic(out, &manifest)?;
    if let Some(target) = &archive_target {
        copy_db_archive_to_target(wal, out, target)?;
    }
    println!(
        "db archive: {} written from {}",
        out.display(),
        wal.display()
    );
    Ok(())
}

type DbCrashMatrixCheckFn = fn(&Path) -> anyhow::Result<()>;

pub fn cmd_db_crash_matrix(out: &Path) -> anyhow::Result<()> {
    let scratch = db_crash_matrix_temp_dir()?;
    let checks = [
        (
            "wal_replays_complete_records",
            db_crash_matrix_wal_replays_complete_records as DbCrashMatrixCheckFn,
        ),
        (
            "wal_ignores_torn_final_record",
            db_crash_matrix_wal_ignores_torn_final_record as DbCrashMatrixCheckFn,
        ),
        (
            "wal_rejects_midstream_corruption",
            db_crash_matrix_wal_rejects_midstream_corruption as DbCrashMatrixCheckFn,
        ),
        (
            "checkpoint_replay_restores_snapshot",
            db_crash_matrix_checkpoint_replay_restores_snapshot as DbCrashMatrixCheckFn,
        ),
        (
            "savepoint_rollback_replay_restores_checkpoint",
            db_crash_matrix_savepoint_rollback_replay_restores_checkpoint as DbCrashMatrixCheckFn,
        ),
        (
            "point_in_time_replay_stops_at_timestamp",
            db_crash_matrix_point_in_time_replay_stops_at_timestamp as DbCrashMatrixCheckFn,
        ),
        (
            "archive_hash_mismatch_rejected",
            db_crash_matrix_archive_hash_mismatch_rejected as DbCrashMatrixCheckFn,
        ),
    ];
    let mut results = Vec::new();
    for (name, check) in checks {
        let check_dir = scratch.join(dependency_cache_component(name));
        std::fs::create_dir_all(&check_dir).map_err(|e| {
            anyhow::anyhow!(
                "failed to create crash matrix dir {}: {e}",
                check_dir.display()
            )
        })?;
        results.push(db_crash_matrix_run_check(name, &check_dir, check));
    }
    let passed = results
        .iter()
        .filter(|result| result.get("status").and_then(serde_json::Value::as_str) == Some("passed"))
        .count();
    let failed = results.len().saturating_sub(passed);
    let report = serde_json::json!({
        "schema_version": 1,
        "kind": "orv.db.crash_matrix",
        "status": if failed == 0 { "passed" } else { "failed" },
        "summary": {
            "total": results.len(),
            "passed": passed,
            "failed": failed,
        },
        "checks": results,
    });
    write_json_atomic(out, &report)?;
    let _ = std::fs::remove_dir_all(&scratch);
    if failed > 0 {
        anyhow::bail!(
            "db crash matrix failed: {failed} check(s); report {}",
            out.display()
        );
    }
    println!(
        "db crash matrix: {} written ({} passed)",
        out.display(),
        passed
    );
    Ok(())
}

fn db_crash_matrix_temp_dir() -> anyhow::Result<PathBuf> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| anyhow::anyhow!("system clock before unix epoch: {e}"))?
        .as_nanos();
    Ok(std::env::temp_dir().join(format!(
        "orv-db-crash-matrix-{}-{nanos}",
        std::process::id()
    )))
}

fn db_crash_matrix_run_check(
    name: &str,
    dir: &Path,
    check: DbCrashMatrixCheckFn,
) -> serde_json::Value {
    match check(dir) {
        Ok(()) => serde_json::json!({
            "name": name,
            "status": "passed",
        }),
        Err(err) => serde_json::json!({
            "name": name,
            "status": "failed",
            "error": err.to_string(),
        }),
    }
}

fn db_crash_matrix_wal_replays_complete_records(dir: &Path) -> anyhow::Result<()> {
    let wal = dir.join("db.wal.jsonl");
    let mut db = orv_runtime::db::InMemoryDb::load_wal(&wal)
        .map_err(|e| anyhow::anyhow!("open wal: {e}"))?;
    db.create_logged("users", db_crash_string_fields("name", "Ada"))?;
    db.create_logged("users", db_crash_string_fields("name", "Grace"))?;
    let query = orv_runtime::db::DbQuery::from_equality(&db_crash_string_fields("name", "Ada"));
    let empty_fields: Vec<(String, orv_runtime::Value)> = Vec::new();
    db.update_logged(
        "users",
        &query,
        &db_crash_int_fields("score", 42),
        &empty_fields,
    )?;
    let delete_query =
        orv_runtime::db::DbQuery::from_equality(&db_crash_string_fields("name", "Grace"));
    db.delete_logged("users", &delete_query)?;

    let restored = orv_runtime::db::InMemoryDb::load_wal(&wal)
        .map_err(|e| anyhow::anyhow!("replay wal: {e}"))?;
    db_crash_assert_row(
        &restored,
        "users",
        "name",
        orv_runtime::Value::Str("Ada".to_string()),
    )?;
    db_crash_assert_row(&restored, "users", "score", orv_runtime::Value::Int(42))?;
    db_crash_assert_missing(
        &restored,
        "users",
        "name",
        orv_runtime::Value::Str("Grace".to_string()),
    )
}

fn db_crash_matrix_wal_ignores_torn_final_record(dir: &Path) -> anyhow::Result<()> {
    let wal = dir.join("db.wal.jsonl");
    let mut db = orv_runtime::db::InMemoryDb::load_wal(&wal)
        .map_err(|e| anyhow::anyhow!("open wal: {e}"))?;
    db.create_logged("users", db_crash_string_fields("name", "Ada"))?;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&wal)
        .map_err(|e| anyhow::anyhow!("open torn wal {}: {e}", wal.display()))?;
    std::io::Write::write_all(
        &mut file,
        br#"{"schema_version":1,"op":"create","table":"users","data":{"name":"Eve"#,
    )
    .and_then(|()| file.sync_data())
    .map_err(|e| anyhow::anyhow!("write torn wal {}: {e}", wal.display()))?;

    let restored = orv_runtime::db::InMemoryDb::load_wal(&wal)
        .map_err(|e| anyhow::anyhow!("replay torn wal: {e}"))?;
    db_crash_assert_row(
        &restored,
        "users",
        "name",
        orv_runtime::Value::Str("Ada".to_string()),
    )?;
    db_crash_assert_missing(
        &restored,
        "users",
        "name",
        orv_runtime::Value::Str("Eve".to_string()),
    )
}

fn db_crash_matrix_wal_rejects_midstream_corruption(dir: &Path) -> anyhow::Result<()> {
    let wal = dir.join("db.wal.jsonl");
    std::fs::write(
        &wal,
        concat!(
            "{\"schema_version\":1,\"op\":\"create\",\"table\":\"users\",\"data\":{\"name\":\"Ada\"}}\n",
            "{\"schema_version\":1,\"op\":\"create\",\"table\":\"users\",\"data\":{\"name\":\"Broken\"}\n",
            "{\"schema_version\":1,\"op\":\"create\",\"table\":\"users\",\"data\":{\"name\":\"Grace\"}}\n",
        ),
    )
    .map_err(|e| anyhow::anyhow!("write corrupt wal {}: {e}", wal.display()))?;
    match orv_runtime::db::InMemoryDb::load_wal(&wal) {
        Ok(_) => anyhow::bail!("midstream corrupt WAL replay unexpectedly succeeded"),
        Err(_) => Ok(()),
    }
}

fn db_crash_matrix_checkpoint_replay_restores_snapshot(dir: &Path) -> anyhow::Result<()> {
    let wal = dir.join("db.wal.jsonl");
    let mut db = orv_runtime::db::InMemoryDb::load_wal(&wal)
        .map_err(|e| anyhow::anyhow!("open wal: {e}"))?;
    db.create_logged("users", db_crash_string_fields("name", "Ada"))?;
    db.create_logged("users", db_crash_string_fields("name", "Grace"))?;
    db.checkpoint_wal()
        .map_err(|e| anyhow::anyhow!("checkpoint wal: {e}"))?;

    let mut restored = orv_runtime::db::InMemoryDb::load_wal(&wal)
        .map_err(|e| anyhow::anyhow!("replay checkpoint wal: {e}"))?;
    if db_crash_row_count(&restored, "users") != 2 {
        anyhow::bail!("checkpoint replay did not restore 2 rows");
    }
    let created = restored.create("users", db_crash_string_fields("name", "Cam"));
    db_crash_assert_object_int_field(&created, "id", 3)
}

fn db_crash_matrix_savepoint_rollback_replay_restores_checkpoint(dir: &Path) -> anyhow::Result<()> {
    let wal = dir.join("db.wal.jsonl");
    let mut db = orv_runtime::db::InMemoryDb::load_wal(&wal)
        .map_err(|e| anyhow::anyhow!("open wal: {e}"))?;
    db.create_logged("users", db_crash_string_fields("name", "Ada"))?;
    let savepoint = db.savepoint();
    db.create_logged("users", db_crash_string_fields("name", "Eve"))?;
    db.restore_savepoint(&savepoint)
        .map_err(|e| anyhow::anyhow!("restore savepoint: {e}"))?;

    let restored = orv_runtime::db::InMemoryDb::load_wal(&wal)
        .map_err(|e| anyhow::anyhow!("replay savepoint rollback wal: {e}"))?;
    db_crash_assert_row(
        &restored,
        "users",
        "name",
        orv_runtime::Value::Str("Ada".to_string()),
    )?;
    db_crash_assert_missing(
        &restored,
        "users",
        "name",
        orv_runtime::Value::Str("Eve".to_string()),
    )
}

fn db_crash_matrix_point_in_time_replay_stops_at_timestamp(dir: &Path) -> anyhow::Result<()> {
    let wal = dir.join("db.wal.jsonl");
    std::fs::write(
        &wal,
        concat!(
            "{\"schema_version\":1,\"op\":\"create\",\"ts_unix_ms\":1000,\"table\":\"users\",\"data\":{\"name\":\"Ada\"}}\n",
            "{\"schema_version\":1,\"op\":\"create\",\"ts_unix_ms\":2000,\"table\":\"users\",\"data\":{\"name\":\"Grace\"}}\n",
            "{\"schema_version\":1,\"op\":\"create\",\"ts_unix_ms\":3000,\"table\":\"users\",\"data\":{\"name\":\"Eve\"}}\n",
        ),
    )
    .map_err(|e| anyhow::anyhow!("write PITR wal {}: {e}", wal.display()))?;
    let restored = orv_runtime::db::InMemoryDb::load_wal_until_unix_ms(&wal, Some(2000))
        .map_err(|e| anyhow::anyhow!("replay PITR wal: {e}"))?;
    if db_crash_row_count(&restored, "users") != 2 {
        anyhow::bail!("point-in-time replay did not stop at 2 rows");
    }
    db_crash_assert_missing(
        &restored,
        "users",
        "name",
        orv_runtime::Value::Str("Eve".to_string()),
    )
}

fn db_crash_matrix_archive_hash_mismatch_rejected(dir: &Path) -> anyhow::Result<()> {
    let wal = dir.join("db.wal.jsonl");
    let archive = dir.join("archive.json");
    let mut db = orv_runtime::db::InMemoryDb::load_wal(&wal)
        .map_err(|e| anyhow::anyhow!("open wal: {e}"))?;
    db.create_logged("users", db_crash_string_fields("name", "Ada"))?;
    let manifest = db_wal_archive_manifest(&wal)?;
    write_json_atomic(&archive, &manifest)?;
    let tampered = std::fs::read_to_string(&wal)
        .map_err(|e| anyhow::anyhow!("read WAL {}: {e}", wal.display()))?
        .replace("Ada", "Eve");
    std::fs::write(&wal, tampered)
        .map_err(|e| anyhow::anyhow!("tamper WAL {}: {e}", wal.display()))?;

    match db_archive_manifest_wal_path(&archive) {
        Ok(_) => anyhow::bail!("archive hash mismatch validation unexpectedly succeeded"),
        Err(err) if err.to_string().contains("db archive WAL hash mismatch") => Ok(()),
        Err(err) => Err(anyhow::anyhow!("unexpected archive mismatch error: {err}")),
    }
}

fn db_crash_string_fields(key: &str, value: &str) -> Vec<(String, orv_runtime::Value)> {
    vec![(key.to_string(), orv_runtime::Value::Str(value.to_string()))]
}

fn db_crash_int_fields(key: &str, value: i64) -> Vec<(String, orv_runtime::Value)> {
    vec![(key.to_string(), orv_runtime::Value::Int(value))]
}

fn db_crash_row_count(db: &orv_runtime::db::InMemoryDb, table: &str) -> usize {
    match db.find_all(table, &[]) {
        orv_runtime::Value::Array(rows) => rows.len(),
        _ => 0,
    }
}

fn db_crash_assert_row(
    db: &orv_runtime::db::InMemoryDb,
    table: &str,
    field: &str,
    value: orv_runtime::Value,
) -> anyhow::Result<()> {
    if matches!(
        db.find_one(table, &[(field.to_string(), value.clone())]),
        orv_runtime::Value::Object(_)
    ) {
        Ok(())
    } else {
        anyhow::bail!("missing row {table}.{field}={value}");
    }
}

fn db_crash_assert_missing(
    db: &orv_runtime::db::InMemoryDb,
    table: &str,
    field: &str,
    value: orv_runtime::Value,
) -> anyhow::Result<()> {
    if matches!(
        db.find_one(table, &[(field.to_string(), value.clone())]),
        orv_runtime::Value::Void
    ) {
        Ok(())
    } else {
        anyhow::bail!("unexpected row {table}.{field}={value}");
    }
}

fn db_crash_assert_object_int_field(
    value: &orv_runtime::Value,
    field: &str,
    expected: i64,
) -> anyhow::Result<()> {
    let orv_runtime::Value::Object(fields) = value else {
        anyhow::bail!("expected object value, got {value}");
    };
    if fields.iter().any(|(key, value)| {
        key == field && matches!(value, orv_runtime::Value::Int(n) if *n == expected)
    }) {
        Ok(())
    } else {
        anyhow::bail!("object field {field} did not equal {expected}: {value}");
    }
}

fn db_wal_archive_manifest(wal: &Path) -> anyhow::Result<serde_json::Value> {
    let source = std::fs::read_to_string(wal)
        .map_err(|e| anyhow::anyhow!("failed to read WAL {}: {e}", wal.display()))?;
    let lines = source.lines().collect::<Vec<_>>();
    let has_complete_tail = source.ends_with('\n');
    let mut records = Vec::new();
    let mut first_ts_unix_ms = None;
    let mut last_ts_unix_ms = None;
    for (line_index, line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let record: serde_json::Value = match serde_json::from_str(line) {
            Ok(record) => record,
            Err(source)
                if line_index + 1 == lines.len() && !has_complete_tail && source.is_eof() =>
            {
                break;
            }
            Err(source) => {
                return Err(anyhow::anyhow!(
                    "failed to parse WAL {} line {}: {source}",
                    wal.display(),
                    line_index + 1
                ));
            }
        };
        let record_number = records.len() + 1;
        let timestamp = record.get("ts_unix_ms").and_then(serde_json::Value::as_u64);
        if let Some(timestamp) = timestamp {
            first_ts_unix_ms.get_or_insert(timestamp);
            last_ts_unix_ms = Some(timestamp);
        }
        let mut item = serde_json::Map::new();
        item.insert(
            "record".to_string(),
            serde_json::Value::from(u64::try_from(record_number).unwrap_or(u64::MAX)),
        );
        if let Some(timestamp) = timestamp {
            item.insert("ts_unix_ms".to_string(), serde_json::Value::from(timestamp));
        }
        records.push(serde_json::Value::Object(item));
    }
    Ok(serde_json::json!({
        "schema_version": 1,
        "kind": "orv.db.wal_archive",
        "wal": {
            "path": wal.display().to_string(),
            "hash": format!("fnv1a64:{:016x}", fnv1a64(source.as_bytes())),
            "byte_count": source.len(),
            "record_count": records.len(),
            "first_ts_unix_ms": first_ts_unix_ms,
            "last_ts_unix_ms": last_ts_unix_ms,
        },
        "records": records,
    }))
}

pub fn db_archive_manifest_wal_path(archive: &Path) -> anyhow::Result<PathBuf> {
    let manifest = read_json_value(archive)?;
    if manifest
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
    {
        anyhow::bail!("db archive schema_version must be 1");
    }
    if manifest.get("kind").and_then(serde_json::Value::as_str) != Some("orv.db.wal_archive") {
        anyhow::bail!("db archive kind must be orv.db.wal_archive");
    }
    let target_kind = manifest
        .pointer("/target/kind")
        .and_then(serde_json::Value::as_str);
    let target_wal_path = manifest
        .pointer("/target/wal/path")
        .and_then(serde_json::Value::as_str);
    if let Some(target_path) = target_wal_path {
        if target_kind == Some("file")
            || (target_kind.is_none() && target_path.starts_with("file://"))
        {
            let wal_path = lsp_file_uri_path(target_path)?;
            verify_db_archive_wal(&manifest, &wal_path)?;
            return Ok(wal_path);
        }
    }
    let wal_path = manifest
        .pointer("/wal/path")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("db archive wal.path must be a string"))?;
    let wal_path = db_archive_source_wal_path(archive, wal_path);
    if wal_path.is_file() {
        verify_db_archive_wal(&manifest, &wal_path)?;
        return Ok(wal_path);
    }
    if target_kind == Some("http") {
        if let Some(target_path) = target_wal_path {
            return download_db_archive_wal_to_cache(archive, &manifest, target_path);
        }
    }
    if target_kind == Some("s3") {
        if let Some(target_path) = target_wal_path {
            let url = db_archive_s3_http_url(target_path)?;
            let headers = db_archive_s3_request_headers("GET", &url, &[])?;
            return download_db_archive_wal_to_cache_with_headers(
                archive, &manifest, &url, &headers,
            );
        }
    }
    verify_db_archive_wal(&manifest, &wal_path)?;
    Ok(wal_path)
}

fn db_archive_source_wal_path(archive: &Path, wal_path: &str) -> PathBuf {
    let wal_path = PathBuf::from(wal_path);
    if wal_path.is_absolute() {
        return wal_path;
    }
    if let Some(parent) = archive.parent() {
        parent.join(wal_path)
    } else {
        wal_path
    }
}

fn verify_db_archive_wal(manifest: &serde_json::Value, wal: &Path) -> anyhow::Result<()> {
    let bytes = std::fs::read(wal)
        .map_err(|e| anyhow::anyhow!("failed to read WAL {}: {e}", wal.display()))?;
    verify_db_archive_wal_bytes(manifest, &bytes, &wal.display().to_string())
}

fn verify_db_archive_wal_bytes(
    manifest: &serde_json::Value,
    bytes: &[u8],
    label: &str,
) -> anyhow::Result<()> {
    let expected_hash = manifest
        .pointer("/wal/hash")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("db archive wal.hash must be a string"))?;
    let actual_hash = format!("fnv1a64:{:016x}", fnv1a64(bytes));
    if actual_hash != expected_hash {
        anyhow::bail!("db archive WAL hash mismatch for {label}");
    }
    let expected_bytes = manifest
        .pointer("/wal/byte_count")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow::anyhow!("db archive wal.byte_count must be a number"))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != expected_bytes {
        anyhow::bail!("db archive WAL byte count mismatch for {label}");
    }
    Ok(())
}

fn download_db_archive_wal_to_cache(
    archive: &Path,
    manifest: &serde_json::Value,
    url: &str,
) -> anyhow::Result<PathBuf> {
    download_db_archive_wal_to_cache_with_headers(archive, manifest, url, &[])
}

fn download_db_archive_wal_to_cache_with_headers(
    archive: &Path,
    manifest: &serde_json::Value,
    url: &str,
    headers: &[(String, String)],
) -> anyhow::Result<PathBuf> {
    let bytes = http_get_bytes_with_headers(url, "db archive WAL download", headers)?;
    verify_db_archive_wal_bytes(manifest, &bytes, url)?;
    let cache_path = db_archive_http_cache_path(archive, manifest, url)?;
    if let Some(parent) = cache_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            anyhow::anyhow!("failed to create archive cache {}: {e}", parent.display())
        })?;
    }
    std::fs::write(&cache_path, &bytes).map_err(|e| {
        anyhow::anyhow!(
            "failed to write archive cache WAL {}: {e}",
            cache_path.display()
        )
    })?;
    Ok(cache_path)
}

fn db_archive_http_cache_path(
    archive: &Path,
    manifest: &serde_json::Value,
    url: &str,
) -> anyhow::Result<PathBuf> {
    let hash = manifest
        .pointer("/wal/hash")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("db archive wal.hash must be a string"))?;
    let name = url
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("db.wal.jsonl");
    let file = format!(
        "{}-{}",
        dependency_cache_component(hash),
        dependency_cache_component(name)
    );
    let parent = archive.parent().map_or_else(
        || PathBuf::from(".orv-db-archive-cache"),
        |parent| parent.join(".orv-db-archive-cache"),
    );
    Ok(parent.join(file))
}

struct DbArchiveFileTarget {
    uri: String,
    wal_path: PathBuf,
    manifest_path: PathBuf,
}

struct DbArchiveHttpTarget {
    uri: String,
    wal_url: String,
    manifest_url: String,
}

struct DbArchiveS3Target {
    uri: String,
    bucket: String,
    prefix: String,
    wal_uri: String,
    manifest_uri: String,
    wal_url: String,
    manifest_url: String,
}

enum DbArchiveTarget {
    File(DbArchiveFileTarget),
    Http(DbArchiveHttpTarget),
    S3(DbArchiveS3Target),
}

fn db_archive_target(target: &str, wal: &Path, manifest: &Path) -> anyhow::Result<DbArchiveTarget> {
    if target.starts_with("file://") {
        return db_archive_file_target(target, wal, manifest).map(DbArchiveTarget::File);
    }
    if target.starts_with("http://") {
        return db_archive_http_target(target, wal, manifest).map(DbArchiveTarget::Http);
    }
    if target.starts_with("s3://") {
        return db_archive_s3_target(target, wal, manifest).map(DbArchiveTarget::S3);
    }
    anyhow::bail!("unsupported db archive target `{target}`");
}

fn db_archive_file_target(
    target: &str,
    wal: &Path,
    manifest: &Path,
) -> anyhow::Result<DbArchiveFileTarget> {
    let target_dir = lsp_file_uri_path(target)?;
    let wal_name = wal
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("WAL path must include a file name"))?;
    let manifest_name = manifest
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("archive manifest path must include a file name"))?;
    Ok(DbArchiveFileTarget {
        uri: target.to_string(),
        wal_path: target_dir.join(wal_name),
        manifest_path: target_dir.join(manifest_name),
    })
}

fn db_archive_file_target_json(target: &DbArchiveFileTarget) -> serde_json::Value {
    serde_json::json!({
        "kind": "file",
        "uri": target.uri.clone(),
        "wal": {
            "path": lsp_file_uri_for_path(&target.wal_path),
        },
        "manifest": {
            "path": lsp_file_uri_for_path(&target.manifest_path),
        },
    })
}

fn db_archive_http_target(
    target: &str,
    wal: &Path,
    manifest: &Path,
) -> anyhow::Result<DbArchiveHttpTarget> {
    let _ = parse_http_url(target)?;
    let wal_name = db_archive_target_file_name(wal, "WAL path")?;
    let manifest_name = db_archive_target_file_name(manifest, "archive manifest path")?;
    let base = target.trim_end_matches('/');
    Ok(DbArchiveHttpTarget {
        uri: target.to_string(),
        wal_url: format!("{base}/{wal_name}"),
        manifest_url: format!("{base}/{manifest_name}"),
    })
}

fn db_archive_target_file_name(path: &Path, label: &str) -> anyhow::Result<String> {
    let name = path
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("{label} must include a file name"))?
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("{label} file name must be UTF-8"))?;
    if name.is_empty()
        || name
            .chars()
            .any(|ch| matches!(ch, '/' | '\\' | '?' | '#' | '\r' | '\n'))
    {
        anyhow::bail!("{label} file name contains characters unsupported by archive targets");
    }
    Ok(name.to_string())
}

fn db_archive_http_target_json(target: &DbArchiveHttpTarget) -> serde_json::Value {
    serde_json::json!({
        "kind": "http",
        "uri": target.uri.clone(),
        "retry_attempts_env": DB_ARCHIVE_HTTP_RETRY_ATTEMPTS_ENV,
        "wal": {
            "method": "POST",
            "path": target.wal_url.clone(),
        },
        "manifest": {
            "method": "POST",
            "path": target.manifest_url.clone(),
        },
    })
}

fn db_archive_s3_target(
    target: &str,
    wal: &Path,
    manifest: &Path,
) -> anyhow::Result<DbArchiveS3Target> {
    let location = parse_db_archive_s3_uri(target)?;
    let wal_name = db_archive_target_file_name(wal, "WAL path")?;
    let manifest_name = db_archive_target_file_name(manifest, "archive manifest path")?;
    let wal_uri = db_archive_s3_child_uri(&location, &wal_name);
    let manifest_uri = db_archive_s3_child_uri(&location, &manifest_name);
    Ok(DbArchiveS3Target {
        uri: target.to_string(),
        bucket: location.bucket,
        prefix: location.prefix,
        wal_url: db_archive_s3_http_url(&wal_uri)?,
        manifest_url: db_archive_s3_http_url(&manifest_uri)?,
        wal_uri,
        manifest_uri,
    })
}

fn db_archive_s3_target_json(target: &DbArchiveS3Target) -> serde_json::Value {
    serde_json::json!({
        "kind": "s3",
        "uri": target.uri.clone(),
        "endpoint_env": DB_ARCHIVE_S3_ENDPOINT_ENV,
        "auth_mode_env": DB_ARCHIVE_S3_AUTH_ENV,
        "auth_token_env": DB_ARCHIVE_S3_AUTH_TOKEN_ENV,
        "retry_attempts_env": DB_ARCHIVE_HTTP_RETRY_ATTEMPTS_ENV,
        "aws_sigv4": {
            "access_key_env": DB_ARCHIVE_S3_AWS_ACCESS_KEY_ENV,
            "secret_key_env": DB_ARCHIVE_S3_AWS_SECRET_KEY_ENV,
            "session_token_env": DB_ARCHIVE_S3_AWS_SESSION_TOKEN_ENV,
            "region_env": DB_ARCHIVE_S3_AWS_REGION_ENV,
            "default_region_env": DB_ARCHIVE_S3_AWS_DEFAULT_REGION_ENV,
        },
        "bucket": target.bucket.clone(),
        "prefix": target.prefix.clone(),
        "wal": {
            "method": "PUT",
            "path": target.wal_uri.clone(),
        },
        "manifest": {
            "method": "PUT",
            "path": target.manifest_uri.clone(),
        },
    })
}

fn db_archive_target_json(target: &DbArchiveTarget) -> serde_json::Value {
    match target {
        DbArchiveTarget::File(target) => db_archive_file_target_json(target),
        DbArchiveTarget::Http(target) => db_archive_http_target_json(target),
        DbArchiveTarget::S3(target) => db_archive_s3_target_json(target),
    }
}

fn copy_db_archive_to_target(
    wal: &Path,
    manifest: &Path,
    target: &DbArchiveTarget,
) -> anyhow::Result<()> {
    match target {
        DbArchiveTarget::File(target) => copy_db_archive_to_file_target(wal, manifest, target),
        DbArchiveTarget::Http(target) => upload_db_archive_to_http_target(wal, manifest, target),
        DbArchiveTarget::S3(target) => upload_db_archive_to_s3_target(wal, manifest, target),
    }
}

fn copy_db_archive_to_file_target(
    wal: &Path,
    manifest: &Path,
    target: &DbArchiveFileTarget,
) -> anyhow::Result<()> {
    if let Some(parent) = target.wal_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            anyhow::anyhow!("failed to create archive target {}: {e}", parent.display())
        })?;
    }
    std::fs::copy(wal, &target.wal_path).map_err(|e| {
        anyhow::anyhow!(
            "failed to copy WAL to archive target {}: {e}",
            target.wal_path.display()
        )
    })?;
    std::fs::copy(manifest, &target.manifest_path).map_err(|e| {
        anyhow::anyhow!(
            "failed to copy archive manifest to target {}: {e}",
            target.manifest_path.display()
        )
    })?;
    Ok(())
}

fn upload_db_archive_to_http_target(
    wal: &Path,
    manifest: &Path,
    target: &DbArchiveHttpTarget,
) -> anyhow::Result<()> {
    let wal_body = std::fs::read(wal)
        .map_err(|e| anyhow::anyhow!("failed to read WAL {}: {e}", wal.display()))?;
    http_post_bytes(
        &target.wal_url,
        "application/x-jsonlines; charset=utf-8",
        &wal_body,
        "db archive WAL upload",
    )?;
    let manifest_body = std::fs::read(manifest).map_err(|e| {
        anyhow::anyhow!(
            "failed to read archive manifest {}: {e}",
            manifest.display()
        )
    })?;
    http_post_bytes(
        &target.manifest_url,
        "application/json; charset=utf-8",
        &manifest_body,
        "db archive manifest upload",
    )
}

fn upload_db_archive_to_s3_target(
    wal: &Path,
    manifest: &Path,
    target: &DbArchiveS3Target,
) -> anyhow::Result<()> {
    let wal_body = std::fs::read(wal)
        .map_err(|e| anyhow::anyhow!("failed to read WAL {}: {e}", wal.display()))?;
    let wal_headers = db_archive_s3_request_headers("PUT", &target.wal_url, &wal_body)?;
    http_put_bytes_with_headers(
        &target.wal_url,
        "application/x-jsonlines; charset=utf-8",
        &wal_body,
        "db archive S3 WAL upload",
        &wal_headers,
    )?;
    let manifest_body = std::fs::read(manifest).map_err(|e| {
        anyhow::anyhow!(
            "failed to read archive manifest {}: {e}",
            manifest.display()
        )
    })?;
    let manifest_headers =
        db_archive_s3_request_headers("PUT", &target.manifest_url, &manifest_body)?;
    http_put_bytes_with_headers(
        &target.manifest_url,
        "application/json; charset=utf-8",
        &manifest_body,
        "db archive S3 manifest upload",
        &manifest_headers,
    )
}

struct DbArchiveS3Location {
    bucket: String,
    prefix: String,
}

fn parse_db_archive_s3_uri(uri: &str) -> anyhow::Result<DbArchiveS3Location> {
    let rest = uri
        .strip_prefix("s3://")
        .ok_or_else(|| anyhow::anyhow!("db archive S3 target must start with s3://"))?;
    let (bucket, prefix) = rest.split_once('/').map_or((rest, ""), |(bucket, prefix)| {
        (bucket, prefix.trim_matches('/'))
    });
    if bucket.trim().is_empty() {
        anyhow::bail!("db archive S3 target bucket must not be empty");
    }
    if bucket.contains('?')
        || bucket.contains('#')
        || bucket.contains('\r')
        || bucket.contains('\n')
    {
        anyhow::bail!("db archive S3 target bucket contains unsupported characters");
    }
    if prefix.contains('?')
        || prefix.contains('#')
        || prefix.contains('\r')
        || prefix.contains('\n')
    {
        anyhow::bail!("db archive S3 target prefix contains unsupported characters");
    }
    Ok(DbArchiveS3Location {
        bucket: bucket.to_string(),
        prefix: prefix.to_string(),
    })
}

fn db_archive_s3_child_uri(location: &DbArchiveS3Location, name: &str) -> String {
    if location.prefix.is_empty() {
        format!("s3://{}/{}", location.bucket, name)
    } else {
        format!("s3://{}/{}/{}", location.bucket, location.prefix, name)
    }
}

fn db_archive_s3_http_url(uri: &str) -> anyhow::Result<String> {
    let location = parse_db_archive_s3_uri(uri)?;
    let endpoint = std::env::var(DB_ARCHIVE_S3_ENDPOINT_ENV).map_err(|_| {
        anyhow::anyhow!(
            "db archive S3 target requires {DB_ARCHIVE_S3_ENDPOINT_ENV}=http://host[:port] or https://host"
        )
    })?;
    let endpoint = endpoint.trim_end_matches('/');
    validate_db_archive_s3_endpoint(endpoint)?;
    let mut url = format!("{endpoint}/{}", location.bucket);
    if !location.prefix.is_empty() {
        url.push('/');
        url.push_str(&location.prefix);
    }
    Ok(url)
}

fn validate_db_archive_s3_endpoint(endpoint: &str) -> anyhow::Result<()> {
    if endpoint.contains('\r') || endpoint.contains('\n') {
        anyhow::bail!("db archive S3 endpoint must not contain newlines");
    }
    if endpoint.starts_with("http://") {
        let _ = parse_http_url(endpoint)?;
        return Ok(());
    }
    if let Some(rest) = endpoint.strip_prefix("https://") {
        let authority = rest
            .split_once('/')
            .map_or(rest, |(authority, _)| authority);
        if authority.is_empty() {
            anyhow::bail!("db archive S3 endpoint host must not be empty");
        }
        return Ok(());
    }
    anyhow::bail!(
        "db archive S3 endpoint must start with http:// or https:// via {DB_ARCHIVE_S3_ENDPOINT_ENV}"
    );
}

fn db_archive_s3_request_headers(
    method: &str,
    url: &str,
    body: &[u8],
) -> anyhow::Result<ArchiveHttpHeaders> {
    match std::env::var(DB_ARCHIVE_S3_AUTH_ENV) {
        Ok(mode) if mode == "aws-sigv4" => {
            db_archive_s3_sigv4_headers(method, url, body, db_archive_s3_sigv4_config()?)
        }
        Ok(mode) if mode == "bearer" => db_archive_s3_bearer_headers(true),
        Ok(mode) if mode == "none" => Ok(Vec::new()),
        Ok(mode) => anyhow::bail!(
            "db archive S3 auth mode `{mode}` is unsupported; use `none`, `bearer`, or `aws-sigv4`"
        ),
        Err(std::env::VarError::NotPresent) => db_archive_s3_bearer_headers(false),
        Err(source) => Err(anyhow::anyhow!(
            "db archive S3 auth mode env {DB_ARCHIVE_S3_AUTH_ENV} is invalid: {source}"
        )),
    }
}

fn db_archive_s3_bearer_headers(required: bool) -> anyhow::Result<ArchiveHttpHeaders> {
    let token = match std::env::var(DB_ARCHIVE_S3_AUTH_TOKEN_ENV) {
        Ok(token) => token,
        Err(std::env::VarError::NotPresent) if required => {
            anyhow::bail!(
                "db archive S3 auth mode `bearer` requires {DB_ARCHIVE_S3_AUTH_TOKEN_ENV}"
            );
        }
        Err(std::env::VarError::NotPresent) => return Ok(Vec::new()),
        Err(source) => {
            return Err(anyhow::anyhow!(
                "db archive S3 auth token env {DB_ARCHIVE_S3_AUTH_TOKEN_ENV} is invalid: {source}"
            ));
        }
    };
    if token.trim().is_empty() {
        anyhow::bail!(
            "db archive S3 auth token env {DB_ARCHIVE_S3_AUTH_TOKEN_ENV} must not be empty"
        );
    }
    if token.chars().any(char::is_whitespace) {
        anyhow::bail!(
            "db archive S3 auth token env {DB_ARCHIVE_S3_AUTH_TOKEN_ENV} must not contain whitespace"
        );
    }
    Ok(vec![(
        "Authorization".to_string(),
        format!("Bearer {token}"),
    )])
}

struct AwsSigV4Config {
    access_key: String,
    secret_key: String,
    session_token: Option<String>,
    region: String,
    date_stamp: String,
    amz_datetime: String,
}

fn db_archive_s3_sigv4_config() -> anyhow::Result<AwsSigV4Config> {
    let access_key = required_env_clean(DB_ARCHIVE_S3_AWS_ACCESS_KEY_ENV, "AWS access key")?;
    let secret_key = required_env_clean(DB_ARCHIVE_S3_AWS_SECRET_KEY_ENV, "AWS secret key")?;
    let session_token = optional_env_clean(DB_ARCHIVE_S3_AWS_SESSION_TOKEN_ENV)?;
    let region = match optional_env_clean(DB_ARCHIVE_S3_AWS_REGION_ENV)? {
        Some(region) => Some(region),
        None => optional_env_clean(DB_ARCHIVE_S3_AWS_DEFAULT_REGION_ENV)?,
    };
    let Some(region) = region else {
        anyhow::bail!(
            "db archive S3 aws-sigv4 auth requires {DB_ARCHIVE_S3_AWS_REGION_ENV} or {DB_ARCHIVE_S3_AWS_DEFAULT_REGION_ENV}"
        );
    };
    let (date_stamp, amz_datetime) = aws_sigv4_now()?;
    Ok(AwsSigV4Config {
        access_key,
        secret_key,
        session_token,
        region,
        date_stamp,
        amz_datetime,
    })
}

fn required_env_clean(name: &str, label: &str) -> anyhow::Result<String> {
    let value = std::env::var(name)
        .map_err(|_| anyhow::anyhow!("db archive S3 aws-sigv4 auth requires {name} ({label})"))?;
    validate_env_header_value(name, &value)?;
    Ok(value)
}

fn optional_env_clean(name: &str) -> anyhow::Result<Option<String>> {
    match std::env::var(name) {
        Ok(value) => {
            validate_env_header_value(name, &value)?;
            Ok(Some(value))
        }
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(source) => Err(anyhow::anyhow!(
            "db archive S3 environment variable {name} is invalid: {source}"
        )),
    }
}

fn validate_env_header_value(name: &str, value: &str) -> anyhow::Result<()> {
    if value.trim().is_empty() {
        anyhow::bail!("db archive S3 environment variable {name} must not be empty");
    }
    if value.contains('\r') || value.contains('\n') {
        anyhow::bail!("db archive S3 environment variable {name} must not contain newlines");
    }
    Ok(())
}

fn db_archive_s3_sigv4_headers(
    method: &str,
    url: &str,
    body: &[u8],
    config: AwsSigV4Config,
) -> anyhow::Result<ArchiveHttpHeaders> {
    let (host, canonical_uri) = parse_http_or_https_authority_path(url)?;
    let payload_hash = sha256_hex(body);
    let credential_scope = format!("{}/{}/s3/aws4_request", config.date_stamp, config.region);
    let mut canonical_headers = format!(
        "host:{host}\nx-amz-content-sha256:{payload_hash}\nx-amz-date:{}\n",
        config.amz_datetime
    );
    let mut signed_headers = "host;x-amz-content-sha256;x-amz-date".to_string();
    if let Some(session_token) = &config.session_token {
        canonical_headers.push_str("x-amz-security-token:");
        canonical_headers.push_str(session_token);
        canonical_headers.push('\n');
        signed_headers.push_str(";x-amz-security-token");
    }
    let canonical_request = format!(
        "{method}\n{canonical_uri}\n\n{canonical_headers}\n{signed_headers}\n{payload_hash}"
    );
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{}\n{credential_scope}\n{}",
        config.amz_datetime,
        sha256_hex(canonical_request.as_bytes())
    );
    let signing_key = aws_sigv4_signing_key(&config.secret_key, &config.date_stamp, &config.region);
    let signature = hex_encode(&hmac_sha256(&signing_key, string_to_sign.as_bytes()));
    let mut headers = vec![
        ("x-amz-content-sha256".to_string(), payload_hash),
        ("x-amz-date".to_string(), config.amz_datetime),
    ];
    if let Some(session_token) = config.session_token {
        headers.push(("x-amz-security-token".to_string(), session_token));
    }
    headers.push((
        "Authorization".to_string(),
        format!(
            "AWS4-HMAC-SHA256 Credential={}/{credential_scope}, SignedHeaders={signed_headers}, Signature={signature}",
            config.access_key
        ),
    ));
    Ok(headers)
}

fn parse_http_or_https_authority_path(url: &str) -> anyhow::Result<(String, String)> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .ok_or_else(|| anyhow::anyhow!("S3 signed URL must start with http:// or https://"))?;
    let (authority, path) = rest
        .split_once('/')
        .map_or((rest, "/"), |(authority, path)| {
            (authority, path.strip_prefix('/').unwrap_or(path))
        });
    if authority.is_empty() {
        anyhow::bail!("S3 signed URL host must not be empty");
    }
    if authority.contains('@') {
        anyhow::bail!("S3 signed URL must not include userinfo");
    }
    Ok((
        authority.to_string(),
        format!("/{}", path.trim_start_matches('/')),
    ))
}

fn aws_sigv4_signing_key(secret_key: &str, date_stamp: &str, region: &str) -> Vec<u8> {
    let date_key = hmac_sha256(
        format!("AWS4{secret_key}").as_bytes(),
        date_stamp.as_bytes(),
    );
    let date_region_key = hmac_sha256(&date_key, region.as_bytes());
    let date_region_service_key = hmac_sha256(&date_region_key, b"s3");
    hmac_sha256(&date_region_service_key, b"aws4_request")
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC-SHA256 accepts any key length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    hex_encode(&digest)
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    out
}

fn aws_sigv4_now() -> anyhow::Result<(String, String)> {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| anyhow::anyhow!("system clock before unix epoch: {e}"))?
        .as_secs();
    Ok(aws_sigv4_timestamp_from_unix_secs(seconds))
}

fn aws_sigv4_timestamp_from_unix_secs(seconds: u64) -> (String, String) {
    let days = i64::try_from(seconds / 86_400).unwrap_or(i64::MAX);
    let second_of_day = seconds % 86_400;
    let (year, month, day) = civil_from_unix_days(days);
    let hour = second_of_day / 3_600;
    let minute = (second_of_day % 3_600) / 60;
    let second = second_of_day % 60;
    let date_stamp = format!("{year:04}{month:02}{day:02}");
    let amz_datetime = format!("{date_stamp}T{hour:02}{minute:02}{second:02}Z");
    (date_stamp, amz_datetime)
}

fn civil_from_unix_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    (
        year,
        u32::try_from(month).unwrap_or(u32::MAX),
        u32::try_from(day).unwrap_or(u32::MAX),
    )
}

fn http_post_bytes(
    url: &str,
    content_type: &str,
    body: &[u8],
    context: &str,
) -> anyhow::Result<()> {
    http_send_bytes("POST", url, content_type, body, context)
}

fn http_put_bytes_with_headers(
    url: &str,
    content_type: &str,
    body: &[u8],
    context: &str,
    headers: &[(String, String)],
) -> anyhow::Result<()> {
    http_send_bytes_with_headers("PUT", url, content_type, body, context, headers)
}

fn http_send_bytes(
    method: &str,
    url: &str,
    content_type: &str,
    body: &[u8],
    context: &str,
) -> anyhow::Result<()> {
    http_send_bytes_with_headers(method, url, content_type, body, context, &[])
}

fn http_send_bytes_with_headers(
    method: &str,
    url: &str,
    content_type: &str,
    body: &[u8],
    context: &str,
    headers: &[(String, String)],
) -> anyhow::Result<()> {
    db_archive_http_retry(context, || {
        http_send_bytes_with_headers_once(method, url, content_type, body, context, headers)
    })
}

fn http_send_bytes_with_headers_once(
    method: &str,
    url: &str,
    content_type: &str,
    body: &[u8],
    context: &str,
    headers: &[(String, String)],
) -> anyhow::Result<()> {
    if url.starts_with("https://") {
        return https_send_bytes_with_headers(method, url, content_type, body, context, headers);
    }
    let (host, port, path) = parse_http_url(url)?;
    let mut stream = std::net::TcpStream::connect((host.as_str(), port))
        .map_err(|e| anyhow::anyhow!("{context} failed to connect {host}:{port}: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| anyhow::anyhow!("{context} failed to configure read timeout: {e}"))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| anyhow::anyhow!("{context} failed to configure write timeout: {e}"))?;
    let host_header = if port == 80 {
        host
    } else {
        format!("{host}:{port}")
    };
    let mut request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host_header}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (name, value) in headers {
        request.push_str(name);
        request.push_str(": ");
        request.push_str(value);
        request.push_str("\r\n");
    }
    request.push_str("Connection: close\r\n\r\n");
    std::io::Write::write_all(&mut stream, request.as_bytes())
        .map_err(|e| anyhow::anyhow!("{context} failed to write request {url}: {e}"))?;
    std::io::Write::write_all(&mut stream, body)
        .map_err(|e| anyhow::anyhow!("{context} failed to write body {url}: {e}"))?;
    let mut response = Vec::new();
    std::io::Read::read_to_end(&mut stream, &mut response)
        .map_err(|e| anyhow::anyhow!("{context} failed to read response {url}: {e}"))?;
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| anyhow::anyhow!("{context} response missing HTTP header terminator"))?;
    let headers = std::str::from_utf8(&response[..header_end])
        .map_err(|e| anyhow::anyhow!("{context} response headers are not UTF-8: {e}"))?;
    let status = headers.lines().next().unwrap_or_default();
    if !http_status_is_success(status) {
        let response_body = String::from_utf8_lossy(&response[header_end + 4..]);
        anyhow::bail!("{context} {url} failed with {status}: {response_body}");
    }
    Ok(())
}

fn https_send_bytes_with_headers(
    method: &str,
    url: &str,
    content_type: &str,
    body: &[u8],
    context: &str,
    headers: &[(String, String)],
) -> anyhow::Result<()> {
    let mut request = match method {
        "POST" => ureq::post(url),
        "PUT" => ureq::put(url),
        other => anyhow::bail!("{context} unsupported HTTPS method {other}"),
    }
    .content_type(content_type);
    for (name, value) in headers {
        request = request.header(name.as_str(), value.as_str());
    }
    let mut response = request
        .send(body)
        .map_err(|e| anyhow::anyhow!("{context} {url} failed: {e}"))?;
    response
        .body_mut()
        .read_to_vec()
        .map_err(|e| anyhow::anyhow!("{context} failed to read response {url}: {e}"))?;
    Ok(())
}

fn http_get_bytes_with_headers(
    url: &str,
    context: &str,
    headers: &[(String, String)],
) -> anyhow::Result<Vec<u8>> {
    db_archive_http_retry(context, || {
        http_get_bytes_with_headers_once(url, context, headers)
    })
}

fn http_get_bytes_with_headers_once(
    url: &str,
    context: &str,
    headers: &[(String, String)],
) -> anyhow::Result<Vec<u8>> {
    if url.starts_with("https://") {
        let mut request = ureq::get(url);
        for (name, value) in headers {
            request = request.header(name.as_str(), value.as_str());
        }
        let mut response = request
            .call()
            .map_err(|e| anyhow::anyhow!("{context} {url} failed: {e}"))?;
        return response
            .body_mut()
            .read_to_vec()
            .map_err(|e| anyhow::anyhow!("{context} failed to read response {url}: {e}"));
    }
    let (host, port, path) = parse_http_url(url)?;
    let mut stream = std::net::TcpStream::connect((host.as_str(), port))
        .map_err(|e| anyhow::anyhow!("{context} failed to connect {host}:{port}: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| anyhow::anyhow!("{context} failed to configure read timeout: {e}"))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| anyhow::anyhow!("{context} failed to configure write timeout: {e}"))?;
    let host_header = if port == 80 {
        host
    } else {
        format!("{host}:{port}")
    };
    let mut request = format!("GET {path} HTTP/1.1\r\nHost: {host_header}\r\n");
    for (name, value) in headers {
        request.push_str(name);
        request.push_str(": ");
        request.push_str(value);
        request.push_str("\r\n");
    }
    request.push_str("Connection: close\r\n\r\n");
    std::io::Write::write_all(&mut stream, request.as_bytes())
        .map_err(|e| anyhow::anyhow!("{context} failed to write request {url}: {e}"))?;
    let mut response = Vec::new();
    std::io::Read::read_to_end(&mut stream, &mut response)
        .map_err(|e| anyhow::anyhow!("{context} failed to read response {url}: {e}"))?;
    let header_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| anyhow::anyhow!("{context} response missing HTTP header terminator"))?;
    let headers = std::str::from_utf8(&response[..header_end])
        .map_err(|e| anyhow::anyhow!("{context} response headers are not UTF-8: {e}"))?;
    let status = headers.lines().next().unwrap_or_default();
    if !http_status_is_success(status) {
        let response_body = String::from_utf8_lossy(&response[header_end + 4..]);
        anyhow::bail!("{context} {url} failed with {status}: {response_body}");
    }
    Ok(response[header_end + 4..].to_vec())
}

fn db_archive_http_retry<T>(
    context: &str,
    mut operation: impl FnMut() -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let attempts = db_archive_http_retry_attempts()?;
    let mut last_error = None;
    for attempt in 1..=attempts {
        match operation() {
            Ok(value) => return Ok(value),
            Err(err) if attempt < attempts && db_archive_http_error_is_retryable(&err) => {
                last_error = Some(err);
                std::thread::sleep(db_archive_http_retry_delay(attempt));
            }
            Err(err) => return Err(err),
        }
    }
    Err(last_error
        .unwrap_or_else(|| anyhow::anyhow!("{context} failed without recording an error")))
}

fn db_archive_http_retry_attempts() -> anyhow::Result<usize> {
    match std::env::var(DB_ARCHIVE_HTTP_RETRY_ATTEMPTS_ENV) {
        Ok(value) => {
            let attempts = value.parse::<usize>().map_err(|e| {
                anyhow::anyhow!(
                    "{DB_ARCHIVE_HTTP_RETRY_ATTEMPTS_ENV} must be a positive integer: {e}"
                )
            })?;
            if attempts == 0 || attempts > DB_ARCHIVE_HTTP_RETRY_MAX_ATTEMPTS {
                anyhow::bail!(
                    "{DB_ARCHIVE_HTTP_RETRY_ATTEMPTS_ENV} must be between 1 and {DB_ARCHIVE_HTTP_RETRY_MAX_ATTEMPTS}"
                );
            }
            Ok(attempts)
        }
        Err(std::env::VarError::NotPresent) => Ok(DB_ARCHIVE_HTTP_RETRY_DEFAULT_ATTEMPTS),
        Err(source) => Err(anyhow::anyhow!(
            "{DB_ARCHIVE_HTTP_RETRY_ATTEMPTS_ENV} is invalid: {source}"
        )),
    }
}

fn db_archive_http_retry_delay(attempt: usize) -> Duration {
    Duration::from_millis(u64::try_from(attempt).unwrap_or(u64::MAX).min(5) * 25)
}

fn db_archive_http_error_is_retryable(err: &anyhow::Error) -> bool {
    let message = err.to_string();
    !(message.contains(" failed with HTTP/1.1 4")
        || message.contains(" failed with HTTP/1.0 4")
        || message.contains("unsupported HTTPS method")
        || message.contains("must start with http://")
        || message.contains("must start with http:// or https://"))
}

fn http_status_is_success(status: &str) -> bool {
    status.starts_with("HTTP/1.1 2") || status.starts_with("HTTP/1.0 2")
}

fn parse_db_recover_time_unix_ms(input: &str) -> anyhow::Result<u64> {
    let bytes = input.as_bytes();
    if bytes.len() < 20 {
        anyhow::bail!("--until-time must be an RFC3339 timestamp like 2026-05-02T12:00:00Z");
    }
    expect_time_byte(bytes, 4, b'-')?;
    expect_time_byte(bytes, 7, b'-')?;
    if !matches!(bytes.get(10), Some(b'T' | b't')) {
        anyhow::bail!("--until-time must separate date and time with `T`");
    }
    expect_time_byte(bytes, 13, b':')?;
    expect_time_byte(bytes, 16, b':')?;

    let year = i64::from(parse_time_digits(bytes, 0, 4, "year")?);
    let month = parse_time_digits(bytes, 5, 7, "month")?;
    let day = parse_time_digits(bytes, 8, 10, "day")?;
    let hour = parse_time_digits(bytes, 11, 13, "hour")?;
    let minute = parse_time_digits(bytes, 14, 16, "minute")?;
    let second = parse_time_digits(bytes, 17, 19, "second")?;
    validate_recover_time_parts(year, month, day, hour, minute, second)?;

    let mut index = 19usize;
    let mut millisecond = 0u32;
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        let fraction_start = index;
        while bytes.get(index).is_some_and(|byte| byte.is_ascii_digit()) {
            if index - fraction_start < 3 {
                millisecond = millisecond
                    .saturating_mul(10)
                    .saturating_add(u32::from(bytes[index] - b'0'));
            }
            index += 1;
        }
        let fraction_digits = index.saturating_sub(fraction_start);
        if fraction_digits == 0 {
            anyhow::bail!("--until-time fractional seconds must contain digits");
        }
        for _ in fraction_digits..3 {
            millisecond = millisecond.saturating_mul(10);
        }
    }

    let offset_seconds = parse_recover_time_offset(bytes, index)?;
    let days = days_from_civil(year, month, day);
    let seconds = days
        .checked_mul(86_400)
        .and_then(|value| value.checked_add(i64::from(hour) * 3_600))
        .and_then(|value| value.checked_add(i64::from(minute) * 60))
        .and_then(|value| value.checked_add(i64::from(second)))
        .and_then(|value| value.checked_sub(offset_seconds))
        .ok_or_else(|| anyhow::anyhow!("--until-time is out of supported range"))?;
    let unix_ms = seconds
        .checked_mul(1_000)
        .and_then(|value| value.checked_add(i64::from(millisecond)))
        .ok_or_else(|| anyhow::anyhow!("--until-time is out of supported range"))?;
    if unix_ms < 0 {
        anyhow::bail!("--until-time must not be before the Unix epoch");
    }
    Ok(u64::try_from(unix_ms).unwrap_or(u64::MAX))
}

fn parse_time_digits(bytes: &[u8], start: usize, end: usize, label: &str) -> anyhow::Result<u32> {
    let Some(slice) = bytes.get(start..end) else {
        anyhow::bail!("--until-time is missing {label}");
    };
    let mut value = 0u32;
    for byte in slice {
        if !byte.is_ascii_digit() {
            anyhow::bail!("--until-time has invalid {label}");
        }
        value = value
            .saturating_mul(10)
            .saturating_add(u32::from(byte - b'0'));
    }
    Ok(value)
}

fn expect_time_byte(bytes: &[u8], index: usize, expected: u8) -> anyhow::Result<()> {
    if bytes.get(index) != Some(&expected) {
        anyhow::bail!("--until-time must be an RFC3339 timestamp like 2026-05-02T12:00:00Z");
    }
    Ok(())
}

fn validate_recover_time_parts(
    year: i64,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
) -> anyhow::Result<()> {
    if !(1..=12).contains(&month) {
        anyhow::bail!("--until-time month is out of range");
    }
    if day == 0 || day > days_in_month(year, month) {
        anyhow::bail!("--until-time day is out of range");
    }
    if hour > 23 || minute > 59 || second > 59 {
        anyhow::bail!("--until-time clock is out of range");
    }
    Ok(())
}

fn parse_recover_time_offset(bytes: &[u8], index: usize) -> anyhow::Result<i64> {
    match bytes.get(index) {
        Some(b'Z' | b'z') if index + 1 == bytes.len() => Ok(0),
        Some(sign @ (b'+' | b'-')) => {
            if index + 6 != bytes.len() {
                anyhow::bail!("--until-time timezone offset must use HH:MM");
            }
            expect_time_byte(bytes, index + 3, b':')?;
            let hour = parse_time_digits(bytes, index + 1, index + 3, "timezone hour")?;
            let minute = parse_time_digits(bytes, index + 4, index + 6, "timezone minute")?;
            if hour > 23 || minute > 59 {
                anyhow::bail!("--until-time timezone offset is out of range");
            }
            let offset = i64::from(hour) * 3_600 + i64::from(minute) * 60;
            if *sign == b'+' {
                Ok(offset)
            } else {
                Ok(-offset)
            }
        }
        _ => anyhow::bail!("--until-time must end with `Z` or a timezone offset"),
    }
}

fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

const fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month = i64::from(month);
    let month_prime = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * month_prime + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

pub fn cmd_db_squash(history: &Path, out: &Path) -> anyhow::Result<()> {
    let history_value = read_json_value(history)?;
    let entries = history_value
        .get("entries")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("db history entries must be an array"))?;
    let mut actions = Vec::new();
    for entry in entries {
        let entry_actions = entry
            .get("actions")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| anyhow::anyhow!("db history entry actions must be an array"))?;
        actions.extend(entry_actions.iter().cloned());
    }
    let schema_hash = entries
        .last()
        .and_then(|entry| entry.get("schema_hash"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    let squashed = serde_json::json!({
        "schema_version": 1,
        "source_history": history.display().to_string(),
        "entries": entries.len(),
        "schema_hash": schema_hash,
        "actions": actions,
    });
    write_json_atomic(out, &squashed)?;
    println!("db squash: {} written", out.display());
    Ok(())
}

fn validate_db_data_snapshot(snapshot: &serde_json::Value) -> anyhow::Result<()> {
    snapshot
        .get("tables")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| anyhow::anyhow!("db data snapshot tables must be an object"))?;
    Ok(())
}

fn migrated_db_data_snapshot(
    data: &Path,
    actions: &[serde_json::Value],
) -> anyhow::Result<serde_json::Value> {
    let mut snapshot = read_json_value(data)?;
    validate_db_data_snapshot(&snapshot)?;
    let tables = snapshot
        .get_mut("tables")
        .and_then(serde_json::Value::as_object_mut)
        .expect("validated db data tables");
    for action in actions {
        let Some(kind) = action.get("kind").and_then(serde_json::Value::as_str) else {
            continue;
        };
        match kind {
            "create_struct" => {
                let struct_name = required_action_string(action, "struct")?;
                tables
                    .entry(struct_name.to_string())
                    .or_insert_with(|| serde_json::json!({ "next_id": 1, "rows": [] }));
            }
            "drop_struct" => {
                let struct_name = required_action_string(action, "struct")?;
                tables.remove(struct_name);
            }
            "add_field" => {
                let struct_name = required_action_string(action, "struct")?;
                let field_name = required_action_string(action, "field")?;
                if let Some(rows) = db_data_rows_mut(tables, struct_name)? {
                    for row in rows {
                        let row = row.as_object_mut().ok_or_else(|| {
                            anyhow::anyhow!("db data row in {struct_name} must be an object")
                        })?;
                        row.entry(field_name.to_string())
                            .or_insert(serde_json::Value::Null);
                    }
                }
            }
            "drop_field" => {
                let struct_name = required_action_string(action, "struct")?;
                let field_name = required_action_string(action, "field")?;
                if let Some(rows) = db_data_rows_mut(tables, struct_name)? {
                    for row in rows {
                        let row = row.as_object_mut().ok_or_else(|| {
                            anyhow::anyhow!("db data row in {struct_name} must be an object")
                        })?;
                        row.remove(field_name);
                    }
                }
            }
            "change_field" => {}
            _ => {}
        }
    }
    Ok(snapshot)
}

fn required_action_string<'a>(action: &'a serde_json::Value, key: &str) -> anyhow::Result<&'a str> {
    action
        .get(key)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("db migration action missing string `{key}`"))
}

fn db_data_rows_mut<'a>(
    tables: &'a mut serde_json::Map<String, serde_json::Value>,
    struct_name: &str,
) -> anyhow::Result<Option<&'a mut Vec<serde_json::Value>>> {
    let Some(table) = tables.get_mut(struct_name) else {
        return Ok(None);
    };
    let rows = table
        .get_mut("rows")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or_else(|| anyhow::anyhow!("db data table {struct_name} rows must be an array"))?;
    Ok(Some(rows))
}

fn append_db_history(
    history: &Path,
    source: &Path,
    schema: &serde_json::Value,
    actions: Vec<serde_json::Value>,
) -> anyhow::Result<()> {
    let mut value = if history.is_file() {
        read_json_value(history)?
    } else {
        serde_json::json!({
            "schema_version": 1,
            "entries": [],
        })
    };
    let entries = value
        .get_mut("entries")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or_else(|| anyhow::anyhow!("db history entries must be an array"))?;
    entries.push(serde_json::json!({
        "source": source.display().to_string(),
        "schema_hash": stable_json_hash(schema)?,
        "actions": actions,
    }));
    write_json_atomic(history, &value)
}

pub fn db_plan_json(path: &Path, applied: Option<&Path>) -> anyhow::Result<serde_json::Value> {
    let current_schema = current_db_schema_snapshot(path)?;
    let applied_schema = if let Some(applied) = applied {
        read_json_value(applied)?
    } else {
        empty_db_schema_snapshot()
    };
    let actions = db_schema_diff_actions(&applied_schema, &current_schema);
    Ok(serde_json::json!({
        "schema_version": 1,
        "current_schema": current_schema,
        "actions": actions,
    }))
}
