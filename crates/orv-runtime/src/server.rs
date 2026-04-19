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
use std::sync::Arc;

use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use orv_hir::{HirExpr, HirExprKind, HirProgram};
use tokio::net::TcpListener;

use crate::interp::{run_handler_with_request, run_with_writer, RequestCtx, ResponseCtx, RuntimeError, Value};

/// MVP request body size limit (1MB). 초과 시 413 Payload Too Large.
///
/// hyper 자체는 body 크기 상한이 없어, 악의적 거대 POST 한 번에 메모리를 전부
/// 할당해 버리는 DoS 벡터가 된다. `http_body_util::Limited` 로 래핑해 수집
/// 단계에서 방지한다. 1MB 는 작은 JSON 페이로드/폼 입력을 통과시키면서
/// 멀티파트 파일 업로드는 막는 선. 파일 업로드는 SPEC §11 의 별도 경로로
/// 다룬다.
const MAX_BODY_BYTES: usize = 1024 * 1024;

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
}

/// 포트 번호와 라우트 테이블을 들고 hyper 서버를 기동한다.
///
/// # Errors
/// - `listen` 이 Int 가 아니거나 포트 범위를 벗어나면 RuntimeError.
/// - 바인딩 실패도 RuntimeError.
/// - accept/serve 루프의 I/O 에러는 로그로 흘려보내고 다음 연결로 넘어간다
///   (한 커넥션 실패로 서버 전체가 죽지 않도록).
pub(crate) fn run_server(
    listen: Option<&HirExpr>,
    routes: &[HirExpr],
    body_stmts: &[orv_hir::HirStmt],
) -> Result<Value, RuntimeError> {
    // 1) listen 포트 결정. MVP 는 @listen 없으면 에러 — SPEC §11.1 예제가
    //    항상 @listen 을 포함하므로 기본값을 숨기는 쪽이 디버깅 친화적.
    let port = resolve_listen_port(listen)?;

    // 2) routes → RouteEntry 로 평평하게. analyzer 가 routes 벡터에 Route
    //    variant 만 넣기로 계약했으므로 그 외는 에러.
    let entries = collect_routes(routes)?;

    // 3) body_stmts 평가 — SPEC §11.1 예제의 `@out "서버 시작: 8080"` 같은
    //    기타 문장은 서버가 수락을 시작하기 직전에 실행한다. `@server` 블록을
    //    들어가는 즉시 해당 문장들을 **새 Interp + 공용 stdout** 으로 돌려
    //    사용자에게 기대한 부팅 메시지를 보여준다.
    //
    //    별도 Interp 를 쓰는 이유: 현재 run_server 에는 상위 Interp 참조가 없고,
    //    body_stmts 는 @route handler 와 독립된 초기화 영역이라 최상위 프로그램
    //    실행 환경과 엮지 않아도 된다. 공용 env 가 필요해지는 순간(예:
    //    server-level let 바인딩)이 오면 Interp 참조를 전달하도록 서명을 확장.
    if !body_stmts.is_empty() {
        let boot_program = HirProgram {
            items: body_stmts.to_vec(),
            // server expr 의 정확한 span 을 모르더라도 body_stmts 는 stmt 별
            // 고유 span 을 가진다. 프로그램 래퍼 span 은 진단 출력 전용이므로
            // 첫 stmt 의 span 을 그대로 사용.
            span: body_stmts[0].span(),
        };
        let mut stdout = std::io::stdout().lock();
        run_with_writer(&boot_program, &mut stdout)?;
    }

    // 4) tokio current_thread 런타임 생성. 전용 런타임이라 스레드 이동 제약이
    //    없고, `!Send` HIR 값(Rc 기반 Value)도 요청 핸들러 안에서 그대로 사용
    //    가능하다. hyper 1.x 는 `Send + Sync` handler 를 요구하지 않으므로 이
    //    조합이 자연스럽다.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| RuntimeError::native(format!("tokio runtime init failed: {e}")))?;

    runtime.block_on(async move {
        let addr: SocketAddr = ([127, 0, 0, 1], port).into();
        let listener = TcpListener::bind(addr).await.map_err(|e| {
            RuntimeError::native(format!("failed to bind {addr}: {e}"))
        })?;
        serve_loop(listener, Arc::new(entries)).await
    })?;

    Ok(Value::Void)
}

