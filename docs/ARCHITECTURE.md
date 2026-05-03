# orv 아키텍처

## 개요

orv는 Rust workspace로 구성된 10개 크레이트의 파이프라인 아키텍처를 따른다. 현재 구현은 `.orv` 소스를 로드/파싱/해석/분석한 뒤 HIR을 레퍼런스 tree-walking 런타임으로 실행하는 MVP다. `orv-compiler`는 HIR 기반 origin map과 build manifest artifact를 생성할 수 있고, `@server` 런타임은 매칭된 route origin id를 HTTP 응답 헤더로 노출하며 JSON과 form-urlencoded 요청 body를 `@body` 값으로 제공한다. `orv init`은 최소 `orv.toml`/`src/main.orv` 프로젝트 또는 `--template shop` 쇼핑몰 `GET /` HTML form 홈과 회원/결제/배송 route scaffold 및 검증/배포 README를 만들고, source-entry CLI는 단일 파일뿐 아니라 `orv.toml` `[project].entry`와 프로젝트 디렉터리 입력도 받는다. `orv build`는 현재 manifest/origin-map/project-graph/source-bundle/source-bundled server runtime artifact JSON과 reference server launch artifact 디렉터리를 만들며, HTML-only entry는 zero-runtime `pages/index.html` 정적 페이지도 출력한다. `let sig` 또는 HTML await가 필요한 client entry는 non-zero-runtime `pages/index.html` shell, source-bundle/wasm target metadata를 노출하고 `orv_start` WASM export를 호출하는 `client/app.js` bootstrap, `orv.client` custom section과 `orv_start` export를 담은 `client/app.wasm` callable placeholder로 client bundle 계약을 고정한다. `orv build --prod`는 여기에 `deploy/manifest.json`, `deploy/routes.json`, `deploy/container.json`, `deploy/Dockerfile`, `deploy/compose.yaml`, request trace capture/editor trace 명령을 담은 `deploy/README.md`, `deploy/server.sh` reference server entrypoint를 추가한다. CLI는 build directory target, deploy manifest, server/client/source-bundle artifact 검증, artifact 재분석, reference runtime 실행, build directory launch 실행, build artifact origin reveal을 할 수 있다. native 서버 바이너리와 실제 클라이언트 WASM/JS 코드젠은 아직 구현되지 않은 컴파일러 로드맵이다.

이 문서는 **현재 구현 구조와 데이터 흐름**을 설명하는 문서다. 언어 문법과 의미론의 공식 기준은 `docs/SPEC.md`이며, 이 문서는 그 사양을 구현 관점에서 해설한다.

소스 위치 타입(`Span`, `ByteRange`)은 별도 크레이트 대신 `orv-diagnostics`에 통합되어 진단 메시지와 함께 관리된다.

## 컴파일 파이프라인

```
  orv-cli (`run` / `check` / `dump` / `origins` / `graph` / `build` / `*-artifact`)
      │
      ▼
┌──────────────┐
│ orv-project  │  파일/Import 로드 + ProjectGraph v1
│ + orv-syntax │  각 파일 lex/parse
└──────┬───────┘
       │ 병합된 AST Program + source map + ProjectGraph v1
      │
      ▼
┌─────────────┐     ┌──────────────────────┐
│ orv-resolve │────▶│ 이름 해석 결과        │
│ 스코프 분석  │     │ (바인딩, 스코프 연결)  │
└─────────────┘     └──────────────────────┘
      │
      ▼
┌──────────────┐     ┌─────────────────┐
│ orv-analyzer │────▶│  HIR (고수준 IR) │
│ 의미 분석     │     └─────────────────┘
└──────────────┘
      │
      ▼
┌──────────────┐     ┌──────────────────────────┐
│ orv-runtime  │────▶│ 레퍼런스 실행 결과        │
│ 인터프리터    │     │ (`@server`는 HTTP/1.1)   │
└──────────────┘     └──────────────────────────┘
```

진단/위치 정보(`orv-diagnostics`)는 파이프라인의 모든 단계가 공통으로 사용한다. `orv-core`/`orv-macros`는 공유 인프라로 별도 단계 없이 여러 크레이트가 참조한다. `orv-compiler`는 현재 origin map과 build manifest artifact 생성을 담당하며, 향후 최적화/번들링 단계가 들어갈 자리다.

## 크레이트 상세

