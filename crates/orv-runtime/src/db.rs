//! `C_db` MVP — in-memory 테이블 + CRUD/query helpers + JSON snapshot/WAL persistence.
//!
//! # 범위
//! - 테이블 = `String → Vec<Value::Object>` 맵. row 는 자동 `id` 필드를 받는다.
//! - `create/find/update/delete` 와 equality/range/contains filter.
//! - 명시적 JSON snapshot save/load 와 JSONL WAL replay/checkpoint.
//!
//! # 범위 밖
//! - 인덱스 (linear scan).
//! - 트랜잭션/savepoint — WAL 은 단일 파일 append+fsync replay/checkpoint v1 만 지원한다.
//! - 외부 DB query planner.
//! - 마이그레이션/스키마 diff, 외부 DB 어댑터.
//! - async/await — 호출 측이 `await` 를 쓰더라도 현재 인터프리터는 sync.

use std::collections::{BTreeMap, HashMap};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::interp::Value;

/// Current persistent DB snapshot schema version.
pub const DB_SNAPSHOT_SCHEMA_VERSION: u32 = 1;

/// Current persistent DB WAL record schema version.
pub const DB_WAL_SCHEMA_VERSION: u32 = 1;

/// DB filter operator.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DbFilterOp {
    /// `field = value`.
    Eq,
    /// `field != value`.
    Ne,
    /// `field > value`.
    Gt,
    /// `field >= value`.
    Ge,
    /// `field < value`.
    Lt,
    /// `field <= value`.
    Le,
    /// string contains / array contains.
    Contains,
    /// value is in array.
    In,
}

/// One DB filter.
#[derive(Clone, Debug)]
pub struct DbFilter {
    /// Field name.
    pub field: String,
    /// Operator.
    pub op: DbFilterOp,
    /// Compared value.
    pub value: Value,
}

/// One DB order key.
#[derive(Clone, Debug)]
pub struct DbOrder {
    /// Field name.
    pub field: String,
    /// Sort descending if true.
    pub desc: bool,
}

/// Vector-near query option.
#[derive(Clone, Debug)]
pub struct DbNear {
    /// Vector field name.
    pub field: String,
    /// Query vector.
    pub vector: Vec<f64>,
}

/// Query options.
#[derive(Clone, Debug, Default)]
pub struct DbQuery {
    /// Filters.
    pub filters: Vec<DbFilter>,
    /// Order keys.
    pub order: Vec<DbOrder>,
    /// Rows to skip.
    pub skip: Option<usize>,
    /// Max rows.
    pub limit: Option<usize>,
    /// Projected fields. Empty means all fields.
    pub fields: Vec<String>,
    /// Optional vector-near ordering.
    pub near: Option<DbNear>,
}

impl DbQuery {
    /// Equality filters from old object form.
    #[must_use]
    pub fn from_equality(filter: &[(String, Value)]) -> Self {
        Self {
            filters: filter
                .iter()
                .map(|(field, value)| DbFilter {
                    field: field.clone(),
                    op: DbFilterOp::Eq,
                    value: value.clone(),
                })
                .collect(),
            ..Self::default()
        }
    }
}

/// In-memory DB — 요청 간 `Rc<RefCell<>>` 로 공유된다.
#[derive(Clone, Debug, Default)]
pub struct InMemoryDb {
    tables: HashMap<String, Table>,
    wal_path: Option<PathBuf>,
}

/// DB snapshot load/save error.
#[derive(Debug, thiserror::Error)]
pub enum DbSnapshotError {
    /// Filesystem error.
    #[error("i/o error for {path}: {source}")]
    Io {
        /// Path being read or written.
        path: PathBuf,
        /// Source error.
        #[source]
        source: std::io::Error,
    },
    /// JSON parse or serialization error.
    #[error("json error for {path}: {source}")]
    Json {
        /// Path being parsed or serialized.
        path: PathBuf,
        /// Source error.
        #[source]
        source: serde_json::Error,
    },
    /// Snapshot shape is invalid.
    #[error("{0}")]
    Invalid(String),
}

#[derive(Clone, Debug, Default)]
struct Table {
    rows: Vec<Vec<(String, Value)>>,
    next_id: i64,
}

impl InMemoryDb {
    /// 새 DB.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// `data` object 를 테이블에 삽입하고 id 가 채워진 row 전체를 반환.
    pub fn create(&mut self, table_name: &str, data: Vec<(String, Value)>) -> Value {
        let table = self.tables.entry(table_name.to_string()).or_default();
        let id = table.next_id + 1;
        table.next_id = id;
        let mut row: Vec<(String, Value)> = Vec::with_capacity(data.len() + 1);
        row.push(("id".to_string(), Value::Int(id)));
        for (k, v) in data {
            if k == "id" {
                // 사용자 지정 id 는 MVP 에서 허용하지 않고 자동 id 만 사용.
                continue;
            }
            row.push((k, v));
        }
        table.rows.push(row.clone());
        Value::Object(row)
    }

    /// Append a create record to the configured WAL before applying it.
    ///
    /// # Errors
    /// Returns an error when WAL append/fsync fails.
    pub fn create_logged(
        &mut self,
        table_name: &str,
        data: Vec<(String, Value)>,
    ) -> Result<Value, DbSnapshotError> {
        if let Some(path) = &self.wal_path {
            append_wal_record(
                path,
                &serde_json::json!({
                    "schema_version": DB_WAL_SCHEMA_VERSION,
                    "op": "create",
                    "table": table_name,
                    "data": fields_to_json(&data),
                }),
            )?;
        }
        Ok(self.create(table_name, data))
    }