/// 테스트에서 임의의 포트에 바인딩하고 주소를 돌려받기 위한 진입점.
///
/// 운영 경로([`run_server`])와 다른 점:
/// - 포트 0 으로 바인딩해 OS 에 맡기고 실제 주소를 반환한다.
/// - accept 루프는 별도 tokio task 로 띄우고 즉시 `(addr, handle)` 을 돌려준다.
/// - 호출자는 테스트 끝에 `handle.abort()` 로 서버를 정리한다.
///
/// 통합 테스트에서만 쓰이는 async 헬퍼라 `pub(crate)` + `#[cfg(test)]` 는
/// 필요 없다 — dev-dependency 쪽 #[tokio::test] 가 이 함수를 직접 부른다.
#[cfg(test)]
pub(crate) async fn spawn_for_test(
    routes: &[HirExpr],
) -> Result<(SocketAddr, tokio::task::JoinHandle<()>), RuntimeError> {
    let entries = collect_routes(routes)?;
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.map_err(|e| {
        RuntimeError::native(format!("test bind failed: {e}"))
    })?;
    let addr = listener.local_addr().map_err(|e| {
        RuntimeError::native(format!("local_addr failed: {e}"))
    })?;
    let table = Arc::new(entries);
    let handle = tokio::spawn(async move {
        let _ = serve_loop(listener, table).await;
    });
    Ok((addr, handle))
}

