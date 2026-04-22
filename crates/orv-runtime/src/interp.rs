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
    HirPattern, HirProgram, HirStmt, HirStringSegment, HirTypeRef, HirTypeRefKind, NameId, UnaryOp,
};
use std::collections::HashMap;
use std::fmt;
use std::io::Write;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

/// B4 `@env` 테스트 override.
///
/// `std::env::set_var` 는 Rust 2024 에서 `unsafe` 가 되었고 워크스페이스는
/// `unsafe_code = "forbid"` 라 단위 테스트가 직접 env 를 조작할 수 없다.
/// `#[cfg(test)]` 전용 맵을 두어 테스트에서 override 를 주입하고, Domain
/// arm 에서 `@env` 평가 시 이 맵을 병합한다. production 빌드에는 이 모듈이
/// 남지 않는다.
#[cfg(test)]
mod test_env {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};

    pub(super) static ENV_OVERRIDES: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

    pub(crate) fn set(key: &str, value: &str) {
        let lock = ENV_OVERRIDES.get_or_init(|| Mutex::new(HashMap::new()));
        lock.lock()
            .unwrap()
            .insert(key.to_string(), value.to_string());
    }

    pub(crate) fn clear(key: &str) {
        if let Some(lock) = ENV_OVERRIDES.get() {
            if let Ok(mut map) = lock.lock() {
                map.remove(key);
            }
        }
    }
}

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
    /// SPEC §11.9: `@redirect` 로 기록된 Location URL. `Some` 이면 서버는
    /// `Location` 헤더를 추가한다. body 는 빈 값으로 내보낸다.
    pub location: Option<String>,
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
    /// A3 하이브리드: handler 가 server-level `let` 으로 선언된 이름을
    /// 재할당한 경우의 경고 메시지들. 기능은 허용 (per-request clone) 되지만
    /// 개발자에게 "상태는 요청 간 공유되지 않으며 영속 상태는 `@db`/`@cache`
    /// 를 사용하라" 는 신호를 준다. 호출자(`handle_request`)가 stderr 로
    /// 흘려보낸다.
    pub warnings: Vec<String>,
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
    /// C_db: in-memory DB handle. `@db` 평가 결과이며 `.create` 같은 field
    /// 접근으로 bound method 를 얻어 호출한다.
    Db(Arc<Mutex<crate::db::InMemoryDb>>),
    /// SPEC §4.9: 원시 타입 namespace 핸들. `int` / `string` / `float` / `bool`
    /// 같은 이름이 값 맥락에서 평가되면 이 variant. field access `.from` 이
    /// BoundMethod 를 만들어 호출하면 타입별 파싱/포맷을 수행한다.
    TypeName(String),
    /// 내장 전역 함수 핸들 — `max`, `min`, `sin`, `now`, `sleep` 등.
    /// 값 자체는 이름만 담고 실제 dispatch 는 [`Interpreter::call_builtin`]
    /// 이 수행한다. `Type` 같이 내장 식별자를 변수로 섀도잉하려 하면 스코프
    /// 테이블에 같은 이름이 먼저 들어가므로 런타임은 자연스럽게 스코프
    /// 값을 우선한다 ([`Interpreter::lookup`] 참고).
    Builtin(String),
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
            Self::Db(_) => write!(f, "<db>"),
            Self::TypeName(n) => write!(f, "<type {n}>"),
            Self::Builtin(n) => write!(f, "<builtin {n}>"),
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
    let db = Arc::new(Mutex::new(crate::db::InMemoryDb::new()));
    run_handler_with_request_in_env(handler, request, HashMap::new(), db, writer)
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
    db: Arc<Mutex<crate::db::InMemoryDb>>,
    writer: &mut W,
) -> Result<HandlerOutcome, RuntimeError> {
    let mut interp = Interp::new_with_env(writer, env);
    interp.db = db;
    // A3: 진입 시점의 env 키를 "server-level captured" 로 기록. handler 가
    // 이 이름을 재할당하면 경고를 적립한다 (기능은 허용).
    interp.captured_names = interp.env.keys().copied().collect();
    interp.request = Some(request);
    let value = interp.eval(handler)?;
    // `@respond` 가 있었다면 pending_return 도 세팅돼 있다. handler 종료
    // 시점이라 pending_return 은 의미가 다 했으므로 치워두고 response 만
    // 돌려준다.
    interp.pending_return = None;
    // C_middleware: `@after` 로 등록된 post-handler block 들을 순서대로 평가.
    // 이 단계에서는 `@respond` 가 이미 기록된 상태라 after 가 status 를 바꾸지
    // 못한다 (첫 `@respond` 만 유지하는 기존 규칙). after 의 주 목적은 로깅/
    // 메트릭/cleanup 이므로 부작용만 실행되고 반환값은 버린다.
    let after_blocks = std::mem::take(&mut interp.after_queue);
    for block in after_blocks {
        // after 자체가 다시 @respond 를 시도해도 response 슬롯은 이미 Some
        // 이라 no-op. pending_return 은 계속 None 유지.
        interp.eval_block(&block)?;
        interp.pending_return = None;
    }
    Ok(HandlerOutcome {
        value,
        response: interp.response.take(),
        warnings: std::mem::take(&mut interp.warnings),
    })
}

/// 캡처된 환경 위에서 단일 표현식을 평가한다.
///
/// `@listen` 처럼 프로그램/핸들러 전체를 실행하지 않고 "식 하나의 값"만
/// 필요할 때 사용한다. request 컨텍스트는 주입하지 않으므로 request-state
/// 도메인(`@param`, `@body` 등)은 그대로 unsupported 에러가 난다.
pub(crate) fn eval_expr_in_env<W: Write>(
    expr: &HirExpr,
    env: &HashMap<NameId, Value>,
    writer: &mut W,
) -> Result<Value, RuntimeError> {
    let mut interp = Interp::new_with_env(writer, env.clone());
    interp.eval(expr)
}