    /// equality filter 로 첫 매칭 row 반환. 없으면 `Value::Void`.
    pub fn find_one(&self, table_name: &str, filter: &[(String, Value)]) -> Value {
        self.find_one_query(table_name, &DbQuery::from_equality(filter))
    }

    /// query 로 첫 매칭 row 반환. 없으면 `Value::Void`.
    pub fn find_one_query(&self, table_name: &str, query: &DbQuery) -> Value {
        self.find_query(table_name, query)
            .into_iter()
            .next()
            .unwrap_or(Value::Void)
    }

    /// equality filter 로 모든 매칭 row 의 배열 반환.
    pub fn find_all(&self, table_name: &str, filter: &[(String, Value)]) -> Value {
        Value::Array(self.find_query(table_name, &DbQuery::from_equality(filter)))
    }

    /// query 로 모든 매칭 row 반환.
    pub fn find_query(&self, table_name: &str, query: &DbQuery) -> Vec<Value> {
        let Some(table) = self.tables.get(table_name) else {
            return Vec::new();
        };
        let mut rows: Vec<Vec<(String, Value)>> = table
            .rows
            .iter()
            .filter(|row| matches_query(row, query))
            .cloned()
            .collect();
        if let Some(near) = &query.near {
            rows.sort_by(|a, b| compare_near_rows(a, b, near));
        } else if !query.order.is_empty() {
            rows.sort_by(|a, b| compare_ordered_rows(a, b, &query.order));
        }
        let skip = query.skip.unwrap_or(0);
        let limit = query.limit.unwrap_or(usize::MAX);
        rows.into_iter()
            .skip(skip)
            .take(limit)
            .map(|row| Value::Object(project_row(row, &query.fields)))
            .collect()
    }

    /// query 매칭 row 수.
    pub fn count_query(&self, table_name: &str, query: &DbQuery) -> i64 {
        let Some(table) = self.tables.get(table_name) else {
            return 0;
        };
        table
            .rows
            .iter()
            .filter(|row| matches_query(row, query))
            .count()
            .try_into()
            .unwrap_or(i64::MAX)
    }

    /// query 매칭 row 의 numeric field 합계.
    pub fn sum_query(&self, table_name: &str, query: &DbQuery, field: &str) -> Value {
        let Some(table) = self.tables.get(table_name) else {
            return Value::Int(0);
        };
        let mut int_sum = 0i64;
        let mut float_sum = 0.0f64;
        let mut has_float = false;
        for row in table.rows.iter().filter(|row| matches_query(row, query)) {
            let Some(value) = row
                .iter()
                .find(|(key, _)| key == field)
                .map(|(_, value)| value)
            else {
                continue;
            };
            match value {
                Value::Int(n) if has_float => float_sum += *n as f64,
                Value::Int(n) => int_sum += *n,
                Value::Float(n) => {
                    if !has_float {
                        float_sum = int_sum as f64;
                        has_float = true;
                    }
                    float_sum += *n;
                }
                _ => {}
            }
        }
        if has_float {
            Value::Float(float_sum)
        } else {
            Value::Int(int_sum)
        }
    }

    /// filter 매칭 row 에 `data` 를 병합. 갱신된 row 수 반환.
    pub fn update(
        &mut self,
        table_name: &str,
        filter: &[(String, Value)],
        data: &[(String, Value)],
    ) -> i64 {
        self.update_query(table_name, &DbQuery::from_equality(filter), data, &[])
    }

    /// query 매칭 row 에 `data` 병합과 `inc` 증감을 적용.
    pub fn update_query(
        &mut self,
        table_name: &str,
        query: &DbQuery,
        data: &[(String, Value)],
        inc: &[(String, Value)],
    ) -> i64 {
        let Some(table) = self.tables.get_mut(table_name) else {
            return 0;
        };
        let mut n = 0i64;
        for row in &mut table.rows {
            if matches_query(row, query) {
                for (k, v) in data {
                    if k == "id" {
                        continue;
                    }
                    if let Some(slot) = row.iter_mut().find(|(ek, _)| ek == k) {
                        slot.1 = v.clone();
                    } else {
                        row.push((k.clone(), v.clone()));
                    }
                }
                for (k, delta) in inc {
                    apply_increment(row, k, delta);
                }
                n += 1;
            }
        }
        n
    }

    /// Append an update record to the configured WAL before applying it.
    ///
    /// # Errors
    /// Returns an error when WAL append/fsync fails.
    pub fn update_logged(
        &mut self,
        table_name: &str,
        query: &DbQuery,
        data: &[(String, Value)],
        inc: &[(String, Value)],
    ) -> Result<i64, DbSnapshotError> {
        if let Some(path) = &self.wal_path {
            append_wal_record(
                path,
                &serde_json::json!({
                    "schema_version": DB_WAL_SCHEMA_VERSION,
                    "op": "update",
                    "table": table_name,
                    "query": query_to_json(query),
                    "data": fields_to_json(data),
                    "inc": fields_to_json(inc),
                }),
            )?;
        }
        Ok(self.update_query(table_name, query, data, inc))
    }

    /// filter 매칭 row 제거. 제거된 수 반환.
    pub fn delete(&mut self, table_name: &str, filter: &[(String, Value)]) -> i64 {
        self.delete_query(table_name, &DbQuery::from_equality(filter))
    }

