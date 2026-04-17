//! tree-walking 인터프리터 — HIR 버전.
//!
//! SPEC §0 에서 채택한 V8 Ignition 모델의 "영구 dev-loop 실행 경로" 다.
//! [`orv_analyzer::lower`] 가 만든 [`HirProgram`] 을 직접 평가한다. 타입
//! 검사는 아직 붙지 않았으므로 런타임에서 값 타입을 확인해 에러를 낸다.
//!
//! # 환경 모델
//! 환경은 `HashMap<NameId, Value>` 다. [`orv_resolve`] 가 모든 식별자에
//! 유일한 `NameId` 를 부여하므로 문자열 기반 조회가 사라진다. `$` 가드는
//! 스코프 바인딩이 아니므로 별도 슬롯 [`Interp::dollar`] 로 관리한다.
//!
//! # 함수 호출 규칙 (커밋 21 의 동작을 유지)
//! 호출 시점의 환경 전체를 복제해 파라미터로 오버레이한 뒤, 호출이 끝나면
//! 원본으로 복원한다. 이렇게 하면 함수 본문이 전역 선언을 볼 수 있으면서도
//! 본문에서 생긴 로컬은 호출자에 새지 않는다. 정밀한 capture 분석은 이후
//! 최적화로 미룬다.

use orv_hir::{
    BinaryOp, HirBlock, HirExpr, HirExprKind, HirFunctionBody, HirFunctionStmt, HirParam,
    HirPattern, HirProgram, HirStmt, HirStringSegment, NameId, UnaryOp,
};
use std::collections::HashMap;
use std::fmt;
use std::io::Write;
use std::rc::Rc;

/// 런타임 에러.
///
/// `thrown` 필드에 사용자 `throw` 값이 담긴 경우 try/catch 가 잡아낼 수
/// 있다. `native` 에러는 인터프리터 내부 오류로 catch 되지 않는다.
#[derive(Clone, Debug, Default)]
pub struct RuntimeError {
    /// 사람이 읽을 메시지.
    pub message: String,
    /// `throw` 로 발생한 사용자 에러면 그 값, 아니면 None.
    pub thrown: Option<Value>,
}

impl RuntimeError {
    /// 인터프리터 내부 에러 — catch 불가.
    pub(crate) fn native(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            thrown: None,
        }
    }

    /// `throw` 문으로 발생한 사용자 에러 — try/catch 로 처리 가능.
    pub(crate) fn thrown(value: Value) -> Self {
        Self {
            message: format!("{value}"),
            thrown: Some(value),
        }
    }
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.thrown {
            Some(v) => write!(f, "uncaught: {v}"),
            None => write!(f, "runtime error: {}", self.message),
        }
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
    /// 사용자 정의 함수.
    Function(Rc<HirFunctionStmt>),
    /// 람다 — 파라미터와 본문 + 캡처 환경.
    Lambda(Rc<LambdaValue>),
    /// 바인딩된 내장 메서드 — `arr.map` 처럼 receiver 에 붙은 함수. 메서드
    /// 이름은 값 타입 기반 dispatch 이므로 `NameId` 가 아닌 문자열을 유지.
    BoundMethod {
        /// 수신자 값.
        receiver: Box<Value>,
        /// 메서드 이름.
        method: String,
    },
    /// 배열.
    Array(Vec<Value>),
    /// 오브젝트 — 필드 이름 순서 유지. 필드명은 구조체 멤버이므로 문자열.
    Object(Vec<(String, Value)>),
}

/// 람다 값 — 파라미터 + 본문 + 캡처된 환경 스냅샷.
#[derive(Clone, Debug)]
pub struct LambdaValue {
    /// 파라미터.
    pub params: Vec<HirParam>,
    /// 본문.
    pub body: HirFunctionBody,
    /// 선언 시점의 환경 스냅샷(클로저).
    pub env: HashMap<NameId, Value>,
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
    None,
    Continue,
    Break,
}

