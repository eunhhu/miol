//! 최소 tree-walking 인터프리터.
//!
//! 타입체크/HIR 경로가 아직 구현되지 않아, AST에서 바로 실행한다.
//! 범위: 리터럴 표현식, 단순 `let` 바인딩, 이항/단항 연산, `@out` 호출.
//! 이후 커밋에서 HIR 기반 정식 실행 경로로 교체된다.

use orv_syntax::ast::{
    BinaryOp, Block, Expr, ExprKind, FunctionBody, FunctionStmt, Param, Pattern, Program, Stmt,
    StringSegment, UnaryOp,
};
use std::rc::Rc;
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
    /// 함수 — 선언 AST에 대한 참조.
    Function(Rc<FunctionStmt>),
    /// 람다 — 파라미터와 본문 + 캡처 환경.
    Lambda(Rc<LambdaValue>),
    /// 바인딩된 내장 메서드 — `arr.map`처럼 receiver에 붙은 함수.
    BoundMethod {
        /// 수신자 값.
        receiver: Box<Value>,
        /// 메서드 이름.
        method: String,
    },
    /// 배열.
    Array(Vec<Value>),
    /// 오브젝트 — 필드 이름 순서 유지.
    Object(Vec<(String, Value)>),
}

/// 람다 값 — 파라미터 + 본문 + 캡처된 환경 스냅샷.
#[derive(Clone, Debug)]
pub struct LambdaValue {
    /// 파라미터.
    pub params: Vec<Param>,
    /// 본문.
    pub body: FunctionBody,
    /// 선언 시점의 환경 스냅샷(클로저).
    pub env: HashMap<String, Value>,
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Int(v) => write!(f, "{v}"),
            Self::Float(v) => write!(f, "{v}"),
            Self::Str(v) => write!(f, "{v}"),
            Self::Bool(v) => write!(f, "{v}"),
            Self::Void => write!(f, "void"),
            Self::Function(func) => write!(f, "<function {}>", func.name.name),
            Self::Lambda(_) => write!(f, "<lambda>"),
            Self::BoundMethod { method, .. } => write!(f, "<method {method}>"),
            Self::Array(items) => {
                write!(f, "[")?;
                for (i, v) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{v}")?;
                }
                write!(f, "]")
            }
            Self::Object(fields) => {
                write!(f, "{{ ")?;
                for (i, (k, v)) in fields.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{k}: {v}")?;
                }
                write!(f, " }}")
            }
        }
    }
}

/// 제어 흐름 신호 — return 문에서 사용.
enum ControlFlow {
    Normal(Value),
    Return(Value),
}

impl ControlFlow {
    fn into_value(self) -> Value {
        match self {
            Self::Normal(v) | Self::Return(v) => v,
        }
    }
}

/// 루프 탈출 신호.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LoopSignal {
    /// 정상 진행.
    None,
    /// 다음 반복으로.
    Continue,
    /// 루프 종료.
    Break,
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
    /// 함수 본문 평가 중 `return` 문이 실행되면 해당 값을 담아 상위로 전파한다.
    /// 상위 호출자(call_function)가 소비해 초기화한다.
    pending_return: Option<Value>,
    /// 루프 본문 평가 중 break/continue 시그널. 루프 밖에서는 None.
    loop_signal: LoopSignal,
}