    /// query 매칭 row 제거. 제거된 수 반환.
    pub fn delete_query(&mut self, table_name: &str, query: &DbQuery) -> i64 {
        let Some(table) = self.tables.get_mut(table_name) else {
            return 0;
        };
        let before = table.rows.len();
        table.rows.retain(|row| !matches_query(row, query));
        i64::try_from(before - table.rows.len()).unwrap_or(0)
    }

    /// Append a delete record to the configured WAL before applying it.
    ///
    /// # Errors
    /// Returns an error when WAL append/fsync fails.
    pub fn delete_logged(
        &mut self,
        table_name: &str,
        query: &DbQuery,
    ) -> Result<i64, DbSnapshotError> {
        if let Some(path) = &self.wal_path {
            append_wal_record(
                path,
                &serde_json::json!({
                    "schema_version": DB_WAL_SCHEMA_VERSION,
                    "op": "delete",
                    "table": table_name,
                    "query": query_to_json(query),
                }),
            )?;
        }
        Ok(self.delete_query(table_name, query))
    }

    /// Serialize the current DB state as a deterministic JSON snapshot.
    #[must_use]
    pub fn snapshot_json(&self) -> serde_json::Value {
        let mut tables = BTreeMap::new();
        for (name, table) in &self.tables {
            let rows = table
                .rows
                .iter()
                .map(|row| {
                    let fields = row
                        .iter()
                        .map(|(key, value)| (key.clone(), value_to_json(value)))
                        .collect();
                    serde_json::Value::Object(fields)
                })
                .collect::<Vec<_>>();
            tables.insert(
                name.clone(),
                serde_json::json!({
                    "next_id": table.next_id,
                    "rows": rows,
                }),
            );
        }
        serde_json::json!({
            "schema_version": DB_SNAPSHOT_SCHEMA_VERSION,
            "tables": tables,
        })
    }

    /// Save the current DB snapshot to disk.
    ///
    /// # Errors
    /// Returns an error when the parent directory cannot be created, JSON
    /// serialization fails, or the file cannot be written.
    pub fn save_snapshot(&self, path: &Path) -> Result<(), DbSnapshotError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| DbSnapshotError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        let bytes = serde_json::to_vec_pretty(&self.snapshot_json()).map_err(|source| {
            DbSnapshotError::Json {
                path: path.to_path_buf(),
                source,
            }
        })?;
        std::fs::write(path, bytes).map_err(|source| DbSnapshotError::Io {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Load a DB snapshot from disk.
    ///
    /// # Errors
    /// Returns an error when the file cannot be read, parsed, or restored.
    pub fn load_snapshot(path: &Path) -> Result<Self, DbSnapshotError> {
        let source = std::fs::read_to_string(path).map_err(|source| DbSnapshotError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        let snapshot: serde_json::Value =
            serde_json::from_str(&source).map_err(|source| DbSnapshotError::Json {
                path: path.to_path_buf(),
                source,
            })?;
        Self::restore_json(&snapshot)
    }

    /// Restore a DB from a JSON snapshot created by [`Self::snapshot_json`].
    ///
    /// # Errors
    /// Returns an error when the schema version or row/table shape is invalid.
    pub fn restore_json(snapshot: &serde_json::Value) -> Result<Self, DbSnapshotError> {
        let version = snapshot
            .get("schema_version")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| DbSnapshotError::Invalid("missing db snapshot schema".to_string()))?;
        if version != u64::from(DB_SNAPSHOT_SCHEMA_VERSION) {
            return Err(DbSnapshotError::Invalid(format!(
                "unsupported db snapshot schema {version}"
            )));
        }
        let tables_value = snapshot
            .get("tables")
            .and_then(serde_json::Value::as_object)
            .ok_or_else(|| {
                DbSnapshotError::Invalid("snapshot tables must be object".to_string())
            })?;
        let mut tables = HashMap::new();
        for (name, table_value) in tables_value {
            let next_id = table_value
                .get("next_id")
                .and_then(serde_json::Value::as_i64)
                .ok_or_else(|| {
                    DbSnapshotError::Invalid(format!("table {name} next_id must be int"))
                })?;
            let rows_value = table_value
                .get("rows")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| {
                    DbSnapshotError::Invalid(format!("table {name} rows must be array"))
                })?;
            let mut rows = Vec::new();
            for row_value in rows_value {
                let row_object = row_value.as_object().ok_or_else(|| {
                    DbSnapshotError::Invalid(format!("table {name} row must be object"))
                })?;
                let mut row = Vec::new();
                for (field, value) in row_object {
                    row.push((field.clone(), json_to_value(value)?));
                }
                rows.push(row);
            }
            tables.insert(name.clone(), Table { rows, next_id });
        }
        Ok(Self {
            tables,
            wal_path: None,
        })
    }

    /// Capture a lightweight in-memory savepoint.
    #[must_use]
    pub fn savepoint(&self) -> Self {
        Self {
            tables: self.tables.clone(),
            wal_path: None,
        }
    }

    /// Restore table state from a savepoint while preserving this DB's WAL path.
    ///
    /// # Errors
    /// Returns an error when the WAL-backed DB cannot checkpoint the restored
    /// state.
    pub fn restore_savepoint(&mut self, savepoint: &Self) -> Result<(), DbSnapshotError> {
        self.tables.clone_from(&savepoint.tables);
        self.checkpoint_wal_if_enabled()
    }

    /// Load a DB by replaying a JSONL WAL. Missing WAL means empty DB.
    ///
    /// # Errors
    /// Returns an error when the WAL cannot be read, parsed, or replayed.
    pub fn load_wal(path: &Path) -> Result<Self, DbSnapshotError> {
        Self::load_wal_records(path, None, None)
    }

    /// Load a DB by replaying at most `until_record` complete WAL records.
    /// Missing WAL means empty DB.
    ///
    /// # Errors
    /// Returns an error when the WAL cannot be read, parsed, or replayed.
    pub fn load_wal_until_record(
        path: &Path,
        until_record: Option<usize>,
    ) -> Result<Self, DbSnapshotError> {
        Self::load_wal_records(path, until_record, None)
    }

    /// Load a DB by replaying complete WAL records whose `ts_unix_ms` is not
    /// newer than `until_unix_ms`. Missing WAL means empty DB.
    ///
    /// # Errors
    /// Returns an error when the WAL cannot be read, parsed, or replayed.
    pub fn load_wal_until_unix_ms(
        path: &Path,
        until_unix_ms: Option<u64>,
    ) -> Result<Self, DbSnapshotError> {
        Self::load_wal_records(path, None, until_unix_ms)
    }

    fn load_wal_records(
        path: &Path,
        until_record: Option<usize>,
        until_unix_ms: Option<u64>,
    ) -> Result<Self, DbSnapshotError> {
        let mut db = Self {
            tables: HashMap::new(),
            wal_path: Some(path.to_path_buf()),
        };
        let source = match std::fs::read_to_string(path) {
            Ok(source) => source,
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(db),
            Err(source) => {
                return Err(DbSnapshotError::Io {
                    path: path.to_path_buf(),
                    source,
                });
            }
        };
        let lines = source.lines().collect::<Vec<_>>();
        let has_complete_tail = source.ends_with('\n');
        let mut replayed_records = 0usize;
        for (line_index, line) in lines.iter().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            if until_record.is_some_and(|limit| replayed_records >= limit) {
                break;
            }
            let record: serde_json::Value = match serde_json::from_str(line) {
                Ok(record) => record,
                Err(source)
                    if line_index + 1 == lines.len() && !has_complete_tail && source.is_eof() =>
                {
                    break;
                }
                Err(source) => {
                    return Err(DbSnapshotError::Json {
                        path: path.to_path_buf(),
                        source,
                    });
                }
            };
            if let Some(limit) = until_unix_ms {
                let timestamp = record
                    .get("ts_unix_ms")
                    .and_then(serde_json::Value::as_u64)
                    .ok_or_else(|| {
                        DbSnapshotError::Invalid(format!(
                            "wal line {}: missing ts_unix_ms for timestamp recovery",
                            line_index + 1
                        ))
                    })?;
                if timestamp > limit {
                    break;
                }
            }
            replay_wal_record(&mut db, &record).map_err(|err| {
                DbSnapshotError::Invalid(format!("wal line {}: {err}", line_index + 1))
            })?;
            replayed_records += 1;
        }
        Ok(db)
    }

