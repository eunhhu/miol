//! C_db MVP — in-memory 테이블 + CRUD/query helpers.
//!
//! # 범위
//! - 테이블 = `String → Vec<Value::Object>` 맵. row 는 자동 `id` 필드를 받는다.
//! - `create/find/update/delete` 와 equality/range/contains filter.
//!
//! # 범위 밖
//! - 인덱스 (linear scan).
//! - 트랜잭션/WAL/fsync — 모든 쓰기는 process memory 에만 적용되며 종료 시
//!   사라진다.
//! - 외부 DB query planner.
//! - 마이그레이션/스키마 diff, 외부 DB 어댑터.
//! - async/await — 호출 측이 `await` 를 쓰더라도 현재 인터프리터는 sync.

use std::collections::HashMap;

use crate::interp::Value;

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

/// In-memory DB — 요청 간 Rc<RefCell<>> 로 공유된다.
#[derive(Clone, Debug, Default)]
pub struct InMemoryDb {
    tables: HashMap<String, Table>,
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
}