| 크레이트 | 역할 | 주요 의존성 |
|----------|------|------------|
| `orv-diagnostics` | 소스 위치(`Span`, `ByteRange`) + 구조화된 컴파일러 진단 메시지. codespan-reporting 기반 포매팅 | codespan-reporting |
| `orv-macros` | Rust proc-macro 유틸리티. 컴파일러 내부에서 사용하는 derive 매크로 | syn, quote, proc-macro2 |
| `orv-core` | 핵심 타입 정의와 공유 인프라. 모든 크레이트가 공통으로 사용하는 타입 | orv-macros, orv-diagnostics, wgpu |
| `orv-syntax` | 렉서(Lexer)와 파서(Parser). `.orv` 소스를 AST로 변환 | orv-diagnostics |
| `orv-resolve` | 이름 해석(Name Resolution)과 스코프 분석. AST의 식별자를 선언에 연결 | orv-diagnostics, orv-syntax |
| `orv-hir` | 고수준 중간 표현(HIR) 정의. 의미 분석 이후의 타입 정보와 origin id 계산 규칙이 포함된 IR | — |
| `orv-analyzer` | 의미 분석(Semantic Analysis)과 HIR 로우어링. 타입 검사, 도메인 검증 | orv-diagnostics, orv-hir, orv-resolve, orv-syntax |
| `orv-project` | entry 파일에서 import를 따라 멀티파일 프로그램을 로드/병합하고, 파일/import/선언/domain 기반 AST ProjectGraph v1을 추출 | orv-syntax, orv-diagnostics, thiserror, serde |
| `orv-compiler` | HIR origin map과 build manifest artifact 생성. 코드 생성/최적화 단계는 로드맵 | orv-diagnostics, orv-hir, serde |
| `orv-runtime` | 레퍼런스 tree-walking 런타임. `@server`는 hyper HTTP/1.1 서버로 실행하며 route 응답에 `x-orv-origin-id`를 붙인다 | orv-diagnostics, orv-hir, orv-syntax, serde, serde_json, regex, thiserror, tokio, hyper |
| `orv-cli` | CLI 프론트엔드. 현재 `init`, `run`, `dev`, `check`, `dump`, `origins`, `graph`, `test`, `lsp snapshot/reveal/serve --stdio`, `dap serve --stdio`, `build`, `build --prod`, `verify-build`, `verify-artifact`, `check-artifact`, `run-artifact`, `run-build`, `reveal`, `db plan/verify/apply/migrate/rollback/backup/restore/recover/archive/squash`으로 프로젝트 scaffold, 로드/해석/분석/실행, origin map 출력, ProjectGraph v1 출력, test block runner bootstrap, editor bootstrap snapshot, LSP-style production reveal, stdio LSP initialize/shutdown/documentSymbol/codeLens/codeAction/documentLink/foldingRange/selectionRange/semanticTokens/diagnostic/workspaceDiagnostic/executeCommand/definition/references/documentHighlight/prepareRename/rename/hover/signatureHelp/completion/workspaceSymbol와 didOpen/didChange open-buffer cache, stdio DAP initialize/launch/attach/configurationDone/setBreakpoints/setFunctionBreakpoints/setInstructionBreakpoints/dataBreakpointInfo/setDataBreakpoints/breakpointLocations/gotoTargets/stepInTargets/exceptionInfo/threads/stackTrace/scopes/variables/setVariable/evaluate/setExpression/completions/loadedSources/modules/source/disassemble/continue/reverseContinue/goto/stepIn/stepBack/restartFrame/disconnect/terminate/terminateThreads bootstrap, `orv.toml` `[project].entry` source-entry resolution, 초기 build artifact 및 prod deploy manifest 출력/검증/재분석/reference 실행, build+verify+run-build dev bootstrap, DB schema/data snapshot workflow를 오케스트레이션 | orv-core, orv-diagnostics, orv-syntax, orv-resolve, orv-analyzer, orv-hir, orv-compiler, orv-project, orv-runtime, clap |