    /// Compact the configured WAL into a single checkpoint snapshot record.
    ///
    /// # Errors
    /// Returns an error when this DB was not opened from a WAL, serialization
    /// fails, or the WAL file cannot be rewritten.
    pub fn checkpoint_wal(&self) -> Result<(), DbSnapshotError> {
        let path = self
            .wal_path
            .as_ref()
            .ok_or_else(|| DbSnapshotError::Invalid("db has no wal path".to_string()))?;
        replace_wal_with_record(
            path,
            &serde_json::json!({
                "schema_version": DB_WAL_SCHEMA_VERSION,
                "op": "checkpoint",
                "snapshot": self.snapshot_json(),
            }),
        )
    }

    /// Compact the WAL when this DB is WAL-backed; otherwise do nothing.
    ///
    /// # Errors
    /// Returns an error when WAL compaction fails.
    pub fn checkpoint_wal_if_enabled(&self) -> Result<(), DbSnapshotError> {
        if self.wal_path.is_some() {
            self.checkpoint_wal()
        } else {
            Ok(())
        }
    }
}

fn append_wal_record(path: &Path, record: &serde_json::Value) -> Result<(), DbSnapshotError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| DbSnapshotError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|source| DbSnapshotError::Io {
            path: path.to_path_buf(),
            source,
        })?;
    let record = wal_record_with_timestamp(record);
    let bytes = serde_json::to_vec(&record).map_err(|source| DbSnapshotError::Json {
        path: path.to_path_buf(),
        source,
    })?;
    file.write_all(&bytes)
        .and_then(|()| file.write_all(b"\n"))
        .and_then(|()| file.sync_data())
        .map_err(|source| DbSnapshotError::Io {
            path: path.to_path_buf(),
            source,
        })
}

fn replace_wal_with_record(path: &Path, record: &serde_json::Value) -> Result<(), DbSnapshotError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| DbSnapshotError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    let temp_path = wal_checkpoint_temp_path(path);
    let mut file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&temp_path)
        .map_err(|source| DbSnapshotError::Io {
            path: temp_path.clone(),
            source,
        })?;
    let record = wal_record_with_timestamp(record);
    let bytes = serde_json::to_vec(&record).map_err(|source| DbSnapshotError::Json {
        path: temp_path.clone(),
        source,
    })?;
    file.write_all(&bytes)
        .and_then(|()| file.write_all(b"\n"))
        .and_then(|()| file.sync_data())
        .map_err(|source| DbSnapshotError::Io {
            path: temp_path.clone(),
            source,
        })?;
    std::fs::rename(&temp_path, path).map_err(|source| DbSnapshotError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn wal_checkpoint_temp_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("orv-db.wal");
    path.with_file_name(format!(".{file_name}.checkpoint.tmp"))
}

