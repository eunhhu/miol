//! 최소 tree-walking 인터프리터.
//!
//! 타입체크/HIR 경로가 아직 구현되지 않아, AST에서 바로 실행한다.
//! 범위: 리터럴 표현식, 단순 `let` 바인딩, 이항/단항 연산, `@out` 호출.
//! 이후 커밋에서 HIR 기반 정식 실행 경로로 교체된다.

use orv_syntax::ast::{BinaryOp, Expr, ExprKind, Program, Stmt, UnaryOp};
use std::collections::HashMap;
use std::fmt;
use std::io::Write;

/// 런타임 에러.
#[derive(Clone, Debug)]
pub struct RuntimeError {
    /// 사람이 읽을 메시지.
    pub message: String,
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "runtime error: {}", self.message)
    }
}

impl std::error::Error for RuntimeError {}

/// 인터프리터 값.
#[derive(Clone, Debug)]
pub enum Value {
    /// 정수.
    Int(i64),
    /// 부동소수점.
    Float(f64),
    /// 문자열.
    Str(String),
    /// 불리언.
    Bool(bool),
    /// void (값 없음).
    Void,
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Int(v) => write!(f, "{v}"),
            Self::Float(v) => write!(f, "{v}"),
            Self::Str(v) => write!(f, "{v}"),
            Self::Bool(v) => write!(f, "{v}"),
            Self::Void => write!(f, "void"),
        }
    }
}

/// 프로그램을 stdout에 실행한다.
///
/// # Errors
/// 실행 중 정의되지 않은 식별자, 타입 불일치 등이 발생하면 반환한다.
pub fn run(program: &Program) -> Result<(), RuntimeError> {
    let mut stdout = std::io::stdout().lock();
    run_with_writer(program, &mut stdout)
}

/// 테스트 가능한 버전 — 임의의 `Write`에 출력한다.
///
/// # Errors
/// `run`과 동일.
pub fn run_with_writer<W: Write>(
    program: &Program,
    writer: &mut W,
) -> Result<(), RuntimeError> {
    let mut interp = Interp::new(writer);
    interp.run(program)
}

struct Interp<'w, W: Write> {
    env: HashMap<String, Value>,
    writer: &'w mut W,
}

impl<'w, W: Write> Interp<'w, W> {
    fn new(writer: &'w mut W) -> Self {
        Self {
            env: HashMap::new(),
            writer,
        }
    }

    fn run(&mut self, program: &Program) -> Result<(), RuntimeError> {
        let last_idx = program.items.len().saturating_sub(1);
        for (idx, stmt) in program.items.iter().enumerate() {
            let is_last = idx == last_idx;
            self.exec_stmt(stmt, is_last)?;
        }
        Ok(())
    }