struct Interp<'w, W: Write> {
    env: HashMap<NameId, Value>,
    writer: &'w mut W,
    pending_return: Option<Value>,
    loop_signal: LoopSignal,
    /// A3 하이브리드: handler 진입 시점에 보유하고 있던 env 키들. 이후
    /// `Assign` arm 이 이 집합 안의 name 을 타깃으로 삼으면 [`Self::warnings`]
    /// 에 기록한다 (기능은 허용, 신호만 남김).
    captured_names: std::collections::HashSet<NameId>,
    /// 누적 경고. 동일 name 은 1회만 기록한다.
    warnings: Vec<String>,
    /// 경고 중복 방지 집합.
    warned_names: std::collections::HashSet<NameId>,
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
    /// C_middleware: `@next {k: v}` 로 middleware 가 쌓아 올린 문맥 값.
    /// Route handler 안에서 `@context.k` 로 조회된다. `None` 이면 handler
    /// 바깥(예: REPL) — `@context` 참조 시 빈 Object 를 돌려준다.
    ///
    /// Vec 순서 유지 이유: `@next {a: 1}` 후 `@next {a: 2}` 순서로 덮어쓰려면
    /// 뒤에 붙인 값이 우세해야 한다. [`push_context`] 가 기존 키를 제거하고
    /// 새로 push 하므로 `Value::Object` 와 같은 "마지막 value 가 우세" 의미.
    context: Vec<(String, Value)>,
    /// C_middleware: `@after { body }` 로 등록된 post-handler block 큐.
    /// Route handler 본문이 끝난 뒤 (with `@respond` or not) 이 큐가 순서대로
    /// 평가된다. `@after` 는 `@respond` 를 바꾸지 못한다 (이미 기록됨).
    /// Handler 경계 밖에서는 register 되지 않고 즉시 body 평가된다.
    after_queue: Vec<HirBlock>,
    /// SPEC §9.5: `@content` 지시어가 평가할 현재 slot. 호출부가 domain
    /// invoke 에 block literal 을 넘겼다면 `call_user_domain` 이 이 필드에
    /// 해당 block 을 밀어넣는다. define body 안에서 `@content` domain 을
    /// 만나면 이 block 을 평가한다. slot 이 `None` 이면 silent noop.
    ///
    /// 호출 스택 깊이에 따른 저장/복원은 `call_function*` 가 담당 — Rust
    /// 스택을 타고 함수 호출 경계마다 save/restore.
    content_slot: Option<HirBlock>,
    /// C_db: 프로세스 내 in-memory DB. handler 호출 간 공유되어 이전 요청이
    /// 쓴 데이터를 다음 요청이 읽을 수 있다. 서버 재시작 시 소실.
    db: Arc<Mutex<crate::db::InMemoryDb>>,
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
            captured_names: std::collections::HashSet::new(),
            warnings: Vec::new(),
            warned_names: std::collections::HashSet::new(),
            context: Vec::new(),
            after_queue: Vec::new(),
            content_slot: None,
            db: Arc::new(Mutex::new(crate::db::InMemoryDb::new())),
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
                let rc = Rc::new((**f).clone());
                self.env.insert(f.name.id, Value::Function(rc.clone()));
                // SPEC §9.6: nested define 은 외부에서 `@Parent.Child` dotted
                // 경로로 접근 가능해야 한다. parent body 를 재귀 탐색해 nested
                // function 들을 dotted name 을 가진 별도 Rc<HirFunctionStmt>
                // 로 env 에 추가 등록한다. 이름을 dotted 로 바꾼 clone 을
                // 만들어 domain-call 선형 탐색(`f.name.name == name`)이 그대로
                // 매칭되게 한다.
                if f.is_define {
                    register_nested_defines(&mut self.env, &f.name.name, f);
                }
            }
            HirStmt::Struct(_) => {
                // MVP: 타입 정보만 필요하며 런타임은 noop. 이후 커밋에서 확장.
            }
            HirStmt::Enum(e) => {
                // SPEC §4.4: enum 을 Value::Object 로 env 에 바인딩.
                // `Name.Variant` 는 기존 Field arm 이 처리.
                let mut fields: Vec<(String, Value)> = Vec::with_capacity(e.variants.len());
                for v in &e.variants {
                    let val = self.eval(&v.value)?;
                    fields.push((v.name.clone(), val));
                }
                self.env.insert(e.name.id, Value::Object(fields));
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
            // SPEC §8: import 는 멀티파일 로더가 병합을 끝낸 시점부터 참조
            // 바인딩이 실제로 env 에 존재한다. 런타임은 noop.
            HirStmt::Import(_) => {}
        }
        Ok(())
    }

    fn eval(&mut self, expr: &HirExpr) -> Result<Value, RuntimeError> {
        match &expr.kind {
            HirExprKind::Integer(s) => {
                // SPEC §4.1: ulong/uint 범위까지 지원해야 하므로 i64 범위를
                // 벗어난 리터럴은 u64 로 재시도 후 i64 로 bit-cast 한다.
                // MVP 인터프리터는 모든 정수를 `Value::Int(i64)` 로 저장하지만
                // 비트 폭이 동일하므로 ulong MAX 값도 보존된다.
                let cleaned = s.replace('_', "");
                if let Ok(n) = cleaned.parse::<i64>() {
                    return Ok(Value::Int(n));
                }
                if let Ok(u) = cleaned.parse::<u64>() {
                    return Ok(Value::Int(u as i64));
                }
                Err(RuntimeError::native(format!(
                    "invalid integer literal `{s}`"
                )))
            }
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
                // SPEC §3.x: `??` 는 LHS 가 void 일 때만 RHS 로 폴백.
                // short-circuit — LHS 가 non-void 면 RHS 평가 금지.
                if matches!(op, BinaryOp::Coalesce) {
                    let l = self.eval(lhs)?;
                    return if matches!(l, Value::Void) {
                        self.eval(rhs)
                    } else {
                        Ok(l)
                    };
                }
                // `&&` / `||` 도 short-circuit. 우측이 평가되기 전에 좌측
                // 결과로 전체 값이 확정될 수 있다. apply_binary 는 두 값을
                // 다 받는 구조라 여기서 분기.
                if matches!(op, BinaryOp::And | BinaryOp::Or) {
                    let l = self.eval(lhs)?;
                    let Value::Bool(lb) = l else {
                        return Err(RuntimeError::native(format!(
                            "logical `{op:?}` expects bool on left, got {l}"
                        )));
                    };
                    match op {
                        BinaryOp::And if !lb => return Ok(Value::Bool(false)),
                        BinaryOp::Or if lb => return Ok(Value::Bool(true)),
                        _ => {}
                    }
                    let r = self.eval(rhs)?;
                    let Value::Bool(rb) = r else {
                        return Err(RuntimeError::native(format!(
                            "logical `{op:?}` expects bool on right, got {r}"
                        )));
                    };
                    return Ok(Value::Bool(rb));
                }
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
                        location: None,
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
                // C_middleware: `@before`/`@after`/`@next`/`@context` 처리.
                // `@before { body }` — define 본문 안에서 middleware 선언의
                //   표식이자 동시에 body 평가. Route handler 경로에서 `@Auth`
                //   처럼 호출되면 call_function 이 body 를 평가하며 `@before`
                //   arm 에 도달, 그 안의 block 을 순차 실행한다.
                // `@after { body }` — body 를 바로 실행하지 않고 현재
                //   handler 의 after_queue 에 등록. handler 본문 평가가 끝난
                //   뒤 큐가 순서대로 flush 된다.
                // `@next {k: v}` — object literal 의 key/value 를 context 에
                //   머지. `@next` 단독(인자 0) 은 pass-through.
                // `@context` — 현재 문맥을 Value::Object 로 노출. `@context.x`
                //   접근은 기존 Field arm 이 처리.
                if name == "before" {
                    return self.eval_before(args);
                }
                if name == "after" {
                    return self.eval_after(args);
                }
                if name == "next" {
                    return self.eval_next(args);
                }
                if name == "context" && args.is_empty() {
                    return Ok(Value::Object(self.context.clone()));
                }
                // SPEC §9.5: `@content` — 호출부 block literal 을 평가해 이 자리에
                // 확장한다. slot 이 비었으면 noop (에러 아님 — SPEC 관용).
                if name == "content" && args.is_empty() {
                    if let Some(block) = self.content_slot.clone() {
                        self.eval_block(&block)?;
                    }
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
                // SPEC §9.2~§9.4: 대문자 user-domain 호출.
                //
                // args 는 parser 가 수집한 property (`ExprKind::Assign`) 와
                // positional (token/block/scalar) 의 섞인 시퀀스다. 이번 단계
                // (Stage 1) 는 property-by-name 만 정식 지원한다:
                //   - Assign { target, value } → function param 중 target 이름과
                //     매칭해 바인딩. 미선언 name 은 에러.
                //   - positional 값 → 남은 param 에 순서대로. SPEC 의 token
                //     시맨틱(always-array) 은 후속 단계에서 define body 의
                //     `token { ... }` 선언과 함께 도입한다.
                //   - 누락 param 이 nullable 이면 `Value::Void`, 아니면 에러.
                //
                // Domain name 은 resolve 에서 NameId 바인딩을 받지 않아 env
                // 선형 탐색. 함수 수 적어 실용.
                if name.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
                    let func = self
                        .env
                        .values()
                        .find_map(|v| match v {
                            Value::Function(f) if f.name.name == *name => Some(f.clone()),
                            _ => None,
                        });
                    if let Some(func) = func {
                        return self.call_user_domain(&func, args);
                    }
                }
                // B4: `@env` — 환경 변수. Field access 로 쓰이므로 요청
                // 컨텍스트와 독립. 사용자가 `@env.NAME` 을 쓰려면 env 가
                // `{NAME: value}` 꼴의 Object 로 평가돼야 한다. 전체 env
                // 맵을 한 번 스냅샷해 넘긴다 — 프로세스 env 는 handler 생애
                // 동안 안정적이라 캐싱 없이 매 호출에서 다시 읽어도 무방
                // (실전에서 @env 참조 빈도는 낮음).
                // SPEC §10.2 `@in "prompt"` — 콘솔 표준 입력. MVP 는
                // 비대화식 실행(테스트/스크립트) 을 기본 가정하므로 stdin 이
                // 터미널에 연결됐을 때만 실제 readline 을 시도한다. 아니면
                // 빈 문자열을 돌려 스크립트가 끊기지 않게 한다. prompt 는
                // 있으면 `@out` 과 동일한 방식으로 출력.
                if name == "in" {
                    if let Some(prompt) = args.first() {
                        let s = self.eval(prompt)?;
                        print!("{}", value_to_display(&s));
                        use std::io::Write;
                        let _ = std::io::stdout().flush();
                    }
                    let mut line = String::new();
                    let _ = std::io::stdin().read_line(&mut line);
                    if line.ends_with('\n') {
                        line.pop();
                        if line.ends_with('\r') {
                            line.pop();
                        }
                    }
                    return Ok(Value::Str(line));
                }
                // SPEC 부록 `@fs` — 파일 I/O. MVP: read/write 만.
                // `@fs.read "path"` / `@fs.write "path" "content"`.
                if name == "fs" && args.is_empty() {
                    return Ok(Value::TypeName("fs".to_string()));
                }
                // SPEC 부록 `@process` — 서브프로세스 실행. MVP: `.run(cmd)` 만.
                if name == "process" && args.is_empty() {
                    return Ok(Value::TypeName("process".to_string()));
                }
                // SPEC §11.18 `@cron` / `@job` — 스케줄링/백그라운드 작업.
                // SPEC §10.7 `@design` — 디자인 토큰 선언 (빌드 타임 CSS emit).
                // SPEC §11.11-11.14 `@ws` / `@wt` / `@webrtc` — 실시간 채널.
                // SPEC §11.15 `@upload` — chunked 업로드.
                // SPEC §11.19 `@plugin` — 런타임 확장.
                // MVP 는 선언을 silent 로 받아들이고 즉시 실행하지 않는다.
                // 실제 구현은 후속 마일스톤.
                // SPEC §10.5 `@fetch METHOD url` — HTTP 요청. MVP 는 실제
                // 네트워크 I/O 를 보내지 않고 Response 구조의 stub object 를
                // 돌려 예시 스크립트가 `.status` / `.body` 필드 접근까지
                // 이어지게 한다. 실제 구현은 후속 마일스톤에서 `reqwest`
                // 또는 `hyper-rustls` 로 붙는다.
                if name == "fetch" {
                    let mut method = String::new();
                    let mut url = String::new();
                    for (i, a) in args.iter().enumerate() {
                        let v = self.eval(a)?;
                        match (i, v) {
                            (0, Value::Str(s)) => method = s,
                            (1, Value::Str(s)) => url = s,
                            _ => {}
                        }
                    }
                    let _ = method;
                    return Ok(Value::Object(vec![
                        ("status".to_string(), Value::Int(200)),
                        ("body".to_string(), Value::Str(String::new())),
                        ("url".to_string(), Value::Str(url)),
                    ]));
                }
                if matches!(
                    name.as_str(),
                    "cron"
                        | "job"
                        | "design"
                        | "ws"
                        | "wt"
                        | "webrtc"
                        | "upload"
                        | "plugin"
                        | "net"
                        | "mail"
                        | "sync"
                        | "gpu"
                        | "media"
                        | "offline"
                        | "push"
                        | "storage"
                        | "ffi"
                        | "unsafe"
                ) {
                    return Ok(Value::Void);
                }
                // SPEC §11.9: `@redirect` — route handler 안에서 HTTP redirect.
                // `@redirect "/path"` → 302 Found, `@redirect 301 "/moved"` → 301.
                // response 에 status + Location 기록하고 early-return.
                if name == "redirect" && self.request.is_some() {
                    return self.eval_redirect(args);
                }
                // C_db: `@db` — in-memory DB handle. Interp 내부 싱글톤으로 유지.
                // field access `.create`/`.find`/`.update`/`.delete` 는 기존
                // Field 경로가 BoundMethod 를 만든다 (아래 Field arm 에서 Db
                // receiver 를 감지).
                if name == "db" && args.is_empty() {
                    return Ok(Value::Db(self.db.clone()));
                }
                if name == "env" && args.is_empty() {
                    let pairs: Vec<(String, Value)> =
                        std::env::vars().map(|(k, v)| (k, Value::Str(v))).collect();
                    #[cfg(test)]
                    let pairs = {
                        let mut pairs = pairs;
                        if let Some(lock) = test_env::ENV_OVERRIDES.get() {
                            if let Ok(map) = lock.lock() {
                                for (k, v) in map.iter() {
                                    // override 가 우선. 기존 pair 제거 후 삽입.
                                    pairs.retain(|(pk, _)| pk != k);
                                    pairs.push((k.clone(), Value::Str(v.clone())));
                                }
                            }
                        }
                        pairs
                    };
                    return Ok(Value::Object(pairs));
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
                // A3 하이브리드: handler 가 server-level (또는 top-level)
                // 바인딩을 재할당하면 1회 경고 적립. 실제 동작은 per-request
                // clone 이라 다른 요청에 누수되지 않지만, 개발자에게 "요청 간
                // 공유되지 않는다, 영속 상태는 @db/@cache 를 쓰라" 는 신호.
                if self.captured_names.contains(&target.id) && self.warned_names.insert(target.id) {
                    self.warnings.push(format!(
                        "[orv] assignment to server-level `{}` is per-request only; use @db or @cache for shared state",
                        target.name
                    ));
                }
                let v = self.eval(value)?;
                self.env.insert(target.id, v.clone());
                Ok(v)
            }
            HirExprKind::AssignField {
                object,
                field,
                value,
                ..
            } => {
                // SPEC §4.6: `obj.field = value`. object 평가 후 Object
                // variant 여야 한다. 새 값을 생성해 env 에 재삽입 — Rust 의
                // Value::Object 는 Vec 소유라 in-place mutation 이 불가.
                let obj_value = self.eval(object)?;
                let mut fields = match obj_value {
                    Value::Object(f) => f,
                    other => {
                        return Err(RuntimeError::native(format!(
                            "cannot assign field `{field}` on non-object: {other}"
                        )));
                    }
                };
                let new_value = self.eval(value)?;
                if let Some(slot) = fields.iter_mut().find(|(k, _)| k == field) {
                    slot.1 = new_value.clone();
                } else {
                    fields.push((field.clone(), new_value.clone()));
                }
                // object 가 Ident 면 env 의 원본도 갱신. 중첩 Field 체인은
                // 지금 지원하지 않으며 expr 결과만 업데이트된다 (MVP).
                if let HirExprKind::Ident(id) = &object.kind {
                    self.env.insert(id.id, Value::Object(fields));
                }
                Ok(new_value)
            }
            HirExprKind::For {
                var,
                index_var,
                iter,
                body,
            } => {
                // SPEC §6.4: range/array/string 순회를 지원한다. Range 는
                // lazy evaluation 으로 lo/hi 만 추출하고, 그 외는 iter 를 먼저
                // eval 해 Value 로 받은 뒤 내부를 순회한다.
                if matches!(iter.kind, HirExprKind::Range { .. }) {
                    let (lo, hi, incl) = self.interpret_range(iter)?;
                    let mut i = lo;
                    let mut idx: i64 = 0;
                    while if incl { i <= hi } else { i < hi } {
                        self.env.insert(var.id, Value::Int(i));
                        if let Some(iv) = index_var {
                            self.env.insert(iv.id, Value::Int(idx));
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
                        i += 1;
                        idx += 1;
                    }
                    return Ok(Value::Void);
                }

                // 일반 컬렉션 순회.
                let iter_value = self.eval(iter)?;
                let items: Vec<Value> = match iter_value {
                    Value::Array(xs) => xs,
                    Value::Str(s) => s
                        .chars()
                        .map(|c| Value::Str(c.to_string()))
                        .collect(),
                    other => {
                        return Err(RuntimeError::native(format!(
                            "for loop iterable must be a range, array, or string, got {other}"
                        )));
                    }
                };
                for (i, item) in items.into_iter().enumerate() {
                    self.env.insert(var.id, item);
                    if let Some(iv) = index_var {
                        self.env.insert(iv.id, Value::Int(i64::try_from(i).unwrap_or(0)));
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
                // SPEC §2.5 spread: `{...base, key: value}`. is_spread 필드면
                // 평가 결과 Object 의 key/value 를 순서대로 병합한다. 같은
                // key 가 뒤에 다시 나오면 뒤가 우세 (override) — 일반 object
                // literal 동작과 일치.
                let mut out: Vec<(String, Value)> = Vec::with_capacity(fields.len());
                for f in fields {
                    let v = self.eval(&f.value)?;
                    if f.is_spread {
                        let Value::Object(source) = v else {
                            return Err(RuntimeError::native(
                                "object spread `...expr` requires an object value",
                            ));
                        };
                        for (k, v) in source {
                            out.retain(|(ek, _)| ek != &k);
                            out.push((k, v));
                        }
                    } else {
                        out.retain(|(ek, _)| ek != &f.name);
                        out.push((f.name.clone(), v));
                    }
                }
                Ok(Value::Object(out))
            }
            HirExprKind::Slice { target, start, end } => {
                let t = self.eval(target)?;
                let start_v = match start {
                    Some(e) => Some(self.eval(e)?),
                    None => None,
                };
                let end_v = match end {
                    Some(e) => Some(self.eval(e)?),
                    None => None,
                };
                apply_slice(t, start_v, end_v)
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
                // B4: `@env.NAME` 은 SPEC 의 nullable string 모델을 따른다.
                // 즉 env var 이 없으면 에러 대신 Void 를 돌려주어 `??` 와
                // 결합 가능해야 한다. Domain{name:"env"} 타깃일 때만 이
                // 특수 경로를 탄다 — 일반 object 의 missing-field 동작은
                // 기존대로 RuntimeError (기존 테스트 호환).
                if let HirExprKind::Domain {
                    name: dname,
                    args: dargs,
                    ..
                } = &target.kind
                {
                    if dname == "env" && dargs.is_empty() {
                        let key = field.as_str();
                        let value = {
                            #[cfg(test)]
                            {
                                let override_v = test_env::ENV_OVERRIDES
                                    .get()
                                    .and_then(|l| l.lock().ok()?.get(key).cloned());
                                override_v.or_else(|| std::env::var(key).ok())
                            }
                            #[cfg(not(test))]
                            {
                                std::env::var(key).ok()
                            }
                        };
                        return Ok(value.map_or(Value::Void, Value::Str));
                    }
                }
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
                    // SPEC §3.3 소유권 연산자 — `.move()` 와 `.copy()` 는 모든
                    // 값 타입에서 허용되며 MVP interpreter 에서는 동일하게
                    // clone 한다. 실제 borrow-checker 는 analyzer 계층이 수행.
                    (
                        Value::Str(_)
                        | Value::Array(_)
                        | Value::Object(_)
                        | Value::Int(_)
                        | Value::Float(_)
                        | Value::Bool(_)
                        | Value::Void,
                        "move" | "copy",
                    ) => Ok(Value::BoundMethod {
                        receiver: Box::new(t),
                        method: name.to_string(),
                    }),
                    // C_db: `@db.create` / `@db.find` / `@db.update` / `@db.delete`.
                    (Value::Db(_), "create" | "find" | "findAll" | "update" | "delete") => {
                        Ok(Value::BoundMethod {
                            receiver: Box::new(t),
                            method: name.to_string(),
                        })
                    }
                    // SPEC §4.9: `int.from` / `string.from` / `float.from` / `bool.from`.
                    (Value::TypeName(_), "from") => Ok(Value::BoundMethod {
                        receiver: Box::new(t),
                        method: name.to_string(),
                    }),
                    // SPEC 부록 `@fs.read` / `@fs.write`. Native namespace 공유.
                    (Value::TypeName(ns), "read" | "write") if ns == "fs" => {
                        Ok(Value::BoundMethod {
                            receiver: Box::new(t),
                            method: name.to_string(),
                        })
                    }
                    // SPEC 부록 `@process.run`.
                    (Value::TypeName(ns), "run") if ns == "process" => {
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
            HirExprKind::Await(inner) => {
                // B2 MVP: identity. Future 추상이 아직 없으므로 피연산자를
                // 평가해 그대로 돌려준다. 실제 스케줄링은 후속 마일스톤.
                self.eval(inner)
            }
            HirExprKind::Cast { expr, ty } => {
                let v = self.eval(expr)?;
                apply_cast(v, ty)
            }
            HirExprKind::Try { try_block, catch } => match self.eval_block(try_block) {
                Ok(v) => Ok(v),
                Err(e) => {
                    // SPEC §6.4: try/catch 는 throw 된 사용자 값과 native 런타임
                    // 에러를 모두 잡는다. native 에러는 메시지를 `Value::Str` 로
                    // 래핑해 catch binding 에 전달한다. catch 가 없으면 그대로
                    // 상위로 전파.
                    let Some(clause) = catch else {
                        return Err(e);
                    };
                    let thrown = e
                        .thrown
                        .clone()
                        .unwrap_or_else(|| Value::Str(e.message.clone()));
                    if let Some(name) = &clause.binding {
                        self.env.insert(name.id, thrown);
                    }
                    self.eval_block(&clause.body)
                }
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
        // 스코프 우선. 같은 이름의 사용자 변수가 있으면 그쪽.
        if let Some(v) = self.env.get(&id) {
            return Ok(v.clone());
        }
        // SPEC §4.9: env 에 없고 원시 타입 이름이면 namespace 핸들.
        if is_primitive_type_name(debug_name) {
            return Ok(Value::TypeName(debug_name.to_string()));
        }
        // SPEC §13 내장 전역 함수.
        if is_builtin_fn_name(debug_name) {
            return Ok(Value::Builtin(debug_name.to_string()));
        }
        Err(RuntimeError::native(format!(
            "undefined variable `{debug_name}`"
        )))
    }

    fn call_value(&mut self, callee: Value, args: Vec<Value>) -> Result<Value, RuntimeError> {
        match callee {
            Value::Function(func) => self.call_function(&func, args),
            Value::Lambda(lam) => self.call_lambda(&lam, args),
            Value::BoundMethod { receiver, method } => self.call_method(*receiver, &method, args),
            Value::Builtin(name) => call_builtin(&name, args),
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
        for (p, v) in lam.params.iter().zip(args) {
            self.env.insert(p.name.id, v);
        }
        let saved_return = self.pending_return.take();
        let saved_html = self.html_buffer.take();
        let saved_loop = self.loop_signal;
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
        self.loop_signal = saved_loop;
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
            // SPEC §3.3 소유권/복사 연산 — interpreter 는 값이 기본 clone
            // 이므로 `.move()` / `.copy()` 모두 identity 처럼 동작한다.
            // static borrow-check 는 analyzer (B5) 책임.
            (
                v @ (Value::Str(_)
                | Value::Array(_)
                | Value::Object(_)
                | Value::Int(_)
                | Value::Float(_)
                | Value::Bool(_)
                | Value::Void),
                "move" | "copy",
            ) => Ok(v),
            // ── SPEC §4.9 타입 변환 ──
            //
            // `int.from(v)` / `string.from(v)` / `float.from(v)` / `bool.from(v)`
            // 형태 파싱/포맷. 실패 시 RuntimeError — SPEC 은 throw 를 규정하지만
            // MVP 는 native error 로 보고.
            (Value::TypeName(type_name), "from") if type_name != "fs" => {
                let arg = args
                    .into_iter()
                    .next()
                    .ok_or_else(|| RuntimeError::native(format!("`{type_name}.from` expects one argument")))?;
                convert_from(&type_name, arg)
            }
            // ── SPEC 부록 @fs.read / @fs.write ──
            //
            // SPEC §10.3 `File` 레코드: `{name, path, content}` 구조로 노출해
            // `file.content` / `file.path` 등 필드 접근을 지원한다. 파일이
            // 없거나 읽기에 실패하면 동일 형태의 빈 레코드를 돌려 예시
            // 스크립트가 끊기지 않도록 한다 (실패 모델은 후속 마일스톤).
            (Value::TypeName(ns), "read") if ns == "fs" => {
                let Some(Value::Str(path)) = args.into_iter().next() else {
                    return Err(RuntimeError::native("`@fs.read` expects a string path"));
                };
                let content = std::fs::read_to_string(&path).unwrap_or_default();
                let name = std::path::Path::new(&path)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();
                Ok(Value::Object(vec![
                    ("name".to_string(), Value::Str(name)),
                    ("path".to_string(), Value::Str(path)),
                    ("content".to_string(), Value::Str(content)),
                ]))
            }
            (Value::TypeName(ns), "write") if ns == "fs" => {
                let mut it = args.into_iter();
                let Some(Value::Str(path)) = it.next() else {
                    return Err(RuntimeError::native("`@fs.write` expects (path, content)"));
                };
                // content 는 임의 값을 허용한다. 문자열은 그대로, 객체/배열은
                // JSON 유사 직렬화(MVP: Display) 로 떨어뜨린다. encoding 인자
                // (`utf-8` 등) 는 MVP 에서 무시한다.
                let content_v = it
                    .next()
                    .ok_or_else(|| RuntimeError::native("`@fs.write` expects (path, content)"))?;
                let content = match content_v {
                    Value::Str(s) => s,
                    other => format!("{other}"),
                };
                std::fs::write(&path, &content)
                    .map(|_| Value::Void)
                    .map_err(|e| RuntimeError::native(format!("`@fs.write` failed: {e}")))
            }
            // ── SPEC 부록 @process.run ──
            //
            // `sh -c <cmd>` 로 실행하고 stdout/stderr/status 를 포함한 object 를
            // 반환한다. stdin / env / cwd 는 MVP 에 포함되지 않는다.
            (Value::TypeName(ns), "run") if ns == "process" => {
                let Some(Value::Str(cmd)) = args.into_iter().next() else {
                    return Err(RuntimeError::native(
                        "`@process.run` expects a string command",
                    ));
                };
                let output = std::process::Command::new("sh")
                    .arg("-c")
                    .arg(&cmd)
                    .output()
                    .map_err(|e| {
                        RuntimeError::native(format!("`@process.run` failed: {e}"))
                    })?;
                let stdout_s = String::from_utf8_lossy(&output.stdout).into_owned();
                let stderr_s = String::from_utf8_lossy(&output.stderr).into_owned();
                let status = i64::from(output.status.code().unwrap_or(-1));
                // SPEC §10.6 ProcessResult: `{code, output, error}` + 기존
                // `{stdout, stderr, status}` 별칭 유지. 두 이름 집합을 모두
                // 노출해 fixture 예시(`.code` / `.output`) 와 기존 테스트
                // (`.stdout` / `.status`) 가 공존한다.
                Ok(Value::Object(vec![
                    ("stdout".into(), Value::Str(stdout_s.clone())),
                    ("stderr".into(), Value::Str(stderr_s.clone())),
                    ("status".into(), Value::Int(status)),
                    ("code".into(), Value::Int(status)),
                    ("output".into(), Value::Str(stdout_s)),
                    ("error".into(), Value::Str(stderr_s)),
                ]))
            }
            // ── C_db 메서드 ──
            //
            // 시그니처 (MVP):
            //   db.create(table: string, data: object) -> object
            //   db.find(table: string, filter: object) -> object | void
            //   db.findAll(table: string, filter: object?) -> object[]
            //   db.update(table: string, filter: object, data: object) -> int
            //   db.delete(table: string, filter: object) -> int
            (Value::Db(db), m @ ("create" | "find" | "findAll" | "update" | "delete")) => {
                call_db_method(&db, m, args)
            }
            (recv, m) => Err(RuntimeError::native(format!("no method `{m}` on {recv}"))),
        }
    }

    /// `call_function` 의 확장 — param 인자 외 추가 바인딩(token slot 등)을
    /// 함수 스코프에 같이 삽입한다. 현재는 `call_user_domain` 에서 token slot
    /// 을 전달할 때만 사용. 일반 호출 경로는 `call_function` 그대로 유지.
    fn call_function_with_extras(
        &mut self,
        func: &HirFunctionStmt,
        args: Vec<Value>,
        extras: Vec<(NameId, Value)>,
    ) -> Result<Value, RuntimeError> {
        if args.len() != func.params.len() {
            return Err(RuntimeError::native(format!(
                "function `{}` expects {} arguments, got {}",
                func.name.name,
                func.params.len(),
                args.len()
            )));
        }
        let saved = std::mem::take(&mut self.env);
        self.env = saved.clone();
        for (p, v) in func.params.iter().zip(args) {
            self.env.insert(p.name.id, v);
        }
        for (id, v) in extras {
            self.env.insert(id, v);
        }
        let saved_return = self.pending_return.take();
        let saved_html = self.html_buffer.take();
        let saved_loop = self.loop_signal;
        let result_value = match &func.body {
            HirFunctionBody::Block(b) => {
                let ctl = self.eval_block_ctl(b)?;
                self.pending_return = None;
                ctl.into_value()
            }
            HirFunctionBody::Expr(e) => self.eval(e)?,
        };
        self.html_buffer = saved_html;
        if self.response.is_some() {
            self.pending_return = Some(Value::Void);
        } else {
            self.pending_return = saved_return;
        }
        self.env = saved;
        self.loop_signal = saved_loop;
        Ok(result_value)
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
        let saved = std::mem::take(&mut self.env);
        self.env = saved.clone();
        for (p, v) in func.params.iter().zip(args) {
            self.env.insert(p.name.id, v);
        }
        let saved_return = self.pending_return.take();
        let saved_html = self.html_buffer.take();
        let saved_loop = self.loop_signal;
        let result_value = match &func.body {
            HirFunctionBody::Block(b) => {
                let ctl = self.eval_block_ctl(b)?;
                self.pending_return = None;
                ctl.into_value()
            }
            HirFunctionBody::Expr(e) => self.eval(e)?,
        };
        self.html_buffer = saved_html;
        if self.response.is_some() {
            self.pending_return = Some(Value::Void);
        } else {
            self.pending_return = saved_return;
        }
        self.env = saved;
        self.loop_signal = saved_loop;
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
                    let rc = Rc::new((**f).clone());
                    self.env.insert(f.name.id, Value::Function(rc.clone()));
                    if f.is_define {
                        register_nested_defines(&mut self.env, &f.name.name, f);
                    }
                }
                HirStmt::Struct(_) => {}
                HirStmt::Enum(e) => {
                    let mut fields: Vec<(String, Value)> = Vec::with_capacity(e.variants.len());
                    for v in &e.variants {
                        let val = self.eval(&v.value)?;
                        fields.push((v.name.clone(), val));
                    }
                    self.env.insert(e.name.id, Value::Object(fields));
                }
                HirStmt::Import(_) => {}
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
            HirPattern::Not(expr) => {
                // `!EXPR` — 값이 expected 와 같지 않으면 매치.
                let expected = self.eval(expr)?;
                !values_equal(&expected, value)
            }
            HirPattern::Contains(expr) => {
                // `in EXPR` — 스크루티니 컬렉션/문자열이 값을 포함하면 매치.
                let needle = self.eval(expr)?;
                match (value, &needle) {
                    (Value::Array(items), _) => items.iter().any(|v| values_equal(v, &needle)),
                    (Value::Str(s), Value::Str(sub)) => s.contains(sub.as_str()),
                    (Value::Object(fields), Value::Str(key)) => {
                        fields.iter().any(|(k, _)| k == key.as_str())
                    }
                    _ => false,
                }
            }
        })
    }

    /// SPEC §9.3: 대문자 user-domain 호출 — property + positional 을 function
    /// signature 에 바인딩해 호출한다.
    ///
    /// property (`ExprKind::Assign { target, value }`) 는 target 이름으로 param
    /// 매칭. positional 은 property 가 아직 채우지 않은 param 에 순서대로.
    /// 누락된 nullable param 은 `Value::Void` 로 채운다. non-nullable 은 에러.
    ///
    /// `HirTypeRef` 의 nullable 판정은 현재 `HirTypeRefKind::Nullable` 구조
    /// 기반 — 타입 어노테이션이 없거나 Nullable 이면 void 허용, 그 외는 필수.
    /// Stage 2 이후 token slot 까지 오면 positional 은 token array 로 흡수
    /// 되므로 이 매핑은 그 시점에 재정의된다.
    fn call_user_domain(
        &mut self,
        func: &Rc<HirFunctionStmt>,
        args: &[HirExpr],
    ) -> Result<Value, RuntimeError> {
        use orv_hir::HirTypeRefKind;
        // 1) property / positional / content-block 분리 + 평가.
        //    SPEC §9.5 규칙상 block literal 은 호출 인자 목록의 마지막 항목이
        //    며, 정확히 하나만 허용된다. @content 가 평가 시 소비한다.
        let mut props: Vec<(String, Value)> = Vec::new();
        let mut positional: Vec<Value> = Vec::new();
        let mut content_block: Option<HirBlock> = None;
        for a in args {
            match &a.kind {
                HirExprKind::Assign { target, value } => {
                    let v = self.eval(value)?;
                    props.push((target.name.clone(), v));
                }
                HirExprKind::Block(block) => {
                    // block 이 여러 번 오면 마지막을 content slot 으로 쓴다.
                    content_block = Some(block.clone());
                }
                _ => {
                    let v = self.eval(a)?;
                    positional.push(v);
                }
            }
        }

        // 2) param 별 값 결정. 규칙:
        //    - property (key=value) 는 param 이름으로 매칭 (최우선).
        //    - token slot 이 선언돼 있지 않으면 남은 positional 을 param 에
        //      순서대로 할당 — paren 호출 `@Add(1, 2)` 같은 일반 호출 형태를
        //      계속 지원하기 위함.
        //    - token slot 이 있으면 positional 은 전부 token slot 으로 흡수
        //      (Stage 2 규약).
        //    - nullable 은 누락 시 void, non-nullable 은 에러.
        let has_token_slots = !func.token_slots.is_empty();
        let param_values = func
            .params
            .iter()
            .map(|p| {
                let pname = &p.name.name;
                if let Some(idx) = props.iter().position(|(k, _)| k == pname) {
                    return Ok(props.remove(idx).1);
                }
                if !has_token_slots && !positional.is_empty() {
                    return Ok(positional.remove(0));
                }
                let is_nullable = matches!(
                    p.annotation.as_ref().map(|t| &t.kind),
                    Some(HirTypeRefKind::Nullable(_))
                );
                if is_nullable {
                    Ok(Value::Void)
                } else {
                    Err(RuntimeError::native(format!(
                        "`@{}` missing required property `{pname}`",
                        func.name.name
                    )))
                }
            })
            .collect::<Result<Vec<_>, _>>()?;

        // 3) 초과 property 는 에러 (param 에 없는 key).
        if let Some((k, _)) = props.first() {
            return Err(RuntimeError::native(format!(
                "`@{}` got unknown property `{k}`",
                func.name.name
            )));
        }

        // 4) SPEC §9.4: 남은 positional 은 token slot 에 `Value::Array` 로 흡수.
        //    현재 MVP 는 첫 slot 에 모든 positional 을 catch-all 로 넣는다
        //    (타입 패턴 매칭은 타입 체커 합류 이후).
        //    slot 이 없으면 positional 은 에러 — 기존 `call_function` 의 arity
        //    검사가 잡아 주지만 더 이른 진단을 위해 여기서도 확인.
        let token_bindings: Vec<(NameId, Value)> = if func.token_slots.is_empty() {
            if !positional.is_empty() {
                return Err(RuntimeError::native(format!(
                    "`@{}` got {} positional arg(s) but declares no token slot",
                    func.name.name,
                    positional.len()
                )));
            }
            Vec::new()
        } else {
            let first = &func.token_slots[0];
            let values = std::mem::take(&mut positional);
            let mut pairs: Vec<(NameId, Value)> =
                vec![(first.name.id, Value::Array(values))];
            // 다른 slot 들은 현재 빈 배열로 초기화 (MVP).
            for slot in func.token_slots.iter().skip(1) {
                pairs.push((slot.name.id, Value::Array(Vec::new())));
            }
            pairs
        };

        // 5) SPEC §9.5 `@content`: 호출부 block 을 slot 에 장착 후 body 평가.
        //    호출 경계에서 save/restore — nested define 호출도 자기 slot 을 본다.
        let saved_content = std::mem::replace(&mut self.content_slot, content_block);
        let result = self.call_function_with_extras(func, param_values, token_bindings);
        self.content_slot = saved_content;
        result
    }

    /// C_middleware: `@before { body }` 를 평가한다.
    ///
    /// define 본문 안에 등장하면 middleware 로서의 역할을 하며, body block 을
    /// 즉시 평가한다. body 안의 `@next {k: v}` 는 context 에 값을 쌓고,
    /// `@respond` 는 early-return (handler/caller 모두 종료).
    ///
    /// define 외부(REPL 등) 에서 호출돼도 body 가 그대로 평가되는 건 동일.
    /// SPEC §11.6 의 `@before` 는 "route handler 실행 전에 확장" 이므로
    /// 확장 = body 평가로 모델링한다.
    fn eval_before(&mut self, args: &[HirExpr]) -> Result<Value, RuntimeError> {
        // 인자 없는 `@before` 는 선언 위치 표식용. noop.
        let Some(arg) = args.first() else {
            return Ok(Value::Void);
        };
        if let HirExprKind::Block(block) = &arg.kind {
            self.eval_block(block)?;
            Ok(Value::Void)
        } else {
            // `@before expr` 형태는 SPEC 에 없지만 관용적으로 평가한다.
            self.eval(arg)
        }
    }

    /// C_middleware: `@after { body }` 등록.
    ///
    /// body 는 handler 본문이 완전히 끝난 뒤 flush 되므로, 이 지점에서는 평가
    /// 하지 않고 block 을 복제해 `after_queue` 에 push 한다. handler 경계 밖
    /// (request 없음) 에서는 즉시 평가 — fixture/REPL 동작 단순화.
    fn eval_after(&mut self, args: &[HirExpr]) -> Result<Value, RuntimeError> {
        let Some(arg) = args.first() else {
            return Ok(Value::Void);
        };
        if let HirExprKind::Block(block) = &arg.kind {
            if self.request.is_some() {
                self.after_queue.push(block.clone());
                return Ok(Value::Void);
            }
            // handler 경계 밖: 즉시 평가 (대부분 fixture/test 용).
            self.eval_block(block)?;
            Ok(Value::Void)
        } else {
            self.eval(arg)
        }
    }

    /// C_middleware: `@next {k: v}` 로 context 에 값 머지.
    ///
    /// 인자 없는 `@next` 는 pass-through — middleware 체인에서 "변경 없이 다음
    /// 단계로" 신호. 인자가 object literal 이 아니면 에러.
    fn eval_next(&mut self, args: &[HirExpr]) -> Result<Value, RuntimeError> {
        let Some(arg) = args.first() else {
            return Ok(Value::Void);
        };
        let value = self.eval(arg)?;
        match value {
            Value::Object(pairs) => {
                for (k, v) in pairs {
                    // 같은 key 가 이미 있으면 제거 후 새로 push — 마지막 값 우세.
                    self.context.retain(|(ek, _)| ek != &k);
                    self.context.push((k, v));
                }
                Ok(Value::Void)
            }
            Value::Void => Ok(Value::Void),
            other => Err(RuntimeError::native(format!(
                "`@next` expects an object literal `{{...}}`, got {other}"
            ))),
        }
    }

    /// SPEC §11.9 `@redirect` — HTTP redirect 응답 기록 + early-return.
    ///
    /// 형태:
    /// - `@redirect "/path"` — 302 Found.
    /// - `@redirect 301 "/moved"` — 명시적 status + URL.
    ///
    /// let-binding 된 route 를 넘기는 `@redirect loginRoute` 형태는 현재 range
    /// 밖 — route 메타데이터 lookup 이 필요하다.
    fn eval_redirect(&mut self, args: &[HirExpr]) -> Result<Value, RuntimeError> {
        let (status, target) = match args.len() {
            1 => (302i64, self.eval(&args[0])?),
            2 => {
                let status_val = self.eval(&args[0])?;
                let Value::Int(n) = status_val else {
                    return Err(RuntimeError::native(format!(
                        "`@redirect` first argument must be integer status, got {status_val}"
                    )));
                };
                (n, self.eval(&args[1])?)
            }
            _ => {
                return Err(RuntimeError::native(
                    "`@redirect` expects URL or (status, URL)",
                ));
            }
        };
        let Value::Str(url) = target else {
            return Err(RuntimeError::native(format!(
                "`@redirect` URL must be a string, got {target}"
            )));
        };
        if self.response.is_none() {
            self.response = Some(ResponseCtx {
                status,
                payload: Value::Void,
                raw_body: None,
                location: Some(url),
            });
        }
        self.pending_return = Some(Value::Void);
        Ok(Value::Void)
    }

    /// `@serve "path"` — 정적 파일/디렉토리 서빙 (A5a + A5b).
    ///
    /// 두 모드:
    /// - **A5a 단일 파일**: `path` 가 regular file → 바이트 그대로 + MIME.
    /// - **A5b 디렉토리**: `path` 가 directory → 요청 핸들러의 `@param.rest`
    ///   (예약 이름) 를 `/` 로 join 해 최종 파일 경로 생성. 파일 발견되면
    ///   A5a 와 같은 경로로 응답.
    ///
    /// 크기 캡 10MB 공통. 에러 상태:
    /// - 파일 없음 → 404
    /// - 디렉토리지만 `rest` 파라미터 없음 → 500 (라우트 선언 오류)
    /// - `rest` 에 `..` 세그먼트 포함 → 403 (문법적 traversal)
    /// - canonicalize 결과가 root 밖 → 403 (심볼릭/상대경로 traversal)
    /// - 심볼릭 링크 → 403 (더 관대한 정책은 후속 논의)
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
        let declared = std::path::Path::new(&path_str);

        // 1) 대상 분류 — 파일이면 바로 서빙, 디렉토리면 rest join 후 재시도.
        let meta = match std::fs::metadata(declared) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return self.respond_status(404);
            }
            Err(e) => {
                return Err(RuntimeError::native(format!(
                    "`@serve` metadata failed: {e}"
                )));
            }
        };

        let target_path: std::path::PathBuf = if meta.is_file() {
            declared.to_path_buf()
        } else if meta.is_dir() {
            let rest = self
                .request
                .as_ref()
                .and_then(|r| r.params.get("rest"))
                .cloned();
            let Some(rest) = rest else {
                return Err(RuntimeError::native(
                    "`@serve` on directory requires `@param.rest` — declare route as `/prefix/:rest*`"
                ));
            };
            // 문법적 traversal 차단.
            if rest.split('/').any(|seg| seg == "..") {
                return self.respond_status(403);
            }
            let candidate = declared.join(&rest);

            // canonicalize 양쪽 후 prefix 검사.
            let root_canon = match declared.canonicalize() {
                Ok(p) => p,
                Err(e) => {
                    return Err(RuntimeError::native(format!(
                        "`@serve` root canonicalize failed: {e}"
                    )));
                }
            };
            let target_canon = match candidate.canonicalize() {
                Ok(p) => p,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return self.respond_status(404);
                }
                Err(e) => {
                    return Err(RuntimeError::native(format!(
                        "`@serve` target canonicalize failed: {e}"
                    )));
                }
            };
            if !target_canon.starts_with(&root_canon) {
                return self.respond_status(403);
            }

            // 심볼릭 링크 거부: canonicalize 는 따라가므로 별도로 symlink
            // metadata 로 확인한다.
            match std::fs::symlink_metadata(&candidate) {
                Ok(sm) if sm.file_type().is_symlink() => {
                    return self.respond_status(403);
                }
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return self.respond_status(404);
                }
                Err(e) => {
                    return Err(RuntimeError::native(format!(
                        "`@serve` symlink check failed: {e}"
                    )));
                }
            }

            target_canon
        } else {
            return Err(RuntimeError::native(format!(
                "`@serve` target is neither file nor directory: {path_str}"
            )));
        };

        // 2) 최종 대상 파일 읽어 응답.
        let final_meta = match std::fs::metadata(&target_path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return self.respond_status(404);
            }
            Err(e) => {
                return Err(RuntimeError::native(format!(
                    "`@serve` final metadata failed: {e}"
                )));
            }
        };
        if !final_meta.is_file() {
            // 디렉토리 인덱스 서빙은 범위 밖 — 404.
            return self.respond_status(404);
        }
        const MAX_SERVE_BYTES: u64 = 10 * 1024 * 1024;
        if final_meta.len() > MAX_SERVE_BYTES {
            return Err(RuntimeError::native(format!(
                "`@serve` file exceeds {MAX_SERVE_BYTES} bytes: {}",
                target_path.display()
            )));
        }
        let bytes = std::fs::read(&target_path)
            .map_err(|e| RuntimeError::native(format!("`@serve` read failed: {e}")))?;
        let mime = mime_for_path(&target_path);
        if self.response.is_none() {
            self.response = Some(ResponseCtx {
                status: 200,
                payload: Value::Void,
                raw_body: Some(RawResponseBody {
                    bytes,
                    content_type: mime,
                }),
                location: None,
            });
        }
        self.pending_return = Some(Value::Void);
        Ok(Value::Void)
    }

    /// 단순 상태 코드만 가진 빈 body 응답을 기록하고 early-return 한다.
    /// `@serve` 가 404/403 같이 body 없는 실패 응답을 반환할 때 사용한다.
    fn respond_status(&mut self, status: i64) -> Result<Value, RuntimeError> {
        if self.response.is_none() {
            self.response = Some(ResponseCtx {
                status,
                payload: Value::Void,
                raw_body: None,
                location: None,
            });
        }
        self.pending_return = Some(Value::Void);
        Ok(Value::Void)
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

/// SPEC §9.6: parent define body 안에 선언된 nested `define` 들을 dotted
/// 이름(`Parent.Child.Inner` 등) 으로 바꾼 clone 을 만들어 env 에 등록한다.
/// 재귀적으로 더 깊은 중첩도 따라 내려간다.
///
/// 기존 domain-call 선형 탐색(`f.name.name == requested_name`)이 dotted 이름을
/// 그대로 매칭하도록, `HirIdent::name` 만 바꾼 새 `HirFunctionStmt` 를 만들어
/// 새 `NameId` 없이 등록한다 (NameId 충돌 방지 위해 기존 id 와 다른 충분히 큰
/// 값을 쓰거나, id 는 그대로 두고 이름만 바꾼다 — 런타임 lookup 은 이름으로
/// 하므로 id 충돌은 실제로는 영향 없음).
fn register_nested_defines(
    env: &mut HashMap<NameId, Value>,
    parent_path: &str,
    parent: &HirFunctionStmt,
) {
    let stmts = match &parent.body {
        HirFunctionBody::Block(b) => &b.stmts[..],
        HirFunctionBody::Expr(_) => return,
    };
    for stmt in stmts {
        if let HirStmt::Function(child) = stmt {
            if !child.is_define {
                continue;
            }
            let dotted = format!("{parent_path}.{}", child.name.name);
            // 이름만 dotted 로 교체한 clone. NameId 는 원본 그대로 — domain
            // lookup 은 name 문자열 비교. env 맵 key 충돌을 피하기 위해
            // dotted-name 항목은 새 NameId 슬롯(u32::MAX - serial) 을 쓴다.
            // 간단히 현재 env 크기를 뒤집어 유일 키 생성.
            let mut cloned = (**child).clone();
            cloned.name.name = dotted.clone();
            let slot = NameId(u32::MAX - u32::try_from(env.len()).unwrap_or(0));
            env.insert(slot, Value::Function(Rc::new(cloned)));
            // 재귀 — `Parent.Child.Inner` 도 등록.
            register_nested_defines(env, &dotted, child);
        }
    }
}

/// SPEC §4.1/§4.9: 원시 타입 이름 여부. 스코프 섀도잉이 없을 때 namespace
/// 핸들로 해석되는 식별자 집합.
fn is_primitive_type_name(name: &str) -> bool {
    matches!(
        name,
        "int" | "uint"
            | "byte"
            | "ubyte"
            | "short"
            | "ushort"
            | "long"
            | "ulong"
            | "float"
            | "double"
            | "string"
            | "bool"
    )
}

/// SPEC §13 내장 전역 함수 이름 — resolver 의 `is_builtin_name` 과 대칭.
fn is_builtin_fn_name(name: &str) -> bool {
    matches!(
        name,
        "Type"
            | "max"
            | "min"
            | "abs"
            | "sin"
            | "cos"
            | "tan"
            | "log"
            | "sqrt"
            | "pow"
            | "floor"
            | "ceil"
            | "round"
            | "now"
            | "today"
            | "tomorrow"
            | "yesterday"
            | "sleep"
    )
}

/// 내장 함수 호출 dispatcher.
///
/// MVP 구현 — 실제 OS/네트워크 연동이 필요한 항목(`sleep`) 은 no-op 로
/// 떨어뜨리고, 시간 함수는 `chrono` 가 선택 의존이 아니므로 자체 struct
/// 로 현재 시각을 구한다. 각 함수의 인자 검증은 가볍게 하고 실패 시
/// [`RuntimeError::native`] 로 내린다.
fn call_builtin(name: &str, args: Vec<Value>) -> Result<Value, RuntimeError> {
    match name {
        "Type" => {
            let v = args
                .into_iter()
                .next()
                .ok_or_else(|| RuntimeError::native("Type expects 1 argument"))?;
            Ok(Value::Str(type_of(&v).to_string()))
        }
        "max" | "min" => numeric_fold(name, args),
        "abs" => {
            let v = args
                .into_iter()
                .next()
                .ok_or_else(|| RuntimeError::native("abs expects 1 argument"))?;
            match v {
                Value::Int(n) => Ok(Value::Int(n.abs())),
                Value::Float(f) => Ok(Value::Float(f.abs())),
                other => Err(RuntimeError::native(format!(
                    "abs: unsupported argument type {other}"
                ))),
            }
        }
        "sin" | "cos" | "tan" | "log" | "sqrt" | "floor" | "ceil" | "round" => {
            let v = args
                .into_iter()
                .next()
                .ok_or_else(|| RuntimeError::native(format!("{name} expects 1 argument")))?;
            let x = value_to_f64(&v).ok_or_else(|| {
                RuntimeError::native(format!("{name}: expected number, got {v}"))
            })?;
            let result = match name {
                "sin" => x.sin(),
                "cos" => x.cos(),
                "tan" => x.tan(),
                "log" => x.ln(),
                "sqrt" => x.sqrt(),
                "floor" => x.floor(),
                "ceil" => x.ceil(),
                "round" => x.round(),
                _ => unreachable!(),
            };
            Ok(Value::Float(result))
        }
        "pow" => {
            let mut it = args.into_iter();
            let base = it.next().ok_or_else(|| RuntimeError::native("pow expects 2 arguments"))?;
            let exp = it.next().ok_or_else(|| RuntimeError::native("pow expects 2 arguments"))?;
            let b = value_to_f64(&base)
                .ok_or_else(|| RuntimeError::native(format!("pow: expected number, got {base}")))?;
            let e = value_to_f64(&exp)
                .ok_or_else(|| RuntimeError::native(format!("pow: expected number, got {exp}")))?;
            Ok(Value::Float(b.powf(e)))
        }
        "now" => Ok(time_value(SystemTimeOffset::Now)),
        "today" => Ok(time_value(SystemTimeOffset::Today)),
        "tomorrow" => Ok(time_value(SystemTimeOffset::Tomorrow)),
        "yesterday" => Ok(time_value(SystemTimeOffset::Yesterday)),
        // MVP: `sleep` 은 값만 인정하고 실제 대기는 하지 않는다. 테스트
        // 런타임(동기 interpreter) 에서 실제 대기를 끼얹으면 UX 저하가 커서
        // no-op. 향후 비동기 스케줄러가 붙으면 실구현으로 교체.
        "sleep" => Ok(Value::Void),
        other => Err(RuntimeError::native(format!(
            "unknown builtin `{other}`"
        ))),
    }
}

fn type_of(v: &Value) -> &'static str {
    match v {
        Value::Int(_) => "int",
        Value::Float(_) => "float",
        Value::Str(_) => "string",
        Value::Bool(_) => "bool",
        Value::Void => "void",
        Value::Function(_) | Value::Lambda(_) | Value::BoundMethod { .. } | Value::Builtin(_) => {
            "function"
        }
        Value::Array(_) => "array",
        Value::Object(_) => "object",
        Value::Db(_) => "db",
        Value::TypeName(_) => "type",
    }
}