DAP bootstrap은 `orv-cli` 안에서 프로젝트 로더와 AST/ProjectGraph를 재사용한다. `setBreakpoints`는 source path 또는 loaded sourceReference를 받아 nested AST statement line까지 executable line으로 verified 처리하고 Locals 기반 condition/hitCondition을 평가하며, `logMessage`는 멈추지 않는 console logpoint로 출력한다. `setFunctionBreakpoints`는 captured runtime call-stack 함수 이름에 매칭하며, `setInstructionBreakpoints`는 ORV가 stable instruction address를 제공하지 않는다는 unverified breakpoint response를 반환한다. `disassemble`은 `orv:frame:<n>` memoryReference를 captured runtime source-frame pseudo instruction 목록으로 매핑한다. `dataBreakpointInfo`/`setDataBreakpoints`는 현재 Locals 변수의 값 변경 frame에 매칭한다. `gotoTargets`는 sourceReference+line target id를 만든다. `stepInTargets`는 현재 runtime frame에서 직접 진입 가능한 captured function call target id를 만든다. `setExceptionBreakpoints`는 ORV diagnostics/runtime filters를 수용하고 선택된 축만 launch stopped reason을 `exception`으로 만든다. 기본 `launch`는 `orv-runtime`의 debug trace로 실행된 HIR statement frame, active function call stack, 실제 env locals, stdout delta를 캡처하며, `attach`는 같은 launch pipeline을 `attachRuntime=true`로 실행한다. `launch.arguments.live=true`는 `DebugStepper`로 첫 frame까지만 실행한 뒤 `next`/`stepIn`/`stepOut`/`continue`에서 interpreter state를 계속 전진시킨다. `@server`처럼 장기 실행 런타임을 시작할 수 있는 launch는 `paused` long-running frame으로 열어 `continue`/`pause` event loop를 제공한다. initialize는 custom `supportsOrvRuntimeAttach`/`supportsOrvRuntimeTracePath`와 standard `supportsDisassembleRequest` capability를 광고한다. 기본값은 HTTP accept loop를 직접 기동하지 않지만, `launch.arguments.attachRuntime=true` 또는 `attach` request는 `continue`에서 런타임을 attach한다. 기본 process mode는 child `orv run <program>` process와 process STOP/CONT transport를 사용하고, `attachRuntimeMode="inProcess"`는 같은 DAP process 안의 attached server thread와 shutdown/restart transport를 사용한다. runtime JSON과 `variables`/`evaluate`/`completions`에는 async runtime kind/state, static/env listen endpoint, route inventory, pause/resume count, transport kind/state/process id 또는 address를 노출한다. in-process attached runtime은 실제 HTTP 요청의 method/path/status/route/params/query/body summary를 request frame으로 캡처하고 `runtimeRequestCount`/`runtimeLastRequest`/`runtimeRequestFrames` inspect 값을 제공한다. `loadedSources`/`modules`는 같은 프로젝트 파일 inventory를 source 또는 module shape로 노출하고, `source`는 disk를 다시 읽지 않고 launch-time source snapshot content를 반환한다. Debug-control 요청은 ORV reference thread id `1`만 허용하고 unknown thread id를 거부한다. `scopes`/`variables`/`evaluate`/`completions`는 현재 frame의 runtime value를 읽는다. `setVariable`/`setExpression`은 현재 frame Locals snapshot 값을 갱신한다. `stackTrace`는 captured function frame hierarchy를 반환한다. `next`는 같은 call-stack 깊이의 다음 frame으로 step-over하고, `stepIn`은 captured mode에서 targetId가 있으면 선택된 function call frame으로, targetId가 없으면 다음 captured runtime frame 또는 live runtime frame으로 진입하며, live mode의 targetId는 거부한다. `stepOut`은 caller frame으로 빠져나가고, 마지막 frame 이후 session을 종료한다. `stepBack`은 직전 captured frame으로, `restartFrame`은 현재 captured function frame의 첫 runtime frame으로, `reverseContinue`는 이전 line/function/data breakpoint frame으로 이동한다. `goto`는 target id에 대응하는 captured runtime frame으로 이동한다. `continue`는 남은 line/function/data breakpoint frame으로 이동하거나, live mode에서는 다음 breakpoint까지 실제 stepper를 실행하고 종료 시 terminated event를 낸다. `restart`는 현재 launch program을 재로딩하고 명시적 override가 없으면 이전 `live=true`/`attachRuntime=true` 모드를 보존하며, stdio transport는 response 뒤에 output/lifecycle event frame을 flush한다.

## 의존성 그래프

```
orv-cli
├── orv-project ──▶ orv-syntax ──▶ orv-diagnostics
├── orv-resolve ──▶ orv-syntax ──▶ orv-diagnostics
├── orv-analyzer ─▶ orv-hir / orv-resolve / orv-syntax / orv-diagnostics
├── orv-runtime ──▶ orv-hir / orv-syntax / orv-diagnostics
└── orv-compiler ─▶ orv-hir / orv-diagnostics / serde

orv-core ──▶ orv-macros / orv-diagnostics
```

