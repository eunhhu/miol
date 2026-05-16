//! `@server` HTTP 런타임 (C5b, MVP).
//!
//! tokio 의 `current_thread` 런타임 위에서 hyper 1.x HTTP/1.1 서버를 기동한다.
//! 요청마다 매칭된 route 의 handler HIR 을 **복제**하고 새 [`crate::interp::Interp`]
//! 를 만들어 [`crate::interp::run_handler_with_request`] 로 평가한다. 이 구조의
//! 이점:
//!
//! - 인터프리터 자체는 여전히 순수 동기 — async 는 이 파일 안에만 갇힌다.
//! - 요청 간 상태 누수 없음. 각 요청이 새 env, 새 writer(버퍼), 새 response 슬롯
//!   을 갖는다.
//! - 기존 interp 구조 변경 최소. Server arm 이 이 모듈의 [`run_server`] 를
//!   부르기만 한다.
//!
//! MVP 범위 / 비범위
//! - HTTP/1.1 단일. SPEC §11 의 QUIC/HTTP3 기본값은 이후 마일스톤.
//! - JSON 직렬화는 [`value_to_json`] — object/array/스칼라/void 만.
//! - 경로 매처는 [`match_route`] — 선형 탐색, `:param` 추출, `*` wildcard segment
//!   미지원 (C5 범위 밖, §11.7 중첩 라우트와 함께 후속).

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::mpsc;
use std::sync::Arc;
use std::sync::Mutex;
use std::task::{Context, Poll};
use std::thread;
use std::time::{Duration, Instant};

use bytes::Bytes;
use http_body::{Body as HttpBody, Frame, SizeHint};
#[cfg(test)]
use http_body_util::Full;
use http_body_util::{BodyExt, Limited};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use orv_hir::{origin_id, HirExpr, HirExprKind, HirProgram, HirStmt, NameId};
use tokio::net::TcpListener;
use tokio::sync::mpsc as tokio_mpsc;

use crate::db::{new_db_handle, DbHandle};
use crate::interp::{
    eval_expr_in_env, run_handler_with_request_in_env, run_with_writer_in_env_with_db, RequestCtx,
    ResponseCtx, RuntimeError, Value,
};

/// MVP request body size limit (1MB). 초과 시 413 Payload Too Large.
///
/// hyper 자체는 body 크기 상한이 없어, 악의적 거대 POST 한 번에 메모리를 전부
/// 할당해 버리는 DoS 벡터가 된다. `http_body_util::Limited` 로 래핑해 수집
/// 단계에서 방지한다. 1MB 는 작은 JSON 페이로드/폼 입력을 통과시키면서
/// 멀티파트 파일 업로드는 막는 선. 파일 업로드는 SPEC §11 의 별도 경로로
/// 다룬다.
const MAX_BODY_BYTES: usize = 1024 * 1024;
const ORV_ORIGIN_ID_HEADER: &str = "x-orv-origin-id";
const ORV_RUNTIME_REQUEST_TRACE_PATH_ENV: &str = "ORV_RUNTIME_REQUEST_TRACE_PATH";
const ORV_TRACE_EVENTS_PATH: &str = "/__orv/trace/events";
const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(60);

/// `@server` 가 수집한 단일 라우트 — handler HIR 의 스냅샷.
///
/// HIR 은 `Clone` 이므로 서버 기동 시점에 한번 복제해 두고 요청마다 또 한 번
/// clone 해서 handler 평가에 넘긴다. 이중 clone 이 비효율적으로 보이지만 MVP
/// 에서는 라우트 수와 handler 크기가 작고, 이 구조 덕에 Interp 가 HIR 에 대한
/// 참조 수명을 가질 필요가 없어 전체 설계가 단순해진다.
#[derive(Clone)]
struct RouteEntry {
    method: String,
    path: String,
    handler: HirExpr,
    origin_id: String,
}

/// Route table shared only inside a tokio current-thread server loop.
///
/// `RouteEntry` contains HIR values backed by non-thread-safe runtime data, so
/// this type deliberately uses `Rc` instead of `Arc`. If the server execution
/// model moves to multi-threaded request handling, the compiler will force this
/// boundary to be redesigned instead of silently cloning non-Send state across
/// tasks.
#[derive(Clone)]
struct LocalRoutes(Rc<Vec<RouteEntry>>);

impl LocalRoutes {
    fn new(routes: Vec<RouteEntry>) -> Self {
        Self(Rc::new(routes))
    }

    fn iter(&self) -> std::slice::Iter<'_, RouteEntry> {
        self.0.iter()
    }
}

/// Captured server environment shared only by local request evaluation.
///
/// The values can contain `Rc`-backed runtime data. Keeping the state behind an
/// explicit local wrapper makes the current-thread invariant visible at the
/// function boundary.
#[derive(Clone)]
struct LocalCapturedEnv(Rc<HashMap<NameId, Value>>);

impl LocalCapturedEnv {
    fn new(captured_env: HashMap<NameId, Value>) -> Self {
        Self(Rc::new(captured_env))
    }

    fn snapshot(&self) -> HashMap<NameId, Value> {
        self.0.as_ref().clone()
    }
}

type ServerResponse = Response<RuntimeBody>;

enum RuntimeBody {
    Full(Option<Bytes>),
    Trace(TraceEventBody),
}

impl RuntimeBody {
    fn full(body: impl Into<Bytes>) -> Self {
        Self::Full(Some(body.into()))
    }

    fn trace(initial: String, rx: tokio_mpsc::UnboundedReceiver<Bytes>) -> Self {
        Self::Trace(TraceEventBody {
            initial: Some(Bytes::from(initial)),
            rx,
        })
    }
}

impl HttpBody for RuntimeBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        match &mut *self {
            Self::Full(body) => Poll::Ready(body.take().map(|bytes| Ok(Frame::data(bytes)))),
            Self::Trace(body) => Pin::new(body).poll_frame(cx),
        }
    }

    fn is_end_stream(&self) -> bool {
        matches!(self, Self::Full(None))
    }

    fn size_hint(&self) -> SizeHint {
        let mut hint = SizeHint::new();
        if let Self::Full(Some(bytes)) = self {
            hint.set_exact(bytes.len() as u64);
        }
        hint
    }
}

struct TraceEventBody {
    initial: Option<Bytes>,
    rx: tokio_mpsc::UnboundedReceiver<Bytes>,
}

impl HttpBody for TraceEventBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        if let Some(initial) = self.initial.take() {
            return Poll::Ready(Some(Ok(Frame::data(initial))));
        }
        Pin::new(&mut self.rx)
            .poll_recv(cx)
            .map(|item| item.map(|bytes| Ok(Frame::data(bytes))))
    }

    fn is_end_stream(&self) -> bool {
        false
    }
}

#[derive(Clone)]
struct TraceState {
    frames: Arc<Mutex<Vec<ServerRequestFrame>>>,
    subscribers: Arc<Mutex<Vec<tokio_mpsc::UnboundedSender<Bytes>>>>,
}

impl TraceState {
    fn new() -> Self {
        Self {
            frames: Arc::new(Mutex::new(Vec::new())),
            subscribers: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn frames_handle(&self) -> Arc<Mutex<Vec<ServerRequestFrame>>> {
        Arc::clone(&self.frames)
    }

    fn frames(&self) -> Vec<ServerRequestFrame> {
        self.frames
            .lock()
            .map_or_else(|_| Vec::new(), |frames| frames.clone())
    }

    fn subscribe(&self) -> tokio_mpsc::UnboundedReceiver<Bytes> {
        let (tx, rx) = tokio_mpsc::unbounded_channel();
        if let Ok(mut subscribers) = self.subscribers.lock() {
            subscribers.push(tx);
        }
        rx
    }

    fn record(&self, frame: ServerRequestFrame) {
        let index = if let Ok(mut frames) = self.frames.lock() {
            let index = frames.len();
            frames.push(frame.clone());
            index
        } else {
            0
        };
        let event = Bytes::from(request_trace_frame_event_body(index, &frame));
        if let Ok(mut subscribers) = self.subscribers.lock() {
            subscribers.retain(|subscriber| subscriber.send(event.clone()).is_ok());
        }
    }
}

#[derive(Clone, Default)]
struct RateLimitState {
    buckets: Arc<Mutex<HashMap<String, Vec<Instant>>>>,
}

impl RateLimitState {
    fn check(&self, key: &str, limit: usize, window: Duration) -> bool {
        let now = Instant::now();
        let cutoff = now - window;
        let Ok(mut buckets) = self.buckets.lock() else {
            return true;
        };
        let bucket = buckets.entry(key.to_string()).or_default();
        bucket.retain(|instant| *instant >= cutoff);
        if bucket.len() >= limit {
            return false;
        }
        bucket.push(now);
        true
    }
}

#[derive(Clone, Copy)]
struct RateLimitPolicy {
    limit: usize,
    window: Duration,
}

fn default_rate_limit_policy(method: &str, path: &str) -> Option<RateLimitPolicy> {
    match (method, path) {
        ("POST", "/members/login" | "/checkout") => Some(RateLimitPolicy {
            limit: 10,
            window: RATE_LIMIT_WINDOW,
        }),
        ("POST", "/webhooks/stripe") => Some(RateLimitPolicy {
            limit: 60,
            window: RATE_LIMIT_WINDOW,
        }),
        _ => None,
    }
}

/// Captured metadata for one HTTP request handled by an attached runtime.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ServerRequestFrame {
    /// Request method after HTTP parsing.
    pub method: String,
    /// Normalized request path, without query string.
    pub path: String,
    /// Route method that matched the request.
    pub route_method: Option<String>,
    /// Route path pattern that matched the request.
    pub route_path: Option<String>,
    /// Origin id for the matched route.
    pub route_origin_id: Option<String>,
    /// HTTP response status returned to the client.
    pub status: u16,
    /// Captured path parameters.
    pub params: HashMap<String, String>,
    /// Captured query parameters.
    pub query: HashMap<String, String>,
    /// Compact request body display.
    pub body: String,
}

/// 포트 번호와 라우트 테이블을 들고 hyper 서버를 기동한다.
///
/// # Errors
/// - `listen` 이 Int 가 아니거나 포트 범위를 벗어나면 RuntimeError.
/// - 바인딩 실패도 RuntimeError.
/// - accept/serve 루프의 I/O 에러는 로그로 흘려보내고 다음 연결로 넘어간다
///   (한 커넥션 실패로 서버 전체가 죽지 않도록).
pub(crate) fn run_server_with_request_trace_path(
    listen: Option<&HirExpr>,
    routes: &[HirExpr],
    body_stmts: &[orv_hir::HirStmt],
    captured_env: HashMap<NameId, Value>,
    db: DbHandle,
    request_trace_path: Option<std::path::PathBuf>,
) -> Result<Value, RuntimeError> {
    let mut stdout = std::io::stdout().lock();
    let (port, entries, captured_env, db) = prepare_server_state(
        listen,
        routes,
        body_stmts,
        captured_env,
        db,
        &mut stdout,
        false,
    )?;

    // 4) tokio current_thread 런타임 생성. 전용 런타임이라 스레드 이동 제약이
    //    없고, `!Send` HIR 값(Rc 기반 Value)도 요청 핸들러 안에서 그대로 사용
    //    가능하다. hyper 1.x 는 `Send + Sync` handler 를 요구하지 않으므로 이
    //    조합이 자연스럽다.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| RuntimeError::native(format!("tokio runtime init failed: {e}")))?;

    let request_trace_path = request_trace_path.or_else(runtime_request_trace_path_from_env);
    let local = tokio::task::LocalSet::new();
    runtime.block_on(local.run_until(async move {
        let addr: SocketAddr = ([127, 0, 0, 1], port).into();
        let listener = TcpListener::bind(addr)
            .await
            .map_err(|e| RuntimeError::native(format!("failed to bind {addr}: {e}")))?;
        // Graceful shutdown — SIGINT (ctrl_c) + SIGTERM (Unix).
        //
        // SIGTERM 은 컨테이너/systemd 가 기본으로 보내는 신호라 SIGINT 만으로는
        // 프로덕션 배포에서 graceful 이 안 먹는다. Windows 타깃은 SIGTERM
        // 개념이 없으므로 `#[cfg(unix)]` 로 갈라친다.
        serve_loop_with_request_trace_file(
            listener,
            LocalRoutes::new(entries),
            LocalCapturedEnv::new(captured_env),
            db,
            None,
            request_trace_path,
            shutdown_signal(),
        )
        .await
    }))?;

    Ok(Value::Void)
}

/// Handle for an in-process attached `@server` runtime.
///
/// Dropping the handle sends a graceful shutdown signal and joins the runtime
/// thread before returning.
pub struct AttachedServer {
    addr: SocketAddr,
    boot_output: Vec<u8>,
    request_frames: Arc<Mutex<Vec<ServerRequestFrame>>>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    handle: Option<thread::JoinHandle<()>>,
}

type AttachedStartup = Result<
    (
        SocketAddr,
        Vec<u8>,
        Arc<Mutex<Vec<ServerRequestFrame>>>,
        tokio::sync::oneshot::Sender<()>,
    ),
    String,
>;

impl AttachedServer {
    /// Bound socket address for the attached server.
    #[must_use]
    pub const fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Output produced while preparing the attached server.
    #[must_use]
    pub fn boot_output(&self) -> &[u8] {
        &self.boot_output
    }