fn wal_record_with_timestamp(record: &serde_json::Value) -> serde_json::Value {
    let mut record = record.clone();
    if let Some(object) = record.as_object_mut() {
        object
            .entry("ts_unix_ms".to_string())
            .or_insert_with(|| serde_json::json!(current_unix_ms()));
    }
    record
}

fn current_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().try_into().unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn replay_wal_record(db: &mut InMemoryDb, record: &serde_json::Value) -> Result<(), String> {
    let version = record
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| "missing wal schema version".to_string())?;
    if version != u64::from(DB_WAL_SCHEMA_VERSION) {
        return Err(format!("unsupported wal schema {version}"));
    }
    let op = record
        .get("op")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "missing wal op".to_string())?;
    match op {
        "checkpoint" => {
            let snapshot = record
                .get("snapshot")
                .ok_or_else(|| "missing wal checkpoint snapshot".to_string())?;
            let restored = InMemoryDb::restore_json(snapshot).map_err(|err| err.to_string())?;
            db.tables = restored.tables;
            Ok(())
        }
        "create" => {
            let table = wal_table(record)?;
            let data = fields_from_json(record.get("data").unwrap_or(&serde_json::Value::Null))
                .map_err(|err| format!("create data: {err}"))?;
            db.create(table, data);
            Ok(())
        }
        "update" => {
            let table = wal_table(record)?;
            let query = query_from_json(record.get("query").unwrap_or(&serde_json::Value::Null))
                .map_err(|err| format!("update query: {err}"))?;
            let data = fields_from_json(record.get("data").unwrap_or(&serde_json::Value::Null))
                .map_err(|err| format!("update data: {err}"))?;
            let inc = fields_from_json(record.get("inc").unwrap_or(&serde_json::Value::Null))
                .map_err(|err| format!("update inc: {err}"))?;
            db.update_query(table, &query, &data, &inc);
            Ok(())
        }
        "delete" => {
            let table = wal_table(record)?;
            let query = query_from_json(record.get("query").unwrap_or(&serde_json::Value::Null))
                .map_err(|err| format!("delete query: {err}"))?;
            db.delete_query(table, &query);
            Ok(())
        }
        other => Err(format!("unknown wal op `{other}`")),
    }
}

fn wal_table(record: &serde_json::Value) -> Result<&str, String> {
    record
        .get("table")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "missing wal table".to_string())
}

fn fields_to_json(fields: &[(String, Value)]) -> serde_json::Value {
    serde_json::Value::Object(
        fields
            .iter()
            .map(|(key, value)| (key.clone(), value_to_json(value)))
            .collect(),
    )
}

fn fields_from_json(value: &serde_json::Value) -> Result<Vec<(String, Value)>, DbSnapshotError> {
    let object = value
        .as_object()
        .ok_or_else(|| DbSnapshotError::Invalid("wal fields must be object".to_string()))?;
    object
        .iter()
        .map(|(key, value)| Ok((key.clone(), json_to_value(value)?)))
        .collect()
}

fn query_to_json(query: &DbQuery) -> serde_json::Value {
    serde_json::json!({
        "filters": query.filters.iter().map(|filter| {
            serde_json::json!({
                "field": filter.field,
                "op": filter_op_to_str(filter.op),
                "value": value_to_json(&filter.value),
            })
        }).collect::<Vec<_>>(),
    })
}

fn query_from_json(value: &serde_json::Value) -> Result<DbQuery, DbSnapshotError> {
    let object = value
        .as_object()
        .ok_or_else(|| DbSnapshotError::Invalid("wal query must be object".to_string()))?;
    let filters = object
        .get("filters")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| DbSnapshotError::Invalid("wal query filters must be array".to_string()))?
        .iter()
        .map(|filter| {
            let object = filter.as_object().ok_or_else(|| {
                DbSnapshotError::Invalid("wal query filter must be object".to_string())
            })?;
            let field = object
                .get("field")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| DbSnapshotError::Invalid("wal filter field missing".to_string()))?;
            let op = object
                .get("op")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| DbSnapshotError::Invalid("wal filter op missing".to_string()))
                .and_then(filter_op_from_str)?;
            let value = object
                .get("value")
                .ok_or_else(|| DbSnapshotError::Invalid("wal filter value missing".to_string()))
                .and_then(json_to_value)?;
            Ok(DbFilter {
                field: field.to_string(),
                op,
                value,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(DbQuery {
        filters,
        ..DbQuery::default()
    })
}

const fn filter_op_to_str(op: DbFilterOp) -> &'static str {
    match op {
        DbFilterOp::Eq => "eq",
        DbFilterOp::Ne => "ne",
        DbFilterOp::Gt => "gt",
        DbFilterOp::Ge => "ge",
        DbFilterOp::Lt => "lt",
        DbFilterOp::Le => "le",
        DbFilterOp::Contains => "contains",
        DbFilterOp::In => "in",
    }
}

