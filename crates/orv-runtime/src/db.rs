//! C_db MVP — in-memory 테이블 + equality CRUD.
//!
//! # 범위
//! - 테이블 = `String → Vec<Value::Object>` 맵. row 는 자동 `id` 필드를 받는다.
//! - `create/find/update/delete` 네 메서드, 모두 equality `@where` 만 지원.
//!
//! # 범위 밖
//! - 인덱스 (linear scan).
//! - 트랜잭션/WAL/fsync — 모든 쓰기는 process memory 에만 적용되며 종료 시
//!   사라진다.
//! - 범위/정렬/페이지네이션, `%inc` 증감.
//! - 마이그레이션/스키마 diff, 외부 DB 어댑터.
//! - async/await — 호출 측이 `await` 를 쓰더라도 현재 인터프리터는 sync.

use std::collections::HashMap;

use crate::interp::Value;

/// In-memory DB — 요청 간 Rc<RefCell<>> 로 공유된다.
#[derive(Debug, Default)]
pub struct InMemoryDb {
    tables: HashMap<String, Table>,
}

#[derive(Debug, Default)]
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

    /// equality filter 로 첫 매칭 row 반환. 없으면 `Value::Void`.
    pub fn find_one(&self, table_name: &str, filter: &[(String, Value)]) -> Value {
        let Some(table) = self.tables.get(table_name) else {
            return Value::Void;
        };
        for row in &table.rows {
            if matches_filter(row, filter) {
                return Value::Object(row.clone());
            }
        }
        Value::Void
    }

    /// equality filter 로 모든 매칭 row 의 배열 반환.
    pub fn find_all(&self, table_name: &str, filter: &[(String, Value)]) -> Value {
        let Some(table) = self.tables.get(table_name) else {
            return Value::Array(Vec::new());
        };
        let matches: Vec<Value> = table
            .rows
            .iter()
            .filter(|row| matches_filter(row, filter))
            .map(|row| Value::Object(row.clone()))
            .collect();
        Value::Array(matches)
    }

    /// filter 매칭 row 에 `data` 를 병합. 갱신된 row 수 반환.
    pub fn update(
        &mut self,
        table_name: &str,
        filter: &[(String, Value)],
        data: &[(String, Value)],
    ) -> i64 {
        let Some(table) = self.tables.get_mut(table_name) else {
            return 0;
        };
        let mut n = 0i64;
        for row in &mut table.rows {
            if matches_filter(row, filter) {
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
                n += 1;
            }
        }
        n
    }

    /// filter 매칭 row 제거. 제거된 수 반환.
    pub fn delete(&mut self, table_name: &str, filter: &[(String, Value)]) -> i64 {
        let Some(table) = self.tables.get_mut(table_name) else {
            return 0;
        };
        let before = table.rows.len();
        table.rows.retain(|row| !matches_filter(row, filter));
        i64::try_from(before - table.rows.len()).unwrap_or(0)
    }
}

fn matches_filter(row: &[(String, Value)], filter: &[(String, Value)]) -> bool {
    for (fk, fv) in filter {
        let Some(rv) = row.iter().find(|(k, _)| k == fk).map(|(_, v)| v) else {
            return false;
        };
        if !values_eq(rv, fv) {
            return false;
        }
    }
    true
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
}