    /// Request frames captured by this attached server so far.
    ///
    /// If the internal lock is poisoned, returns an empty vector. The debug
    /// surface treats request frames as best-effort telemetry.
    #[must_use]
    pub fn request_frames(&self) -> Vec<ServerRequestFrame> {
        self.request_frames
            .lock()
            .map_or_else(|_| Vec::new(), |frames| frames.clone())
    }
}

impl Drop for AttachedServer {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Start the first `@server` expression in `program` on a dedicated in-process
/// runtime thread.
///
/// This tooling entry point permits `@listen 0` so callers can request an
/// ephemeral local port.
///
/// # Errors
/// Returns a runtime error when the program does not contain an `@server`, when
/// the server cannot prepare its routes/listen port, or when the listener fails
/// to bind.
pub fn spawn_attached_server(program: HirProgram) -> Result<AttachedServer, RuntimeError> {
    let (startup_tx, startup_rx) = mpsc::sync_channel::<AttachedStartup>(1);
    let handle = thread::spawn(move || attached_server_thread(&program, &startup_tx));
    match startup_rx.recv() {
        Ok(Ok((addr, boot_output, request_frames, shutdown))) => Ok(AttachedServer {
            addr,
            boot_output,
            request_frames,
            shutdown: Some(shutdown),
            handle: Some(handle),
        }),
        Ok(Err(message)) => {
            let _ = handle.join();
            Err(RuntimeError::native(message))
        }
        Err(err) => {
            let _ = handle.join();
            Err(RuntimeError::native(format!(
                "attached server failed before startup: {err}"
            )))
        }
    }
}

fn attached_server_thread(program: &HirProgram, startup: &mpsc::SyncSender<AttachedStartup>) {
    if let Err(message) = run_attached_server_thread(program, startup) {
        let _ = startup.send(Err(message));
    }
}

fn run_attached_server_thread(
    program: &HirProgram,
    startup: &mpsc::SyncSender<AttachedStartup>,
) -> Result<(), String> {
    let mut boot_output = Vec::new();
    let (port, entries, captured_env, db) =
        attached_server_state(program, &mut boot_output).map_err(|e| e.to_string())?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("tokio runtime init failed: {e}"))?;
    let local = tokio::task::LocalSet::new();
    runtime.block_on(local.run_until(async move {
        let listener = TcpListener::bind(("127.0.0.1", port))
            .await
            .map_err(|e| format!("attached server bind failed: {e}"))?;
        let addr = listener
            .local_addr()
            .map_err(|e| format!("attached server local_addr failed: {e}"))?;
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let trace_state = TraceState::new();
        let request_frames = trace_state.frames_handle();
        startup
            .send(Ok((
                addr,
                boot_output,
                Arc::clone(&request_frames),
                shutdown_tx,
            )))
            .map_err(|_| "attached server startup receiver dropped".to_string())?;
        serve_loop(
            listener,
            LocalRoutes::new(entries),
            LocalCapturedEnv::new(captured_env),
            db,
            Some(trace_state),
            async move {
                let _ = shutdown_rx.await;
            },
        )
        .await
        .map_err(|e| e.to_string())
    }))
}

fn attached_server_state<W: std::io::Write>(
    program: &HirProgram,
    boot_writer: &mut W,
) -> Result<PreparedServerState, RuntimeError> {
    let db = new_db_handle();
    let server_idx = program
        .items
        .iter()
        .position(|stmt| matches!(stmt, HirStmt::Expr(expr) if matches!(expr.kind, HirExprKind::Server { .. })))
        .ok_or_else(|| RuntimeError::native("attached runtime requires an `@server` expression"))?;
    let captured_env = if server_idx == 0 {
        HashMap::new()
    } else {
        let prefix = HirProgram {
            items: program.items[..server_idx].to_vec(),
            span: program.items[0]
                .span()
                .join(program.items[server_idx - 1].span()),
        };
        run_with_writer_in_env_with_db(&prefix, HashMap::new(), db.clone(), boot_writer)?
    };
    let HirStmt::Expr(expr) = &program.items[server_idx] else {
        return Err(RuntimeError::native("attached runtime expected expression"));
    };
    let HirExprKind::Server {
        listen,
        routes,
        body_stmts,
    } = &expr.kind
    else {
        return Err(RuntimeError::native("attached runtime expected server"));
    };
    prepare_server_state(
        listen.as_deref(),
        routes,
        body_stmts,
        captured_env,
        db,
        boot_writer,
        true,
    )
}

/// SIGINT + (Unix) SIGTERM 둘 중 하나가 오면 resolve 되는 Future.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("failed to install SIGTERM handler: {e}");
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = sigterm.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// 테스트에서 임의의 포트에 바인딩하고 주소를 돌려받기 위한 진입점.
///
/// 운영 경로([`run_server`])와 다른 점:
/// - 포트 0 으로 바인딩해 OS 에 맡기고 실제 주소를 반환한다.
/// - accept 루프는 별도 tokio task 로 띄우고 즉시 `(addr, handle, boot)` 를
///   돌려준다.
/// - 호출자는 테스트 끝에 `handle.abort()` 로 서버를 정리한다.
///
/// `body_stmts` 는 `@server { @out "boot" @listen 0 ... }` 처럼 @server 블록
/// 최상단에 있던 non-route 문장들이다. [`run_server`] 는 이들을 accept 시작
/// 전에 **공용 stdout** 으로 흘린다. 테스트에서는 stdout 을 가로챌 수 없어
/// 같은 순서로 `Vec<u8>` writer 에 캡처해 돌려준다 — C5c 의 body_stmts 패치가
/// 실제로 런타임에 도달하는지 fixture 수준에서 증명하기 위함.
#[cfg(test)]
pub(crate) async fn spawn_for_test<S>(
    listen: Option<&HirExpr>,
    routes: &[HirExpr],
    body_stmts: &[orv_hir::HirStmt],
    captured_env: HashMap<NameId, Value>,
    shutdown: S,
) -> Result<(SocketAddr, tokio::task::JoinHandle<()>, Vec<u8>), RuntimeError>
where
    S: std::future::Future<Output = ()> + 'static,
{
    let mut boot_buf: Vec<u8> = Vec::new();
    let (port, entries, captured_env, db) = prepare_server_state(
        listen,
        routes,
        body_stmts,
        captured_env,
        new_db_handle(),
        &mut boot_buf,
        true,
    )?;

    let listener = TcpListener::bind(("127.0.0.1", port))
        .await
        .map_err(|e| RuntimeError::native(format!("test bind failed: {e}")))?;
    let addr = listener
        .local_addr()
        .map_err(|e| RuntimeError::native(format!("local_addr failed: {e}")))?;
    let table = LocalRoutes::new(entries);
    let captured_env = LocalCapturedEnv::new(captured_env);
    let handle = tokio::task::spawn_local(async move {
        let _ = serve_loop(listener, table, captured_env, db, None, shutdown).await;
    });
    Ok((addr, handle, boot_buf))
}

#[cfg(test)]
#[allow(clippy::future_not_send)]
pub(crate) async fn spawn_for_test_with_request_trace_file<S>(
    listen: Option<&HirExpr>,
    routes: &[HirExpr],
    body_stmts: &[orv_hir::HirStmt],
    captured_env: HashMap<NameId, Value>,
    request_trace_path: std::path::PathBuf,
    shutdown: S,
) -> Result<(SocketAddr, tokio::task::JoinHandle<()>, Vec<u8>), RuntimeError>
where
    S: std::future::Future<Output = ()> + 'static,
{
    let mut boot_buf: Vec<u8> = Vec::new();
    let (port, entries, captured_env, db) = prepare_server_state(
        listen,
        routes,
        body_stmts,
        captured_env,
        new_db_handle(),
        &mut boot_buf,
        true,
    )?;

    let listener = TcpListener::bind(("127.0.0.1", port))
        .await
        .map_err(|e| RuntimeError::native(format!("test bind failed: {e}")))?;
    let addr = listener
        .local_addr()
        .map_err(|e| RuntimeError::native(format!("local_addr failed: {e}")))?;
    let table = LocalRoutes::new(entries);
    let captured_env = LocalCapturedEnv::new(captured_env);
    let handle = tokio::task::spawn_local(async move {
        let _ = serve_loop_with_request_trace_file(
            listener,
            table,
            captured_env,
            db,
            None,
            Some(request_trace_path),
            shutdown,
        )
        .await;
    });
    Ok((addr, handle, boot_buf))
}

/// 서버 기동 전 상태 — `(포트, 라우트 테이블, 캡처 환경, 공유 DB)`.
type PreparedServerState = (u16, Vec<RouteEntry>, HashMap<NameId, Value>, DbHandle);

fn prepare_server_state<W: std::io::Write>(
    listen: Option<&HirExpr>,
    routes: &[HirExpr],
    body_stmts: &[orv_hir::HirStmt],
    captured_env: HashMap<NameId, Value>,
    db: DbHandle,
    boot_writer: &mut W,
    allow_ephemeral_port: bool,
) -> Result<PreparedServerState, RuntimeError> {
    // 1) body_stmts 평가 — `@out` 같은 부트 출력뿐 아니라 server-level
    //    let/const/function 선언도 여기서 캡처된 환경 위에 쌓아 handler 가
    //    볼 수 있게 만든다. `@listen port` 같은 표현식도 이 환경을 보게 하기
    //    위해 포트 결정보다 먼저 수행한다.
    let captured_env = if body_stmts.is_empty() {
        captured_env
    } else {
        let boot_program = HirProgram {
            items: body_stmts.to_vec(),
            span: body_stmts[0].span(),
        };
        run_with_writer_in_env_with_db(&boot_program, captured_env, db.clone(), boot_writer)?
    };

    // 2) listen 포트 결정. 운영 경로는 @listen 없으면 에러, 테스트 경로는 `0`
    //    을 허용해 OS 임의 포트 바인딩을 사용할 수 있다.
    let port = resolve_listen_port(listen, &captured_env, allow_ephemeral_port)?;

    // 3) routes → RouteEntry 로 평평하게. analyzer 가 routes 벡터에 Route
    //    variant 만 넣기로 계약했으므로 그 외는 에러.
    let entries = collect_routes(routes)?;

    Ok((port, entries, captured_env, db))
}

fn resolve_listen_port(
    listen: Option<&HirExpr>,
    env: &HashMap<NameId, Value>,
    allow_ephemeral_port: bool,
) -> Result<u16, RuntimeError> {
    let Some(expr) = listen else {
        return Err(RuntimeError::native(
            "`@server` requires an `@listen PORT` declaration",
        ));
    };
    // `@listen` 은 이제 캡처 환경을 보는 식을 허용한다. top-level/server-level
    // 바인딩, 괄호식, 간단한 산술 등을 평가한 뒤 정수 포트로 해석한다.
    let mut sink = Vec::new();
    let value = eval_expr_in_env(expr, env, &mut sink)?;
    let n = match value {
        Value::Int(n) => n,
        other => {
            return Err(RuntimeError::native(format!(
                "`@listen` port expression must evaluate to int, got {other}"
            )));
        }
    };
    let valid = if allow_ephemeral_port {
        (0..=65535).contains(&n)
    } else {
        (1..=65535).contains(&n)
    };
    if !valid {
        let range = if allow_ephemeral_port {
            "0..=65535"
        } else {
            "1..=65535"
        };
        return Err(RuntimeError::native(format!(
            "@listen port out of range {range}: {n}"
        )));
    }
    Ok(n as u16)
}

fn collect_routes(routes: &[HirExpr]) -> Result<Vec<RouteEntry>, RuntimeError> {
    let mut out = Vec::with_capacity(routes.len());
    for expr in routes {
        let HirExprKind::Route {
            method,
            path,
            handler,
            ..
        } = &expr.kind
        else {
            return Err(RuntimeError::native(
                "internal: @server routes slot contains non-Route HIR (analyzer contract violated)",
            ));
        };
        // handler 는 HirBlock 이지만 Interp::eval 은 HirExpr 을 받는다. 요청
        // 시점에 HirExprKind::Block 으로 감싸 평가하기 쉽도록 미리 변환.
        let handler_expr = HirExpr {
            kind: HirExprKind::Block(handler.clone()),
            ty: orv_hir::Type::Unknown,
            span: expr.span,
        };
        out.push(RouteEntry {
            method: method.clone(),
            path: path.clone(),
            handler: handler_expr,
            origin_id: origin_id("route", &format!("{method} {path}"), expr.span),
        });
    }
    Ok(out)
}

async fn serve_loop<S>(
    listener: TcpListener,
    routes: LocalRoutes,
    captured_env: LocalCapturedEnv,
    db: DbHandle,
    trace_state: Option<TraceState>,
    shutdown: S,
) -> Result<(), RuntimeError>
where
    S: std::future::Future<Output = ()>,
{
    // C_db: 서버 수명 동안 공유하는 DB handle. Server boot body가 `@db.wal`
    // 또는 `@db.load`로 구성한 persistence 설정도 같은 handle을 통해 route
    // handler에 전달된다.
    // shutdown 은 단일 해상도 이벤트라 `tokio::pin!` 로 고정해 `select!` 에서
    // `&mut` 참조로 폴링한다. 이렇게 해야 매 반복에서 future 를 소비하지 않고
    // 재진입이 가능하다.
    tokio::pin!(shutdown);
    let rate_limits = RateLimitState::default();
    loop {
        let (stream, peer) = tokio::select! {
            biased;
            // shutdown 우선. accept 가 동시에 준비되어도 먼저 빠져나간다.
            () = &mut shutdown => return Ok(()),
            accept_result = listener.accept() => match accept_result {
                Ok(pair) => pair,
                Err(e) => {
                    eprintln!("accept error: {e}");
                    continue;
                }
            }
        };
        let io = TokioIo::new(stream);
        let routes = routes.clone();
        let captured_env = captured_env.clone();
        let db = db.clone();
        let trace_state = trace_state.clone();
        let rate_limits = rate_limits.clone();
        let client_ip = peer.ip().to_string();
        let service = service_fn(move |req| {
            let routes = routes.clone();
            let captured_env = captured_env.clone();
            let db = db.clone();
            let trace_state = trace_state.clone();
            let rate_limits = rate_limits.clone();
            let client_ip = client_ip.clone();
            async move {
                Ok::<_, Infallible>(
                    handle_request(
                        req,
                        routes,
                        captured_env,
                        db,
                        client_ip,
                        trace_state,
                        rate_limits,
                    )
                    .await,
                )
            }
        });
        tokio::task::spawn_local(async move {
            if let Err(e) = hyper::server::conn::http1::Builder::new()
                .keep_alive(false)
                .serve_connection(io, service)
                .await
            {
                eprintln!("connection error: {e}");
            }
        });
    }
}

#[allow(clippy::future_not_send)]
async fn serve_loop_with_request_trace_file<S>(
    listener: TcpListener,
    routes: LocalRoutes,
    captured_env: LocalCapturedEnv,
    db: DbHandle,
    trace_state: Option<TraceState>,
    request_trace_path: Option<std::path::PathBuf>,
    shutdown: S,
) -> Result<(), RuntimeError>
where
    S: std::future::Future<Output = ()>,
{
    let trace_state =
        trace_state.or_else(|| request_trace_path.as_ref().map(|_| TraceState::new()));
    serve_loop(
        listener,
        routes,
        captured_env,
        db,
        trace_state.clone(),
        shutdown,
    )
    .await?;
    if let (Some(path), Some(trace_state)) = (request_trace_path, trace_state) {
        write_request_trace_file(&path, &trace_state.frames())?;
    }
    Ok(())
}

fn runtime_request_trace_path_from_env() -> Option<std::path::PathBuf> {
    std::env::var_os(ORV_RUNTIME_REQUEST_TRACE_PATH_ENV)
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
}

#[allow(clippy::too_many_lines)]
async fn handle_request(
    req: Request<Incoming>,
    routes: LocalRoutes,
    captured_env: LocalCapturedEnv,
    db: DbHandle,
    client_ip: String,
    trace_state: Option<TraceState>,
    rate_limits: RateLimitState,
) -> ServerResponse {
    let method = req.method().as_str().to_string();
    let uri = req.uri().clone();
    // hyper 는 요청 경로의 trailing `/` 를 그대로 보존한다. curl 사용자가 흔히
    // `/users/42/` 로 쳐도 `/users/:id` 매치 대상이 되도록 정규화한다. 루트
    // `/` 자체는 예외 — 빈 문자열이 되면 매칭 규칙이 무의미해진다.
    let path_raw = uri.path().to_string();
    let path = normalize_path(&path_raw);
    let query = uri.query().map(parse_query).unwrap_or_default();
    let headers: HashMap<String, String> = req
        .headers()
        .iter()
        .map(|(k, v)| (k.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
        .collect();

    let (body_value, raw_body) = match request_body_value(req, &headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };

    if method == "GET" && path == ORV_TRACE_EVENTS_PATH {
        if let Some(trace_state) = trace_state.as_ref() {
            return request_trace_events_response(trace_state);
        }
    }

    // 라우트 매칭 — 선형 탐색. method 는 "*" wildcard 허용.
    let mut matched: Option<(RouteEntry, HashMap<String, String>)> = None;
    for entry in routes.iter() {
        if entry.method != "*" && entry.method != method {
            continue;
        }
        if let Some(params) = match_route(&entry.path, &path) {
            matched = Some((entry.clone(), params));
            break;
        }
    }

    let Some((entry, params)) = matched else {
        let response = plain_response(StatusCode::NOT_FOUND, "Not Found".into());
        record_request_frame(
            trace_state.as_ref(),
            ServerRequestFrame {
                method,
                path,
                route_method: None,
                route_path: None,
                route_origin_id: None,
                status: response.status().as_u16(),
                params: HashMap::new(),
                query,
                body: request_body_display(&body_value),
            },
        );
        return response;
    };

    if let Some(policy) = default_rate_limit_policy(&entry.method, &entry.path) {
        let key = format!("{}:{}:{client_ip}", entry.method, entry.path);
        if !rate_limits.check(&key, policy.limit, policy.window) {
            let response = plain_response(
                StatusCode::TOO_MANY_REQUESTS,
                "Too Many Requests: route rate limit exceeded".into(),
            );
            record_request_frame(
                trace_state.as_ref(),
                ServerRequestFrame {
                    method,
                    path,
                    route_method: Some(entry.method),
                    route_path: Some(entry.path),
                    route_origin_id: Some(entry.origin_id),
                    status: response.status().as_u16(),
                    params,
                    query,
                    body: request_body_display(&body_value),
                },
            );
            return response;
        }
    }

    let frame_method = method.clone();
    let frame_path = path.clone();
    let frame_params = params.clone();
    let frame_query = query.clone();
    let frame_body = request_body_display(&body_value);
    let ctx = RequestCtx {
        method,
        path,
        ip: client_ip,
        params,
        query,
        headers,
        raw_body,
        body: body_value,
    };

    // handler 평가는 동기. stdout 은 버리는 버퍼로 흘려 — `@out` 은 서버
    // 콘솔이 아니라 요청 단위로 캡처해 반환 헤더에 싣는 편이 정석이지만
    // MVP 는 단순히 버린다.
    let mut sink = Vec::<u8>::new();
    let outcome = match run_handler_with_request_in_env(
        &entry.handler,
        ctx,
        captured_env.snapshot(),
        db.clone(),
        &mut sink,
    ) {
        Ok(o) => o,
        Err(e) => {
            // 스택 트레이스나 내부 메시지 누출을 막기 위해 일반 메시지만.
            eprintln!("handler runtime error: {e}");
            return plain_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Internal Server Error".into(),
            );
        }
    };

    // A3 하이브리드: server-level 바인딩 재할당 경고는 stderr 로 흘린다.
    // 프로덕션 로깅 레이어가 없는 MVP 이므로 단순 eprintln.
    for w in &outcome.warnings {
        eprintln!("{w}");
    }

    let response = match outcome.response {
        Some(resp) => {
            let extra_headers = response_extra_headers(&entry.method, &entry.path, &resp);
            response_from_respond(resp, Some(&entry.origin_id), &extra_headers)
        }
        None => default_response(&outcome.value, Some(&entry.origin_id)),
    };
    record_request_frame(
        trace_state.as_ref(),
        ServerRequestFrame {
            method: frame_method,
            path: frame_path,
            route_method: Some(entry.method),
            route_path: Some(entry.path),
            route_origin_id: Some(entry.origin_id),
            status: response.status().as_u16(),
            params: frame_params,
            query: frame_query,
            body: frame_body,
        },
    );
    response
}

async fn request_body_value(
    req: Request<Incoming>,
    headers: &HashMap<String, String>,
) -> Result<(Value, String), ServerResponse> {
    // `Limited` 로 크기 상한을 걸어 거대 POST 의 메모리 폭주를 차단. 초과 시
    // 413 응답.
    let limited = Limited::new(req.into_body(), MAX_BODY_BYTES);
    let body_bytes = match limited.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(e) => {
            let msg = format!("{e}");
            if msg.contains("length limit exceeded") {
                return Err(plain_response(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    format!("request body exceeds {MAX_BODY_BYTES} bytes"),
                ));
            }
            return Err(plain_response(
                StatusCode::BAD_REQUEST,
                format!("failed to read request body: {msg}"),
            ));
        }
    };
    let content_type = headers
        .get("content-type")
        .map(|ct| ct.to_ascii_lowercase())
        .unwrap_or_default();
    let is_json = content_type.starts_with("application/json");
    let is_form_urlencoded = content_type.starts_with("application/x-www-form-urlencoded");
    let raw_body = String::from_utf8_lossy(&body_bytes).into_owned();
    if body_bytes.is_empty() {
        Ok((Value::Void, raw_body))
    } else if is_json {
        let body = serde_json::from_slice::<serde_json::Value>(&body_bytes)
            .map(json_to_value)
            .map_err(|e| {
                plain_response(StatusCode::BAD_REQUEST, format!("invalid JSON body: {e}"))
            })?;
        Ok((body, raw_body))
    } else if is_form_urlencoded {
        Ok((
            Value::Object(
                parse_query(&raw_body)
                    .into_iter()
                    .map(|(key, value)| (key, Value::Str(value)))
                    .collect(),
            ),
            raw_body,
        ))
    } else {
        Ok((Value::Str(raw_body.clone()), raw_body))
    }
}

fn record_request_frame(trace_state: Option<&TraceState>, frame: ServerRequestFrame) {
    if let Some(trace_state) = trace_state {
        trace_state.record(frame);
    }
}

fn request_trace_events_response(trace_state: &TraceState) -> ServerResponse {
    let frames = trace_state.frames();
    let body = request_trace_events_body(&frames);
    let rx = trace_state.subscribe();
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "text/event-stream")
        .header("cache-control", "no-cache")
        .body(RuntimeBody::trace(body, rx))
        .expect("valid trace event stream response")
}

fn request_trace_events_body(frames: &[ServerRequestFrame]) -> String {
    let mut body = String::new();
    let payload = serde_json::to_string(&request_trace_json(frames)).unwrap_or_default();
    body.push_str("event: orv:trace\n");
    body.push_str("data: ");
    body.push_str(&payload);
    body.push_str("\n\n");
    for (index, frame) in frames.iter().enumerate() {
        body.push_str(&request_trace_frame_event_body(index, frame));
    }
    body
}

fn request_trace_frame_event_body(index: usize, frame: &ServerRequestFrame) -> String {
    let payload =
        serde_json::to_string(&request_trace_frame_event_json(index, frame)).unwrap_or_default();
    format!("event: orv:trace.frame\ndata: {payload}\n\n")
}

/// Build the shared production request trace JSON document.
#[must_use]
pub fn request_trace_json(frames: &[ServerRequestFrame]) -> serde_json::Value {
    serde_json::json!({
        "schema_version": 1,
        "kind": "orv.production.trace",
        "frame_count": frames.len(),
        "frames": frames
            .iter()
            .map(request_frame_json)
            .collect::<Vec<_>>(),
    })
}

fn request_trace_frame_event_json(index: usize, frame: &ServerRequestFrame) -> serde_json::Value {
    serde_json::json!({
        "schema_version": 1,
        "kind": "orv.production.trace.frame",
        "index": index,
        "frame": request_frame_json(frame),
    })
}

/// Write captured request frames as an `orv.production.trace` JSON file.
///
/// # Errors
/// Returns a runtime error if a parent directory cannot be created or if the
/// JSON file cannot be serialized/written.
pub fn write_request_trace_file(
    path: &std::path::Path,
    frames: &[ServerRequestFrame],
) -> Result<(), RuntimeError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            RuntimeError::native(format!(
                "failed to create request trace directory {}: {e}",
                parent.display()
            ))
        })?;
    }
    let bytes = serde_json::to_vec_pretty(&request_trace_json(frames))
        .map_err(|e| RuntimeError::native(format!("failed to encode request trace JSON: {e}")))?;
    std::fs::write(path, bytes).map_err(|e| {
        RuntimeError::native(format!(
            "failed to write request trace file {}: {e}",
            path.display()
        ))
    })
}

fn request_frame_json(frame: &ServerRequestFrame) -> serde_json::Value {
    serde_json::json!({
        "method": &frame.method,
        "path": &frame.path,
        "status": frame.status,
        "route_method": frame.route_method.as_deref(),
        "route_path": frame.route_path.as_deref(),
        "route_origin_id": frame.route_origin_id.as_deref(),
        "params": &frame.params,
        "query": &frame.query,
        "body": &frame.body,
    })
}

fn request_body_display(value: &Value) -> String {
    if matches!(value, Value::Void) {
        return String::new();
    }
    serde_json::to_string(&value_to_json(value)).unwrap_or_default()
}