/// HIR 프로그램을 stdout 에 실행한다.
///
/// # Errors
/// 실행 중 타입 불일치, 인덱스 초과, 메서드 미지원 등이 발생하면 반환한다.
pub fn run(program: &HirProgram) -> Result<(), RuntimeError> {
    let mut stdout = std::io::stdout().lock();
    run_with_writer(program, &mut stdout)
}

/// 테스트 가능한 버전 — 임의의 `Write` 에 출력한다.
///
/// # Errors
/// `run` 과 동일.
pub fn run_with_writer<W: Write>(program: &HirProgram, writer: &mut W) -> Result<(), RuntimeError> {
    let mut interp = Interp::new(writer);
    interp.run(program)
}

struct Interp<'w, W: Write> {
    env: HashMap<NameId, Value>,
    writer: &'w mut W,
    pending_return: Option<Value>,
    loop_signal: LoopSignal,
    /// when 가드의 `$` — 스코프 바인딩이 아니므로 별도 슬롯에 보관한다.
    dollar: Option<Value>,
}

impl<'w, W: Write> Interp<'w, W> {
    fn new(writer: &'w mut W) -> Self {
        Self {
            env: HashMap::new(),
            writer,
            pending_return: None,
            loop_signal: LoopSignal::None,
            dollar: None,
        }
    }

    fn run(&mut self, program: &HirProgram) -> Result<(), RuntimeError> {
        let last_idx = program.items.len().saturating_sub(1);
        for (idx, stmt) in program.items.iter().enumerate() {
            let is_last = idx == last_idx;
            self.exec_stmt(stmt, is_last)?;
        }
        Ok(())
    }