## 데이터 흐름

### 0단계: 프로젝트 로드 (orv-project)

```
entry .orv → import DFS → 병합된 AST Program + source map + ProjectGraph v1
```

entry 파일에서 시작해 `import` 문을 따라 `.orv` 파일을 재귀적으로 로드한다. 현재는 import 된 파일의 top-level 문장을 entry 앞에 붙여 하나의 AST `Program`으로 병합하고, `LoadedProject`에 파일별 source map과 AST 기반 `ProjectGraph`를 함께 담는다. 같은 병합 규칙은 파일 시스템 로드와 build artifact source bundle 재수화 모두에 적용된다. ProjectGraph v1은 `File`, `Import`, `Struct`, `Enum`, `TypeAlias`, `Function`, `Define`, `Domain` 노드와 `Contains`, `Imports` 엣지를 제공한다. 파일별 scope 격리, visibility enforcement, 외부 레지스트리 의존성, 정교한 사이클 진단은 로드맵이다.

### 0.5단계: 프로젝트 그래프 v1 (orv-project)

```
AST Program + source map → ProjectGraph v1
```

현재 `orv-project`는 AST와 source map만으로 멀티파일 프로젝트의 구조 그래프를 만든다. 이 그래프는 파일, import, 선언, domain 경계를 표현하고, 파일이 포함하는 노드와 import 대상 파일을 연결한다. `orv graph <file>`은 이 구조 그래프와 HIR origin map을 함께 JSON으로 출력하고, `stats`에 node/edge/file/import/declaration/domain count, source `contains` 최대 깊이, semantic origin/edge/call count, semantic `contains` 최대 깊이를 담는다. exact span 매칭이 가능한 origin에는 `semantic.origin_links`로 AST node id를 붙인다. 또한 `orv-compiler`가 생성한 HIR origin edge를 `semantic.origin_edges`로 노출한다. 현재 edge는 traversal parent stack 기반 `contains`와 call expression에서 resolved function으로 이어지는 `calls`를 포함하므로, `server -> route -> respond` 같은 의미 실행 계층과 `call -> function` 호출 관계를 볼 수 있다. 따라서 route/respond/function/call 같은 의미 실행 노드와 원본 구조 노드를 같은 artifact에서 볼 수 있다. CLI 진단도 source map을 사용해 import된 파일의 `FileId`와 실제 경로를 맞춰 출력한다.

### 1단계: 파싱 (orv-syntax)

```
.orv 소스 텍스트 → 토큰 스트림(Lexer) → AST(Parser)
```

소스 텍스트를 토큰으로 분해한 뒤 구문 트리(AST)를 생성한다. 모든 노드에 `Span` 정보가 부착되어 에러 보고 시 정확한 위치를 가리킨다.

### 2단계: 이름 해석 (orv-resolve)

```
AST → 스코프 테이블 + 바인딩 맵
```

식별자를 선언에 연결하고 스코프 계층을 구성한다. `import`/`pub` 가시성 규칙을 적용한다.

### 3단계: 의미 분석 (orv-analyzer)

```
AST + 바인딩 맵 → HIR
```

타입 검사, 도메인 유효성 검증, 스키마 제약조건 확인을 수행한다. 결과를 HIR(고수준 중간 표현)로 로우어링한다.

### 4단계: 레퍼런스 실행 (orv-runtime)

```
HIR → tree-walking 실행
```

현재 런타임은 HIR을 직접 평가한다. 일반 표현식, 함수, 타입/캐스트, HTML 값, 서버 라우트, 인메모리 `@db`, 명시적 `@db.save/load` JSON snapshot, `@db.wal(path)` JSONL append+fsync WAL replay with `ts_unix_ms` record timestamps, `@db.checkpoint()` WAL snapshot compaction, `@db.savepoint()`/`@db.rollback(point)` 메모리 상태 복원, 정적 파일 `@serve`, 그리고 일부 고급 도메인의 reference stub을 실행한다. CLI `orv db recover`는 raw WAL 또는 WAL archive manifest를 complete record count, unix ms timestamp, 또는 RFC3339 timestamp 경계까지 재생해 `@db.save` 호환 snapshot으로 복구하고, archive manifest 경로는 WAL hash/byte count를 검증한 뒤 사용한다. `orv db archive`는 WAL record/timestamp/hash manifest를 생성하며 `--target file://...`이면 WAL과 manifest를 file archive target으로 복사한다. `@server`는 tokio current-thread 런타임과 hyper HTTP/1.1 서버를 사용하며, 매칭된 route의 origin id를 `x-orv-origin-id` 응답 헤더에 싣는다. 런타임은 attached server request frame을 공유 `orv.production.trace` JSON schema/file로 직렬화하는 helper도 소유하고, `ORV_RUNTIME_REQUEST_TRACE_PATH`가 있으면 graceful shutdown 때 같은 trace file을 쓴다.