fn response_from_respond(
    resp: ResponseCtx,
    origin_id: Option<&str>,
    extra_headers: &[(String, String)],
) -> ServerResponse {
    let status = u16::try_from(resp.status)
        .ok()
        .and_then(|s| StatusCode::from_u16(s).ok())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

    // SPEC §11.9: `@redirect` 가 기록한 Location 이 있으면 body 없이
    // `Location:` 헤더 + 상태로 응답한다. payload/raw_body 는 무시.
    if let Some(loc) = resp.location {
        let builder = response_builder(status, origin_id).header("location", loc);
        return apply_extra_response_headers(builder, extra_headers)
            .body(RuntimeBody::full(Bytes::new()))
            .expect("valid response");
    }

    // A5a: `@serve` 가 기록한 raw body 는 JSON 경로를 우회하고 그대로 나간다.
    // body 금지 상태(204/304/1xx)에서도 파일은 있을 수 없는 조합이라 일반
    // 경로보다 먼저 잡는다.
    if let Some(raw) = resp.raw_body {
        let builder = response_builder(status, origin_id).header("content-type", raw.content_type);
        return apply_extra_response_headers(builder, extra_headers)
            .body(RuntimeBody::full(Bytes::from(raw.bytes)))
            .expect("valid response");
    }

    // RFC 상 body 가 허용되지 않는 상태(204/304/1xx)와 Void payload 는 항상
    // 빈 body 로 보낸다. SPEC 도 `@respond 204 {}` 에서 body 인코더 제거를
    // 기대하므로, payload 값과 무관하게 no-body 경로를 우선한다.
    if status_disallows_body(status) || matches!(resp.payload, Value::Void) {
        return apply_extra_response_headers(response_builder(status, origin_id), extra_headers)
            .body(RuntimeBody::full(Bytes::new()))
            .expect("valid response");
    }
    let json = value_to_json(&resp.payload);
    let body = serde_json::to_vec(&json).unwrap_or_else(|_| b"null".to_vec());
    let builder = response_builder(status, origin_id).header("content-type", "application/json");
    apply_extra_response_headers(builder, extra_headers)
        .body(RuntimeBody::full(Bytes::from(body)))
        .expect("valid response")
}

fn response_extra_headers(method: &str, path: &str, resp: &ResponseCtx) -> Vec<(String, String)> {
    login_session_cookie(method, path, resp)
        .into_iter()
        .map(|cookie| ("set-cookie".to_string(), cookie))
        .collect()
}

fn login_session_cookie(method: &str, path: &str, resp: &ResponseCtx) -> Option<String> {
    if method != "POST" || path != "/members/login" || !(200..300).contains(&resp.status) {
        return None;
    }
    let session = object_field_value(&resp.payload, "session")?;
    let session_id = object_field_value(session, "id")?;
    let cookie_value = cookie_scalar_value(session_id)?;
    Some(format!(
        "orv_session={cookie_value}; Path=/; Max-Age=86400; HttpOnly; SameSite=Lax; Secure"
    ))
}

fn cookie_scalar_value(value: &Value) -> Option<String> {
    let value = match value {
        Value::Int(id) => id.to_string(),
        Value::Str(id) if !id.is_empty() => id.clone(),
        _ => return None,
    };
    value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | '~'))
        .then_some(value)
}

fn object_field_value<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    let Value::Object(fields) = value else {
        return None;
    };
    fields
        .iter()
        .rev()
        .find_map(|(field, value)| (field == name).then_some(value))
}

fn apply_extra_response_headers(
    mut builder: hyper::http::response::Builder,
    headers: &[(String, String)],
) -> hyper::http::response::Builder {
    for (name, value) in headers {
        builder = builder.header(name.as_str(), value.as_str());
    }
    builder
}

fn status_disallows_body(status: StatusCode) -> bool {
    status.is_informational()
        || status == StatusCode::NO_CONTENT
        || status == StatusCode::NOT_MODIFIED
}

fn default_response(value: &Value, origin_id: Option<&str>) -> ServerResponse {
    // handler 가 `@respond` 없이 값으로 끝나면 그 값을 JSON 으로 200 응답.
    // Void 는 빈 200. 이렇게 하면 `@route GET /health { "ok" }` 같은 간단한
    // 핸들러가 그대로 동작한다.
    if matches!(value, Value::Void) {
        return response_builder(StatusCode::OK, origin_id)
            .body(RuntimeBody::full(Bytes::new()))
            .expect("valid response");
    }
    let json = value_to_json(value);
    let body = serde_json::to_vec(&json).unwrap_or_else(|_| b"null".to_vec());
    response_builder(StatusCode::OK, origin_id)
        .header("content-type", "application/json")
        .body(RuntimeBody::full(Bytes::from(body)))
        .expect("valid response")
}

fn response_builder(status: StatusCode, origin_id: Option<&str>) -> hyper::http::response::Builder {
    let mut builder = Response::builder().status(status);
    if let Some(origin_id) = origin_id {
        builder = builder.header(ORV_ORIGIN_ID_HEADER, origin_id);
    }
    builder
}

fn plain_response(status: StatusCode, body: String) -> ServerResponse {
    Response::builder()
        .status(status)
        .header("content-type", "text/plain; charset=utf-8")
        .body(RuntimeBody::full(Bytes::from(body)))
        .expect("valid response")
}

/// `?a=1&b=hello` 형태 쿼리 문자열을 맵으로.
///
/// SPEC §11.3 은 쿼리 디코딩 규칙을 깊게 정의하지 않는다. 적용 순서:
/// 1. `+` → space (application/x-www-form-urlencoded 관습. value 에만 적용해
///    key 의 literal `+` 는 그대로 두는 게 안전하지만, 키에 `+` 가 등장할 일
///    자체가 드물어 양쪽 모두 치환한다).
/// 2. percent-decoding — RFC 3986 `%HH` 두 자리 hex. 잘못된 시퀀스(`%ZZ`,
///    `%2`) 는 raw 로 보존해 요청을 거부하지 않는다 (best-effort 파싱).
/// 3. UTF-8 검증 — 디코딩 결과가 UTF-8 이 아니면 raw 문자열로 폴백.
pub(crate) fn parse_query(raw: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for pair in raw.split('&') {
        if pair.is_empty() {
            continue;
        }
        let mut it = pair.splitn(2, '=');
        let k = percent_decode_form(it.next().unwrap_or(""));
        let v = percent_decode_form(it.next().unwrap_or(""));
        out.insert(k, v);
    }
    out
}

/// application/x-www-form-urlencoded 규칙으로 한 토큰을 디코딩한다.
///
/// `+` → space → `%HH` → UTF-8 조립. `%HH` 가 잘못되면 해당 `%` 는 literal
/// 로 남기고 다음 문자부터 계속 스캔한다. 결과 바이트가 UTF-8 이 아니면
/// 입력을 그대로 반환한다.
fn percent_decode_form(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hi = hex_value(bytes[i + 1]);
                let lo = hex_value(bytes[i + 2]);
                match (hi, lo) {
                    (Some(h), Some(l)) => {
                        out.push((h << 4) | l);
                        i += 3;
                    }
                    _ => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| raw.to_string())
}

fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// 요청 경로의 trailing `/` 를 제거한다 (단 `/` 자체는 그대로 유지).
///
/// hyper 는 경로를 원문 그대로 전달해 `/users/42` 와 `/users/42/` 가 다른
/// 값이 된다. 대부분의 사용자는 두 형태를 동치로 기대하므로 여기서 정규화해
/// 라우트 매처가 동일하게 처리하도록 돕는다.
pub(crate) fn normalize_path(path: &str) -> String {
    if path == "/" {
        return path.to_string();
    }
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else {
        trimmed.to_string()
    }
}

/// 라우트 패턴(`/users/:id`) 과 실제 경로(`/users/42`) 를 segment 단위로 비교.
///
/// 매칭되면 `:param` 자리의 값을 맵으로 반환. 빈 segment(`//` 연속)는 분할
/// 그대로 보존한다.
///
/// 특수 패턴:
/// - `*` (catchall) — 패턴 전체가 단일 `"*"` 면 어떤 경로든 매치. SPEC §11.2
///   의 `@route GET * { @respond 404 ... }` 구문을 지원하기 위한 규칙.
///   params 는 비어 있다. 세그먼트 수준 wildcard(`/a/*`)는 이번 범위 밖.
pub(crate) fn match_route(pattern: &str, path: &str) -> Option<HashMap<String, String>> {
    if pattern == "*" {
        return Some(HashMap::new());
    }
    let pat_parts: Vec<&str> = pattern.split('/').collect();
    let path_parts: Vec<&str> = path.split('/').collect();

    // A2b: named wildcard suffix `:NAME*` — 패턴 마지막 세그먼트가 이 형태면
    // 앞쪽은 정확 매치, 그 이후의 모든 세그먼트는 `/` 로 join 해 `NAME` 에
    // 캡처. rest 는 최소 1개 세그먼트를 요구 (0 segments 는 일반 prefix 매치와
    // 모호해지므로 거부).
    if let Some(last) = pat_parts.last() {
        if let Some(name) = last.strip_prefix(':').and_then(|n| n.strip_suffix('*')) {
            // 앞쪽 세그먼트 수가 path 의 세그먼트 수보다 작아야 rest 가
            // 최소 1개 존재한다. `:rest*` 는 필수 캡처이므로 같거나 적으면 실패.
            let prefix_len = pat_parts.len() - 1;
            if path_parts.len() <= prefix_len {
                return None;
            }
            let mut params = HashMap::new();
            for (pp, ap) in pat_parts.iter().take(prefix_len).zip(path_parts.iter()) {
                if !match_route_segment(pp, ap, &mut params) {
                    return None;
                }
            }
            let rest = path_parts[prefix_len..].join("/");
            params.insert(name.to_string(), rest);
            return Some(params);
        }
    }

    if pat_parts.len() != path_parts.len() {
        return None;
    }
    let mut params = HashMap::new();
    for (pp, ap) in pat_parts.iter().zip(path_parts.iter()) {
        if !match_route_segment(pp, ap, &mut params) {
            return None;
        }
    }
    Some(params)
}

fn match_route_segment(
    pattern_segment: &str,
    path_segment: &str,
    params: &mut HashMap<String, String>,
) -> bool {
    let Some((name, suffix)) = route_param_segment(pattern_segment) else {
        return pattern_segment == path_segment;
    };
    let Some(value) = path_segment.strip_suffix(suffix) else {
        return false;
    };
    params.insert(name.to_string(), value.to_string());
    true
}

fn route_param_segment(segment: &str) -> Option<(&str, &str)> {
    let body = segment.strip_prefix(':')?;
    let end = body
        .char_indices()
        .find_map(|(index, ch)| (!route_param_name_char(ch)).then_some(index))
        .unwrap_or(body.len());
    (end > 0).then_some((&body[..end], &body[end..]))
}

const fn route_param_name_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_'
}

/// orv [`Value`] → `serde_json::Value`.
///
/// 변환 규칙 (MVP):
/// - Int/Float/Bool/Str → scalar JSON.
/// - Void → `null` (SPEC §11.4 가 Void payload 를 "빈 body" 로 규정하지만
///   직렬화 경로에 들어올 일이 없도록 상위에서 분기. 안전망으로 null.).
/// - Array → JSON array (재귀).
/// - Object → JSON object (필드 순서 보존은 serde_json::Map 이 기본 BTreeMap
///   이 아니라 `preserve_order` feature 가 꺼져 있으면 알파벳 순이 될 수
///   있다. 테스트가 순서에 의존하지 않도록 값만 비교).
/// - Function/Lambda/BoundMethod → 문자열로 표시 (SPEC 은 직렬화 불가를
///   규정하지만 panic 대신 문자열로 떨어뜨려 진단이 쉽다).
pub(crate) fn value_to_json(v: &Value) -> serde_json::Value {
    use serde_json::Value as J;
    match v {
        Value::Int(n) => J::from(*n),
        Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(J::Number)
            .unwrap_or(J::Null),
        Value::Bool(b) => J::Bool(*b),
        Value::Str(s) => J::String(s.clone()),
        Value::Regex { pattern, flags } => J::String(format!("r\"{pattern}\"{flags}")),
        Value::Void => J::Null,
        Value::Array(items) => J::Array(items.iter().map(value_to_json).collect()),
        Value::Tuple(elems) => J::Array(elems.iter().map(value_to_json).collect()),
        Value::Object(fields) => {
            let mut map = serde_json::Map::new();
            for (k, v) in fields {
                map.insert(k.clone(), value_to_json(v));
            }
            J::Object(map)
        }
        Value::Function(f) => J::String(format!("<function {}>", f.name.name)),
        Value::Lambda(_) => J::String("<lambda>".into()),
        Value::BoundMethod { method, .. } => J::String(format!("<method {method}>")),
        Value::Db(_) => J::String("<db>".into()),
        Value::TypeName(n) => J::String(format!("<type {n}>")),
        Value::Builtin(n) => J::String(format!("<builtin {n}>")),
    }
}