    fn exec_stmt(&mut self, stmt: &HirStmt, is_last: bool) -> Result<(), RuntimeError> {
        match stmt {
            HirStmt::Let(l) => {
                let v = self.eval(&l.init)?;
                self.env.insert(l.name.id, v);
            }
            HirStmt::Const(c) => {
                let v = self.eval(&c.init)?;
                self.env.insert(c.name.id, v);
            }
            HirStmt::Function(f) => {
                self.env
                    .insert(f.name.id, Value::Function(Rc::new((**f).clone())));
            }
            HirStmt::Struct(_) => {
                // MVP: 타입 정보만 필요하며 런타임은 noop. 이후 커밋에서 확장.
            }
            HirStmt::Return(_) => {
                return Err(RuntimeError::native("`return` outside of a function"));
            }
            HirStmt::Expr(e) => {
                let v = self.eval(e)?;
                // SPEC §12.2 — void scope 에서 마지막이 아닌 표현식은 자동 출력.
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

    fn eval(&mut self, expr: &HirExpr) -> Result<Value, RuntimeError> {
        match &expr.kind {
            HirExprKind::Integer(s) => s
                .replace('_', "")
                .parse::<i64>()
                .map(Value::Int)
                .map_err(|_| RuntimeError::native(format!("invalid integer literal `{s}`"))),
            HirExprKind::Float(s) => s
                .replace('_', "")
                .parse::<f64>()
                .map(Value::Float)
                .map_err(|_| RuntimeError::native(format!("invalid float literal `{s}`"))),
            HirExprKind::String(segments) => {
                let mut out = String::new();
                for seg in segments {
                    match seg {
                        HirStringSegment::Str(lit) => out.push_str(lit),
                        HirStringSegment::Interp(e) => {
                            let v = self.eval(e)?;
                            out.push_str(&value_to_display(&v));
                        }
                    }
                }
                Ok(Value::Str(out))
            }
            HirExprKind::True => Ok(Value::Bool(true)),
            HirExprKind::False => Ok(Value::Bool(false)),
            HirExprKind::Void => Ok(Value::Void),
            HirExprKind::Ident(id) => self.lookup(id.id, &id.name),
            HirExprKind::Paren(inner) => self.eval(inner),
            HirExprKind::Unary { op, expr } => {
                let v = self.eval(expr)?;
                apply_unary(*op, v)
            }
            HirExprKind::Binary { op, lhs, rhs } => {
                let l = self.eval(lhs)?;
                let r = self.eval(rhs)?;
                apply_binary(*op, l, r)
            }
            HirExprKind::Domain { name, args, .. } => {
                if name == "out" {
                    if let Some(a) = args.first() {
                        let v = self.eval(a)?;
                        self.println(&v)?;
                    } else {
                        self.println(&Value::Str(String::new()))?;
                    }
                    Ok(Value::Void)
                } else {
                    Err(RuntimeError::native(format!(
                        "unsupported domain `@{name}` in MVP interpreter"
                    )))
                }
            }
            HirExprKind::Block(b) => self.eval_block(b),
            HirExprKind::If {
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
            HirExprKind::When { scrutinee, arms } => {
                let value = self.eval(scrutinee)?;
                for arm in arms {
                    if self.pattern_matches(&arm.pattern, &value)? {
                        return self.eval(&arm.body);
                    }
                }
                Ok(Value::Void)
            }
            HirExprKind::Assign { target, value } => {
                if !self.env.contains_key(&target.id) {
                    // resolve 가 허용한 참조만 여기까지 오지만, 방어적 체크.
                    return Err(RuntimeError::native(format!(
                        "cannot assign to undefined `{}`",
                        target.name
                    )));
                }
                let v = self.eval(value)?;
                self.env.insert(target.id, v.clone());
                Ok(v)
            }
            HirExprKind::For { var, iter, body } => {
                let (lo, hi, incl) = self.interpret_range(iter)?;
                let mut i = lo;
                while if incl { i <= hi } else { i < hi } {
                    self.env.insert(var.id, Value::Int(i));
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
            HirExprKind::Range { .. } => Err(RuntimeError::native(
                "range expression can only be used in `for ... in` or `when` patterns",
            )),
            HirExprKind::Array(items) => {
                let mut values = Vec::with_capacity(items.len());
                for e in items {
                    values.push(self.eval(e)?);
                }
                Ok(Value::Array(values))
            }
            HirExprKind::Object(fields) => {
                let mut out = Vec::with_capacity(fields.len());
                for f in fields {
                    let v = self.eval(&f.value)?;
                    out.push((f.name.clone(), v));
                }
                Ok(Value::Object(out))
            }
            HirExprKind::Index { target, index } => {
                let t = self.eval(target)?;
                let i = self.eval(index)?;
                let Value::Int(idx) = i else {
                    return Err(RuntimeError::native("index must be an integer"));
                };
                match t {
                    Value::Array(items) => {
                        let n = i64::try_from(items.len()).unwrap_or(i64::MAX);
                        let actual = if idx < 0 { idx + n } else { idx };
                        if actual < 0 || actual >= n {
                            return Err(RuntimeError::native(format!(
                                "index {idx} out of bounds for length {n}"
                            )));
                        }
                        Ok(items[actual as usize].clone())
                    }
                    Value::Str(s) => {
                        let chars: Vec<char> = s.chars().collect();
                        let n = i64::try_from(chars.len()).unwrap_or(i64::MAX);
                        let actual = if idx < 0 { idx + n } else { idx };
                        if actual < 0 || actual >= n {
                            return Err(RuntimeError::native(format!(
                                "index {idx} out of bounds for length {n}"
                            )));
                        }
                        Ok(Value::Str(chars[actual as usize].to_string()))
                    }
                    other => Err(RuntimeError::native(format!("cannot index into {other}"))),
                }
            }
            HirExprKind::Field { target, field, .. } => {
                let t = self.eval(target)?;
                let name = field.as_str();
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
                        .find(|(k, _)| k == field)
                        .map(|(_, v)| v.clone())
                        .ok_or_else(|| {
                            RuntimeError::native(format!("no field `{field}` on object"))
                        }),
                    _ => Err(RuntimeError::native(format!("no field `{field}` on {t}"))),
                }
            }
            HirExprKind::Lambda { params, body } => Ok(Value::Lambda(Rc::new(LambdaValue {
                params: params.clone(),
                body: (**body).clone(),
                env: self.env.clone(),
            }))),
            HirExprKind::Throw(inner) => {
                let v = self.eval(inner)?;
                Err(RuntimeError::thrown(v))
            }
            HirExprKind::Try { try_block, catch } => match self.eval_block(try_block) {
                Ok(v) => Ok(v),
                Err(e) if e.thrown.is_some() => {
                    let Some(clause) = catch else {
                        return Err(e);
                    };
                    let thrown = e.thrown.clone().unwrap();
                    if let Some(name) = &clause.binding {
                        self.env.insert(name.id, thrown);
                    }
                    self.eval_block(&clause.body)
                }
                Err(e) => Err(e),
            },
            HirExprKind::While { cond, body } => {
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
            HirExprKind::Break => {
                self.loop_signal = LoopSignal::Break;
                Ok(Value::Void)
            }
            HirExprKind::Continue => {
                self.loop_signal = LoopSignal::Continue;
                Ok(Value::Void)
            }
            HirExprKind::Call { callee, args } => {
                let callee_value = self.eval(callee)?;
                let mut evaluated = Vec::with_capacity(args.len());
                for a in args {
                    evaluated.push(self.eval(a)?);
                }
                self.call_value(callee_value, evaluated)
            }
        }
    }

    fn lookup(&self, id: NameId, debug_name: &str) -> Result<Value, RuntimeError> {
        // `$` 가드는 스코프 바인딩이 아니므로 NameId 가 없다. resolver 는 이를
        // 건너뛰므로 `Ident("$")` 가 여기 도달할 수 있다.
        if debug_name == "$" {
            if let Some(v) = &self.dollar {
                return Ok(v.clone());
            }
            return Err(RuntimeError::native("`$` used outside of a when guard"));
        }
        self.env.get(&id).cloned().ok_or_else(|| {
            RuntimeError::native(format!("undefined variable `{debug_name}`"))
        })
    }

    fn call_value(&mut self, callee: Value, args: Vec<Value>) -> Result<Value, RuntimeError> {
        match callee {
            Value::Function(func) => self.call_function(&func, args),
            Value::Lambda(lam) => self.call_lambda(&lam, args),
            Value::BoundMethod { receiver, method } => self.call_method(*receiver, &method, args),
            other => Err(RuntimeError::native(format!(
                "value is not callable: {other}"
            ))),
        }
    }

    fn call_lambda(&mut self, lam: &LambdaValue, args: Vec<Value>) -> Result<Value, RuntimeError> {
        if args.len() != lam.params.len() {
            return Err(RuntimeError::native(format!(
                "lambda expects {} arguments, got {}",
                lam.params.len(),
                args.len()
            )));
        }
        let saved = std::mem::replace(&mut self.env, lam.env.clone());
        for (p, v) in lam.params.iter().zip(args.into_iter()) {
            self.env.insert(p.name.id, v);
        }
        let saved_return = self.pending_return.take();
        let result = match &lam.body {
            HirFunctionBody::Block(b) => {
                let ctl = self.eval_block_ctl(b)?;
                self.pending_return = None;
                ctl.into_value()
            }
            HirFunctionBody::Expr(e) => self.eval(e)?,
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
                    .ok_or_else(|| RuntimeError::native("map expects a function"))?;
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
                    .ok_or_else(|| RuntimeError::native("filter expects a function"))?;
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
                let mut iter = args.into_iter();
                let init = iter.next().ok_or_else(|| {
                    RuntimeError::native("reduce expects initial value and function")
                })?;
                let fn_val = iter.next().ok_or_else(|| {
                    RuntimeError::native("reduce expects initial value and function")
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
                        return Err(RuntimeError::native("concat expects array argument"));
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
                    _ => return Err(RuntimeError::native("contains expects string argument")),
                };
                Ok(Value::Bool(s.contains(&needle)))
            }
            (Value::Str(s), "replace") => {
                let mut it = args.into_iter();
                let from = match it.next() {
                    Some(Value::Str(v)) => v,
                    _ => return Err(RuntimeError::native("replace expects (from, to) strings")),
                };
                let to = match it.next() {
                    Some(Value::Str(v)) => v,
                    _ => return Err(RuntimeError::native("replace expects (from, to) strings")),
                };
                Ok(Value::Str(s.replace(&from, &to)))
            }
            (recv, m) => Err(RuntimeError::native(format!("no method `{m}` on {recv}"))),
        }
    }

    fn call_function(
        &mut self,
        func: &HirFunctionStmt,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        if args.len() != func.params.len() {
            return Err(RuntimeError::native(format!(
                "function `{}` expects {} arguments, got {}",
                func.name.name,
                func.params.len(),
                args.len()
            )));
        }
        // 함수 호출 스코프 — 커밋 21 에서 확립한 동작: 호출자 환경 전체를
        // 복제해 파라미터로 오버레이하고, 호출 종료 시 원본으로 복원.
        let saved = std::mem::take(&mut self.env);
        self.env = saved.clone();
        for (p, v) in func.params.iter().zip(args.into_iter()) {
            self.env.insert(p.name.id, v);
        }
        let saved_return = self.pending_return.take();
        let result_value = match &func.body {
            HirFunctionBody::Block(b) => {
                let ctl = self.eval_block_ctl(b)?;
                self.pending_return = None;
                ctl.into_value()
            }
            HirFunctionBody::Expr(e) => self.eval(e)?,
        };
        self.pending_return = saved_return;
        self.env = saved;
        Ok(result_value)
    }

    fn eval_block_ctl(&mut self, block: &HirBlock) -> Result<ControlFlow, RuntimeError> {
        let last = block.stmts.len().saturating_sub(1);
        let mut final_value = Value::Void;
        for (i, s) in block.stmts.iter().enumerate() {
            let is_last = i == last;
            match s {
                HirStmt::Let(l) => {
                    let v = self.eval(&l.init)?;
                    self.env.insert(l.name.id, v);
                }
                HirStmt::Const(c) => {
                    let v = self.eval(&c.init)?;
                    self.env.insert(c.name.id, v);
                }
                HirStmt::Function(f) => {
                    self.env
                        .insert(f.name.id, Value::Function(Rc::new((**f).clone())));
                }
                HirStmt::Struct(_) => {}
                HirStmt::Return(r) => {
                    let v = match &r.value {
                        Some(e) => self.eval(e)?,
                        None => Value::Void,
                    };
                    self.pending_return = Some(v.clone());
                    return Ok(ControlFlow::Return(v));
                }
                HirStmt::Expr(e) => {
                    let v = self.eval(e)?;
                    if let Some(ret) = self.pending_return.clone() {
                        return Ok(ControlFlow::Return(ret));
                    }
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

    fn eval_block(&mut self, block: &HirBlock) -> Result<Value, RuntimeError> {
        Ok(self.eval_block_ctl(block)?.into_value())
    }

    fn interpret_range(&mut self, expr: &HirExpr) -> Result<(i64, i64, bool), RuntimeError> {
        if let HirExprKind::Range {
            start,
            end,
            inclusive,
        } = &expr.kind
        {
            let s = self.eval(start)?;
            let e = self.eval(end)?;
            match (s, e) {
                (Value::Int(a), Value::Int(b)) => return Ok((a, b, *inclusive)),
                _ => return Err(RuntimeError::native("for loop range must be integer")),
            }
        }
        Err(RuntimeError::native(
            "for loop requires a range expression (a..b or a..=b)",
        ))
    }

    fn pattern_matches(
        &mut self,
        pat: &HirPattern,
        value: &Value,
    ) -> Result<bool, RuntimeError> {
        Ok(match pat {
            HirPattern::Wildcard => true,
            HirPattern::Literal(lit) => {
                let expected = self.eval(lit)?;
                values_equal(&expected, value)
            }
            HirPattern::Range {
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
            HirPattern::Guard(expr) => {
                // `$` 슬롯에 현재값을 바인딩하고 평가, 끝나면 복원.
                let previous = self.dollar.replace(value.clone());
                let result = self.eval(expr)?;
                self.dollar = previous;
                is_truthy(&result)
            }
        })
    }

    fn println(&mut self, v: &Value) -> Result<(), RuntimeError> {
        writeln!(self.writer, "{v}").map_err(|e| RuntimeError::native(format!("io error: {e}")))
    }
}

/// void-scope 자동 출력을 피해야 하는 표현식인지.
fn has_side_effect(expr: &HirExpr) -> bool {
    matches!(
        &expr.kind,
        HirExprKind::Domain { .. }
            | HirExprKind::Assign { .. }
            | HirExprKind::Block(_)
            | HirExprKind::If { .. }
            | HirExprKind::When { .. }
            | HirExprKind::Call { .. }
    )
}

fn apply_unary(op: UnaryOp, v: Value) -> Result<Value, RuntimeError> {
    match (op, v) {
        (UnaryOp::Not, Value::Bool(b)) => Ok(Value::Bool(!b)),
        (UnaryOp::Neg, Value::Int(i)) => Ok(Value::Int(-i)),
        (UnaryOp::Neg, Value::Float(f)) => Ok(Value::Float(-f)),
        (UnaryOp::BitNot, Value::Int(i)) => Ok(Value::Int(!i)),
        (op, v) => Err(RuntimeError::native(format!(
            "unsupported unary `{op:?}` on {v}"
        ))),
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
        (op, l, r) => Err(RuntimeError::native(format!(
            "unsupported binary `{op:?}` on {l} and {r}"
        ))),
    }
}

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
    use orv_analyzer::lower;
    use orv_diagnostics::FileId;
    use orv_resolve::resolve;
    use orv_syntax::{lex, parse};

    fn run_str(src: &str) -> Result<String, RuntimeError> {
        let lx = lex(src, FileId(0));
        assert!(lx.diagnostics.is_empty(), "lex errors: {:?}", lx.diagnostics);
        let pr = parse(lx.tokens, FileId(0));
        assert!(pr.diagnostics.is_empty(), "parse errors: {:?}", pr.diagnostics);
        let resolved = resolve(&pr.program);
        assert!(
            resolved.diagnostics.is_empty(),
            "resolve errors: {:?}",
            resolved.diagnostics
        );
        let hir = lower(&pr.program, &resolved);
        let mut buf = Vec::new();
        run_with_writer(&hir, &mut buf)?;
        Ok(String::from_utf8(buf).unwrap())
    }

    #[test]
    fn explicit_out_prints_string() {
        let out = run_str(r#"@out "Hello, Orv!""#).unwrap();
        assert_eq!(out, "Hello, Orv!\n");
    }

    #[test]
    fn void_scope_autooutput_string() {
        let out = run_str(
            r#""first"
"second"
@out "third""#,
        )
        .unwrap();
        assert_eq!(out, "first\nsecond\nthird\n");
    }

    #[test]
    fn let_and_ident_reference() {
        let out = run_str(
            r#"
            let name: string = "Alice"
            @out name
            "#,
        )
        .unwrap();
        assert_eq!(out, "Alice\n");
    }

    #[test]
    fn arithmetic_then_out() {
        let out = run_str(
            r#"
            let n: int = 1 + 2 * 3
            @out n
            "#,
        )
        .unwrap();
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
        )
        .unwrap();
        assert_eq!(out, "Hello, World\n");
    }

    #[test]
    fn comparison() {
        let out = run_str("@out 5 > 3").unwrap();
        assert_eq!(out, "true\n");
    }

    #[test]
    fn string_interpolation() {
        let out = run_str(
            r#"
            let name: string = "Alice"
            @out "Hello, {name}!"
            "#,
        )
        .unwrap();
        assert_eq!(out, "Hello, Alice!\n");
    }

    #[test]
    fn string_interp_with_arithmetic() {
        let out = run_str(
            r#"
            let x: int = 7
            @out "answer: {x * 6}"
            "#,
        )
        .unwrap();
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
        )
        .unwrap();
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
        )
        .unwrap();
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
        )
        .unwrap();
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
        )
        .unwrap();
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
        )
        .unwrap();
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
        )
        .unwrap();
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
        )
        .unwrap();
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
        )
        .unwrap();
        assert_eq!(out, "2\n");
    }

    #[test]
    fn function_call_basic() {
        let out = run_str(
            r#"
            function add(a: int, b: int): int -> {
              a + b
            }
            @out add(2, 3)
            "#,
        )
        .unwrap();
        assert_eq!(out, "5\n");
    }

    #[test]
    fn function_expression_body() {
        let out = run_str(
            r#"
            function double(x: int): int -> x * 2
            @out double(7)
            "#,
        )
        .unwrap();
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
        )
        .unwrap();
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
        )
        .unwrap();
        assert_eq!(out, "120\n");
    }