### 4.5단계: Origin map artifact (orv-compiler)

```
HIR → origin map JSON
```

현재 `orv-compiler`는 HIR의 실행 가능한 도메인/라우트/응답/호출 노드에서 안정적인 origin id, source span fingerprint, traversal 기반 parent-child `contains` edge, call expression에서 resolved function으로 이어지는 `calls` edge를 생성한다. `orv origins <file>`은 이 artifact를 JSON으로 출력한다. `orv reveal <dir> <origin-id>`는 build artifact directory의 origin map, ProjectGraph, server runtime artifact, bundle plan을 읽어 source span, graph node, route descriptor 또는 client bundle target을 JSON으로 반환한다. `orv editor reveal <dir> <origin-id>`는 같은 origin id를 first-party editor focus/source/production navigation payload로 변환한다. `orv editor trace <dir> --trace <trace.json>`은 captured request trace frame의 origin id를 같은 editor navigation payload로 확장한다. DAP in-process attach와 env 기반 server run capture는 `orv-runtime`의 공유 `orv.production.trace` JSON schema/file writer를 사용하고, 외부 live trace streaming transport는 로드맵이다.

### 4.6단계: 초기 build artifact (orv-compiler + orv-cli)

```
HIR + ProjectGraph v1 → build-manifest.json + bundle-plan.json + origin-map.json + project-graph.json + source-bundle.json + server/app.orv-runtime.json + server/launch.json | pages/index.html | client/app.js | client/app.wasm
```