fn value_to_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Int(n) => Some(*n as f64),
        Value::Float(f) => Some(*f),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    }
}

fn numeric_fold(op: &str, args: Vec<Value>) -> Result<Value, RuntimeError> {
    if args.is_empty() {
        return Err(RuntimeError::native(format!(
            "{op} expects at least one argument"
        )));
    }
    // int-only 인지 판별 — 모든 인자가 Int 이면 Int 로 반환, 하나라도 Float 면
    // Float 로 포함시킨다. 대소 비교는 공통으로 f64 경유.
    let mut all_int = matches!(&args[0], Value::Int(_));
    if !matches!(&args[0], Value::Int(_) | Value::Float(_)) {
        return Err(RuntimeError::native(format!(
            "{op}: unsupported argument type {}",
            &args[0]
        )));
    }
    let mut best_f = value_to_f64(&args[0]).unwrap();
    let mut best_idx = 0usize;
    for (i, v) in args.iter().enumerate().skip(1) {
        let f = value_to_f64(v)
            .ok_or_else(|| RuntimeError::native(format!("{op}: unsupported argument type {v}")))?;
        let is_better = match op {
            "max" => f > best_f,
            "min" => f < best_f,
            _ => unreachable!(),
        };
        if is_better {
            best_f = f;
            best_idx = i;
        }
        if matches!(v, Value::Float(_)) {
            all_int = false;
        }
    }
    if all_int {
        if let Value::Int(n) = args[best_idx] {
            return Ok(Value::Int(n));
        }
    }
    Ok(Value::Float(best_f))
}