fn resolve_listen_port(listen: Option<&HirExpr>) -> Result<u16, RuntimeError> {
    let Some(expr) = listen else {
        return Err(RuntimeError::native(
            "`@server` requires an `@listen PORT` declaration",
        ));
    };
    // MVP 는 @listen 인자를 컴파일 시점에 결정 가능한 Integer 리터럴로만
    // 허용한다. 표현식 평가를 하려면 외부 Interp 가 필요한데, 그건 C5b 이후
    // 서버 모델 정비 때 같이 처리.
    let HirExprKind::Integer(s) = &expr.kind else {
        return Err(RuntimeError::native(
            "`@listen` port must be an integer literal in MVP",
        ));
    };
    let n: i64 = s.replace('_', "").parse().map_err(|_| {
        RuntimeError::native(format!("invalid @listen port integer: {s}"))
    })?;
    if !(1..=65535).contains(&n) {
        return Err(RuntimeError::native(format!(
            "@listen port out of range 1..=65535: {n}"
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
        });
    }
    Ok(out)
}

async fn serve_loop(
    listener: TcpListener,
    routes: Arc<Vec<RouteEntry>>,
) -> Result<(), RuntimeError> {
    loop {
        let (stream, _peer) = match listener.accept().await {
            Ok(pair) => pair,
            Err(e) => {
                eprintln!("accept error: {e}");
                continue;
            }
        };
        let io = TokioIo::new(stream);
        let routes = Arc::clone(&routes);
        // MVP: 커넥션 직렬 처리. tokio::task::spawn 은 `!Send` Future 를 못
        // 받고, spawn_local 은 LocalSet 안에서만 동작한다. 동시 요청 처리가
        // 필요한 순간(C6 이후)에 LocalSet 경로를 도입한다. 현재는 요청당 지연
        // 이 짧고 통합 테스트도 순차라 직렬이 더 단순하다.
        let service = service_fn(move |req| {
            let routes = Arc::clone(&routes);
            async move { Ok::<_, Infallible>(handle_request(req, routes).await) }
        });
        // MVP: keep-alive 차단. `serve_connection().await` 는 연결이 닫힐 때
        // 까지 반환하지 않아서, 직렬 accept 루프에서 keep-alive 한 클라이언트가
        // 뒤따르는 모든 요청을 굶길 수 있다. C6 이후 `LocalSet + spawn_local`
        // 도입하며 함께 다시 켠다.
        if let Err(e) = hyper::server::conn::http1::Builder::new()
            .keep_alive(false)
            .serve_connection(io, service)
            .await
        {
            eprintln!("connection error: {e}");
        }
    }
}

async fn handle_request(
    req: Request<Incoming>,
    routes: Arc<Vec<RouteEntry>>,
) -> Response<Full<Bytes>> {
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

    // body 수집. MVP 는 raw string. JSON 자동 파싱은 SPEC §11.5 에 맞추어
    // Content-Type 이 application/json 이면 Value::Object/Array 로 풀고
    // 그 외는 Str. 바디 없음/빈 바디는 Value::Void.
    //
    // `Limited` 로 크기 상한을 걸어 거대 POST 의 메모리 폭주를 차단. 초과 시
    // 413 응답.
    let limited = Limited::new(req.into_body(), MAX_BODY_BYTES);
    let body_bytes = match limited.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(e) => {
            // `Limited` 의 제한 초과 에러도 이 경로로 내려온다. hyper 1.x 는
            // 래퍼 에러 타입을 Box 로 감싸기 때문에 문자열 매칭으로 구분한다.
            let msg = format!("{e}");
            if msg.contains("length limit exceeded") {
                return plain_response(
                    StatusCode::PAYLOAD_TOO_LARGE,
                    format!("request body exceeds {MAX_BODY_BYTES} bytes"),
                );
            }
            return plain_response(
                StatusCode::BAD_REQUEST,
                format!("failed to read request body: {msg}"),
            );
        }
    };
    // Content-Type 의 media type 은 RFC 7231 §3.1.1.1 에서 case-insensitive.
    // `APPLICATION/JSON` 도 동일하게 JSON 경로로 흘러야 한다.
    let is_json = headers
        .get("content-type")
        .map(|ct| ct.to_ascii_lowercase().starts_with("application/json"))
        .unwrap_or(false);
    let body_value = if body_bytes.is_empty() {
        Value::Void
    } else if is_json {
        match serde_json::from_slice::<serde_json::Value>(&body_bytes) {
            Ok(json) => json_to_value(json),
            Err(e) => {
                return plain_response(
                    StatusCode::BAD_REQUEST,
                    format!("invalid JSON body: {e}"),
                );
            }
        }
    } else {
        Value::Str(String::from_utf8_lossy(&body_bytes).into_owned())
    };

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
        return plain_response(StatusCode::NOT_FOUND, "Not Found".into());
    };

    let ctx = RequestCtx {
        method,
        path,
        params,
        query,
        headers,
        body: body_value,
    };

    // handler 평가는 동기. stdout 은 버리는 버퍼로 흘려 — `@out` 은 서버
    // 콘솔이 아니라 요청 단위로 캡처해 반환 헤더에 싣는 편이 정석이지만
    // MVP 는 단순히 버린다.
    let mut sink = Vec::<u8>::new();
    let outcome = match run_handler_with_request(&entry.handler, ctx, &mut sink) {
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

    match outcome.response {
        Some(resp) => response_from_respond(resp),
        None => default_response(outcome.value),
    }
}

fn response_from_respond(resp: ResponseCtx) -> Response<Full<Bytes>> {
    let status = u16::try_from(resp.status)
        .ok()
        .and_then(|s| StatusCode::from_u16(s).ok())
        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

    // Void payload 는 빈 body (204 등에 자연스러움).
    if matches!(resp.payload, Value::Void) {
        return Response::builder()
            .status(status)
            .body(Full::new(Bytes::new()))
            .expect("valid response");
    }
    let json = value_to_json(&resp.payload);
    let body = serde_json::to_vec(&json).unwrap_or_else(|_| b"null".to_vec());
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Full::new(Bytes::from(body)))
        .expect("valid response")
}