fn filter_op_from_str(op: &str) -> Result<DbFilterOp, DbSnapshotError> {
    match op {
        "eq" => Ok(DbFilterOp::Eq),
        "ne" => Ok(DbFilterOp::Ne),
        "gt" => Ok(DbFilterOp::Gt),
        "ge" => Ok(DbFilterOp::Ge),
        "lt" => Ok(DbFilterOp::Lt),
        "le" => Ok(DbFilterOp::Le),
        "contains" => Ok(DbFilterOp::Contains),
        "in" => Ok(DbFilterOp::In),
        _ => Err(DbSnapshotError::Invalid(format!(
            "unknown wal filter op `{op}`"
        ))),
    }
}

fn value_to_json(value: &Value) -> serde_json::Value {
    match value {
        Value::Int(n) => serde_json::Value::Number((*n).into()),
        Value::Float(n) => serde_json::Number::from_f64(*n)
            .map_or(serde_json::Value::Null, serde_json::Value::Number),
        Value::Str(s) => serde_json::Value::String(s.clone()),
        Value::Regex { pattern, flags } => {
            serde_json::json!({ "$regex": pattern, "$flags": flags })
        }
        Value::Bool(v) => serde_json::Value::Bool(*v),
        Value::Void => serde_json::Value::Null,
        Value::Array(items) | Value::Tuple(items) => {
            serde_json::Value::Array(items.iter().map(value_to_json).collect())
        }
        Value::Object(fields) => serde_json::Value::Object(
            fields
                .iter()
                .map(|(key, value)| (key.clone(), value_to_json(value)))
                .collect(),
        ),
        Value::Function(_)
        | Value::Lambda(_)
        | Value::BoundMethod { .. }
        | Value::Db(_)
        | Value::TypeName(_)
        | Value::Builtin(_) => serde_json::Value::String(value.to_string()),
    }
}

fn json_to_value(value: &serde_json::Value) -> Result<Value, DbSnapshotError> {
    match value {
        serde_json::Value::Null => Ok(Value::Void),
        serde_json::Value::Bool(v) => Ok(Value::Bool(*v)),
        serde_json::Value::Number(n) => n.as_i64().map_or_else(
            || {
                n.as_f64().map_or_else(
                    || {
                        Err(DbSnapshotError::Invalid(
                            "snapshot number is not representable".to_string(),
                        ))
                    },
                    |f| Ok(Value::Float(f)),
                )
            },
            |i| Ok(Value::Int(i)),
        ),
        serde_json::Value::String(s) => Ok(Value::Str(s.clone())),
        serde_json::Value::Array(items) => items
            .iter()
            .map(json_to_value)
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        serde_json::Value::Object(fields) => {
            if fields.contains_key("$regex") {
                let pattern = fields
                    .get("$regex")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| {
                        DbSnapshotError::Invalid("regex pattern must be string".to_string())
                    })?;
                let flags = fields
                    .get("$flags")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                return Ok(Value::Regex {
                    pattern: pattern.to_string(),
                    flags: flags.to_string(),
                });
            }
            fields
                .iter()
                .map(|(key, value)| Ok((key.clone(), json_to_value(value)?)))
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Object)
        }
    }
}

fn matches_query(row: &[(String, Value)], query: &DbQuery) -> bool {
    for filter in &query.filters {
        let Some(rv) = row.iter().find(|(k, _)| k == &filter.field).map(|(_, v)| v) else {
            return false;
        };
        if !matches_filter_op(rv, &filter.op, &filter.value) {
            return false;
        }
    }
    true
}

fn project_row(row: Vec<(String, Value)>, fields: &[String]) -> Vec<(String, Value)> {
    if fields.is_empty() {
        return row;
    }
    fields
        .iter()
        .filter_map(|field| {
            row.iter()
                .find(|(key, _)| key == field)
                .map(|(_, value)| (field.clone(), value.clone()))
        })
        .collect()
}

fn matches_filter_op(row_value: &Value, op: &DbFilterOp, filter_value: &Value) -> bool {
    match op {
        DbFilterOp::Eq => values_eq(row_value, filter_value),
        DbFilterOp::Ne => !values_eq(row_value, filter_value),
        DbFilterOp::Gt => {
            compare_values(row_value, filter_value).is_some_and(std::cmp::Ordering::is_gt)
        }
        DbFilterOp::Ge => {
            compare_values(row_value, filter_value).is_some_and(|o| o.is_gt() || o.is_eq())
        }
        DbFilterOp::Lt => {
            compare_values(row_value, filter_value).is_some_and(std::cmp::Ordering::is_lt)
        }
        DbFilterOp::Le => {
            compare_values(row_value, filter_value).is_some_and(|o| o.is_lt() || o.is_eq())
        }
        DbFilterOp::Contains => match (row_value, filter_value) {
            (Value::Str(haystack), Value::Str(needle)) => haystack.contains(needle),
            (Value::Array(items), needle) => items.iter().any(|item| values_eq(item, needle)),
            _ => false,
        },
        DbFilterOp::In => match filter_value {
            Value::Array(items) => items.iter().any(|item| values_eq(row_value, item)),
            _ => false,
        },
    }
}

fn compare_ordered_rows(
    a: &[(String, Value)],
    b: &[(String, Value)],
    order: &[DbOrder],
) -> std::cmp::Ordering {
    for item in order {
        let av = a.iter().find(|(k, _)| k == &item.field).map(|(_, v)| v);
        let bv = b.iter().find(|(k, _)| k == &item.field).map(|(_, v)| v);
        let mut ord = match (av, bv) {
            (Some(left), Some(right)) => {
                compare_values(left, right).unwrap_or(std::cmp::Ordering::Equal)
            }
            (Some(_), None) => std::cmp::Ordering::Greater,
            (None, Some(_)) => std::cmp::Ordering::Less,
            (None, None) => std::cmp::Ordering::Equal,
        };
        if item.desc {
            ord = ord.reverse();
        }
        if !ord.is_eq() {
            return ord;
        }
    }
    std::cmp::Ordering::Equal
}