/// `now()` 류 시간 함수가 반환할 오프셋.
enum SystemTimeOffset {
    Now,
    Today,
    Tomorrow,
    Yesterday,
}

/// 현재 시각 기반으로 `{year, month, day, hour, minute, second}` object Value 를
/// 만든다. `today`/`tomorrow`/`yesterday` 는 시/분/초가 0. `chrono` 의존을
/// 늘리지 않기 위해 `std::time::SystemTime` + 작은 달력 계산으로 충분하게
/// 근사한다 (그레고리안 기준, UTC).
fn time_value(offset: SystemTimeOffset) -> Value {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    // 날짜 단위 오프셋.
    let day_secs: i64 = 86_400;
    let base_secs = match offset {
        SystemTimeOffset::Now => secs,
        SystemTimeOffset::Today => secs - (secs.rem_euclid(day_secs)),
        SystemTimeOffset::Tomorrow => secs - (secs.rem_euclid(day_secs)) + day_secs,
        SystemTimeOffset::Yesterday => secs - (secs.rem_euclid(day_secs)) - day_secs,
    };
    let (y, mo, d, h, mi, s) = seconds_to_ymdhms(base_secs);
    Value::Object(vec![
        ("year".to_string(), Value::Int(y)),
        ("month".to_string(), Value::Int(mo)),
        ("day".to_string(), Value::Int(d)),
        ("hour".to_string(), Value::Int(h)),
        ("minute".to_string(), Value::Int(mi)),
        ("second".to_string(), Value::Int(s)),
    ])
}