    fn exec_stmt(&mut self, stmt: &Stmt, is_last: bool) -> Result<(), RuntimeError> {
        match stmt {
            Stmt::Let(l) => {
                let v = self.eval(&l.init)?;
                self.env.insert(l.name.name.clone(), v);
            }
            Stmt::Const(c) => {
                let v = self.eval(&c.init)?;
                self.env.insert(c.name.name.clone(), v);
            }
            Stmt::Expr(e) => {
                let v = self.eval(e)?;
                // SPEC §12.2 — void scope에서 마지막이 아닌 표현식은 자동 출력.
                // 마지막 표현식은 반환값으로 취급 — MVP에서는 무시.
                if !is_last && matches!(&v, Value::Str(_) | Value::Int(_) | Value::Float(_) | Value::Bool(_)) {
                    if !is_domain_out(e) {
                        self.println(&v)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn eval(&mut self, expr: &Expr) -> Result<Value, RuntimeError> {
        match &expr.kind {
            ExprKind::Integer(s) => s
                .replace('_', "")
                .parse::<i64>()
                .map(Value::Int)
                .map_err(|_| RuntimeError {
                    message: format!("invalid integer literal `{s}`"),
                }),
            ExprKind::Float(s) => s
                .replace('_', "")
                .parse::<f64>()
                .map(Value::Float)
                .map_err(|_| RuntimeError {
                    message: format!("invalid float literal `{s}`"),
                }),
            ExprKind::String(s) => Ok(Value::Str(s.clone())),
            ExprKind::True => Ok(Value::Bool(true)),
            ExprKind::False => Ok(Value::Bool(false)),
            ExprKind::Void => Ok(Value::Void),
            ExprKind::Ident(id) => self.env.get(&id.name).cloned().ok_or_else(|| RuntimeError {
                message: format!("undefined variable `{}`", id.name),
            }),
            ExprKind::Paren(inner) => self.eval(inner),
            ExprKind::Unary { op, expr } => {
                let v = self.eval(expr)?;
                apply_unary(*op, v)
            }
            ExprKind::Binary { op, lhs, rhs } => {
                let l = self.eval(lhs)?;
                let r = self.eval(rhs)?;
                apply_binary(*op, l, r)
            }
            ExprKind::Domain { name, args } => {
                if name.name == "out" {
                    // @out arg → 인자 평가 후 한 줄로 출력
                    if let Some(a) = args.first() {
                        let v = self.eval(a)?;
                        self.println(&v)?;
                    } else {
                        self.println(&Value::Str(String::new()))?;
                    }
                    Ok(Value::Void)
                } else {
                    Err(RuntimeError {
                        message: format!("unsupported domain `@{}` in MVP interpreter", name.name),
                    })
                }
            }
        }
    }

    fn println(&mut self, v: &Value) -> Result<(), RuntimeError> {
        writeln!(self.writer, "{v}").map_err(|e| RuntimeError {
            message: format!("io error: {e}"),
        })
    }
}

/// `stmt`가 이미 `@out` 호출이면 자동 출력 중복을 피한다.
fn is_domain_out(expr: &Expr) -> bool {
    matches!(&expr.kind, ExprKind::Domain { name, .. } if name.name == "out")
}

fn apply_unary(op: UnaryOp, v: Value) -> Result<Value, RuntimeError> {
    match (op, v) {
        (UnaryOp::Not, Value::Bool(b)) => Ok(Value::Bool(!b)),
        (UnaryOp::Neg, Value::Int(i)) => Ok(Value::Int(-i)),
        (UnaryOp::Neg, Value::Float(f)) => Ok(Value::Float(-f)),
        (UnaryOp::BitNot, Value::Int(i)) => Ok(Value::Int(!i)),
        (op, v) => Err(RuntimeError {
            message: format!("unsupported unary `{op:?}` on {v}"),
        }),
    }
}

fn apply_binary(op: BinaryOp, l: Value, r: Value) -> Result<Value, RuntimeError> {
    use BinaryOp::*;
    match (op, l, r) {
        (Add, Value::Int(a), Value::Int(b)) => Ok(Value::Int(a + b)),
        (Sub, Value::Int(a), Value::Int(b)) => Ok(Value::Int(a - b)),
        (Mul, Value::Int(a), Value::Int(b)) => Ok(Value::Int(a * b)),
        (Div, Value::Int(a), Value::Int(b)) if b != 0 => Ok(Value::Int(a / b)),
        (Rem, Value::Int(a), Value::Int(b)) if b != 0 => Ok(Value::Int(a % b)),
        (Pow, Value::Int(a), Value::Int(b)) if (0..=63).contains(&b) => {
            Ok(Value::Int(a.pow(u32::try_from(b).unwrap_or(0))))
        }
        (Pow, Value::Float(a), Value::Float(b)) => Ok(Value::Float(a.powf(b))),
        (Add, Value::Float(a), Value::Float(b)) => Ok(Value::Float(a + b)),
        (Sub, Value::Float(a), Value::Float(b)) => Ok(Value::Float(a - b)),
        (Mul, Value::Float(a), Value::Float(b)) => Ok(Value::Float(a * b)),
        (Div, Value::Float(a), Value::Float(b)) => Ok(Value::Float(a / b)),
        (Add, Value::Str(a), Value::Str(b)) => Ok(Value::Str(a + &b)),
        (Eq, a, b) => Ok(Value::Bool(values_equal(&a, &b))),
        (Ne, a, b) => Ok(Value::Bool(!values_equal(&a, &b))),
        (Lt, Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a < b)),
        (Gt, Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a > b)),
        (Le, Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a <= b)),
        (Ge, Value::Int(a), Value::Int(b)) => Ok(Value::Bool(a >= b)),
        (And, Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(a && b)),
        (Or, Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(a || b)),
        (op, l, r) => Err(RuntimeError {
            message: format!("unsupported binary `{op:?}` on {l} and {r}"),
        }),
    }
}

fn values_equal(a: &Value, b: &Value) -> bool {
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
    use orv_diagnostics::FileId;
    use orv_syntax::{lex, parse};

    fn run_str(src: &str) -> Result<String, RuntimeError> {
        let lx = lex(src, FileId(0));
        assert!(lx.diagnostics.is_empty(), "lex errors: {:?}", lx.diagnostics);
        let pr = parse(lx.tokens, FileId(0));
        assert!(pr.diagnostics.is_empty(), "parse errors: {:?}", pr.diagnostics);
        let mut buf = Vec::new();
        run_with_writer(&pr.program, &mut buf)?;
        Ok(String::from_utf8(buf).unwrap())
    }

    #[test]
    fn explicit_out_prints_string() {
        let out = run_str(r#"@out "Hello, Orv!""#).unwrap();
        assert_eq!(out, "Hello, Orv!\n");
    }

    #[test]
    fn void_scope_autooutput_string() {
        // 마지막이 아닌 문자열은 자동 출력
        let out = run_str(r#""first"
"second"
@out "third""#).unwrap();
        assert_eq!(out, "first\nsecond\nthird\n");
    }

    #[test]
    fn let_and_ident_reference() {
        let out = run_str(
            r#"
            let name: string = "Alice"
            @out name
            "#,
        ).unwrap();
        assert_eq!(out, "Alice\n");
    }

    #[test]
    fn arithmetic_then_out() {
        let out = run_str(
            r#"
            let n: int = 1 + 2 * 3
            @out n
            "#,
        ).unwrap();
        assert_eq!(out, "7\n");
    }

    #[test]
    fn string_concat() {
        let out = run_str(
            r#"
            let a: string = "Hello, "
            let b: string = "World"
            @out a + b
            "#,
        ).unwrap();
        assert_eq!(out, "Hello, World\n");
    }

    #[test]
    fn comparison() {
        let out = run_str("@out 5 > 3").unwrap();
        assert_eq!(out, "true\n");
    }

    #[test]
    fn undefined_variable_errors() {
        let err = run_str("@out missing").unwrap_err();
        assert!(err.message.contains("undefined"));
    }
}
