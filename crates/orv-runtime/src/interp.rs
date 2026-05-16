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

use crate::db::{
    new_db_handle, DbFilter, DbFilterOp, DbHandle, DbNear, DbOrder, DbQuery, InMemoryDb,
};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use hmac::{Hmac, Mac};
use orv_hir::{
    BinaryOp, HirBlock, HirConstraintValue, HirExpr, HirExprKind, HirFunctionBody, HirFunctionStmt,
    HirParam, HirPattern, HirProgram, HirStmt, HirStringSegment, HirTypeConstraint, HirTypeRef,
    HirTypeRefKind, NameId, UnaryOp,
};
use rand_core::OsRng;
use sha2::Sha256;
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

type HmacSha256 = Hmac<Sha256>;

pub(crate) const ORV_CSRF_COOKIE_NAME: &str = "orv_csrf";
pub(crate) const ORV_REFERENCE_CSRF_TOKEN: &str = "orv-reference-csrf";
pub(crate) const ORV_SESSION_COOKIE_NAME: &str = "orv_session";
pub(crate) const ORV_SESSION_ROLE_COOKIE_NAME: &str = "orv_session_role";

fn resolve_runtime_path(path: &str, working_dir: Option<&Path>) -> PathBuf {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else if let Some(working_dir) = working_dir {
        working_dir.join(path)
    } else {
        path.to_path_buf()
    }
}

/// B4 `@env` 테스트 override.
///
/// `std::env::set_var` 는 Rust 2024 에서 `unsafe` 가 되었고 워크스페이스는
/// `unsafe_code = "forbid"` 라 단위 테스트가 직접 env 를 조작할 수 없다.
/// `#[cfg(test)]` 전용 맵을 두어 테스트에서 override 를 주입하고, Domain
/// arm 에서 `@env` 평가 시 이 맵을 병합한다. production 빌드에는 이 모듈이
/// 남지 않는다.
#[cfg(test)]
pub(crate) mod test_env {
    use std::collections::HashMap;
    use std::sync::{Mutex, MutexGuard, OnceLock};

    pub(super) static ENV_OVERRIDES: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    static ENV_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    pub(crate) fn guard() -> MutexGuard<'static, ()> {
        ENV_TEST_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
    }

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
    /// 클라이언트 IP.
    pub ip: String,
    /// 경로 매개변수 (`:id` → `"42"`).
    pub params: HashMap<String, String>,
    /// 쿼리 매개변수.
    pub query: HashMap<String, String>,
    /// `@query: Type` 검증 후 노출할 정규화된 query 값.
    pub query_value: Option<Value>,
    /// 요청 헤더.
    pub headers: HashMap<String, String>,
    /// UTF-8/lossy 원문 요청 body. Webhook signature checks need the exact body
    /// string that reached the server before JSON/form parsing.
    pub raw_body: String,
    /// 파싱된 body. JSON/form bodies are exposed as objects; unknown content
    /// types remain raw strings, and empty bodies are void.
    pub body: Value,
    /// `@form: Type` 검증 후 노출할 정규화된 form 값. URL-encoded form input은
    /// 최초에는 `body` 와 같은 object 로 들어오며, binding 성공 후 `@form`
    /// 이 이 값을 우선 노출한다.
    pub form: Option<Value>,
}