/// UNIX epoch 초를 UTC `(year, month, day, hour, minute, second)` 로 분해.
///
/// 윤년 포함 Zeller-free 날짜 계산. `chrono` 의존 없이 MVP 시간 함수에만
/// 쓰이므로 정밀도는 초 단위로 충분하다.
fn seconds_to_ymdhms(total: i64) -> (i64, i64, i64, i64, i64, i64) {
    let day_secs: i64 = 86_400;
    let days = total.div_euclid(day_secs);
    let time_of_day = total.rem_euclid(day_secs);
    let h = time_of_day / 3600;
    let mi = (time_of_day % 3600) / 60;
    let s = time_of_day % 60;

    // 1970-01-01 을 0 일 기준으로 역산.
    let mut year: i64 = 1970;
    let mut days_left = days;
    loop {
        let y_days = if is_leap_year(year) { 366 } else { 365 };
        if days_left >= y_days {
            days_left -= y_days;
            year += 1;
        } else if days_left < 0 {
            year -= 1;
            let prev_y_days = if is_leap_year(year) { 366 } else { 365 };
            days_left += prev_y_days;
        } else {
            break;
        }
    }
    let month_lens = month_lengths(year);
    let mut month: i64 = 1;
    for len in month_lens {
        if days_left < len {
            break;
        }
        days_left -= len;
        month += 1;
    }
    let day = days_left + 1;
    (year, month, day, h, mi, s)
}