    #[test]
    fn try_catch_string_error() {
        let out = run_str(
            r#"
            try {
              throw "boom"
            } catch e {
              @out "caught: {e}"
            }
            "#,
        )
        .unwrap();
        assert_eq!(out, "caught: boom\n");
    }

    #[test]
    fn try_catch_object_error() {
        let out = run_str(
            r#"
            try {
              throw { code: 404, msg: "not found" }
            } catch err {
              @out "code={err.code} msg={err.msg}"
            }
            "#,
        )
        .unwrap();
        assert_eq!(out, "code=404 msg=not found\n");
    }

    #[test]
    fn try_without_throw_returns_value() {
        let out = run_str(
            r#"
            let v: int = try { 42 } catch e { 0 }
            @out v
            "#,
        )
        .unwrap();
        assert_eq!(out, "42\n");
    }

    #[test]
    fn throw_without_try_is_uncaught() {
        let err = run_str(r#"throw "panic!""#).unwrap_err();
        assert_eq!(err.thrown.as_ref().map(|_| true), Some(true));
    }

    #[test]
    fn catch_propagates_through_function() {
        let out = run_str(
            r#"
            function risky(): int -> {
              throw { code: 500 }
            }
            try {
              @out risky()
            } catch e {
              @out "caught code {e.code}"
            }
            "#,
        )
        .unwrap();
        assert_eq!(out, "caught code 500\n");
    }

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

    #[test]
    fn for_range_exclusive() {
        let out = run_str(
            r#"
            for i in 0..3 {
              @out i
            }
            "#,
        )
        .unwrap();
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
        )
        .unwrap();
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
        )
        .unwrap();
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
        )
        .unwrap();
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
        )
        .unwrap();
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
        )
        .unwrap();
        assert_eq!(out, "0,0\n0,1\n1,0\n1,1\n");
    }

    #[test]
    fn function_arity_mismatch() {
        let err = run_str(
            r#"
            function f(a: int, b: int): int -> a + b
            @out f(1)
            "#,
        )
        .unwrap_err();
        assert!(err.message.contains("expects 2 arguments"));
    }

    #[test]
    fn block_value_from_last_expr() {
        let out = run_str(
            r#"
            let n: int = 5
            let label: string = if n > 0 { "plus" } else { "neg" }
            @out label
            "#,
        )
        .unwrap();
        assert_eq!(out, "plus\n");
    }
}
