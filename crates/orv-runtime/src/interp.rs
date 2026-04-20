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

/// HTTP 요청 컨텍스트 — `@param`/`@query`/`@header`/`@body`/`@request` 가
/// 조회하는 키-값 저장소.
///
/// C5 에서 tokio/hyper 가 실제 요청을 파싱해 채운다. 테스트는 수동으로
/// 채워서 [`run_handler_with_request`] 로 주입한다.
#[derive(Clone, Debug)]
pub struct RequestCtx {
    /// HTTP 메서드.
    pub method: String,
    /// 요청 경로 (매칭된 원본).
    pub path: String,
    /// 경로 매개변수 (`:id` → `"42"`).
    pub params: HashMap<String, String>,
    /// 쿼리 매개변수.
    pub query: HashMap<String, String>,
    /// 요청 헤더.
    pub headers: HashMap<String, String>,
    /// 파싱된 body. MVP 는 JSON 파싱 전 원문 문자열이거나 void.
    pub body: Value,
}

impl Default for RequestCtx {
    fn default() -> Self {
        Self {
            method: String::new(),
            path: String::new(),
            params: HashMap::new(),
            query: HashMap::new(),
            headers: HashMap::new(),
            body: Value::Void,
        }
    }
}

/// `@respond` 로 기록된 HTTP 응답.
///
/// SPEC §11.4: status 코드와 payload body 쌍. C5 의 HTTP 런타임은 payload
/// 를 JSON 직렬화해서 `application/json` body 로 내보낸다 (MVP). `204 {}`
/// 처럼 빈 객체가 오면 그대로 빈 오브젝트 JSON 이 된다.
#[derive(Clone, Debug)]
pub struct ResponseCtx {
    /// HTTP status code (예: `200`, `404`). MVP 범위는 i64 로 받되 런타임
    /// 검증 시 1xx–5xx 만 허용한다.
    pub status: i64,
    /// 응답 body. `@respond` 가 생략된 payload 는 `Value::Void` 로 기록된다
    /// (`@respond 204` 등).
    pub payload: Value,
    /// 파일 서빙(`@serve "path"`)처럼 JSON 직렬화를 우회해야 하는 경우
    /// raw 바이트와 Content-Type 을 이 필드로 전달한다. `Some` 이면 서버는
    /// `payload` 를 무시하고 이 바이트를 그대로 응답 body 로 쓴다.
    pub raw_body: Option<RawResponseBody>,
}

/// A5a 파일 서빙용 raw 응답 body.
///
/// `@serve "path"` 가 기록한 값. 서버 측 렌더러는 [`ResponseCtx::raw_body`]
/// 가 `Some` 이면 JSON 직렬화를 건너뛰고 이 바이트를 그대로 body 로 사용한다.
#[derive(Clone, Debug)]
pub struct RawResponseBody {
    /// 파일 바이트 그대로 (HTML/CSS/ICO 등).
    pub bytes: Vec<u8>,
    /// 확장자 기반 MIME. 맵 미스 시 `application/octet-stream`.
    pub content_type: String,
}

/// [`run_handler_with_request`] 의 반환값.
///
/// `response` 가 `Some` 이면 handler 안에서 `@respond` 가 실행되어
/// early-return 한 것이다. `value` 는 `@respond` 로 종료되지 않은 handler
/// 블록의 최종 표현식 값 (C5 에서 기본 응답 합성에 사용).
#[derive(Clone, Debug)]
pub struct HandlerOutcome {
    /// handler 블록 최종 값.
    pub value: Value,
    /// `@respond` 로 기록된 응답. 없으면 `None`.
    pub response: Option<ResponseCtx>,
}

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
    run_with_writer_in_env(program, HashMap::new(), writer).map(|_| ())
}

/// 주어진 초기 환경 위에서 프로그램을 실행하고, 실행 후 환경 스냅샷을 돌려준다.
///
/// `@server` 부팅 단계처럼 기존 top-level 바인딩을 본문에 주입해야 하는 경로가
/// 사용한다. 반환된 환경에는 body 안에서 선언된 `let`/`const`/`function` 이
/// 반영되어 이후 handler 평가에 재사용할 수 있다.
pub(crate) fn run_with_writer_in_env<W: Write>(
    program: &HirProgram,
    env: HashMap<NameId, Value>,
    writer: &mut W,
) -> Result<HashMap<NameId, Value>, RuntimeError> {
    let mut interp = Interp::new_with_env(writer, env);
    interp.run(program)?;
    Ok(interp.env)
}