현재 `orv build <file-or-orv.toml> --out <dir>`은 native 프로덕션 바이너리를 만들지 않고 deterministic build artifact directory를 생성한다. `build-manifest.json`은 `reference-interpreter` runtime, artifact 목록, 서버 route 수, client WASM 포함 여부, HIR origin map에서 추론한 `runtime_features`를 기록한다. 예를 들어 서버 route는 `http_server`/`router`, `@db` 사용은 `in_memory_db`, `@html` 사용은 `html_renderer`, `let sig` 또는 HTML await는 `client_wasm`, `@serve` 사용은 `static_file_server`를 요구한다. `bundle-plan.json`은 이 capability에서 future bundler가 만들 target을 선언하며, 현재 서버 입력은 `server/app.orv-runtime.json`과 `server/launch.json` output으로 이어진다. 모든 build는 `source-bundle.json`에 source path/source/content hash snapshot을 기록해 원본 파일 없이도 reveal/LSP reveal이 source span을 복구할 수 있게 한다. Server 없는 HTML-only entry는 `static_page` target과 `pages/index.html`을 만들고, 이 target의 `runtime_features`는 빈 배열이라 배포 산출물에 런타임 계층을 싣지 않는 zero-runtime 계약을 시작한다. Interactive HTML entry는 zero-runtime static page 대신 `client_page`/`client_js`/`client_wasm` target과 `pages/index.html`, `ORV_CLIENT_BOOTSTRAP` metadata를 export하고 `orv_start`를 호출하는 `client/app.js`, `orv.client` custom section과 `orv_start` function export를 담은 유효 WASM module인 `client/app.wasm`을 출력해 future WASM bundler path와 source-bundle 연결을 검증한다. `orv build --prod`는 static `@listen 0`을 test-only ephemeral port로 거부하고, `deploy/manifest.json`에 prod profile, runtime features, source/server/static/client targets를 기록하고, 서버가 있으면 `deploy/routes.json` route inventory, `deploy/container.json` reference container contract with static/env listen/ports, `deploy/Dockerfile` with static or env-default EXPOSE, `deploy/compose.yaml` with matching build args/ports/environment, route-aware `deploy/README.md` runbook with request trace capture/editor trace commands, `deploy/server.sh` entrypoint를 만들어 `orv run-artifact` 기반 reference server 컨테이너 배포 실행 경로를 고정한다. `orv verify-build <dir>`은 manifest artifact path, source bundle hash, bundle target path, deploy manifest/entrypoint/routes inventory/container/Dockerfile/Compose/runbook, server runtime artifact/launcher 검증, static page zero-runtime/HTML shape, client page shell, client JS bootstrap metadata/`orv_start` call, client WASM magic/version, 파싱된 `orv.client` custom metadata field, `orv_start` export, optional `dev/session.json` HMR 계약과 `dev/watch.json` watch loop 계약을 검사한다. 이 server artifact는 entry/runtime/runtime_features, listen origin/static/env port descriptor, route method/path/origin id, source bundle path/source/content hash를 담아 production-to-code 추적과 future runner hydration 계약을 고정한다. `server/launch.json`은 reference runner 명령(`orv run-artifact server/app.orv-runtime.json`), HTTP/1 protocol, listen descriptor, route 목록을 담아 native binary 전 단계의 배포 실행 계약을 고정한다. `orv run-build <dir>`은 `bundle-plan.json`의 target을 기준으로 launcher 계약을 검증한 뒤 `server/app.orv-runtime.json`을 실행하고, server 없는 static/client page build에서는 verified HTML을 stdout으로 출력한다. `orv dev <file-or-orv.toml> --out <dir>`은 현재 build, verify-build, run-build를 순서대로 묶는 reference dev bootstrap이고, `--hmr`은 `dev/session.json`에 source hash watch set, bundle targets, hot-reload/full-reload fallback 전략을 기록하며, `--watch`는 `dev/watch.json`에 poll loop/watch target/manifest transport 계약을 기록한다. `orv verify-artifact <file>`은 source hash와 route descriptor shape를 검증하고, `orv check-artifact <file>`은 artifact source bundle을 import 포함 in-memory project로 다시 lex/parse/resolve/lower 하며, `orv run-artifact <file>`은 같은 source bundle을 재수화해 reference runtime으로 실행한다. `orv reveal <dir> <origin-id>`는 `origin-map.json`, `project-graph.json`, `source-bundle.json`, server runtime artifact, bundle plan을 결합해 해당 origin의 source snippet, graph node, route artifact 또는 client bundle target을 보여준다. `origin-map.json`과 `project-graph.json`은 `orv origins`/`orv graph`와 같은 compiler/source graph 정보를 보존한다. 이 단계는 production bundler가 사용할 zero-overhead 입력 계약을 먼저 고정하는 목적이다.

### 로드맵: 의미 기반 프로젝트 그래프 확장

```
HIR + ProjectGraph v1 → 의미 기반 프로젝트 그래프
```

라우트-페이지 연결, 데이터 의존성, 호출 그래프, 번들 포함 여부, DB schema 영향 범위처럼 의미 분석이 필요한 관계는 HIR 기반 확장 단계에서 추가한다.

### 로드맵: 최적화 및 코드 생성 (orv-compiler)

```
HIR + 프로젝트 그래프 → 최적화된 출력 코드
```

향후 프로젝트 특화 최적화를 수행한다:
- **DCE**: 모듈/도메인/기능 수준 데드 코드 제거
- **Auto-batching**: 루프 내 fetch를 단일 배치 요청으로 변환
- **Auto-parallelization**: 독립적 쿼리의 병렬 실행
- **렌더링 전략 추론**: 페이지별 SSG/CSR/SSR 자동 결정
- **번들 분할**: 서버/클라이언트 코드 자동 분리

### 로드맵: 번들 출력

```
최적화된 코드 → 실행 가능 번들
```

서버 네이티브 바이너리와 실제 클라이언트 WASM/JS 코드젠은 아직 구현되어 있지 않다. 현재는 `let sig` 또는 client-side HTML await가 필요한 entry에서 page shell, `ORV_CLIENT_BOOTSTRAP` metadata JS bootstrap, `orv.client` custom section과 `orv_start` export를 담은 유효 WASM module인 `client/app.wasm`을 출력해 bundle/verify/deploy 계약을 먼저 고정한다.

## 로드맵 번들 출력 구조