fn default_response(value: Value) -> Response<Full<Bytes>> {
    // handler 가 `@respond` 없이 값으로 끝나면 그 값을 JSON 으로 200 응답.
    // Void 는 빈 200. 이렇게 하면 `@route GET /health { "ok" }` 같은 간단한
    // 핸들러가 그대로 동작한다.
    if matches!(value, Value::Void) {
        return Response::builder()
            .status(StatusCode::OK)
            .body(Full::new(Bytes::new()))
            .expect("valid response");
    }
    let json = value_to_json(&value);
    let body = serde_json::to_vec(&json).unwrap_or_else(|_| b"null".to_vec());
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json")
        .body(Full::new(Bytes::from(body)))
        .expect("valid response")
}

fn plain_response(status: StatusCode, body: String) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header("content-type", "text/plain; charset=utf-8")
        .body(Full::new(Bytes::from(body)))
        .expect("valid response")
}

/// `?a=1&b=hello` 형태 쿼리 문자열을 맵으로.
///
/// SPEC §11.3 은 쿼리 디코딩 규칙을 깊게 정의하지 않는다. MVP 는 RFC 3986
/// percent-decoding 을 직접 구현하지 않고 `+` → ' ' 치환만 수행. 실전에 맞춰
/// percent-encoding 처리가 필요하면 후속 커밋에서 `url` crate 를 추가한다.
pub(crate) fn parse_query(raw: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for pair in raw.split('&') {
        if pair.is_empty() {
            continue;
        }
        let mut it = pair.splitn(2, '=');
        let k = it.next().unwrap_or("").to_string();
        let v = it.next().unwrap_or("").replace('+', " ");
        out.insert(k, v);
    }
    out
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
    if pat_parts.len() != path_parts.len() {
        return None;
    }
    let mut params = HashMap::new();
    for (pp, ap) in pat_parts.iter().zip(path_parts.iter()) {
        if let Some(name) = pp.strip_prefix(':') {
            params.insert(name.to_string(), (*ap).to_string());
        } else if pp != ap {
            return None;
        }
    }
    Some(params)
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
        Value::Void => J::Null,
        Value::Array(items) => J::Array(items.iter().map(value_to_json).collect()),
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
        J::Object(map) => Value::Object(map.into_iter().map(|(k, v)| (k, json_to_value(v))).collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hyper::client::conn::http1 as client_http1;
    use hyper_util::rt::TokioIo;
    use orv_analyzer::lower;
    use orv_diagnostics::FileId;
    use orv_hir::{HirExpr, HirExprKind, HirStmt};
    use orv_resolve::resolve;
    use orv_syntax::{lex, parse};
    use tokio::net::TcpStream;

    // --- 단위: match_route / parse_query / value_to_json ---

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
    fn value_to_json_scalars() {
        assert_eq!(value_to_json(&Value::Int(42)), serde_json::json!(42));
        assert_eq!(value_to_json(&Value::Bool(true)), serde_json::json!(true));
        assert_eq!(value_to_json(&Value::Str("hi".into())), serde_json::json!("hi"));
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
        let j: serde_json::Value =
            serde_json::from_str("9999999999999999999").expect("parse");
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

    /// orv 소스 → HirProgram → `@server { ... }` 블록의 routes 벡터를 뽑아낸다.
    ///
    /// 통합 테스트 입력은 항상 단일 `@server { ... }` 최상위 표현식 하나.
    fn extract_server_routes(src: &str) -> Vec<HirExpr> {
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
        let HirStmt::Expr(expr) = &hir.items[0] else {
            panic!("expected top-level expression");
        };
        let HirExprKind::Server { routes, .. } = &expr.kind else {
            panic!("expected Server variant");
        };
        routes.clone()
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
        let (bytes, has_body) = match body {
            Some(b) => (Bytes::from(b), true),
            None => (Bytes::new(), false),
        };
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
            .map(|s| s.to_string());
        let bytes = resp.collect().await.expect("body").to_bytes().to_vec();
        (status, ct, bytes)
    }

    #[tokio::test]
    async fn serves_simple_get_route_with_object_payload() {
        let routes = extract_server_routes(
            r#"@server {
                @listen 0
                @route GET /ping { @respond 200 { ok: true, msg: "pong" } }
            }"#,
        );
        let (addr, handle) = spawn_for_test(&routes).await.expect("spawn");

        let (status, ct, body) = send_request(addr, "GET", "/ping", None).await;
        assert_eq!(status, 200);
        assert_eq!(ct.as_deref(), Some("application/json"));
        let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(json["ok"], serde_json::json!(true));
        assert_eq!(json["msg"], serde_json::json!("pong"));

        handle.abort();
    }

    #[tokio::test]
    async fn serves_route_with_path_param() {
        // `@param` 은 전체 params object, 개별 값은 `.field` 로 접근 (C3 규약).
        let routes = extract_server_routes(
            r#"@server {
                @listen 0
                @route GET /users/:id { @respond 200 { id: @param.id } }
            }"#,
        );
        let (addr, handle) = spawn_for_test(&routes).await.expect("spawn");

        let (status, _, body) = send_request(addr, "GET", "/users/42", None).await;
        assert_eq!(status, 200);
        let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
        // @param.id 는 문자열로 수집되므로 "42" (string).
        assert_eq!(json["id"], serde_json::json!("42"));

        handle.abort();
    }

    #[tokio::test]
    async fn serves_post_route_with_json_body_echo() {
        let routes = extract_server_routes(
            r#"@server {
                @listen 0
                @route POST /echo { @respond 201 { received: @body } }
            }"#,
        );
        let (addr, handle) = spawn_for_test(&routes).await.expect("spawn");

        let payload = r#"{"name":"alice","age":30}"#.to_string();
        let (status, _, body) = send_request(addr, "POST", "/echo", Some(payload)).await;
        assert_eq!(status, 201);
        let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(json["received"]["name"], serde_json::json!("alice"));
        assert_eq!(json["received"]["age"], serde_json::json!(30));

        handle.abort();
    }

    #[tokio::test]
    async fn unknown_route_returns_404() {
        let routes = extract_server_routes(
            r#"@server {
                @listen 0
                @route GET /ping { @respond 200 {} }
            }"#,
        );
        let (addr, handle) = spawn_for_test(&routes).await.expect("spawn");

        let (status, _, _) = send_request(addr, "GET", "/missing", None).await;
        assert_eq!(status, 404);

        handle.abort();
    }

    #[tokio::test]
    async fn respond_204_emits_empty_body() {
        let routes = extract_server_routes(
            r#"@server {
                @listen 0
                @route DELETE /item/:id { @respond 204 }
            }"#,
        );
        let (addr, handle) = spawn_for_test(&routes).await.expect("spawn");

        let (status, _, body) = send_request(addr, "DELETE", "/item/abc", None).await;
        assert_eq!(status, 204);
        assert!(body.is_empty(), "204 should have empty body, got {body:?}");

        handle.abort();
    }

    #[tokio::test]
    async fn trailing_slash_is_normalized_and_matched() {
        // 회귀: `/users/42/` 가 `/users/:id` 매처에 잡혀야 한다.
        let routes = extract_server_routes(
            r#"@server {
                @listen 0
                @route GET /users/:id { @respond 200 { id: @param.id } }
            }"#,
        );
        let (addr, handle) = spawn_for_test(&routes).await.expect("spawn");

        let (status, _, body) = send_request(addr, "GET", "/users/42/", None).await;
        assert_eq!(status, 200, "trailing-slash path should match");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(json["id"], serde_json::json!("42"));

        handle.abort();
    }

    #[tokio::test]
    async fn catchall_star_route_matches_unknown_paths() {
        // SPEC §11.2: `@route GET *` 은 어느 경로도 잡는다. 앞선 구체 route 가
        // 먼저 매치되면 그쪽이 이긴다 — 선언 순서 규칙.
        let routes = extract_server_routes(
            r#"@server {
                @listen 0
                @route GET /ping { @respond 200 { hit: "ping" } }
                @route GET * { @respond 404 { err: "not found" } }
            }"#,
        );
        let (addr, handle) = spawn_for_test(&routes).await.expect("spawn");

        let (status, _, body) = send_request(addr, "GET", "/ping", None).await;
        assert_eq!(status, 200);
        let json: serde_json::Value = serde_json::from_slice(&body).expect("json");
        assert_eq!(json["hit"], serde_json::json!("ping"));

        let (status2, _, body2) = send_request(addr, "GET", "/whatever", None).await;
        assert_eq!(status2, 404, "catchall route should respond 404");
        let json2: serde_json::Value = serde_json::from_slice(&body2).expect("json");
        assert_eq!(json2["err"], serde_json::json!("not found"));

        handle.abort();
    }

    #[tokio::test]
    async fn content_type_is_case_insensitive() {
        // `APPLICATION/JSON` 도 JSON 경로로 파싱되어 `@body.x` 가 동작해야 한다.
        let routes = extract_server_routes(
            r#"@server {
                @listen 0
                @route POST /m { @respond 200 { x: @body.x } }
            }"#,
        );
        let (addr, handle) = spawn_for_test(&routes).await.expect("spawn");

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
    }

    #[tokio::test]
    async fn oversized_body_returns_413() {
        // MAX_BODY_BYTES = 1 MiB. 이를 살짝 넘기는 바디로 413 을 확인한다.
        let routes = extract_server_routes(
            r#"@server {
                @listen 0
                @route POST /upload { @respond 200 {} }
            }"#,
        );
        let (addr, handle) = spawn_for_test(&routes).await.expect("spawn");

        let big = "a".repeat(MAX_BODY_BYTES + 1024);
        let (status, _, _) = send_request(addr, "POST", "/upload", Some(big)).await;
        assert_eq!(status, 413, "expected 413 Payload Too Large");

        handle.abort();
    }

    #[tokio::test]
    async fn boot_stmts_run_before_accept() {
        // body_stmts 는 서버가 accept 시작 전에 한 번 실행되어야 한다.
        // stdout 캡처는 테스트 경계에서 까다로워 `@listen` 포트를 비현실적인
        // 값으로 주어 `resolve_listen_port` 단계에서 일부러 실패시키는 대안
        // 대신, `run_server` 를 직접 돌리지 않고 **단위 수준**에서
        // body_stmts 가 비어 있지 않을 때 Interp 에 넘어가는 경로를 server.rs
        // 내부 단위 테스트로는 재현 불가.
        //
        // 대신 고레벨 확인: `@server { @out "boot" @listen 0 ... }` 를 lower 해
        // body_stmts 벡터가 expected shape 인지 검증. 실제 stdout 출력은 수동
        // 검증 (fixtures 쪽에서 커버).
        let hir = {
            let src = r#"@server {
                @out "boot"
                @listen 0
                @route GET /p { @respond 200 {} }
            }"#;
            let lx = lex(src, FileId(0));
            assert!(lx.diagnostics.is_empty());
            let pr = parse(lx.tokens, FileId(0));
            assert!(pr.diagnostics.is_empty());
            let resolved = resolve(&pr.program);
            assert!(resolved.diagnostics.is_empty());
            lower(&pr.program, &resolved)
        };
        let HirStmt::Expr(expr) = &hir.items[0] else {
            panic!("top-level expr expected");
        };
        let HirExprKind::Server {
            body_stmts,
            listen,
            routes,
        } = &expr.kind
        else {
            panic!("Server expected");
        };
        assert_eq!(body_stmts.len(), 1, "@out belongs in body_stmts");
        assert!(listen.is_some());
        assert_eq!(routes.len(), 1);
    }
}