const fn is_leap_year(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn month_lengths(year: i64) -> [i64; 12] {
    [
        31,
        if is_leap_year(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ]
}

/// SPEC §4.9 `T.from(v)` 타입 변환 dispatcher.
///
/// MVP 규약:
/// - `int.from(str)` — 10진 정수 파싱, 실패 시 RuntimeError.
/// - `int.from(float)` — truncate.
/// - `int.from(bool)` — true→1, false→0.
/// - `float.from(str)` — 부동소수점 파싱.
/// - `float.from(int)` — 단순 캐스트.
/// - `string.from(any)` — `Display` 기반 문자열화.
/// - `bool.from(str)` — "true"/"false".
fn convert_from(type_name: &str, v: Value) -> Result<Value, RuntimeError> {
    match (type_name, v) {
        ("int", Value::Int(n)) => Ok(Value::Int(n)),
        ("int", Value::Float(f)) => Ok(Value::Int(f as i64)),
        ("int", Value::Bool(b)) => Ok(Value::Int(i64::from(b))),
        ("int", Value::Str(s)) => s
            .trim()
            .parse::<i64>()
            .map(Value::Int)
            .map_err(|_| RuntimeError::native(format!("int.from failed to parse `{s}`"))),
        ("float", Value::Float(f)) => Ok(Value::Float(f)),
        ("float", Value::Int(n)) => Ok(Value::Float(n as f64)),
        ("float", Value::Str(s)) => s
            .trim()
            .parse::<f64>()
            .map(Value::Float)
            .map_err(|_| RuntimeError::native(format!("float.from failed to parse `{s}`"))),
        ("string", v) => Ok(Value::Str(format!("{v}"))),
        ("bool", Value::Bool(b)) => Ok(Value::Bool(b)),
        ("bool", Value::Str(s)) => match s.as_str() {
            "true" => Ok(Value::Bool(true)),
            "false" => Ok(Value::Bool(false)),
            _ => Err(RuntimeError::native(format!(
                "bool.from expects \"true\" or \"false\", got \"{s}\""
            ))),
        },
        (ty, v) => Err(RuntimeError::native(format!(
            "{ty}.from: unsupported conversion from {v}"
        ))),
    }
}

/// C_db MVP 메서드 dispatcher.
///
/// 호출 규약:
/// - `create(table, data)` — 새 row insert, id 자동.
/// - `find(table, filter)` — equality filter 로 첫 매칭 or void.
/// - `findAll(table, filter?)` — equality filter 로 매칭 배열. filter 생략 시
///   전체 반환.
/// - `update(table, filter, data)` — filter 매칭에 data 병합. 갱신 수 반환.
/// - `delete(table, filter)` — filter 매칭 제거. 삭제 수 반환.
fn call_db_method(
    db: &Arc<Mutex<crate::db::InMemoryDb>>,
    method: &str,
    args: Vec<Value>,
) -> Result<Value, RuntimeError> {
    let require_str = |v: &Value, what: &str| -> Result<String, RuntimeError> {
        match v {
            Value::Str(s) => Ok(s.clone()),
            other => Err(RuntimeError::native(format!(
                "`db.{method}` expects {what} to be string, got {other}"
            ))),
        }
    };
    let require_obj = |v: &Value, what: &str| -> Result<Vec<(String, Value)>, RuntimeError> {
        match v {
            Value::Object(fields) => Ok(fields.clone()),
            other => Err(RuntimeError::native(format!(
                "`db.{method}` expects {what} to be object, got {other}"
            ))),
        }
    };
    match method {
        "create" => {
            if args.len() != 2 {
                return Err(RuntimeError::native(
                    "`db.create` expects (table, data)",
                ));
            }
            let table = require_str(&args[0], "table name")?;
            let data = require_obj(&args[1], "data")?;
            Ok(db.lock().unwrap().create(&table, data))
        }
        "find" => {
            if args.len() != 2 {
                return Err(RuntimeError::native(
                    "`db.find` expects (table, filter)",
                ));
            }
            let table = require_str(&args[0], "table name")?;
            let filter = require_obj(&args[1], "filter")?;
            Ok(db.lock().unwrap().find_one(&table, &filter))
        }
        "findAll" => {
            if args.is_empty() || args.len() > 2 {
                return Err(RuntimeError::native(
                    "`db.findAll` expects (table[, filter])",
                ));
            }
            let table = require_str(&args[0], "table name")?;
            let filter = if let Some(f) = args.get(1) {
                require_obj(f, "filter")?
            } else {
                Vec::new()
            };
            Ok(db.lock().unwrap().find_all(&table, &filter))
        }
        "update" => {
            if args.len() != 3 {
                return Err(RuntimeError::native(
                    "`db.update` expects (table, filter, data)",
                ));
            }
            let table = require_str(&args[0], "table name")?;
            let filter = require_obj(&args[1], "filter")?;
            let data = require_obj(&args[2], "data")?;
            Ok(Value::Int(db.lock().unwrap().update(&table, &filter, &data)))
        }
        "delete" => {
            if args.len() != 2 {
                return Err(RuntimeError::native(
                    "`db.delete` expects (table, filter)",
                ));
            }
            let table = require_str(&args[0], "table name")?;
            let filter = require_obj(&args[1], "filter")?;
            Ok(Value::Int(db.lock().unwrap().delete(&table, &filter)))
        }
        other => Err(RuntimeError::native(format!("unknown db method `{other}`"))),
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
            | HirExprKind::AssignField { .. }
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

/// SPEC §4.9 `expr as <type>` 런타임 변환.
///
/// MVP 규칙:
/// - 대상이 `Nullable(T)` 이면 안쪽 `T` 로 재귀 캐스트. void 는 그대로 통과.
/// - `int` 계열(`int`, `short`, `byte`, `long`, `uint`, `ulong` 등): Float 는
///   truncate, String 은 정수 파싱, Bool 은 0/1.
/// - `float` / `double`: Int 는 f64 로 확장, String 은 부동소수점 파싱.
/// - `string`: 모든 원시 값을 [`value_to_display`] 로 직렬화.
/// - `bool`: Int 는 nonzero, String 은 `"true"`/`"false"`, Bool 은 그대로.
/// - `void`: 무엇이든 `Value::Void`.
/// - 이외 이름 (사용자 struct 등) 은 원본 값을 그대로 유지한다 — 타입 체커
///   합류 후에 엄격화 가능.
fn apply_cast(value: Value, ty: &HirTypeRef) -> Result<Value, RuntimeError> {
    // Array(T) 는 pass-through (원소별 캐스트는 아직 미지원 — MVP 범위 밖).
    if matches!(ty.kind, HirTypeRefKind::Array(_)) {
        return Ok(value);
    }
    if let HirTypeRefKind::Nullable(inner) = &ty.kind {
        if matches!(value, Value::Void) {
            return Ok(Value::Void);
        }
        return apply_cast(value, inner);
    }
    let HirTypeRefKind::Named(name) = &ty.kind else {
        return Ok(value);
    };
    match name.as_str() {
        "int" | "uint" | "byte" | "ubyte" | "short" | "ushort" | "long" | "ulong" => match value {
            Value::Int(i) => Ok(Value::Int(i)),
            Value::Float(f) => Ok(Value::Int(f.trunc() as i64)),
            Value::Bool(b) => Ok(Value::Int(i64::from(b))),
            Value::Str(s) => s
                .trim()
                .parse::<i64>()
                .map(Value::Int)
                .map_err(|_| RuntimeError::native(format!("cannot cast string `{s}` to int"))),
            other => Err(RuntimeError::native(format!(
                "cannot cast {other} to int"
            ))),
        },
        "float" | "double" => match value {
            Value::Float(f) => Ok(Value::Float(f)),
            #[allow(clippy::cast_precision_loss)]
            Value::Int(i) => Ok(Value::Float(i as f64)),
            Value::Bool(b) => Ok(Value::Float(if b { 1.0 } else { 0.0 })),
            Value::Str(s) => s
                .trim()
                .parse::<f64>()
                .map(Value::Float)
                .map_err(|_| RuntimeError::native(format!("cannot cast string `{s}` to float"))),
            other => Err(RuntimeError::native(format!(
                "cannot cast {other} to float"
            ))),
        },
        "string" => Ok(Value::Str(value_to_display(&value))),
        "bool" => match value {
            Value::Bool(b) => Ok(Value::Bool(b)),
            Value::Int(i) => Ok(Value::Bool(i != 0)),
            Value::Float(f) => Ok(Value::Bool(f != 0.0)),
            Value::Str(s) => match s.as_str() {
                "true" => Ok(Value::Bool(true)),
                "false" => Ok(Value::Bool(false)),
                _ => Err(RuntimeError::native(format!(
                    "cannot cast string `{s}` to bool"
                ))),
            },
            other => Err(RuntimeError::native(format!(
                "cannot cast {other} to bool"
            ))),
        },
        "void" => Ok(Value::Void),
        // 사용자 정의 타입 (struct 이름 등) — MVP 는 pass-through.
        _ => Ok(value),
    }
}

/// SPEC 부록: `target[start:end]` 슬라이싱 — 문자열은 char 경계, 배열은 원소
/// 경계로 자른다. 음수 인덱스는 파이썬식으로 `length` 에서 뺀 값이며, 범위를
/// 벗어나면 단순 clamp (에러 대신 빈 결과) 한다.
fn apply_slice(
    target: Value,
    start: Option<Value>,
    end: Option<Value>,
) -> Result<Value, RuntimeError> {
    fn as_int(v: Option<Value>, what: &str) -> Result<Option<i64>, RuntimeError> {
        match v {
            None => Ok(None),
            Some(Value::Int(n)) => Ok(Some(n)),
            Some(other) => Err(RuntimeError::native(format!(
                "slice {what} must be an integer, got {other}"
            ))),
        }
    }
    fn resolve(range: Option<i64>, default: i64, n: i64) -> i64 {
        let raw = range.unwrap_or(default);
        let adjusted = if raw < 0 { raw + n } else { raw };
        adjusted.clamp(0, n)
    }
    let start_i = as_int(start, "start")?;
    let end_i = as_int(end, "end")?;
    match target {
        Value::Str(s) => {
            let chars: Vec<char> = s.chars().collect();
            let n = i64::try_from(chars.len()).unwrap_or(i64::MAX);
            let lo = resolve(start_i, 0, n);
            let hi = resolve(end_i, n, n);
            let (lo, hi) = if lo > hi { (lo, lo) } else { (lo, hi) };
            let slice: String = chars[lo as usize..hi as usize].iter().collect();
            Ok(Value::Str(slice))
        }
        Value::Array(items) => {
            let n = i64::try_from(items.len()).unwrap_or(i64::MAX);
            let lo = resolve(start_i, 0, n);
            let hi = resolve(end_i, n, n);
            let (lo, hi) = if lo > hi { (lo, lo) } else { (lo, hi) };
            Ok(Value::Array(items[lo as usize..hi as usize].to_vec()))
        }
        other => Err(RuntimeError::native(format!("cannot slice {other}"))),
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
        // SPEC §2.5 비트/시프트 — 정수 피연산자 한정.
        (BitAnd, Value::Int(a), Value::Int(b)) => Ok(Value::Int(a & b)),
        (BitOr, Value::Int(a), Value::Int(b)) => Ok(Value::Int(a | b)),
        (BitXor, Value::Int(a), Value::Int(b)) => Ok(Value::Int(a ^ b)),
        (Shl, Value::Int(a), Value::Int(b)) if (0..64).contains(&b) => {
            Ok(Value::Int(((a as i128) << b) as i64))
        }
        (Shr, Value::Int(a), Value::Int(b)) if (0..64).contains(&b) => Ok(Value::Int(a >> b)),
        // 부동소수점 비교.
        (Lt, Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a < b)),
        (Gt, Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a > b)),
        (Le, Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a <= b)),
        (Ge, Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a >= b)),
        // 문자열 비교 (사전식).
        (Lt, Value::Str(a), Value::Str(b)) => Ok(Value::Bool(a < b)),
        (Gt, Value::Str(a), Value::Str(b)) => Ok(Value::Bool(a > b)),
        (Le, Value::Str(a), Value::Str(b)) => Ok(Value::Bool(a <= b)),
        (Ge, Value::Str(a), Value::Str(b)) => Ok(Value::Bool(a >= b)),
        (Coalesce, l, r) => {
            if matches!(l, Value::Void) {
                Ok(r)
            } else {
                Ok(l)
            }
        }
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
        Value::Function(_)
        | Value::Lambda(_)
        | Value::BoundMethod { .. }
        | Value::Db(_)
        | Value::TypeName(_)
        | Value::Builtin(_) => true,
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
        (Value::TypeName(a), Value::TypeName(b)) => a == b,
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
    use orv_syntax::{lex, parse_with_newlines};

    fn run_str(src: &str) -> Result<String, RuntimeError> {
        let lx = lex(src, FileId(0));
        assert!(
            lx.diagnostics.is_empty(),
            "lex errors: {:?}",
            lx.diagnostics
        );
        let pr = parse_with_newlines(lx.tokens, FileId(0), lx.newlines);
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
    fn cast_int_to_float_and_back() {
        // SPEC §4.9: numeric width 캐스팅.
        let out = run_str(
            r#"
            let n: int = 8
            let f: float = n as float
            @out f
            let m: int = 3.9 as int
            @out m
            "#,
        )
        .unwrap();
        assert_eq!(out, "8\n3\n");
    }

    #[test]
    fn cast_string_to_int_parses() {
        // `@param.id as int` 같은 경로를 흉내 — string → int 는 파싱한다.
        let out = run_str(
            r#"
            let s: string = "42"
            let n: int = s as int
            @out n + 1
            "#,
        )
        .unwrap();
        assert_eq!(out, "43\n");
    }

    #[test]
    fn string_slice_basic() {
        // SPEC 부록 문자열 메서드: `[a:b]` / `[:b]` / `[a:]`.
        let out = run_str(
            r#"
            let s: string = "Hello World"
            @out s[0:5]
            @out s[6:]
            @out s[:5]
            "#,
        )
        .unwrap();
        assert_eq!(out, "Hello\nWorld\nHello\n");
    }

    #[test]
    fn string_slice_negative_and_full() {
        let out = run_str(
            r#"
            let s: string = "abcdef"
            @out s[-3:]
            @out s[:-2]
            @out s[:]
            "#,
        )
        .unwrap();
        assert_eq!(out, "def\nabcd\nabcdef\n");
    }

    #[test]
    fn single_line_if_body() {
        // SPEC §6.1: `if cond : <stmt>` 한 줄 조건문.
        let out = run_str(
            r#"
            let num: int = 10
            if num > 5 : @out "greater"
            "#,
        )
        .unwrap();
        assert_eq!(out, "greater\n");
    }

    #[test]
    fn single_line_if_inside_for_loop() {
        // 루프 본문의 한 줄 `continue` 도 같은 규약.
        let out = run_str(
            r#"
            for item in [1, 2, 3] {
              if item == 2 : continue
              @out item
            }
            "#,
        )
        .unwrap();
        assert_eq!(out, "1\n3\n");
    }

    #[test]
    fn array_slice_copies_range() {
        let out = run_str(
            r#"
            let arr: int[] = [10, 20, 30, 40, 50]
            let mid: int[] = arr[1:4]
            @out mid.length
            for v in mid { @out v }
            "#,
        )
        .unwrap();
        assert_eq!(out, "3\n20\n30\n40\n");
    }

    #[test]
    fn cast_to_string_uses_display() {
        let out = run_str(
            r#"
            let n: int = 7
            let s: string = n as string
            @out s + "!"
            "#,
        )
        .unwrap();
        assert_eq!(out, "7!\n");
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

    // --- B1: when 패턴 보강 (SPEC §6.3) ---

    #[test]
    fn when_guard_with_dollar_field_access() {
        // `$.length > 3` — `$` 에서 파생된 모든 식은 guard 로 인식돼야 함.
        let out = run_str(
            r#"
            let v = [1, 2, 3, 4, 5]
            when v {
              $.length > 3 -> @out "long"
              _ -> @out "short"
            }
            "#,
        )
        .unwrap();
        assert_eq!(out, "long\n");
    }

    #[test]
    fn when_negation_pattern() {
        // `!5` — 값이 5 가 아니면 매치.
        let out = run_str(
            r#"
            let n: int = 3
            when n {
              !5 -> @out "not five"
              _ -> @out "five"
            }
            "#,
        )
        .unwrap();
        assert_eq!(out, "not five\n");
    }

    #[test]
    fn when_negation_pattern_falls_through_on_equal() {
        let out = run_str(
            r#"
            let n: int = 5
            when n {
              !5 -> @out "not five"
              _ -> @out "five"
            }
            "#,
        )
        .unwrap();
        assert_eq!(out, "five\n");
    }

    #[test]
    fn when_in_pattern_on_array() {
        // `in 4` — 스크루티니 배열에 4 포함되면 매치.
        let out = run_str(
            r#"
            let v = [1, 2, 3, 4]
            when v {
              in 4 -> @out "has four"
              _ -> @out "no four"
            }
            "#,
        )
        .unwrap();
        assert_eq!(out, "has four\n");
    }

    #[test]
    fn when_in_pattern_on_string() {
        let out = run_str(
            r#"
            let s = "hello world"
            when s {
              in "world" -> @out "greeting"
              _ -> @out "other"
            }
            "#,
        )
        .unwrap();
        assert_eq!(out, "greeting\n");
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
        let pr = parse_with_newlines(lx.tokens, FileId(0), lx.newlines);
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
        let pr = parse_with_newlines(lx.tokens, FileId(0), lx.newlines);
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

    // --- C_html-min: @Name invoke ---

    #[test]
    fn user_domain_invoke_single_arg() {
        // SPEC §9.9: `@Name(arg)` — 대문자 시작 도메인은 사용자 정의
        // function/define 호출.
        let out = run_str(
            r#"
            define Greet(name: string) -> "Hello, {name}!"
            @out @Greet("orv")
            "#,
        )
        .unwrap();
        assert_eq!(out, "Hello, orv!\n");
    }

    #[test]
    fn user_domain_invoke_multi_arg() {
        let out = run_str(
            r#"
            define Add(a: int, b: int) -> a + b
            @out @Add(2, 3)
            "#,
        )
        .unwrap();
        assert_eq!(out, "5\n");
    }

    #[test]
    fn user_domain_invoke_no_args() {
        let out = run_str(
            r#"
            define Pi() -> 3.14159
            @out @Pi()
            "#,
        )
        .unwrap();
        assert_eq!(out, "3.14159\n");
    }

    #[test]
    fn user_domain_returning_html_renders() {
        // `-> @html { ... }` define 의 결과를 @Name 호출로 조합.
        let out = run_str(
            r#"
            define Title(text: string) -> @html { @h1 "{text}" }
            @out @Title("Welcome")
            "#,
        )
        .unwrap();
        assert_eq!(out, "<html><h1>Welcome</h1></html>\n");
    }

    // --- C0: define / pub define ---

    #[test]
    fn define_is_callable_like_function() {
        // SPEC §9: `define Name() -> body` 는 function 과 같은 invoke 경로.
        // C0 는 표면 키워드만 추가, 런타임은 function 처럼 동작.
        let out = run_str(
            r#"
            define Pi() -> 3.14159
            @out Pi()
            "#,
        )
        .unwrap();
        assert_eq!(out, "3.14159\n");
    }

    #[test]
    fn define_with_block_body_returns_last_expr() {
        let out = run_str(
            r#"
            define Greet(name: string) -> {
              "Hello, {name}!"
            }
            @out Greet("orv")
            "#,
        )
        .unwrap();
        assert_eq!(out, "Hello, orv!\n");
    }

    #[test]
    fn pub_define_parses() {
        // `pub` modifier 는 파서 통과만 필요. 의미론(export) 는 B3 import 에서.
        let out = run_str(
            r#"
            pub define Answer() -> 42
            @out Answer()
            "#,
        )
        .unwrap();
        assert_eq!(out, "42\n");
    }

    #[test]
    fn pub_function_parses() {
        let out = run_str(
            r#"
            pub function add(a: int, b: int): int -> a + b
            @out add(2, 3)
            "#,
        )
        .unwrap();
        assert_eq!(out, "5\n");
    }

    // --- B2: async/await (sync MVP) ---

    #[test]
    fn async_function_runs_synchronously() {
        // SPEC §7.1: `async function` 선언 + `await EXPR` 호출.
        // MVP 의미: async 는 타입 표면만, 실행은 sync. await 는 identity.
        let out = run_str(
            r#"
            async function greet(): string -> {
              "hello"
            }
            let msg: string = await greet()
            @out msg
            "#,
        )
        .unwrap();
        assert_eq!(out, "hello\n");
    }

    #[test]
    fn await_on_plain_value_is_identity() {
        // MVP: await 가 Future 아닌 값에 대해도 그대로 통과.
        let out = run_str(
            r#"
            let x: int = await 42
            @out x
            "#,
        )
        .unwrap();
        assert_eq!(out, "42\n");
    }

    #[test]
    fn await_inside_async_function_body() {
        // async 함수 내부에서 await 사용. 중첩 동작.
        let out = run_str(
            r#"
            async function inner(): int -> {
              await 10
            }
            async function outer(): int -> {
              let n = await inner()
              n + 1
            }
            @out await outer()
            "#,
        )
        .unwrap();
        assert_eq!(out, "11\n");
    }

    #[test]
    fn await_keeps_prefix_operator_precedence() {
        let out = run_str(
            r#"
            @out -await 1 + 2
            @out !await false || true
            "#,
        )
        .unwrap();
        assert_eq!(out, "1\ntrue\n");
    }

    // --- B4: @env domain ---

    #[test]
    fn env_reads_existing_var_as_string() {
        // test_env::set 은 process-wide static 맵에 기록. 다른 테스트와
        // 키 충돌을 피하기 위해 pid + 고정 suffix 로 namespace 분리.
        let key = format!("ORV_B4_EXIST_{}", std::process::id());
        super::test_env::set(&key, "hello");
        let src = format!(r#"@out @env.{key}"#);
        let out = run_str(&src).unwrap();
        assert_eq!(out, "hello\n");
        super::test_env::clear(&key);
    }

    #[test]
    fn env_missing_var_is_void() {
        // override 에도 없고 프로세스 env 에도 없으면 `@env.X` 는 Void.
        // @out 은 Void 면 빈 줄.
        let key = format!("ORV_B4_MISSING_{}", std::process::id());
        super::test_env::clear(&key);
        let src = format!(r#"@out @env.{key}"#);
        let out = run_str(&src).unwrap();
        assert_eq!(out, "\n");
    }

    #[test]
    fn env_nullish_default_operator() {
        // `@env.X ?? "default"` — 미존재 시 디폴트 문자열.
        let key = format!("ORV_B4_NULLISH_{}", std::process::id());
        super::test_env::clear(&key);
        let src = format!(
            r#"let v: string = @env.{key} ?? "8080"
@out v"#
        );
        let out = run_str(&src).unwrap();
        assert_eq!(out, "8080\n");
    }

    // ── C_middleware 도메인 (@before/@after/@next/@context) ──

    #[test]
    fn middleware_before_pushes_context_via_next() {
        // define Auth() -> @before { @next {payload: "alice"} }
        // route handler 가 @Auth 를 부른 뒤 @context.payload 로 값을 읽는다.
        let src = r#"{
            define Auth() -> @before {
                @next {payload: "alice"}
            }
            @Auth
            @out @context.payload
            @respond 200 {}
        }"#;
        let (stdout, resp) = run_handler(src, RequestCtx::default());
        assert_eq!(stdout, "alice\n");
        let resp = resp.expect("response recorded");
        assert_eq!(resp.status, 200);
    }

    #[test]
    fn middleware_before_can_short_circuit_via_respond() {
        // `@before` 안에서 `@respond` 를 호출하면 handler 본문은 실행되지 않아야 한다.
        let src = r#"{
            define GuardUnauth() -> @before {
                @respond 401 {error: "unauth"}
            }
            @GuardUnauth
            @out "SHOULD-NOT-RUN"
            @respond 200 {}
        }"#;
        let (stdout, resp) = run_handler(src, RequestCtx::default());
        assert_eq!(stdout, "", "handler body must not run after @respond in @before");
        let resp = resp.expect("response recorded");
        assert_eq!(resp.status, 401);
    }

    #[test]
    fn middleware_after_runs_post_handler() {
        // `@after` 는 handler 본문 뒤에 평가된다. 기록된 `@respond` 는 변경되지 않으나,
        // `@after` 본문의 부작용(@out)은 handler stdout 에 append 된다.
        let src = r#"{
            define Log() -> @after {
                @out "after-ran"
            }
            @Log
            @out "handler-ran"
            @respond 200 {}
        }"#;
        let (stdout, resp) = run_handler(src, RequestCtx::default());
        assert_eq!(stdout, "handler-ran\nafter-ran\n");
        let resp = resp.expect("response recorded");
        assert_eq!(resp.status, 200);
    }

    #[test]
    fn middleware_next_without_body_is_noop() {
        // 인자 없는 `@next` — 단순 pass-through. context 비어 있어야 한다.
        // `@context.foo` 접근은 `no field` 에러 — RuntimeError.
        let src = r#"{
            define Pass() -> @before {
                @next
            }
            @Pass
            @respond 200 {}
        }"#;
        let (stdout, resp) = run_handler(src, RequestCtx::default());
        assert_eq!(stdout, "");
        let resp = resp.expect("response recorded");
        assert_eq!(resp.status, 200);
    }

    #[test]
    fn middleware_multiple_before_accumulate_context() {
        // 두 개의 `@before` middleware 가 각각 다른 키를 context 에 push.
        let src = r#"{
            define M1() -> @before { @next {a: 1} }
            define M2() -> @before { @next {b: 2} }
            @M1
            @M2
            @out @context.a
            @out @context.b
            @respond 200 {}
        }"#;
        let (stdout, resp) = run_handler(src, RequestCtx::default());
        assert_eq!(stdout, "1\n2\n");
        assert_eq!(resp.unwrap().status, 200);
    }

    #[test]
    fn user_domain_property_by_name() {
        // SPEC §9.3: `@Name key=value` 로 property 매칭.
        let src = r#"
define Greet(name: string) -> {
  @out "Hello, {name}!"
}
@Greet name="Alice"
"#;
        let out = run_str(src).unwrap();
        assert_eq!(out, "Hello, Alice!\n");
    }

    #[test]
    fn user_domain_nullable_property_defaults_to_void() {
        // nullable param (`T?`) 에 property 가 누락되면 void. `??` 로 디폴트.
        let src = r#"
define Badge(label: string, color: string?) -> {
  let c: string = color ?? "gray"
  @out "[{c}] {label}"
}
@Badge label="admin"
@Badge label="vip" color="gold"
"#;
        let out = run_str(src).unwrap();
        assert_eq!(out, "[gray] admin\n[gold] vip\n");
    }

    #[test]
    fn user_domain_property_order_does_not_matter() {
        // property 순서 무관 — key 기반 매칭.
        let src = r#"
define G(a: string, b: string) -> {
  @out "{a} {b}"
}
@G a="first" b="second"
@G b="SECOND" a="FIRST"
"#;
        let out = run_str(src).unwrap();
        assert_eq!(out, "first second\nFIRST SECOND\n");
    }

    #[test]
    fn user_domain_missing_required_property_errors() {
        // non-nullable param 에 property 가 빠지면 런타임 에러.
        let src = r#"
define Req(x: string) -> { @out x }
@Req
"#;
        let err = run_str(src).unwrap_err();
        assert!(err.message.contains("missing required property"), "got: {}", err.message);
    }

    #[test]
    fn user_domain_unknown_property_errors() {
        // signature 에 없는 key 는 에러.
        let src = r#"
define P(a: string) -> { @out a }
@P a="ok" b="nope"
"#;
        let err = run_str(src).unwrap_err();
        assert!(err.message.contains("unknown property"), "got: {}", err.message);
    }

    // ── SPEC §9.4 Token slot (Stage 2) ──

    #[test]
    fn token_slot_inline_collects_positional() {
        let src = r#"
define Echo() -> {
  token msg: string
  @out msg[0]
  @out msg.length
}
@Echo "first" "second" "third"
"#;
        let out = run_str(src).unwrap();
        assert_eq!(out, "first\n3\n");
    }

    #[test]
    fn token_slot_block_form_with_property() {
        // property + token slot 혼합.
        let src = r#"
define Log(label: string?) -> {
  token { message: string }
  let lbl: string = label ?? "LOG"
  @out "[{lbl}] {message[0]}"
}
@Log "msg" label="INFO"
@Log "basic"
"#;
        let out = run_str(src).unwrap();
        assert_eq!(out, "[INFO] msg\n[LOG] basic\n");
    }

    #[test]
    fn no_token_slot_rejects_positional() {
        // slot 이 없으면 positional 은 에러.
        let src = r#"
define P() -> { @out "x" }
@P "stray"
"#;
        let err = run_str(src).unwrap_err();
        assert!(
            err.message.contains("got 1 positional arg(s) but declares no token slot"),
            "got: {}",
            err.message
        );
    }

    // ── SPEC §9.5 @content (Stage 3) ──

    #[test]
    fn content_injects_caller_block() {
        let src = r#"
define Section(title: string) -> {
  @out "=== {title} ==="
  @content
  @out "=== /{title} ==="
}
@Section title="Intro" {
  @out "body"
}
"#;
        let out = run_str(src).unwrap();
        assert_eq!(out, "=== Intro ===\nbody\n=== /Intro ===\n");
    }

    #[test]
    fn content_without_caller_block_is_noop() {
        let src = r#"
define W() -> {
  @out "before"
  @content
  @out "after"
}
@W
"#;
        let out = run_str(src).unwrap();
        assert_eq!(out, "before\nafter\n");
    }

    // ── SPEC §9.6 Nested dotted path (Stage 4) ──

    #[test]
    fn nested_dotted_domain_call() {
        let src = r#"
define Outer() -> {
  define Inner(label: string) -> {
    @out "- {label}"
  }
}
@Outer.Inner label="hi"
"#;
        let out = run_str(src).unwrap();
        assert_eq!(out, "- hi\n");
    }

    #[test]
    fn nested_dotted_three_levels() {
        let src = r#"
define A() -> {
  define B() -> {
    define C(x: int) -> { @out "C({x})" }
  }
}
@A.B.C x=42
"#;
        let out = run_str(src).unwrap();
        assert_eq!(out, "C(42)\n");
    }

    // ── SPEC §10.4 Boolean shorthand (Stage 5) ──

    // ── SPEC §6.4 for in collection ──

    #[test]
    fn for_in_array_iterates_elements() {
        let out = run_str(
            r#"for x in [10, 20, 30] {
              @out x
            }"#,
        )
        .unwrap();
        assert_eq!(out, "10\n20\n30\n");
    }

    #[test]
    fn for_in_string_iterates_chars() {
        let out = run_str(
            r#"for c in "xyz" {
              @out c
            }"#,
        )
        .unwrap();
        assert_eq!(out, "x\ny\nz\n");
    }

    #[test]
    fn for_in_range_still_works() {
        // Regression — range 경로가 깨지지 않아야 한다.
        let out = run_str(
            r#"for i in 0..3 {
              @out i
            }"#,
        )
        .unwrap();
        assert_eq!(out, "0\n1\n2\n");
    }

    #[test]
    fn for_in_token_slot_iterates_positional_args() {
        let out = run_str(
            r#"define Echo() -> {
              token msg: string
              for m in msg {
                @out m
              }
            }
            @Echo "a" "b" "c""#,
        )
        .unwrap();
        assert_eq!(out, "a\nb\nc\n");
    }

    // ── SPEC §4.9 T.from(v) numeric parsing ──

    #[test]
    fn int_from_string_parses() {
        let out = run_str(
            r#"let n: int = int.from("42")
@out n"#,
        )
        .unwrap();
        assert_eq!(out, "42\n");
    }

    #[test]
    fn int_from_float_truncates() {
        let out = run_str(
            r#"let n: int = int.from(3.9)
@out n"#,
        )
        .unwrap();
        assert_eq!(out, "3\n");
    }

    #[test]
    fn float_from_string_parses() {
        let out = run_str(
            r#"let f: float = float.from("1.5")
@out f"#,
        )
        .unwrap();
        assert_eq!(out, "1.5\n");
    }

    #[test]
    fn string_from_any_displays() {
        let out = run_str(
            r#"let s: string = string.from(42)
@out s"#,
        )
        .unwrap();
        assert_eq!(out, "42\n");
    }

    #[test]
    fn int_from_invalid_string_errors() {
        let err = run_str(
            r#"let n: int = int.from("nope")"#,
        )
        .unwrap_err();
        assert!(err.message.contains("int.from"));
    }

    // ── SPEC §6.4 tuple destructuring for in ──

    #[test]
    fn for_in_array_with_index_tuple() {
        let out = run_str(
            r#"for (x, i) in [10, 20, 30] {
              @out "{i}:{x}"
            }"#,
        )
        .unwrap();
        assert_eq!(out, "0:10\n1:20\n2:30\n");
    }

    #[test]
    fn ternary_returns_value() {
        let out = run_str(
            r#"let n: int = 10
let label: string = n > 5 ? "big" : "small"
@out label"#,
        )
        .unwrap();
        assert_eq!(out, "big\n");
    }

    #[test]
    fn ternary_with_block_branch() {
        let out = run_str(
            r#"let x: int = 3
let msg: string = x > 0 ? { "pos" } : "neg"
@out msg"#,
        )
        .unwrap();
        assert_eq!(out, "pos\n");
    }

    #[test]
    fn enum_variants_accessible_by_dot() {
        let out = run_str(
            r#"enum Status { Pending = 0, Running = 1 }
@out Status.Pending
@out Status.Running"#,
        )
        .unwrap();
        assert_eq!(out, "0\n1\n");
    }

    #[test]
    fn enum_string_valued() {
        let out = run_str(
            r#"enum SizeUnit { Px = "px", Em = "em" }
@out SizeUnit.Px"#,
        )
        .unwrap();
        assert_eq!(out, "px\n");
    }

    #[test]
    fn assert_true_passes() {
        let out = run_str(
            r#"assert 1 + 1 == 2
@out "ok""#,
        )
        .unwrap();
        assert_eq!(out, "ok\n");
    }

    #[test]
    fn assert_false_throws() {
        let err = run_str(r#"assert 1 == 2"#).unwrap_err();
        assert!(err.thrown.is_some());
    }

    #[test]
    fn test_block_executes_body() {
        let out = run_str(
            r#"test "t1" {
  @out "ran"
}"#,
        )
        .unwrap();
        assert_eq!(out, "ran\n");
    }

    #[test]
    fn field_assignment_mutates_struct_field() {
        let out = run_str(
            r#"struct Config { port: int }
let mut c: Config = { port: 8080 }
c.port = 3000
@out c.port"#,
        )
        .unwrap();
        assert_eq!(out, "3000\n");
    }

    #[test]
    fn object_spread_merges_fields() {
        let out = run_str(
            r#"let base = { name: "Alice", age: 30 }
let updated = { ...base, age: 31 }
@out updated"#,
        )
        .unwrap();
        assert_eq!(out, "{ name: Alice, age: 31 }\n");
    }

    #[test]
    fn object_spread_with_new_field() {
        let out = run_str(
            r#"let base = { a: 1 }
let m = { ...base, b: 2 }
@out m"#,
        )
        .unwrap();
        assert_eq!(out, "{ a: 1, b: 2 }\n");
    }

    #[test]
    fn spawn_block_executes_immediately() {
        let out = run_str(
            r#"spawn {
  @out "inside spawn"
}
@out "after""#,
        )
        .unwrap();
        assert_eq!(out, "inside spawn\nafter\n");
    }

    #[test]
    fn process_run_captures_output() {
        let out = run_str(
            r#"let r = await @process.run("echo hi")
@out r.stdout
@out "status: {r.status}""#,
        )
        .unwrap();
        assert_eq!(out, "hi\n\nstatus: 0\n");
    }

    #[test]
    fn fs_read_write_roundtrip() {
        let path = format!("/tmp/orv_fs_test_{}.txt", std::process::id());
        let src = format!(
            r#"await @fs.write("{path}", "hello")
let file = await @fs.read("{path}")
@out file.content"#
        );
        let out = run_str(&src).unwrap();
        assert_eq!(out, "hello\n");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn for_in_range_with_index_tuple() {
        let out = run_str(
            r#"for (n, i) in 5..8 {
              @out "{i}->{n}"
            }"#,
        )
        .unwrap();
        assert_eq!(out, "0->5\n1->6\n2->7\n");
    }

    #[test]
    fn for_in_rejects_non_iterable() {
        let err = run_str(
            r#"for x in 42 {
              @out x
            }"#,
        )
        .unwrap_err();
        assert!(
            err.message.contains("for loop iterable must be"),
            "got: {}",
            err.message
        );
    }

    #[test]
    fn boolean_shorthand_assigns_true() {
        let src = r#"
define Btn(label: string, disabled: bool?) -> {
  let d: bool = disabled ?? false
  if d { @out "OFF:{label}" } else { @out "ON:{label}" }
}
@Btn label="A"
@Btn label="B" disabled
@Btn label="C" disabled=false
"#;
        let out = run_str(src).unwrap();
        assert_eq!(out, "ON:A\nOFF:B\nON:C\n");
    }

    #[test]
    fn middleware_next_overwrites_same_key() {
        // 같은 키를 두 번 push 하면 뒤의 값이 우세.
        let src = r#"{
            define First() -> @before { @next {user: "alice"} }
            define Second() -> @before { @next {user: "bob"} }
            @First
            @Second
            @out @context.user
            @respond 200 {}
        }"#;
        let (stdout, _) = run_handler(src, RequestCtx::default());
        assert_eq!(stdout, "bob\n");
    }

    // ── 내장 함수 회귀 테스트 ──

    #[test]
    fn builtin_type_returns_name() {
        let out = run_str(
            r#"@out Type(1)
@out Type("hi")
@out Type(void)
@out Type(true)"#,
        )
        .unwrap();
        assert_eq!(out, "int\nstring\nvoid\nbool\n");
    }

    #[test]
    fn builtin_max_min_abs_preserve_int() {
        let out = run_str(
            r#"@out max(1, 2, 3)
@out min(3, 2, 1)
@out abs(-5)"#,
        )
        .unwrap();
        assert_eq!(out, "3\n1\n5\n");
    }

    #[test]
    fn builtin_math_functions_are_floats() {
        // sqrt/floor/ceil/round 결과는 항상 f64 로 떨어진다.
        let out = run_str(
            r#"@out sqrt(4)
@out floor(1.9)
@out ceil(1.1)
@out round(1.5)"#,
        )
        .unwrap();
        assert_eq!(out, "2\n1\n2\n2\n");
    }

    #[test]
    fn builtin_now_returns_time_object_with_fields() {
        let out = run_str(
            r#"let t = now()
@out t.year > 2000
@out t.month >= 1 && t.month <= 12
@out t.day >= 1 && t.day <= 31"#,
        )
        .unwrap();
        assert_eq!(out, "true\ntrue\ntrue\n");
    }

    #[test]
    fn builtin_sleep_is_noop() {
        let out = run_str(r#"await sleep(10)
@out "ok""#)
            .unwrap();
        assert_eq!(out, "ok\n");
    }

    // ── SPEC §3.3 소유권/복사 회귀 ──

    #[test]
    fn string_move_and_copy_return_equivalent_value() {
        let out = run_str(
            r#"let a = "hi"
let b = a.move()
@out b
let c = b.copy()
@out c"#,
        )
        .unwrap();
        assert_eq!(out, "hi\nhi\n");
    }

    // ── 비트 연산 회귀 ──

    #[test]
    fn bitwise_operators_on_ints() {
        let out = run_str(
            r#"@out 5 & 3
@out 5 | 3
@out 5 ^ 3
@out 1 << 3
@out 16 >> 2"#,
        )
        .unwrap();
        assert_eq!(out, "1\n7\n6\n8\n4\n");
    }

    // ── try/catch 가 native error 를 잡는지 확인 ──

    #[test]
    fn try_catch_captures_native_error_as_message() {
        let out = run_str(
            r#"try {
  let x: int = int.from("not a number")
} catch err {
  @out "caught: {err}"
}"#,
        )
        .unwrap();
        assert!(out.starts_with("caught: int.from failed"), "got: {out}");
    }

    // ── 줄바꿈 경계 회귀: @fs.write 가 다음 줄 stmt 를 흡수하지 않음 ──

    #[test]
    fn io_domain_stops_args_at_newline() {
        let path = format!(
            "/tmp/orv_io_newline_{}.txt",
            std::process::id()
        );
        let src = format!(
            r#"await @fs.write "{path}" "hello"
let v = 1
@out v"#
        );
        let out = run_str(&src).unwrap();
        assert_eq!(out, "1\n");
        let _ = std::fs::remove_file(&path);
    }
}