/// `serde_json::Value` → orv [`Value`]. 요청 body JSON 파싱 경로에서만 사용.
///
/// 숫자 매핑 규칙:
/// - `i64` 범위면 `Value::Int`.
/// - `f64` 로 표현 가능한 부동소수점이면 `Value::Float`.
/// - `u64::MAX` 쪽으로 i64 상한을 넘는 큰 정수는 **precision 손실을 피하려고
///   원문 문자열을 `Value::Str`** 로 보존한다. 사용자가 명시적으로 처리하도록
///   미는 선택 — 조용히 f64 로 몰아서 `9999999999999999999` → `1e19` 가 되는
///   경우를 막는다.
fn json_to_value(j: serde_json::Value) -> Value {
    use serde_json::Value as J;
    match j {
        J::Null => Value::Void,
        J::Bool(b) => Value::Bool(b),
        J::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Int(i)
            } else if n.is_f64() {
                // 명시적으로 소수점이 있는 표기면 float 로 받는다.
                n.as_f64().map(Value::Float).unwrap_or(Value::Void)
            } else {
                // i64 를 넘는 정수(u64 상단)는 원문을 보존.
                Value::Str(n.to_string())
            }
        }
        J::String(s) => Value::Str(s),
        J::Array(items) => Value::Array(items.into_iter().map(json_to_value).collect()),
        J::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| (k, json_to_value(v)))
                .collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hmac::{Hmac, Mac};
    use hyper::client::conn::http1 as client_http1;
    use hyper_util::rt::TokioIo;
    use orv_analyzer::lower;
    use orv_diagnostics::{FileId, Span};
    use orv_hir::{HirExpr, HirExprKind, HirProgram, HirStmt};
    use orv_resolve::resolve;
    use orv_syntax::{lex, parse};
    use sha2::Sha256;
    use tokio::net::TcpStream;

    // --- 단위: match_route / parse_query / value_to_json ---

    #[test]
    fn request_trace_json_uses_production_trace_schema() {
        let frame = ServerRequestFrame {
            method: "GET".to_string(),
            path: "/users/42".to_string(),
            route_method: Some("GET".to_string()),
            route_path: Some("/users/:id".to_string()),
            route_origin_id: Some("ori_route_user".to_string()),
            status: 200,
            params: HashMap::from([("id".to_string(), "42".to_string())]),
            query: HashMap::from([("tab".to_string(), "orders".to_string())]),
            body: "{\"active\":true}".to_string(),
        };

        let trace = request_trace_json(&[frame]);

        assert_eq!(trace["schema_version"], 1);
        assert_eq!(trace["kind"], "orv.production.trace");
        assert_eq!(trace["frame_count"], 1);
        assert_eq!(trace["frames"][0]["method"], "GET");
        assert_eq!(trace["frames"][0]["path"], "/users/42");
        assert_eq!(trace["frames"][0]["status"], 200);
        assert_eq!(trace["frames"][0]["route_method"], "GET");
        assert_eq!(trace["frames"][0]["route_path"], "/users/:id");
        assert_eq!(trace["frames"][0]["route_origin_id"], "ori_route_user");
        assert_eq!(trace["frames"][0]["params"]["id"], "42");
        assert_eq!(trace["frames"][0]["query"]["tab"], "orders");
        assert_eq!(trace["frames"][0]["body"], "{\"active\":true}");
    }

    #[test]
    fn write_request_trace_file_creates_parent_dirs() {
        let dir =
            std::env::temp_dir().join(format!("orv-runtime-trace-file-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("trace").join("requests.json");
        let frame = ServerRequestFrame {
            method: "POST".to_string(),
            path: "/orders".to_string(),
            route_method: Some("POST".to_string()),
            route_path: Some("/orders".to_string()),
            route_origin_id: Some("ori_route_order".to_string()),
            status: 201,
            params: HashMap::new(),
            query: HashMap::new(),
            body: "{\"sku\":\"book\"}".to_string(),
        };

        write_request_trace_file(&path, &[frame]).expect("write trace");

        let trace: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read trace file"))
                .expect("trace json");
        assert_eq!(trace["kind"], "orv.production.trace");
        assert_eq!(trace["frames"][0]["method"], "POST");
        assert_eq!(trace["frames"][0]["status"], 201);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn match_route_static_equal() {
        let m = match_route("/ping", "/ping").unwrap();
        assert!(m.is_empty());
    }

    #[test]
    fn match_route_static_mismatch_returns_none() {
        assert!(match_route("/ping", "/pong").is_none());
    }

    #[test]
    fn match_route_param_captures_value() {
        let m = match_route("/users/:id", "/users/42").unwrap();
        assert_eq!(m.get("id"), Some(&"42".to_string()));
    }

    #[test]
    fn match_route_param_captures_value_with_static_suffix() {
        let m = match_route("/calendar/:userId.ics", "/calendar/42.ics").unwrap();
        assert_eq!(m.get("userId"), Some(&"42".to_string()));
        assert!(match_route("/calendar/:userId.ics", "/calendar/42.json").is_none());
    }

    #[test]
    fn match_route_multiple_params() {
        let m = match_route("/users/:uid/posts/:pid", "/users/7/posts/hello").unwrap();
        assert_eq!(m.get("uid"), Some(&"7".to_string()));
        assert_eq!(m.get("pid"), Some(&"hello".to_string()));
    }

    #[test]
    fn match_route_length_mismatch() {
        // segment 수가 다르면 단순 실패.
        assert!(match_route("/users/:id", "/users/42/extra").is_none());
        assert!(match_route("/users/:id", "/users").is_none());
    }

    #[test]
    fn match_route_catchall_star_matches_any_path() {
        // SPEC §11.2: `@route GET *` 은 어느 경로든 잡는다. 매처 단에서 path
        // 가 "*" 면 params 없이 success.
        assert_eq!(match_route("*", "/").unwrap().len(), 0);
        assert_eq!(match_route("*", "/some/deep/path").unwrap().len(), 0);
        assert_eq!(match_route("*", "/users/42/things/99").unwrap().len(), 0);
    }

    #[test]
    fn match_route_named_wildcard_captures_rest_path() {
        // A2b: `/assets/:rest*` 는 `/assets/` 이후의 모든 세그먼트를 `/` 로
        // 이어 붙여 `rest` 에 캡처.
        let p = match_route("/assets/:rest*", "/assets/foo/bar.png").unwrap();
        assert_eq!(p.get("rest"), Some(&"foo/bar.png".to_string()));

        // 단일 세그먼트도 잡힌다.
        let p = match_route("/assets/:rest*", "/assets/favicon.ico").unwrap();
        assert_eq!(p.get("rest"), Some(&"favicon.ico".to_string()));
    }

    #[test]
    fn match_route_named_wildcard_requires_prefix_match() {
        // prefix(`/assets/`) 가 안 맞으면 실패.
        assert!(match_route("/assets/:rest*", "/other/foo").is_none());
    }

    #[test]
    fn match_route_named_wildcard_needs_at_least_one_segment() {
        // `/assets/:rest*` 에서 rest 는 최소 1개 세그먼트 — `/assets` 만 오면
        // 매치 실패 (rest 가 필수 파라미터).
        assert!(match_route("/assets/:rest*", "/assets").is_none());
    }

    #[test]
    fn match_route_named_wildcard_combined_with_leading_params() {
        // `/api/:ver/files/:rest*` 처럼 앞쪽 :param 과 조합.
        let p = match_route("/api/:ver/files/:rest*", "/api/v1/files/a/b/c.txt").unwrap();
        assert_eq!(p.get("ver"), Some(&"v1".to_string()));
        assert_eq!(p.get("rest"), Some(&"a/b/c.txt".to_string()));
    }

    #[test]
    fn normalize_path_strips_trailing_slash() {
        assert_eq!(normalize_path("/users/42/"), "/users/42");
        assert_eq!(normalize_path("/users/42"), "/users/42");
    }

    #[test]
    fn normalize_path_preserves_root() {
        // `/` 자체는 빈 문자열이 되면 의미가 무너지므로 예외.
        assert_eq!(normalize_path("/"), "/");
        assert_eq!(normalize_path("///"), "/");
    }

    #[test]
    fn parse_query_basic() {
        let q = parse_query("a=1&b=hello");
        assert_eq!(q.get("a"), Some(&"1".to_string()));
        assert_eq!(q.get("b"), Some(&"hello".to_string()));
    }

    #[test]
    fn parse_query_plus_becomes_space() {
        let q = parse_query("msg=hello+world");
        assert_eq!(q.get("msg"), Some(&"hello world".to_string()));
    }

    #[test]
    fn parse_query_empty_returns_empty() {
        assert!(parse_query("").is_empty());
    }

    #[test]
    fn parse_query_percent_decodes_value() {
        // RFC 3986 percent-encoding: %20 → space, %26 → &, %3D → =.
        let q = parse_query("q=hello%20world&amp=%26&eq=%3D");
        assert_eq!(q.get("q"), Some(&"hello world".to_string()));
        assert_eq!(q.get("amp"), Some(&"&".to_string()));
        assert_eq!(q.get("eq"), Some(&"=".to_string()));
    }

    #[test]
    fn parse_query_percent_decodes_key() {
        // 드물지만 key 도 encoded 될 수 있다 (`foo bar=1` → `foo%20bar=1`).
        let q = parse_query("foo%20bar=1");
        assert_eq!(q.get("foo bar"), Some(&"1".to_string()));
    }

    #[test]
    fn parse_query_percent_decodes_utf8() {
        // `안녕` UTF-8 = E0 95 88 EB 85 95 (3+3 바이트). percent-encoded 로 오면
        // 바이트 시퀀스를 재조립해 UTF-8 문자열로 복원해야 한다.
        let q = parse_query("name=%EC%95%88%EB%85%95");
        assert_eq!(q.get("name"), Some(&"안녕".to_string()));
    }

    #[test]
    fn parse_query_plus_and_percent_mix() {
        // `+` 는 space, `%2B` 는 literal `+`. 둘이 한 value 에 섞여도 구분돼야 한다.
        let q = parse_query("x=a+b%2Bc");
        assert_eq!(q.get("x"), Some(&"a b+c".to_string()));
    }

    #[test]
    fn parse_query_malformed_percent_kept_raw() {
        // `%ZZ` 같이 잘못된 encoding 은 raw 로 보존한다 (400 대신 best-effort).
        // SPEC §11.3 에 명시 규칙이 없어 MVP 는 관대한 파싱 채택.
        let q = parse_query("x=%ZZ&y=%2");
        assert_eq!(q.get("x"), Some(&"%ZZ".to_string()));
        assert_eq!(q.get("y"), Some(&"%2".to_string()));
    }

    #[test]
    fn value_to_json_scalars() {
        assert_eq!(value_to_json(&Value::Int(42)), serde_json::json!(42));
        assert_eq!(value_to_json(&Value::Bool(true)), serde_json::json!(true));
        assert_eq!(
            value_to_json(&Value::Str("hi".into())),
            serde_json::json!("hi")
        );
        assert_eq!(value_to_json(&Value::Void), serde_json::Value::Null);
    }

    #[test]
    fn value_to_json_object_roundtrip() {
        let v = Value::Object(vec![
            ("id".into(), Value::Int(1)),
            ("name".into(), Value::Str("alice".into())),
        ]);
        let j = value_to_json(&v);
        assert_eq!(j["id"], serde_json::json!(1));
        assert_eq!(j["name"], serde_json::json!("alice"));
    }

    #[test]
    fn value_to_json_nested_array_of_objects() {
        let v = Value::Array(vec![
            Value::Object(vec![("n".into(), Value::Int(1))]),
            Value::Object(vec![("n".into(), Value::Int(2))]),
        ]);
        let j = value_to_json(&v);
        assert_eq!(j[0]["n"], serde_json::json!(1));
        assert_eq!(j[1]["n"], serde_json::json!(2));
    }

    #[test]
    fn json_to_value_preserves_big_integers_as_string() {
        // 9_999_999_999_999_999_999 는 i64::MAX(9_223_372_036_854_775_807)를
        // 넘고, f64 로 몰면 표현이 어긋난다. 원문 그대로 Value::Str 로 보존.
        let j: serde_json::Value = serde_json::from_str("9999999999999999999").expect("parse");
        match json_to_value(j) {
            Value::Str(s) => assert_eq!(s, "9999999999999999999"),
            other => panic!("expected Str for big int, got {other:?}"),
        }
    }

    #[test]
    fn json_to_value_int_within_i64_range() {
        let j: serde_json::Value = serde_json::from_str("42").expect("parse");
        match json_to_value(j) {
            Value::Int(n) => assert_eq!(n, 42),
            other => panic!("expected Int, got {other:?}"),
        }
    }

    #[test]
    fn json_to_value_float_with_decimal() {
        // `1.5` 는 float — i64 가 아니므로 Float 로 떨어진다.
        let j: serde_json::Value = serde_json::from_str("1.5").expect("parse");
        match json_to_value(j) {
            Value::Float(f) => assert!((f - 1.5).abs() < f64::EPSILON),
            other => panic!("expected Float, got {other:?}"),
        }
    }

    // --- 통합: 실제 hyper 서버에 HTTP 요청을 쏴서 응답 검증 ---
    //
    // 모든 통합 테스트는 `#[tokio::test]` (멀티스레드 기본) 로 돌린다.
    // `spawn_for_test` 가 accept 루프를 별도 task 로 띄우고, 테스트는 클라이언트
    // TcpStream + hyper client::conn 으로 요청을 쏜다. 테스트 종료 시
    // `handle.abort()` 로 루프 task 를 정리.

    #[derive(Debug)]
    struct ServerTestCase {
        listen: Option<Box<HirExpr>>,
        routes: Vec<HirExpr>,
        body_stmts: Vec<HirStmt>,
        captured_env: HashMap<NameId, Value>,
    }

    fn lower_src(src: &str) -> HirProgram {
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
        lower(&pr.program, &resolved)
    }

    /// orv 소스에서 첫 `@server` 표현식과 그 직전까지의 캡처 환경을 뽑아낸다.
    ///
    /// top-level `let`/`const`/`function` 선언은 production 경로와 같은 방식으로
    /// 먼저 실행해 `@server` 의 captured env 에 담는다.
    fn extract_server_case(src: &str) -> ServerTestCase {
        let hir = lower_src(src);
        let server_idx = hir
            .items
            .iter()
            .position(|stmt| {
                matches!(
                    stmt,
                    HirStmt::Expr(HirExpr {
                        kind: HirExprKind::Server { .. },
                        ..
                    })
                )
            })
            .expect("expected top-level @server expression");

        let captured_env = if server_idx == 0 {
            HashMap::new()
        } else {
            let prefix = HirProgram {
                items: hir.items[..server_idx].to_vec(),
                span: hir.items[0].span().join(hir.items[server_idx - 1].span()),
            };
            let mut sink = Vec::new();
            crate::interp::run_with_writer_in_env(&prefix, HashMap::new(), &mut sink)
                .expect("prefix program should execute")
        };

        let HirStmt::Expr(expr) = &hir.items[server_idx] else {
            panic!("expected server expr");
        };
        let HirExprKind::Server {
            listen,
            routes,
            body_stmts,
        } = &expr.kind
        else {
            panic!("expected Server variant");
        };
        ServerTestCase {
            listen: listen.clone(),
            routes: routes.clone(),
            body_stmts: body_stmts.clone(),
            captured_env,
        }
    }

    const TEST_ORIGIN_HEADER: &str = "x-orv-origin-id";

    fn stripe_test_signature(secret: &str, timestamp: &str, payload: &str) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac key");
        mac.update(format!("{timestamp}.{payload}").as_bytes());
        let digest = mac.finalize().into_bytes();
        let mut hex = String::with_capacity(digest.len() * 2);
        for byte in digest {
            use std::fmt::Write as _;
            write!(&mut hex, "{byte:02x}").expect("write hex");
        }
        format!("t={timestamp},v1={hex}")
    }

    fn expected_origin_id(kind: &str, name: &str, span: Span) -> String {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for byte in kind
            .as_bytes()
            .iter()
            .chain(name.as_bytes())
            .copied()
            .chain(span.file.index().to_le_bytes())
            .chain(span.range.start.to_le_bytes())
            .chain(span.range.end.to_le_bytes())
        {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        format!("ori_{hash:016x}")
    }

    /// 요청을 쏘고 (status, content-type, origin id, body 바이트) 튜플로 돌려준다.
    ///
    /// Request body 는 `body` 가 `Some` 이면 application/json 으로 보낸다.
    async fn send_request_full(
        addr: SocketAddr,
        method: &str,
        path: &str,
        body: Option<String>,
    ) -> (
        u16,
        Option<String>,
        Option<String>,
        HashMap<String, String>,
        Vec<u8>,
    ) {
        let stream = TcpStream::connect(addr).await.expect("connect");
        let io = TokioIo::new(stream);
        let (mut sender, conn) = client_http1::handshake(io).await.expect("handshake");
        // 커넥션 드라이버는 백그라운드 task 로.
        tokio::spawn(async move {
            let _ = conn.await;
        });

        let uri: hyper::Uri = path.parse().expect("uri");
        // body 가 없으면 빈 Full<Bytes> 로 통일 — 핸드셰이크 센더가 단일 body
        // 타입만 받으므로 if/else 분기에서 타입을 섞을 수 없다.
        let (bytes, has_body) =
            body.map_or_else(|| (Bytes::new(), false), |b| (Bytes::from(b), true));
        let mut builder = Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "localhost");
        if has_body {
            builder = builder.header("content-type", "application/json");
        }
        let req = builder.body(Full::new(bytes)).expect("build req");
        let resp = sender.send_request(req).await.expect("send");

        let status = resp.status().as_u16();
        let ct = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(std::string::ToString::to_string);
        let origin = resp
            .headers()
            .get(TEST_ORIGIN_HEADER)
            .and_then(|v| v.to_str().ok())
            .map(std::string::ToString::to_string);
        let headers = resp
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|value| (name.as_str().to_string(), value.to_string()))
            })
            .collect();
        let bytes = resp.collect().await.expect("body").to_bytes().to_vec();
        (status, ct, origin, headers, bytes)
    }

    async fn open_trace_event_stream(
        addr: SocketAddr,
    ) -> (u16, Option<String>, hyper::body::Incoming) {
        let stream = TcpStream::connect(addr).await.expect("connect");
        let io = TokioIo::new(stream);
        let (mut sender, conn) = client_http1::handshake(io).await.expect("handshake");
        tokio::spawn(async move {
            let _ = conn.await;
        });

        let req = Request::builder()
            .method("GET")
            .uri("/__orv/trace/events")
            .header("host", "localhost")
            .body(Full::new(Bytes::new()))
            .expect("build req");
        let resp = sender.send_request(req).await.expect("send");
        let status = resp.status().as_u16();
        let ct = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(std::string::ToString::to_string);
        (status, ct, resp.into_body())
    }

    async fn send_request_with_content_type(
        addr: SocketAddr,
        method: &str,
        path: &str,
        body: String,
        content_type: &str,
    ) -> (u16, Option<String>, Vec<u8>) {
        let stream = TcpStream::connect(addr).await.expect("connect");
        let io = TokioIo::new(stream);
        let (mut sender, conn) = client_http1::handshake(io).await.expect("handshake");
        tokio::spawn(async move {
            let _ = conn.await;
        });

        let uri: hyper::Uri = path.parse().expect("uri");
        let req = Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "localhost")
            .header("content-type", content_type)
            .body(Full::new(Bytes::from(body)))
            .expect("build req");
        let resp = sender.send_request(req).await.expect("send");
        let status = resp.status().as_u16();
        let ct = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(std::string::ToString::to_string);
        let bytes = resp.collect().await.expect("body").to_bytes().to_vec();
        (status, ct, bytes)
    }

    async fn send_request_with_headers(
        addr: SocketAddr,
        method: &str,
        path: &str,
        body: Option<String>,
        headers: &[(&str, &str)],
    ) -> (u16, Option<String>, Vec<u8>) {
        let stream = TcpStream::connect(addr).await.expect("connect");
        let io = TokioIo::new(stream);
        let (mut sender, conn) = client_http1::handshake(io).await.expect("handshake");
        tokio::spawn(async move {
            let _ = conn.await;
        });

        let uri: hyper::Uri = path.parse().expect("uri");
        let (bytes, has_body) =
            body.map_or_else(|| (Bytes::new(), false), |b| (Bytes::from(b), true));
        let mut builder = Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "localhost");
        if has_body {
            builder = builder.header("content-type", "application/json");
        }
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let req = builder.body(Full::new(bytes)).expect("build req");
        let resp = sender.send_request(req).await.expect("send");
        let status = resp.status().as_u16();
        let ct = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(std::string::ToString::to_string);
        let bytes = resp.collect().await.expect("body").to_bytes().to_vec();
        (status, ct, bytes)
    }

    /// 요청을 쏘고 (status, content-type, body 바이트) 튜플로 돌려준다.
    ///
    /// Request body 는 `body` 가 `Some` 이면 application/json 으로 보낸다.
    async fn send_request(
        addr: SocketAddr,
        method: &str,
        path: &str,
        body: Option<String>,
    ) -> (u16, Option<String>, Vec<u8>) {
        let (status, ct, _origin, _headers, bytes) =
            send_request_full(addr, method, path, body).await;
        (status, ct, bytes)
    }

    async fn run_on_localset<F: std::future::Future>(future: F) -> F::Output {
        tokio::task::LocalSet::new().run_until(future).await
    }

    #[tokio::test]
    async fn serves_simple_get_route_with_object_payload() {
        run_on_localset(async {
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_case(
                r#"@server {
                    @listen 0
                    @route GET /ping { @respond 200 { ok: true, msg: "pong" } }
                }"#,
            );
            let (addr, handle, _boot) = spawn_for_test(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                std::future::pending::<()>(),
            )
            .await
            .expect("spawn");

            let (status, ct, body) = send_request(addr, "GET", "/ping", None).await;
            assert_eq!(status, 200);
            assert_eq!(ct.as_deref(), Some("application/json"));
            let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
            assert_eq!(json["ok"], serde_json::json!(true));
            assert_eq!(json["msg"], serde_json::json!("pong"));

            handle.abort();
        })
        .await;
    }

    #[tokio::test]
    async fn writes_request_trace_file_on_graceful_shutdown() {
        run_on_localset(async {
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_case(
                "@server {
                    @listen 0
                    @route GET /ping { @respond 200 { ok: true } }
                }",
            );
            let dir = std::env::temp_dir()
                .join(format!("orv-runtime-serve-trace-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            let trace_path = dir.join("trace").join("requests.json");
            let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
            let (addr, handle, _boot) = spawn_for_test_with_request_trace_file(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                trace_path.clone(),
                async move {
                    let _ = shutdown_rx.await;
                },
            )
            .await
            .expect("spawn");

            let (status, _ct, _body) = send_request(addr, "GET", "/ping", None).await;
            assert_eq!(status, 200);
            shutdown_tx.send(()).expect("shutdown send");
            handle.await.expect("server task join");

            let trace: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(&trace_path).expect("read trace file"),
            )
            .expect("trace json");
            assert_eq!(trace["kind"], "orv.production.trace");
            assert_eq!(trace["frame_count"], 1);
            assert_eq!(trace["frames"][0]["method"], "GET");
            assert_eq!(trace["frames"][0]["path"], "/ping");
            assert_eq!(trace["frames"][0]["status"], 200);
            let _ = std::fs::remove_dir_all(&dir);
        })
        .await;
    }

    #[tokio::test]
    async fn request_trace_events_endpoint_streams_captured_frames() {
        run_on_localset(async {
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_case(
                "@server {
                    @listen 0
                    @route GET /ping { @respond 200 { ok: true } }
                }",
            );
            let dir = std::env::temp_dir()
                .join(format!("orv-runtime-trace-events-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            let trace_path = dir.join("trace").join("requests.json");
            let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
            let (addr, handle, _boot) = spawn_for_test_with_request_trace_file(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                trace_path,
                async move {
                    let _ = shutdown_rx.await;
                },
            )
            .await
            .expect("spawn");

            let (status, _, _) = send_request(addr, "GET", "/ping", None).await;
            assert_eq!(status, 200);
            let (stream_status, content_type, mut body) = open_trace_event_stream(addr).await;

            assert_eq!(stream_status, 200);
            assert_eq!(content_type.as_deref(), Some("text/event-stream"));
            let body = tokio::time::timeout(std::time::Duration::from_secs(1), body.frame())
                .await
                .expect("trace event timeout")
                .expect("trace event")
                .expect("trace event frame")
                .into_data()
                .expect("trace event data");
            let body = String::from_utf8(body.to_vec()).expect("event stream utf8");
            assert!(body.contains("event: orv:trace"));
            assert!(body.contains("\"kind\":\"orv.production.trace\""));
            assert!(body.contains("\"frame_count\":1"));
            assert!(body.contains("\"path\":\"/ping\""));
            shutdown_tx.send(()).expect("shutdown send");
            handle.await.expect("server task join");
            let _ = std::fs::remove_dir_all(dir);
        })
        .await;
    }

    #[tokio::test]
    async fn request_trace_events_endpoint_emits_per_frame_events() {
        run_on_localset(async {
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_case(
                "@server {
                    @listen 0
                    @route GET /ping { @respond 200 { ok: true } }
                }",
            );
            let dir = std::env::temp_dir().join(format!(
                "orv-runtime-trace-frame-events-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            let trace_path = dir.join("trace").join("requests.json");
            let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
            let (addr, handle, _boot) = spawn_for_test_with_request_trace_file(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                trace_path,
                async move {
                    let _ = shutdown_rx.await;
                },
            )
            .await
            .expect("spawn");

            assert_eq!(send_request(addr, "GET", "/ping", None).await.0, 200);
            assert_eq!(send_request(addr, "GET", "/ping", None).await.0, 200);
            let (_, _, mut body) = open_trace_event_stream(addr).await;

            let body = tokio::time::timeout(std::time::Duration::from_secs(1), body.frame())
                .await
                .expect("trace event timeout")
                .expect("trace event")
                .expect("trace event frame")
                .into_data()
                .expect("trace event data");
            let body = String::from_utf8(body.to_vec()).expect("event stream utf8");
            assert_eq!(body.matches("event: orv:trace.frame").count(), 2);
            assert!(body.contains("\"kind\":\"orv.production.trace.frame\""));
            assert!(body.contains("\"index\":0"));
            assert!(body.contains("\"index\":1"));
            shutdown_tx.send(()).expect("shutdown send");
            handle.await.expect("server task join");
            let _ = std::fs::remove_dir_all(dir);
        })
        .await;
    }

    #[tokio::test]
    async fn request_trace_events_endpoint_stays_open_for_new_frames() {
        run_on_localset(async {
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_case(
                "@server {
                    @listen 0
                    @route GET /ping { @respond 200 { ok: true } }
                }",
            );
            let dir = std::env::temp_dir().join(format!(
                "orv-runtime-trace-open-stream-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            let trace_path = dir.join("trace").join("requests.json");
            let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
            let (addr, handle, _boot) = spawn_for_test_with_request_trace_file(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                trace_path,
                async move {
                    let _ = shutdown_rx.await;
                },
            )
            .await
            .expect("spawn");

            let (status, content_type, mut body) = open_trace_event_stream(addr).await;
            assert_eq!(status, 200);
            assert_eq!(content_type.as_deref(), Some("text/event-stream"));
            let first = tokio::time::timeout(std::time::Duration::from_secs(1), body.frame())
                .await
                .expect("initial event timeout")
                .expect("initial event")
                .expect("initial event frame")
                .into_data()
                .expect("initial data");
            let first = String::from_utf8(first.to_vec()).expect("initial utf8");
            assert!(first.contains("event: orv:trace"));
            assert!(first.contains("\"frame_count\":0"));
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(20), body.frame())
                    .await
                    .is_err(),
                "trace stream ended before new request frames"
            );

            let ping = tokio::time::timeout(
                std::time::Duration::from_secs(1),
                send_request(addr, "GET", "/ping", None),
            )
            .await
            .expect("ping while trace stream is open");
            assert_eq!(ping.0, 200);
            let next = tokio::time::timeout(std::time::Duration::from_secs(1), body.frame())
                .await
                .expect("frame event timeout")
                .expect("frame event")
                .expect("frame event frame")
                .into_data()
                .expect("frame data");
            let next = String::from_utf8(next.to_vec()).expect("frame utf8");
            assert!(next.contains("event: orv:trace.frame"));
            assert!(next.contains("\"path\":\"/ping\""));
            shutdown_tx.send(()).expect("shutdown send");
            handle.await.expect("server task join");
            let _ = std::fs::remove_dir_all(dir);
        })
        .await;
    }

    #[tokio::test]
    async fn route_response_includes_route_origin_id_header() {
        run_on_localset(async {
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_case(
                r#"@server {
                    @listen 0
                    @route GET /ping { @respond 200 { ok: true } }
                }"#,
            );
            let route = routes
                .iter()
                .find(|expr| matches!(expr.kind, HirExprKind::Route { .. }))
                .expect("route");
            let expected_origin = expected_origin_id("route", "GET /ping", route.span);
            let (addr, handle, _boot) = spawn_for_test(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                std::future::pending::<()>(),
            )
            .await
            .expect("spawn");

            let (status, ct, origin, _headers, body) =
                send_request_full(addr, "GET", "/ping", None).await;
            assert_eq!(status, 200);
            assert_eq!(ct.as_deref(), Some("application/json"));
            assert_eq!(origin.as_deref(), Some(expected_origin.as_str()));
            let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
            assert_eq!(json["ok"], serde_json::json!(true));

            handle.abort();
        })
        .await;
    }

    #[tokio::test]
    async fn serves_route_with_path_param() {
        run_on_localset(async {
            // `@param` 은 전체 params object, 개별 값은 `.field` 로 접근 (C3 규약).
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_case(
                r#"@server {
                    @listen 0
                    @route GET /users/:id { @respond 200 { id: @param.id } }
                }"#,
            );
            let (addr, handle, _boot) = spawn_for_test(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                std::future::pending::<()>(),
            )
            .await
            .expect("spawn");

            let (status, _, body) = send_request(addr, "GET", "/users/42", None).await;
            assert_eq!(status, 200);
            let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
            // @param.id 는 문자열로 수집되므로 "42" (string).
            assert_eq!(json["id"], serde_json::json!("42"));

            handle.abort();
        })
        .await;
    }

    #[tokio::test]
    async fn serves_post_route_with_json_body_echo() {
        run_on_localset(async {
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_case(
                r#"@server {
                    @listen 0
                    @route POST /echo { @respond 201 { received: @body } }
                }"#,
            );
            let (addr, handle, _boot) = spawn_for_test(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                std::future::pending::<()>(),
            )
            .await
            .expect("spawn");

            let payload = r#"{"name":"alice","age":30}"#.to_string();
            let (status, _, body) = send_request(addr, "POST", "/echo", Some(payload)).await;
            assert_eq!(status, 201);
            let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
            assert_eq!(json["received"]["name"], serde_json::json!("alice"));
            assert_eq!(json["received"]["age"], serde_json::json!(30));

            handle.abort();
        })
        .await;
    }

    #[tokio::test]
    async fn checkout_route_has_reference_rate_limit() {
        run_on_localset(async {
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_case(
                r#"@server {
                    @listen 0
                    @route POST /checkout { @respond 200 { ok: true } }
                }"#,
            );
            let (addr, handle, _boot) = spawn_for_test(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                std::future::pending::<()>(),
            )
            .await
            .expect("spawn");

            for _ in 0..10 {
                let (status, _, _) =
                    send_request(addr, "POST", "/checkout", Some("{}".into())).await;
                assert_eq!(status, 200);
            }
            let (status, content_type, body) =
                send_request(addr, "POST", "/checkout", Some("{}".into())).await;
            assert_eq!(status, 429);
            assert_eq!(content_type.as_deref(), Some("text/plain; charset=utf-8"));
            let body = String::from_utf8(body).expect("rate-limit body utf8");
            assert!(body.contains("rate limit exceeded"));

            handle.abort();
        })
        .await;
    }

    #[tokio::test]
    async fn member_login_sets_reference_session_cookie_defaults() {
        run_on_localset(async {
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_case(
                r#"@server {
                    @listen 0
                    @route POST /members/login {
                        @respond 201 {
                          session: { id: 42, handle: "ada", status: "active" }
                        }
                    }
                }"#,
            );
            let (addr, handle, _boot) = spawn_for_test(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                std::future::pending::<()>(),
            )
            .await
            .expect("spawn");

            let (status, _, _, headers, _) =
                send_request_full(addr, "POST", "/members/login", Some("{}".into())).await;
            assert_eq!(status, 201);
            let cookie = headers.get("set-cookie").expect("set-cookie header");
            assert!(cookie.contains("orv_session=42"));
            assert!(cookie.contains("Path=/"));
            assert!(cookie.contains("Max-Age=86400"));
            assert!(cookie.contains("HttpOnly"));
            assert!(cookie.contains("SameSite=Lax"));
            assert!(cookie.contains("Secure"));

            handle.abort();
        })
        .await;
    }

    #[test]
    fn member_login_cookie_rejects_unsafe_session_id() {
        let resp = ResponseCtx {
            status: 201,
            payload: Value::Object(vec![(
                "session".to_string(),
                Value::Object(vec![(
                    "id".to_string(),
                    Value::Str("bad value; Path=/".to_string()),
                )]),
            )]),
            raw_body: None,
            location: None,
        };

        assert_eq!(login_session_cookie("POST", "/members/login", &resp), None);
    }

    #[tokio::test]
    async fn serves_post_route_with_form_urlencoded_body() {
        run_on_localset(async {
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_case(
                r"@server {
                    @listen 0
                    @route POST /members {
                        @respond 201 {
                            handle: @body.handle,
                            email: @body.email,
                            name: @body.name
                        }
                    }
                }",
            );
            let (addr, handle, _boot) = spawn_for_test(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                std::future::pending::<()>(),
            )
            .await
            .expect("spawn");

            let (status, _, body) = send_request_with_content_type(
                addr,
                "POST",
                "/members",
                "handle=ada&email=ada%40example.test&name=Ada+Lovelace".to_string(),
                "application/x-www-form-urlencoded; charset=utf-8",
            )
            .await;
            assert_eq!(status, 201);
            let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
            assert_eq!(json["handle"], serde_json::json!("ada"));
            assert_eq!(json["email"], serde_json::json!("ada@example.test"));
            assert_eq!(json["name"], serde_json::json!("Ada Lovelace"));

            handle.abort();
        })
        .await;
    }

    #[tokio::test]
    async fn unknown_route_returns_404() {
        run_on_localset(async {
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_case(
                r#"@server {
                    @listen 0
                    @route GET /ping { @respond 200 {} }
                }"#,
            );
            let (addr, handle, _boot) = spawn_for_test(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                std::future::pending::<()>(),
            )
            .await
            .expect("spawn");

            let (status, _, _) = send_request(addr, "GET", "/missing", None).await;
            assert_eq!(status, 404);

            handle.abort();
        })
        .await;
    }

    #[tokio::test]
    async fn respond_204_emits_empty_body() {
        run_on_localset(async {
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_case(
                r#"@server {
                    @listen 0
                    @route DELETE /item/:id { @respond 204 {} }
                }"#,
            );
            let (addr, handle, _boot) = spawn_for_test(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                std::future::pending::<()>(),
            )
            .await
            .expect("spawn");

            let (status, ct, body) = send_request(addr, "DELETE", "/item/abc", None).await;
            assert_eq!(status, 204);
            assert!(body.is_empty(), "204 should have empty body, got {body:?}");
            assert!(ct.is_none(), "204 should not set a body content-type");

            handle.abort();
        })
        .await;
    }

    #[tokio::test]
    async fn trailing_slash_is_normalized_and_matched() {
        run_on_localset(async {
            // 회귀: `/users/42/` 가 `/users/:id` 매처에 잡혀야 한다.
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_case(
                r#"@server {
                    @listen 0
                    @route GET /users/:id { @respond 200 { id: @param.id } }
                }"#,
            );
            let (addr, handle, _boot) = spawn_for_test(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                std::future::pending::<()>(),
            )
            .await
            .expect("spawn");

            let (status, _, body) = send_request(addr, "GET", "/users/42/", None).await;
            assert_eq!(status, 200, "trailing-slash path should match");
            let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
            assert_eq!(json["id"], serde_json::json!("42"));

            handle.abort();
        })
        .await;
    }

    #[tokio::test]
    async fn catchall_star_route_matches_unknown_paths() {
        run_on_localset(async {
            // SPEC §11.2: `@route GET *` 은 어느 경로도 잡는다. 앞선 구체 route 가
            // 먼저 매치되면 그쪽이 이긴다 — 선언 순서 규칙.
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_case(
                r#"@server {
                    @listen 0
                    @route GET /ping { @respond 200 { hit: "ping" } }
                    @route GET * { @respond 404 { err: "not found" } }
                }"#,
            );
            let (addr, handle, _boot) = spawn_for_test(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                std::future::pending::<()>(),
            )
            .await
            .expect("spawn");

            let (status, _, body) = send_request(addr, "GET", "/ping", None).await;
            assert_eq!(status, 200);
            let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
            assert_eq!(json["hit"], serde_json::json!("ping"));

            let (status2, _, body2) = send_request(addr, "GET", "/whatever", None).await;
            assert_eq!(status2, 404, "catchall route should respond 404");
            let json2: serde_json::Value = serde_json::from_slice(&body2).expect("json");
            assert_eq!(json2["err"], serde_json::json!("not found"));

            handle.abort();
        })
        .await;
    }

    #[tokio::test]
    async fn content_type_is_case_insensitive() {
        run_on_localset(async {
            // `APPLICATION/JSON` 도 JSON 경로로 파싱되어 `@body.x` 가 동작해야 한다.
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_case(
                r#"@server {
                    @listen 0
                    @route POST /m { @respond 200 { x: @body.x } }
                }"#,
            );
            let (addr, handle, _boot) = spawn_for_test(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                std::future::pending::<()>(),
            )
            .await
            .expect("spawn");

            // 일반 send_request 는 소문자 content-type 을 붙이므로 저수준 커스텀
            // 헤더로 보낸다.
            use hyper::client::conn::http1 as client_http1;
            let stream = tokio::net::TcpStream::connect(addr).await.expect("connect");
            let io = TokioIo::new(stream);
            let (mut sender, conn) = client_http1::handshake(io).await.expect("handshake");
            tokio::spawn(async move {
                let _ = conn.await;
            });

            let req = Request::builder()
                .method("POST")
                .uri("/m")
                .header("host", "localhost")
                .header("content-type", "APPLICATION/JSON")
                .body(Full::new(Bytes::from(r#"{"x":7}"#)))
                .expect("build req");
            let resp = sender.send_request(req).await.expect("send");
            let status = resp.status().as_u16();
            let bytes = resp.collect().await.expect("body").to_bytes().to_vec();
            let json: serde_json::Value = serde_json::from_slice(&bytes).expect("json");

            assert_eq!(status, 200);
            assert_eq!(json["x"], serde_json::json!(7));

            handle.abort();
        })
        .await;
    }

    #[tokio::test]
    async fn oversized_body_returns_413() {
        run_on_localset(async {
            // MAX_BODY_BYTES = 1 MiB. 이를 살짝 넘기는 바디로 413 을 확인한다.
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_case(
                r#"@server {
                    @listen 0
                    @route POST /upload { @respond 200 {} }
                }"#,
            );
            let (addr, handle, _boot) = spawn_for_test(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                std::future::pending::<()>(),
            )
            .await
            .expect("spawn");

            let big = "a".repeat(MAX_BODY_BYTES + 1024);
            let (status, _, _) = send_request(addr, "POST", "/upload", Some(big)).await;
            assert_eq!(status, 413, "expected 413 Payload Too Large");

            handle.abort();
        })
        .await;
    }

    #[tokio::test]
    async fn boot_stmts_run_before_accept() {
        run_on_localset(async {
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_case(
                r#"@server {
                    @out "boot"
                    @listen 0
                    @route GET /p { @respond 200 {} }
                }"#,
            );
            let (addr, handle, boot) = spawn_for_test(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                std::future::pending::<()>(),
            )
            .await
            .expect("spawn");

            let boot_str = String::from_utf8(boot).expect("utf-8");
            assert_eq!(boot_str, "boot\n");
            let (status, _, body) = send_request(addr, "GET", "/p", None).await;
            assert_eq!(status, 200);
            assert_eq!(body, b"{}".to_vec());

            handle.abort();
        })
        .await;
    }

    // --- C6 E2E: fixtures/e2e/*.orv 파일을 실제로 lower 하고 서버를 띄워 ---
    // --- 실제 HTTP 요청으로 응답을 검증한다. ---

    /// `fixtures/e2e/<name>` 를 읽어 production 과 같은 server prep 입력으로
    /// 바꾼다. fixture 는 대개 `@server` 단일 표현식이지만, helper 함수 같은
    /// top-level 바인딩이 추가되어도 captured env 로 흘러간다.
    fn read_e2e_fixture(name: &str) -> String {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/e2e")
            .join(name);
        std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()))
    }

    fn extract_server_from_fixture(name: &str) -> ServerTestCase {
        extract_server_case(&read_e2e_fixture(name))
    }

    #[tokio::test]
    async fn fixture_hello_serves_ping() {
        run_on_localset(async {
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_from_fixture("hello.orv");
            assert!(body_stmts.is_empty(), "hello.orv has no boot stmts");
            let (addr, handle, boot) = spawn_for_test(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                std::future::pending::<()>(),
            )
            .await
            .expect("spawn");
            assert!(boot.is_empty(), "hello.orv should produce no boot output");

            let (status, ct, body) = send_request(addr, "GET", "/ping", None).await;
            assert_eq!(status, 200);
            assert_eq!(ct.as_deref(), Some("application/json"));
            let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
            assert_eq!(json["ok"], serde_json::json!(true));
            assert_eq!(json["msg"], serde_json::json!("pong"));

            handle.abort();
        })
        .await;
    }

    #[tokio::test]
    async fn fixture_path_param_covers_param_query_and_json_body() {
        run_on_localset(async {
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_from_fixture("path_param.orv");
            let (addr, handle, _boot) = spawn_for_test(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                std::future::pending::<()>(),
            )
            .await
            .expect("spawn");

            // 1) :id 경로 파라미터
            let (s1, _, b1) = send_request(addr, "GET", "/users/42", None).await;
            assert_eq!(s1, 200);
            let j1: serde_json::Value = serde_json::from_slice(&b1).expect("json");
            assert_eq!(j1["id"], serde_json::json!("42"));

            // 2) @query.q — URI 에 쿼리스트링 직접 포함
            let (s2, _, b2) = send_request(addr, "GET", "/search?q=orv", None).await;
            assert_eq!(s2, 200);
            let j2: serde_json::Value = serde_json::from_slice(&b2).expect("json");
            assert_eq!(j2["q"], serde_json::json!("orv"));

            // 2b) percent-encoded + `+` 혼합 쿼리 — `hello world` 와 UTF-8 `안녕`
            //     모두 핸들러까지 디코딩된 채로 도달해야 한다 (A1).
            let (s2b, _, b2b) = send_request(
                addr,
                "GET",
                "/search?q=hello+world%20%EC%95%88%EB%85%95",
                None,
            )
            .await;
            assert_eq!(s2b, 200);
            let j2b: serde_json::Value = serde_json::from_slice(&b2b).expect("json");
            assert_eq!(j2b["q"], serde_json::json!("hello world 안녕"));

            // 3) POST /echo 에 JSON body 보내면 그대로 되돌려받아야 한다
            let payload = r#"{"name":"alice","age":30}"#.to_string();
            let (s3, _, b3) = send_request(addr, "POST", "/echo", Some(payload)).await;
            assert_eq!(s3, 201);
            let j3: serde_json::Value = serde_json::from_slice(&b3).expect("json");
            assert_eq!(j3["received"]["name"], serde_json::json!("alice"));
            assert_eq!(j3["received"]["age"], serde_json::json!(30));

            handle.abort();
        })
        .await;
    }

    #[tokio::test]
    async fn fixture_shopping_mall_covers_home_catalog_and_order_flow() {
        run_on_localset(async {
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos();
            let sqlite_path = std::env::temp_dir().join(format!(
                "orv-shopping-fixture-{}-{unique}.sqlite",
                std::process::id()
            ));
            let payment_path = std::env::temp_dir().join(format!(
                "orv-shopping-fixture-payments-{}-{unique}.jsonl",
                std::process::id()
            ));
            let shipping_path = std::env::temp_dir().join(format!(
                "orv-shopping-fixture-shipments-{}-{unique}.jsonl",
                std::process::id()
            ));
            let src = read_e2e_fixture("shopping_mall.orv")
                .replace(
                    "sqlite://data/shop.sqlite",
                    &format!("sqlite://{}", sqlite_path.display()),
                )
                .replace(
                    "file://data/payments.jsonl",
                    &format!("file://{}", payment_path.display()),
                )
                .replace(
                    "file://data/shipments.jsonl",
                    &format!("file://{}", shipping_path.display()),
                );
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_case(&src);
            let (addr, handle, _boot) = spawn_for_test(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                std::future::pending::<()>(),
            )
            .await
            .expect("spawn");

            let (home_status, home_ct, home_body) = send_request(addr, "GET", "/", None).await;
            assert_eq!(home_status, 200);
            assert_eq!(home_ct.as_deref(), Some("text/html; charset=utf-8"));
            let home_html = String::from_utf8(home_body).expect("home html");
            assert!(home_html.contains("<h1>Miol Shop</h1>"));
            assert!(home_html.contains("<form action=\"/products\" method=\"post\">"));
            assert!(home_html.contains("<input type=\"number\" name=\"stock\" required>"));
            assert!(home_html.contains("<form action=\"/orders\" method=\"post\">"));
            assert!(home_html.contains("<form action=\"/members/login\" method=\"post\">"));
            assert!(home_html.contains("<form action=\"/checkout\" method=\"post\">"));
            assert!(home_html.contains("<form action=\"/cart/items\" method=\"post\">"));
            assert!(home_html.contains("<a href=\"/admin\">Admin dashboard</a>"));
            assert!(home_html.contains("<a href=\"/catalog\">Shop catalog</a>"));
            assert!(home_html.contains("<a href=\"/cart\">Cart</a>"));
            assert!(home_html.contains("<a href=\"/account/sessions\">My sessions</a>"));
            assert!(home_html.contains("POST /payments"));
            assert!(home_html.contains("POST /webhooks/stripe"));
            assert!(home_html.contains("POST /shipments"));

            let (admin_status, admin_ct, admin_body) =
                send_request(addr, "GET", "/admin", None).await;
            assert_eq!(admin_status, 200);
            assert_eq!(admin_ct.as_deref(), Some("text/html; charset=utf-8"));
            let admin_html = String::from_utf8(admin_body).expect("admin html");
            assert!(admin_html.contains("<h1>Miol Shop Admin</h1>"));
            assert!(admin_html.contains("Operations dashboard"));
            assert!(admin_html.contains("<a href=\"/admin/catalog\">Catalog read model</a>"));
            assert!(admin_html.contains("<a href=\"/admin/summary\">Operations summary</a>"));
            assert!(admin_html.contains("<a href=\"/admin/orders\">Order read model</a>"));
            assert!(admin_html.contains("<a href=\"/admin/payments\">Payment read model</a>"));
            assert!(admin_html.contains("<a href=\"/admin/shipments\">Shipment read model</a>"));
            assert!(admin_html.contains("<a href=\"/admin/webhooks\">Webhook read model</a>"));
            assert!(admin_html.contains("<a href=\"/admin/audit\">Audit read model</a>"));
            assert!(admin_html.contains("Stripe webhook events: POST /webhooks/stripe"));
            assert!(admin_html.contains("data/shop.sqlite"));

            let (health_status, _, health_body) = send_request(addr, "GET", "/health", None).await;
            assert_eq!(health_status, 200);
            let health: serde_json::Value =
                serde_json::from_slice(&health_body).expect("health json");
            assert_eq!(health["ok"], serde_json::json!(true));

            let product_payload = serde_json::json!({
                "sku": "kettle",
                "name": "Kettle",
                "price": 25000,
                "stock": 2
            })
            .to_string();
            let (create_product_status, _, create_product_body) =
                send_request(addr, "POST", "/products", Some(product_payload)).await;
            assert_eq!(create_product_status, 201);
            let created_product: serde_json::Value =
                serde_json::from_slice(&create_product_body).expect("product json");
            assert_eq!(created_product["product"]["id"], serde_json::json!(1));
            assert_eq!(
                created_product["product"]["sku"],
                serde_json::json!("kettle")
            );

            let (form_product_status, _, form_product_body) = send_request_with_content_type(
                addr,
                "POST",
                "/products",
                "sku=mug&name=Mug&price=1200&stock=3".to_string(),
                "application/x-www-form-urlencoded",
            )
            .await;
            assert_eq!(form_product_status, 201);
            let form_product: serde_json::Value =
                serde_json::from_slice(&form_product_body).expect("form product json");
            assert_eq!(form_product["product"]["sku"], serde_json::json!("mug"));
            assert_eq!(form_product["product"]["stock"], serde_json::json!(3));

            let (list_status, _, list_body) = send_request(addr, "GET", "/products", None).await;
            assert_eq!(list_status, 200);
            let list: serde_json::Value = serde_json::from_slice(&list_body).expect("list json");
            assert_eq!(list["products"].as_array().map(Vec::len), Some(2));
            assert_eq!(list["products"][0]["name"], serde_json::json!("Kettle"));

            let (catalog_status, catalog_ct, catalog_body) =
                send_request(addr, "GET", "/catalog", None).await;
            assert_eq!(catalog_status, 200);
            assert_eq!(catalog_ct.as_deref(), Some("text/html; charset=utf-8"));
            let catalog_html = String::from_utf8(catalog_body).expect("catalog html");
            assert!(catalog_html.contains("<h1>Shop Catalog</h1>"));
            assert!(catalog_html.contains("Kettle"));
            assert!(catalog_html.contains("Mug"));
            assert!(catalog_html.contains("stock 3"));

            let (admin_catalog_status, admin_catalog_ct, admin_catalog_body) =
                send_request(addr, "GET", "/admin/catalog", None).await;
            assert_eq!(admin_catalog_status, 200);
            assert_eq!(
                admin_catalog_ct.as_deref(),
                Some("text/html; charset=utf-8")
            );
            let admin_catalog_html =
                String::from_utf8(admin_catalog_body).expect("admin catalog html");
            assert!(admin_catalog_html.contains("<h1>Catalog</h1>"));
            assert!(admin_catalog_html.contains("kettle: Kettle / stock 2"));
            assert!(admin_catalog_html.contains("mug: Mug / stock 3"));

            let (form_order_status, _, form_order_body) = send_request_with_content_type(
                addr,
                "POST",
                "/orders",
                "customer=bea&sku=mug&quantity=2&total=2400".to_string(),
                "application/x-www-form-urlencoded",
            )
            .await;
            assert_eq!(form_order_status, 201);
            let form_order: serde_json::Value =
                serde_json::from_slice(&form_order_body).expect("form order json");
            assert_eq!(form_order["order"]["customer"], serde_json::json!("bea"));
            assert_eq!(form_order["order"]["quantity"], serde_json::json!(2));
            assert_eq!(form_order["remainingStock"], serde_json::json!(1));

            let order_payload = serde_json::json!({
                "customer": "ada",
                "sku": "kettle",
                "quantity": 1,
                "total": 25000
            })
            .to_string();
            let (create_order_status, _, create_order_body) =
                send_request(addr, "POST", "/orders", Some(order_payload)).await;
            assert_eq!(create_order_status, 201);
            let created_order: serde_json::Value =
                serde_json::from_slice(&create_order_body).expect("order json");
            assert_eq!(
                created_order["order"]["status"],
                serde_json::json!("reserved")
            );
            assert_eq!(created_order["order"]["total"], serde_json::json!(25000));
            assert_eq!(created_order["remainingStock"], serde_json::json!(1));
            let order_id = created_order["order"]["id"].as_i64().expect("order id");

            let (find_order_status, _, find_order_body) =
                send_request(addr, "GET", "/orders/ada", None).await;
            assert_eq!(find_order_status, 200);
            let found_order: serde_json::Value =
                serde_json::from_slice(&find_order_body).expect("found order json");
            assert_eq!(found_order["order"]["customer"], serde_json::json!("ada"));
            assert_eq!(found_order["order"]["sku"], serde_json::json!("kettle"));

            let (find_product_status, _, find_product_body) =
                send_request(addr, "GET", "/products/kettle", None).await;
            assert_eq!(find_product_status, 200);
            let found_product: serde_json::Value =
                serde_json::from_slice(&find_product_body).expect("found product json");
            assert_eq!(found_product["product"]["stock"], serde_json::json!(1));

            let oversell_payload = serde_json::json!({
                "customer": "grace",
                "sku": "kettle",
                "quantity": 2,
                "total": 50000
            })
            .to_string();
            let (oversell_status, _, oversell_body) =
                send_request(addr, "POST", "/orders", Some(oversell_payload)).await;
            assert_eq!(oversell_status, 409);
            let oversell: serde_json::Value =
                serde_json::from_slice(&oversell_body).expect("oversell json");
            assert_eq!(oversell["err"], serde_json::json!("out_of_stock"));
            assert_eq!(oversell["stock"], serde_json::json!(1));

            let member_payload = serde_json::json!({
                "handle": "ada",
                "name": "Ada Lovelace",
                "email": "ada@example.test"
            })
            .to_string();
            let (create_member_status, _, create_member_body) =
                send_request(addr, "POST", "/members", Some(member_payload)).await;
            assert_eq!(create_member_status, 201);
            let created_member: serde_json::Value =
                serde_json::from_slice(&create_member_body).expect("member json");
            assert_eq!(created_member["member"]["handle"], serde_json::json!("ada"));

            let (find_member_status, _, find_member_body) =
                send_request(addr, "GET", "/members/ada", None).await;
            assert_eq!(find_member_status, 200);
            let found_member: serde_json::Value =
                serde_json::from_slice(&find_member_body).expect("found member json");
            assert_eq!(
                found_member["member"]["email"],
                serde_json::json!("ada@example.test")
            );

            let login_payload = serde_json::json!({
                "handle": "ada",
                "email": "ada@example.test"
            })
            .to_string();
            let (login_status, _, _, login_headers, login_body) =
                send_request_full(addr, "POST", "/members/login", Some(login_payload)).await;
            assert_eq!(login_status, 201);
            let login_cookie = login_headers.get("set-cookie").expect("login set-cookie");
            assert!(login_cookie.contains("orv_session=1"));
            assert!(login_cookie.contains("Path=/"));
            assert!(login_cookie.contains("Max-Age=86400"));
            assert!(login_cookie.contains("HttpOnly"));
            assert!(login_cookie.contains("SameSite=Lax"));
            assert!(login_cookie.contains("Secure"));
            let login: serde_json::Value = serde_json::from_slice(&login_body).expect("login json");
            assert_eq!(login["session"]["handle"], serde_json::json!("ada"));
            assert_eq!(login["session"]["status"], serde_json::json!("active"));

            let cart_payload = serde_json::json!({
                "handle": "ada",
                "sku": "mug",
                "quantity": 1
            })
            .to_string();
            let (cart_item_status, _, cart_item_body) =
                send_request(addr, "POST", "/cart/items", Some(cart_payload)).await;
            assert_eq!(cart_item_status, 201);
            let cart_item: serde_json::Value =
                serde_json::from_slice(&cart_item_body).expect("cart item json");
            assert_eq!(cart_item["cartItem"]["handle"], serde_json::json!("ada"));
            assert_eq!(cart_item["cartItem"]["sku"], serde_json::json!("mug"));
            assert_eq!(cart_item["cartItem"]["quantity"], serde_json::json!(1));

            let (cart_status, cart_ct, cart_body) = send_request(addr, "GET", "/cart", None).await;
            assert_eq!(cart_status, 200);
            assert_eq!(cart_ct.as_deref(), Some("text/html; charset=utf-8"));
            let cart_html = String::from_utf8(cart_body).expect("cart html");
            assert!(cart_html.contains("<h1>Cart</h1>"));
            assert!(cart_html.contains("ada"));
            assert!(cart_html.contains("mug"));
            assert!(cart_html.contains("quantity 1"));

            let (sessions_status, sessions_ct, sessions_body) =
                send_request(addr, "GET", "/account/sessions", None).await;
            assert_eq!(sessions_status, 200);
            assert_eq!(sessions_ct.as_deref(), Some("text/html; charset=utf-8"));
            let sessions_html = String::from_utf8(sessions_body).expect("sessions html");
            assert!(sessions_html.contains("<h1>Account Sessions</h1>"));
            assert!(sessions_html.contains("ada"));
            assert!(sessions_html.contains("active"));

            let payment_payload = serde_json::json!({
                "orderId": order_id,
                "amount": 25000,
                "method": "card"
            })
            .to_string();
            let (payment_status, _, payment_body) =
                send_request(addr, "POST", "/payments", Some(payment_payload)).await;
            assert_eq!(payment_status, 201);
            let payment: serde_json::Value =
                serde_json::from_slice(&payment_body).expect("payment json");
            assert_eq!(payment["payment"]["status"], serde_json::json!("captured"));
            assert_eq!(payment["payment"]["provider"], serde_json::json!("file"));
            assert_eq!(payment["order"]["status"], serde_json::json!("paid"));

            let shipment_payload = serde_json::json!({
                "orderId": order_id,
                "carrier": "post",
                "address": "Seoul"
            })
            .to_string();
            let (shipment_status, _, shipment_body) =
                send_request(addr, "POST", "/shipments", Some(shipment_payload)).await;
            assert_eq!(shipment_status, 201);
            let shipment: serde_json::Value =
                serde_json::from_slice(&shipment_body).expect("shipment json");
            assert_eq!(shipment["shipment"]["status"], serde_json::json!("ready"));
            assert_eq!(shipment["shipment"]["provider"], serde_json::json!("file"));
            assert_eq!(shipment["order"]["status"], serde_json::json!("shipped"));

            let shipment_path = format!("/shipments/{order_id}");
            let (find_shipment_status, _, find_shipment_body) =
                send_request(addr, "GET", &shipment_path, None).await;
            assert_eq!(find_shipment_status, 200);
            let found_shipment: serde_json::Value =
                serde_json::from_slice(&find_shipment_body).expect("found shipment json");
            assert_eq!(
                found_shipment["shipment"]["tracking"],
                serde_json::json!("TRK-LOCAL")
            );

            crate::interp::test_env::set("STRIPE_WEBHOOK_SECRET", "whsec_test");
            let webhook_payload = r#"{"id":"evt_1"}"#.to_string();
            let webhook_signature =
                "t=1700000000,v1=c89214b5b5da833daed6f0b8c5bb6bd58cea9022bd80ccc78230f3942d632925";
            let (webhook_status, _, webhook_body) = send_request_with_headers(
                addr,
                "POST",
                "/webhooks/stripe",
                Some(webhook_payload.clone()),
                &[("stripe-signature", webhook_signature)],
            )
            .await;
            assert_eq!(webhook_status, 202);
            let webhook: serde_json::Value =
                serde_json::from_slice(&webhook_body).expect("webhook json");
            assert_eq!(webhook["duplicate"], serde_json::json!(false));
            assert_eq!(
                webhook["verification"]["status"],
                serde_json::json!("verified")
            );
            assert_eq!(webhook["webhook"]["provider"], serde_json::json!("stripe"));
            assert_eq!(webhook["webhook"]["status"], serde_json::json!("verified"));
            assert_eq!(webhook["webhook"]["eventId"], serde_json::json!("evt_1"));

            let (duplicate_webhook_status, _, duplicate_webhook_body) = send_request_with_headers(
                addr,
                "POST",
                "/webhooks/stripe",
                Some(webhook_payload),
                &[("stripe-signature", webhook_signature)],
            )
            .await;
            crate::interp::test_env::clear("STRIPE_WEBHOOK_SECRET");
            assert_eq!(duplicate_webhook_status, 200);
            let duplicate_webhook: serde_json::Value =
                serde_json::from_slice(&duplicate_webhook_body).expect("duplicate webhook json");
            assert_eq!(duplicate_webhook["duplicate"], serde_json::json!(true));
            assert_eq!(
                duplicate_webhook["webhook"]["eventId"],
                serde_json::json!("evt_1")
            );

            let checkout_payload = serde_json::json!({
                "handle": "ada",
                "sku": "mug",
                "quantity": 1,
                "total": 1200,
                "method": "card",
                "carrier": "post",
                "address": "Seoul"
            })
            .to_string();
            let (checkout_status, _, checkout_body) =
                send_request(addr, "POST", "/checkout", Some(checkout_payload)).await;
            assert_eq!(checkout_status, 201);
            let checkout: serde_json::Value =
                serde_json::from_slice(&checkout_body).expect("checkout json");
            assert_eq!(checkout["order"]["customer"], serde_json::json!("ada"));
            assert_eq!(checkout["order"]["status"], serde_json::json!("shipped"));
            assert_eq!(checkout["payment"]["status"], serde_json::json!("captured"));
            assert_eq!(
                checkout["shipment"]["tracking"],
                serde_json::json!("TRK-LOCAL")
            );
            let checkout_order_id = checkout["order"]["id"].as_i64().expect("checkout order id");

            crate::interp::test_env::set("STRIPE_WEBHOOK_SECRET", "whsec_test");
            let reconciliation_payload = serde_json::json!({
                "id": "evt_checkout_paid",
                "orderId": checkout_order_id,
                "paymentStatus": "provider_paid",
                "orderStatus": "provider_reconciled"
            })
            .to_string();
            let reconciliation_signature =
                stripe_test_signature("whsec_test", "1700000001", &reconciliation_payload);
            let (reconciliation_status, _, reconciliation_body) = send_request_with_headers(
                addr,
                "POST",
                "/webhooks/stripe",
                Some(reconciliation_payload),
                &[("stripe-signature", &reconciliation_signature)],
            )
            .await;
            crate::interp::test_env::clear("STRIPE_WEBHOOK_SECRET");
            assert_eq!(reconciliation_status, 202);
            let reconciliation: serde_json::Value =
                serde_json::from_slice(&reconciliation_body).expect("reconciliation json");
            assert_eq!(
                reconciliation["reconciledPayment"]["status"],
                serde_json::json!("provider_paid")
            );
            assert_eq!(
                reconciliation["reconciledOrder"]["status"],
                serde_json::json!("provider_reconciled")
            );

            let (admin_orders_status, _, admin_orders_body) =
                send_request(addr, "GET", "/admin/orders", None).await;
            assert_eq!(admin_orders_status, 200);
            let admin_orders_html = String::from_utf8(admin_orders_body).expect("orders html utf8");
            assert!(admin_orders_html.contains("ada"));
            assert!(admin_orders_html.contains("provider_reconciled"));

            let (admin_payments_status, _, admin_payments_body) =
                send_request(addr, "GET", "/admin/payments", None).await;
            assert_eq!(admin_payments_status, 200);
            let admin_payments_html =
                String::from_utf8(admin_payments_body).expect("payments html utf8");
            assert!(admin_payments_html.contains("captured"));
            assert!(admin_payments_html.contains("provider_paid"));
            assert!(admin_payments_html.contains("file"));

            let (admin_shipments_status, _, admin_shipments_body) =
                send_request(addr, "GET", "/admin/shipments", None).await;
            assert_eq!(admin_shipments_status, 200);
            let admin_shipments_html =
                String::from_utf8(admin_shipments_body).expect("shipments html utf8");
            assert!(admin_shipments_html.contains("TRK-LOCAL"));

            let (admin_webhooks_status, _, admin_webhooks_body) =
                send_request(addr, "GET", "/admin/webhooks", None).await;
            assert_eq!(admin_webhooks_status, 200);
            let admin_webhooks_html =
                String::from_utf8(admin_webhooks_body).expect("webhooks html utf8");
            assert!(admin_webhooks_html.contains("evt_1"));
            assert!(admin_webhooks_html.contains("evt_checkout_paid"));
            assert!(admin_webhooks_html.contains("verified"));

            let (admin_audit_status, _, admin_audit_body) =
                send_request(addr, "GET", "/admin/audit", None).await;
            assert_eq!(admin_audit_status, 200);
            let admin_audit_html = String::from_utf8(admin_audit_body).expect("audit html utf8");
            assert!(admin_audit_html.contains("checkout.complete"));
            assert!(admin_audit_html.contains("payment.capture"));
            assert!(admin_audit_html.contains("shipment.book"));
            assert!(admin_audit_html.contains("webhook.received"));

            let (summary_status, _, summary_body) =
                send_request(addr, "GET", "/admin/summary", None).await;
            assert_eq!(summary_status, 200);
            let summary: serde_json::Value =
                serde_json::from_slice(&summary_body).expect("admin summary json");
            assert_eq!(summary["products"], serde_json::json!(2));
            assert_eq!(summary["members"], serde_json::json!(1));
            assert_eq!(summary["orders"], serde_json::json!(3));
            assert_eq!(summary["payments"], serde_json::json!(2));
            assert_eq!(summary["shipments"], serde_json::json!(2));
            assert_eq!(summary["webhookEvents"], serde_json::json!(2));
            assert_eq!(summary["auditEvents"], serde_json::json!(15));

            handle.abort();

            let restored = crate::db::InMemoryDb::load_sqlite(&sqlite_path)
                .expect("reload shopping fixture sqlite");
            let snapshot = restored.snapshot_json();
            assert_eq!(
                snapshot["tables"]["Member"]["rows"][0]["handle"],
                serde_json::json!("ada")
            );
            assert_eq!(
                snapshot["tables"]["Order"]["rows"].as_array().map(Vec::len),
                Some(3)
            );
            assert_eq!(
                snapshot["tables"]["Payment"]["rows"]
                    .as_array()
                    .map(Vec::len),
                Some(2)
            );
            assert_eq!(
                snapshot["tables"]["Shipment"]["rows"]
                    .as_array()
                    .map(Vec::len),
                Some(2)
            );
            assert_eq!(
                snapshot["tables"]["Shipment"]["rows"][0]["tracking"],
                serde_json::json!("TRK-LOCAL")
            );
            assert_eq!(
                snapshot["tables"]["WebhookEvent"]["rows"]
                    .as_array()
                    .map(Vec::len),
                Some(2)
            );
            assert_eq!(
                snapshot["tables"]["WebhookEvent"]["rows"][0]["eventId"],
                serde_json::json!("evt_1")
            );
            assert_eq!(
                snapshot["tables"]["AuditEvent"]["rows"]
                    .as_array()
                    .map(Vec::len),
                Some(15)
            );
            assert_eq!(
                snapshot["tables"]["AuditEvent"]["rows"][0]["kind"],
                serde_json::json!("product.create")
            );
            let payment_records =
                std::fs::read_to_string(&payment_path).expect("payment record log");
            let shipping_records =
                std::fs::read_to_string(&shipping_path).expect("shipping record log");
            assert!(payment_records.contains(r#""kind":"payment.capture""#));
            assert!(payment_records.contains(&format!(r#""orderId":{order_id}"#)));
            assert!(shipping_records.contains(r#""kind":"shipping.booking""#));
            assert!(shipping_records.contains(r#""tracking":"TRK-LOCAL""#));
            let _ = std::fs::remove_file(sqlite_path);
            let _ = std::fs::remove_file(payment_path);
            let _ = std::fs::remove_file(shipping_path);
        })
        .await;
    }

    #[tokio::test]
    async fn server_body_wal_persists_route_db_mutations() {
        run_on_localset(async {
            let unique = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock after epoch")
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "orv-server-db-wal-{}-{unique}.jsonl",
                std::process::id()
            ));
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_case(&format!(
                r#"@server {{
                    @listen 0
                    @db.wal "{}"
                    @route POST /members {{
                        let member = await @db.create("Member", @body)
                        @respond 201 {{ member: member }}
                    }}
                }}"#,
                path.display()
            ));
            let (addr, handle, _boot) = spawn_for_test(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                std::future::pending::<()>(),
            )
            .await
            .expect("spawn");

            let payload = serde_json::json!({
                "handle": "ada",
                "email": "ada@example.test"
            })
            .to_string();
            let (status, _, body) = send_request(addr, "POST", "/members", Some(payload)).await;
            assert_eq!(status, 201);
            let created: serde_json::Value = serde_json::from_slice(&body).expect("member json");
            assert_eq!(created["member"]["handle"], serde_json::json!("ada"));
            handle.abort();

            let restored = crate::db::InMemoryDb::load_wal(&path).expect("replay server wal");
            let snapshot = restored.snapshot_json();
            assert_eq!(
                snapshot["tables"]["Member"]["rows"][0]["handle"],
                serde_json::json!("ada")
            );
            let _ = std::fs::remove_file(path);
        })
        .await;
    }

    #[tokio::test]
    async fn fixture_catchall_boots_specific_route_and_wildcard_fallback() {
        run_on_localset(async {
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_from_fixture("catchall.orv");
            assert_eq!(body_stmts.len(), 1, "catchall.orv has one boot @out");
            let (addr, handle, boot) = spawn_for_test(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                std::future::pending::<()>(),
            )
            .await
            .expect("spawn");

            // 부트 출력 — C5c 의 body_stmts 패치가 실제로 런타임에 도달하는지
            // 검증. `@out` 은 줄바꿈을 붙여 기록한다.
            let boot_str = String::from_utf8(boot).expect("utf-8");
            assert_eq!(boot_str, "boot ok\n");

            // 1) 구체 라우트가 catchall 보다 먼저 매치
            let (s1, _, b1) = send_request(addr, "GET", "/ping", None).await;
            assert_eq!(s1, 200);
            let j1: serde_json::Value = serde_json::from_slice(&b1).expect("json");
            assert_eq!(j1["hit"], serde_json::json!("ping"));

            // 2) 그 외 경로는 `@route GET *` 이 잡아 404
            let (s2, _, b2) = send_request(addr, "GET", "/unknown/path", None).await;
            assert_eq!(s2, 404);
            let j2: serde_json::Value = serde_json::from_slice(&b2).expect("json");
            assert_eq!(j2["err"], serde_json::json!("not found"));

            handle.abort();
        })
        .await;
    }

    #[tokio::test]
    async fn fixture_middleware_accumulates_context_and_runs_after() {
        // C_middleware: `@Inject` (@before) 가 @next 로 context 에 값을 쌓고
        // `@Audit` (@after) 가 handler 뒤에 실행된다. `@respond` payload 는
        // `@context.role`/`@context.uid` 를 읽어 검증. `@after` 의 stdout 출력은
        // hyper 경로에서 sink 로 버려지므로(보수적 MVP) 응답 바디만 본다.
        run_on_localset(async {
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_from_fixture("middleware.orv");
            let (addr, handle, _boot) = spawn_for_test(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                std::future::pending::<()>(),
            )
            .await
            .expect("spawn");

            let (status, ct, body) = send_request(addr, "GET", "/me", None).await;
            assert_eq!(status, 200);
            assert_eq!(ct.as_deref(), Some("application/json"));
            let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
            assert_eq!(json["role"], serde_json::json!("admin"));
            assert_eq!(json["uid"], serde_json::json!(42));

            handle.abort();
        })
        .await;
    }

    #[tokio::test]
    async fn fixture_domains_exercises_reference_runtime_stubs() {
        run_on_localset(async {
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_from_fixture("domains.orv");
            assert!(body_stmts.is_empty(), "domains.orv has no boot stmts");
            let (addr, handle, boot) = spawn_for_test(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                std::future::pending::<()>(),
            )
            .await
            .expect("spawn");
            assert!(boot.is_empty(), "domains.orv should produce no boot output");

            let (status, ct, body) = send_request(addr, "GET", "/domains", None).await;
            assert_eq!(status, 200);
            assert_eq!(ct.as_deref(), Some("application/json"));
            let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
            assert_eq!(json["chunkSize"], serde_json::json!(5));
            assert_eq!(json["path"], serde_json::json!("files/upload-1.txt"));
            assert_eq!(
                json["url"],
                serde_json::json!("/orv-storage/files/upload-1.txt?signed=1")
            );
            assert_eq!(json["job"], serde_json::json!("queued"));
            assert_eq!(json["videoId"], serde_json::json!("upload-1"));
            assert_eq!(json["doc"], serde_json::json!("42"));
            assert_eq!(json["mail"], serde_json::json!(true));
            assert_eq!(json["media"], serde_json::json!("camera"));
            assert_eq!(json["upload"], serde_json::json!("upload-1"));
            assert_eq!(json["push"], serde_json::json!(true));
            assert_eq!(
                json["subscription"],
                serde_json::json!("push://subscription")
            );
            assert_eq!(json["sent"], serde_json::json!("sent"));
            assert_eq!(json["cache"], serde_json::json!("assets-v1"));
            assert_eq!(json["cached"], serde_json::json!("stored"));
            assert_eq!(json["loaded"], serde_json::json!("code"));
            assert_eq!(json["local"], serde_json::json!("logo"));
            assert_eq!(json["tun"], serde_json::json!("orv0"));
            assert_eq!(json["packetBytes"], serde_json::json!(6));
            assert_eq!(
                json["plugin"],
                serde_json::json!("ext/markdown-preview.wasm")
            );
            assert_eq!(json["activation"], serde_json::json!("activated"));
            assert_eq!(json["compute"], serde_json::json!("compute"));
            assert_eq!(json["observability"], serde_json::json!("superapp"));

            handle.abort();
        })
        .await;
    }

    #[tokio::test]
    async fn handlers_can_use_top_level_function_bindings() {
        run_on_localset(async {
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_case(
                r#"function helper() -> "pong"

                @server {
                    @listen 0
                    @route GET /ping { @respond 200 { msg: helper() } }
                }"#,
            );
            let (addr, handle, _boot) = spawn_for_test(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                std::future::pending::<()>(),
            )
            .await
            .expect("spawn");

            let (status, _, body) = send_request(addr, "GET", "/ping", None).await;
            assert_eq!(status, 200);
            let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
            assert_eq!(json["msg"], serde_json::json!("pong"));

            handle.abort();
        })
        .await;
    }

    #[tokio::test]
    async fn handlers_can_use_server_level_function_bindings() {
        run_on_localset(async {
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_case(
                r#"@server {
                    @listen 0
                    function helper() -> "pong"
                    @route GET /ping { @respond 200 { msg: helper() } }
                }"#,
            );
            let (addr, handle, _boot) = spawn_for_test(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                std::future::pending::<()>(),
            )
            .await
            .expect("spawn");

            let (status, _, body) = send_request(addr, "GET", "/ping", None).await;
            assert_eq!(status, 200);
            let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
            assert_eq!(json["msg"], serde_json::json!("pong"));

            handle.abort();
        })
        .await;
    }

    #[tokio::test]
    async fn shutdown_signal_stops_accept_loop_gracefully() {
        // A4: graceful shutdown.
        //
        // 시나리오:
        //   1) 서버 기동 → 첫 요청 200 확인
        //   2) shutdown 채널에 `()` 전송
        //   3) `handle.await` 가 정상 종료 (Ok, not aborted)
        //   4) 같은 주소로 재연결 시도 → listener 닫혀 연결 실패
        //
        // `handle.abort()` 가 아니라 자연 종료 경로라는 점이 핵심. in-flight
        // 연결이 있어도 serve_loop 는 select 에서 빠져나오기만 하고, 이미
        // accept 된 커넥션은 `serve_connection.await` 안에서 자연 완료된다.
        run_on_localset(async {
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_case(
                r#"@server {
                    @listen 0
                    @route GET /ping { @respond 200 { ok: true } }
                }"#,
            );
            let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
            let (addr, handle, _boot) = spawn_for_test(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                async move {
                    let _ = shutdown_rx.await;
                },
            )
            .await
            .expect("spawn");

            // 1) 첫 요청 — 서버 정상 동작 확인
            let (s1, _, _) = send_request(addr, "GET", "/ping", None).await;
            assert_eq!(s1, 200);

            // 2) shutdown 신호 → 3) 루프가 자연 종료해야 handle.await 가 완료됨
            let _ = shutdown_tx.send(());
            tokio::time::timeout(std::time::Duration::from_secs(2), handle)
                .await
                .expect("serve_loop did not exit within timeout")
                .expect("join handle err");

            // 4) 리스너 닫혔으니 재연결 실패. 일부 OS 는 TIME_WAIT 상태로
            //    잠깐 연결을 받아줄 수 있으므로 에러 자체를 강제하기보다
            //    "핸들이 끝났다" 까지가 primary assertion. 연결 시도는
            //    정상 경로 smoke check.
            let probe = tokio::time::timeout(
                std::time::Duration::from_millis(500),
                TcpStream::connect(addr),
            )
            .await;
            match probe {
                Ok(Ok(_)) => {
                    // 연결은 맺혔지만 accept 가 닫혀 요청 처리 불가.
                    // 여기까지는 OS TCP 스택 거동이라 허용.
                }
                Ok(Err(_)) | Err(_) => {
                    // ConnectionRefused 또는 timeout — 기대 경로.
                }
            }
        })
        .await;
    }

    #[tokio::test]
    async fn attached_server_handle_serves_until_drop() {
        let hir = lower_src(
            r"@server {
                @listen 0
                @route GET /ping { @respond 200 { ok: true } }
            }",
        );
        let server = spawn_attached_server(hir).expect("spawn attached server");
        let addr = server.addr();

        let (status, _, body) = send_request(addr, "GET", "/ping", None).await;
        let json: serde_json::Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(status, 200);
        assert_eq!(json["ok"], serde_json::json!(true));

        drop(server);
        let probe = tokio::time::timeout(
            std::time::Duration::from_millis(500),
            TcpStream::connect(addr),
        )
        .await;
        if let Ok(Ok(_)) = probe {
            panic!("attached server still accepted connections after drop");
        }
    }

    #[tokio::test]
    async fn attached_server_prefix_wal_persists_route_db_mutations() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "orv-attached-db-wal-{}-{unique}.jsonl",
            std::process::id()
        ));
        let hir = lower_src(&format!(
            r#"@db.wal "{}"
            @server {{
                @listen 0
                @route POST /members {{
                    let member = await @db.create("Member", @body)
                    @respond 201 {{ member: member }}
                }}
            }}"#,
            path.display()
        ));
        let server = spawn_attached_server(hir).expect("spawn attached server");
        let addr = server.addr();

        let payload = serde_json::json!({
            "handle": "ada",
            "email": "ada@example.test"
        })
        .to_string();
        let (status, _, body) = send_request(addr, "POST", "/members", Some(payload)).await;
        assert_eq!(status, 201);
        let created: serde_json::Value = serde_json::from_slice(&body).expect("member json");
        assert_eq!(created["member"]["handle"], serde_json::json!("ada"));
        drop(server);

        let restored = crate::db::InMemoryDb::load_wal(&path).expect("replay attached wal");
        let snapshot = restored.snapshot_json();
        assert_eq!(
            snapshot["tables"]["Member"]["rows"][0]["handle"],
            serde_json::json!("ada")
        );
        let _ = std::fs::remove_file(path);
    }

    #[tokio::test]
    async fn serve_single_file_returns_bytes_and_mime() {
        // A5a: `@serve "path"` — 단일 파일 서빙. 파일 바이트 그대로 + 확장자
        // 기반 Content-Type 헤더. 이 테스트는 세 가지를 한 번에 검증한다:
        //
        //   1. HTML 확장자는 text/html charset=utf-8
        //   2. body bytes 가 파일 내용 그대로 (JSON 직렬화 안 됨)
        //   3. 바이너리 파일 (ICO) 은 image/x-icon
        run_on_localset(async {
            let tmp = std::env::temp_dir().join(format!("orv_serve_test_{}", std::process::id()));
            std::fs::create_dir_all(&tmp).expect("mktemp");
            let html_path = tmp.join("index.html");
            let ico_path = tmp.join("favicon.ico");
            std::fs::write(&html_path, b"<!doctype html><h1>hi</h1>").expect("write html");
            // ICO magic bytes — 단순 바이너리 검증용
            std::fs::write(&ico_path, [0u8, 0, 1, 0, 1, 0]).expect("write ico");

            let src = format!(
                r#"@server {{
                    @listen 0
                    @route GET /index.html {{ @serve "{}" }}
                    @route GET /favicon.ico {{ @serve "{}" }}
                }}"#,
                html_path.display(),
                ico_path.display()
            );
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_case(&src);
            let (addr, handle, _boot) = spawn_for_test(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                std::future::pending::<()>(),
            )
            .await
            .expect("spawn");

            // 1+2) HTML
            let (s_html, ct_html, b_html) = send_request(addr, "GET", "/index.html", None).await;
            assert_eq!(s_html, 200);
            assert_eq!(ct_html.as_deref(), Some("text/html; charset=utf-8"));
            assert_eq!(b_html, b"<!doctype html><h1>hi</h1>");

            // 3) ICO
            let (s_ico, ct_ico, b_ico) = send_request(addr, "GET", "/favicon.ico", None).await;
            assert_eq!(s_ico, 200);
            assert_eq!(ct_ico.as_deref(), Some("image/x-icon"));
            assert_eq!(b_ico, vec![0u8, 0, 1, 0, 1, 0]);

            handle.abort();
            std::fs::remove_dir_all(&tmp).ok();
        })
        .await;
    }

    #[tokio::test]
    async fn nested_route_group_prefixes_match_flat() {
        // A2a E2E: `@route /admin { @route GET /users {...} }` 가 실제
        // HTTP 요청 `/admin/users` 에 매치되어야 한다. analyzer 의 unfold 가
        // runtime 매처까지 이어지는지 검증.
        run_on_localset(async {
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_case(
                r#"@server {
                    @listen 0
                    @route /admin {
                        @route GET /users { @respond 200 { hit: "users" } }
                        @route GET /posts { @respond 200 { hit: "posts" } }
                    }
                }"#,
            );
            let (addr, handle, _boot) = spawn_for_test(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                std::future::pending::<()>(),
            )
            .await
            .expect("spawn");

            let (s1, _, b1) = send_request(addr, "GET", "/admin/users", None).await;
            assert_eq!(s1, 200);
            let j1: serde_json::Value = serde_json::from_slice(&b1).expect("json");
            assert_eq!(j1["hit"], serde_json::json!("users"));

            let (s2, _, b2) = send_request(addr, "GET", "/admin/posts", None).await;
            assert_eq!(s2, 200);
            let j2: serde_json::Value = serde_json::from_slice(&b2).expect("json");
            assert_eq!(j2["hit"], serde_json::json!("posts"));

            // unjoin 경로는 매치 안 돼 404
            let (s3, _, _) = send_request(addr, "GET", "/users", None).await;
            assert_eq!(s3, 404);

            handle.abort();
        })
        .await;
    }

    #[tokio::test]
    async fn group_middleware_applies_to_all_inner_routes() {
        // C_middleware 확장: `@route /admin { @Auth; @route ... }` 에서 `@Auth`
        // (@before) 가 내부 모든 route 의 handler 앞에 prepend 되어야 한다.
        // analyzer 의 `inherited_stmts` 경로가 middleware stmt 도 누적한다.
        run_on_localset(async {
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_case(
                r#"@server {
                    @listen 0
                    define Auth() -> @before { @next {user: "admin"} }
                    @route /admin {
                        @Auth
                        @route GET /users { @respond 200 { u: @context.user, kind: "users" } }
                        @route GET /posts { @respond 200 { u: @context.user, kind: "posts" } }
                    }
                }"#,
            );
            let (addr, handle, _boot) = spawn_for_test(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                std::future::pending::<()>(),
            )
            .await
            .expect("spawn");

            for (path, kind) in [("/admin/users", "users"), ("/admin/posts", "posts")] {
                let (status, _, body) = send_request(addr, "GET", path, None).await;
                assert_eq!(status, 200, "path {path}");
                let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
                assert_eq!(json["u"], serde_json::json!("admin"), "path {path}");
                assert_eq!(json["kind"], serde_json::json!(kind), "path {path}");
            }

            handle.abort();
        })
        .await;
    }

    #[tokio::test]
    async fn nested_group_middleware_stacks_outer_first() {
        // 중첩 그룹: outer 그룹의 middleware 가 inner 그룹 middleware 보다 먼저
        // 실행되어 context 누적 순서가 outer → inner 이어야 한다. `@next` 가
        // 같은 key 를 덮어쓰는 규칙(마지막 push 우세)과 결합해, inner 가 outer
        // 의 값을 override 할 수 있는지도 본다.
        run_on_localset(async {
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_case(
                r#"@server {
                    @listen 0
                    define Outer() -> @before { @next {scope: "outer", depth: 1} }
                    define Inner() -> @before { @next {scope: "inner"} }
                    @route /api {
                        @Outer
                        @route /v1 {
                            @Inner
                            @route GET /ping {
                                @respond 200 { scope: @context.scope, depth: @context.depth }
                            }
                        }
                    }
                }"#,
            );
            let (addr, handle, _boot) = spawn_for_test(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                std::future::pending::<()>(),
            )
            .await
            .expect("spawn");

            let (status, _, body) = send_request(addr, "GET", "/api/v1/ping", None).await;
            assert_eq!(status, 200);
            let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
            // inner 가 scope 을 override — 마지막 push 우세.
            assert_eq!(json["scope"], serde_json::json!("inner"));
            // depth 는 outer 에서만 push 되어 그대로 유지.
            assert_eq!(json["depth"], serde_json::json!(1));

            handle.abort();
        })
        .await;
    }

    #[tokio::test]
    async fn server_redirect_default_302() {
        // SPEC §11.9: `@redirect "/path"` → 302 + Location 헤더.
        run_on_localset(async {
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_case(
                r#"@server {
                    @listen 0
                    @route GET /old {
                        @redirect "/new"
                    }
                }"#,
            );
            let (addr, handle, _boot) = spawn_for_test(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                std::future::pending::<()>(),
            )
            .await
            .expect("spawn");

            let (status, _, body) = send_request(addr, "GET", "/old", None).await;
            assert_eq!(status, 302);
            assert_eq!(body.len(), 0);

            handle.abort();
        })
        .await;
    }

    #[tokio::test]
    async fn server_redirect_explicit_status() {
        run_on_localset(async {
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_case(
                r#"@server {
                    @listen 0
                    @route GET /old {
                        @redirect 301 "/new-home"
                    }
                }"#,
            );
            let (addr, handle, _boot) = spawn_for_test(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                std::future::pending::<()>(),
            )
            .await
            .expect("spawn");

            let (status, _, _) = send_request(addr, "GET", "/old", None).await;
            assert_eq!(status, 301);

            handle.abort();
        })
        .await;
    }

    #[tokio::test]
    async fn server_db_create_find_roundtrip() {
        // C_db E2E: POST /users 로 row 생성, GET /users/:id 로 조회, GET /users
        // 로 전체 목록 조회. 요청 간 db 가 공유되는지 검증.
        run_on_localset(async {
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_case(
                r#"@server {
                    @listen 0
                    @route POST /users {
                        let u = await @db.create("User", @body)
                        @respond 201 u
                    }
                    @route GET /users/:id {
                        let raw: string = @param.id
                        let found = await @db.find("User", { name: raw })
                        @respond 200 found
                    }
                    @route GET /users {
                        let all = await @db.findAll("User", {})
                        @respond 200 all
                    }
                }"#,
            );
            let (addr, handle, _boot) = spawn_for_test(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                std::future::pending::<()>(),
            )
            .await
            .expect("spawn");

            // 1) 생성.
            let (s1, _, b1) = send_request(
                addr,
                "POST",
                "/users",
                Some(r#"{"name":"alice","age":30}"#.into()),
            )
            .await;
            assert_eq!(s1, 201);
            let j1: serde_json::Value = serde_json::from_slice(&b1).expect("json");
            assert_eq!(j1["id"], serde_json::json!(1));
            assert_eq!(j1["name"], serde_json::json!("alice"));

            // 2) name 으로 조회 (MVP: int.from 미구현이라 string filter 사용).
            let (s2, _, b2) = send_request(addr, "GET", "/users/alice", None).await;
            assert_eq!(s2, 200);
            let j2: serde_json::Value = serde_json::from_slice(&b2).expect("json");
            assert_eq!(j2["name"], serde_json::json!("alice"));

            // 3) 또 하나 생성 후 전체 조회.
            let (_, _, _) = send_request(
                addr,
                "POST",
                "/users",
                Some(r#"{"name":"bob","age":25}"#.into()),
            )
            .await;
            let (s3, _, b3) = send_request(addr, "GET", "/users", None).await;
            assert_eq!(s3, 200);
            let j3: serde_json::Value = serde_json::from_slice(&b3).expect("json");
            assert_eq!(j3.as_array().map(Vec::len), Some(2));

            handle.abort();
        })
        .await;
    }

    #[tokio::test]
    async fn server_level_middleware_applies_to_all_routes() {
        // SPEC §11.7: `@server { @AccessLog; @route ... }` — server block
        // 최상단의 middleware 는 이후 모든 route 에 prepend.
        run_on_localset(async {
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_case(
                r#"@server {
                    @listen 0
                    define Inject() -> @before { @next {v: "top"} }
                    @Inject
                    @route GET /a { @respond 200 { v: @context.v, kind: "a" } }
                    @route GET /b { @respond 200 { v: @context.v, kind: "b" } }
                }"#,
            );
            let (addr, handle, _boot) = spawn_for_test(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                std::future::pending::<()>(),
            )
            .await
            .expect("spawn");

            for path in ["/a", "/b"] {
                let (status, _, body) = send_request(addr, "GET", path, None).await;
                assert_eq!(status, 200, "path {path}");
                let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
                assert_eq!(json["v"], serde_json::json!("top"), "path {path}");
            }

            handle.abort();
        })
        .await;
    }

    #[tokio::test]
    async fn server_level_middleware_only_applies_to_routes_declared_after() {
        // 선언 순서 규칙: `@Cors` 이전 route 는 middleware 미적용, 이후는 적용.
        // group-flatten 과 동일 의미론.
        run_on_localset(async {
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_case(
                r#"@server {
                    @listen 0
                    define First() -> @before { @next {hdr: "first"} }
                    @route GET /before { @respond 200 { hdr: @context.hdr, tag: "pre" } }
                    @First
                    @route GET /after { @respond 200 { hdr: @context.hdr, tag: "post" } }
                }"#,
            );
            let (addr, handle, _boot) = spawn_for_test(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                std::future::pending::<()>(),
            )
            .await
            .expect("spawn");

            // /before: middleware 선언 전 → context.hdr 없음 → @context.hdr 접근 에러
            // 로 500 이 나야 한다 (handler 에 no field hdr).
            let (s_before, _, _) = send_request(addr, "GET", "/before", None).await;
            assert_eq!(s_before, 500, "/before must not have middleware applied");

            // /after: middleware 선언 뒤 → context.hdr == "first"
            let (s_after, _, b) = send_request(addr, "GET", "/after", None).await;
            assert_eq!(s_after, 200);
            let json: serde_json::Value = serde_json::from_slice(&b).expect("json");
            assert_eq!(json["hdr"], serde_json::json!("first"));

            handle.abort();
        })
        .await;
    }

    #[tokio::test]
    async fn group_and_leaf_middleware_compose_in_declared_order() {
        // 그룹 middleware → leaf route 내부 middleware 순서로 쌓여야 한다.
        // 그룹이 `role: "user"` 를 넣고, leaf 가 `role: "admin"` 으로 덮어쓴다.
        // 마지막 push 우세 규칙이 선언 순서와 일치해야 한다.
        run_on_localset(async {
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_case(
                r#"@server {
                    @listen 0
                    define Base() -> @before { @next {role: "user", gid: 1} }
                    define Elevate() -> @before { @next {role: "admin"} }
                    @route /api {
                        @Base
                        @route GET /public { @respond 200 { role: @context.role, gid: @context.gid } }
                        @route GET /secret {
                            @Elevate
                            @respond 200 { role: @context.role, gid: @context.gid }
                        }
                    }
                }"#,
            );
            let (addr, handle, _boot) = spawn_for_test(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                std::future::pending::<()>(),
            )
            .await
            .expect("spawn");

            let (s1, _, b1) = send_request(addr, "GET", "/api/public", None).await;
            assert_eq!(s1, 200);
            let j1: serde_json::Value = serde_json::from_slice(&b1).expect("json");
            assert_eq!(j1["role"], serde_json::json!("user"));
            assert_eq!(j1["gid"], serde_json::json!(1));

            let (s2, _, b2) = send_request(addr, "GET", "/api/secret", None).await;
            assert_eq!(s2, 200);
            let j2: serde_json::Value = serde_json::from_slice(&b2).expect("json");
            // leaf 내부 @Elevate 가 role 덮어씀, gid 는 Base 값 유지.
            assert_eq!(j2["role"], serde_json::json!("admin"));
            assert_eq!(j2["gid"], serde_json::json!(1));

            handle.abort();
        })
        .await;
    }

    #[tokio::test]
    async fn group_middleware_before_can_short_circuit_all_inner_routes() {
        // 그룹 middleware 의 `@respond` 로 인증 실패 단락. `/admin/*` 내 모든
        // route 가 handler 본문 실행 없이 401 을 돌려줘야 한다.
        run_on_localset(async {
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_case(
                r#"@server {
                    @listen 0
                    define Deny() -> @before { @respond 401 { err: "unauth" } }
                    @route /admin {
                        @Deny
                        @route GET /users { @respond 200 { hit: "users" } }
                        @route DELETE /users/:id { @respond 200 { hit: "deleted" } }
                    }
                }"#,
            );
            let (addr, handle, _boot) = spawn_for_test(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                std::future::pending::<()>(),
            )
            .await
            .expect("spawn");

            for (method, path) in [("GET", "/admin/users"), ("DELETE", "/admin/users/42")] {
                let (status, _, body) = send_request(addr, method, path, None).await;
                assert_eq!(status, 401, "{method} {path}");
                let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
                assert_eq!(json["err"], serde_json::json!("unauth"), "{method} {path}");
            }

            handle.abort();
        })
        .await;
    }

    #[tokio::test]
    async fn server_level_let_is_visible_to_handlers() {
        // A3: `@server { let x = ...; @route ... }` 에서 선언된 바인딩이
        // 라우트 핸들러 스코프 안에서 읽힌다. @out 같은 부트 문장과 나란히
        // 섞여 있어도 동작해야 한다.
        run_on_localset(async {
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_case(
                r#"@server {
                    @listen 0
                    @out "boot"
                    let version = "1.0.0"
                    let greeting = "hello"
                    @route GET /v { @respond 200 { v: version, g: greeting } }
                }"#,
            );
            let (addr, handle, _boot) = spawn_for_test(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                std::future::pending::<()>(),
            )
            .await
            .expect("spawn");

            let (status, _, body) = send_request(addr, "GET", "/v", None).await;
            assert_eq!(status, 200);
            let j: serde_json::Value = serde_json::from_slice(&body).expect("json");
            assert_eq!(j["v"], serde_json::json!("1.0.0"));
            assert_eq!(j["g"], serde_json::json!("hello"));

            handle.abort();
        })
        .await;
    }

    #[tokio::test]
    async fn nested_group_let_is_visible_to_handlers() {
        run_on_localset(async {
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_case(
                r#"@server {
                    @listen 0
                    @route /admin {
                        let version = "1.0.0"
                        @route GET /v { @respond 200 { v: version } }
                    }
                }"#,
            );
            let (addr, handle, _boot) = spawn_for_test(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                std::future::pending::<()>(),
            )
            .await
            .expect("spawn");

            let (status, _, body) = send_request(addr, "GET", "/admin/v", None).await;
            assert_eq!(status, 200);
            let j: serde_json::Value = serde_json::from_slice(&body).expect("json");
            assert_eq!(j["v"], serde_json::json!("1.0.0"));

            handle.abort();
        })
        .await;
    }

    #[tokio::test]
    async fn listen_can_use_top_level_binding() {
        run_on_localset(async {
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_case(
                r#"let port = 0

                @server {
                    @listen port
                    @route GET /ping { @respond 200 { ok: true } }
                }"#,
            );
            let (addr, handle, _boot) = spawn_for_test(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                std::future::pending::<()>(),
            )
            .await
            .expect("spawn");

            let (status, _, body) = send_request(addr, "GET", "/ping", None).await;
            assert_eq!(status, 200);
            let j: serde_json::Value = serde_json::from_slice(&body).expect("json");
            assert_eq!(j["ok"], serde_json::json!(true));

            handle.abort();
        })
        .await;
    }

    #[tokio::test]
    async fn listen_can_use_server_level_binding() {
        run_on_localset(async {
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_case(
                r#"@server {
                    let port = 0
                    @listen port
                    @route GET /ping { @respond 200 { ok: true } }
                }"#,
            );
            let (addr, handle, _boot) = spawn_for_test(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                std::future::pending::<()>(),
            )
            .await
            .expect("spawn");

            let (status, _, body) = send_request(addr, "GET", "/ping", None).await;
            assert_eq!(status, 200);
            let j: serde_json::Value = serde_json::from_slice(&body).expect("json");
            assert_eq!(j["ok"], serde_json::json!(true));

            handle.abort();
        })
        .await;
    }

    #[tokio::test]
    async fn server_level_let_reassignment_is_per_request() {
        // A3 하이브리드: 핸들러가 server-level `let` 을 재할당해도 per-request
        // clone 이라 다른 요청에 안 샌다. 두 번 호출 시 둘 다 counter == 1.
        run_on_localset(async {
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_case(
                r#"@server {
                    @listen 0
                    let mut counter = 0
                    @route GET /inc {
                        counter = counter + 1
                        @respond 200 { counter: counter }
                    }
                }"#,
            );
            let (addr, handle, _boot) = spawn_for_test(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                std::future::pending::<()>(),
            )
            .await
            .expect("spawn");

            // 두 번 연속 호출 — 공유 상태면 1, 2 가 나오고, per-request clone
            // 이면 둘 다 1 이 나온다. 후자가 A3 가 약속한 동작.
            let (s1, _, b1) = send_request(addr, "GET", "/inc", None).await;
            assert_eq!(s1, 200);
            let j1: serde_json::Value = serde_json::from_slice(&b1).expect("json");
            assert_eq!(j1["counter"], serde_json::json!(1));

            let (s2, _, b2) = send_request(addr, "GET", "/inc", None).await;
            assert_eq!(s2, 200);
            let j2: serde_json::Value = serde_json::from_slice(&b2).expect("json");
            assert_eq!(
                j2["counter"],
                serde_json::json!(1),
                "second request saw leaked mutation from first"
            );

            handle.abort();
        })
        .await;
    }

    #[tokio::test]
    async fn serve_directory_resolves_rest_param() {
        // A5b: `@serve "./dir"` + `@route GET /prefix/:rest* { ... }` 조합.
        // 디렉토리 대상이면 `@param.rest` 와 join 해 파일을 찾는다.
        run_on_localset(async {
            let tmp = std::env::temp_dir().join(format!("orv_serve_dir_{}", std::process::id()));
            let sub = tmp.join("sub");
            std::fs::create_dir_all(&sub).expect("mkdir");
            std::fs::write(tmp.join("index.html"), b"<h1>root</h1>").expect("w1");
            std::fs::write(sub.join("deep.txt"), b"deep file").expect("w2");

            let src = format!(
                r#"@server {{
                    @listen 0
                    @route GET /assets/:rest* {{ @serve "{}" }}
                }}"#,
                tmp.display()
            );
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_case(&src);
            let (addr, handle, _boot) = spawn_for_test(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                std::future::pending::<()>(),
            )
            .await
            .expect("spawn");

            // 1) 루트 파일
            let (s1, ct1, b1) = send_request(addr, "GET", "/assets/index.html", None).await;
            assert_eq!(s1, 200);
            assert_eq!(ct1.as_deref(), Some("text/html; charset=utf-8"));
            assert_eq!(b1, b"<h1>root</h1>");

            // 2) 하위 디렉토리 파일
            let (s2, _, b2) = send_request(addr, "GET", "/assets/sub/deep.txt", None).await;
            assert_eq!(s2, 200);
            assert_eq!(b2, b"deep file");

            // 3) 없는 파일 → 404
            let (s3, _, _) = send_request(addr, "GET", "/assets/missing.txt", None).await;
            assert_eq!(s3, 404);

            handle.abort();
            std::fs::remove_dir_all(&tmp).ok();
        })
        .await;
    }

    #[tokio::test]
    async fn serve_directory_rejects_traversal_attempts() {
        // A5b 보안: `..` 세그먼트가 포함된 rest 는 403. canonicalize 후 root
        // prefix 검사가 통과하더라도 문법적 signal 로 먼저 차단.
        run_on_localset(async {
            let tmp =
                std::env::temp_dir().join(format!("orv_serve_traverse_{}", std::process::id()));
            std::fs::create_dir_all(&tmp).expect("mkdir");
            std::fs::write(tmp.join("ok.txt"), b"ok").expect("w");
            // 바깥 파일
            let outside = tmp
                .parent()
                .unwrap()
                .join(format!("orv_serve_outside_{}.txt", std::process::id()));
            std::fs::write(&outside, b"secret").expect("w outside");

            let src = format!(
                r#"@server {{
                    @listen 0
                    @route GET /a/:rest* {{ @serve "{}" }}
                }}"#,
                tmp.display()
            );
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_case(&src);
            let (addr, handle, _boot) = spawn_for_test(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                std::future::pending::<()>(),
            )
            .await
            .expect("spawn");

            // `..` 포함 경로 — 실제로 바깥 파일을 탈출하려는 시도.
            let (status, _, _) = send_request(
                addr,
                "GET",
                &format!("/a/../orv_serve_outside_{}.txt", std::process::id()),
                None,
            )
            .await;
            // hyper 가 `/a/..` 를 정규화할 수 있으므로 403 또는 404 / 200
            // 중에 secret 은 절대 안 나와야 한다. 핵심: 200 이면 body 에
            // "secret" 이 나오지 않아야 한다.
            if status == 200 {
                panic!("traversal should not succeed");
            }

            handle.abort();
            std::fs::remove_dir_all(&tmp).ok();
            std::fs::remove_file(&outside).ok();
        })
        .await;
    }

    #[tokio::test]
    async fn serve_missing_file_returns_404() {
        run_on_localset(async {
            let missing = std::env::temp_dir().join("orv_serve_nonexistent_xyz.html");
            let _ = std::fs::remove_file(&missing);
            let src = format!(
                r#"@server {{
                    @listen 0
                    @route GET /missing {{ @serve "{}" }}
                }}"#,
                missing.display()
            );
            let ServerTestCase {
                listen,
                routes,
                body_stmts,
                captured_env,
            } = extract_server_case(&src);
            let (addr, handle, _boot) = spawn_for_test(
                listen.as_deref(),
                &routes,
                &body_stmts,
                captured_env,
                std::future::pending::<()>(),
            )
            .await
            .expect("spawn");

            let (status, _, _) = send_request(addr, "GET", "/missing", None).await;
            assert_eq!(status, 404);

            handle.abort();
        })
        .await;
    }
}