impl<'w, W: Write> Interp<'w, W> {
    fn new(writer: &'w mut W) -> Self {
        Self {
            env: HashMap::new(),
            writer,
            pending_return: None,
            loop_signal: LoopSignal::None,
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
            Stmt::Function(f) => {
                let name = f.name.name.clone();
                self.env
                    .insert(name, Value::Function(Rc::new((**f).clone())));
            }
            Stmt::Struct(_) => {
                // MVP: 타입 검사가 아직 없으므로 struct 선언은 파서 단계에서
                // 필드 목록만 기록하고 런타임은 noop. 이후 HIR 단계에서 활용.
            }
            Stmt::Return(_) => {
                return Err(RuntimeError {
                    message: "`return` outside of a function".into(),
                });
            }
            Stmt::Expr(e) => {
                let v = self.eval(e)?;
                // SPEC §12.2 — void scope에서 마지막이 아닌 표현식은 자동 출력.
                // 대입/블록/if/when/호출/@out 호출은 제외.
                if !is_last
                    && matches!(
                        &v,
                        Value::Str(_) | Value::Int(_) | Value::Float(_) | Value::Bool(_)
                    )
                    && !has_side_effect(e)
                {
                    self.println(&v)?;
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
            ExprKind::String(segments) => {
                let mut out = String::new();
                for seg in segments {
                    match seg {
                        StringSegment::Str(lit) => out.push_str(lit),
                        StringSegment::Interp(e) => {
                            let v = self.eval(e)?;
                            out.push_str(&value_to_display(&v));
                        }
                    }
                }
                Ok(Value::Str(out))
            }
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
            ExprKind::Block(b) => self.eval_block(b),
            ExprKind::If {
                cond,
                then,
                else_branch,
            } => {
                let c = self.eval(cond)?;
                if is_truthy(&c) {
                    self.eval_block(then)
                } else if let Some(e) = else_branch {
                    self.eval(e)
                } else {
                    Ok(Value::Void)
                }
            }
            ExprKind::When { scrutinee, arms } => {
                let value = self.eval(scrutinee)?;
                for arm in arms {
                    if self.pattern_matches(&arm.pattern, &value)? {
                        return self.eval(&arm.body);
                    }
                }
                Ok(Value::Void)
            }
            ExprKind::Assign { target, value } => {
                if !self.env.contains_key(&target.name) {
                    return Err(RuntimeError {
                        message: format!("cannot assign to undefined `{}`", target.name),
                    });
                }
                let v = self.eval(value)?;
                self.env.insert(target.name.clone(), v.clone());
                Ok(v)
            }
            ExprKind::For { var, iter, body } => {
                let (lo, hi, incl) = self.interpret_range(iter)?;
                let mut i = lo;
                while if incl { i <= hi } else { i < hi } {
                    self.env.insert(var.name.clone(), Value::Int(i));
                    self.eval_block(body)?;
                    match self.loop_signal {
                        LoopSignal::Break => {
                            self.loop_signal = LoopSignal::None;
                            break;
                        }
                        LoopSignal::Continue => self.loop_signal = LoopSignal::None,
                        LoopSignal::None => {}
                    }
                    if self.pending_return.is_some() {
                        break;
                    }
                    i += 1;
                }
                Ok(Value::Void)
            }
            ExprKind::Range { .. } => {
                // 범위는 값으로 평가되지 않는다. for의 iter 또는 when 패턴에서만 소비.
                Err(RuntimeError {
                    message: "range expression can only be used in `for ... in` or `when` patterns".into(),
                })
            }
            ExprKind::Array(items) => {
                let mut values = Vec::with_capacity(items.len());
                for e in items {
                    values.push(self.eval(e)?);
                }
                Ok(Value::Array(values))
            }
            ExprKind::Object(fields) => {
                let mut out = Vec::with_capacity(fields.len());
                for f in fields {
                    let v = self.eval(&f.value)?;
                    out.push((f.name.name.clone(), v));
                }
                Ok(Value::Object(out))
            }
            ExprKind::Index { target, index } => {
                let t = self.eval(target)?;
                let i = self.eval(index)?;
                let Value::Int(idx) = i else {
                    return Err(RuntimeError {
                        message: "index must be an integer".into(),
                    });
                };
                match t {
                    Value::Array(items) => {
                        let n = i64::try_from(items.len()).unwrap_or(i64::MAX);
                        let actual = if idx < 0 { idx + n } else { idx };
                        if actual < 0 || actual >= n {
                            return Err(RuntimeError {
                                message: format!("index {idx} out of bounds for length {n}"),
                            });
                        }
                        Ok(items[actual as usize].clone())
                    }
                    Value::Str(s) => {
                        let chars: Vec<char> = s.chars().collect();
                        let n = i64::try_from(chars.len()).unwrap_or(i64::MAX);
                        let actual = if idx < 0 { idx + n } else { idx };
                        if actual < 0 || actual >= n {
                            return Err(RuntimeError {
                                message: format!("index {idx} out of bounds for length {n}"),
                            });
                        }
                        Ok(Value::Str(chars[actual as usize].to_string()))
                    }
                    other => Err(RuntimeError {
                        message: format!("cannot index into {other}"),
                    }),
                }
            }
            ExprKind::Field { target, field } => {
                let t = self.eval(target)?;
                let name = field.name.as_str();
                match (&t, name) {
                    (Value::Array(items), "length") => Ok(Value::Int(items.len() as i64)),
                    (Value::Str(s), "length") => Ok(Value::Int(s.chars().count() as i64)),
                    (Value::Array(_), "map" | "filter" | "reduce" | "push" | "concat" | "join") => {
                        Ok(Value::BoundMethod {
                            receiver: Box::new(t),
                            method: name.to_string(),
                        })
                    }
                    (Value::Str(_), "toLowerCase" | "toUpperCase" | "contains" | "replace") => {
                        Ok(Value::BoundMethod {
                            receiver: Box::new(t),
                            method: name.to_string(),
                        })
                    }
                    (Value::Object(fields), _) => fields
                        .iter()
                        .find(|(k, _)| k == &field.name)
                        .map(|(_, v)| v.clone())
                        .ok_or_else(|| RuntimeError {
                            message: format!("no field `{}` on object", field.name),
                        }),
                    _ => Err(RuntimeError {
                        message: format!("no field `{}` on {t}", field.name),
                    }),
                }
            }
            ExprKind::Lambda { params, body } => Ok(Value::Lambda(Rc::new(LambdaValue {
                params: params.clone(),
                body: (**body).clone(),
                env: self.env.clone(),
            }))),
            ExprKind::While { cond, body } => {
                loop {
                    let c = self.eval(cond)?;
                    if !is_truthy(&c) {
                        break;
                    }
                    self.eval_block(body)?;
                    match self.loop_signal {
                        LoopSignal::Break => {
                            self.loop_signal = LoopSignal::None;
                            break;
                        }
                        LoopSignal::Continue => self.loop_signal = LoopSignal::None,
                        LoopSignal::None => {}
                    }
                    if self.pending_return.is_some() {
                        break;
                    }
                }
                Ok(Value::Void)
            }
            ExprKind::Break => {
                self.loop_signal = LoopSignal::Break;
                Ok(Value::Void)
            }
            ExprKind::Continue => {
                self.loop_signal = LoopSignal::Continue;
                Ok(Value::Void)
            }
            ExprKind::Call { callee, args } => {
                let callee_value = self.eval(callee)?;
                let mut evaluated = Vec::with_capacity(args.len());
                for a in args {
                    evaluated.push(self.eval(a)?);
                }
                self.call_value(callee_value, evaluated)
            }
        }
    }

    fn call_value(&mut self, callee: Value, args: Vec<Value>) -> Result<Value, RuntimeError> {
        match callee {
            Value::Function(func) => self.call_function(&func, args),
            Value::Lambda(lam) => self.call_lambda(&lam, args),
            Value::BoundMethod { receiver, method } => {
                self.call_method(*receiver, &method, args)
            }
            other => Err(RuntimeError {
                message: format!("value is not callable: {other}"),
            }),
        }
    }

    fn call_lambda(&mut self, lam: &LambdaValue, args: Vec<Value>) -> Result<Value, RuntimeError> {
        if args.len() != lam.params.len() {
            return Err(RuntimeError {
                message: format!(
                    "lambda expects {} arguments, got {}",
                    lam.params.len(),
                    args.len()
                ),
            });
        }
        let saved = std::mem::replace(&mut self.env, lam.env.clone());
        for (p, v) in lam.params.iter().zip(args.into_iter()) {
            self.env.insert(p.name.name.clone(), v);
        }
        let saved_return = self.pending_return.take();
        let result = match &lam.body {
            FunctionBody::Block(b) => {
                let ctl = self.eval_block_ctl(b)?;
                self.pending_return = None;
                ctl.into_value()
            }
            FunctionBody::Expr(e) => self.eval(e)?,
        };
        self.pending_return = saved_return;
        self.env = saved;
        Ok(result)
    }

    fn call_method(
        &mut self,
        receiver: Value,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match (receiver, method) {
            // ── 배열 메서드 ──
            (Value::Array(items), "map") => {
                let fn_val = args
                    .into_iter()
                    .next()
                    .ok_or_else(|| RuntimeError {
                        message: "map expects a function".into(),
                    })?;
                let mut out = Vec::with_capacity(items.len());
                for v in items {
                    let r = self.call_value(fn_val.clone(), vec![v])?;
                    out.push(r);
                }
                Ok(Value::Array(out))
            }
            (Value::Array(items), "filter") => {
                let fn_val = args
                    .into_iter()
                    .next()
                    .ok_or_else(|| RuntimeError {
                        message: "filter expects a function".into(),
                    })?;
                let mut out = Vec::new();
                for v in items {
                    let r = self.call_value(fn_val.clone(), vec![v.clone()])?;
                    if is_truthy(&r) {
                        out.push(v);
                    }
                }
                Ok(Value::Array(out))
            }
            (Value::Array(items), "reduce") => {
                // reduce(init, fn)
                let mut iter = args.into_iter();
                let init = iter.next().ok_or_else(|| RuntimeError {
                    message: "reduce expects initial value and function".into(),
                })?;
                let fn_val = iter.next().ok_or_else(|| RuntimeError {
                    message: "reduce expects initial value and function".into(),
                })?;
                let mut acc = init;
                for v in items {
                    acc = self.call_value(fn_val.clone(), vec![acc, v])?;
                }
                Ok(acc)
            }
            (Value::Array(mut items), "push") => {
                for a in args {
                    items.push(a);
                }
                Ok(Value::Array(items))
            }
            (Value::Array(a), "concat") => {
                let mut out = a;
                for arg in args {
                    if let Value::Array(b) = arg {
                        out.extend(b);
                    } else {
                        return Err(RuntimeError {
                            message: "concat expects array argument".into(),
                        });
                    }
                }
                Ok(Value::Array(out))
            }
            (Value::Array(items), "join") => {
                let sep = match args.into_iter().next() {
                    Some(Value::Str(s)) => s,
                    _ => String::new(),
                };
                let parts: Vec<String> = items.iter().map(|v| format!("{v}")).collect();
                Ok(Value::Str(parts.join(&sep)))
            }
            // ── 문자열 메서드 ──
            (Value::Str(s), "toLowerCase") => Ok(Value::Str(s.to_lowercase())),
            (Value::Str(s), "toUpperCase") => Ok(Value::Str(s.to_uppercase())),
            (Value::Str(s), "contains") => {
                let needle = match args.into_iter().next() {
                    Some(Value::Str(v)) => v,
                    _ => {
                        return Err(RuntimeError {
                            message: "contains expects string argument".into(),
                        })
                    }
                };
                Ok(Value::Bool(s.contains(&needle)))
            }
            (Value::Str(s), "replace") => {
                let mut it = args.into_iter();
                let from = match it.next() {
                    Some(Value::Str(v)) => v,
                    _ => {
                        return Err(RuntimeError {
                            message: "replace expects (from, to) strings".into(),
                        })
                    }
                };
                let to = match it.next() {
                    Some(Value::Str(v)) => v,
                    _ => {
                        return Err(RuntimeError {
                            message: "replace expects (from, to) strings".into(),
                        })
                    }
                };
                Ok(Value::Str(s.replace(&from, &to)))
            }
            (recv, m) => Err(RuntimeError {
                message: format!("no method `{m}` on {recv}"),
            }),
        }
    }

    fn call_function(
        &mut self,
        func: &FunctionStmt,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        if args.len() != func.params.len() {
            return Err(RuntimeError {
                message: format!(
                    "function `{}` expects {} arguments, got {}",
                    func.name.name,
                    func.params.len(),
                    args.len()
                ),
            });
        }
        // 함수 스코프 — 현재 환경을 저장/복원하여 간단 샌드박스.
        let saved = std::mem::take(&mut self.env);
        // 호출자의 함수 정의를 유지해 재귀 호출이 가능하도록 함수들을 복사.
        for (k, v) in &saved {
            if matches!(v, Value::Function(_)) {
                self.env.insert(k.clone(), v.clone());
            }
        }
        for (p, v) in func.params.iter().zip(args.into_iter()) {
            self.env.insert(p.name.name.clone(), v);
        }
        // 이 호출 경계까지 pending_return 전파를 잠시 격리.
        let saved_return = self.pending_return.take();
        let result_value = match &func.body {
            FunctionBody::Block(b) => {
                let ctl = self.eval_block_ctl(b)?;
                // return이 발생했으면 pending_return도 클리어.
                self.pending_return = None;
                ctl.into_value()
            }
            FunctionBody::Expr(e) => self.eval(e)?,
        };
        self.pending_return = saved_return;
        self.env = saved;
        Ok(result_value)
    }

    fn eval_block_ctl(&mut self, block: &Block) -> Result<ControlFlow, RuntimeError> {
        let last = block.stmts.len().saturating_sub(1);
        let mut final_value = Value::Void;
        for (i, s) in block.stmts.iter().enumerate() {
            let is_last = i == last;
            match s {
                Stmt::Let(l) => {
                    let v = self.eval(&l.init)?;
                    self.env.insert(l.name.name.clone(), v);
                }
                Stmt::Const(c) => {
                    let v = self.eval(&c.init)?;
                    self.env.insert(c.name.name.clone(), v);
                }
                Stmt::Function(f) => {
                    self.env
                        .insert(f.name.name.clone(), Value::Function(Rc::new((**f).clone())));
                }
                Stmt::Struct(_) => {
                    // noop — 타입 정보 기록만 필요하며 MVP에서는 생략.
                }
                Stmt::Return(r) => {
                    let v = match &r.value {
                        Some(e) => self.eval(e)?,
                        None => Value::Void,
                    };
                    // 상위 블록/호출에 전파.
                    self.pending_return = Some(v.clone());
                    return Ok(ControlFlow::Return(v));
                }
                Stmt::Expr(e) => {
                    let v = self.eval(e)?;
                    if let Some(ret) = self.pending_return.clone() {
                        return Ok(ControlFlow::Return(ret));
                    }
                    // break/continue가 설정됐다면 블록 실행을 멈춘다.
                    if self.loop_signal != LoopSignal::None {
                        return Ok(ControlFlow::Normal(Value::Void));
                    }
                    if is_last {
                        final_value = v;
                    }
                }
            }
        }
        Ok(ControlFlow::Normal(final_value))
    }

    fn eval_block(&mut self, block: &Block) -> Result<Value, RuntimeError> {
        Ok(self.eval_block_ctl(block)?.into_value())
    }

    /// iter 표현식에서 `start..end` / `start..=end`를 해석.
    /// MVP: range 리터럴(이항 연산 `..`/`..=`)만 지원. AST에는 별도 타입이 없어
    /// 표현식 경로로 `start`와 `end`를 찾아 런타임에 평가한다.
    fn interpret_range(&mut self, expr: &Expr) -> Result<(i64, i64, bool), RuntimeError> {
        // `a..b`/`a..=b`는 AST에서 이항 연산이 아니라 범위 토큰이므로,
        // Pattern의 Range처럼 처리되지 않는다. MVP에서는 파서가 Range를
        // 표현식으로 노출하지 않으므로 여기서는 단일 이항 연산을 가정하지
        // 않고, `iter`가 실제로는 Binary {op: Range...} 같은 형태가 아니다.
        // 대안: iter 식이 이항 비교/산술이 아니라 범위라면, ExprKind에 Range를
        // 추가해야 한다. 여기서는 그 추가가 되어있다고 가정하지 않고,
        // expr의 kind를 직접 확인한다.
        if let ExprKind::Range { start, end, inclusive } = &expr.kind {
            let s = self.eval(start)?;
            let e = self.eval(end)?;
            match (s, e) {
                (Value::Int(a), Value::Int(b)) => return Ok((a, b, *inclusive)),
                _ => {
                    return Err(RuntimeError {
                        message: "for loop range must be integer".into(),
                    });
                }
            }
        }
        Err(RuntimeError {
            message: "for loop requires a range expression (a..b or a..=b)".into(),
        })
    }

    fn pattern_matches(&mut self, pat: &Pattern, value: &Value) -> Result<bool, RuntimeError> {
        Ok(match pat {
            Pattern::Wildcard => true,
            Pattern::Literal(lit) => {
                let expected = self.eval(lit)?;
                values_equal(&expected, value)
            }
            Pattern::Range {
                start,
                end,
                inclusive,
            } => {
                let lo = self.eval(start)?;
                let hi = self.eval(end)?;
                match (value, lo, hi) {
                    (Value::Int(v), Value::Int(lo), Value::Int(hi)) => {
                        if *inclusive {
                            *v >= lo && *v <= hi
                        } else {
                            *v >= lo && *v < hi
                        }
                    }
                    _ => false,
                }
            }
            Pattern::Guard(expr) => {
                // `$`는 현재 값(value)으로 치환해 평가. MVP: 단순히 일시적
                // 바인딩으로 `$`를 환경에 주입하여 expr 평가.
                let previous = self.env.insert("$".to_string(), value.clone());
                let result = self.eval(expr)?;
                if let Some(p) = previous {
                    self.env.insert("$".to_string(), p);
                } else {
                    self.env.remove("$");
                }
                is_truthy(&result)
            }
        })
    }

    fn println(&mut self, v: &Value) -> Result<(), RuntimeError> {
        writeln!(self.writer, "{v}").map_err(|e| RuntimeError {
            message: format!("io error: {e}"),
        })
    }
}

/// void-scope 자동 출력을 피해야 하는 표현식인지.
/// 부수효과 전용 문법(@out 호출, 대입, 제어 흐름 블록, 함수 호출)은
/// 자동 출력 제외.
fn has_side_effect(expr: &Expr) -> bool {
    matches!(
        &expr.kind,
        ExprKind::Domain { .. }
            | ExprKind::Assign { .. }
            | ExprKind::Block(_)
            | ExprKind::If { .. }
            | ExprKind::When { .. }
            | ExprKind::Call { .. }
    )
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

/// 문자열 보간에 쓰일 값의 사용자 표시.
fn value_to_display(v: &Value) -> String {
    match v {
        Value::Str(s) => s.clone(),
        _ => format!("{v}"),
    }
}

fn is_truthy(v: &Value) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Void => false,
        Value::Int(n) => *n != 0,
        Value::Float(f) => *f != 0.0,
        Value::Str(s) => !s.is_empty(),
        Value::Function(_) | Value::Lambda(_) | Value::BoundMethod { .. } => true,
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Float(x), Value::Float(y)) => (x - y).abs() < f64::EPSILON,
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Void, Value::Void) => true,
        (Value::Function(a), Value::Function(b)) => Rc::ptr_eq(a, b),
        (Value::Lambda(a), Value::Lambda(b)) => Rc::ptr_eq(a, b),
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| values_equal(x, y))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter().all(|(k, v)| {
                    b.iter().any(|(k2, v2)| k == k2 && values_equal(v, v2))
                })
        }
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

    #[test]
    fn string_interpolation() {
        let out = run_str(
            r#"
            let name: string = "Alice"
            @out "Hello, {name}!"
            "#,
        ).unwrap();
        assert_eq!(out, "Hello, Alice!\n");
    }

    #[test]
    fn string_interp_with_arithmetic() {
        let out = run_str(
            r#"
            let x: int = 7
            @out "answer: {x * 6}"
            "#,
        ).unwrap();
        assert_eq!(out, "answer: 42\n");
    }

    #[test]
    fn string_escapes_runtime() {
        let out = run_str(r#"@out "a\tb\nc""#).unwrap();
        assert_eq!(out, "a\tb\nc\n");
    }

    #[test]
    fn brace_escape_preserved_in_output() {
        let out = run_str(r#"@out "literal \{42\}""#).unwrap();
        assert_eq!(out, "literal {42}\n");
    }

    #[test]
    fn if_true_branch() {
        let out = run_str(
            r#"
            let n: int = 5
            if n > 0 {
              @out "positive"
            } else {
              @out "non-positive"
            }
            "#,
        ).unwrap();
        assert_eq!(out, "positive\n");
    }

    #[test]
    fn if_else_branch() {
        let out = run_str(
            r#"
            let n: int = -3
            if n > 0 {
              @out "positive"
            } else {
              @out "non-positive"
            }
            "#,
        ).unwrap();
        assert_eq!(out, "non-positive\n");
    }

    #[test]
    fn else_if_chain() {
        let out = run_str(
            r#"
            let n: int = 0
            if n > 0 {
              @out "positive"
            } else if n < 0 {
              @out "negative"
            } else {
              @out "zero"
            }
            "#,
        ).unwrap();
        assert_eq!(out, "zero\n");
    }

    #[test]
    fn when_literal_match() {
        let out = run_str(
            r#"
            let x: int = 2
            when x {
              1 -> @out "one"
              2 -> @out "two"
              _ -> @out "many"
            }
            "#,
        ).unwrap();
        assert_eq!(out, "two\n");
    }

    #[test]
    fn when_wildcard_fallback() {
        let out = run_str(
            r#"
            let x: int = 99
            when x {
              1 -> @out "one"
              _ -> @out "other"
            }
            "#,
        ).unwrap();
        assert_eq!(out, "other\n");
    }

    #[test]
    fn when_range_inclusive() {
        let out = run_str(
            r#"
            let x: int = 5
            when x {
              0..=9 -> @out "digit"
              _ -> @out "big"
            }
            "#,
        ).unwrap();
        assert_eq!(out, "digit\n");
    }

    #[test]
    fn when_guard_with_dollar() {
        let out = run_str(
            r#"
            let x: int = 7
            when x {
              $ > 5 -> @out "gt5"
              _ -> @out "le5"
            }
            "#,
        ).unwrap();
        assert_eq!(out, "gt5\n");
    }

    #[test]
    fn mutable_reassign() {
        let out = run_str(
            r#"
            let mut count: int = 0
            count = count + 1
            count = count + 1
            @out count
            "#,
        ).unwrap();
        assert_eq!(out, "2\n");
    }

    // ── 함수 선언 / 호출 ──

    #[test]
    fn function_call_basic() {
        let out = run_str(
            r#"
            function add(a: int, b: int): int -> {
              a + b
            }
            @out add(2, 3)
            "#,
        ).unwrap();
        assert_eq!(out, "5\n");
    }

    #[test]
    fn function_expression_body() {
        let out = run_str(
            r#"
            function double(x: int): int -> x * 2
            @out double(7)
            "#,
        ).unwrap();
        assert_eq!(out, "14\n");
    }

    #[test]
    fn function_with_explicit_return() {
        let out = run_str(
            r#"
            function abs(x: int): int -> {
              if x < 0 { return -x }
              x
            }
            @out abs(-4)
            @out abs(9)
            "#,
        ).unwrap();
        assert_eq!(out, "4\n9\n");
    }

    #[test]
    fn recursive_function() {
        let out = run_str(
            r#"
            function fact(n: int): int -> {
              if n <= 1 { return 1 }
              n * fact(n - 1)
            }
            @out fact(5)
            "#,
        ).unwrap();
        assert_eq!(out, "120\n");
    }

    // ── 람다 / 배열·문자열 메서드 ──

    #[test]
    fn lambda_literal_call() {
        let out = run_str(
            r#"
            let double = (x) -> x * 2
            @out double(5)
            "#,
        )
        .unwrap();
        assert_eq!(out, "10\n");
    }

    #[test]
    fn array_map_doubles() {
        let out = run_str(
            r#"
            let xs: int[] = [1, 2, 3]
            @out xs.map((x) -> x * 10)
            "#,
        )
        .unwrap();
        assert_eq!(out, "[10, 20, 30]\n");
    }

    #[test]
    fn array_filter_evens() {
        let out = run_str(
            r#"
            let xs: int[] = [1, 2, 3, 4, 5]
            @out xs.filter((x) -> x % 2 == 0)
            "#,
        )
        .unwrap();
        assert_eq!(out, "[2, 4]\n");
    }

    #[test]
    fn array_reduce_sum() {
        let out = run_str(
            r#"
            let xs: int[] = [1, 2, 3, 4, 5]
            @out xs.reduce(0, (acc, x) -> acc + x)
            "#,
        )
        .unwrap();
        assert_eq!(out, "15\n");
    }

    #[test]
    fn array_concat_and_push() {
        let out = run_str(
            r#"
            let a: int[] = [1, 2]
            let b: int[] = [3, 4]
            @out a.concat(b).push(5)
            "#,
        )
        .unwrap();
        assert_eq!(out, "[1, 2, 3, 4, 5]\n");
    }

    #[test]
    fn array_join() {
        let out = run_str(
            r#"
            let parts: int[] = [1, 2, 3]
            @out parts.join(", ")
            "#,
        )
        .unwrap();
        assert_eq!(out, "1, 2, 3\n");
    }

    #[test]
    fn string_methods() {
        let out = run_str(
            r#"
            let s: string = "Hello, Orv"
            @out s.toLowerCase()
            @out s.toUpperCase()
            @out s.contains("Orv")
            @out s.replace("Orv", "World")
            "#,
        )
        .unwrap();
        assert_eq!(out, "hello, orv\nHELLO, ORV\ntrue\nHello, World\n");
    }

    #[test]
    fn lambda_closure_captures_env() {
        let out = run_str(
            r#"
            let base: int = 100
            let addBase = (x) -> x + base
            @out addBase(5)
            "#,
        )
        .unwrap();
        assert_eq!(out, "105\n");
    }

    #[test]
    fn chained_array_pipeline() {
        let out = run_str(
            r#"
            let xs: int[] = [1, 2, 3, 4, 5]
            let result: int = xs
              .filter((x) -> x % 2 == 1)
              .map((x) -> x * 10)
              .reduce(0, (acc, x) -> acc + x)
            @out result
            "#,
        )
        .unwrap();
        assert_eq!(out, "90\n");
    }

    // ── struct / 객체 리터럴 ──

    #[test]
    fn struct_decl_and_object_field_access() {
        let out = run_str(
            r#"
            struct User {
              name: string
              age: int
            }
            let u: User = { name: "Alice", age: 30 }
            @out u.name
            @out u.age
            "#,
        )
        .unwrap();
        assert_eq!(out, "Alice\n30\n");
    }

    #[test]
    fn nested_object_fields() {
        let out = run_str(
            r#"
            let post = { title: "Hi", author: { name: "Bob" } }
            @out post.title
            @out post.author.name
            "#,
        )
        .unwrap();
        assert_eq!(out, "Hi\nBob\n");
    }

    #[test]
    fn object_in_string_interpolation() {
        let out = run_str(
            r#"
            let u = { name: "Orv", score: 100 }
            @out "{u.name}: {u.score}"
            "#,
        )
        .unwrap();
        assert_eq!(out, "Orv: 100\n");
    }

    #[test]
    fn missing_field_errors() {
        let err = run_str(
            r#"
            let u = { name: "Alice" }
            @out u.age
            "#,
        )
        .unwrap_err();
        assert!(err.message.contains("no field"));
    }

    // ── 배열 / 인덱싱 / 필드 ──

    #[test]
    fn array_literal_and_length() {
        let out = run_str(
            r#"
            let xs: int[] = [10, 20, 30]
            @out xs.length
            "#,
        )
        .unwrap();
        assert_eq!(out, "3\n");
    }

    #[test]
    fn array_index_access() {
        let out = run_str(
            r#"
            let xs: int[] = [100, 200, 300]
            @out xs[0]
            @out xs[2]
            @out xs[-1]
            "#,
        )
        .unwrap();
        assert_eq!(out, "100\n300\n300\n");
    }

    #[test]
    fn array_out_of_bounds_errors() {
        let err = run_str(
            r#"
            let xs: int[] = [1, 2]
            @out xs[5]
            "#,
        )
        .unwrap_err();
        assert!(err.message.contains("out of bounds"));
    }

    #[test]
    fn string_length_and_index() {
        let out = run_str(
            r#"
            let s: string = "Orv"
            @out s.length
            @out s[0]
            @out s[2]
            "#,
        )
        .unwrap();
        assert_eq!(out, "3\nO\nv\n");
    }

    #[test]
    fn for_iterates_and_sums_array_via_index() {
        let out = run_str(
            r#"
            let xs: int[] = [5, 10, 15, 20]
            let mut total: int = 0
            for i in 0..xs.length {
              total = total + xs[i]
            }
            @out total
            "#,
        )
        .unwrap();
        assert_eq!(out, "50\n");
    }

    // ── 루프 ──

    #[test]
    fn for_range_exclusive() {
        let out = run_str(
            r#"
            for i in 0..3 {
              @out i
            }
            "#,
        ).unwrap();
        assert_eq!(out, "0\n1\n2\n");
    }

    #[test]
    fn for_range_inclusive() {
        let out = run_str(
            r#"
            for i in 1..=3 {
              @out i
            }
            "#,
        ).unwrap();
        assert_eq!(out, "1\n2\n3\n");
    }

    #[test]
    fn while_with_counter() {
        let out = run_str(
            r#"
            let mut n: int = 0
            while n < 3 {
              @out n
              n = n + 1
            }
            "#,
        ).unwrap();
        assert_eq!(out, "0\n1\n2\n");
    }

    #[test]
    fn break_exits_loop() {
        let out = run_str(
            r#"
            for i in 0..10 {
              if i == 2 { break }
              @out i
            }
            "#,
        ).unwrap();
        assert_eq!(out, "0\n1\n");
    }

    #[test]
    fn continue_skips_iteration() {
        let out = run_str(
            r#"
            for i in 0..5 {
              if i == 2 { continue }
              @out i
            }
            "#,
        ).unwrap();
        assert_eq!(out, "0\n1\n3\n4\n");
    }

    #[test]
    fn nested_for_loops() {
        let out = run_str(
            r#"
            for i in 0..2 {
              for j in 0..2 {
                @out "{i},{j}"
              }
            }
            "#,
        ).unwrap();
        assert_eq!(out, "0,0\n0,1\n1,0\n1,1\n");
    }

    #[test]
    fn function_arity_mismatch() {
        let err = run_str(
            r#"
            function f(a: int, b: int): int -> a + b
            @out f(1)
            "#,
        ).unwrap_err();
        assert!(err.message.contains("expects 2 arguments"));
    }

    #[test]
    fn block_value_from_last_expr() {
        // if 표현식 값 사용
        let out = run_str(
            r#"
            let n: int = 5
            let label: string = if n > 0 { "plus" } else { "neg" }
            @out label
            "#,
        ).unwrap();
        assert_eq!(out, "plus\n");
    }
}