fn compare_near_rows(
    a: &[(String, Value)],
    b: &[(String, Value)],
    near: &DbNear,
) -> std::cmp::Ordering {
    let ad = row_vector_distance(a, near).unwrap_or(f64::INFINITY);
    let bd = row_vector_distance(b, near).unwrap_or(f64::INFINITY);
    ad.partial_cmp(&bd).unwrap_or(std::cmp::Ordering::Equal)
}

fn row_vector_distance(row: &[(String, Value)], near: &DbNear) -> Option<f64> {
    let vector_value = row
        .iter()
        .find(|(key, _)| key == &near.field)
        .map(|(_, value)| value)?;
    let vector = value_to_vector(vector_value)?;
    if vector.len() != near.vector.len() {
        return None;
    }
    Some(
        vector
            .iter()
            .zip(&near.vector)
            .map(|(a, b)| {
                let d = a - b;
                d * d
            })
            .sum(),
    )
}

fn value_to_vector(value: &Value) -> Option<Vec<f64>> {
    let Value::Array(items) = value else {
        return None;
    };
    items
        .iter()
        .map(|item| match item {
            Value::Int(n) => Some(*n as f64),
            Value::Float(n) => Some(*n),
            _ => None,
        })
        .collect()
}

fn compare_values(a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => Some(x.cmp(y)),
        (Value::Float(x), Value::Float(y)) => x.partial_cmp(y),
        (Value::Int(x), Value::Float(y)) => (*x as f64).partial_cmp(y),
        (Value::Float(x), Value::Int(y)) => x.partial_cmp(&(*y as f64)),
        (Value::Str(x), Value::Str(y)) => Some(x.cmp(y)),
        (Value::Bool(x), Value::Bool(y)) => Some(x.cmp(y)),
        _ => None,
    }
}

fn apply_increment(row: &mut Vec<(String, Value)>, key: &str, delta: &Value) {
    if key == "id" {
        return;
    }
    if let Some((_, slot)) = row.iter_mut().find(|(existing, _)| existing == key) {
        *slot = incremented_value(slot, delta);
    } else {
        row.push((key.to_string(), delta.clone()));
    }
}

fn incremented_value(current: &Value, delta: &Value) -> Value {
    match (current, delta) {
        (Value::Int(a), Value::Int(b)) => Value::Int(a + b),
        (Value::Float(a), Value::Float(b)) => Value::Float(a + b),
        (Value::Int(a), Value::Float(b)) => Value::Float(*a as f64 + b),
        (Value::Float(a), Value::Int(b)) => Value::Float(a + *b as f64),
        _ => delta.clone(),
    }
}