/// 요청 컨텍스트를 주입한 상태에서 단일 표현식(보통 `@route` handler 의
/// HIR 노드나 그 block)을 평가한다. C5 의 HTTP 런타임이 요청마다 호출하는
/// 기본 진입점이며, C3 에서는 request-state 도메인 동작을 검증하기 위한
/// 테스트 인터페이스이기도 하다.
///
/// # Errors
/// 평가 중 타입 불일치, 미지원 도메인 등.
pub fn run_handler_with_request<W: Write>(
    handler: &HirExpr,
    request: RequestCtx,
    writer: &mut W,
) -> Result<HandlerOutcome, RuntimeError> {
    run_handler_with_request_in_env(handler, request, HashMap::new(), writer)
}

/// 요청 컨텍스트와 캡처된 환경을 함께 주입한 상태에서 handler 를 평가한다.
///
/// `@server` 는 top-level / server-level 바인딩을 여기로 넘겨 route handler 가
/// 일반 함수/상수처럼 접근할 수 있게 한다. 요청 간에는 같은 환경 스냅샷을
/// 매번 복제해 쓰므로 상태 누수는 없다.
pub(crate) fn run_handler_with_request_in_env<W: Write>(
    handler: &HirExpr,
    request: RequestCtx,
    env: HashMap<NameId, Value>,
    writer: &mut W,
) -> Result<HandlerOutcome, RuntimeError> {
    let mut interp = Interp::new_with_env(writer, env);
    interp.request = Some(request);
    let value = interp.eval(handler)?;
    // `@respond` 가 있었다면 pending_return 도 세팅돼 있다. handler 종료
    // 시점이라 pending_return 은 의미가 다 했으므로 치워두고 response 만
    // 돌려준다.
    interp.pending_return = None;
    Ok(HandlerOutcome {
        value,
        response: interp.response.take(),
    })
}

struct Interp<'w, W: Write> {
    env: HashMap<NameId, Value>,
    writer: &'w mut W,
    pending_return: Option<Value>,
    loop_signal: LoopSignal,
    /// when 가드의 `$` — 스코프 바인딩이 아니므로 별도 슬롯에 보관한다.
    dollar: Option<Value>,
    /// HTML 렌더 모드 버퍼. `Some` 이면 `@tag` 도메인 호출과 자동 출력이
    /// stdout 대신 이 버퍼에 쌓인다. 함수/람다 호출 경계에서는 잠시
    /// `take()` 해 격리 — HTML body 안에서 호출된 함수의 `@out` 은 stdout
    /// 으로 나간다.
    html_buffer: Option<String>,
    /// 현재 처리 중인 HTTP 요청. `@param`/`@query`/`@header`/`@body`/
    /// `@request` 가 이 컨텍스트를 읽는다. `html_buffer` 와 달리 함수 호출
    /// 경계에서 격리하지 않는다 — 요청 전체 수명 동안 유효하며 handler 가
    /// 부른 함수 안에서도 접근 가능해야 한다.
    request: Option<RequestCtx>,
    /// `@respond` 로 기록된 응답. `Some` 이 되면 현재 route handler 의
    /// early-return 신호로 동작한다. `request` 와 같은 이유로 함수 경계에서
    /// 격리하지 않는다 — handler 안에서 부른 함수가 `@respond` 를 호출한
    /// 경우도 상위 handler 가 즉시 종료돼야 하기 때문.
    response: Option<ResponseCtx>,
}