impl Default for RequestCtx {
    fn default() -> Self {
        Self {
            method: String::new(),
            path: String::new(),
            ip: String::new(),
            params: HashMap::new(),
            query: HashMap::new(),
            query_value: None,
            headers: HashMap::new(),
            raw_body: String::new(),
            body: Value::Void,
            form: None,
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

/// Runtime schema/type registry captured across interpreter boundaries.
///
/// HTTP server boot evaluates server-level statements once, then each request
/// runs in a fresh interpreter. The lexical env alone is not enough for
/// `@body: SignupForm` because struct fields and type aliases live in these
/// validator maps, so server code carries this registry alongside env values.
#[derive(Clone, Default)]
pub(crate) struct RuntimeTypeRegistry {
    pub type_structs: HashMap<String, Vec<(String, HirTypeRef)>>,
    pub type_aliases: HashMap<String, HirTypeRef>,
}

/// Result of a reference-runtime debug run.
#[derive(Clone, Debug, Default)]
pub struct DebugRun {
    /// Ordered snapshots captured after runtime-executed statements.
    pub frames: Vec<DebugFrame>,
}

/// One debugger-visible frame snapshot.
#[derive(Clone, Debug)]
pub struct DebugFrame {
    /// Source span of the statement that produced this snapshot.
    pub span: orv_diagnostics::Span,
    /// Lexical bindings visible in the runtime environment at this point.
    pub locals: Vec<DebugVariable>,
    /// Function/domain call stack active while this statement executed.
    pub stack: Vec<DebugStackFrame>,
    /// Stdout text emitted while this statement executed.
    pub output: String,
}

/// Runtime value for one debugger-visible binding.
#[derive(Clone, Debug)]
pub struct DebugVariable {
    /// Source-level binding name.
    pub name: String,
    /// Runtime value captured for the binding.
    pub value: Value,
}

/// One runtime call-stack entry captured for debugger frames.
#[derive(Clone, Debug)]
pub struct DebugStackFrame {
    /// Display name for the callable.
    pub name: String,
    /// Source span for the callable declaration.
    pub span: orv_diagnostics::Span,
}

/// Incremental debugger runner for a single HIR program.
///
/// Unlike [`run_with_debug`], this runner keeps interpreter state alive and
/// executes only until the next debugger-visible frame is available.
pub struct DebugStepper<W: Write> {
    program: HirProgram,
    interp: Interp<W>,
    next_item: usize,
    pending_frames: VecDeque<DebugFrame>,
    pending_error: Option<RuntimeError>,
    completed: bool,
}

impl<W: Write> DebugStepper<W> {
    /// Create a stepper from a lowered program and writer.
    #[must_use]
    pub fn new(program: HirProgram, writer: W) -> Self {
        let mut interp = Interp::new_with_env(writer, HashMap::new());
        interp.debug = Some(DebugTraceState::default());
        Self {
            program,
            interp,
            next_item: 0,
            pending_frames: VecDeque::new(),
            pending_error: None,
            completed: false,
        }
    }

    /// Execute until the next debugger frame is available.
    ///
    /// `Ok(None)` means the program has completed. If a runtime error happens
    /// after one or more frames were captured, those frames are returned first
    /// and the stored error is returned by a later call.
    ///
    /// # Errors
    /// Returns the runtime error raised by the interpreted program once all
    /// debugger frames captured before that error have been drained.
    pub fn step(&mut self) -> Result<Option<DebugFrame>, RuntimeError> {
        if let Some(frame) = self.pending_frames.pop_front() {
            return Ok(Some(frame));
        }
        if let Some(error) = self.pending_error.take() {
            return Err(error);
        }
        if self.completed {
            return Ok(None);
        }
        while self.next_item < self.program.items.len() {
            let item_index = self.next_item;
            self.next_item += 1;
            let is_last = item_index + 1 == self.program.items.len();
            let result = self
                .interp
                .exec_stmt(&self.program.items[item_index], is_last);
            self.drain_debug_frames();
            match result {
                Ok(()) => {
                    if let Some(frame) = self.pending_frames.pop_front() {
                        return Ok(Some(frame));
                    }
                }
                Err(error) => {
                    self.completed = true;
                    if let Some(frame) = self.pending_frames.pop_front() {
                        self.pending_error = Some(error);
                        return Ok(Some(frame));
                    }
                    return Err(error);
                }
            }
        }
        self.completed = true;
        Ok(None)
    }

    /// Return the writer currently owned by this stepper.
    #[must_use]
    pub const fn writer(&self) -> &W {
        &self.interp.writer
    }

    fn drain_debug_frames(&mut self) {
        if let Some(debug) = &mut self.interp.debug {
            self.pending_frames.extend(debug.frames.drain(..));
        }
    }
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
    /// 정규식 리터럴 `r"pattern"flags`.
    Regex {
        /// 정규식 본문.
        pattern: String,
        /// 플래그 문자열 (`g`, `i`, `m`).
        flags: String,
    },
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
    /// 튜플 — 고정 길이, heterogeneous.
    Tuple(Vec<Value>),
    /// 오브젝트 — 필드 이름 순서 유지. 필드명은 구조체 멤버이므로 문자열.
    Object(Vec<(String, Value)>),
    /// C_db: in-memory DB handle. `@db` 평가 결과이며 `.create` 같은 field
    /// 접근으로 bound method 를 얻어 호출한다.
    Db(DbHandle),
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

#[derive(Clone, Debug)]
struct JobHandler {
    params: Vec<String>,
    body: HirBlock,
    retries: usize,
}

#[derive(Clone, Debug)]
struct CronHandler {
    schedule: String,
    body: HirBlock,
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Int(v) => write!(f, "{v}"),
            Self::Float(v) => write!(f, "{v}"),
            Self::Str(v) => write!(f, "{v}"),
            Self::Regex { pattern, flags } => write!(f, "r\"{pattern}\"{flags}"),
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
            Self::Tuple(elems) => {
                write!(f, "(")?;
                for (i, v) in elems.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{v}")?;
                }
                write!(f, ")")
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

/// Runtime execution options shared by direct CLI runs and future launchers.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RuntimeOptions {
    /// Optional path for `@server` request trace capture.
    pub request_trace_path: Option<PathBuf>,
    /// Optional runtime working directory for relative file-backed adapters.
    pub working_dir: Option<PathBuf>,
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

/// Run a program with explicit runtime options.
///
/// # Errors
/// `run_with_writer` 과 동일.
pub fn run_with_writer_with_options<W: Write>(
    program: &HirProgram,
    writer: &mut W,
    options: RuntimeOptions,
) -> Result<(), RuntimeError> {
    run_with_writer_in_env_with_options(program, HashMap::new(), writer, options).map(|_| ())
}

/// Run a program with debugger snapshots enabled.
///
/// The returned [`DebugRun`] contains snapshots captured after executable HIR
/// statements. Runtime success or failure is returned separately so callers can
/// still inspect partial output and frames when execution fails.
pub fn run_with_debug<W: Write>(
    program: &HirProgram,
    writer: &mut W,
) -> (DebugRun, Result<(), RuntimeError>) {
    let mut interp = Interp::new_with_env(writer, HashMap::new());
    interp.debug = Some(DebugTraceState::default());
    let result = interp.run(program);
    let debug = interp
        .debug
        .take()
        .map_or_else(DebugRun::default, |state| DebugRun {
            frames: state.frames,
        });
    (debug, result)
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
    run_with_writer_in_env_with_options(program, env, writer, RuntimeOptions::default())
}

pub(crate) fn run_with_writer_in_env_with_db<W: Write>(
    program: &HirProgram,
    env: HashMap<NameId, Value>,
    db: DbHandle,
    writer: &mut W,
) -> Result<HashMap<NameId, Value>, RuntimeError> {
    run_with_writer_in_env_and_types_with_db(
        program,
        env,
        RuntimeTypeRegistry::default(),
        db,
        writer,
    )
    .map(|(env, _)| env)
}

pub(crate) fn run_with_writer_in_env_and_types_with_db<W: Write>(
    program: &HirProgram,
    env: HashMap<NameId, Value>,
    types: RuntimeTypeRegistry,
    db: DbHandle,
    writer: &mut W,
) -> Result<(HashMap<NameId, Value>, RuntimeTypeRegistry), RuntimeError> {
    run_with_writer_in_env_and_types_with_db_and_options(
        program,
        env,
        types,
        db,
        writer,
        RuntimeOptions::default(),
    )
}

pub(crate) fn run_with_writer_in_env_and_types_with_db_and_options<W: Write>(
    program: &HirProgram,
    env: HashMap<NameId, Value>,
    types: RuntimeTypeRegistry,
    db: DbHandle,
    writer: &mut W,
    options: RuntimeOptions,
) -> Result<(HashMap<NameId, Value>, RuntimeTypeRegistry), RuntimeError> {
    let mut interp = Interp::new_with_env_and_options(writer, env, options);
    interp.db = db;
    interp.apply_type_registry(types);
    interp.run(program)?;
    let types = interp.type_registry();
    Ok((interp.env, types))
}

pub(crate) fn run_with_writer_in_env_with_options<W: Write>(
    program: &HirProgram,
    env: HashMap<NameId, Value>,
    writer: &mut W,
    options: RuntimeOptions,
) -> Result<HashMap<NameId, Value>, RuntimeError> {
    let mut interp = Interp::new_with_env_and_options(writer, env, options);
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
    let db = new_db_handle();
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
    db: DbHandle,
    writer: &mut W,
) -> Result<HandlerOutcome, RuntimeError> {
    run_handler_with_request_in_env_and_types(
        handler,
        request,
        env,
        RuntimeTypeRegistry::default(),
        db,
        writer,
    )
}

pub(crate) fn run_handler_with_request_in_env_and_types<W: Write>(
    handler: &HirExpr,
    request: RequestCtx,
    env: HashMap<NameId, Value>,
    types: RuntimeTypeRegistry,
    db: DbHandle,
    writer: &mut W,
) -> Result<HandlerOutcome, RuntimeError> {
    run_handler_with_request_in_env_and_types_with_options(
        handler,
        request,
        env,
        types,
        db,
        writer,
        RuntimeOptions::default(),
    )
}

pub(crate) fn run_handler_with_request_in_env_and_types_with_options<W: Write>(
    handler: &HirExpr,
    request: RequestCtx,
    env: HashMap<NameId, Value>,
    types: RuntimeTypeRegistry,
    db: DbHandle,
    writer: &mut W,
    options: RuntimeOptions,
) -> Result<HandlerOutcome, RuntimeError> {
    let mut interp = Interp::new_with_env_and_options(writer, env, options);
    interp.db = db;
    interp.apply_type_registry(types);
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

struct Interp<W: Write> {
    env: HashMap<NameId, Value>,
    writer: W,
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
    db: DbHandle,
    /// SPEC §4 runtime validator: struct 이름 -> 필드 타입.
    type_structs: HashMap<String, Vec<(String, HirTypeRef)>>,
    /// SPEC §4 runtime validator: type alias 이름 -> 실제 타입.
    type_aliases: HashMap<String, HirTypeRef>,
    /// SPEC §11.15/§11.20 참조 런타임: chunked upload 와 storage API 가
    /// fixture/e2e 값 흐름을 검증할 수 있게 인터프리터 생애 동안만 보관한다.
    storage_chunks: HashMap<String, Vec<(i64, Value)>>,
    storage_files: HashMap<String, Value>,
    /// SPEC §10.13 reference runtime cache/store state.
    cache_entries: HashMap<String, HashMap<String, Value>>,
    offline_entries: HashMap<String, HashMap<String, Value>>,
    /// SPEC §10.7 design token sections.
    design_tokens: HashMap<String, Value>,
    /// SPEC §11.18 job declarations registered in this interpreter.
    job_handlers: HashMap<String, JobHandler>,
    /// SPEC §11.18 cron declarations registered in this interpreter.
    cron_handlers: Vec<CronHandler>,
    /// Current `@unsafe` lexical/runtime boundary depth.
    unsafe_depth: usize,
    /// Runtime-only bindings for domain handlers whose parameter names are not
    /// resolver-managed lexical declarations yet.
    dynamic_scopes: Vec<HashMap<String, Value>>,
    /// Optional debugger trace collector.
    debug: Option<DebugTraceState>,
    runtime_options: RuntimeOptions,
}

#[derive(Default)]
struct DebugTraceState {
    names: Vec<(NameId, String)>,
    frames: Vec<DebugFrame>,
    stack: Vec<DebugStackFrame>,
    output: String,
}

impl<W: Write> Interp<W> {
    fn new_with_env(writer: W, env: HashMap<NameId, Value>) -> Self {
        Self::new_with_env_and_options(writer, env, RuntimeOptions::default())
    }

    fn new_with_env_and_options(
        writer: W,
        env: HashMap<NameId, Value>,
        options: RuntimeOptions,
    ) -> Self {
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
            db: new_db_handle(),
            type_structs: HashMap::new(),
            type_aliases: HashMap::new(),
            storage_chunks: HashMap::new(),
            storage_files: HashMap::new(),
            cache_entries: HashMap::new(),
            offline_entries: HashMap::new(),
            design_tokens: HashMap::new(),
            job_handlers: HashMap::new(),
            cron_handlers: Vec::new(),
            unsafe_depth: 0,
            dynamic_scopes: Vec::new(),
            debug: None,
            runtime_options: options,
        }
    }

    fn runtime_path(&self, path: &str) -> PathBuf {
        resolve_runtime_path(path, self.runtime_options.working_dir.as_deref())
    }

    fn type_registry(&self) -> RuntimeTypeRegistry {
        RuntimeTypeRegistry {
            type_structs: self.type_structs.clone(),
            type_aliases: self.type_aliases.clone(),
        }
    }

    fn apply_type_registry(&mut self, types: RuntimeTypeRegistry) {
        self.type_structs = types.type_structs;
        self.type_aliases = types.type_aliases;
    }

    fn run(&mut self, program: &HirProgram) -> Result<(), RuntimeError> {
        let last_idx = program.items.len().saturating_sub(1);
        for (idx, stmt) in program.items.iter().enumerate() {
            let is_last = idx == last_idx;
            self.exec_stmt(stmt, is_last)?;
        }
        Ok(())
    }

    fn debug_register_ident(&mut self, ident: &orv_hir::HirIdent) {
        let Some(debug) = &mut self.debug else {
            return;
        };
        if !debug.names.iter().any(|(id, _)| *id == ident.id) {
            debug.names.push((ident.id, ident.name.clone()));
        }
    }

    fn debug_register_params(&mut self, params: &[HirParam]) {
        for param in params {
            self.debug_register_ident(&param.name);
        }
    }

    fn debug_capture(&mut self, span: orv_diagnostics::Span) {
        let Some(debug) = &self.debug else {
            return;
        };
        let names = debug.names.clone();
        let stack = debug.stack.clone();
        let output = debug.output.clone();
        let locals = names
            .into_iter()
            .filter_map(|(id, name)| {
                self.env
                    .get(&id)
                    .cloned()
                    .map(|value| DebugVariable { name, value })
            })
            .collect();
        if let Some(debug) = &mut self.debug {
            debug.output.clear();
            debug.frames.push(DebugFrame {
                span,
                locals,
                stack,
                output,
            });
        }
    }

    fn debug_push_call(&mut self, name: &str, span: orv_diagnostics::Span) {
        if let Some(debug) = &mut self.debug {
            debug.stack.push(DebugStackFrame {
                name: name.to_string(),
                span,
            });
        }
    }

    fn debug_pop_call(&mut self) {
        if let Some(debug) = &mut self.debug {
            debug.stack.pop();
        }
    }

    fn debug_record_output(&mut self, text: &str) {
        if let Some(debug) = &mut self.debug {
            debug.output.push_str(text);
        }
    }

    fn exec_stmt(&mut self, stmt: &HirStmt, is_last: bool) -> Result<(), RuntimeError> {
        match stmt {
            HirStmt::Let(l) => {
                let v = self.eval(&l.init)?;
                self.env.insert(l.name.id, v);
                self.debug_register_ident(&l.name);
                self.debug_capture(l.span);
            }
            HirStmt::Const(c) => {
                let v = self.eval(&c.init)?;
                self.env.insert(c.name.id, v);
                self.debug_register_ident(&c.name);
                self.debug_capture(c.span);
            }
            HirStmt::Function(f) => {
                let rc = Rc::new((**f).clone());
                self.env.insert(f.name.id, Value::Function(rc.clone()));
                self.debug_register_ident(&f.name);
                // SPEC §9.6: nested define 은 외부에서 `@Parent.Child` dotted
                // 경로로 접근 가능해야 한다. parent body 를 재귀 탐색해 nested
                // function 들을 dotted name 을 가진 별도 Rc<HirFunctionStmt>
                // 로 env 에 추가 등록한다. 이름을 dotted 로 바꾼 clone 을
                // 만들어 domain-call 선형 탐색(`f.name.name == name`)이 그대로
                // 매칭되게 한다.
                if f.is_define {
                    register_nested_defines(&mut self.env, &f.name.name, f);
                }
                self.debug_capture(f.span);
            }
            HirStmt::Struct(s) => {
                self.type_structs.insert(
                    s.name.name.clone(),
                    s.fields
                        .iter()
                        .map(|field| (field.name.clone(), field.annotation.clone()))
                        .collect(),
                );
                self.env
                    .insert(s.name.id, Value::TypeName(s.name.name.clone()));
                self.debug_register_ident(&s.name);
                self.debug_capture(s.span);
            }
            HirStmt::TypeAlias(alias) => {
                if alias.params.is_empty() {
                    self.type_aliases
                        .insert(alias.name.name.clone(), alias.ty.clone());
                }
                self.env
                    .insert(alias.name.id, Value::TypeName(alias.name.name.clone()));
                self.debug_register_ident(&alias.name);
                self.debug_capture(alias.span);
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
                self.debug_register_ident(&e.name);
                self.debug_capture(e.span);
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
                self.debug_capture(e.span);
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
            HirExprKind::Regex { pattern, flags } => Ok(Value::Regex {
                pattern: pattern.clone(),
                flags: flags.clone(),
            }),
            HirExprKind::True => Ok(Value::Bool(true)),
            HirExprKind::False => Ok(Value::Bool(false)),
            HirExprKind::Void => Ok(Value::Void),
            HirExprKind::TypeName(name) => Ok(Value::TypeName(name.clone())),
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
                crate::server::run_server_with_options(
                    listen.as_deref(),
                    routes,
                    body_stmts,
                    self.env.clone(),
                    self.type_registry(),
                    self.db.clone(),
                    self.runtime_options.clone(),
                )
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
                if name == "session" && self.request.is_some() {
                    return self.eval_session_domain(args);
                }
                if name == "csrf" && self.request.is_some() {
                    return self.eval_csrf_domain(args);
                }
                if name == "Auth" && self.request.is_some() && is_declarative_auth_invocation(args)
                {
                    return self.eval_auth_domain(args);
                }
                // 요청 컨텍스트가 있다면 request-state 도메인을 해석한다.
                if self.request.is_some() {
                    if let Some(v) = self.eval_request_binding_domain(name, args)? {
                        return Ok(v);
                    }
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
                    let func = self.env.values().find_map(|v| match v {
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
                if name == "job" {
                    return self.eval_job_domain(args);
                }
                if name == "cron" {
                    return self.eval_cron_domain(args);
                }
                if name == "unsafe" {
                    return self.eval_unsafe_domain(args);
                }
                if name == "observability" {
                    return self.eval_observability_domain(args);
                }
                if name == "offline" {
                    return self.eval_offline_domain(args);
                }
                if name == "ffi" {
                    return self.eval_ffi_domain(args);
                }
                if name == "cache" {
                    return self.eval_cache_domain(args);
                }
                if name == "design" {
                    return self.eval_design_domain(args);
                }
                if let Some(value) = eval_reference_domain(name, args) {
                    return Ok(value);
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
                        ("method".to_string(), Value::Str(method)),
                        ("body".to_string(), Value::Str(String::new())),
                        ("url".to_string(), Value::Str(url)),
                    ]));
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
            HirExprKind::AssignIndex {
                object,
                index,
                value,
            } => {
                let obj_value = self.eval(object)?;
                let key = self.eval(index)?;
                let new_value = self.eval(value)?;
                match (obj_value, key) {
                    (Value::Object(mut fields), Value::Str(key)) => {
                        if let Some(slot) = fields.iter_mut().find(|(k, _)| k == &key) {
                            slot.1 = new_value.clone();
                        } else {
                            fields.push((key, new_value.clone()));
                        }
                        if let HirExprKind::Ident(id) = &object.kind {
                            self.env.insert(id.id, Value::Object(fields));
                        }
                        Ok(new_value)
                    }
                    (Value::Array(mut items), Value::Int(idx)) => {
                        let idx = usize::try_from(idx).map_err(|_| {
                            RuntimeError::native("array index must be non-negative")
                        })?;
                        let Some(slot) = items.get_mut(idx) else {
                            return Err(RuntimeError::native(format!(
                                "array index {idx} out of bounds"
                            )));
                        };
                        *slot = new_value.clone();
                        if let HirExprKind::Ident(id) = &object.kind {
                            self.env.insert(id.id, Value::Array(items));
                        }
                        Ok(new_value)
                    }
                    (Value::Object(_), other) => Err(RuntimeError::native(format!(
                        "object index assignment key must be string, got {other}"
                    ))),
                    (Value::Array(_), other) => Err(RuntimeError::native(format!(
                        "array index assignment key must be int, got {other}"
                    ))),
                    (other, _) => Err(RuntimeError::native(format!(
                        "cannot assign index on non-indexable value: {other}"
                    ))),
                }
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
                    self.debug_register_ident(var);
                    if let Some(iv) = index_var {
                        self.debug_register_ident(iv);
                    }
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
                    Value::Str(s) => s.chars().map(|c| Value::Str(c.to_string())).collect(),
                    other => {
                        return Err(RuntimeError::native(format!(
                            "for loop iterable must be a range, array, or string, got {other}"
                        )));
                    }
                };
                self.debug_register_ident(var);
                if let Some(iv) = index_var {
                    self.debug_register_ident(iv);
                }
                for (i, item) in items.into_iter().enumerate() {
                    self.env.insert(var.id, item);
                    if let Some(iv) = index_var {
                        self.env
                            .insert(iv.id, Value::Int(i64::try_from(i).unwrap_or(0)));
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
            HirExprKind::Tuple(elems) => {
                let mut values = Vec::with_capacity(elems.len());
                for e in elems {
                    values.push(self.eval(e)?);
                }
                Ok(Value::Tuple(values))
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
            HirExprKind::TypedObject { ty, fields } => {
                // Set{...} → Array로, Map{...} → Object로 처리
                match ty.as_str() {
                    "Set" => {
                        let mut values = Vec::with_capacity(fields.len());
                        for f in fields {
                            let v = self.eval(&f.value)?;
                            if f.is_spread {
                                let Value::Array(source) = v else {
                                    return Err(RuntimeError::native(
                                        "Set spread `...expr` requires an array value",
                                    ));
                                };
                                values.extend(source);
                            } else {
                                values.push(v);
                            }
                        }
                        Ok(Value::Array(values))
                    }
                    "Map" => {
                        let mut out: Vec<(String, Value)> = Vec::with_capacity(fields.len());
                        for f in fields {
                            let v = self.eval(&f.value)?;
                            if f.is_spread {
                                let Value::Object(source) = v else {
                                    return Err(RuntimeError::native(
                                        "Map spread `...expr` requires an object value",
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
                    _ => {
                        // 기본: Object로 처리
                        let mut out: Vec<(String, Value)> = Vec::with_capacity(fields.len());
                        for f in fields {
                            let v = self.eval(&f.value)?;
                            if f.is_spread {
                                let Value::Object(source) = v else {
                                    return Err(RuntimeError::native(
                                        "typed object spread `...expr` requires an object value",
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
                }
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
                match (t, i) {
                    (Value::Object(fields), Value::Str(key)) => Ok(fields
                        .into_iter()
                        .find(|(field, _)| field == &key)
                        .map(|(_, value)| value)
                        .unwrap_or(Value::Void)),
                    (Value::Array(items), Value::Int(idx)) => {
                        let n = i64::try_from(items.len()).unwrap_or(i64::MAX);
                        let actual = if idx < 0 { idx + n } else { idx };
                        if actual < 0 || actual >= n {
                            return Err(RuntimeError::native(format!(
                                "index {idx} out of bounds for length {n}"
                            )));
                        }
                        Ok(items[actual as usize].clone())
                    }
                    (Value::Str(s), Value::Int(idx)) => {
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
                    (Value::Object(_), other) => Err(RuntimeError::native(format!(
                        "object index must be a string, got {other}"
                    ))),
                    (other, Value::Int(_)) => {
                        Err(RuntimeError::native(format!("cannot index into {other}")))
                    }
                    (_, other) => Err(RuntimeError::native(format!(
                        "index must be an integer or object string key, got {other}"
                    ))),
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
                field_value(t, field, false)
            }
            HirExprKind::OptionalField { target, field, .. } => {
                let t = self.eval(target)?;
                if matches!(t, Value::Void) {
                    return Ok(Value::Void);
                }
                field_value(t, field, true)
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
                if matches!(callee.kind, HirExprKind::OptionalField { .. })
                    && matches!(callee_value, Value::Void)
                {
                    return Ok(Value::Void);
                }
                if let Value::BoundMethod { receiver, method } = &callee_value {
                    if method == "transaction" {
                        if let Value::Db(db) = receiver.as_ref() {
                            return self.eval_db_transaction(db.clone(), args);
                        }
                    }
                    if method == "render" {
                        if let Value::TypeName(ns) = receiver.as_ref() {
                            if ns == "gpu" {
                                return Ok(gpu_render_raw(args));
                            }
                        }
                    }
                }
                let mut evaluated = Vec::with_capacity(args.len());
                for a in args {
                    evaluated.push(self.eval_call_arg(a)?);
                }
                self.call_value(callee_value, evaluated)
            }
        }
    }

    fn eval_call_arg(&mut self, arg: &HirExpr) -> Result<Value, RuntimeError> {
        if let HirExprKind::Assign { target, value } = &arg.kind {
            let value = self.eval(value)?;
            return Ok(Value::Object(vec![(target.name.clone(), value)]));
        }
        self.eval(arg)
    }

    fn eval_db_transaction(
        &mut self,
        db: DbHandle,
        args: &[HirExpr],
    ) -> Result<Value, RuntimeError> {
        let snapshot = db.borrow().clone();
        let mut last = Value::Void;
        for arg in args {
            match self.eval_call_arg(arg) {
                Ok(value) => last = value,
                Err(err) => {
                    let rollback = {
                        let mut guard = db.borrow_mut();
                        *guard = snapshot;
                        guard.checkpoint_wal_if_enabled()
                    };
                    rollback.map_err(|rollback_err| {
                        RuntimeError::native(format!(
                            "db.transaction rollback failed: {rollback_err}; original error: {err}"
                        ))
                    })?;
                    return Err(err);
                }
            }
        }
        Ok(last)
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
        for scope in self.dynamic_scopes.iter().rev() {
            if let Some(v) = scope.get(debug_name) {
                return Ok(v.clone());
            }
        }
        if matches!(debug_name, "audit" | "hash") {
            return Ok(Value::TypeName(debug_name.to_string()));
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
        self.debug_register_params(&lam.params);
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

    fn call_type_validation_method(
        &self,
        type_name: &str,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        let [input] = args.as_slice() else {
            return Err(RuntimeError::native(format!(
                "`{type_name}.{method}` expects one argument"
            )));
        };
        let result = self.validate_type_name(type_name, input.clone(), "$");
        match method {
            "parse" => result.map_err(|errors| RuntimeError::thrown(Value::Array(errors))),
            "safeParse" => Ok(match result {
                Ok(value) => Value::Object(vec![
                    ("ok".to_string(), Value::Bool(true)),
                    ("value".to_string(), value),
                ]),
                Err(errors) => Value::Object(vec![
                    ("ok".to_string(), Value::Bool(false)),
                    ("error".to_string(), Value::Array(errors)),
                ]),
            }),
            "errors" => Ok(match result {
                Ok(_) => Value::Array(Vec::new()),
                Err(errors) => Value::Array(errors),
            }),
            "is" | "validate" => Ok(Value::Bool(result.is_ok())),
            _ => unreachable!("field access only exposes known validator methods"),
        }
    }

    fn validate_type_name(
        &self,
        type_name: &str,
        value: Value,
        path: &str,
    ) -> Result<Value, Vec<Value>> {
        if let Some(alias) = self.type_aliases.get(type_name).cloned() {
            return self.validate_type_ref(&alias, value, path);
        }
        if let Some(fields) = self.type_structs.get(type_name).cloned() {
            return self.validate_struct_fields(type_name, &fields, value, path);
        }
        let ty = HirTypeRef {
            kind: HirTypeRefKind::Named(type_name.to_string()),
            constraints: Vec::new(),
            span: orv_diagnostics::Span::DUMMY,
        };
        self.validate_type_ref(&ty, value, path)
    }

    fn validate_type_ref(
        &self,
        ty: &HirTypeRef,
        value: Value,
        path: &str,
    ) -> Result<Value, Vec<Value>> {
        if let HirTypeRefKind::Named(name) = &ty.kind {
            if let Some(alias) = self.type_aliases.get(name).cloned() {
                let value = self.validate_type_ref(&alias, value, path)?;
                return self.apply_validation_constraints(value, &ty.constraints, path, ty);
            }
            if let Some(fields) = self.type_structs.get(name).cloned() {
                let value = self.validate_struct_fields(name, &fields, value, path)?;
                return self.apply_validation_constraints(value, &ty.constraints, path, ty);
            }
        }
        let actual = value.clone();
        apply_cast(value, ty).map_err(|err| {
            vec![validation_error(
                path,
                "type_mismatch",
                &err.message,
                &display_type_ref(ty),
                actual,
            )]
        })
    }

    fn validate_struct_fields(
        &self,
        type_name: &str,
        fields: &[(String, HirTypeRef)],
        value: Value,
        path: &str,
    ) -> Result<Value, Vec<Value>> {
        let actual = value.clone();
        let Value::Object(input_fields) = value else {
            return Err(vec![validation_error(
                path,
                "type_mismatch",
                &format!("expected object for `{type_name}`"),
                type_name,
                actual,
            )]);
        };

        let mut errors = Vec::new();
        let mut out = Vec::with_capacity(fields.len());
        for (field_name, field_ty) in fields {
            let field_path = child_path(path, field_name);
            match input_fields
                .iter()
                .find(|(input_name, _)| input_name == field_name)
            {
                Some((_, field_value)) => {
                    match self.validate_type_ref(field_ty, field_value.clone(), &field_path) {
                        Ok(value) => out.push((field_name.clone(), value)),
                        Err(mut field_errors) => errors.append(&mut field_errors),
                    }
                }
                None if self.type_ref_allows_void(field_ty) => {
                    out.push((field_name.clone(), Value::Void));
                }
                None => errors.push(validation_error(
                    &field_path,
                    "missing_required",
                    &format!("missing required property `{field_name}`"),
                    &display_type_ref(field_ty),
                    Value::Void,
                )),
            }
        }

        for (input_name, input_value) in &input_fields {
            if !fields
                .iter()
                .any(|(field_name, _)| field_name == input_name)
            {
                errors.push(validation_error(
                    &child_path(path, input_name),
                    "unknown_property",
                    &format!("unknown property `{input_name}`"),
                    type_name,
                    input_value.clone(),
                ));
            }
        }

        if errors.is_empty() {
            Ok(Value::Object(out))
        } else {
            Err(errors)
        }
    }

    fn apply_validation_constraints(
        &self,
        value: Value,
        constraints: &[HirTypeConstraint],
        path: &str,
        ty: &HirTypeRef,
    ) -> Result<Value, Vec<Value>> {
        let actual = value.clone();
        apply_value_constraints(value, constraints).map_err(|err| {
            vec![validation_error(
                path,
                "constraint_mismatch",
                &err.message,
                &display_type_ref(ty),
                actual,
            )]
        })
    }

    fn type_ref_allows_void(&self, ty: &HirTypeRef) -> bool {
        match &ty.kind {
            HirTypeRefKind::Nullable(_) => true,
            HirTypeRefKind::Named(name) => self
                .type_aliases
                .get(name)
                .is_some_and(|alias| self.type_ref_allows_void(alias)),
            _ => false,
        }
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
            (Value::Str(s), "contains") => match args.into_iter().next() {
                Some(Value::Str(needle)) => Ok(Value::Bool(s.contains(&needle))),
                Some(Value::Regex { pattern, flags }) => {
                    regex_contains(&s, &pattern, &flags).map(Value::Bool)
                }
                _ => Err(RuntimeError::native(
                    "contains expects string or regex argument",
                )),
            },
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
                | Value::Regex { .. }
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
                let arg = args.into_iter().next().ok_or_else(|| {
                    RuntimeError::native(format!("`{type_name}.from` expects one argument"))
                })?;
                convert_from(&type_name, arg)
            }
            (Value::TypeName(type_name), "parse" | "safeParse" | "errors" | "is" | "validate") => {
                self.call_type_validation_method(&type_name, method, args)
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
                let resolved_path = self.runtime_path(&path);
                let content = std::fs::read_to_string(&resolved_path).unwrap_or_default();
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
                let resolved_path = self.runtime_path(&path);
                std::fs::write(&resolved_path, &content)
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
                    .map_err(|e| RuntimeError::native(format!("`@process.run` failed: {e}")))?;
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
            (Value::TypeName(ns), "runDue" | "tick") if ns == "cron" => self.run_due_crons(),
            (Value::TypeName(ns), method) if is_reference_namespace_method(&ns, method) => {
                self.call_reference_method(&ns, method, &args)
            }
            (Value::Object(fields), "put" | "get" | "delete")
                if object_kind(&fields)
                    .is_some_and(|kind| matches!(kind, "cache" | "offline.store")) =>
            {
                self.call_stateful_object_method(&fields, method, &args)
            }
            (Value::Object(fields), "capture" | "book" | "verifyWebhook")
                if object_kind(&fields)
                    .is_some_and(|kind| matches!(kind, "payment.adapter" | "shipping.adapter")) =>
            {
                call_commerce_adapter_method(
                    &fields,
                    method,
                    &args,
                    self.runtime_options.working_dir.as_deref(),
                )
            }
            (
                Value::Object(fields),
                m @ ("create" | "find" | "findAll" | "update" | "delete" | "upsert" | "search"
                | "count" | "sum" | "transaction" | "schema" | "analyze"),
            ) if object_kind(&fields).is_some_and(|kind| kind == "db.adapter") => {
                call_external_db_adapter_method(&fields, m)
            }
            // ── C_db 메서드 ──
            //
            // 시그니처 (MVP):
            //   db.create(table: string, data: object) -> object
            //   db.find(table: string, filter: object) -> object | void
            //   db.findAll(table: string, filter: object?) -> object[]
            //   db.update(table: string, filter: object, data: object) -> int
            //   db.delete(table: string, filter: object) -> int
            (
                Value::Db(db),
                m @ ("create" | "find" | "findAll" | "update" | "delete" | "upsert" | "search"
                | "count" | "sum" | "transaction" | "schema" | "connect" | "analyze" | "save"
                | "load" | "wal" | "checkpoint" | "savepoint" | "rollback"),
            ) => call_db_method(&db, m, args, self.runtime_options.working_dir.as_deref()),
            (recv, m) => Err(RuntimeError::native(format!("no method `{m}` on {recv}"))),
        }
    }

    fn call_stateful_object_method(
        &mut self,
        fields: &[(String, Value)],
        method: &str,
        args: &[Value],
    ) -> Result<Value, RuntimeError> {
        let kind = object_kind(fields).unwrap_or_default();
        let name = object_field(fields, "name")
            .map(value_to_display)
            .unwrap_or_default();
        match (kind, method) {
            ("cache", "put") | ("offline.store", "put") => {
                let key = string_arg(args, 0, "`put` expects (key, value)")?;
                let value = args
                    .get(1)
                    .cloned()
                    .ok_or_else(|| RuntimeError::native("`put` expects (key, value)"))?;
                self.state_bucket_mut(kind, &name)
                    .insert(key.clone(), value);
                Ok(Value::Object(vec![
                    ("status".to_string(), Value::Str("stored".to_string())),
                    ("key".to_string(), Value::Str(key)),
                ]))
            }
            ("cache", "get") | ("offline.store", "get") => {
                let key = string_arg(args, 0, "`get` expects key")?;
                let value = self
                    .state_bucket(kind, &name)
                    .and_then(|items| items.get(&key).cloned())
                    .unwrap_or(Value::Void);
                Ok(Value::Object(vec![
                    ("key".to_string(), Value::Str(key)),
                    ("value".to_string(), value),
                ]))
            }
            ("cache", "delete") | ("offline.store", "delete") => {
                let key = string_arg(args, 0, "`delete` expects key")?;
                let removed = self.state_bucket_mut(kind, &name).remove(&key).is_some();
                Ok(Value::Object(vec![
                    ("key".to_string(), Value::Str(key)),
                    ("removed".to_string(), Value::Bool(removed)),
                ]))
            }
            _ => Err(RuntimeError::native(format!(
                "no method `{method}` on {kind}"
            ))),
        }
    }

    fn state_bucket(&self, kind: &str, name: &str) -> Option<&HashMap<String, Value>> {
        match kind {
            "cache" => self.cache_entries.get(name),
            "offline.store" => self.offline_entries.get(name),
            _ => None,
        }
    }

    fn state_bucket_mut(&mut self, kind: &str, name: &str) -> &mut HashMap<String, Value> {
        match kind {
            "cache" => self.cache_entries.entry(name.to_string()).or_default(),
            "offline.store" => self.offline_entries.entry(name.to_string()).or_default(),
            _ => unreachable!("state bucket checked before call"),
        }
    }

    fn call_reference_method(
        &mut self,
        ns: &str,
        method: &str,
        args: &[Value],
    ) -> Result<Value, RuntimeError> {
        match ns {
            "storage" => self.call_storage_method(method, args),
            "sync" => call_sync_method(method, args),
            "mail" | "mail.verify" => call_mail_method(ns, method, args),
            "media" => call_media_method(method, args),
            "push" => call_push_method(method, args),
            "payment" => call_payment_method(method, args),
            "shipping" => call_shipping_method(method, args),
            "offline" => call_offline_method(method, args),
            "cache" => call_cache_method(method, args),
            "net" | "net.tcp" | "net.udp" | "net.tun" => {
                self.require_unsafe_boundary(ns, method)?;
                call_net_method(ns, method, args)
            }
            "plugin" | "plugin.host" => call_plugin_method(ns, method, args),
            "gpu" => call_gpu_method(method, args),
            "observability" => call_observability_method(method, args),
            "ffi" => {
                self.require_unsafe_boundary(ns, method)?;
                call_ffi_method(method, args)
            }
            "audit" => call_audit_method(method, args),
            "hash" => call_hash_method(method, args),
            _ if ns.starts_with("job.") && method == "enqueue" => {
                let name = ns.strip_prefix("job.").unwrap_or(ns).to_string();
                let payload = args.first().cloned().unwrap_or(Value::Void);
                let (status, result, error) =
                    if let Some(handler) = self.job_handlers.get(&name).cloned() {
                        match self.run_job_handler_with_retries(&handler, args) {
                            Ok(result) => ("completed", result, None),
                            Err(err) => ("failed", Value::Void, Some(err.message)),
                        }
                    } else {
                        ("queued", Value::Void, None)
                    };
                let mut fields = vec![
                    ("name".to_string(), Value::Str(name)),
                    ("status".to_string(), Value::Str(status.to_string())),
                    ("payload".to_string(), payload),
                ];
                if !matches!(result, Value::Void) {
                    fields.push(("result".to_string(), result));
                }
                if let Some(error) = error {
                    fields.push(("error".to_string(), Value::Str(error)));
                }
                Ok(self.db.borrow_mut().create("Job", fields))
            }
            _ => Err(RuntimeError::native(format!(
                "no method `{method}` on <type {ns}>"
            ))),
        }
    }

    fn eval_job_domain(&mut self, args: &[HirExpr]) -> Result<Value, RuntimeError> {
        if args.is_empty() {
            return Ok(Value::TypeName("job".to_string()));
        }
        let job_name = args
            .first()
            .and_then(string_literal_from_expr)
            .unwrap_or_else(|| "job".to_string());
        if let Some(body) = args.iter().find_map(|arg| match &arg.kind {
            HirExprKind::Block(block) => Some(block.clone()),
            _ => None,
        }) {
            let params = args
                .iter()
                .find_map(job_params_from_expr)
                .unwrap_or_default();
            let retries = self.job_retries_from_args(args)?;
            self.job_handlers.insert(
                job_name.clone(),
                JobHandler {
                    params,
                    body: body.clone(),
                    retries,
                },
            );
            return Ok(Value::Object(vec![
                ("name".to_string(), Value::Str(job_name)),
                ("status".to_string(), Value::Str("registered".to_string())),
            ]));
        }
        Ok(Value::TypeName(format!("job.{job_name}")))
    }

    fn eval_cron_domain(&mut self, args: &[HirExpr]) -> Result<Value, RuntimeError> {
        if args.is_empty() {
            return Ok(Value::TypeName("cron".to_string()));
        }
        let schedule = args
            .iter()
            .find_map(string_literal_from_expr)
            .unwrap_or_else(|| "manual".to_string());
        if let Some(body) = args.iter().find_map(|arg| match &arg.kind {
            HirExprKind::Block(block) => Some(block.clone()),
            _ => None,
        }) {
            self.cron_handlers.push(CronHandler {
                schedule: schedule.clone(),
                body,
            });
        }
        Ok(self.db.borrow_mut().create(
            "Cron",
            vec![
                ("schedule".to_string(), Value::Str(schedule)),
                ("status".to_string(), Value::Str("registered".to_string())),
            ],
        ))
    }

    fn run_due_crons(&mut self) -> Result<Value, RuntimeError> {
        let handlers = self.cron_handlers.clone();
        let mut ran = 0i64;
        for handler in handlers {
            match self.eval_block(&handler.body) {
                Ok(_) => {
                    self.db.borrow_mut().create(
                        "CronRun",
                        vec![
                            ("schedule".to_string(), Value::Str(handler.schedule)),
                            ("ok".to_string(), Value::Bool(true)),
                        ],
                    );
                    ran += 1;
                }
                Err(err) => {
                    self.db.borrow_mut().create(
                        "CronRun",
                        vec![
                            ("schedule".to_string(), Value::Str(handler.schedule)),
                            ("ok".to_string(), Value::Bool(false)),
                            ("error".to_string(), Value::Str(err.message.clone())),
                        ],
                    );
                    return Err(err);
                }
            }
        }
        Ok(Value::Int(ran))
    }

    fn eval_unsafe_domain(&mut self, args: &[HirExpr]) -> Result<Value, RuntimeError> {
        for arg in args {
            if let HirExprKind::Block(block) = &arg.kind {
                self.unsafe_depth += 1;
                let result = self.eval_block(block);
                self.unsafe_depth -= 1;
                return result;
            }
        }
        Ok(Value::Object(vec![(
            "kind".to_string(),
            Value::Str("unsafe".to_string()),
        )]))
    }

    fn require_unsafe_boundary(&self, ns: &str, method: &str) -> Result<(), RuntimeError> {
        if self.unsafe_depth == 0 {
            return Err(RuntimeError::native(format!(
                "{ns} method `{method}` requires @unsafe boundary"
            )));
        }
        Ok(())
    }

    fn eval_observability_domain(&mut self, args: &[HirExpr]) -> Result<Value, RuntimeError> {
        if args.is_empty() {
            return Ok(Value::TypeName("observability".to_string()));
        }
        let config = if let Some(arg) = args.first() {
            self.eval_call_arg(arg)?
        } else {
            Value::Object(Vec::new())
        };
        let service = match &config {
            Value::Object(fields) => object_field(fields, "service")
                .cloned()
                .unwrap_or_else(|| Value::Str("orv".to_string())),
            _ => Value::Str("orv".to_string()),
        };
        Ok(Value::Object(vec![
            ("kind".to_string(), Value::Str("observability".to_string())),
            ("service".to_string(), service),
            ("config".to_string(), config),
        ]))
    }

    fn eval_offline_domain(&mut self, args: &[HirExpr]) -> Result<Value, RuntimeError> {
        if args.is_empty() {
            return Ok(Value::TypeName("offline".to_string()));
        }
        for arg in args {
            if let HirExprKind::Block(block) = &arg.kind {
                self.eval_block(block)?;
            }
        }
        Ok(Value::Object(vec![(
            "kind".to_string(),
            Value::Str("offline".to_string()),
        )]))
    }

    fn eval_ffi_domain(&mut self, args: &[HirExpr]) -> Result<Value, RuntimeError> {
        if args.is_empty() {
            return Ok(Value::TypeName("ffi".to_string()));
        }
        let abi = args
            .iter()
            .find_map(string_literal_from_expr)
            .unwrap_or_else(|| "native".to_string());
        Ok(Value::Object(vec![
            ("kind".to_string(), Value::Str("ffi".to_string())),
            ("abi".to_string(), Value::Str(abi)),
        ]))
    }

    fn eval_cache_domain(&mut self, args: &[HirExpr]) -> Result<Value, RuntimeError> {
        if args.is_empty() {
            return Ok(Value::TypeName("cache".to_string()));
        }
        let name = args
            .iter()
            .find_map(string_literal_from_expr)
            .unwrap_or_else(|| "cache".to_string());
        Ok(Value::Object(vec![
            ("kind".to_string(), Value::Str("cache".to_string())),
            ("name".to_string(), Value::Str(name)),
        ]))
    }

    fn eval_design_domain(&mut self, args: &[HirExpr]) -> Result<Value, RuntimeError> {
        if args.is_empty() {
            return Ok(self.design_value());
        }
        for arg in args {
            if let HirExprKind::Block(block) = &arg.kind {
                self.capture_design_block(block)?;
            }
        }
        Ok(self.design_value())
    }

    fn capture_design_block(&mut self, block: &HirBlock) -> Result<(), RuntimeError> {
        for stmt in &block.stmts {
            let HirStmt::Expr(expr) = stmt else {
                continue;
            };
            let HirExprKind::Domain { name, args, .. } = &expr.kind else {
                continue;
            };
            if !matches!(
                name.as_str(),
                "colors" | "spacing" | "typography" | "breakpoints"
            ) {
                continue;
            }
            let section = args
                .first()
                .map(|arg| self.eval_call_arg(arg))
                .transpose()?
                .unwrap_or_else(|| Value::Object(Vec::new()));
            self.design_tokens.insert(name.clone(), section);
        }
        Ok(())
    }

    fn design_value(&self) -> Value {
        Value::Object(
            self.design_tokens
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect(),
        )
    }

    fn job_retries_from_args(&mut self, args: &[HirExpr]) -> Result<usize, RuntimeError> {
        for arg in args {
            if let HirExprKind::Assign { target, value } = &arg.kind {
                if target.name == "retries" {
                    return match self.eval(value)? {
                        Value::Int(n) if n >= 0 => usize::try_from(n).map_err(|_| {
                            RuntimeError::native("`@job retries` is too large for this runtime")
                        }),
                        other => Err(RuntimeError::native(format!(
                            "`@job retries` expects non-negative int, got {other}"
                        ))),
                    };
                }
            }
        }
        Ok(0)
    }

    fn run_job_handler_with_retries(
        &mut self,
        handler: &JobHandler,
        args: &[Value],
    ) -> Result<Value, RuntimeError> {
        let mut last_err = None;
        for _ in 0..=handler.retries {
            match self.run_job_handler_once(handler, args) {
                Ok(value) => return Ok(value),
                Err(err) => last_err = Some(err),
            }
        }
        Err(last_err.unwrap_or_else(|| RuntimeError::native("job failed")))
    }

    fn run_job_handler_once(
        &mut self,
        handler: &JobHandler,
        args: &[Value],
    ) -> Result<Value, RuntimeError> {
        let mut scope = HashMap::new();
        if handler.params.is_empty() {
            scope.insert(
                "payload".to_string(),
                args.first().cloned().unwrap_or(Value::Void),
            );
        } else {
            for (idx, param) in handler.params.iter().enumerate() {
                scope.insert(param.clone(), args.get(idx).cloned().unwrap_or(Value::Void));
            }
        }
        self.dynamic_scopes.push(scope);
        let result = self.eval_block(&handler.body);
        self.dynamic_scopes.pop();
        result
    }

    fn call_storage_method(&mut self, method: &str, args: &[Value]) -> Result<Value, RuntimeError> {
        match method {
            "put" => {
                let path = string_arg(args, 0, "`@storage.put` expects (path, data)")?;
                let data = args
                    .get(1)
                    .cloned()
                    .ok_or_else(|| RuntimeError::native("`@storage.put` expects (path, data)"))?;
                let file = storage_file_record(&path, data);
                self.storage_files.insert(path, file.clone());
                Ok(file)
            }
            "get" => {
                let path = string_arg(args, 0, "`@storage.get` expects a string path")?;
                Ok(self
                    .storage_files
                    .get(&path)
                    .cloned()
                    .unwrap_or(Value::Void))
            }
            "delete" => {
                let path = string_arg(args, 0, "`@storage.delete` expects a string path")?;
                Ok(Value::Bool(self.storage_files.remove(&path).is_some()))
            }
            "putChunk" => {
                let upload_id = string_arg(
                    args,
                    0,
                    "`@storage.putChunk` expects (uploadId, index, data)",
                )?;
                let index = int_arg(
                    args,
                    1,
                    "`@storage.putChunk` expects (uploadId, index, data)",
                )?;
                let data = args.get(2).cloned().ok_or_else(|| {
                    RuntimeError::native("`@storage.putChunk` expects (uploadId, index, data)")
                })?;
                let chunks = self.storage_chunks.entry(upload_id.clone()).or_default();
                if let Some((_, slot)) = chunks.iter_mut().find(|(i, _)| *i == index) {
                    *slot = data.clone();
                } else {
                    chunks.push((index, data.clone()));
                }
                Ok(Value::Object(vec![
                    ("uploadId".to_string(), Value::Str(upload_id)),
                    ("index".to_string(), Value::Int(index)),
                    ("size".to_string(), Value::Int(storage_value_size(&data))),
                ]))
            }
            "merge" => self.merge_storage_chunks(args),
            "signedUrl" => {
                let path = string_arg(args, 0, "`@storage.signedUrl` expects a string path")?;
                Ok(Value::Str(format!("/orv-storage/{path}?signed=1")))
            }
            "stream" => {
                let path = string_arg(args, 0, "`@storage.stream` expects a string path")?;
                Ok(Value::Object(vec![
                    ("path".to_string(), Value::Str(path.clone())),
                    (
                        "url".to_string(),
                        Value::Str(format!("/orv-storage/{path}?signed=1")),
                    ),
                    (
                        "file".to_string(),
                        self.storage_files
                            .get(&path)
                            .cloned()
                            .unwrap_or(Value::Void),
                    ),
                ]))
            }
            _ => Err(RuntimeError::native(format!(
                "no method `{method}` on <type storage>"
            ))),
        }
    }

    fn merge_storage_chunks(&mut self, args: &[Value]) -> Result<Value, RuntimeError> {
        let upload_id = string_arg(args, 0, "`@storage.merge` expects an upload id string")?;
        let target = named_arg(args, "target")
            .and_then(|v| match v {
                Value::Str(path) => Some(path),
                _ => None,
            })
            .unwrap_or_else(|| format!("files/{upload_id}"));
        let mut chunks = self.storage_chunks.remove(&upload_id).unwrap_or_default();
        chunks.sort_by_key(|(idx, _)| *idx);
        let content = Value::Str(
            chunks
                .iter()
                .map(|(_, value)| value_to_display(value))
                .collect::<Vec<_>>()
                .join(""),
        );
        let size = chunks
            .iter()
            .map(|(_, value)| storage_value_size(value))
            .sum::<i64>();
        let file = Value::Object(vec![
            ("id".to_string(), Value::Str(upload_id)),
            ("path".to_string(), Value::Str(target.clone())),
            ("size".to_string(), Value::Int(size)),
            ("content".to_string(), content),
        ]);
        self.storage_files.insert(target, file.clone());
        Ok(file)
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
        self.debug_register_params(&func.params);
        for (id, v) in extras {
            self.env.insert(id, v);
        }
        let saved_return = self.pending_return.take();
        let saved_html = self.html_buffer.take();
        let saved_loop = self.loop_signal;
        self.debug_push_call(&func.name.name, func.span);
        let result = match &func.body {
            HirFunctionBody::Block(b) => self.eval_block_ctl(b).map(|ctl| {
                self.pending_return = None;
                ctl.into_value()
            }),
            HirFunctionBody::Expr(e) => self.eval(e),
        };
        self.debug_pop_call();
        let result_value = result?;
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
        self.debug_register_params(&func.params);
        let saved_return = self.pending_return.take();
        let saved_html = self.html_buffer.take();
        let saved_loop = self.loop_signal;
        self.debug_push_call(&func.name.name, func.span);
        let result = match &func.body {
            HirFunctionBody::Block(b) => self.eval_block_ctl(b).map(|ctl| {
                self.pending_return = None;
                ctl.into_value()
            }),
            HirFunctionBody::Expr(e) => self.eval(e),
        };
        self.debug_pop_call();
        let result_value = result?;
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
                    self.debug_register_ident(&l.name);
                    self.debug_capture(l.span);
                }
                HirStmt::Const(c) => {
                    let v = self.eval(&c.init)?;
                    self.env.insert(c.name.id, v);
                    self.debug_register_ident(&c.name);
                    self.debug_capture(c.span);
                }
                HirStmt::Function(f) => {
                    let rc = Rc::new((**f).clone());
                    self.env.insert(f.name.id, Value::Function(rc.clone()));
                    self.debug_register_ident(&f.name);
                    if f.is_define {
                        register_nested_defines(&mut self.env, &f.name.name, f);
                    }
                    self.debug_capture(f.span);
                }
                HirStmt::Struct(s) => {
                    self.type_structs.insert(
                        s.name.name.clone(),
                        s.fields
                            .iter()
                            .map(|field| (field.name.clone(), field.annotation.clone()))
                            .collect(),
                    );
                    self.env
                        .insert(s.name.id, Value::TypeName(s.name.name.clone()));
                    self.debug_register_ident(&s.name);
                    self.debug_capture(s.span);
                }
                HirStmt::TypeAlias(alias) => {
                    if alias.params.is_empty() {
                        self.type_aliases
                            .insert(alias.name.name.clone(), alias.ty.clone());
                    }
                    self.env
                        .insert(alias.name.id, Value::TypeName(alias.name.name.clone()));
                    self.debug_register_ident(&alias.name);
                    self.debug_capture(alias.span);
                }
                HirStmt::Enum(e) => {
                    let mut fields: Vec<(String, Value)> = Vec::with_capacity(e.variants.len());
                    for v in &e.variants {
                        let val = self.eval(&v.value)?;
                        fields.push((v.name.clone(), val));
                    }
                    self.env.insert(e.name.id, Value::Object(fields));
                    self.debug_register_ident(&e.name);
                    self.debug_capture(e.span);
                }
                HirStmt::Import(_) => {}
                HirStmt::Return(r) => {
                    let v = match &r.value {
                        Some(e) => self.eval(e)?,
                        None => Value::Void,
                    };
                    self.pending_return = Some(v.clone());
                    self.debug_capture(r.span);
                    return Ok(ControlFlow::Return(v));
                }
                HirStmt::Expr(e) => {
                    let v = self.eval(e)?;
                    self.debug_capture(e.span);
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
            let mut pairs: Vec<(NameId, Value)> = vec![(first.name.id, Value::Array(values))];
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
        if looks_like_html_value(&path_str) {
            return self.respond_raw(
                200,
                path_str.into_bytes(),
                "text/html; charset=utf-8".to_string(),
            );
        }
        let declared = self.runtime_path(&path_str);

        // 1) 대상 분류 — 파일이면 바로 서빙, 디렉토리면 rest join 후 재시도.
        let meta = match std::fs::metadata(&declared) {
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
            declared
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
        self.respond_raw(200, bytes, mime)
    }

    fn respond_raw(
        &mut self,
        status: i64,
        bytes: Vec<u8>,
        content_type: String,
    ) -> Result<Value, RuntimeError> {
        if self.response.is_none() {
            self.response = Some(ResponseCtx {
                status,
                payload: Value::Void,
                raw_body: Some(RawResponseBody {
                    bytes,
                    content_type,
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

    fn respond_value(&mut self, status: i64, payload: Value) -> Value {
        if self.response.is_none() {
            self.response = Some(ResponseCtx {
                status,
                payload,
                raw_body: None,
                location: None,
            });
        }
        self.pending_return = Some(Value::Void);
        Value::Void
    }

    /// SPEC §4.10 HTTP/Form binding: `@body: T`, `@query: T`, `@form: T`.
    ///
    /// The parser lowers this syntax to a request-state domain with one type
    /// handle argument. Success replaces the request-state value with the
    /// parsed/normalized value. Failure short-circuits the route with the
    /// standard validation response shape.
    fn eval_request_binding_domain(
        &mut self,
        name: &str,
        args: &[HirExpr],
    ) -> Result<Option<Value>, RuntimeError> {
        if !matches!(name, "body" | "query" | "form") || args.is_empty() {
            return Ok(None);
        }
        if args.len() != 1 {
            return Err(RuntimeError::native(format!(
                "`@{name}: Type` expects exactly one schema type"
            )));
        }
        let schema = self.eval(&args[0])?;
        let Value::TypeName(type_name) = schema else {
            return Err(RuntimeError::native(format!(
                "`@{name}: Type` expects a schema type, got {schema}"
            )));
        };
        let input = {
            let ctx = self
                .request
                .as_ref()
                .ok_or_else(|| RuntimeError::native("request binding requires a request"))?;
            match name {
                "body" => ctx.body.clone(),
                "query" => ctx
                    .query_value
                    .clone()
                    .unwrap_or_else(|| request_map_to_object(&ctx.query)),
                "form" => ctx.form.clone().unwrap_or_else(|| ctx.body.clone()),
                _ => unreachable!("request binding domain checked above"),
            }
        };
        match self.validate_type_name(&type_name, input, "$") {
            Ok(value) => {
                if let Some(ctx) = &mut self.request {
                    match name {
                        "body" => ctx.body = value,
                        "query" => ctx.query_value = Some(value),
                        "form" => ctx.form = Some(value),
                        _ => unreachable!("request binding domain checked above"),
                    }
                }
                Ok(Some(Value::Void))
            }
            Err(errors) => {
                let payload = Value::Object(vec![
                    (
                        "error".to_string(),
                        Value::Str("validation_failed".to_string()),
                    ),
                    ("fields".to_string(), Value::Array(errors)),
                ]);
                Ok(Some(self.respond_value(400, payload)))
            }
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
        Ok(Some(match name {
            "param" => request_map_to_object(&ctx.params),
            "query" => ctx
                .query_value
                .clone()
                .unwrap_or_else(|| request_map_to_object(&ctx.query)),
            "header" => request_map_to_object(&ctx.headers),
            "body" => ctx.body.clone(),
            "form" => ctx.form.clone().unwrap_or_else(|| ctx.body.clone()),
            "session" => self.session_value(),
            "request" => Value::Object(vec![
                ("method".into(), Value::Str(ctx.method.clone())),
                ("path".into(), Value::Str(ctx.path.clone())),
                ("ip".into(), Value::Str(ctx.ip.clone())),
                ("rawBody".into(), Value::Str(ctx.raw_body.clone())),
            ]),
            "response" => {
                let (status, headers) = match &self.response {
                    Some(resp) => (resp.status, response_headers_object(resp)),
                    None => (200, Value::Object(Vec::new())),
                };
                Value::Object(vec![
                    ("status".into(), Value::Int(status)),
                    ("headers".into(), headers),
                    ("duration".into(), Value::Int(0)),
                ])
            }
            _ => return Ok(None),
        }))
    }

    fn eval_session_domain(&mut self, args: &[HirExpr]) -> Result<Value, RuntimeError> {
        match args {
            [] => Ok(self.session_value()),
            [arg] if is_required_arg(arg) => {
                if let Some(id) = self.session_id_from_cookie() {
                    Ok(session_object(Some(id), self.session_role_from_cookie()))
                } else {
                    Ok(self.respond_value(
                        401,
                        Value::Object(vec![(
                            "err".to_string(),
                            Value::Str("session_required".to_string()),
                        )]),
                    ))
                }
            }
            _ => Err(RuntimeError::native(
                "`@session` expects no arguments or `required`",
            )),
        }
    }

    fn session_value(&self) -> Value {
        session_object(
            self.session_id_from_cookie(),
            self.session_role_from_cookie(),
        )
    }

    fn session_id_from_cookie(&self) -> Option<String> {
        let ctx = self.request.as_ref()?;
        cookie_value_from_headers(&ctx.headers, ORV_SESSION_COOKIE_NAME)
    }

    fn session_role_from_cookie(&self) -> Option<String> {
        let ctx = self.request.as_ref()?;
        cookie_value_from_headers(&ctx.headers, ORV_SESSION_ROLE_COOKIE_NAME)
    }

    fn eval_csrf_domain(&mut self, args: &[HirExpr]) -> Result<Value, RuntimeError> {
        if !args.is_empty() {
            return Err(RuntimeError::native("`@csrf` expects no arguments"));
        }
        let Some(ctx) = self.request.as_ref() else {
            return Ok(Value::Void);
        };
        if csrf_token_is_valid(ctx) {
            Ok(Value::Object(vec![
                ("status".to_string(), Value::Str("verified".to_string())),
                (
                    "cookie".to_string(),
                    Value::Str(ORV_CSRF_COOKIE_NAME.to_string()),
                ),
            ]))
        } else {
            Ok(self.respond_value(
                403,
                Value::Object(vec![(
                    "err".to_string(),
                    Value::Str("csrf_token_required".to_string()),
                )]),
            ))
        }
    }

    fn eval_auth_domain(&mut self, args: &[HirExpr]) -> Result<Value, RuntimeError> {
        let policy = self.eval_auth_policy(args)?;
        let session_id = self.session_id_from_cookie();
        let session_role = self.session_role_from_cookie();
        let requires_session = policy.required || policy.role.is_some();

        if requires_session && session_id.is_none() {
            return Ok(self.respond_value(
                401,
                Value::Object(vec![(
                    "err".to_string(),
                    Value::Str("auth_required".to_string()),
                )]),
            ));
        }

        if let Some(required_role) = &policy.role {
            if session_role.as_deref() != Some(required_role.as_str()) {
                return Ok(self.respond_value(
                    403,
                    Value::Object(vec![
                        ("err".to_string(), Value::Str("role_required".to_string())),
                        (
                            "requiredRole".to_string(),
                            Value::Str(required_role.clone()),
                        ),
                        (
                            "role".to_string(),
                            session_role.clone().map(Value::Str).unwrap_or(Value::Void),
                        ),
                    ]),
                ));
            }
        }

        Ok(Value::Object(vec![
            ("status".to_string(), Value::Str("authorized".to_string())),
            (
                "session".to_string(),
                session_object(session_id, session_role.clone()),
            ),
            (
                "role".to_string(),
                session_role.map(Value::Str).unwrap_or(Value::Void),
            ),
            (
                "requiredRole".to_string(),
                policy.role.map(Value::Str).unwrap_or(Value::Void),
            ),
        ]))
    }

    fn eval_auth_policy(&mut self, args: &[HirExpr]) -> Result<AuthPolicy, RuntimeError> {
        let mut policy = AuthPolicy::default();
        for arg in args {
            match &arg.kind {
                HirExprKind::Ident(ident) if ident.name == "required" => {
                    policy.required = true;
                }
                HirExprKind::Assign { target, value } if target.name == "required" => {
                    match self.eval(value)? {
                        Value::Bool(required) => policy.required = required,
                        other => {
                            return Err(RuntimeError::native(format!(
                                "`@Auth required` expects bool, got {other}"
                            )));
                        }
                    }
                }
                HirExprKind::Assign { target, value } if target.name == "role" => {
                    match self.eval(value)? {
                        Value::Str(role) if !role.is_empty() => policy.role = Some(role),
                        other => {
                            return Err(RuntimeError::native(format!(
                                "`@Auth role` expects non-empty string, got {other}"
                            )));
                        }
                    }
                }
                HirExprKind::Assign { target, .. } => {
                    return Err(RuntimeError::native(format!(
                        "`@Auth` got unknown policy `{}`",
                        target.name
                    )));
                }
                _ => {
                    return Err(RuntimeError::native(
                        "`@Auth` expects `required` and optional `role=\"...\"`",
                    ));
                }
            }
        }
        Ok(policy)
    }

    /// HTML 모드에서 `@tag ...` 도메인 호출 하나를 현재 버퍼에 렌더한다.
    ///
    /// - `@tag { ... }` — block 인자면 블록 본문을 HTML 모드로 재귀 평가.
    ///   태그 사이에 자식 태그/텍스트가 누적된다.
    /// - `@tag expr` — expr 을 평가해 텍스트 콘텐츠로 넣는다.
    /// - `@tag` — 빈 태그.
    fn render_tag(&mut self, name: &str, args: &[HirExpr]) -> Result<(), RuntimeError> {
        let mut attrs = String::new();
        let mut block_attr_prefixes = Vec::new();
        for arg in args {
            match &arg.kind {
                HirExprKind::Assign { target, value } => {
                    if let Some(attr) = self.render_attr(&target.name, value)? {
                        attrs.push_str(&attr);
                    }
                }
                HirExprKind::Block(block) => {
                    let prefix_len = self.render_block_attrs(block, &mut attrs)?;
                    block_attr_prefixes.push((arg.span, prefix_len));
                }
                _ => {}
            }
        }

        self.html_push(&format!("<{name}{attrs}>"));
        if is_html_void_tag(name) {
            return Ok(());
        }
        for arg in args {
            match &arg.kind {
                HirExprKind::Assign { .. } => {}
                HirExprKind::Block(inner) => {
                    let skip = block_attr_prefixes
                        .iter()
                        .find_map(|(span, len)| (*span == arg.span).then_some(*len))
                        .unwrap_or(0);
                    self.eval_html_child_block(inner, skip)?;
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

    fn render_block_attrs(
        &mut self,
        block: &HirBlock,
        attrs: &mut String,
    ) -> Result<usize, RuntimeError> {
        let mut prefix_len = 0;
        for stmt in &block.stmts {
            let Some((name, value)) = html_attr_stmt(stmt) else {
                break;
            };
            if let Some(attr) = self.render_attr(name, value)? {
                attrs.push_str(&attr);
            }
            prefix_len += 1;
        }
        Ok(prefix_len)
    }

    fn eval_html_child_block(
        &mut self,
        block: &HirBlock,
        skip: usize,
    ) -> Result<Value, RuntimeError> {
        if skip == 0 {
            return self.eval_block(block);
        }
        let child_block = HirBlock {
            stmts: block.stmts.iter().skip(skip).cloned().collect(),
            span: block.span,
        };
        self.eval_block(&child_block)
    }

    fn render_attr(&mut self, name: &str, value: &HirExpr) -> Result<Option<String>, RuntimeError> {
        let v = self.eval(value)?;
        Ok(match &v {
            Value::Void | Value::Bool(false) => None,
            Value::Bool(true) => Some(format!(" {name}")),
            Value::Function(_) | Value::Lambda(_) | Value::BoundMethod { .. }
                if is_event_attr(name) =>
            {
                Some(format!(" {name}=\"handler\""))
            }
            _ => Some(format!(
                " {name}=\"{}\"",
                html_escape_attr(&value_to_display(&v))
            )),
        })
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
        self.html_push(&html_escape_text(&s));
    }

    fn println(&mut self, v: &Value) -> Result<(), RuntimeError> {
        writeln!(self.writer, "{v}").map_err(|e| RuntimeError::native(format!("io error: {e}")))?;
        self.debug_record_output(&format!("{v}\n"));
        Ok(())
    }
}

fn string_literal_from_expr(expr: &HirExpr) -> Option<String> {
    let HirExprKind::String(segments) = &expr.kind else {
        return None;
    };
    let mut out = String::new();
    for segment in segments {
        let HirStringSegment::Str(s) = segment else {
            return None;
        };
        out.push_str(s);
    }
    Some(out)
}

fn request_map_to_object(map: &HashMap<String, String>) -> Value {
    Value::Object(
        map.iter()
            .map(|(key, value)| (key.clone(), Value::Str(value.clone())))
            .collect(),
    )
}

fn html_attr_stmt(stmt: &HirStmt) -> Option<(&str, &HirExpr)> {
    let HirStmt::Expr(expr) = stmt else {
        return None;
    };
    let HirExprKind::Assign { target, value } = &expr.kind else {
        return None;
    };
    Some((&target.name, value))
}

fn response_headers_object(resp: &ResponseCtx) -> Value {
    let mut headers = Vec::new();
    if let Some(location) = &resp.location {
        headers.push(("Location".to_string(), Value::Str(location.clone())));
    }
    if let Some(raw) = &resp.raw_body {
        headers.push((
            "Content-Type".to_string(),
            Value::Str(raw.content_type.clone()),
        ));
    }
    Value::Object(headers)
}

#[derive(Default)]
struct AuthPolicy {
    required: bool,
    role: Option<String>,
}

fn is_required_arg(expr: &HirExpr) -> bool {
    matches!(&expr.kind, HirExprKind::Ident(ident) if ident.name == "required")
}

fn is_declarative_auth_invocation(args: &[HirExpr]) -> bool {
    args.iter().any(|arg| match &arg.kind {
        HirExprKind::Ident(ident) => ident.name == "required",
        HirExprKind::Assign { target, .. } => matches!(target.name.as_str(), "required" | "role"),
        _ => false,
    })
}

fn session_object(id: Option<String>, role: Option<String>) -> Value {
    let present = id.is_some();
    Value::Object(vec![
        ("id".to_string(), id.map(Value::Str).unwrap_or(Value::Void)),
        (
            "role".to_string(),
            role.map(Value::Str).unwrap_or(Value::Void),
        ),
        ("present".to_string(), Value::Bool(present)),
    ])
}

fn cookie_value_from_headers(
    headers: &HashMap<String, String>,
    cookie_name: &str,
) -> Option<String> {
    let cookie_header = headers
        .iter()
        .find_map(|(name, value)| name.eq_ignore_ascii_case("cookie").then_some(value))?;
    cookie_header.split(';').find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        let value = value.trim();
        (name.trim() == cookie_name && !value.is_empty()).then(|| value.to_string())
    })
}

fn csrf_token_is_valid(ctx: &RequestCtx) -> bool {
    let Some(cookie) = cookie_value_from_headers(&ctx.headers, ORV_CSRF_COOKIE_NAME) else {
        return false;
    };
    let Some(submitted) = submitted_csrf_token(ctx) else {
        return false;
    };
    cookie == ORV_REFERENCE_CSRF_TOKEN && submitted == cookie
}

fn submitted_csrf_token(ctx: &RequestCtx) -> Option<String> {
    header_value_case_insensitive(&ctx.headers, "x-csrf-token")
        .or_else(|| header_value_case_insensitive(&ctx.headers, "x-orv-csrf-token"))
        .or_else(|| object_value_string(&ctx.body, "_csrf"))
        .or_else(|| object_value_string(&ctx.body, "csrf"))
        .or_else(|| ctx.query.get("_csrf").cloned())
}

fn header_value_case_insensitive(headers: &HashMap<String, String>, name: &str) -> Option<String> {
    headers.iter().find_map(|(header, value)| {
        (header.eq_ignore_ascii_case(name) && !value.is_empty()).then(|| value.clone())
    })
}

fn object_value_string(value: &Value, name: &str) -> Option<String> {
    let Value::Object(fields) = value else {
        return None;
    };
    match object_field(fields, name) {
        Some(Value::Str(value)) if !value.is_empty() => Some(value.clone()),
        Some(Value::Int(value)) => Some(value.to_string()),
        _ => None,
    }
}

fn is_event_attr(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("on") else {
        return false;
    };
    rest.chars().next().is_some_and(|c| c.is_ascii_uppercase())
}

fn is_html_void_tag(name: &str) -> bool {
    matches!(
        name,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

fn html_escape_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn html_escape_attr(s: &str) -> String {
    html_escape_text(s).replace('"', "&quot;")
}

fn looks_like_html_value(s: &str) -> bool {
    let trimmed = s.trim_start();
    trimmed.starts_with("<html") || trimmed.starts_with("<!doctype")
}

fn job_params_from_expr(expr: &HirExpr) -> Option<Vec<String>> {
    let HirExprKind::Object(fields) = &expr.kind else {
        return None;
    };
    let [field] = fields.as_slice() else {
        return None;
    };
    if field.name != "__params__" {
        return None;
    }
    let HirExprKind::Array(items) = &field.value.kind else {
        return None;
    };
    Some(items.iter().filter_map(string_literal_from_expr).collect())
}

fn call_sync_method(method: &str, args: &[Value]) -> Result<Value, RuntimeError> {
    match method {
        "open" => {
            let kind = string_arg(args, 0, "`@sync.open` expects (kind, id)")?;
            let id = string_arg(args, 1, "`@sync.open` expects (kind, id)")?;
            Ok(Value::Object(vec![
                ("kind".to_string(), Value::Str(kind.clone())),
                ("id".to_string(), Value::Str(id.clone())),
                ("path".to_string(), Value::Str(format!("/sync/{kind}/{id}"))),
                ("state".to_string(), Value::Object(Vec::new())),
            ]))
        }
        "connect" => {
            let kind = string_arg(args, 0, "`@sync.connect` expects (kind, path)")?;
            let path = string_arg(args, 1, "`@sync.connect` expects (kind, path)")?;
            Ok(Value::Object(vec![
                ("kind".to_string(), Value::Str(kind)),
                ("id".to_string(), Value::Str(path.clone())),
                ("path".to_string(), Value::Str(path)),
                ("state".to_string(), Value::Object(Vec::new())),
            ]))
        }
        "buffer" => Ok(Value::Object(vec![
            ("kind".to_string(), Value::Str("buffer".to_string())),
            ("ops".to_string(), Value::Array(Vec::new())),
        ])),
        _ => Err(RuntimeError::native(format!(
            "no method `{method}` on <type sync>"
        ))),
    }
}

fn call_mail_method(ns: &str, method: &str, args: &[Value]) -> Result<Value, RuntimeError> {
    match (ns, method) {
        ("mail", "send") => Ok(Value::Object(vec![
            ("status".to_string(), Value::Str("sent".to_string())),
            (
                "message".to_string(),
                args.first().cloned().unwrap_or(Value::Void),
            ),
        ])),
        ("mail.verify", "dkim" | "spf" | "dmarc") => Ok(Value::Bool(true)),
        _ => Err(RuntimeError::native(format!(
            "no method `{method}` on <type {ns}>"
        ))),
    }
}

fn call_media_method(method: &str, args: &[Value]) -> Result<Value, RuntimeError> {
    match method {
        "camera" | "screen" => Ok(Value::Object(vec![
            ("kind".to_string(), Value::Str(method.to_string())),
            (
                "constraints".to_string(),
                args.first()
                    .cloned()
                    .unwrap_or_else(|| Value::Object(Vec::new())),
            ),
        ])),
        "pipeline" | "player" => Ok(Value::Object(vec![
            ("kind".to_string(), Value::Str(method.to_string())),
            (
                "source".to_string(),
                args.first().cloned().unwrap_or(Value::Void),
            ),
        ])),
        _ => Err(RuntimeError::native(format!(
            "no method `{method}` on <type media>"
        ))),
    }
}

fn call_push_method(method: &str, args: &[Value]) -> Result<Value, RuntimeError> {
    match method {
        "request" => Ok(Value::Bool(true)),
        "subscribe" => Ok(Value::Object(vec![
            (
                "endpoint".to_string(),
                Value::Str("push://subscription".to_string()),
            ),
            (
                "keys".to_string(),
                Value::Object(vec![
                    ("p256dh".to_string(), Value::Str("local-p256dh".to_string())),
                    ("auth".to_string(), Value::Str("local-auth".to_string())),
                ]),
            ),
            (
                "options".to_string(),
                args.first()
                    .cloned()
                    .unwrap_or_else(|| Value::Object(Vec::new())),
            ),
        ])),
        "send" => Ok(Value::Object(vec![
            ("status".to_string(), Value::Str("sent".to_string())),
            (
                "message".to_string(),
                args.first().cloned().unwrap_or(Value::Void),
            ),
        ])),
        _ => Err(RuntimeError::native(format!(
            "no method `{method}` on <type push>"
        ))),
    }
}

fn call_payment_method(method: &str, args: &[Value]) -> Result<Value, RuntimeError> {
    match method {
        "connect" => {
            let url = string_arg(args, 0, "`@payment.connect` expects adapter url")?;
            let provider = reference_adapter_provider(&url, "payment")?;
            Ok(Value::Object(vec![
                (
                    "kind".to_string(),
                    Value::Str("payment.adapter".to_string()),
                ),
                ("provider".to_string(), Value::Str(provider)),
                ("url".to_string(), Value::Str(url)),
            ]))
        }
        "capture" => payment_capture_value("test", args),
        _ => Err(RuntimeError::native(format!(
            "no method `{method}` on <type payment>"
        ))),
    }
}

fn call_shipping_method(method: &str, args: &[Value]) -> Result<Value, RuntimeError> {
    match method {
        "connect" => {
            let url = string_arg(args, 0, "`@shipping.connect` expects adapter url")?;
            let provider = reference_adapter_provider(&url, "shipping")?;
            Ok(Value::Object(vec![
                (
                    "kind".to_string(),
                    Value::Str("shipping.adapter".to_string()),
                ),
                ("provider".to_string(), Value::Str(provider)),
                ("url".to_string(), Value::Str(url)),
            ]))
        }
        "book" => shipping_booking_value("test", args),
        _ => Err(RuntimeError::native(format!(
            "no method `{method}` on <type shipping>"
        ))),
    }
}

fn call_commerce_adapter_method(
    fields: &[(String, Value)],
    method: &str,
    args: &[Value],
    working_dir: Option<&Path>,
) -> Result<Value, RuntimeError> {
    let provider = object_field(fields, "provider")
        .map(value_to_display)
        .unwrap_or_else(|| "test".to_string());
    let result = match (object_kind(fields).unwrap_or_default(), method) {
        ("payment.adapter", "capture") if provider == "http" => {
            payment_capture_value(&provider, args)?;
            http_commerce_adapter_value(fields, "payment.capture", args)?
        }
        ("payment.adapter", "verifyWebhook") if provider == "stripe" => {
            stripe_webhook_verification_value(args)?
        }
        ("payment.adapter", "verifyWebhook") => {
            return Err(RuntimeError::native(format!(
                "payment.verifyWebhook is not implemented for provider `{provider}`"
            )));
        }
        ("shipping.adapter", "book") if provider == "http" => {
            shipping_booking_value(&provider, args)?;
            http_commerce_adapter_value(fields, "shipping.booking", args)?
        }
        ("payment.adapter", "capture") if provider == "stripe" => {
            stripe_provider_capture_value(args)?
        }
        ("shipping.adapter", "book") if provider == "carrier" => {
            carrier_provider_booking_value(args)?
        }
        ("payment.adapter", "capture") => payment_capture_value(&provider, args)?,
        ("shipping.adapter", "book") => shipping_booking_value(&provider, args)?,
        (kind, _) => {
            return Err(RuntimeError::native(format!(
                "no method `{method}` on {kind}"
            )))
        }
    };
    append_file_commerce_adapter_record(fields, &result, working_dir)?;
    Ok(result)
}

fn call_external_db_adapter_method(
    fields: &[(String, Value)],
    method: &str,
) -> Result<Value, RuntimeError> {
    match method {
        "analyze" | "schema" => Ok(Value::Object(fields.to_vec())),
        _ => {
            let provider =
                object_string_field(fields, "provider").unwrap_or_else(|| "external".to_string());
            Err(RuntimeError::native(format!(
                "external db adapter {provider} is not implemented in the reference runtime"
            )))
        }
    }
}

fn reference_adapter_provider(url: &str, kind: &str) -> Result<String, RuntimeError> {
    let Some((scheme, _target)) = url.split_once("://") else {
        return Err(RuntimeError::native(format!(
            "`@{kind}.connect` expects adapter url"
        )));
    };
    if matches!(scheme, "test" | "local" | "file" | "http")
        || (kind == "payment" && scheme == "stripe")
        || (kind == "shipping" && scheme == "carrier")
    {
        Ok(scheme.to_string())
    } else {
        Err(RuntimeError::native(format!(
            "external {kind} adapters are not implemented for `{url}`; supported schemes are test://, local://, file://, http://, stripe:// for payment, and carrier:// for shipping"
        )))
    }
}

fn http_commerce_adapter_value(
    fields: &[(String, Value)],
    kind: &str,
    args: &[Value],
) -> Result<Value, RuntimeError> {
    let Some(url) = object_field(fields, "url").map(value_to_display) else {
        return Err(RuntimeError::native("http commerce adapter missing url"));
    };
    let payload = args.first().cloned().unwrap_or(Value::Void);
    let request = serde_json::json!({
        "kind": kind,
        "payload": runtime_value_json(&payload),
    })
    .to_string();
    let response = http_post_json(&url, &request)?;
    let value = serde_json::from_str::<serde_json::Value>(&response).map_err(|source| {
        RuntimeError::native(format!(
            "http commerce adapter response was not JSON: {source}"
        ))
    })?;
    Ok(runtime_value_from_json(value))
}

struct HttpAdapterUrl {
    host: String,
    host_header: String,
    port: u16,
    path: String,
}

fn http_post_json(url: &str, body: &str) -> Result<String, RuntimeError> {
    http_post_json_with_headers(url, body, &[])
}

fn http_post_json_with_headers(
    url: &str,
    body: &str,
    extra_headers: &[(&str, String)],
) -> Result<String, RuntimeError> {
    let parsed = parse_http_adapter_url(url)?;
    let mut stream = TcpStream::connect((parsed.host.as_str(), parsed.port)).map_err(|source| {
        RuntimeError::native(format!(
            "http commerce adapter failed to connect {url}: {source}"
        ))
    })?;
    let timeout = Some(Duration::from_secs(5));
    stream.set_read_timeout(timeout).map_err(|source| {
        RuntimeError::native(format!(
            "http commerce adapter failed to set read timeout: {source}"
        ))
    })?;
    stream.set_write_timeout(timeout).map_err(|source| {
        RuntimeError::native(format!(
            "http commerce adapter failed to set write timeout: {source}"
        ))
    })?;
    let mut request = format!(
        "POST {} HTTP/1.1\r\nHost: {}\r\ncontent-type: application/json\r\naccept: application/json\r\n",
        parsed.path, parsed.host_header
    );
    for (name, value) in extra_headers {
        request.push_str(name);
        request.push_str(": ");
        request.push_str(value);
        request.push_str("\r\n");
    }
    request.push_str(&format!(
        "content-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body
    ));
    stream.write_all(request.as_bytes()).map_err(|source| {
        RuntimeError::native(format!(
            "http commerce adapter failed to write request: {source}"
        ))
    })?;
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).map_err(|source| {
        RuntimeError::native(format!(
            "http commerce adapter failed to read response: {source}"
        ))
    })?;
    let header_end = bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| RuntimeError::native("http commerce adapter response missing headers"))?;
    let headers = String::from_utf8_lossy(&bytes[..header_end]);
    let status = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| RuntimeError::native("http commerce adapter response missing status"))?;
    let response_body = String::from_utf8(bytes[header_end + 4..].to_vec()).map_err(|source| {
        RuntimeError::native(format!(
            "http commerce adapter response body was not utf-8: {source}"
        ))
    })?;
    if !(200..300).contains(&status) {
        return Err(RuntimeError::native(format!(
            "http commerce adapter returned {status}: {response_body}"
        )));
    }
    Ok(response_body)
}

fn provider_http_post_json(
    url: &str,
    body: &str,
    extra_headers: &[(&str, String)],
) -> Result<String, RuntimeError> {
    let mut last_error = None;
    for attempt in 0..3 {
        match http_post_json_with_headers(url, body, extra_headers) {
            Ok(response) => return Ok(response),
            Err(error) if attempt < 2 && provider_http_error_is_retryable(&error) => {
                last_error = Some(error);
            }
            Err(error) => return Err(error),
        }
    }
    Err(last_error.unwrap_or_else(|| RuntimeError::native("provider request failed")))
}

fn provider_http_error_is_retryable(error: &RuntimeError) -> bool {
    let message = &error.message;
    message.contains(" returned 5")
        || message.contains("failed to connect")
        || message.contains("failed to read response")
        || message.contains("timed out")
}

fn provider_idempotency_key(kind: &str, args: &[Value]) -> String {
    let order = payload_field(args, "orderId")
        .map(|value| value_to_display(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    format!("{kind}:{order}")
}

fn parse_http_adapter_url(url: &str) -> Result<HttpAdapterUrl, RuntimeError> {
    let Some(rest) = url.strip_prefix("http://") else {
        return Err(RuntimeError::native(format!(
            "http commerce adapter only supports http:// URLs in the reference runtime: {url}"
        )));
    };
    let (authority, path) = rest
        .split_once('/')
        .map_or((rest, "/".to_string()), |(authority, path)| {
            (authority, format!("/{path}"))
        });
    if authority.is_empty() {
        return Err(RuntimeError::native(
            "http commerce adapter URL missing host",
        ));
    }
    let (host, port) = if let Some((host, port)) = authority.rsplit_once(':') {
        let port = port.parse::<u16>().map_err(|source| {
            RuntimeError::native(format!(
                "http commerce adapter URL has invalid port: {source}"
            ))
        })?;
        (host.to_string(), port)
    } else {
        (authority.to_string(), 80)
    };
    if host.is_empty() {
        return Err(RuntimeError::native(
            "http commerce adapter URL missing host",
        ));
    }
    Ok(HttpAdapterUrl {
        host,
        host_header: authority.to_string(),
        port,
        path,
    })
}

fn append_file_commerce_adapter_record(
    fields: &[(String, Value)],
    record: &Value,
    working_dir: Option<&Path>,
) -> Result<(), RuntimeError> {
    let Some(url) = object_field(fields, "url").map(value_to_display) else {
        return Ok(());
    };
    let Some(path) = url.strip_prefix("file://") else {
        return Ok(());
    };
    if path.is_empty() {
        return Err(RuntimeError::native(
            "file commerce adapter expects a JSONL record path",
        ));
    }
    let path = resolve_runtime_path(path, working_dir);
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).map_err(|source| {
            RuntimeError::native(format!(
                "commerce adapter failed to create {}: {source}",
                parent.display()
            ))
        })?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|source| {
            RuntimeError::native(format!(
                "commerce adapter failed to open {}: {source}",
                path.display()
            ))
        })?;
    let bytes = serde_json::to_vec(&runtime_value_json(record)).map_err(|source| {
        RuntimeError::native(format!(
            "commerce adapter failed to encode record: {source}"
        ))
    })?;
    file.write_all(&bytes).map_err(|source| {
        RuntimeError::native(format!(
            "commerce adapter failed to write {}: {source}",
            path.display()
        ))
    })?;
    file.write_all(b"\n").map_err(|source| {
        RuntimeError::native(format!(
            "commerce adapter failed to finish {}: {source}",
            path.display()
        ))
    })?;
    file.sync_all().map_err(|source| {
        RuntimeError::native(format!(
            "commerce adapter failed to sync {}: {source}",
            path.display()
        ))
    })
}

fn runtime_value_json(value: &Value) -> serde_json::Value {
    match value {
        Value::Void => serde_json::Value::Null,
        Value::Bool(value) => serde_json::Value::Bool(*value),
        Value::Int(value) => serde_json::Value::Number((*value).into()),
        Value::Float(value) => serde_json::Number::from_f64(*value)
            .map_or(serde_json::Value::Null, serde_json::Value::Number),
        Value::Str(value) => serde_json::Value::String(value.clone()),
        Value::Array(values) | Value::Tuple(values) => {
            serde_json::Value::Array(values.iter().map(runtime_value_json).collect())
        }
        Value::Object(fields) => serde_json::Value::Object(
            fields
                .iter()
                .map(|(key, value)| (key.clone(), runtime_value_json(value)))
                .collect(),
        ),
        other => serde_json::Value::String(value_to_display(other)),
    }
}

fn runtime_value_from_json(value: serde_json::Value) -> Value {
    match value {
        serde_json::Value::Null => Value::Void,
        serde_json::Value::Bool(value) => Value::Bool(value),
        serde_json::Value::Number(value) => value
            .as_i64()
            .map(Value::Int)
            .or_else(|| value.as_f64().map(Value::Float))
            .unwrap_or(Value::Void),
        serde_json::Value::String(value) => Value::Str(value),
        serde_json::Value::Array(values) => {
            Value::Array(values.into_iter().map(runtime_value_from_json).collect())
        }
        serde_json::Value::Object(fields) => Value::Object(
            fields
                .into_iter()
                .map(|(key, value)| (key, runtime_value_from_json(value)))
                .collect(),
        ),
    }
}

fn payment_capture_value(provider: &str, args: &[Value]) -> Result<Value, RuntimeError> {
    let order_id = payload_field(args, "orderId")
        .ok_or_else(|| RuntimeError::native("`payment.capture` expects orderId"))?;
    let amount = payload_field(args, "amount")
        .ok_or_else(|| RuntimeError::native("`payment.capture` expects amount"))?;
    let method = payload_field(args, "method").unwrap_or_else(|| Value::Str("card".to_string()));
    let id = if provider == "stripe" {
        "STRIPE-PAY-LOCAL"
    } else {
        "PAY-LOCAL"
    };
    let mut fields = vec![
        (
            "kind".to_string(),
            Value::Str("payment.capture".to_string()),
        ),
        ("provider".to_string(), Value::Str(provider.to_string())),
        ("status".to_string(), Value::Str("captured".to_string())),
        ("id".to_string(), Value::Str(id.to_string())),
        ("orderId".to_string(), order_id),
        ("amount".to_string(), amount),
        ("method".to_string(), method),
    ];
    fields.extend(payment_provider_credential_fields(provider));
    Ok(Value::Object(fields))
}

fn stripe_provider_capture_value(args: &[Value]) -> Result<Value, RuntimeError> {
    let base = payment_capture_value("stripe", args)?;
    let Some(endpoint) = provider_env_value("STRIPE_API_ENDPOINT") else {
        return Ok(base);
    };
    let secret = provider_env_value("STRIPE_SECRET_KEY")
        .ok_or_else(|| RuntimeError::native("stripe provider capture expects STRIPE_SECRET_KEY"))?;
    let payload = args.first().cloned().unwrap_or(Value::Void);
    let request = serde_json::json!({
        "kind": "stripe.payment_intent.create",
        "payload": runtime_value_json(&payload),
    })
    .to_string();
    let response = provider_http_post_json(
        &endpoint,
        &request,
        &[
            ("authorization", format!("Bearer {secret}")),
            (
                "idempotency-key",
                provider_idempotency_key("stripe.payment_intent.create", args),
            ),
        ],
    )?;
    let remote = serde_json::from_str::<serde_json::Value>(&response).map_err(|source| {
        RuntimeError::native(format!("stripe provider response was not JSON: {source}"))
    })?;
    merge_provider_response(base, remote)
}

fn stripe_webhook_verification_value(args: &[Value]) -> Result<Value, RuntimeError> {
    let payload = payload_field(args, "payload")
        .ok_or_else(|| RuntimeError::native("`payment.verifyWebhook` expects payload"))?;
    let signature = payload_field(args, "signature")
        .ok_or_else(|| RuntimeError::native("`payment.verifyWebhook` expects signature"))?;
    let payload = value_to_display(&payload);
    let signature = value_to_display(&signature);
    let secrets = stripe_webhook_secrets();
    let mut matched_secret = None;
    for secret in &secrets {
        if stripe_signature_matches(&secret.value, &payload, &signature)? {
            matched_secret = Some(secret.label);
            break;
        }
    }
    let status = match (secrets.is_empty(), matched_secret) {
        (_, Some(_)) => "verified",
        (false, None) => "invalid",
        (true, None) => "missing_secret",
    };
    let webhook_secret_status = if secrets.is_empty() {
        "missing"
    } else {
        "configured"
    };
    let webhook_secret_match = matched_secret.unwrap_or("none");

    Ok(Value::Object(vec![
        (
            "kind".to_string(),
            Value::Str("payment.webhook".to_string()),
        ),
        ("provider".to_string(), Value::Str("stripe".to_string())),
        ("status".to_string(), Value::Str(status.to_string())),
        (
            "signatureScheme".to_string(),
            Value::Str("stripe-v1".to_string()),
        ),
        (
            "webhookSecretStatus".to_string(),
            Value::Str(webhook_secret_status.to_string()),
        ),
        (
            "webhookSecretMatch".to_string(),
            Value::Str(webhook_secret_match.to_string()),
        ),
    ]))
}

struct StripeWebhookSecret {
    label: &'static str,
    value: String,
}

fn stripe_webhook_secrets() -> Vec<StripeWebhookSecret> {
    [
        ("primary", "STRIPE_WEBHOOK_SECRET"),
        ("previous", "STRIPE_WEBHOOK_SECRET_PREVIOUS"),
    ]
    .into_iter()
    .filter_map(|(label, env)| {
        provider_env_value(env).map(|value| StripeWebhookSecret { label, value })
    })
    .collect()
}

fn stripe_signature_matches(
    secret: &str,
    payload: &str,
    signature: &str,
) -> Result<bool, RuntimeError> {
    let mut timestamp = None;
    let mut candidates = Vec::new();
    for part in signature.split(',') {
        let Some((name, value)) = part.split_once('=') else {
            continue;
        };
        match name.trim() {
            "t" => timestamp = Some(value.trim()),
            "v1" => candidates.push(value.trim()),
            _ => {}
        }
    }

    if candidates.is_empty() {
        let expected = hmac_sha256_hex(secret, payload)?;
        return Ok(constant_time_ascii_eq(signature.trim(), &expected));
    }

    let signed_payload =
        timestamp.map_or_else(|| payload.to_string(), |t| format!("{t}.{payload}"));
    let expected = hmac_sha256_hex(secret, &signed_payload)?;
    Ok(candidates
        .iter()
        .any(|candidate| constant_time_ascii_eq(candidate, &expected)))
}

fn hmac_sha256_hex(secret: &str, payload: &str) -> Result<String, RuntimeError> {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).map_err(|source| {
        RuntimeError::native(format!("payment webhook HMAC setup failed: {source}"))
    })?;
    mac.update(payload.as_bytes());
    let bytes = mac.finalize().into_bytes();
    Ok(hex_encode(&bytes))
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(HEX[(byte >> 4) as usize]));
        out.push(char::from(HEX[(byte & 0x0f) as usize]));
    }
    out
}

fn constant_time_ascii_eq(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    if left.len() != right.len() {
        return false;
    }
    let mut diff = 0_u8;
    for (a, b) in left.iter().zip(right) {
        diff |= a ^ b;
    }
    diff == 0
}

fn shipping_booking_value(provider: &str, args: &[Value]) -> Result<Value, RuntimeError> {
    let order_id = payload_field(args, "orderId")
        .ok_or_else(|| RuntimeError::native("`shipping.book` expects orderId"))?;
    let carrier = payload_field(args, "carrier").unwrap_or_else(|| Value::Str("post".to_string()));
    let address = payload_field(args, "address").unwrap_or_else(|| Value::Str(String::new()));
    let (id, tracking) = if provider == "carrier" {
        ("CARRIER-SHIP-LOCAL", "TRK-CARRIER-LOCAL")
    } else {
        ("SHIP-LOCAL", "TRK-LOCAL")
    };
    let mut fields = vec![
        (
            "kind".to_string(),
            Value::Str("shipping.booking".to_string()),
        ),
        ("provider".to_string(), Value::Str(provider.to_string())),
        ("status".to_string(), Value::Str("ready".to_string())),
        ("id".to_string(), Value::Str(id.to_string())),
        ("orderId".to_string(), order_id),
        ("carrier".to_string(), carrier),
        ("address".to_string(), address),
        ("tracking".to_string(), Value::Str(tracking.to_string())),
    ];
    fields.extend(shipping_provider_credential_fields(provider));
    Ok(Value::Object(fields))
}

fn carrier_provider_booking_value(args: &[Value]) -> Result<Value, RuntimeError> {
    let base = shipping_booking_value("carrier", args)?;
    let Some(endpoint) = provider_env_value("CARRIER_API_ENDPOINT") else {
        return Ok(base);
    };
    let api_key = provider_env_value("CARRIER_API_KEY")
        .ok_or_else(|| RuntimeError::native("carrier provider booking expects CARRIER_API_KEY"))?;
    let payload = args.first().cloned().unwrap_or(Value::Void);
    let request = serde_json::json!({
        "kind": "carrier.shipment.create",
        "payload": runtime_value_json(&payload),
    })
    .to_string();
    let response = provider_http_post_json(
        &endpoint,
        &request,
        &[
            ("authorization", format!("Bearer {api_key}")),
            (
                "idempotency-key",
                provider_idempotency_key("carrier.shipment.create", args),
            ),
        ],
    )?;
    let remote = serde_json::from_str::<serde_json::Value>(&response).map_err(|source| {
        RuntimeError::native(format!("carrier provider response was not JSON: {source}"))
    })?;
    merge_provider_response(base, remote)
}

fn merge_provider_response(base: Value, remote: serde_json::Value) -> Result<Value, RuntimeError> {
    let Value::Object(mut fields) = base else {
        return Ok(runtime_value_from_json(remote));
    };
    let serde_json::Value::Object(remote_fields) = remote else {
        return Ok(Value::Object(fields));
    };
    for (key, value) in remote_fields {
        if let Some((_, existing)) = fields.iter_mut().find(|(field, _)| field == &key) {
            *existing = runtime_value_from_json(value);
        } else {
            fields.push((key, runtime_value_from_json(value)));
        }
    }
    Ok(Value::Object(fields))
}

fn payment_provider_credential_fields(provider: &str) -> Vec<(String, Value)> {
    if provider != "stripe" {
        return Vec::new();
    }
    vec![
        provider_credential_field("credentialStatus", "STRIPE_SECRET_KEY"),
        (
            "webhookSecretStatus".to_string(),
            Value::Str(stripe_webhook_secret_status().to_string()),
        ),
    ]
}

fn shipping_provider_credential_fields(provider: &str) -> Vec<(String, Value)> {
    if provider != "carrier" {
        return Vec::new();
    }
    vec![
        provider_credential_field("credentialStatus", "CARRIER_API_KEY"),
        provider_credential_field("webhookSecretStatus", "CARRIER_WEBHOOK_SECRET"),
    ]
}

fn provider_credential_field(field: &str, env: &str) -> (String, Value) {
    let status = if provider_env_configured(env) {
        "configured"
    } else {
        "missing"
    };
    (field.to_string(), Value::Str(status.to_string()))
}

fn stripe_webhook_secret_status() -> &'static str {
    if stripe_webhook_secrets().is_empty() {
        "missing"
    } else {
        "configured"
    }
}

fn provider_env_configured(env: &str) -> bool {
    provider_env_value(env).is_some()
}

fn provider_env_value(env: &str) -> Option<String> {
    let value = {
        #[cfg(test)]
        {
            let override_v = test_env::ENV_OVERRIDES
                .get()
                .and_then(|lock| lock.lock().ok()?.get(env).cloned());
            override_v.or_else(|| std::env::var(env).ok())
        }
        #[cfg(not(test))]
        {
            std::env::var(env).ok()
        }
    };
    value.and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    })
}

fn call_offline_method(method: &str, args: &[Value]) -> Result<Value, RuntimeError> {
    match method {
        "store" => {
            let name = string_arg(args, 0, "`@offline.store` expects a store name")?;
            Ok(Value::Object(vec![
                ("kind".to_string(), Value::Str("offline.store".to_string())),
                ("name".to_string(), Value::Str(name)),
                ("records".to_string(), Value::Array(Vec::new())),
            ]))
        }
        _ => Err(RuntimeError::native(format!(
            "no method `{method}` on <type offline>"
        ))),
    }
}

fn call_cache_method(method: &str, args: &[Value]) -> Result<Value, RuntimeError> {
    match method {
        "open" => {
            let name = string_arg(args, 0, "`@cache.open` expects a cache name")?;
            Ok(Value::Object(vec![
                ("kind".to_string(), Value::Str("cache".to_string())),
                ("name".to_string(), Value::Str(name)),
            ]))
        }
        _ => Err(RuntimeError::native(format!(
            "no method `{method}` on <type cache>"
        ))),
    }
}

fn call_net_method(ns: &str, method: &str, args: &[Value]) -> Result<Value, RuntimeError> {
    match (ns, method) {
        ("net", "tcp" | "udp") => Ok(Value::Object(vec![
            ("kind".to_string(), Value::Str(format!("net.{method}"))),
            (
                "port".to_string(),
                named_arg(args, "port").unwrap_or(Value::Int(0)),
            ),
            (
                "path".to_string(),
                args.first().cloned().unwrap_or(Value::Void),
            ),
        ])),
        ("net.tun", "create") => {
            let name = named_string_arg(args, "name").unwrap_or_else(|| "tun0".to_string());
            let ipv4 = named_string_arg(args, "ipv4").unwrap_or_default();
            Ok(Value::Object(vec![
                ("kind".to_string(), Value::Str("net.tun".to_string())),
                ("name".to_string(), Value::Str(name)),
                ("ipv4".to_string(), Value::Str(ipv4)),
                ("status".to_string(), Value::Str("open".to_string())),
            ]))
        }
        ("net.tun", "write") => {
            let packet = args.get(1).cloned().unwrap_or(Value::Void);
            Ok(Value::Object(vec![
                ("status".to_string(), Value::Str("written".to_string())),
                ("bytes".to_string(), Value::Int(storage_value_size(&packet))),
            ]))
        }
        ("net.tun", "read") => Ok(Value::Object(vec![
            ("data".to_string(), Value::Str(String::new())),
            ("bytes".to_string(), Value::Int(0)),
        ])),
        _ => Err(RuntimeError::native(format!(
            "no method `{method}` on <type {ns}>"
        ))),
    }
}

fn call_plugin_method(ns: &str, method: &str, args: &[Value]) -> Result<Value, RuntimeError> {
    match (ns, method) {
        ("plugin", "load") => {
            let path = string_arg(args, 0, "`@plugin.load` expects a path")?;
            Ok(Value::Object(vec![
                ("kind".to_string(), Value::Str("plugin".to_string())),
                ("path".to_string(), Value::Str(path)),
                ("status".to_string(), Value::Str("loaded".to_string())),
                (
                    "activate".to_string(),
                    Value::Builtin("plugin.activate".to_string()),
                ),
            ]))
        }
        ("plugin", "discover") => {
            let root = string_arg(args, 0, "`@plugin.discover` expects a root path")?;
            Ok(Value::Array(vec![Value::Object(vec![
                (
                    "kind".to_string(),
                    Value::Str("plugin.candidate".to_string()),
                ),
                (
                    "path".to_string(),
                    Value::Str(format!("{root}/plugin.wasm")),
                ),
            ])]))
        }
        ("plugin.host", "register") | ("plugin", "host") => Ok(Value::Object(vec![
            ("kind".to_string(), Value::Str("plugin.host".to_string())),
            (
                "name".to_string(),
                args.first().cloned().unwrap_or(Value::Void),
            ),
        ])),
        _ => Err(RuntimeError::native(format!(
            "no method `{method}` on <type {ns}>"
        ))),
    }
}

fn call_gpu_method(method: &str, args: &[Value]) -> Result<Value, RuntimeError> {
    match method {
        "compute" => Ok(Value::Object(vec![
            ("kind".to_string(), Value::Str("compute".to_string())),
            (
                "file".to_string(),
                named_arg(args, "file").unwrap_or(Value::Void),
            ),
            (
                "workgroup".to_string(),
                named_arg(args, "workgroup").unwrap_or(Value::Void),
            ),
        ])),
        "context" => Ok(Value::Object(vec![
            ("kind".to_string(), Value::Str("gpu.context".to_string())),
            (
                "target".to_string(),
                args.first().cloned().unwrap_or(Value::Void),
            ),
        ])),
        "render" => Ok(Value::Object(vec![
            ("kind".to_string(), Value::Str("render".to_string())),
            (
                "commands".to_string(),
                args.first()
                    .cloned()
                    .unwrap_or_else(|| Value::Array(Vec::new())),
            ),
        ])),
        "textureFrom" => Ok(Value::Object(vec![
            ("kind".to_string(), Value::Str("texture".to_string())),
            (
                "source".to_string(),
                args.first().cloned().unwrap_or(Value::Void),
            ),
        ])),
        _ => Err(RuntimeError::native(format!(
            "no method `{method}` on <type gpu>"
        ))),
    }
}

fn gpu_render_raw(args: &[HirExpr]) -> Value {
    let commands = args
        .iter()
        .find_map(|arg| match &arg.kind {
            HirExprKind::Block(block) => Some(len_to_i64(block.stmts.len())),
            _ => None,
        })
        .unwrap_or(0);
    Value::Object(vec![
        ("kind".to_string(), Value::Str("render".to_string())),
        ("commands".to_string(), Value::Int(commands)),
    ])
}

fn call_observability_method(method: &str, args: &[Value]) -> Result<Value, RuntimeError> {
    match method {
        "configure" => {
            let config = args
                .first()
                .cloned()
                .unwrap_or_else(|| Value::Object(Vec::new()));
            let service = match &config {
                Value::Object(fields) => object_field(fields, "service")
                    .cloned()
                    .unwrap_or_else(|| Value::Str("orv".to_string())),
                _ => Value::Str("orv".to_string()),
            };
            Ok(Value::Object(vec![
                ("kind".to_string(), Value::Str("observability".to_string())),
                ("service".to_string(), service),
                ("config".to_string(), config),
            ]))
        }
        _ => Err(RuntimeError::native(format!(
            "no method `{method}` on <type observability>"
        ))),
    }
}

fn call_ffi_method(method: &str, args: &[Value]) -> Result<Value, RuntimeError> {
    match method {
        "library" | "load" => Ok(Value::Object(vec![
            ("kind".to_string(), Value::Str("ffi".to_string())),
            (
                "name".to_string(),
                args.first().cloned().unwrap_or(Value::Void),
            ),
        ])),
        _ => Err(RuntimeError::native(format!(
            "no method `{method}` on <type ffi>"
        ))),
    }
}

fn call_audit_method(method: &str, args: &[Value]) -> Result<Value, RuntimeError> {
    match method {
        "log" => {
            let name = string_arg(args, 0, "`audit.log` expects an event name")?;
            let fields = args
                .get(1)
                .cloned()
                .unwrap_or_else(|| Value::Object(Vec::new()));
            Ok(Value::Object(vec![
                ("kind".to_string(), Value::Str("audit.event".to_string())),
                ("name".to_string(), Value::Str(name)),
                ("fields".to_string(), fields),
            ]))
        }
        _ => Err(RuntimeError::native(format!(
            "no method `{method}` on <type audit>"
        ))),
    }
}

fn call_hash_method(method: &str, args: &[Value]) -> Result<Value, RuntimeError> {
    match method {
        "sha256" => {
            let input = string_arg(args, 0, "`hash.sha256` expects a string")?;
            Ok(Value::Str(sha256_hex(input.as_bytes())))
        }
        "password" => {
            let password = string_arg(args, 0, "`hash.password` expects a password string")?;
            let salt = SaltString::generate(&mut OsRng);
            let encoded = Argon2::default()
                .hash_password(password.as_bytes(), &salt)
                .map_err(|err| RuntimeError::native(format!("password hash failed: {err}")))?
                .to_string();
            Ok(Value::Str(encoded))
        }
        "verify" => {
            let password = string_arg(args, 0, "`hash.verify` expects (password, hash)")?;
            let expected = string_arg(args, 1, "`hash.verify` expects (password, hash)")?;
            let parsed = PasswordHash::new(&expected)
                .map_err(|err| RuntimeError::native(format!("invalid password hash: {err}")))?;
            Ok(Value::Bool(
                Argon2::default()
                    .verify_password(password.as_bytes(), &parsed)
                    .is_ok(),
            ))
        }
        _ => Err(RuntimeError::native(format!(
            "no method `{method}` on <type hash>"
        ))),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(bytes);
    hex_encode(&digest)
}

fn regex_contains(haystack: &str, pattern: &str, flags: &str) -> Result<bool, RuntimeError> {
    let mut builder = regex::RegexBuilder::new(pattern);
    for flag in flags.chars() {
        match flag {
            'g' => {}
            'i' => {
                builder.case_insensitive(true);
            }
            'm' => {
                builder.multi_line(true);
            }
            other => {
                return Err(RuntimeError::native(format!(
                    "unsupported regex flag `{other}`"
                )));
            }
        }
    }
    let regex = builder
        .build()
        .map_err(|e| RuntimeError::native(format!("invalid regex literal: {e}")))?;
    Ok(regex.is_match(haystack))
}

fn eval_reference_domain(name: &str, args: &[HirExpr]) -> Option<Value> {
    match name {
        "storage" if args.is_empty() => Some(Value::TypeName("storage".to_string())),
        "job" if args.is_empty() => Some(Value::TypeName("job".to_string())),
        "job" => {
            let job_name = args
                .first()
                .and_then(string_literal_from_expr)
                .unwrap_or_else(|| "job".to_string());
            if args
                .iter()
                .any(|arg| matches!(arg.kind, HirExprKind::Block(_)))
            {
                Some(Value::Object(vec![
                    ("name".to_string(), Value::Str(job_name)),
                    ("status".to_string(), Value::Str("registered".to_string())),
                ]))
            } else {
                Some(Value::TypeName(format!("job.{job_name}")))
            }
        }
        "sync" | "mail" | "media" | "push" | "payment" | "shipping" | "offline" | "cache"
        | "net" | "plugin" | "gpu" | "observability" | "ffi" | "hash"
            if args.is_empty() =>
        {
            Some(Value::TypeName(name.to_string()))
        }
        "upload" if args.is_empty() => Some(Value::Object(vec![
            ("id".to_string(), Value::Str("upload-1".to_string())),
            ("size".to_string(), Value::Int(0)),
            (
                "path".to_string(),
                Value::Str("uploads/upload-1".to_string()),
            ),
        ])),
        "message" if args.is_empty() => Some(Value::Object(vec![
            ("from".to_string(), Value::Str(String::new())),
            ("to".to_string(), Value::Str(String::new())),
            ("subject".to_string(), Value::Str(String::new())),
            ("body".to_string(), Value::Str(String::new())),
        ])),
        "chunk" if args.is_empty() => Some(Value::Object(vec![
            ("index".to_string(), Value::Int(0)),
            ("data".to_string(), Value::Str(String::new())),
        ])),
        "socket" if args.is_empty() => Some(Value::Object(vec![
            ("id".to_string(), Value::Str("socket-1".to_string())),
            ("room".to_string(), Value::Str(String::new())),
            (
                "join".to_string(),
                Value::Builtin("socket.join".to_string()),
            ),
            (
                "leave".to_string(),
                Value::Builtin("socket.leave".to_string()),
            ),
        ])),
        "packet" | "data" if args.is_empty() => Some(Value::Object(vec![
            ("type".to_string(), Value::Str(String::new())),
            ("room".to_string(), Value::Str(String::new())),
            ("text".to_string(), Value::Str(String::new())),
            ("target".to_string(), Value::Str(String::new())),
        ])),
        "peer" if args.is_empty() => Some(Value::Object(vec![(
            "id".to_string(),
            Value::Str("peer-1".to_string()),
        )])),
        "session" if args.is_empty() => Some(Value::Object(vec![
            ("id".to_string(), Value::Str("session-1".to_string())),
            ("stream".to_string(), Value::Object(Vec::new())),
            ("datagram".to_string(), Value::Object(Vec::new())),
        ])),
        "ws" | "wt" | "webrtc" => {
            let path = args
                .first()
                .and_then(string_literal_from_expr)
                .unwrap_or_default();
            Some(Value::Object(vec![
                ("protocol".to_string(), Value::Str(name.to_string())),
                ("path".to_string(), Value::Str(path)),
            ]))
        }
        "on" | "emit" | "connect" | "disconnect" | "signal" | "send" | "stream" | "datagram"
        | "cron" | "design" | "unsafe" | "hint" | "index" | "shard" | "replica" | "partition" => {
            Some(Value::Void)
        }
        _ => None,
    }
}

fn is_reference_namespace(ns: &str) -> bool {
    let root = ns.split('.').next().unwrap_or(ns);
    matches!(
        root,
        "storage"
            | "job"
            | "sync"
            | "mail"
            | "media"
            | "push"
            | "payment"
            | "shipping"
            | "offline"
            | "cache"
            | "net"
            | "plugin"
            | "gpu"
            | "observability"
            | "ffi"
            | "audit"
            | "hash"
    )
}

fn is_reference_namespace_method(ns: &str, method: &str) -> bool {
    match ns {
        "storage" => matches!(
            method,
            "put" | "get" | "delete" | "putChunk" | "merge" | "signedUrl" | "stream"
        ),
        "sync" => matches!(method, "open" | "connect" | "buffer"),
        "mail" => method == "send",
        "mail.verify" => matches!(method, "dkim" | "spf" | "dmarc"),
        "media" => matches!(method, "camera" | "screen" | "pipeline" | "player"),
        "push" => matches!(method, "request" | "subscribe" | "send"),
        "payment" => matches!(method, "connect" | "capture"),
        "shipping" => matches!(method, "connect" | "book"),
        "offline" => method == "store",
        "cache" => method == "open",
        "net" => matches!(method, "tcp" | "udp"),
        "net.tun" => matches!(method, "create" | "write" | "read"),
        "plugin" => matches!(method, "load" | "host" | "discover"),
        "plugin.host" => method == "register",
        "gpu" => matches!(method, "compute" | "context" | "render" | "textureFrom"),
        "observability" => method == "configure",
        "ffi" => matches!(method, "library" | "load"),
        "audit" => method == "log",
        "hash" => matches!(method, "sha256" | "password" | "verify"),
        _ => ns.starts_with("job.") && method == "enqueue",
    }
}

fn field_value(t: Value, field: &str, missing_object_is_void: bool) -> Result<Value, RuntimeError> {
    match (&t, field) {
        (Value::Array(items), "length") => Ok(Value::Int(items.len() as i64)),
        (Value::Str(s), "length") => Ok(Value::Int(s.chars().count() as i64)),
        (Value::Array(_), "map" | "filter" | "reduce" | "push" | "concat" | "join") => {
            Ok(Value::BoundMethod {
                receiver: Box::new(t),
                method: field.to_string(),
            })
        }
        (Value::Str(_), "toLowerCase" | "toUpperCase" | "contains" | "replace") => {
            Ok(Value::BoundMethod {
                receiver: Box::new(t),
                method: field.to_string(),
            })
        }
        (
            Value::Str(_)
            | Value::Regex { .. }
            | Value::Array(_)
            | Value::Object(_)
            | Value::Int(_)
            | Value::Float(_)
            | Value::Bool(_)
            | Value::Void,
            "move" | "copy",
        ) => Ok(Value::BoundMethod {
            receiver: Box::new(t),
            method: field.to_string(),
        }),
        (
            Value::Db(_),
            "create" | "find" | "findAll" | "update" | "delete" | "upsert" | "search" | "count"
            | "sum" | "transaction" | "schema" | "connect" | "analyze" | "save" | "load" | "wal"
            | "checkpoint" | "savepoint" | "rollback",
        ) => Ok(Value::BoundMethod {
            receiver: Box::new(t),
            method: field.to_string(),
        }),
        (Value::TypeName(_), "from") => Ok(Value::BoundMethod {
            receiver: Box::new(t),
            method: field.to_string(),
        }),
        (Value::TypeName(_), "parse" | "safeParse" | "errors" | "is" | "validate") => {
            Ok(Value::BoundMethod {
                receiver: Box::new(t),
                method: field.to_string(),
            })
        }
        (Value::TypeName(ns), "read" | "write") if ns == "fs" => Ok(Value::BoundMethod {
            receiver: Box::new(t),
            method: field.to_string(),
        }),
        (Value::TypeName(ns), "run") if ns == "process" => Ok(Value::BoundMethod {
            receiver: Box::new(t),
            method: field.to_string(),
        }),
        (Value::TypeName(ns), "runDue" | "tick") if ns == "cron" => Ok(Value::BoundMethod {
            receiver: Box::new(t),
            method: field.to_string(),
        }),
        (Value::TypeName(ns), _) if is_reference_namespace_method(ns, field) => {
            Ok(Value::BoundMethod {
                receiver: Box::new(t),
                method: field.to_string(),
            })
        }
        (Value::TypeName(ns), _) if is_reference_namespace(ns) => {
            Ok(Value::TypeName(format!("{ns}.{field}")))
        }
        (Value::Object(fields), _) => {
            if let Some((_, value)) = fields.iter().find(|(k, _)| k == field) {
                return Ok(value.clone());
            }
            if object_kind(fields).is_some_and(|kind| {
                matches!(kind, "cache" | "offline.store")
                    && matches!(field, "put" | "get" | "delete")
            }) {
                return Ok(Value::BoundMethod {
                    receiver: Box::new(t),
                    method: field.to_string(),
                });
            }
            if object_kind(fields).is_some_and(|kind| {
                matches!(kind, "payment.adapter") && matches!(field, "capture" | "verifyWebhook")
                    || matches!(kind, "shipping.adapter") && field == "book"
            }) {
                return Ok(Value::BoundMethod {
                    receiver: Box::new(t),
                    method: field.to_string(),
                });
            }
            if object_kind(fields).is_some_and(|kind| {
                kind == "db.adapter"
                    && matches!(
                        field,
                        "create"
                            | "find"
                            | "findAll"
                            | "update"
                            | "delete"
                            | "upsert"
                            | "search"
                            | "count"
                            | "sum"
                            | "transaction"
                            | "schema"
                            | "analyze"
                    )
            }) {
                return Ok(Value::BoundMethod {
                    receiver: Box::new(t),
                    method: field.to_string(),
                });
            }
            if missing_object_is_void {
                Ok(Value::Void)
            } else {
                Err(RuntimeError::native(format!(
                    "no field `{field}` on object"
                )))
            }
        }
        _ => Err(RuntimeError::native(format!("no field `{field}` on {t}"))),
    }
}

fn object_kind(fields: &[(String, Value)]) -> Option<&str> {
    match object_field(fields, "kind") {
        Some(Value::Str(kind)) => Some(kind),
        _ => None,
    }
}

fn named_arg(args: &[Value], name: &str) -> Option<Value> {
    args.iter().find_map(|arg| match arg {
        Value::Object(fields) if fields.len() == 1 && fields[0].0 == name => {
            Some(fields[0].1.clone())
        }
        _ => None,
    })
}

fn named_string_arg(args: &[Value], name: &str) -> Option<String> {
    named_arg(args, name).and_then(|value| match value {
        Value::Str(s) => Some(s),
        _ => None,
    })
}

fn payload_field(args: &[Value], name: &str) -> Option<Value> {
    named_arg(args, name).or_else(|| {
        args.first().and_then(|value| match value {
            Value::Object(fields) => object_field(fields, name).cloned(),
            _ => None,
        })
    })
}

fn string_arg(args: &[Value], index: usize, message: &str) -> Result<String, RuntimeError> {
    match args.get(index) {
        Some(Value::Str(value)) => Ok(value.clone()),
        _ => Err(RuntimeError::native(message)),
    }
}

fn int_arg(args: &[Value], index: usize, message: &str) -> Result<i64, RuntimeError> {
    match args.get(index) {
        Some(Value::Int(value)) => Ok(*value),
        _ => Err(RuntimeError::native(message)),
    }
}

fn storage_value_size(value: &Value) -> i64 {
    match value {
        Value::Str(s) => len_to_i64(s.chars().count()),
        Value::Array(items) | Value::Tuple(items) => len_to_i64(items.len()),
        Value::Void => 0,
        _ => len_to_i64(value_to_display(value).chars().count()),
    }
}

fn storage_file_record(path: &str, content: Value) -> Value {
    Value::Object(vec![
        ("path".to_string(), Value::Str(path.to_string())),
        ("size".to_string(), Value::Int(storage_value_size(&content))),
        ("content".to_string(), content),
    ])
}

fn len_to_i64(len: usize) -> i64 {
    i64::try_from(len).unwrap_or(i64::MAX)
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
        "int"
            | "uint"
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
            | "navigate"
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
            let x = value_to_f64(&v)
                .ok_or_else(|| RuntimeError::native(format!("{name}: expected number, got {v}")))?;
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
            let base = it
                .next()
                .ok_or_else(|| RuntimeError::native("pow expects 2 arguments"))?;
            let exp = it
                .next()
                .ok_or_else(|| RuntimeError::native("pow expects 2 arguments"))?;
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
        "sleep" | "socket.join" | "socket.leave" => Ok(Value::Void),
        "navigate" => {
            let path = string_arg(&args, 0, "navigate expects a path")?;
            Ok(Value::Object(vec![
                ("path".to_string(), Value::Str(path)),
                ("status".to_string(), Value::Str("navigated".to_string())),
            ]))
        }
        "plugin.activate" => Ok(Value::Object(vec![(
            "status".to_string(),
            Value::Str("activated".to_string()),
        )])),
        other => Err(RuntimeError::native(format!("unknown builtin `{other}`"))),
    }
}

fn type_of(v: &Value) -> &'static str {
    match v {
        Value::Int(_) => "int",
        Value::Float(_) => "float",
        Value::Str(_) => "string",
        Value::Regex { .. } => "regex",
        Value::Bool(_) => "bool",
        Value::Void => "void",
        Value::Function(_) | Value::Lambda(_) | Value::BoundMethod { .. } | Value::Builtin(_) => {
            "function"
        }
        Value::Array(_) => "array",
        Value::Tuple(_) => "tuple",
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
/// - `upsert(table, filter, data)` — 매칭 row 를 갱신하거나 filter+data 로 생성.
/// - `count(table, filter?)` — 매칭 row 수.
/// - `sum(table, filter, @field)` — 매칭 row 의 numeric field 합계.
/// - `search(table, filter?)` — 현재는 query search alias.
/// - `transaction(values...)` — 이미 평가된 body 값 중 마지막 값 반환.
/// - `save(path)` / `load(path)` — JSON snapshot 파일로 저장/복구.
/// - `wal(path)` — JSONL WAL 을 replay 하고 이후 mutation 을 append+fsync.
/// - `checkpoint()` — 현재 DB 상태를 WAL snapshot record 한 줄로 압축.
/// - `savepoint()` / `rollback(savepoint)` — in-memory savepoint capture/restore.
fn call_db_method(
    db: &DbHandle,
    method: &str,
    args: Vec<Value>,
    working_dir: Option<&Path>,
) -> Result<Value, RuntimeError> {
    let require_str = |v: &Value, what: &str| -> Result<String, RuntimeError> {
        match v {
            Value::Str(s) => Ok(s.clone()),
            other => Err(RuntimeError::native(format!(
                "`db.{method}` expects {what} to be string, got {other}"
            ))),
        }
    };
    match method {
        "create" => {
            if args.len() < 2 {
                return Err(RuntimeError::native("`db.create` expects (table, data)"));
            }
            let table = require_str(&args[0], "table name")?;
            let parsed = parse_db_runtime_args(method, &args[1..])?;
            let data = parsed.data.unwrap_or_default();
            db.borrow_mut()
                .create_logged(&table, data)
                .map_err(|e| RuntimeError::native(format!("db.create failed: {e}")))
        }
        "find" => {
            if args.is_empty() {
                return Err(RuntimeError::native("`db.find` expects (table[, query])"));
            }
            let table = require_str(&args[0], "table name")?;
            let parsed = parse_db_runtime_args(method, &args[1..])?;
            let db = db.borrow();
            if db_find_returns_many(&parsed.query) {
                Ok(Value::Array(db.find_query(&table, &parsed.query)))
            } else {
                Ok(db.find_one_query(&table, &parsed.query))
            }
        }
        "findAll" => {
            if args.is_empty() {
                return Err(RuntimeError::native(
                    "`db.findAll` expects (table[, filter])",
                ));
            }
            let table = require_str(&args[0], "table name")?;
            let parsed = parse_db_runtime_args(method, &args[1..])?;
            Ok(Value::Array(db.borrow().find_query(&table, &parsed.query)))
        }
        "update" => {
            if args.len() < 2 {
                return Err(RuntimeError::native(
                    "`db.update` expects (table, filter, data)",
                ));
            }
            let table = require_str(&args[0], "table name")?;
            let parsed = parse_db_runtime_args(method, &args[1..])?;
            db.borrow_mut()
                .update_logged(
                    &table,
                    &parsed.query,
                    &parsed.data.unwrap_or_default(),
                    &parsed.inc,
                )
                .map(Value::Int)
                .map_err(|e| RuntimeError::native(format!("db.update failed: {e}")))
        }
        "delete" => {
            if args.is_empty() {
                return Err(RuntimeError::native("`db.delete` expects (table, filter)"));
            }
            let table = require_str(&args[0], "table name")?;
            let parsed = parse_db_runtime_args(method, &args[1..])?;
            db.borrow_mut()
                .delete_logged(&table, &parsed.query)
                .map(Value::Int)
                .map_err(|e| RuntimeError::native(format!("db.delete failed: {e}")))
        }
        "upsert" => {
            if args.len() < 2 {
                return Err(RuntimeError::native(
                    "`db.upsert` expects (table, filter, data)",
                ));
            }
            let table = require_str(&args[0], "table name")?;
            let parsed = parse_db_runtime_args(method, &args[1..])?;
            let data = parsed.data.unwrap_or_default();
            let mut db = db.borrow_mut();
            if matches!(db.find_one_query(&table, &parsed.query), Value::Void) {
                db.create_logged(
                    &table,
                    merge_db_objects(&query_equality_fields(&parsed.query), &data),
                )
                .map_err(|e| RuntimeError::native(format!("db.upsert failed: {e}")))
            } else {
                db.update_logged(&table, &parsed.query, &data, &parsed.inc)
                    .map_err(|e| RuntimeError::native(format!("db.upsert failed: {e}")))?;
                Ok(db.find_one_query(&table, &parsed.query))
            }
        }
        "search" => {
            if args.is_empty() {
                return Err(RuntimeError::native(
                    "`db.search` expects (table[, filter])",
                ));
            }
            let table = require_str(&args[0], "table name")?;
            let parsed = parse_db_runtime_args(method, &args[1..])?;
            Ok(Value::Array(db.borrow().find_query(&table, &parsed.query)))
        }
        "count" => {
            if args.is_empty() {
                return Err(RuntimeError::native("`db.count` expects (table[, filter])"));
            }
            let table = require_str(&args[0], "table name")?;
            let parsed = parse_db_runtime_args(method, &args[1..])?;
            Ok(Value::Int(db.borrow().count_query(&table, &parsed.query)))
        }
        "sum" => {
            if args.is_empty() {
                return Err(RuntimeError::native(
                    "`db.sum` expects (table, query with @field)",
                ));
            }
            let table = require_str(&args[0], "table name")?;
            let parsed = parse_db_runtime_args(method, &args[1..])?;
            let [field] = parsed.query.fields.as_slice() else {
                return Err(RuntimeError::native("`db.sum` expects exactly one @field"));
            };
            Ok(db.borrow().sum_query(&table, &parsed.query, field))
        }
        "transaction" => Ok(args.last().cloned().unwrap_or(Value::Void)),
        "schema" => Ok(Value::Void),
        "analyze" => Ok(Value::Void),
        "connect" => {
            if let Some(url) = args.first() {
                let url = require_str(url, "db connect url")?;
                if url.starts_with("memory://") {
                    return Ok(Value::Db(db.clone()));
                }
                if let Some(path) = url.strip_prefix("file://") {
                    if path.is_empty() {
                        return Err(RuntimeError::native(
                            "`@db.connect` file adapter expects a WAL path",
                        ));
                    }
                    let path = resolve_runtime_path(path, working_dir);
                    let restored = InMemoryDb::load_wal(&path).map_err(|e| {
                        RuntimeError::native(format!("db.connect file adapter failed: {e}"))
                    })?;
                    return Ok(Value::Db(Rc::new(std::cell::RefCell::new(restored))));
                }
                if let Some(path) = url.strip_prefix("sqlite://") {
                    if path.is_empty() {
                        return Err(RuntimeError::native(
                            "`@db.connect` sqlite adapter expects a database path",
                        ));
                    }
                    let path = resolve_runtime_path(path, working_dir);
                    let restored = InMemoryDb::load_sqlite(&path).map_err(|e| {
                        RuntimeError::native(format!("db.connect sqlite adapter failed: {e}"))
                    })?;
                    return Ok(Value::Db(Rc::new(std::cell::RefCell::new(restored))));
                }
                if let Some(provider) = external_db_adapter_provider(&url) {
                    return Ok(Value::Object(vec![
                        ("kind".to_string(), Value::Str("db.adapter".to_string())),
                        ("provider".to_string(), Value::Str(provider.to_string())),
                        ("url".to_string(), Value::Str(url)),
                        (
                            "adapterStatus".to_string(),
                            Value::Str("unsupported_runtime".to_string()),
                        ),
                        (
                            "runtime".to_string(),
                            external_db_adapter_runtime_contract_value(),
                        ),
                    ]));
                }
                return Err(RuntimeError::native(format!(
                    "external db adapters are not implemented for `{url}`; supported schemes are memory://, file://, and sqlite://"
                )));
            }
            Ok(Value::Db(db.clone()))
        }
        "save" => {
            let path = args
                .first()
                .ok_or_else(|| RuntimeError::native("`db.save` expects path"))?;
            let path = require_str(path, "path")?;
            let path = resolve_runtime_path(&path, working_dir);
            db.borrow()
                .save_snapshot(&path)
                .map_err(|e| RuntimeError::native(format!("db.save failed: {e}")))?;
            Ok(Value::Void)
        }
        "load" => {
            let path = args
                .first()
                .ok_or_else(|| RuntimeError::native("`db.load` expects path"))?;
            let path = require_str(path, "path")?;
            let path = resolve_runtime_path(&path, working_dir);
            let restored = InMemoryDb::load_snapshot(&path)
                .map_err(|e| RuntimeError::native(format!("db.load failed: {e}")))?;
            *db.borrow_mut() = restored;
            Ok(Value::Void)
        }
        "wal" => {
            let path = args
                .first()
                .ok_or_else(|| RuntimeError::native("`db.wal` expects path"))?;
            let path = require_str(path, "path")?;
            let path = resolve_runtime_path(&path, working_dir);
            let restored = InMemoryDb::load_wal(&path)
                .map_err(|e| RuntimeError::native(format!("db.wal failed: {e}")))?;
            *db.borrow_mut() = restored;
            Ok(Value::Void)
        }
        "checkpoint" => {
            db.borrow_mut()
                .checkpoint_wal()
                .map_err(|e| RuntimeError::native(format!("db.checkpoint failed: {e}")))?;
            Ok(Value::Void)
        }
        "savepoint" => {
            let savepoint = db.borrow().savepoint();
            Ok(Value::Db(Rc::new(std::cell::RefCell::new(savepoint))))
        }
        "rollback" => {
            let savepoint = args
                .first()
                .ok_or_else(|| RuntimeError::native("`db.rollback` expects savepoint"))?;
            let Value::Db(savepoint) = savepoint else {
                return Err(RuntimeError::native(format!(
                    "`db.rollback` expects savepoint, got {savepoint}"
                )));
            };
            let savepoint = savepoint.borrow().clone();
            db.borrow_mut()
                .restore_savepoint(&savepoint)
                .map_err(|e| RuntimeError::native(format!("db.rollback failed: {e}")))?;
            Ok(Value::Void)
        }
        other => Err(RuntimeError::native(format!("unknown db method `{other}`"))),
    }
}

#[derive(Debug, Default)]
struct ParsedDbArgs {
    query: DbQuery,
    data: Option<Vec<(String, Value)>>,
    inc: Vec<(String, Value)>,
}

fn parse_db_runtime_args(method: &str, args: &[Value]) -> Result<ParsedDbArgs, RuntimeError> {
    let mut parsed = ParsedDbArgs::default();
    let mut positional_object_count = 0usize;

    for arg in args {
        let Value::Object(fields) = arg else {
            continue;
        };
        if let Some((sentinel, value)) = single_sentinel(fields) {
            match sentinel {
                "__order__" => parse_db_order(value, &mut parsed.query)?,
                "__skip__" => parsed.query.skip = Some(db_usize(value, "`@skip`")?),
                "__limit__" => parsed.query.limit = Some(db_usize(value, "`@limit`")?),
                "__field__" => parse_db_field(value, &mut parsed.query)?,
                "__rank__" => {}
                "__near__" => parse_db_near(value, &mut parsed.query)?,
                "__inc__" => parsed.inc.extend(db_object_fields(value, "`%inc`")?),
                _ => {}
            }
            continue;
        }

        match method {
            "create" => {
                parsed
                    .data
                    .get_or_insert_with(Vec::new)
                    .extend(fields.clone());
            }
            "update" | "upsert" => {
                if positional_object_count == 0 {
                    extend_query_filters(&mut parsed.query, fields);
                } else {
                    parsed
                        .data
                        .get_or_insert_with(Vec::new)
                        .extend(fields.clone());
                }
                positional_object_count += 1;
            }
            _ => extend_query_filters(&mut parsed.query, fields),
        }
    }

    Ok(parsed)
}

fn single_sentinel(fields: &[(String, Value)]) -> Option<(&str, &Value)> {
    let [(key, value)] = fields else {
        return None;
    };
    key.strip_prefix("__").map(|_| (key.as_str(), value))
}

fn parse_db_order(value: &Value, query: &mut DbQuery) -> Result<(), RuntimeError> {
    for (field, direction) in db_object_fields(value, "`@order`")? {
        let desc = match direction {
            Value::Str(s) if s.eq_ignore_ascii_case("desc") => true,
            Value::Str(s) if s.eq_ignore_ascii_case("asc") => false,
            Value::Bool(desc) => desc,
            other => {
                return Err(RuntimeError::native(format!(
                    "`@order` expects asc/desc direction, got {other}"
                )));
            }
        };
        query.order.push(DbOrder { field, desc });
    }
    Ok(())
}

fn parse_db_field(value: &Value, query: &mut DbQuery) -> Result<(), RuntimeError> {
    match value {
        Value::Str(field) => query.fields.push(field.clone()),
        Value::Array(items) => {
            for item in items {
                let Value::Str(field) = item else {
                    return Err(RuntimeError::native(format!(
                        "`@field` expects field name string, got {item}"
                    )));
                };
                query.fields.push(field.clone());
            }
        }
        other => {
            return Err(RuntimeError::native(format!(
                "`@field` expects field name, got {other}"
            )));
        }
    }
    Ok(())
}

fn parse_db_near(value: &Value, query: &mut DbQuery) -> Result<(), RuntimeError> {
    let fields = db_object_fields(value, "`@near`")?;
    let field = match object_field(&fields, "field") {
        Some(Value::Str(field)) => field.clone(),
        Some(other) => {
            return Err(RuntimeError::native(format!(
                "`@near` field expects string, got {other}"
            )));
        }
        None => return Err(RuntimeError::native("`@near` expects vector field")),
    };
    let vector = match object_field(&fields, "query") {
        Some(value) => db_vector(value, "`@near` query")?,
        None => return Err(RuntimeError::native("`@near` expects query vector")),
    };
    if let Some(k) = object_field(&fields, "k") {
        query.limit = Some(db_usize(k, "`@near k`")?);
    }
    query.near = Some(DbNear { field, vector });
    Ok(())
}

fn object_field<'a>(fields: &'a [(String, Value)], name: &str) -> Option<&'a Value> {
    fields
        .iter()
        .find(|(field, _)| field == name)
        .map(|(_, value)| value)
}

fn object_string_field(fields: &[(String, Value)], name: &str) -> Option<String> {
    match object_field(fields, name) {
        Some(Value::Str(value)) => Some(value.clone()),
        _ => None,
    }
}

fn external_db_adapter_provider(url: &str) -> Option<&str> {
    if url
        .strip_prefix("postgres://")
        .is_some_and(|tail| !tail.is_empty())
    {
        return Some("postgres");
    }
    if url
        .strip_prefix("mysql://")
        .is_some_and(|tail| !tail.is_empty())
    {
        return Some("mysql");
    }
    None
}

fn external_db_adapter_runtime_contract_value() -> Value {
    Value::Object(vec![
        (
            "status".to_string(),
            Value::Str("unsupported_runtime".to_string()),
        ),
        (
            "queryMethods".to_string(),
            Value::Array(
                ["create", "find", "update", "delete", "transaction"]
                    .into_iter()
                    .map(|method| Value::Str(method.to_string()))
                    .collect(),
            ),
        ),
    ])
}

fn db_vector(value: &Value, what: &str) -> Result<Vec<f64>, RuntimeError> {
    let Value::Array(items) = value else {
        return Err(RuntimeError::native(format!(
            "{what} expects numeric array"
        )));
    };
    items
        .iter()
        .map(|item| match item {
            Value::Int(n) => Ok(*n as f64),
            Value::Float(n) => Ok(*n),
            other => Err(RuntimeError::native(format!(
                "{what} expects numeric array, got {other}"
            ))),
        })
        .collect()
}

fn db_object_fields(value: &Value, what: &str) -> Result<Vec<(String, Value)>, RuntimeError> {
    match value {
        Value::Object(fields) => Ok(fields.clone()),
        other => Err(RuntimeError::native(format!(
            "{what} expects object, got {other}"
        ))),
    }
}

fn db_usize(value: &Value, what: &str) -> Result<usize, RuntimeError> {
    match value {
        Value::Int(n) => usize::try_from(*n)
            .map_err(|_| RuntimeError::native(format!("{what} expects non-negative int"))),
        other => Err(RuntimeError::native(format!(
            "{what} expects int, got {other}"
        ))),
    }
}

fn extend_query_filters(query: &mut DbQuery, fields: &[(String, Value)]) {
    for (raw_field, value) in fields {
        let (field, op) = db_filter_field(raw_field);
        query.filters.push(DbFilter {
            field,
            op,
            value: value.clone(),
        });
    }
}

fn db_filter_field(raw: &str) -> (String, DbFilterOp) {
    for (suffix, op) in [
        ("__gte", DbFilterOp::Ge),
        ("__lte", DbFilterOp::Le),
        ("__gt", DbFilterOp::Gt),
        ("__lt", DbFilterOp::Lt),
        ("__ne", DbFilterOp::Ne),
        ("__contains", DbFilterOp::Contains),
        ("__in", DbFilterOp::In),
        ("_contains", DbFilterOp::Contains),
    ] {
        if let Some(field) = raw.strip_suffix(suffix) {
            return (field.to_string(), op);
        }
    }
    (raw.to_string(), DbFilterOp::Eq)
}

fn db_find_returns_many(query: &DbQuery) -> bool {
    query.filters.is_empty()
        || query.skip.is_some()
        || query.limit.is_some()
        || !query.order.is_empty()
        || query
            .filters
            .iter()
            .any(|filter| filter.op != DbFilterOp::Eq)
}

fn query_equality_fields(query: &DbQuery) -> Vec<(String, Value)> {
    query
        .filters
        .iter()
        .filter(|filter| filter.op == DbFilterOp::Eq)
        .map(|filter| (filter.field.clone(), filter.value.clone()))
        .collect()
}

fn merge_db_objects(base: &[(String, Value)], overlay: &[(String, Value)]) -> Vec<(String, Value)> {
    let mut out = base.to_vec();
    for (key, value) in overlay {
        if let Some((_, slot)) = out.iter_mut().find(|(existing, _)| existing == key) {
            *slot = value.clone();
        } else {
            out.push((key.clone(), value.clone()));
        }
    }
    out
}

fn validation_error(path: &str, code: &str, message: &str, expected: &str, actual: Value) -> Value {
    Value::Object(vec![
        ("path".to_string(), Value::Str(path.to_string())),
        ("code".to_string(), Value::Str(code.to_string())),
        ("message".to_string(), Value::Str(message.to_string())),
        ("expected".to_string(), Value::Str(expected.to_string())),
        ("actual".to_string(), actual),
    ])
}

fn child_path(parent: &str, child: &str) -> String {
    if parent == "$" {
        format!("$.{child}")
    } else {
        format!("{parent}.{child}")
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
            | HirExprKind::AssignIndex { .. }
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
fn display_type_ref(ty: &HirTypeRef) -> String {
    let base = match &ty.kind {
        HirTypeRefKind::Named(name) => name.clone(),
        HirTypeRefKind::Nullable(inner) => format!("{}?", display_type_ref(inner)),
        HirTypeRefKind::Array(inner) => format!("{}[]", display_type_ref(inner)),
        HirTypeRefKind::Pattern(raw) => display_pattern(raw).to_string(),
        HirTypeRefKind::Union(items) => items
            .iter()
            .map(display_type_ref)
            .collect::<Vec<_>>()
            .join(" | "),
        HirTypeRefKind::InlineObject(fields) => {
            let fields = fields
                .iter()
                .map(|(name, ty)| format!("{name}: {}", display_type_ref(ty)))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{{{fields}}}")
        }
        HirTypeRefKind::Tuple(items) => {
            let items = items
                .iter()
                .map(display_type_ref)
                .collect::<Vec<_>>()
                .join(", ");
            format!("({items})")
        }
    };
    if ty.constraints.is_empty() {
        base
    } else {
        format!("{}({})", base, display_runtime_constraints(&ty.constraints))
    }
}

fn cast_pattern(value: Value, raw: &str) -> Result<Value, RuntimeError> {
    match raw {
        "true" => match value {
            Value::Bool(true) => Ok(Value::Bool(true)),
            other => Err(RuntimeError::native(format!(
                "pattern mismatch: expected `true`, got {other}"
            ))),
        },
        "false" => match value {
            Value::Bool(false) => Ok(Value::Bool(false)),
            other => Err(RuntimeError::native(format!(
                "pattern mismatch: expected `false`, got {other}"
            ))),
        },
        _ => {
            let Value::Str(s) = value else {
                return Err(RuntimeError::native(format!(
                    "pattern mismatch: expected `{}` string, got {value}",
                    display_pattern(raw)
                )));
            };
            if pattern_matches(raw, &s) {
                Ok(Value::Str(s))
            } else {
                Err(RuntimeError::native(format!(
                    "pattern mismatch: `{s}` does not match `{}`",
                    display_pattern(raw)
                )))
            }
        }
    }
}

fn apply_cast(value: Value, ty: &HirTypeRef) -> Result<Value, RuntimeError> {
    if let HirTypeRefKind::Union(items) = &ty.kind {
        let mut errors = Vec::new();
        for item in items {
            match apply_cast(value.clone(), item) {
                Ok(v) => return apply_value_constraints(v, &ty.constraints),
                Err(err) => errors.push(err.message),
            }
        }
        return Err(RuntimeError::native(format!(
            "cannot cast {value} to union `{}`: {}",
            display_type_ref(ty),
            errors.join("; ")
        )));
    }
    if let HirTypeRefKind::Array(inner) = &ty.kind {
        let Value::Array(items) = value else {
            return Err(RuntimeError::native(format!(
                "cannot cast {value} to {}",
                display_type_ref(ty)
            )));
        };
        let casted = items
            .into_iter()
            .map(|item| apply_cast(item, inner))
            .collect::<Result<Vec<_>, _>>()?;
        return apply_value_constraints(Value::Array(casted), &ty.constraints);
    }
    if let HirTypeRefKind::Nullable(inner) = &ty.kind {
        if matches!(value, Value::Void) {
            return Ok(Value::Void);
        }
        return apply_value_constraints(apply_cast(value, inner)?, &ty.constraints);
    }
    if let HirTypeRefKind::Pattern(raw) = &ty.kind {
        return apply_value_constraints(cast_pattern(value, raw)?, &ty.constraints);
    }
    if let HirTypeRefKind::InlineObject(type_fields) = &ty.kind {
        return apply_value_constraints(cast_inline_object(value, type_fields)?, &ty.constraints);
    }
    if let HirTypeRefKind::Tuple(items) = &ty.kind {
        return apply_value_constraints(cast_tuple(value, items)?, &ty.constraints);
    }
    let HirTypeRefKind::Named(name) = &ty.kind else {
        return apply_value_constraints(value, &ty.constraints);
    };
    let casted =
        match name.as_str() {
            "int" | "uint" | "byte" | "ubyte" | "short" | "ushort" | "long" | "ulong" => {
                match value {
                    Value::Int(i) => Ok(Value::Int(i)),
                    Value::Float(f) => Ok(Value::Int(f.trunc() as i64)),
                    Value::Bool(b) => Ok(Value::Int(i64::from(b))),
                    Value::Str(s) => s.trim().parse::<i64>().map(Value::Int).map_err(|_| {
                        RuntimeError::native(format!("cannot cast string `{s}` to int"))
                    }),
                    other => Err(RuntimeError::native(format!("cannot cast {other} to int"))),
                }
            }
            "float" | "double" => match value {
                Value::Float(f) => Ok(Value::Float(f)),
                #[allow(clippy::cast_precision_loss)]
                Value::Int(i) => Ok(Value::Float(i as f64)),
                Value::Bool(b) => Ok(Value::Float(if b { 1.0 } else { 0.0 })),
                Value::Str(s) => s.trim().parse::<f64>().map(Value::Float).map_err(|_| {
                    RuntimeError::native(format!("cannot cast string `{s}` to float"))
                }),
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
                other => Err(RuntimeError::native(format!("cannot cast {other} to bool"))),
            },
            "void" => Ok(Value::Void),
            "Email" => cast_pattern(value, "@Email"),
            "URL" => cast_pattern(value, "@URL"),
            "UUID" => cast_pattern(value, "@UUID"),
            "IPv4" => cast_pattern(value, "@IPv4"),
            "ISODate" => cast_pattern(value, "@ISODate"),
            // 사용자 정의 타입 (struct 이름 등) — MVP 는 pass-through.
            _ => Ok(value),
        }?;
    apply_value_constraints(casted, &ty.constraints)
}

fn cast_inline_object(
    value: Value,
    type_fields: &[(String, HirTypeRef)],
) -> Result<Value, RuntimeError> {
    let Value::Object(mut fields) = value else {
        return Err(RuntimeError::native(format!(
            "cannot cast {value} to object"
        )));
    };
    for (name, field_ty) in type_fields {
        let Some((_, slot)) = fields.iter_mut().find(|(field, _)| field == name) else {
            return Err(RuntimeError::native(format!(
                "object cast missing required field `{name}`"
            )));
        };
        *slot = apply_cast(slot.clone(), field_ty)?;
    }
    Ok(Value::Object(fields))
}

fn cast_tuple(value: Value, items: &[HirTypeRef]) -> Result<Value, RuntimeError> {
    let Value::Tuple(values) = value else {
        return Err(RuntimeError::native(format!(
            "cannot cast {value} to tuple"
        )));
    };
    if values.len() != items.len() {
        return Err(RuntimeError::native(format!(
            "tuple cast expects {} item(s), got {}",
            items.len(),
            values.len()
        )));
    }
    values
        .into_iter()
        .zip(items)
        .map(|(value, ty)| apply_cast(value, ty))
        .collect::<Result<Vec<_>, _>>()
        .map(Value::Tuple)
}

fn apply_value_constraints(
    mut value: Value,
    constraints: &[HirTypeConstraint],
) -> Result<Value, RuntimeError> {
    if constraints.is_empty() {
        return Ok(value);
    }
    if let Value::Str(s) = &mut value {
        for constraint in constraints {
            if let HirTypeConstraint::Flag(name) = constraint {
                match name.as_str() {
                    "trim" => *s = s.trim().to_string(),
                    "lower" => *s = s.to_ascii_lowercase(),
                    _ => {}
                }
            }
        }
    }
    for constraint in constraints {
        if !value_constraint_satisfied(&value, constraint) {
            return Err(RuntimeError::native(format!(
                "constraint mismatch: {value} does not satisfy `{}`",
                display_runtime_constraint(constraint)
            )));
        }
    }
    Ok(value)
}

fn value_constraint_satisfied(value: &Value, constraint: &HirTypeConstraint) -> bool {
    match constraint {
        HirTypeConstraint::Flag(name) if name == "unique" => value_array_unique(value),
        HirTypeConstraint::Flag(_) => true,
        HirTypeConstraint::ExactInt(n) => value_metric(value).is_none_or(|m| m == *n),
        HirTypeConstraint::Range { start, end, .. } => value_metric(value)
            .is_none_or(|m| start.is_none_or(|s| m >= s) && end.is_none_or(|e| m <= e)),
        HirTypeConstraint::KeyValue { key, value: cvalue } => match (key.as_str(), cvalue) {
            ("min", HirConstraintValue::Int(n)) => value_metric(value).is_none_or(|m| m >= *n),
            ("max", HirConstraintValue::Int(n)) => value_metric(value).is_none_or(|m| m <= *n),
            ("pattern", HirConstraintValue::String(pattern)) => match value {
                Value::Str(s) => simple_pattern_key_matches(pattern, s),
                _ => true,
            },
            _ => true,
        },
        HirTypeConstraint::Modulo { divisor, remainder } => match value {
            Value::Int(n) => *divisor != 0 && n % *divisor == *remainder,
            _ => true,
        },
        HirTypeConstraint::ContainsRegex { pattern, flags } => match value {
            Value::Str(s) => regex_contains(s, pattern, flags).is_ok_and(|matched| matched),
            _ => true,
        },
    }
}

fn value_metric(value: &Value) -> Option<i64> {
    match value {
        Value::Int(n) => Some(*n),
        Value::Str(s) => i64::try_from(s.chars().count()).ok(),
        Value::Array(items) => i64::try_from(items.len()).ok(),
        _ => None,
    }
}

fn value_array_unique(value: &Value) -> bool {
    let Value::Array(items) = value else {
        return true;
    };
    let mut seen = Vec::<String>::new();
    for item in items {
        let Some(key) = value_key(item) else {
            return true;
        };
        if seen.iter().any(|existing| existing == &key) {
            return false;
        }
        seen.push(key);
    }
    true
}

fn value_key(value: &Value) -> Option<String> {
    match value {
        Value::Int(n) => Some(format!("i:{n}")),
        Value::Float(n) => Some(format!("f:{n}")),
        Value::Str(s) => Some(format!("s:{s}")),
        Value::Bool(b) => Some(format!("b:{b}")),
        Value::Void => Some("void".to_string()),
        _ => None,
    }
}

fn simple_pattern_key_matches(pattern: &str, value: &str) -> bool {
    if pattern == "^[a-z0-9_]+$" {
        return !value.is_empty()
            && value
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    }
    true
}

fn display_runtime_constraints(constraints: &[HirTypeConstraint]) -> String {
    constraints
        .iter()
        .map(display_runtime_constraint)
        .collect::<Vec<_>>()
        .join(", ")
}

fn display_runtime_constraint(constraint: &HirTypeConstraint) -> String {
    match constraint {
        HirTypeConstraint::Flag(name) => name.clone(),
        HirTypeConstraint::ExactInt(n) => n.to_string(),
        HirTypeConstraint::Range { start, end, .. } => {
            let start = start.map_or_else(String::new, |n| n.to_string());
            let end = end.map_or_else(String::new, |n| n.to_string());
            format!("{start}..{end}")
        }
        HirTypeConstraint::KeyValue { key, value } => {
            format!("{key}={}", display_runtime_constraint_value(value))
        }
        HirTypeConstraint::Modulo { divisor, remainder } => {
            format!("where $ % {divisor} == {remainder}")
        }
        HirTypeConstraint::ContainsRegex { pattern, flags } => {
            format!("where $.contains(r\"{pattern}\"{flags})")
        }
    }
}

fn display_runtime_constraint_value(value: &HirConstraintValue) -> String {
    match value {
        HirConstraintValue::Int(n) => n.to_string(),
        HirConstraintValue::String(s) => format!("\"{s}\""),
        HirConstraintValue::Bool(b) => b.to_string(),
        HirConstraintValue::Ident(s) => s.clone(),
    }
}

fn display_pattern(raw: &str) -> &str {
    raw.strip_prefix('@').unwrap_or(raw)
}

fn pattern_matches(raw: &str, value: &str) -> bool {
    match raw {
        "@Email" => matches_email(value),
        "@URL" => matches_url(value),
        "@UUID" => matches_uuid(value),
        "@IPv4" => matches_ipv4(value),
        "@ISODate" => matches_iso_date(value),
        _ if raw.contains('{') && raw.contains('}') => template_matches(raw, value),
        _ => raw == value,
    }
}

fn template_matches(pattern: &str, value: &str) -> bool {
    let Some(open) = pattern.find('{') else {
        return pattern == value;
    };
    let prefix = &pattern[..open];
    if !value.starts_with(prefix) {
        return false;
    }
    let after_prefix = &value[prefix.len()..];
    let Some(close_rel) = pattern[open + 1..].find('}') else {
        return pattern == value;
    };
    let close = open + 1 + close_rel;
    let placeholder = &pattern[open + 1..close];
    let rest_pattern = &pattern[close + 1..];

    for end in split_positions(after_prefix) {
        let segment = &after_prefix[..end];
        if placeholder_matches(placeholder, segment)
            && template_matches(rest_pattern, &after_prefix[end..])
        {
            return true;
        }
    }
    false
}

fn split_positions(s: &str) -> Vec<usize> {
    let mut out: Vec<usize> = s.char_indices().map(|(i, _)| i).collect();
    out.push(s.len());
    out
}

fn placeholder_matches(name: &str, value: &str) -> bool {
    if let Some(n) = exact_placeholder_arg(name, "string") {
        return value.chars().count() == n;
    }
    if let Some(n) = exact_placeholder_arg(name, "int") {
        return value.len() == n && value.chars().all(|c| c.is_ascii_digit());
    }
    match name {
        "string" => !value.is_empty(),
        "int" => value.parse::<i64>().is_ok(),
        "uint" => value.parse::<u64>().is_ok(),
        "float" => value.parse::<f64>().is_ok(),
        "bool" => matches!(value, "true" | "false"),
        "IPByte" => value.parse::<u8>().is_ok(),
        _ => !value.is_empty(),
    }
}

fn exact_placeholder_arg(name: &str, prefix: &str) -> Option<usize> {
    let rest = name.strip_prefix(prefix)?;
    let inner = rest.strip_prefix('(')?.strip_suffix(')')?;
    inner.parse::<usize>().ok()
}

fn matches_email(value: &str) -> bool {
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    let Some((host, tld)) = domain.rsplit_once('.') else {
        return false;
    };
    !local.is_empty() && !host.is_empty() && !tld.is_empty()
}

fn matches_url(value: &str) -> bool {
    let Some((scheme, rest)) = value.split_once("://") else {
        return false;
    };
    !scheme.is_empty() && !rest.is_empty()
}

fn matches_uuid(value: &str) -> bool {
    let parts: Vec<&str> = value.split('-').collect();
    let lens = [8usize, 4, 4, 4, 12];
    parts.len() == lens.len()
        && parts
            .iter()
            .zip(lens)
            .all(|(part, len)| part.len() == len && part.chars().all(|c| c.is_ascii_hexdigit()))
}

fn matches_ipv4(value: &str) -> bool {
    let parts: Vec<&str> = value.split('.').collect();
    parts.len() == 4
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.parse::<u8>().is_ok()
                && (part == &"0" || !part.starts_with('0'))
        })
}

fn matches_iso_date(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 10 {
        return false;
    }
    if bytes[4] != b'-' || bytes[7] != b'-' {
        return false;
    }
    let year = &value[0..4];
    let month = &value[5..7];
    let day = &value[8..10];
    year.chars().all(|c| c.is_ascii_digit())
        && month.parse::<u8>().is_ok_and(|m| (1..=12).contains(&m))
        && day.parse::<u8>().is_ok_and(|d| (1..=31).contains(&d))
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
        Value::Regex { .. }
        | Value::Function(_)
        | Value::Lambda(_)
        | Value::BoundMethod { .. }
        | Value::Db(_)
        | Value::TypeName(_)
        | Value::Builtin(_) => true,
        Value::Array(a) => !a.is_empty(),
        Value::Tuple(t) => !t.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Float(x), Value::Float(y)) => (x - y).abs() < f64::EPSILON,
        (Value::Str(x), Value::Str(y)) => x == y,
        (
            Value::Regex {
                pattern: xp,
                flags: xf,
            },
            Value::Regex {
                pattern: yp,
                flags: yf,
            },
        ) => xp == yp && xf == yf,
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
        let hir = lower_src(src);
        let mut buf = Vec::new();
        run_with_writer(&hir, &mut buf)?;
        Ok(String::from_utf8(buf).unwrap())
    }

    fn lower_src(src: &str) -> orv_hir::HirProgram {
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
        lower(&pr.program, &resolved)
    }

    #[test]
    fn run_with_writer_accepts_runtime_options() {
        let hir = lower_src(r#"@out "with options""#);
        let mut buf = Vec::new();
        let options = RuntimeOptions {
            request_trace_path: Some(std::path::PathBuf::from("target/request-trace.json")),
            working_dir: None,
        };

        run_with_writer_with_options(&hir, &mut buf, options).expect("run with options");

        assert_eq!(String::from_utf8(buf).expect("utf-8"), "with options\n");
    }

    #[test]
    fn explicit_out_prints_string() {
        let out = run_str(r#"@out "Hello, Orv!""#).unwrap();
        assert_eq!(out, "Hello, Orv!\n");
    }

    #[test]
    fn debug_stepper_executes_one_visible_frame_at_a_time() {
        let hir = lower_src(
            r#"let first: int = 1
@out "second"
let third: int = 3
"#,
        );
        let mut stepper = DebugStepper::new(hir, Vec::new());

        let first = stepper.step().expect("first step").expect("first frame");
        assert!(first.locals.iter().any(|local| local.name == "first"));
        assert_eq!(stepper.writer(), b"");

        let second = stepper.step().expect("second step").expect("second frame");
        assert_eq!(second.output, "second\n");
        assert_eq!(stepper.writer(), b"second\n");

        let third = stepper.step().expect("third step").expect("third frame");
        assert!(third.locals.iter().any(|local| local.name == "third"));
        assert!(stepper.step().expect("done").is_none());
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
    fn primitive_type_name_can_be_shadowed_by_user_binding() {
        let out = run_str(
            r#"
            let string = "shadowed"
            @out string
            let double = 2.5
            @out double
            "#,
        )
        .unwrap();
        assert_eq!(out, "shadowed\n2.5\n");
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
    fn string_contains_accepts_regex_literal() {
        let out = run_str(
            r#"let password = "Passw0rd!"
@out password.contains(r"[A-Z]")
@out password.contains(r"[0-9]")
@out password.contains(r"[^a-zA-Z0-9]")
@out "password".contains(r"[A-Z]")"#,
        )
        .unwrap();
        assert_eq!(out, "true\ntrue\ntrue\nfalse\n");
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
    fn optional_field_on_void_returns_void_for_coalesce() {
        let out = run_str(
            r#"let user = void
@out user?.name ?? "guest""#,
        )
        .unwrap();
        assert_eq!(out, "guest\n");
    }

    #[test]
    fn optional_field_on_object_returns_field_value() {
        let out = run_str(
            r#"let user = { name: "Ada" }
@out user?.name ?? "guest""#,
        )
        .unwrap();
        assert_eq!(out, "Ada\n");
    }

    #[test]
    fn optional_method_call_on_void_returns_void() {
        let out = run_str(
            r#"let input = void
@out input?.focus() ?? "none""#,
        )
        .unwrap();
        assert_eq!(out, "none\n");
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

    #[test]
    fn html_renders_attributes_boolean_props_and_event_markers() {
        let out = run_str(
            r#"@out @html {
  @a href="/home" class="nav-link" "Home"
  @input type=email required disabled=false
  @button onClick={() -> @out "clicked"} "Click"
}"#,
        )
        .unwrap();
        assert_eq!(
            out,
            "<html><a href=\"/home\" class=\"nav-link\">Home</a><input type=\"email\" required><button onClick=\"handler\">Click</button></html>\n"
        );
    }

    #[test]
    fn html_renders_braced_attribute_expression() {
        let out = run_str(
            r#"let input: string = "hi"
@out @html { @input value={input} }"#,
        )
        .unwrap();
        assert_eq!(out, "<html><input value=\"hi\"></html>\n");
    }

    #[test]
    fn html_renders_block_attributes_before_children() {
        let out = run_str(
            r#"@out @html {
  @nav {
    class="main-nav"
    @a href="/" "Home"
  }
}"#,
        )
        .unwrap();
        assert_eq!(
            out,
            "<html><nav class=\"main-nav\"><a href=\"/\">Home</a></nav></html>\n"
        );
    }

    #[test]
    fn design_domain_preserves_token_sections_for_runtime_lookup() {
        let out = run_str(
            r##"@design {
  @colors { primary: "#0057ff" }
  @spacing { md: "12px" }
}
@out @design.colors.primary
@out @design.spacing.md"##,
        )
        .unwrap();
        assert_eq!(out, "#0057ff\n12px\n");
    }

    #[test]
    fn audit_log_returns_structured_event_handle() {
        let out = run_str(
            r#"let event = audit.log("payment.charged", { amount: 42 })
@out event.name
@out event.fields.amount"#,
        )
        .unwrap();
        assert_eq!(out, "payment.charged\n42\n");
    }

    #[test]
    fn audit_log_accepts_spec_parenless_form() {
        let out = run_str(
            r#"let event = audit.log "payment.charged" { amount: 42 }
@out event.name
@out event.fields.amount"#,
        )
        .unwrap();
        assert_eq!(out, "payment.charged\n42\n");
    }

    #[test]
    fn navigate_builtin_returns_navigation_record() {
        let out = run_str(
            r#"let nav = navigate("/dashboard")
@out nav.path
@out nav.status"#,
        )
        .unwrap();
        assert_eq!(out, "/dashboard\nnavigated\n");
    }

    #[test]
    fn fetch_domain_accepts_method_token_and_url() {
        let out = run_str(
            r#"let res = @fetch GET "https://api.example.test/data"
@out res.status
@out res.method
@out res.url"#,
        )
        .unwrap();
        assert_eq!(out, "200\nGET\nhttps://api.example.test/data\n");
    }

    // ── request-state 도메인 (@param/@query/@header/@body/@request) ──

    fn eval_handler_outcome_src(
        src: &str,
        ctx: RequestCtx,
    ) -> Result<(HandlerOutcome, String), RuntimeError> {
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
        let handler = if hir.items.len() == 1 {
            let orv_hir::HirStmt::Expr(expr) = &hir.items[0] else {
                panic!("expected expr stmt");
            };
            expr.clone()
        } else {
            orv_hir::HirExpr {
                kind: orv_hir::HirExprKind::Block(orv_hir::HirBlock {
                    stmts: hir.items.clone(),
                    span: hir.span,
                }),
                ty: orv_hir::Type::Unknown,
                span: hir.span,
            }
        };
        let mut buf = Vec::new();
        let outcome = run_handler_with_request(&handler, ctx, &mut buf)?;
        Ok((outcome, String::from_utf8(buf).unwrap()))
    }

    fn eval_handler_src(src: &str, ctx: RequestCtx) -> Result<String, RuntimeError> {
        eval_handler_outcome_src(src, ctx).map(|(_, output)| output)
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
    fn request_header_string_index_access() {
        let ctx = RequestCtx {
            headers: [("stripe-signature".into(), "t=1,v1=abc".into())]
                .into_iter()
                .collect(),
            ..Default::default()
        };
        let out = eval_handler_src(r#"@out @header["stripe-signature"]"#, ctx).unwrap();
        assert_eq!(out, "t=1,v1=abc\n");
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
    fn request_body_binding_validates_and_normalizes() {
        let ctx = RequestCtx {
            body: Value::Object(vec![
                ("email".into(), Value::Str(" USER@ORV.DEV ".into())),
                ("age".into(), Value::Str("15".into())),
            ]),
            ..Default::default()
        };
        let out = eval_handler_src(
            r#"struct SignupForm {
  email: string(trim, lower)
  age: int(min=13)
}
@body: SignupForm
@out @body.email
@out @body.age"#,
            ctx,
        )
        .unwrap();
        assert_eq!(out, "user@orv.dev\n15\n");
    }

    #[test]
    fn request_body_binding_returns_validation_response() {
        let ctx = RequestCtx {
            body: Value::Object(vec![
                ("email".into(), Value::Str("ok@orv.dev".into())),
                ("age".into(), Value::Str("12".into())),
            ]),
            ..Default::default()
        };
        let (outcome, out) = eval_handler_outcome_src(
            r#"struct SignupForm {
  email: string(trim, lower)
  age: int(min=13)
}
@body: SignupForm
@out "unreachable""#,
            ctx,
        )
        .unwrap();
        assert_eq!(out, "");
        let response = outcome.response.expect("validation response");
        assert_eq!(response.status, 400);
        let Value::Object(fields) = response.payload else {
            panic!("validation payload must be object");
        };
        assert!(matches!(
            object_field(&fields, "error"),
            Some(Value::Str(error)) if error == "validation_failed"
        ));
        assert!(matches!(
            object_field(&fields, "fields"),
            Some(Value::Array(errors)) if !errors.is_empty()
        ));
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
    fn request_meta_exposes_raw_body() {
        let ctx = RequestCtx {
            raw_body: r#"{"id":"evt_1"}"#.into(),
            ..Default::default()
        };
        let out = eval_handler_src(r#"@out @request.rawBody"#, ctx).unwrap();
        assert_eq!(out, "{\"id\":\"evt_1\"}\n");
    }

    #[test]
    fn request_session_domain_exposes_cookie_id() {
        let ctx = RequestCtx {
            headers: [("Cookie".into(), "theme=dark; orv_session=sess-42".into())]
                .into_iter()
                .collect(),
            ..Default::default()
        };
        let out = eval_handler_src(r#"@out @session.id"#, ctx).unwrap();
        assert_eq!(out, "sess-42\n");
    }

    #[test]
    fn session_required_allows_cookie() {
        let ctx = RequestCtx {
            headers: [("cookie".into(), "orv_session=abc_123".into())]
                .into_iter()
                .collect(),
            ..Default::default()
        };
        let out = eval_handler_src(
            r#"@session required
@out @session.id"#,
            ctx,
        )
        .unwrap();
        assert_eq!(out, "abc_123\n");
    }

    #[test]
    fn session_required_records_unauthorized_response() {
        let (outcome, out) = eval_handler_outcome_src(
            r#"@session required
@out "after""#,
            RequestCtx::default(),
        )
        .unwrap();
        assert_eq!(out, "");
        let response = outcome.response.expect("session response");
        assert_eq!(response.status, 401);
        let Value::Object(fields) = response.payload else {
            panic!("expected object payload");
        };
        assert!(fields.iter().any(|(name, value)| {
            name == "err" && matches!(value, Value::Str(err) if err == "session_required")
        }));
    }

    #[test]
    fn auth_required_role_allows_matching_session_role_cookie() {
        let ctx = RequestCtx {
            headers: [(
                "cookie".into(),
                "orv_session=admin_1; orv_session_role=admin".into(),
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        let out = eval_handler_src(
            r#"@Auth required role="admin"
@out @session.role"#,
            ctx,
        )
        .unwrap();
        assert_eq!(out, "admin\n");
    }

    #[test]
    fn auth_required_records_unauthorized_response() {
        let (outcome, out) = eval_handler_outcome_src(
            r#"@Auth required role="admin"
@out "after""#,
            RequestCtx::default(),
        )
        .unwrap();
        assert_eq!(out, "");
        let response = outcome.response.expect("auth response");
        assert_eq!(response.status, 401);
        let Value::Object(fields) = response.payload else {
            panic!("expected object payload");
        };
        assert!(fields.iter().any(|(name, value)| name == "err"
            && matches!(value, Value::Str(err) if err == "auth_required")));
    }

    #[test]
    fn auth_required_role_records_forbidden_response() {
        let ctx = RequestCtx {
            headers: [(
                "cookie".into(),
                "orv_session=member_1; orv_session_role=member".into(),
            )]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        let (outcome, out) = eval_handler_outcome_src(
            r#"@Auth required role="admin"
@out "after""#,
            ctx,
        )
        .unwrap();
        assert_eq!(out, "");
        let response = outcome.response.expect("auth response");
        assert_eq!(response.status, 403);
        let Value::Object(fields) = response.payload else {
            panic!("expected object payload");
        };
        assert!(fields.iter().any(|(name, value)| name == "err"
            && matches!(value, Value::Str(err) if err == "role_required")));
        assert!(fields.iter().any(|(name, value)| name == "requiredRole"
            && matches!(value, Value::Str(role) if role == "admin")));
    }

    #[test]
    fn csrf_domain_accepts_cookie_and_header_token() {
        let ctx = RequestCtx {
            headers: [
                (
                    "cookie".into(),
                    format!("{ORV_CSRF_COOKIE_NAME}={ORV_REFERENCE_CSRF_TOKEN}"),
                ),
                ("x-csrf-token".into(), ORV_REFERENCE_CSRF_TOKEN.into()),
            ]
            .into_iter()
            .collect(),
            ..Default::default()
        };
        let out = eval_handler_src(
            r#"@csrf
@out "after""#,
            ctx,
        )
        .unwrap();
        assert_eq!(out, "after\n");
    }

    #[test]
    fn csrf_domain_accepts_cookie_and_body_token() {
        let ctx = RequestCtx {
            headers: [(
                "cookie".into(),
                format!("{ORV_CSRF_COOKIE_NAME}={ORV_REFERENCE_CSRF_TOKEN}"),
            )]
            .into_iter()
            .collect(),
            body: Value::Object(vec![(
                "_csrf".to_string(),
                Value::Str(ORV_REFERENCE_CSRF_TOKEN.to_string()),
            )]),
            ..Default::default()
        };
        let out = eval_handler_src(
            r#"@csrf
@out "after""#,
            ctx,
        )
        .unwrap();
        assert_eq!(out, "after\n");
    }

    #[test]
    fn csrf_domain_records_forbidden_response() {
        let (outcome, out) = eval_handler_outcome_src(
            r#"@csrf
@out "after""#,
            RequestCtx::default(),
        )
        .unwrap();
        assert_eq!(out, "");
        let response = outcome.response.expect("csrf response");
        assert_eq!(response.status, 403);
        let Value::Object(fields) = response.payload else {
            panic!("expected object payload");
        };
        assert!(fields.iter().any(|(name, value)| {
            name == "err" && matches!(value, Value::Str(err) if err == "csrf_token_required")
        }));
    }

    #[test]
    fn request_meta_includes_client_ip() {
        let ctx = RequestCtx {
            ip: "127.0.0.1".into(),
            ..Default::default()
        };
        let out = eval_handler_src(r#"@out @request.ip"#, ctx).unwrap();
        assert_eq!(out, "127.0.0.1\n");
    }

    #[test]
    fn response_domain_exposes_current_status() {
        let ctx = RequestCtx::default();
        let out = eval_handler_src(r#"@out @response.status"#, ctx).unwrap();
        assert_eq!(out, "200\n");
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
    fn serve_html_value_records_raw_html_response() {
        let (_, resp) = run_handler(
            r#"{
                @serve @html { @body { @h1 "Home" } }
            }"#,
            RequestCtx::default(),
        );
        let resp = resp.expect("response recorded");
        assert_eq!(resp.status, 200);
        let raw = resp.raw_body.expect("html raw body");
        assert_eq!(raw.content_type, "text/html; charset=utf-8");
        assert_eq!(
            String::from_utf8(raw.bytes).unwrap(),
            "<html><body><h1>Home</h1></body></html>"
        );
    }

    #[test]
    fn serve_html_void_input_records_unclosed_form_control() {
        let (_, resp) = run_handler(
            r#"{
                @serve @html {
                    @body {
                        @form action="/members" method=post {
                            @input type=email name=email required
                            @button type=submit "Join"
                        }
                    }
                }
            }"#,
            RequestCtx::default(),
        );
        let resp = resp.expect("response recorded");
        let raw = resp.raw_body.expect("html raw body");
        assert_eq!(
            String::from_utf8(raw.bytes).unwrap(),
            "<html><body><form action=\"/members\" method=\"post\"><input type=\"email\" name=\"email\" required><button type=\"submit\">Join</button></form></body></html>"
        );
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

    #[test]
    fn hash_namespace_supports_sha256_password_and_verify() {
        let out = run_str(
            r#"let digest = hash.sha256("data")
let passwordHash = await hash.password("correct horse battery staple")
@out digest
@out passwordHash.contains("correct horse battery staple")
@out passwordHash.contains("$argon2")
@out await hash.verify("correct horse battery staple", passwordHash)
@out await hash.verify("wrong password", passwordHash)"#,
        )
        .unwrap();
        assert_eq!(
            out,
            "3a6eb0790f39ac87c94f3856b2dd2c5d110e6811602261a9a923d3bb23adc8b7\nfalse\ntrue\ntrue\nfalse\n"
        );
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
        assert_eq!(
            stdout, "",
            "handler body must not run after @respond in @before"
        );
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
        assert!(
            err.message.contains("missing required property"),
            "got: {}",
            err.message
        );
    }

    #[test]
    fn user_domain_unknown_property_errors() {
        // signature 에 없는 key 는 에러.
        let src = r#"
define P(a: string) -> { @out a }
@P a="ok" b="nope"
"#;
        let err = run_str(src).unwrap_err();
        assert!(
            err.message.contains("unknown property"),
            "got: {}",
            err.message
        );
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
            err.message
                .contains("got 1 positional arg(s) but declares no token slot"),
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
        let err = run_str(r#"let n: int = int.from("nope")"#).unwrap_err();
        assert!(err.message.contains("int.from"));
    }

    #[test]
    fn cast_to_type_alias_uses_aliased_runtime_type() {
        let out = run_str(
            r#"type Num = int
let n: Num = "42" as Num
@out n + 1"#,
        )
        .unwrap();
        assert_eq!(out, "43\n");
    }

    #[test]
    fn cast_to_pattern_alias_rejects_bad_runtime_string() {
        let err = run_str(
            r#"type Email = "{string}@{string}.{string}"
let email: Email = "not-an-email" as Email
@out email"#,
        )
        .unwrap_err();
        assert!(err.message.contains("pattern"));
    }

    #[test]
    fn cast_to_union_alias_tries_member_conversions() {
        let out = run_str(
            r#"type Id = int | string
let id: Id = "42" as Id
@out id + 1"#,
        )
        .unwrap();
        assert_eq!(out, "43\n");
    }

    #[test]
    fn db_upsert_updates_existing_row() {
        let out = run_str(
            r#"let first = @db.upsert User { @where email="a@orv.dev"; %data={ name: "Alice" } }
let second = @db.upsert User { @where email="a@orv.dev"; %data={ name: "Alicia" } }
let user = @db.find User { @where email="a@orv.dev" }
@out user.name"#,
        )
        .unwrap();
        assert_eq!(out, "Alicia\n");
    }

    #[test]
    fn db_search_returns_matching_rows() {
        let out = run_str(
            r#"let a = @db.create Product %data={ name: "Coffee", category: "drink" }
let b = @db.create Product %data={ name: "Tea", category: "drink" }
let c = @db.create Product %data={ name: "Cake", category: "food" }
let hits = @db.search Product { @where category="drink" }
@out hits.length"#,
        )
        .unwrap();
        assert_eq!(out, "2\n");
    }

    #[test]
    fn db_find_query_orders_skips_and_limits_comparison_matches() {
        let out = run_str(
            r#"let a = @db.create User %data={ name: "Ann", age: 20 }
let b = @db.create User %data={ name: "Bea", age: 30 }
let c = @db.create User %data={ name: "Cam", age: 40 }
let hits = @db.find User {
  @where age > 18
  @order age=desc
  @skip 1
  @limit 1
}
@out hits.length
@out hits[0].name"#,
        )
        .unwrap();
        assert_eq!(out, "1\nBea\n");
    }

    #[test]
    fn db_update_applies_increment_modifier() {
        let out = run_str(
            r#"let post = @db.create Post %data={ title: "Orv", likes: 1 }
let _ = @db.update Post {
  @where id=post.id
  %inc={ likes: 2 }
}
let updated = @db.find Post { @where id=post.id }
@out updated.likes"#,
        )
        .unwrap();
        assert_eq!(out, "3\n");
    }

    #[test]
    fn db_find_projects_requested_fields() {
        let out = run_str(
            r#"let user = @db.create User %data={ name: "Ann", age: 20, email: "ann@orv.dev" }
let hits = @db.find User {
  @where age >= 18
  @field name
}
@out hits[0].name
@out hits[0]"#,
        )
        .unwrap();
        assert_eq!(out, "Ann\n{ name: Ann }\n");
    }

    #[test]
    fn db_count_and_sum_respect_query_filters() {
        let out = run_str(
            r#"let a = @db.create Post %data={ authorId: 1, likes: 2 }
let b = @db.create Post %data={ authorId: 1, likes: 3 }
let c = @db.create Post %data={ authorId: 2, likes: 10 }
@out @db.count Post { @where authorId=1 }
@out @db.sum Post { @where authorId=1; @field likes }"#,
        )
        .unwrap();
        assert_eq!(out, "2\n5\n");
    }

    #[test]
    fn db_search_accepts_rank_directive_as_stable_noop() {
        let out = run_str(
            r#"let a = @db.create Post %data={ title: "Orv language", body: "runtime" }
let b = @db.create Post %data={ title: "Other", body: "misc" }
let hits = @db.search Post {
  @match title="Orv"
  @rank bm25
}
@out hits.length
@out hits[0].title"#,
        )
        .unwrap();
        assert_eq!(out, "1\nOrv language\n");
    }

    #[test]
    fn db_search_near_orders_by_vector_distance_and_uses_k() {
        let out = run_str(
            r#"let far = @db.create Doc %data={ title: "far", embedding: [10, 10] }
let near = @db.create Doc %data={ title: "near", embedding: [2, 1] }
let mid = @db.create Doc %data={ title: "mid", embedding: [4, 4] }
let query = [2, 2]
let hits = @db.search Doc {
  @near embedding=query k=2
}
@out hits.length
@out hits[0].title
@out hits[1].title"#,
        )
        .unwrap();
        assert_eq!(out, "2\nnear\nmid\n");
    }

    #[test]
    fn db_transaction_evaluates_body_and_returns_last_value() {
        let out = run_str(
            r#"let created = @db.transaction {
  @db.create User %data={ name: "Tx" }
}
@out created.name"#,
        )
        .unwrap();
        assert_eq!(out, "Tx\n");
    }

    #[test]
    fn db_transaction_accepts_hint_and_block_body() {
        let out = run_str(
            r#"let value = @db.transaction @hint isolation=serializable {
  let x = 41
  x + 1
}
@out value"#,
        )
        .unwrap();
        assert_eq!(out, "42\n");
    }

    #[test]
    fn db_schema_and_index_declarations_are_stable_noops() {
        let out = run_str(
            r#"@db.schema Post {
  @shard key=authorId count=16
  @replica count=2 strategy=async
  @partition by=createdAt interval=1mo
}
@index fulltext Post fields=[title, content] lang=multi
@out "ok""#,
        )
        .unwrap();
        assert_eq!(out, "ok\n");
    }

    #[test]
    fn db_connect_returns_handle_and_analyze_is_noop() {
        let out = run_str(
            r#"let external = @db.connect "memory://local"
@db.analyze()
let created = external.create("User", { name: "Ada" })
let found = external.find("User", { name: "Ada" })
@out found.name"#,
        )
        .unwrap();
        assert_eq!(out, "Ada\n");
    }

    #[test]
    fn db_connect_file_adapter_replays_and_persists_wal() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "orv-runtime-db-connect-file-{}-{unique}.jsonl",
            std::process::id()
        ));
        let first = format!(
            r#"let external = @db.connect "file://{}"
let created = external.create("User", {{ name: "Ada" }})"#,
            path.display()
        );
        let second = format!(
            r#"let external = @db.connect "file://{}"
let found = external.find("User", {{ name: "Ada" }})
@out found.name"#,
            path.display()
        );

        run_str(&first).unwrap();
        let out = run_str(&second).unwrap();

        assert_eq!(out, "Ada\n");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn db_connect_sqlite_adapter_replays_and_persists_rows() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "orv-runtime-db-connect-sqlite-{}-{unique}.sqlite",
            std::process::id()
        ));
        let first = format!(
            r#"let external = @db.connect "sqlite://{}"
let ada = external.create("User", {{ name: "Ada" }})
let grace = external.create("User", {{ name: "Grace" }})"#,
            path.display()
        );
        let second = format!(
            r#"let external = @db.connect "sqlite://{}"
external.update("User", {{ name: "Ada" }}, {{ name: "Ada Lovelace" }})
external.delete("User", {{ name: "Grace" }})"#,
            path.display()
        );
        let third = format!(
            r#"let external = @db.connect "sqlite://{}"
let found = external.find("User", {{ name: "Ada Lovelace" }})
let gone = external.find("User", {{ name: "Grace" }})
let all = external.findAll("User", {{}})
@out found.name
@out gone
@out all.length"#,
            path.display()
        );

        run_str(&first).unwrap();
        run_str(&second).unwrap();
        let out = run_str(&third).unwrap();

        assert_eq!(out, "Ada Lovelace\n\n1\n");
        assert!(path.is_file());
        let conn = rusqlite::Connection::open(&path).expect("open sqlite adapter db");
        let row_json: String = conn
            .query_row(
                "SELECT row_json FROM orv_rows WHERE table_name = 'User' AND row_id = 1",
                [],
                |row| row.get(0),
            )
            .expect("sqlite adapter row");
        assert!(row_json.contains(r#""name":"Ada Lovelace""#));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn db_connect_external_adapter_reports_status_and_rejects_queries() {
        let out = run_str(
            r#"let external = @db.connect "postgres://localhost/shop"
@out external.provider
@out external.adapterStatus
@out external.url
let err = external.analyze()
@out err.adapterStatus
@out external.runtime.status
@out external.runtime.queryMethods[0]
@out external.runtime.queryMethods.length"#,
        )
        .expect("external adapter status");
        assert_eq!(
            out,
            "postgres\nunsupported_runtime\npostgres://localhost/shop\nunsupported_runtime\nunsupported_runtime\ncreate\n5\n"
        );

        let err = run_str(
            r#"let external = @db.connect "mysql://localhost/shop"
external.create("User", { name: "Ada" })"#,
        )
        .expect_err("external adapter query must fail until implemented");

        assert!(err
            .to_string()
            .contains("external db adapter mysql is not implemented"));
    }

    #[test]
    fn db_save_and_load_roundtrip_snapshot_file() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "orv-runtime-db-{}-{unique}.json",
            std::process::id()
        ));
        let src = format!(
            r#"let created = @db.create User %data={{ name: "Ada" }}
@db.save "{}"
@db.delete User {{ @where id=created.id }}
@db.load "{}"
let found = @db.find User {{ @where name="Ada" }}
@out found.name"#,
            path.display(),
            path.display()
        );

        let out = run_str(&src).unwrap();

        assert_eq!(out, "Ada\n");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn db_wal_replays_mutations_between_runtime_runs() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "orv-runtime-db-wal-{}-{unique}.jsonl",
            std::process::id()
        ));
        let first = format!(
            r#"@db.wal "{}"
let created = @db.create User %data={{ name: "Ada" }}
@db.update User {{ @where id=created.id; %data={{ age: 37 }} }}"#,
            path.display()
        );
        let second = format!(
            r#"@db.wal "{}"
let found = @db.find User {{ @where name="Ada" }}
@out found.age"#,
            path.display()
        );

        run_str(&first).unwrap();
        let out = run_str(&second).unwrap();

        assert_eq!(out, "37\n");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn db_checkpoint_compacts_wal_and_preserves_row_ids() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "orv-runtime-db-checkpoint-{}-{unique}.jsonl",
            std::process::id()
        ));
        let first = format!(
            r#"@db.wal "{}"
let ada = @db.create User %data={{ name: "Ada" }}
let bea = @db.create User %data={{ name: "Bea" }}
@db.delete User {{ @where id=ada.id }}
@db.checkpoint()"#,
            path.display()
        );
        let second = format!(
            r#"@db.wal "{}"
let bea = @db.find User {{ @where name="Bea" }}
let cam = @db.create User %data={{ name: "Cam" }}
@out bea.id
@out cam.id"#,
            path.display()
        );

        run_str(&first).unwrap();
        let wal = std::fs::read_to_string(&path).expect("read checkpointed wal");
        assert_eq!(wal.lines().count(), 1);

        let out = run_str(&second).unwrap();

        assert_eq!(out, "2\n3\n");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn db_transaction_rolls_back_on_error() {
        let out = run_str(
            r#"let account = @db.create Account %data={ balance: 10 }
try {
  @db.transaction {
    @db.update Account { @where id=account.id; %data={ balance: 0 } }
    throw "boom"
  }
} catch err {
  @out "caught"
}
let found = @db.find Account { @where id=account.id }
@out found.balance"#,
        )
        .unwrap();
        assert_eq!(out, "caught\n10\n");
    }

    #[test]
    fn db_savepoint_rolls_back_in_memory_state() {
        let out = run_str(
            r"let account = @db.create Account %data={ balance: 10 }
let point = @db.savepoint()
@db.update Account { @where id=account.id; %data={ balance: 0 } }
@db.rollback(point)
let found = @db.find Account { @where id=account.id }
@out found.balance",
        )
        .unwrap();
        assert_eq!(out, "10\n");
    }

    #[test]
    fn db_wal_savepoint_rollback_survives_replay() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "orv-runtime-db-wal-savepoint-{}-{unique}.jsonl",
            std::process::id()
        ));
        let first = format!(
            r#"@db.wal "{}"
let account = @db.create Account %data={{ balance: 10 }}
let point = @db.savepoint()
@db.update Account {{ @where id=account.id; %data={{ balance: 0 }} }}
@db.rollback(point)"#,
            path.display()
        );
        let second = format!(
            r#"@db.wal "{}"
let account = @db.find Account {{ @where id=1 }}
@out account.balance"#,
            path.display()
        );

        run_str(&first).unwrap();
        let second_out = run_str(&second).unwrap();

        assert_eq!(second_out, "10\n");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn db_wal_transaction_rollback_survives_replay() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "orv-runtime-db-wal-rollback-{}-{unique}.jsonl",
            std::process::id()
        ));
        let first = format!(
            r#"@db.wal "{}"
let account = @db.create Account %data={{ balance: 10 }}
try {{
  @db.transaction {{
    @db.update Account {{ @where id=account.id; %data={{ balance: 0 }} }}
    throw "boom"
  }}
}} catch err {{
  @out err
}}"#,
            path.display()
        );
        let second = format!(
            r#"@db.wal "{}"
let account = @db.find Account {{ @where id=1 }}
@out account.balance"#,
            path.display()
        );

        let first_out = run_str(&first).unwrap();
        let second_out = run_str(&second).unwrap();

        assert_eq!(first_out, "boom\n");
        assert_eq!(second_out, "10\n");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn cast_string_constraints_transform_and_validate() {
        let out = run_str(
            r#"let name = "  ALICE  " as string(trim, lower, min=3, max=10)
@out name"#,
        )
        .unwrap();
        assert_eq!(out, "alice\n");
    }

    #[test]
    fn cast_inline_object_constraints_transform_fields() {
        let out = run_str(
            r#"let form = { email: " USER@ORV.DEV " } as { email: string(trim, lower) }
@out form.email"#,
        )
        .unwrap();
        assert_eq!(out, "user@orv.dev\n");
    }

    #[test]
    fn cast_array_unique_constraint_rejects_duplicates() {
        let err = run_str(r#"let tags = ["a", "a"] as string[](unique)"#).unwrap_err();
        assert!(err.message.contains("unique"));
    }

    #[test]
    fn cast_alias_where_modulo_rejects_invalid_value() {
        let err = run_str(
            r"type EvenInt = int where $ % 2 == 0
let n = 3 as EvenInt
@out n",
        )
        .unwrap_err();
        assert!(err.message.contains("where") || err.message.contains("%"));
    }

    #[test]
    fn cast_alias_where_modulo_accepts_valid_value() {
        let out = run_str(
            r"type EvenInt = int where $ % 2 == 0
let n = 4 as EvenInt
@out n",
        )
        .unwrap();
        assert_eq!(out, "4\n");
    }

    #[test]
    fn cast_alias_where_contains_regex_validates_value() {
        let out = run_str(
            r#"type StrongPassword = string(min=8) where
  $.contains(r"[A-Z]") &&
  $.contains(r"[0-9]") &&
  $.contains(r"[^a-zA-Z0-9]")
let password = "Password1!" as StrongPassword
@out password"#,
        )
        .unwrap();
        assert_eq!(out, "Password1!\n");

        let err = run_str(
            r#"type StrongPassword = string(min=8) where
  $.contains(r"[A-Z]") &&
  $.contains(r"[0-9]") &&
  $.contains(r"[^a-zA-Z0-9]")
let password = "password1!" as StrongPassword
@out password"#,
        )
        .unwrap_err();
        assert!(err.message.contains("contains"));
    }

    #[test]
    fn struct_safe_parse_validates_and_transforms_fields() {
        let out = run_str(
            r#"struct SignupForm {
  email: string(trim, lower)
  age: int where $ >= 13
}
let good = SignupForm.safeParse({ email: " USER@ORV.DEV ", age: 15 })
@out good.ok
@out good.value.email
let bad = SignupForm.safeParse({ email: "ok@orv.dev", age: 12 })
@out bad.ok
@out bad.error.length"#,
        )
        .unwrap();
        assert_eq!(out, "true\nuser@orv.dev\nfalse\n1\n");
    }

    #[test]
    fn type_validation_parse_errors_and_is_methods_run() {
        let out = run_str(
            r#"struct User {
  name: string(min=2)
  age: int
}
@out User.is({ name: "Al", age: 1 })
@out User.errors({ name: "A", age: 1 }).length
try {
  let user = User.parse({ name: "A", age: 1 })
  @out user.name
} catch err {
  @out err.length
}"#,
        )
        .unwrap();
        assert_eq!(out, "true\n1\n1\n");
    }

    #[test]
    fn type_alias_safe_parse_uses_alias_constraints() {
        let out = run_str(
            r#"type StrongPassword = string(min=8) where
  $.contains(r"[A-Z]") &&
  $.contains(r"[0-9]") &&
  $.contains(r"[^a-zA-Z0-9]")
@out StrongPassword.safeParse("Password1!").ok
@out StrongPassword.safeParse("password1!").error.length"#,
        )
        .unwrap();
        assert_eq!(out, "true\n1\n");
    }

    #[test]
    fn storage_chunk_merge_and_signed_url_flow_runs() {
        let out = run_str(
            r#"let _ = @storage.putChunk("upload-1", 0, "hello")
let file = @storage.merge("upload-1", target="files/upload-1.txt")
@out file.path
@out @storage.signedUrl(file.path)"#,
        )
        .unwrap();
        assert_eq!(
            out,
            "files/upload-1.txt\n/orv-storage/files/upload-1.txt?signed=1\n"
        );
    }

    #[test]
    fn job_enqueue_returns_queued_record() {
        let out = run_str(
            r#"let queued = @job.transcode.enqueue({ videoId: "v1" })
@out queued.name
@out queued.status
@out queued.payload.videoId"#,
        )
        .unwrap();
        assert_eq!(out, "transcode\nqueued\nv1\n");
    }

    #[test]
    fn job_enqueue_records_status_in_builtin_job_table() {
        let out = run_str(
            r#"let queued = @job.transcode.enqueue({ videoId: "v1" })
let stored = @db.find Job { @where name="transcode" }
@out stored.status
@out stored.payload.videoId"#,
        )
        .unwrap();
        assert_eq!(out, "queued\nv1\n");
    }

    #[test]
    fn job_declaration_runs_registered_handler_on_enqueue() {
        let out = run_str(
            r#"let meta = @db.create VideoMeta %data={ status: "pending" }
@job transcode (payload: { videoId: int }) -> {
  @db.update VideoMeta { @where id=payload.videoId; %data={ status: "ready" } }
  "done"
}
let queued = @job.transcode.enqueue({ videoId: meta.id })
let updated = @db.find VideoMeta { @where id=meta.id }
@out updated.status
@out queued.result"#,
        )
        .unwrap();
        assert_eq!(out, "ready\ndone\n");
    }

    #[test]
    fn job_retries_handler_until_success() {
        let out = run_str(
            r#"let state = @db.create RetryState %data={ tries: 0 }
@job flaky retries=2 (id: int) -> {
  let current = @db.find RetryState { @where id=id }
  @db.update RetryState { @where id=id; %data={ tries: current.tries + 1 } }
  let updated = @db.find RetryState { @where id=id }
  if updated.tries < 3 {
    throw "again"
  }
  "ok"
}
let handle = @job.flaky.enqueue(state.id)
let final = @db.find RetryState { @where id=state.id }
@out final.tries
@out handle.status
@out handle.result"#,
        )
        .unwrap();
        assert_eq!(out, "3\ncompleted\nok\n");
    }

    #[test]
    fn cron_declaration_records_schedule_in_builtin_cron_table() {
        let out = run_str(
            r#"@cron "0 9 * * *" {
  @out "tick"
}
let cron = @db.find Cron { @where schedule="0 9 * * *" }
@out cron.status"#,
        )
        .unwrap();
        assert_eq!(out, "registered\n");
    }

    #[test]
    fn cron_run_due_executes_registered_handlers() {
        let out = run_str(
            r#"@cron "* * * * *" {
  @db.create Tick %data={ label: "ran" }
}
@out @db.count Tick
@out @cron.runDue()
@out @db.count Tick
@out @cron.runDue()
@out @db.count Tick"#,
        )
        .unwrap();
        assert_eq!(out, "0\n1\n1\n1\n2\n");
    }

    #[test]
    fn cron_run_due_records_failed_handler() {
        let out = run_str(
            r#"@cron "* * * * *" {
  throw "cron failed"
}
try {
  @cron.runDue()
} catch err {
  @out err
}
let run = @db.find CronRun { @where ok=false }
@out run.error"#,
        )
        .unwrap();
        assert_eq!(out, "cron failed\ncron failed\n");
    }

    #[test]
    fn sync_open_and_connect_return_document_handles() {
        let out = run_str(
            r#"let opened = @sync.open("Doc", "42")
let connected = @sync.connect("Doc", "/doc/42")
@out opened.id
@out connected.path"#,
        )
        .unwrap();
        assert_eq!(out, "42\n/doc/42\n");
    }

    #[test]
    fn mail_verify_and_media_stubs_return_useful_values() {
        let out = run_str(
            r"let ok = @mail.verify.dkim(@message)
let camera = @media.camera({ audio: true, video: false })
@out ok
@out camera.kind
@out @upload.id",
        )
        .unwrap();
        assert_eq!(out, "true\ncamera\nupload-1\n");
    }

    #[test]
    fn realtime_event_domains_are_stable_noops() {
        let out = run_str(
            r#"let ws = @ws /chat {
  @connect { @emit welcome to=@socket.id { msg: "hi" } }
  @on message { @emit message @packet }
  @disconnect { @emit left @socket.id }
}
@socket.join("room-1")
@emit notice in="room-1" { ok: true }
@out ws.protocol
@out ws.path
@out @socket.id
@out @packet.text"#,
        )
        .unwrap();
        assert_eq!(out, "ws\n/chat\nsocket-1\n\n");
    }

    #[test]
    fn push_and_offline_reference_stubs_return_useful_values() {
        let out = run_str(
            r#"let granted = @push.request()
let sub = @push.subscribe(vapid="public-key")
let sent = @push.send({ to: sub.endpoint, title: "Hi" })
let store = @offline.store("posts")
@out granted
@out sub.endpoint
@out sent.status
@out store.name"#,
        )
        .unwrap();
        assert_eq!(out, "true\npush://subscription\nsent\nposts\n");
    }

    #[test]
    fn cache_and_offline_store_methods_preserve_value_flow() {
        let out = run_str(
            r#"let cache = @cache.open("assets-v1")
let saved = cache.put("/app.js", "code")
let loaded = cache.get("/app.js")
let store = @offline.store("assets")
let local = store.put("logo", "blob")
@out cache.name
@out saved.status
@out loaded.value
@out local.key"#,
        )
        .unwrap();
        assert_eq!(out, "assets-v1\nstored\ncode\nlogo\n");
    }

    #[test]
    fn payment_and_shipping_reference_adapters_support_shop_flow() {
        let out = run_str(
            r#"let payments = @payment.connect("test://local")
let captured = payments.capture({ orderId: 7, amount: 25000, method: "card" })
let shipping = @shipping.connect("test://local")
let booking = shipping.book({ orderId: 7, carrier: "post", address: "Seoul" })
@out payments.provider
@out captured.status
@out captured.amount
@out shipping.provider
@out booking.status
@out booking.tracking"#,
        )
        .unwrap();
        assert_eq!(out, "test\ncaptured\n25000\ntest\nready\nTRK-LOCAL\n");
    }

    #[test]
    fn payment_and_shipping_file_adapters_append_records() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let payment_path = std::env::temp_dir().join(format!(
            "orv-runtime-payment-file-{}-{unique}.jsonl",
            std::process::id()
        ));
        let shipping_path = std::env::temp_dir().join(format!(
            "orv-runtime-shipping-file-{}-{unique}.jsonl",
            std::process::id()
        ));
        let source = format!(
            r#"let payments = @payment.connect("file://{}")
let captured = payments.capture({{ orderId: 7, amount: 25000, method: "card" }})
let shipping = @shipping.connect("file://{}")
let booking = shipping.book({{ orderId: 7, carrier: "post", address: "Seoul" }})
@out captured.provider
@out booking.provider"#,
            payment_path.display(),
            shipping_path.display()
        );

        let out = run_str(&source).unwrap();

        assert_eq!(out, "file\nfile\n");
        let payments = std::fs::read_to_string(&payment_path).expect("payment adapter records");
        let shipments = std::fs::read_to_string(&shipping_path).expect("shipping adapter records");
        assert!(payments.contains(r#""kind":"payment.capture""#));
        assert!(payments.contains(r#""orderId":7"#));
        assert!(shipments.contains(r#""kind":"shipping.booking""#));
        assert!(shipments.contains(r#""tracking":"TRK-LOCAL""#));
        let _ = std::fs::remove_file(payment_path);
        let _ = std::fs::remove_file(shipping_path);
    }

    #[test]
    fn payment_and_shipping_http_adapters_post_json_payloads() {
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("bind adapter test server");
        let address = listener.local_addr().expect("adapter test server address");
        let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let server_requests = requests.clone();
        let server = std::thread::spawn(move || {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().expect("accept adapter request");
                let request = read_test_http_request(&mut stream);
                let response = if request.starts_with("POST /capture ") {
                    r#"{"provider":"http","status":"captured_remote","remoteId":"PAY-HTTP"}"#
                } else if request.starts_with("POST /book ") {
                    r#"{"provider":"http","status":"ready_remote","tracking":"TRK-HTTP"}"#
                } else {
                    r#"{"error":"unexpected path"}"#
                };
                server_requests.lock().unwrap().push(request);
                write_test_http_json_response(&mut stream, response);
            }
        });
        let source = format!(
            r#"let payments = @payment.connect("http://{address}/capture")
let captured = payments.capture({{ orderId: 7, amount: 25000, method: "card" }})
let shipping = @shipping.connect("http://{address}/book")
let booking = shipping.book({{ orderId: 7, carrier: "post", address: "Seoul" }})
@out payments.provider
@out captured.status
@out captured.remoteId
@out shipping.provider
@out booking.status
@out booking.tracking"#
        );

        let out = run_str(&source).unwrap();
        server.join().expect("adapter test server finished");

        assert_eq!(
            out,
            "http\ncaptured_remote\nPAY-HTTP\nhttp\nready_remote\nTRK-HTTP\n"
        );
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].contains(r#""kind":"payment.capture""#));
        assert!(requests[0].contains(r#""orderId":7"#));
        assert!(requests[0].contains(r#""amount":25000"#));
        assert!(requests[1].contains(r#""kind":"shipping.booking""#));
        assert!(requests[1].contains(r#""carrier":"post""#));
        assert!(requests[1].contains(r#""address":"Seoul""#));
    }

    #[test]
    fn payment_and_shipping_provider_adapters_support_shop_flow() {
        let out = run_str(
            r#"let payments = @payment.connect("stripe://local")
let captured = payments.capture({ orderId: 7, amount: 25000, method: "card" })
let shipping = @shipping.connect("carrier://local")
let booking = shipping.book({ orderId: 7, carrier: "post", address: "Seoul" })
@out payments.provider
@out captured.status
@out captured.id
@out shipping.provider
@out booking.status
@out booking.tracking"#,
        )
        .unwrap();

        assert_eq!(
            out,
            "stripe\ncaptured\nSTRIPE-PAY-LOCAL\ncarrier\nready\nTRK-CARRIER-LOCAL\n"
        );
    }

    #[test]
    fn provider_adapters_report_credential_status_without_secret_values() {
        let _env_guard = super::test_env::guard();
        super::test_env::set("STRIPE_SECRET_KEY", "sk_test_never_print");
        super::test_env::set("STRIPE_WEBHOOK_SECRET", "");
        super::test_env::set("CARRIER_API_KEY", "");
        super::test_env::set("CARRIER_WEBHOOK_SECRET", "carrier_webhook_never_print");
        let out = run_str(
            r#"let payments = @payment.connect("stripe://local")
let captured = payments.capture({ orderId: 7, amount: 25000, method: "card" })
let shipping = @shipping.connect("carrier://local")
let booking = shipping.book({ orderId: 7, carrier: "post", address: "Seoul" })
@out captured.credentialStatus
@out captured.webhookSecretStatus
@out booking.credentialStatus
@out booking.webhookSecretStatus"#,
        )
        .unwrap();
        super::test_env::clear("STRIPE_SECRET_KEY");
        super::test_env::clear("STRIPE_WEBHOOK_SECRET");
        super::test_env::clear("CARRIER_API_KEY");
        super::test_env::clear("CARRIER_WEBHOOK_SECRET");

        assert_eq!(out, "configured\nmissing\nmissing\nconfigured\n");
        assert!(!out.contains("sk_test_never_print"));
        assert!(!out.contains("carrier_webhook_never_print"));
    }

    #[test]
    fn provider_adapters_call_configured_provider_endpoints_without_exposing_secrets() {
        let _env_guard = super::test_env::guard();
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("bind provider test server");
        let address = listener.local_addr().expect("provider test server address");
        let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let server_requests = requests.clone();
        let server = std::thread::spawn(move || {
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().expect("accept provider request");
                let request = read_test_http_request(&mut stream);
                let response = if request.starts_with("POST /stripe/payment_intents ") {
                    r#"{"id":"pi_test_123","status":"succeeded","providerStatus":"captured_provider"}"#
                } else if request.starts_with("POST /carrier/shipments ") {
                    r#"{"id":"ship_test_123","status":"booked_provider","tracking":"TRK-PROVIDER"}"#
                } else {
                    r#"{"error":"unexpected path"}"#
                };
                server_requests.lock().unwrap().push(request);
                write_test_http_json_response(&mut stream, response);
            }
        });
        super::test_env::set(
            "STRIPE_API_ENDPOINT",
            &format!("http://{address}/stripe/payment_intents"),
        );
        super::test_env::set("STRIPE_SECRET_KEY", "sk_test_never_print");
        super::test_env::set(
            "CARRIER_API_ENDPOINT",
            &format!("http://{address}/carrier/shipments"),
        );
        super::test_env::set("CARRIER_API_KEY", "carrier_key_never_print");

        let out = run_str(
            r#"let payments = @payment.connect("stripe://local")
let captured = payments.capture({ orderId: 7, amount: 25000, method: "card" })
let shipping = @shipping.connect("carrier://local")
let booking = shipping.book({ orderId: 7, carrier: "post", address: "Seoul" })
@out captured.id
@out captured.status
@out captured.providerStatus
@out booking.id
@out booking.status
@out booking.tracking"#,
        )
        .unwrap();
        super::test_env::clear("STRIPE_API_ENDPOINT");
        super::test_env::clear("STRIPE_SECRET_KEY");
        super::test_env::clear("CARRIER_API_ENDPOINT");
        super::test_env::clear("CARRIER_API_KEY");
        server.join().expect("provider test server finished");

        assert_eq!(
            out,
            "pi_test_123\nsucceeded\ncaptured_provider\nship_test_123\nbooked_provider\nTRK-PROVIDER\n"
        );
        assert!(!out.contains("sk_test_never_print"));
        assert!(!out.contains("carrier_key_never_print"));
        let requests = requests.lock().unwrap();
        assert!(requests[0].contains(r#""kind":"stripe.payment_intent.create""#));
        assert!(requests[0].contains(r#""amount":25000"#));
        assert!(requests[0].contains("authorization: Bearer sk_test_never_print"));
        assert!(requests[1].contains(r#""kind":"carrier.shipment.create""#));
        assert!(requests[1].contains(r#""carrier":"post""#));
        assert!(requests[1].contains("authorization: Bearer carrier_key_never_print"));
    }

    #[test]
    fn provider_adapters_retry_transient_endpoint_errors_with_idempotency_keys() {
        let _env_guard = super::test_env::guard();
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("bind provider retry test server");
        let address = listener
            .local_addr()
            .expect("provider retry test server address");
        let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let server_requests = requests.clone();
        let server = std::thread::spawn(move || {
            for index in 0..3 {
                let (mut stream, _) = listener.accept().expect("accept provider retry request");
                let request = read_test_http_request(&mut stream);
                server_requests.lock().unwrap().push(request);
                if index == 0 {
                    write_test_http_response(
                        &mut stream,
                        "HTTP/1.1 500 Internal Server Error\r\ncontent-length: 17\r\nconnection: close\r\n\r\ntransient failure",
                    );
                } else if index == 1 {
                    write_test_http_json_response(
                        &mut stream,
                        r#"{"id":"pi_retry","status":"succeeded"}"#,
                    );
                } else {
                    write_test_http_json_response(
                        &mut stream,
                        r#"{"id":"ship_retry","status":"booked_provider"}"#,
                    );
                }
            }
        });
        super::test_env::set(
            "STRIPE_API_ENDPOINT",
            &format!("http://{address}/stripe/payment_intents"),
        );
        super::test_env::set("STRIPE_SECRET_KEY", "sk_test_never_print");
        super::test_env::set(
            "CARRIER_API_ENDPOINT",
            &format!("http://{address}/carrier/shipments"),
        );
        super::test_env::set("CARRIER_API_KEY", "carrier_key_never_print");

        let out = run_str(
            r#"let payments = @payment.connect("stripe://local")
let captured = payments.capture({ orderId: "o_retry", amount: 25000, method: "card" })
let shipping = @shipping.connect("carrier://local")
let booking = shipping.book({ orderId: "o_retry", carrier: "post", address: "Seoul" })
@out captured.id
@out booking.id"#,
        )
        .unwrap();
        super::test_env::clear("STRIPE_API_ENDPOINT");
        super::test_env::clear("STRIPE_SECRET_KEY");
        super::test_env::clear("CARRIER_API_ENDPOINT");
        super::test_env::clear("CARRIER_API_KEY");
        server.join().expect("provider retry test server finished");

        assert_eq!(out, "pi_retry\nship_retry\n");
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert!(requests[0].contains("idempotency-key: stripe.payment_intent.create:o_retry"));
        assert!(requests[1].contains("idempotency-key: stripe.payment_intent.create:o_retry"));
        assert!(requests[2].contains("idempotency-key: carrier.shipment.create:o_retry"));
    }

    #[test]
    fn stripe_provider_adapter_verifies_webhook_signature_without_exposing_secret() {
        let _env_guard = super::test_env::guard();
        super::test_env::set("STRIPE_WEBHOOK_SECRET", "whsec_test");
        let out = run_str(
            r#"let payments = @payment.connect("stripe://local")
let verified = payments.verifyWebhook({
  payload: "evt_1",
  signature: "t=1700000000,v1=6d4aa0747f1f67084c320780929635a8fcde580f00b308ac4bfdd04ab75bf6bf"
})
let rejected = payments.verifyWebhook({
  payload: "evt_1",
  signature: "t=1700000000,v1=0000000000000000000000000000000000000000000000000000000000000000"
})
@out verified.status
@out rejected.status
@out verified.provider"#,
        )
        .unwrap();
        super::test_env::clear("STRIPE_WEBHOOK_SECRET");

        assert_eq!(out, "verified\ninvalid\nstripe\n");
        assert!(!out.contains("whsec_test"));
    }

    #[test]
    fn stripe_provider_adapter_accepts_previous_webhook_secret_for_rotation() {
        let _env_guard = super::test_env::guard();
        super::test_env::set("STRIPE_WEBHOOK_SECRET", "whsec_current");
        super::test_env::set("STRIPE_WEBHOOK_SECRET_PREVIOUS", "whsec_previous");
        let signature =
            super::hmac_sha256_hex("whsec_previous", "1700000000.evt_rotated").expect("signature");
        let out = run_str(&format!(
            r#"let payments = @payment.connect("stripe://local")
let verified = payments.verifyWebhook({{
  payload: "evt_rotated",
  signature: "t=1700000000,v1={signature}"
}})
@out verified.status
@out verified.webhookSecretStatus
@out verified.webhookSecretMatch"#
        ))
        .unwrap();
        super::test_env::clear("STRIPE_WEBHOOK_SECRET");
        super::test_env::clear("STRIPE_WEBHOOK_SECRET_PREVIOUS");

        assert_eq!(out, "verified\nconfigured\nprevious\n");
        assert!(!out.contains("whsec_current"));
        assert!(!out.contains("whsec_previous"));
    }

    fn read_test_http_request(stream: &mut std::net::TcpStream) -> String {
        use std::io::Read as _;

        let mut bytes = Vec::new();
        let mut buf = [0_u8; 512];
        let header_end = loop {
            let read = stream.read(&mut buf).expect("read adapter request");
            assert!(read > 0, "adapter request closed before headers");
            bytes.extend_from_slice(&buf[..read]);
            if let Some(index) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                break index + 4;
            }
        };
        let headers = String::from_utf8_lossy(&bytes[..header_end]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
            .unwrap_or(0);
        while bytes.len() < header_end + content_length {
            let read = stream.read(&mut buf).expect("read adapter request body");
            assert!(read > 0, "adapter request closed before body");
            bytes.extend_from_slice(&buf[..read]);
        }
        String::from_utf8(bytes).expect("adapter request utf-8")
    }

    fn write_test_http_json_response(stream: &mut std::net::TcpStream, body: &str) {
        use std::io::Write as _;

        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream
            .write_all(response.as_bytes())
            .expect("write adapter response");
    }

    fn write_test_http_response(stream: &mut std::net::TcpStream, response: &str) {
        use std::io::Write as _;

        stream
            .write_all(response.as_bytes())
            .expect("write raw adapter response");
    }

    #[test]
    fn unsafe_block_evaluates_body_as_reference_runtime_boundary() {
        let out = run_str(
            r#"let value = @unsafe {
  @out "inside"
  "result"
}
@out value"#,
        )
        .unwrap();
        assert_eq!(out, "inside\nresult\n");
    }

    #[test]
    fn ffi_methods_require_unsafe_boundary() {
        let err = run_str(r#"let lib = @ffi.load("native")"#).unwrap_err();
        assert!(err.message.contains("requires @unsafe"), "{}", err.message);
    }

    #[test]
    fn ffi_methods_run_inside_unsafe_boundary() {
        let out = run_str(
            r#"let name = @unsafe {
  let lib = @ffi.load("native")
  lib.name
}
@out name"#,
        )
        .unwrap();
        assert_eq!(out, "native\n");
    }

    #[test]
    fn net_methods_require_unsafe_boundary() {
        let err = run_str(r#"let tun = @net.tun.create(name="orv0")"#).unwrap_err();
        assert!(err.message.contains("requires @unsafe"), "{}", err.message);
    }

    #[test]
    fn net_plugin_gpu_and_observability_reference_stubs_return_handles() {
        let out = run_str(
            r#"let net = @unsafe {
  let tun = @net.tun.create(name="orv0", ipv4="10.8.0.1/24")
  let written = @net.tun.write(tun, "packet")
  { name: tun.name, bytes: written.bytes }
}
let plugin = @plugin.load("ext/markdown-preview.wasm")
let activation = plugin.activate()
let compute = @gpu.compute(file="shaders/blur.wgsl", workgroup=[16, 16, 1])
let ctx = @gpu.context("canvas")
let obs = @observability.configure({ service: "superapp" })
@out net.name
@out net.bytes
@out plugin.path
@out activation.status
@out compute.kind
@out ctx.kind
@out obs.service"#,
        )
        .unwrap();
        assert_eq!(
            out,
            "orv0\n6\next/markdown-preview.wasm\nactivated\ncompute\ngpu.context\nsuperapp\n"
        );
    }

    #[test]
    fn reference_namespace_methods_accept_spec_parenless_form() {
        let out = run_str(
            r#"let plugin = @plugin.load "ext/markdown-preview.wasm"
let compute = @gpu.compute file="shaders/blur.wgsl" workgroup=[16, 16, 1]
let sent = @push.send { to: "u1", title: "Hi" }
@out plugin.path
@out compute.file
@out sent.status"#,
        )
        .unwrap();
        assert_eq!(out, "ext/markdown-preview.wasm\nshaders/blur.wgsl\nsent\n");
    }

    #[test]
    fn plugin_discover_returns_candidate_list() {
        let out = run_str(
            r#"let plugins = @plugin.discover("./.orv/extensions")
@out plugins.length
@out plugins[0].path"#,
        )
        .unwrap();
        assert_eq!(out, "1\n./.orv/extensions/plugin.wasm\n");
    }

    #[test]
    fn gpu_render_block_returns_handle_without_executing_commands() {
        let out = run_str(
            r##"let scene = @gpu.render {
  @clear color="#000"
  @draw mesh="cube"
}
@out scene.kind
@out scene.commands"##,
        )
        .unwrap();
        assert_eq!(out, "render\n2\n");
    }

    #[test]
    fn declaration_style_reference_domains_are_stable_handles() {
        let out = run_str(
            r#"let obs = @observability {
  service: "superapp"
}
let offline = @offline {
  @cache "assets-v1" strategy=cache-first {}
}
let ffi = @ffi "C" {
}
@out obs.service
@out offline.kind
@out ffi.abi"#,
        )
        .unwrap();
        assert_eq!(out, "superapp\noffline\nC\n");
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
    fn typed_map_spread_merges_object_fields() {
        let out = run_str(
            r#"let base = { a: 1 }
let m = Map{...base, "b": 2}
@out m"#,
        )
        .unwrap();
        assert_eq!(out, "{ a: 1, b: 2 }\n");
    }

    #[test]
    fn typed_set_spread_flattens_array_elements() {
        let out = run_str(
            r#"let xs = [1, 2]
let ys = Set{...xs, 3}
@out ys"#,
        )
        .unwrap();
        assert_eq!(out, "[1, 2, 3]\n");
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
        let out = run_str(
            r#"await sleep(10)
@out "ok""#,
        )
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
        let path = format!("/tmp/orv_io_newline_{}.txt", std::process::id());
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