fn values_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Float(x), Value::Float(y)) => (x - y).abs() < f64::EPSILON,
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Void, Value::Void) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(pairs: &[(&str, Value)]) -> Vec<(String, Value)> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn create_assigns_auto_id() {
        let mut db = InMemoryDb::new();
        let v = db.create("User", obj(&[("name", Value::Str("alice".into()))]));
        let Value::Object(fields) = v else {
            panic!("create must return object");
        };
        assert!(matches!(
            fields.iter().find(|(k, _)| k == "id"),
            Some((_, Value::Int(1)))
        ));
        let v2 = db.create("User", obj(&[("name", Value::Str("bob".into()))]));
        let Value::Object(fields2) = v2 else {
            panic!("create must return object");
        };
        assert!(matches!(
            fields2.iter().find(|(k, _)| k == "id"),
            Some((_, Value::Int(2)))
        ));
    }

    #[test]
    fn find_one_returns_void_when_missing() {
        let db = InMemoryDb::new();
        assert!(matches!(
            db.find_one("User", &obj(&[("id", Value::Int(1))])),
            Value::Void
        ));
    }

    #[test]
    fn find_one_roundtrips_created_row() {
        let mut db = InMemoryDb::new();
        db.create("User", obj(&[("name", Value::Str("alice".into()))]));
        let v = db.find_one("User", &obj(&[("id", Value::Int(1))]));
        let Value::Object(fields) = v else {
            panic!("expected object");
        };
        assert!(fields
            .iter()
            .any(|(k, v)| k == "name" && matches!(v, Value::Str(s) if s == "alice")));
    }

    #[test]
    fn find_all_filters_equality() {
        let mut db = InMemoryDb::new();
        db.create("Post", obj(&[("author", Value::Int(1))]));
        db.create("Post", obj(&[("author", Value::Int(2))]));
        db.create("Post", obj(&[("author", Value::Int(1))]));
        let v = db.find_all("Post", &obj(&[("author", Value::Int(1))]));
        let Value::Array(xs) = v else {
            panic!("expected array");
        };
        assert_eq!(xs.len(), 2);
    }

    #[test]
    fn update_mutates_matching_rows() {
        let mut db = InMemoryDb::new();
        db.create(
            "User",
            obj(&[
                ("name", Value::Str("alice".into())),
                ("age", Value::Int(25)),
            ]),
        );
        let n = db.update(
            "User",
            &obj(&[("id", Value::Int(1))]),
            &obj(&[("age", Value::Int(26))]),
        );
        assert_eq!(n, 1);
        let Value::Object(row) = db.find_one("User", &obj(&[("id", Value::Int(1))])) else {
            panic!("expected object");
        };
        assert!(row
            .iter()
            .any(|(k, v)| k == "age" && matches!(v, Value::Int(26))));
    }

    #[test]
    fn delete_removes_matching() {
        let mut db = InMemoryDb::new();
        db.create("User", obj(&[]));
        db.create("User", obj(&[]));
        let n = db.delete("User", &obj(&[("id", Value::Int(1))]));
        assert_eq!(n, 1);
        let Value::Array(all) = db.find_all("User", &[]) else {
            panic!("expected array");
        };
        assert_eq!(all.len(), 1);
    }

    #[test]
    fn snapshot_json_roundtrips_rows_and_next_id() {
        let mut db = InMemoryDb::new();
        db.create(
            "User",
            obj(&[
                ("name", Value::Str("Ada".into())),
                ("active", Value::Bool(true)),
            ]),
        );
        db.create("User", obj(&[("name", Value::Str("Bea".into()))]));

        let snapshot = db.snapshot_json();
        let mut restored = InMemoryDb::restore_json(&snapshot).expect("restore snapshot");
        let found = restored.find_one("User", &obj(&[("name", Value::Str("Ada".into()))]));
        let Value::Object(fields) = found else {
            panic!("expected restored row");
        };
        assert!(fields
            .iter()
            .any(|(key, value)| key == "active" && matches!(value, Value::Bool(true))));

        let next = restored.create("User", obj(&[("name", Value::Str("Cam".into()))]));
        let Value::Object(fields) = next else {
            panic!("expected created row");
        };
        assert!(fields
            .iter()
            .any(|(key, value)| key == "id" && matches!(value, Value::Int(3))));
    }

    #[test]
    fn restore_json_rejects_unknown_schema_version() {
        let err =
            InMemoryDb::restore_json(&serde_json::json!({ "schema_version": 999, "tables": {} }))
                .expect_err("unsupported schema");

        assert!(err.to_string().contains("unsupported db snapshot schema"));
    }

    #[test]
    fn snapshot_file_roundtrips_rows() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "orv-db-snapshot-{}-{unique}.json",
            std::process::id()
        ));
        let mut db = InMemoryDb::new();
        db.create("User", obj(&[("name", Value::Str("Ada".into()))]));

        db.save_snapshot(&path).expect("save snapshot");
        let restored = InMemoryDb::load_snapshot(&path).expect("load snapshot");

        assert!(matches!(
            restored.find_one("User", &obj(&[("name", Value::Str("Ada".into()))])),
            Value::Object(_)
        ));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn wal_file_replays_logged_create_update_and_delete() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("orv-db-wal-{}-{unique}.jsonl", std::process::id()));
        let mut db = InMemoryDb::load_wal(&path).expect("open wal");
        db.create_logged("User", obj(&[("name", Value::Str("Ada".into()))]))
            .expect("logged create");
        db.create_logged("User", obj(&[("name", Value::Str("Bea".into()))]))
            .expect("logged create");
        db.update_logged(
            "User",
            &DbQuery::from_equality(&obj(&[("name", Value::Str("Ada".into()))])),
            &obj(&[("age", Value::Int(37))]),
            &[],
        )
        .expect("logged update");
        db.delete_logged(
            "User",
            &DbQuery::from_equality(&obj(&[("name", Value::Str("Bea".into()))])),
        )
        .expect("logged delete");

        let restored = InMemoryDb::load_wal(&path).expect("replay wal");

        let ada = restored.find_one("User", &obj(&[("name", Value::Str("Ada".into()))]));
        let Value::Object(fields) = ada else {
            panic!("expected Ada row");
        };
        assert!(fields
            .iter()
            .any(|(key, value)| key == "age" && matches!(value, Value::Int(37))));
        assert!(matches!(
            restored.find_one("User", &obj(&[("name", Value::Str("Bea".into()))])),
            Value::Void
        ));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn wal_records_include_unix_ms_timestamp() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "orv-db-wal-timestamp-{}-{unique}.jsonl",
            std::process::id()
        ));
        let mut db = InMemoryDb::load_wal(&path).expect("open wal");

        db.create_logged("User", obj(&[("name", Value::Str("Ada".into()))]))
            .expect("logged create");

        let wal = std::fs::read_to_string(&path).expect("read wal");
        let record: serde_json::Value =
            serde_json::from_str(wal.lines().next().expect("wal record")).expect("parse record");
        assert!(
            record["ts_unix_ms"].as_u64().is_some_and(|value| value > 0),
            "{record}"
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn wal_replay_ignores_torn_final_record_after_crash() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "orv-db-wal-crash-{}-{unique}.jsonl",
            std::process::id()
        ));
        let mut db = InMemoryDb::load_wal(&path).expect("open wal");
        db.create_logged("User", obj(&[("name", Value::Str("Ada".into()))]))
            .expect("logged create");
        let mut file = OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("open wal for torn tail");
        file.write_all(br#"{"schema_version":1,"op":"update""#)
            .expect("write torn tail");
        file.sync_data().expect("sync torn tail");

        let restored = InMemoryDb::load_wal(&path).expect("replay recoverable wal");

        assert!(matches!(
            restored.find_one("User", &obj(&[("name", Value::Str("Ada".into()))])),
            Value::Object(_)
        ));
        let _ = std::fs::remove_file(path);
    }
}