impl<'w, W: Write> Interp<'w, W> {
    fn new_with_env(writer: &'w mut W, env: HashMap<NameId, Value>) -> Self {
        Self {
            env,
            writer,
            pending_return: None,
            loop_signal: LoopSignal::None,
            dollar: None,
            html_buffer: None,
            request: None,
            response: None,
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
            HirExprKind::Route { .. } => {
                // @route 는 선언 노드다. C5 에서 @server { ... } 블록이
                // 라우트 등록기로 동작할 때 이 arm 이 테이블에 push 한다.
                // 지금은 silent noop — fixture 가 깨지지 않게 한다.
                Ok(Value::Void)
            }
            HirExprKind::Server {
                listen,
                routes,
                body_stmts,
            } => {
                // C5b: tokio + hyper HTTP/1.1 서버 기동. `run_server` 가 포트
                // 바인딩과 accept 루프를 담당하며, 요청마다 해당 route 의
                // handler HIR 을 복제해 새 Interp 로 평가한다. 서버가 종료될
                // 때까지 이 arm 은 블록한다 — Interp 입장에서는 현재 스레드
                // 에서 서버가 돌고, 서버가 멈추면 Value::Void 로 이어진다.
                //
                // 동기 tree-walking 인터프리터와 async hyper 의 간극은
                // server::run_server 내부의 current_thread 런타임 + block_on
                // 으로 흡수한다. HIR 값(특히 Rc 기반 Value)이 !Send 라
                // current_thread 가 자연스럽다.
                crate::server::run_server(listen.as_deref(), routes, body_stmts, self.env.clone())
            }
            HirExprKind::Respond { status, payload } => {
                // @respond 는 route handler 안에서만 의미가 있다. 그 외
                // 맥락(REPL 등)에서 호출되면 request ctx 없이 평가되더라도
                // silent 로 status/payload 만 기록하고 넘어간다 — 사용자
                // 프로그램이 `@respond` 를 route 밖에서 쓰는 실수를 해도
                // 컴파일러/타입체크가 잡을 영역이라, 런타임은 관용적이다.
                let status_value = self.eval(status)?;
                let status_code = match status_value {
                    Value::Int(n) => n,
                    other => {
                        return Err(RuntimeError::native(format!(
                            "`@respond` status must be an integer, got {other}"
                        )));
                    }
                };
                let payload_value = self.eval(payload)?;
                // 중첩 `@respond` 는 첫 호출만 유지. 두 번째부터는 이미
                // pending_return 으로 블록들이 빠져나가는 중이라 보통
                // 도달하지 않지만 방어적으로 덮어쓰기 방지.
                if self.response.is_none() {
                    self.response = Some(ResponseCtx {
                        status: status_code,
                        payload: payload_value,
                        raw_body: None,
                    });
                }
                // early-return 신호. Route handler 블록/루프가 `return` 과
                // 같은 경로로 빠져나온다. Route 값 자체는 Void 로 취급.
                self.pending_return = Some(Value::Void);
                Ok(Value::Void)
            }
            HirExprKind::Html(block) => {
                // HTML 렌더 모드 진입. 기존 버퍼(중첩 @html 허용)를 잠시 치워
                // 새 버퍼로 바꾸고, 블록을 평가한 뒤 결과를 `<html>...</html>`
                // 로 감싼다. 블록의 반환 값은 버려진다 — 태그가 버퍼에
                // 누적된 것만 HTML 이다.
                let saved = self.html_buffer.replace(String::new());
                let block_result = self.eval_block(block);
                let rendered = self.html_buffer.take().unwrap_or_default();
                self.html_buffer = saved;
                block_result?;
                Ok(Value::Str(format!("<html>{rendered}</html>")))
            }
            HirExprKind::Out(arg) => {
                let v = self.eval(arg)?;
                // 인자 없는 `@out` 은 lowering 이 `Void` 를 채워 넣었으므로
                // 그 경우 빈 줄을 출력한다.
                if matches!(v, Value::Void) {
                    self.println(&Value::Str(String::new()))?;
                } else {
                    self.println(&v)?;
                }
                Ok(Value::Void)
            }
            HirExprKind::Domain { name, args, .. } => {
                // HTML 렌더 모드에서는 임의 이름의 도메인이 태그로 해석된다.
                if self.html_buffer.is_some() {
                    self.render_tag(name, args)?;
                    return Ok(Value::Void);
                }
                // 요청 컨텍스트가 있다면 request-state 도메인을 해석한다.
                if self.request.is_some() {
                    if let Some(v) = self.eval_request_domain(name)? {
                        return Ok(v);
                    }
                }
                // A5a: `@serve "path"` — 단일 파일 서빙. route handler 안
                // (request_ctx 있음) 에서만 의미가 있다. 평가 결과는
                // `@respond` 와 동일하게 response 슬롯에 기록 + early-return.
                if name == "serve" && self.request.is_some() {
                    return self.eval_serve(args);
                }
                Err(RuntimeError::native(format!(
                    "unsupported domain `@{name}` in MVP interpreter"
                )))
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
        self.env
            .get(&id)
            .cloned()
            .ok_or_else(|| RuntimeError::native(format!("undefined variable `{debug_name}`")))
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
        // HTML 렌더 모드는 호출 경계에서 격리한다. 호출된 람다 본문의
        // `@tag` 는 호출자의 HTML 에 섞이지 않고 기본 모드(태그 미지원)로
        // 평가된다.
        let saved_html = self.html_buffer.take();
        let result = match &lam.body {
            HirFunctionBody::Block(b) => {
                let ctl = self.eval_block_ctl(b)?;
                self.pending_return = None;
                ctl.into_value()
            }
            HirFunctionBody::Expr(e) => self.eval(e)?,
        };
        self.html_buffer = saved_html;
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
        // call_lambda 와 동일한 이유로 HTML 렌더 모드를 격리.
        let saved_html = self.html_buffer.take();
        let result_value = match &func.body {
            HirFunctionBody::Block(b) => {
                let ctl = self.eval_block_ctl(b)?;
                self.pending_return = None;
                ctl.into_value()
            }
            HirFunctionBody::Expr(e) => self.eval(e)?,
        };
        self.html_buffer = saved_html;
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

    fn pattern_matches(&mut self, pat: &HirPattern, value: &Value) -> Result<bool, RuntimeError> {
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

    /// A5a: `@serve "path"` — 단일 파일 서빙.
    ///
    /// 동작:
    /// 1. 인자 1개 평가 → `Value::Str` 이어야 한다. 아니면 RuntimeError.
    /// 2. `std::fs::metadata` 로 존재/파일 여부 확인.
    ///    - `NotFound` → 404 응답 기록 후 early-return.
    ///    - `is_dir` → 이번 마일스톤(A5a)은 단일 파일만. 400 으로 거부하는
    ///      대신 RuntimeError 로 500 응답 유도 (디렉토리는 A2+A5b).
    /// 3. 파일 크기 캡([`MAX_SERVE_BYTES`]) 초과 시 500.
    /// 4. `std::fs::read(path)` → [`RawResponseBody`] 로 래핑, 확장자 기반
    ///    MIME 부여. status 200 기록 + `pending_return` 으로 early-return.
    ///
    /// 경로는 CWD 기준. 여기선 canonicalize 안 함 — 경로가 하드코드 소스에
    /// 박혀 있고 traversal 위협은 요청 path 가 아니라 개발자 손에 달려 있음.
    /// 디렉토리 서빙이 들어올 때 요청 path 를 join 하게 되면 그때 canonicalize
    /// + prefix 검사를 추가한다.
    fn eval_serve(&mut self, args: &[HirExpr]) -> Result<Value, RuntimeError> {
        if args.len() != 1 {
            return Err(RuntimeError::native(format!(
                "`@serve` expects exactly one string argument, got {}",
                args.len()
            )));
        }
        let path_value = self.eval(&args[0])?;
        let path_str = match path_value {
            Value::Str(s) => s,
            other => {
                return Err(RuntimeError::native(format!(
                    "`@serve` argument must be a string, got {other}"
                )));
            }
        };
        let path = std::path::Path::new(&path_str);

        match std::fs::metadata(path) {
            Ok(meta) if meta.is_file() => {
                // 10MB 캡
                const MAX_SERVE_BYTES: u64 = 10 * 1024 * 1024;
                if meta.len() > MAX_SERVE_BYTES {
                    return Err(RuntimeError::native(format!(
                        "`@serve` file exceeds {MAX_SERVE_BYTES} bytes: {path_str}"
                    )));
                }
                let bytes = std::fs::read(path).map_err(|e| {
                    RuntimeError::native(format!("`@serve` read failed: {e}"))
                })?;
                let mime = mime_for_path(path);
                if self.response.is_none() {
                    self.response = Some(ResponseCtx {
                        status: 200,
                        payload: Value::Void,
                        raw_body: Some(RawResponseBody {
                            bytes,
                            content_type: mime,
                        }),
                    });
                }
                self.pending_return = Some(Value::Void);
                Ok(Value::Void)
            }
            Ok(_) => Err(RuntimeError::native(format!(
                "`@serve` target is not a regular file (directory serving not yet supported): {path_str}"
            ))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // 파일 없음 → 404. RuntimeError 를 던지면 500 이 되므로 직접
                // response 를 기록하고 early-return 한다.
                if self.response.is_none() {
                    self.response = Some(ResponseCtx {
                        status: 404,
                        payload: Value::Void,
                        raw_body: None,
                    });
                }
                self.pending_return = Some(Value::Void);
                Ok(Value::Void)
            }
            Err(e) => Err(RuntimeError::native(format!(
                "`@serve` metadata failed: {e}"
            ))),
        }
    }

    /// 요청 컨텍스트가 있을 때 request-state 도메인 (`@param`, `@query`,
    /// `@header`, `@body`, `@request`) 을 평가한다. 맵 성격은 `Value::Object`
    /// 로 노출되어 기존 `.field` 접근 경로로 조회된다. 지원하지 않는 이름은
    /// `None` 을 돌려 상위가 unsupported domain 에러로 보고하게 한다.
    fn eval_request_domain(&self, name: &str) -> Result<Option<Value>, RuntimeError> {
        let Some(ctx) = &self.request else {
            return Ok(None);
        };
        let map_to_object = |m: &HashMap<String, String>| -> Value {
            Value::Object(
                m.iter()
                    .map(|(k, v)| (k.clone(), Value::Str(v.clone())))
                    .collect(),
            )
        };
        Ok(Some(match name {
            "param" => map_to_object(&ctx.params),
            "query" => map_to_object(&ctx.query),
            "header" => map_to_object(&ctx.headers),
            "body" => ctx.body.clone(),
            "request" => Value::Object(vec![
                ("method".into(), Value::Str(ctx.method.clone())),
                ("path".into(), Value::Str(ctx.path.clone())),
            ]),
            _ => return Ok(None),
        }))
    }

    /// HTML 모드에서 `@tag ...` 도메인 호출 하나를 현재 버퍼에 렌더한다.
    ///
    /// - `@tag { ... }` — block 인자면 블록 본문을 HTML 모드로 재귀 평가.
    ///   태그 사이에 자식 태그/텍스트가 누적된다.
    /// - `@tag expr` — expr 을 평가해 텍스트 콘텐츠로 넣는다.
    /// - `@tag` — 빈 태그.
    fn render_tag(&mut self, name: &str, args: &[HirExpr]) -> Result<(), RuntimeError> {
        self.html_push(&format!("<{name}>"));
        if let Some(arg) = args.first() {
            match &arg.kind {
                HirExprKind::Block(inner) => {
                    self.eval_block(inner)?;
                }
                _ => {
                    let v = self.eval(arg)?;
                    self.html_push_value(&v);
                }
            }
        }
        self.html_push(&format!("</{name}>"));
        Ok(())
    }

    /// 현재 HTML 버퍼에 문자열을 붙인다. 버퍼가 없으면 noop (방어적).
    fn html_push(&mut self, s: &str) {
        if let Some(buf) = self.html_buffer.as_mut() {
            buf.push_str(s);
        }
    }

    /// 값을 문자열로 변환해 HTML 버퍼에 붙인다. void 는 무시.
    fn html_push_value(&mut self, v: &Value) {
        if matches!(v, Value::Void) {
            return;
        }
        let s = value_to_display(v);
        self.html_push(&s);
    }

    fn println(&mut self, v: &Value) -> Result<(), RuntimeError> {
        writeln!(self.writer, "{v}").map_err(|e| RuntimeError::native(format!("io error: {e}")))
    }
}

/// void-scope 자동 출력을 피해야 하는 표현식인지.
/// 파일 확장자 → Content-Type. A5a 하드코드 맵.
///
/// 10개 자주 쓰는 웹 asset 확장자만 매핑. 그 외는 `application/octet-stream`.
/// 더 넓은 MIME 커버리지는 `mime_guess` crate 도입 시점(프로덕션 대비 때)에.
fn mime_for_path(path: &std::path::Path) -> String {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    match ext.as_deref() {
        Some("html" | "htm") => "text/html; charset=utf-8".to_string(),
        Some("css") => "text/css; charset=utf-8".to_string(),
        Some("js" | "mjs") => "application/javascript; charset=utf-8".to_string(),
        Some("json") => "application/json".to_string(),
        Some("svg") => "image/svg+xml".to_string(),
        Some("png") => "image/png".to_string(),
        Some("jpg" | "jpeg") => "image/jpeg".to_string(),
        Some("ico") => "image/x-icon".to_string(),
        Some("txt") => "text/plain; charset=utf-8".to_string(),
        Some("woff2") => "font/woff2".to_string(),
        _ => "application/octet-stream".to_string(),
    }
}

fn has_side_effect(expr: &HirExpr) -> bool {
    // `@html { ... }` 은 순수하게 값을 돌려주는 표현식이므로 side-effect
    // 목록에 넣지 않는다. 부수 효과가 있는 건 `@out`, 아직 미지원 도메인,
    // 대입, 제어 흐름 블록, 호출이다. `@route` 는 선언이므로 side-effect
    // 취급 — stmt-level 에서 자동 출력 대상이 되면 안 된다.
    matches!(
        &expr.kind,
        HirExprKind::Out(_)
            | HirExprKind::Domain { .. }
            | HirExprKind::Route { .. }
            | HirExprKind::Respond { .. }
            | HirExprKind::Server { .. }
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
                && a.iter()
                    .all(|(k, v)| b.iter().any(|(k2, v2)| k == k2 && values_equal(v, v2)))
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
        assert!(
            lx.diagnostics.is_empty(),
            "lex errors: {:?}",
            lx.diagnostics
        );
        let pr = parse(lx.tokens, FileId(0));
        assert!(
            pr.diagnostics.is_empty(),
            "parse errors: {:?}",
            pr.diagnostics
        );
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
    fn html_renders_simple_paragraph() {
        let out = run_str(r#"@out @html { @p "hi" }"#).unwrap();
        assert_eq!(out, "<html><p>hi</p></html>\n");
    }

    #[test]
    fn html_renders_interpolated_text() {
        let out = run_str(
            r#"
            let n: string = "world"
            @out @html { @p "hello {n}" }
            "#,
        )
        .unwrap();
        assert_eq!(out, "<html><p>hello world</p></html>\n");
    }

    // ── request-state 도메인 (@param/@query/@header/@body/@request) ──

    fn eval_handler_src(src: &str, ctx: RequestCtx) -> Result<String, RuntimeError> {
        let lx = lex(src, FileId(0));
        assert!(
            lx.diagnostics.is_empty(),
            "lex errors: {:?}",
            lx.diagnostics
        );
        let pr = parse(lx.tokens, FileId(0));
        assert!(
            pr.diagnostics.is_empty(),
            "parse errors: {:?}",
            pr.diagnostics
        );
        let resolved = resolve(&pr.program);
        assert!(
            resolved.diagnostics.is_empty(),
            "resolve errors: {:?}",
            resolved.diagnostics
        );
        let hir = lower(&pr.program, &resolved);
        // 단일 표현식 프로그램을 가정 — 그 표현식을 handler 처럼 평가한다.
        let orv_hir::HirStmt::Expr(expr) = &hir.items[0] else {
            panic!("expected expr stmt");
        };
        let mut buf = Vec::new();
        let _ = run_handler_with_request(expr, ctx, &mut buf)?;
        Ok(String::from_utf8(buf).unwrap())
    }

    #[test]
    fn request_param_field_access() {
        let ctx = RequestCtx {
            method: "GET".into(),
            path: "/users/42".into(),
            params: [("id".into(), "42".into())].into_iter().collect(),
            ..Default::default()
        };
        let out = eval_handler_src(r#"@out @param.id"#, ctx).unwrap();
        assert_eq!(out, "42\n");
    }

    #[test]
    fn request_query_field_access() {
        let ctx = RequestCtx {
            query: [("page".into(), "2".into())].into_iter().collect(),
            ..Default::default()
        };
        let out = eval_handler_src(r#"@out @query.page"#, ctx).unwrap();
        assert_eq!(out, "2\n");
    }

    #[test]
    fn request_header_field_access() {
        let ctx = RequestCtx {
            headers: [("Authorization".into(), "Bearer x".into())]
                .into_iter()
                .collect(),
            ..Default::default()
        };
        let out = eval_handler_src(r#"@out @header.Authorization"#, ctx).unwrap();
        assert_eq!(out, "Bearer x\n");
    }

    #[test]
    fn request_body_returns_value() {
        let ctx = RequestCtx {
            body: Value::Str("raw body".into()),
            ..Default::default()
        };
        let out = eval_handler_src(r#"@out @body"#, ctx).unwrap();
        assert_eq!(out, "raw body\n");
    }

    #[test]
    fn request_meta_method_and_path() {
        let ctx = RequestCtx {
            method: "POST".into(),
            path: "/items".into(),
            ..Default::default()
        };
        let out = eval_handler_src(r#"@out "{@request.method} {@request.path}""#, ctx).unwrap();
        assert_eq!(out, "POST /items\n");
    }

    #[test]
    fn request_missing_param_is_void() {
        // 없는 키 조회 → Value::Void. `??` 로 대체값 사용 가능.
        let ctx = RequestCtx::default();
        // @out 은 void 를 빈 줄로 출력.
        let out = eval_handler_src(r#"@out @param.missing"#, ctx).unwrap_err();
        // MVP: 객체에 없는 필드는 기존 Field 평가가 "no field" 에러로 처리.
        assert!(out.message.contains("no field"));
    }

    #[test]
    fn request_domain_without_context_is_unsupported() {
        // request ctx 가 없으면 `@param` 등은 unsupported 에러.
        let err = run_str(r#"@out @param.id"#).unwrap_err();
        assert!(err.message.contains("unsupported domain"));
    }

    // ── @respond 도메인 (C4) ──

    /// handler 한 표현식을 평가하고 `(stdout, response)` 를 돌려주는 헬퍼.
    /// C3 의 `eval_handler_src` 는 stdout 만 반환하므로, `@respond` 부작용을
    /// 검증할 때 이 쪽을 사용한다.
    fn run_handler(src: &str, ctx: RequestCtx) -> (String, Option<ResponseCtx>) {
        let lx = lex(src, FileId(0));
        assert!(lx.diagnostics.is_empty(), "lex: {:?}", lx.diagnostics);
        let pr = parse(lx.tokens, FileId(0));
        assert!(pr.diagnostics.is_empty(), "parse: {:?}", pr.diagnostics);
        let resolved = resolve(&pr.program);
        assert!(
            resolved.diagnostics.is_empty(),
            "resolve: {:?}",
            resolved.diagnostics
        );
        let hir = lower(&pr.program, &resolved);
        let orv_hir::HirStmt::Expr(expr) = &hir.items[0] else {
            panic!("expected expr stmt");
        };
        let mut buf = Vec::new();
        let outcome = run_handler_with_request(expr, ctx, &mut buf).unwrap();
        (String::from_utf8(buf).unwrap(), outcome.response)
    }

    #[test]
    fn respond_records_status_and_object_payload() {
        let (stdout, resp) = run_handler(
            r#"{
                @respond 201 { id: 7 }
            }"#,
            RequestCtx::default(),
        );
        assert_eq!(stdout, "");
        let resp = resp.expect("response must be recorded");
        assert_eq!(resp.status, 201);
        // payload 는 Object 한 개의 필드를 담고 있어야 한다.
        let Value::Object(fields) = resp.payload else {
            panic!("payload must be object, got {:?}", resp.payload);
        };
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].0, "id");
        assert!(matches!(fields[0].1, Value::Int(7)));
    }

    #[test]
    fn respond_without_payload_records_void() {
        // `@respond 204` — payload 가 void 로 채워진 채 기록된다.
        let (_, resp) = run_handler(r#"{ @respond 204 }"#, RequestCtx::default());
        let resp = resp.expect("response must be recorded");
        assert_eq!(resp.status, 204);
        assert!(matches!(resp.payload, Value::Void));
    }

    #[test]
    fn respond_early_returns_from_handler() {
        // `@respond` 이후 코드가 실행되면 안 된다 (SPEC §11.4 "return 처럼 동작").
        // `@out` 이 실행되면 stdout 에 흔적이 남는다.
        let (stdout, resp) = run_handler(
            r#"{
                @respond 200 { ok: true }
                @out "should-not-run"
            }"#,
            RequestCtx::default(),
        );
        assert_eq!(stdout, "", "handler must stop at @respond");
        let resp = resp.expect("response recorded");
        assert_eq!(resp.status, 200);
    }

    #[test]
    fn respond_inside_if_branch_still_early_returns() {
        // if/else 분기 안에서 `@respond` 를 만나도 상위 블록이 종료돼야 한다.
        // pending_return 전파 경로가 제어 흐름 노드를 타고 올라온다.
        let (stdout, resp) = run_handler(
            r#"{
                if true {
                    @respond 401 { error: "nope" }
                }
                @out "after"
            }"#,
            RequestCtx::default(),
        );
        assert_eq!(stdout, "");
        assert_eq!(resp.unwrap().status, 401);
    }

    #[test]
    fn respond_uses_request_state_in_payload() {
        // payload 안에서 `@param` 같은 request-state 도메인을 참조 가능.
        // C3 의 request ctx 와 C4 의 @respond 가 결합되는 핵심 경로.
        let ctx = RequestCtx {
            params: [("id".into(), "42".into())].into_iter().collect(),
            ..Default::default()
        };
        let (_, resp) = run_handler(r#"{ @respond 200 { id: @param.id } }"#, ctx);
        let resp = resp.unwrap();
        assert_eq!(resp.status, 200);
        let Value::Object(fields) = resp.payload else {
            panic!("object payload");
        };
        assert!(matches!(&fields[0].1, Value::Str(s) if s == "42"));
    }

    #[test]
    fn respond_first_wins_on_double_call() {
        // 같은 handler 안에서 `@respond` 를 연속 호출할 일은 early-return
        // 덕에 정상적으론 없지만, 첫 호출이 유지돼야 한다는 계약을 방어적으로
        // 검증. 두 번째 respond 는 도달 자체가 불가.
        let (_, resp) = run_handler(
            r#"{
                @respond 200 { ok: true }
                @respond 500 { err: "x" }
            }"#,
            RequestCtx::default(),
        );
        assert_eq!(resp.unwrap().status, 200);
    }

    #[test]
    fn server_without_listen_returns_runtime_error() {
        // C5b: @server 는 실제 tokio/hyper 서버를 기동한다. @listen 이 없으면
        // MVP 에서는 명시 에러를 돌려주어 진단을 쉽게 한다. 실 서버 바인딩
        // 테스트는 server.rs 모듈의 통합 테스트(#[tokio::test])가 맡고, 여기
        // 서는 `@server` arm 이 interp eval 경로에 올라오는 것만 검증한다.
        let err = run_str(
            r#"
            @server {
                @route GET /api { @respond 200 {} }
            }
            "#,
        )
        .unwrap_err();
        assert!(
            err.message.contains("@server"),
            "unexpected error: {}",
            err.message
        );
    }

    #[test]
    fn html_for_loop_produces_list() {
        // HTML 전용 제어 흐름 없이 기존 `for` 가 그대로 동작해야 한다.
        let out = run_str(r#"@out @html { for i in 0..3 { @li "{i}" } }"#).unwrap();
        assert_eq!(out, "<html><li>0</li><li>1</li><li>2</li></html>\n");
    }

    #[test]
    fn html_if_inside_for() {
        let out = run_str(
            r#"@out @html {
              for i in 0..3 {
                @span i
                if i == 0 { @div "first" }
              }
            }"#,
        )
        .unwrap();
        assert_eq!(
            out,
            "<html><span>0</span><div>first</div><span>1</span><span>2</span></html>\n"
        );
    }

    #[test]
    fn html_function_call_isolates_render_mode() {
        // 함수 본문의 `@out` 은 stdout 으로, HTML 버퍼에 섞이면 안 된다.
        let out = run_str(
            r#"
            function log(msg: string) -> @out "[log] {msg}"
            let page: string = @html {
              log("rendering")
              @p "content"
            }
            @out page
            "#,
        )
        .unwrap();
        assert_eq!(out, "[log] rendering\n<html><p>content</p></html>\n");
    }

    #[test]
    fn html_renders_nested_head_body() {
        let out = run_str(
            r#"@out @html {
              @head { @title "Hi" }
              @body { @p "hi" }
            }"#,
        )
        .unwrap();
        assert_eq!(
            out,
            "<html><head><title>Hi</title></head><body><p>hi</p></body></html>\n"
        );
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