```
dist/
├── server
│   ├── app              # 서버 네이티브 바이너리 (Rust 컴파일)
│   └── launch.json      # 현재 MVP reference runner launch 계약
├── client/
│   ├── app.wasm          # 클라이언트 WASM (sig 사용 페이지만)
│   ├── app.js            # WASM 바인딩 글루 코드
│   └── style.css         # @design에서 추출된 스타일
├── static/               # @serve로 지정된 정적 에셋
└── pages/
    └── *.html            # SSG 페이지 (정적 렌더링 결과)
```

### 번들 분할 전략

| 페이지 특성 | 출력 | JS/WASM 포함 |
|-------------|------|-------------|
| sig 없는 정적 페이지 | `pages/*.html` | 없음 (Zero-runtime) |
| sig 있는 대화형 페이지 | `pages/*.html` + `client/app.wasm` | WASM |
| 서버 라우트 | `server/app` | — |
| 혼합 (SSR + 대화형) | 서버 렌더링 + 부분 hydration | 해당 컴포넌트만 WASM |

## 보조 인프라

### 진단 (orv-diagnostics)

컴파일러의 모든 단계에서 발생하는 에러, 경고, 힌트를 `codespan-reporting` 기반으로 포매팅한다. `Span` 정보를 활용해 소스 코드의 정확한 위치를 밑줄과 함께 표시한다.

### Proc-macro (orv-macros)

컴파일러 내부에서 반복적인 보일러플레이트를 줄이기 위한 derive 매크로를 제공한다. `syn`/`quote` 기반.

### CLI (orv-cli)

`clap` 기반 CLI로 다음 커맨드를 제공한다:
- `orv init <dir> --name <name> [--template basic|shop]` — 최소 프로젝트 또는 쇼핑몰 `GET /` HTML form 홈, route scaffold, 검증/Compose 배포 README 생성
- `orv run <file>` — 파일을 로드/검사한 뒤 레퍼런스 런타임으로 실행
- `orv check <file>` — 파싱, 이름 해석, 타입/도메인 진단만 수행
- `orv dump <file>` — AST 디버그 출력
- `orv origins <file>` — HIR 기반 origin map JSON 출력
- `orv graph <file>` — AST ProjectGraph v1 + HIR origin map/edge JSON 출력
- `orv test <path> --filter <name> --list` — `.orv` 파일을 찾아 `test "name"` 블록이 있는 파일을 reference runtime 으로 실행하거나 발견 목록 JSON 출력
- `orv editor snapshot/reveal/runtime/export/trace` — first-party editor bootstrap JSON, source-hash watch set, build-origin navigation payload, runtime inspection pane JSON, static editor shell artifact with panel-list/trace-detail rendering and optional trace state, captured request trace navigation payload 출력
- `orv build <file-or-orv.toml> --out <dir> [--prod]` — 초기 build manifest + bundle plan + origin map + project graph + server runtime/launch artifact, HTML-only static page, 또는 client page/JS/WASM bootstrap 출력, prod profile이면 deploy manifest/container/Dockerfile/Compose/runbook/entrypoint 추가
- `orv verify-build <dir>` — build manifest/plan target 존재, source bundle hash, deploy container/runtime image/Compose/runbook contract, server artifact, static page zero-runtime shape, client page/JS/WASM bootstrap, optional dev HMR/watch session 검증
- `orv verify-artifact <file>` — server runtime artifact source hash/route descriptor 검증
- `orv check-artifact <file>` — server runtime artifact source bundle 재분석
- `orv check-build <dir>` — build-level source bundle 재분석
- `orv run-artifact <file>` — server runtime artifact source bundle 재수화 + reference runtime 실행
- `orv run-build <dir>` — bundle plan 기준 reference server artifact 실행, 또는 static page HTML 출력
- `orv dev <file-or-orv.toml> --out <dir> [--hmr] [--watch]` — build + verify-build + run-build reference dev bootstrap, optional HMR/watch session manifest

로드맵 커맨드:
- native 서버 바이너리 + 실제 클라이언트 WASM/JS 코드젠/글루 번들 빌드
- 지속 실행되는 HMR 개발 서버와 browser/server transport
- 이름별 단일 test case 실행/async test isolation — 전체 test runner 확장

## Lint 정책

```toml
[workspace.lints.rust]
unsafe_code = "forbid"       # unsafe 코드 전면 금지
unused_must_use = "deny"

[workspace.lints.clippy]
all = "deny"                 # 모든 clippy 경고를 에러로
pedantic = "warn"            # 엄격한 스타일 검사
nursery = "warn"             # 실험적 린트 활성화
```
