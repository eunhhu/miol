# orv

**프로젝트 특화(Project-Specialized) 풀스택 언어 플랫폼**

> 단순한 언어가 아니다. **언어 + 컴파일러 + 에디터 + 런타임 + 디자인 시스템**이 **단일 프로젝트 그래프**를 공유하는 통합 플랫폼이다.

## 철학

### 북극성 목표 — "비개발자가 AI 없이 5시간 만에 쇼핑몰을 만든다"

생산성 목표는 선택이 아니라 측정 기준이다. Copilot·Cursor 같은 AI 어시스턴트에 의존하지 않고도, 코딩 경험이 거의 없는 사람이 하루가 가기 전에 **실제 결제·배송·회원이 동작하는 쇼핑몰**을 배포할 수 있어야 한다. 이 북극성이 언어 문법부터 에디터 UX, 런타임, 표준 라이브러리, 디자인 토큰까지 모든 설계 결정의 기준이 된다.

이 목표가 자연스러워지려면:

- **우발적 복잡성(accidental complexity) 제거** — 빌드 도구 체인, 프레임워크 조합, 라이브러리 고르기, 환경 설정 같이 "본질과 무관한 일"을 없앤다
- **도메인 문법을 그대로 코드로** — 라우트, DB, 결제, 디자인을 자연어 수준의 도메인 키워드(`@route`, `@db`, `@design`)로 표현
- **에디터가 프로젝트 상태의 라이브 뷰** — 값이 흐르고, 번들에 포함되고, 어디서 호출되는지 코드 옆에서 실시간 확인
- **프로덕션에서 코드로 되짚기** — 장기 목표: 실행 중인 화면·요청·쿼리·잡·로그의 "이 부분"이 어떤 코드와 도메인 노드에서 왔는지 즉시 reveal하고 탐색
- **한 번 쓴 코드가 곧 배포** — 장기 목표: 빌드 산출물이 하나의 바이너리, 하나의 WASM, 하나의 CDN으로 묶임

### 기존 생태계와의 차이

기존 개발 생태계에서 DB, 서버, 네트워크, 웹, 브라우저 엔진 등 각 도메인은 격리된 환경에서 독립적으로 발전해왔다. SQL은 "데이터 저장"이라는 범용 목적으로 설계되었고, HTTP도 랜딩 웹앱이든 게임 웹앱이든 동일한 통신을 사용한다. 호환성을 위해 성능을 포기한 것이다.

orv는 "프로젝트 특화" 프레임워크라는 아이디어에서 출발한다. A라는 기능을 개발하면, 최종 아웃풋은 그 기능을 위해 최적화된 번들이 된다. 마치 수술실이 해당 수술을 위해 세팅되듯, 도메인 간 관계에 따른 최적화를 통해 보다 효율적으로 기능하도록 한다.

이를 위해 범용 호환성보다 프로젝트 특화 최적화를 우선한다. 일부 선택은 기존 생태계와의 직접 호환성을 낮출 수 있지만, 그 대가로 더 높은 생산성과 일관된 개발 경험을 목표로 한다.

## 통합 플랫폼의 네 레이어

orv 플랫폼은 네 개의 레이어가 **같은 프로젝트 그래프**를 공유한다. 언어는 그래프를 만들고, 컴파일러는 최적화하며, 에디터는 시각화하고, 런타임은 실행한다. 외부 VSCode/Neovim 같은 에디터도 지원하지만, 자체 에디터에서 이 그래프를 가장 풍부하게 경험할 수 있다.

```
┌─────────────────────────────────────────┐
│  Editor     — 라이브 뷰, 도메인 시각화   │
├─────────────────────────────────────────┤
│  Language   — 의도를 문법으로            │
├─────────────────────────────────────────┤
│  Compiler   — 프로젝트 그래프 소유       │
├─────────────────────────────────────────┤
│  Runtime    — Zero-overhead 실행         │
└─────────────────────────────────────────┘
```

생산성의 진짜 지렛대는 자동완성 속도가 아니라 **"지금 수정하는 코드가 전체 시스템에 어떤 영향을 미치는가"와 "프로덕션에서 보고 있는 이 상태가 어떤 코드에서 왔는가"를 같은 그래프에서 보여주는 것**이다. 보통 언어와 프레임워크는 `코드 작성 → 화면/서버 실행` 방향을 기본으로 삼지만, orv는 반대 방향도 일급 기능으로 만드는 것을 목표로 한다. 현재 MVP는 HIR origin map 생성과 HTTP route 응답의 `x-orv-origin-id` 헤더를 시작점으로 제공하며, 프로덕션의 DOM 요소, API 응답, DB 쿼리, background job, 로그 이벤트에서 곧바로 해당 `.orv` 코드와 도메인 노드를 reveal하고 탐색하는 기능은 production compiler/editor가 갖춰진 뒤의 로드맵이다. Dark의 구조화 에디터와 Smalltalk의 이미지 기반 라이브 환경, Light Table의 인라인 값 흐름, Zed/Helix의 Tree-sitter 구문 선택이 따로 있었기에 부분적으로만 가능했던 것을, orv는 **언어 수준에서 전체 프로젝트 그래프가 확정되어 있기 때문에** 기본값으로 제공하는 것을 지향한다.

상세한 에디터 사양은 [SPEC.md §16](SPEC.md#16-통합-에디터) 참조.

## 문서 구조와 읽는 순서

orv 문서는 역할이 다른 층위로 나뉜다. 같은 내용을 반복해 적기보다, 각 문서가 책임지는 질문을 분리한다.

1. **비전 문서 — `docs/README.md`**
   - 왜 이 언어를 만드는지
   - 어떤 사용자와 생산성 목표를 상정하는지
   - 어떤 방향성을 우선하는지

2. **기준 사양 — `docs/SPEC.md`**
   - 현재 기준의 공식 문법과 목표 의미론
   - 문법/동작의 authoritative source
   - 구현 상태가 따로 표시된 항목은 현재 MVP와 로드맵을 구분
   - 문서 간 충돌 시 우선 기준

3. **실험 사양 / 탐색 예제 — `fixtures/default-syntax.orv`, `fixtures/plan/*.orv`**
   - 문법을 압박 테스트하는 예제
   - 아직 방향을 탐색 중인 표면 문법이나 사용감 검토
   - 확정 전 아이디어를 구체 코드로 검증하는 공간

보조 문서로는 다음이 있다.

- **문서 구조 가이드 — `docs/DOCUMENTATION.md`**: 비전 문서, 기준 사양, 실험 사양, 구현 문서의 역할과 수정 규칙을 정리한다.
- **구현 아키텍처 — `docs/ARCHITECTURE.md`**: 현재 컴파일러/크레이트 구조를 설명한다. 구현 구조의 기준 문서이지, 언어 의미론의 원천은 아니다.
- **실행 검증 예제 — `fixtures/e2e/*.orv`**: 라우팅/미들웨어 등 핵심 동작을 검증하는 실행 예제다.

처음 읽을 때는 `README -> SPEC -> DOCUMENTATION -> ARCHITECTURE -> fixtures` 순서를 권장한다.

## 핵심 목표

- **Zero-overhead, Zero-runtime**: 사용하지 않는 기능의 코드와 대응 런타임 계층은 번들에 포함하지 않는다
- **프로젝트 특화 최적화**: 컴파일 타임에 프로젝트 전체를 분석하여 도메인 간 최적화 수행
- **통합 에디터**: 에디터가 프로젝트 그래프의 라이브 뷰 — 값 흐름, 도메인 경계, 번들 포함 여부, 호출 그래프를 실시간 표시
- **프로덕션-코드 양방향 추적**: 부분 구현 — HIR origin map, `contains`/`calls` origin edge, HTTP route origin id 헤더, build artifact origin reveal CLI 제공. 화면, DB 쿼리, job, 로그에서 에디터로 직접 reveal하는 기능은 로드맵
- **창의성 우선 (creativity-first)**: 레이아웃·디자인·로직을 의도 수준에서 쓰고, 우발적 복잡성은 컴파일러가 흡수
- **생산성 벤치마크**: 비개발자 + 5시간 → 쇼핑몰 풀서비스. AI 보조 없이 도달 가능해야 한다

## 성능 목표

orv의 "Zero-runtime" 원칙은 **필요한 경우에만 해당 런타임 계층이 포함된다**는 뜻이지, 모든 앱에서 런타임이 완전히 0이 되거나 모든 앱을 3kb로 만든다는 뜻은 아니다. 앱 복잡도에 따라 목표가 분화된다 (상세: SPEC.md §13.11).

| 앱 유형 | 초기 번들 목표 | 예시 |
|---------|--------------|------|
| 정적 랜딩/블로그 | 0 byte (순수 HTML) | 마케팅 사이트, 문서 |
| 가벼운 대화형 SPA | ≤ 3 KB | 간단한 카운터, 폼 |
| 표준 SPA (SSR + hydration) | ≤ 30 KB 초기 + 라우트별 lazy | 대시보드, SNS 피드 |
| 그래픽스/미디어 SPA | ≤ 200 KB 쉘 + 스트리밍 | Figma/Photoshop 급 |
| 게임 / 네이티브급 | ≤ 1 MB 쉘 + 에셋 스트리밍 | Krunker.io 급 |

**백엔드 / 바이너리 목표:**

| 항목 | 목표 | 비교 |
|------|------|------|
| REST/RPC RPS (hello world) | ≥ drogon 수준 | C++ 최상위급 |
| 풀스택 SSR (cold) | < 100µs/page | Next.js 대비 10배 이상 |
| 바이너리 시작 | ≥ Rust 수준 | 프로젝트 도메인이 깊을수록 Rust보다 유리 |
| 증분 빌드 (1M LOC) | 단일 파일 변경 < 1s | Rust cargo check 대비 수십배 |

## 프로젝트 구조

```
miol/
├── crates/
│   ├── orv-analyzer      # 의미 분석, HIR 로우어링
│   ├── orv-cli           # CLI 프론트엔드
│   ├── orv-compiler      # HIR origin map artifact 생성
│   ├── orv-core          # 핵심 타입 및 공유 인프라
│   ├── orv-diagnostics   # 구조화된 진단 메시지 + 소스 위치 정보
│   ├── orv-hir           # 고수준 중간 표현 (HIR)
│   ├── orv-macros        # proc-macro 유틸리티
│   ├── orv-project       # 멀티파일 로드 + AST ProjectGraph v1 추출
│   ├── orv-resolve       # 이름 해석, 스코프 분석
│   ├── orv-runtime       # 레퍼런스 런타임, 어댑터 빌드
│   └── orv-syntax        # 렉서, 파서, AST
├── docs/                 # 프로젝트 문서
│   ├── README.md         # 이 파일
│   ├── SPEC.md           # 언어 사양
│   ├── DOCUMENTATION.md  # 문서 구조와 수정 원칙
│   └── ARCHITECTURE.md   # 크레이트 구조 및 데이터 흐름
└── fixtures/
    ├── default-syntax.orv # 빠른 문법/표현 예제
    ├── e2e/              # 라우팅/미들웨어 e2e 예제
    └── plan/             # 실험 사양 / 탐색 fixture (01~09)
```

## 기술 스택

- **언어**: Rust (edition 2021, MSRV 1.86.0)
- **그래픽스**: wgpu 29
- **진단**: codespan-reporting 0.11
- **직렬화**: serde + serde_json
- **CLI**: clap 4

## 현재 상태

> ⚠️ 초기 개발 단계 — 언어 사양 설계 진행 중

공식 언어 사양은 [SPEC.md](SPEC.md)에 정리되어 있고, `fixtures/plan/`과 `default-syntax.orv`는 그 사양을 압박 테스트하거나 미래 방향을 탐색하는 실험 예제 역할을 한다. 현재 실행 표면은 `orv init/run/dev/check/dump/origins/graph/test/editor snapshot/editor reveal/editor runtime/editor export/editor trace/lsp snapshot/lsp reveal/lsp serve --stdio/dap serve --stdio/build/build --prod/add/remove/lock/fetch/workspace new/workspace graph/workspace lock/workspace fetch/workspace build/verify-build/verify-artifact/check-artifact/check-build/run-artifact/run-build/reveal/db plan/db verify/db apply/db migrate/db rollback/db backup/db restore/db recover/db archive/db squash`과 레퍼런스 인터프리터 중심이며, `orv init <dir> [--template basic|shop]`은 최소 프로젝트 또는 쇼핑몰 `GET /` HTML form 홈과 회원/결제/배송 route scaffold를 생성하고 shop template에는 check/build/verify/run-build와 generated deploy runbook/Compose 실행 명령 README를 함께 쓴다. `@server` request body는 JSON과 `application/x-www-form-urlencoded`를 `@body` object로 노출한다. source-entry 명령은 `orv.toml`의 `[project].entry` 또는 프로젝트 디렉터리의 `orv.toml`을 입력으로 받을 수 있고, `orv add/remove`는 dependency section 편집 후 lockfile을 재생성하며, `orv workspace new`는 root workspace member 등록과 member scaffold를 만들고, `orv workspace graph --view`는 member ProjectGraph/files/dependencies 및 member 간 path dependency edge를 JSON/HTML artifact로 출력하며, `orv workspace build`는 dependency-first member별 build/verify 산출물과 `workspace-build.json` top-level manifest를 만들며 `--incremental`로 unchanged member를 skip하고, `orv fetch`는 최신 lockfile에서 path/local/HTTP/HTTPS registry dependency source-bundle cache를 만들며, `orv lock [dir-or-orv.toml] [--check]`는 project/dependency metadata를 deterministic JSON `orv.lock`으로 고정하거나 최신성을 검증한다. `orv graph`는 멀티파일 source map, AST ProjectGraph v1, source/semantic depth stats, HIR origin map, HIR `contains`/`calls` origin edge, origin-to-AST-node link를 JSON으로 제공하고, `--view --out <dir>`은 같은 데이터를 `graph.json`과 정적 `index.html` graph view로 쓴다. `orv test <path> --filter <name>`은 `.orv` 파일을 재귀적으로 찾고 `test "name"` 블록이 있는 파일을 레퍼런스 런타임으로 실행하며, `--list`는 실행 없이 발견 목록 JSON을 출력하는 초기 test runner다. `orv editor snapshot <file>`은 같은 프로젝트 그래프와 diagnostics를 first-party editor bootstrap JSON으로 만들고 Files/Routes/Schema/Domains 패널 입력을 함께 출력한다. `orv editor reveal <dir> <origin-id>`는 build artifact origin을 first-party editor focus/source/production navigation payload로 변환한다. `orv editor runtime <file>`은 DAP trace/runtime helpers를 재사용해 first-party editor runtime inspection pane JSON을 출력한다. `orv editor export <file> --out <dir>`은 snapshot/runtime/debug state JSON과 static HTML editor shell artifact를 출력하며 ProjectGraph view, panel list, DAP launch/live/attach config, executable breakpoint line, runtime/trace detail을 렌더링하고, `--build <dir> --trace <trace.json>`이 있으면 trace navigation state도 embed한다. `orv editor trace <dir> --trace <trace.json>`은 captured request trace frame을 editor source/production navigation payload로 확장한다. `orv lsp snapshot <file>`은 같은 프로젝트 그래프와 diagnostics, LSP-style document symbols/ranges를 editor bootstrap JSON으로 출력하고, `orv lsp reveal <dir> <origin-id>`는 build artifact origin을 LSP location/range와 production descriptor로 변환한다. `orv lsp serve --stdio`는 Content-Length stdio JSON-RPC에서 initialize/shutdown/notification/unknown-method와 `textDocument/documentSymbol`, `textDocument/codeLens`, `textDocument/codeAction`, `textDocument/documentLink`, `textDocument/documentColor`, `textDocument/colorPresentation`, `textDocument/foldingRange`, `textDocument/selectionRange`, `textDocument/semanticTokens/full`, `textDocument/diagnostic`, `workspace/diagnostic`, `workspace/executeCommand`, `textDocument/definition`, `textDocument/declaration`, `textDocument/typeDefinition`, `textDocument/implementation`, `textDocument/moniker`, `textDocument/prepareCallHierarchy`, `callHierarchy/incomingCalls`, `callHierarchy/outgoingCalls`, `textDocument/prepareTypeHierarchy`, `typeHierarchy/supertypes`, `typeHierarchy/subtypes`, `textDocument/linkedEditingRange`, `textDocument/references`, `textDocument/documentHighlight`, `textDocument/prepareRename`, `textDocument/rename`, `textDocument/hover`, `textDocument/signatureHelp`, `textDocument/inlayHint`, `textDocument/completion`, `workspace/symbol` file URI/root URI 요청을 처리하며, `textDocument/didOpen`/`textDocument/didChange` full-sync unsaved buffer를 세션 안에 보관해 이후 요청에 사용한다. `orv dap serve --stdio`는 Content-Length Debug Adapter Protocol에서 initialize/launch/attach/configurationDone/setBreakpoints/setFunctionBreakpoints/setInstructionBreakpoints/dataBreakpointInfo/setDataBreakpoints/breakpointLocations/gotoTargets/stepInTargets/exceptionInfo/threads/stackTrace/scopes/variables/setVariable/evaluate/setExpression/completions/loadedSources/modules/source/disassemble/readMemory/continue/reverseContinue/goto/stepIn/stepBack/restartFrame/disconnect/terminate/terminateThreads를 처리하고, launch/attach 시 같은 프로젝트 로더/그래프/진단을 사용해 entry source stack frame, line/function/data breakpoint/goto/step-in target list, source-frame pseudo disassemble, source-frame readMemory bytes, exception status, loaded source list/content, project module list, project-scope variables/evaluate/completion 값, reference runtime stdout/status/error variables, runtime frame Locals snapshot 편집을 만든다. 런타임 `@db`는 in-memory 실행 모델에 명시적 `save/load` JSON snapshot, `wal` JSONL append+fsync replay, `checkpoint` WAL snapshot compaction, `savepoint/rollback` 복원을 제공한다. `orv db plan <file> --applied <schema.json>`은 현재 `struct` schema snapshot과 적용된 snapshot을 비교해 migration dry-run JSON을 출력하고, `orv db verify <file> --schema <schema.json>`은 schema drift를 실패로 보고한다. `orv db apply/migrate <file> --schema <schema.json> --history <history.json>`은 현재 snapshot을 적용된 schema 파일로 저장하고 migration history를 append하며, `orv db squash --history <history.json> --out <squashed.json>`은 migration history actions를 하나로 압축한다. `orv db migrate <file> --schema <schema.json> --data <data.json>`은 JSON data snapshot의 struct table과 row field를 schema diff에 맞춰 add/drop 한다. `orv db rollback --schema <schema.json> --data <data.json>`은 직전 schema/data snapshot 백업을 복원한다. `orv db backup --data <data.json> --out <backup.json>`과 `orv db restore --backup <backup.json> --data <data.json>`은 local JSON data snapshot backup/restore를 제공하고, `orv db recover --wal <db.wal.jsonl>` 또는 `--archive <archive.json>`는 WAL/hash-verified archive manifest를 record count 또는 timestamp 경계까지 재생해 `@db.save` 호환 snapshot으로 복구한다. `orv build <file-or-orv.toml> --out <dir>`은 `build-manifest.json`, `bundle-plan.json`, `origin-map.json`, `project-graph.json`, `source-bundle.json`, `server/app.orv-runtime.json`, `server/launch.json`으로 된 초기 artifact directory를 만들고, 순수 HTML-only entry는 zero-runtime `pages/index.html`, `let sig` 또는 client-side HTML await가 필요한 entry는 non-zero-runtime `pages/index.html` shell, `client/app.js` loader, `orv.client` metadata와 `orv_start` export를 담은 `client/app.wasm` callable placeholder를 출력한다. `orv build --prod`는 static `@listen 0`을 test-only ephemeral port로 거부하고 추가로 `deploy/manifest.json`, `deploy/routes.json`, static/env listen ports를 기록하는 `deploy/container.json`, `deploy/Dockerfile`, `deploy/compose.yaml`, `deploy/README.md`, reference server entrypoint를 출력하고, client bundle이 있으면 deploy manifest에 source bundle and page/loader/wasm `client` target도 기록한다. manifest에는 필요한 `runtime_features`만 기록하고, static page bundle target은 빈 `runtime_features`, interactive client bundle targets는 `client_wasm` runtime feature를 가진다. `source-bundle.json`은 모든 build의 source path/source/content hash를 담아 원본 파일 없이도 `orv reveal`과 `orv lsp reveal`이 source span을 복구하게 한다. `orv verify-build <dir>`은 build manifest/plan target, source bundle hash, deploy manifest/entrypoint/routes inventory/container/Dockerfile/Compose/runbook, reference server launcher, static page zero-runtime shape, client page/JS/WASM metadata와 `orv_start` bootstrap, 그리고 존재하는 `dev/session.json` HMR 계약, `dev/transport.json`/`dev/hmr-client.js` HMR transport 계약, `dev/watch.json` watch 계약, `dev/events.json` watch-loop event 계약, `dev/server.json` HMR endpoint 계약을 검증한다. reference server artifact는 source bundle과 hash, listen origin/static/env port descriptor를 포함해 runner hydration 계약을 고정하며, `server/launch.json`은 `orv run-artifact server/app.orv-runtime.json` 실행 계약과 HTTP/1 listen descriptor 및 route 목록을 기록한다. `orv verify-artifact <file>`은 이 계약을 검증하고 `orv check-artifact <file>`은 import 포함 server artifact source bundle을 재분석하고 `orv check-build <dir>`은 build-level source bundle을 재분석하며 `orv run-artifact <file>`은 source bundle을 재수화해 reference runtime으로 실행한다. `orv run-build <dir>`은 bundle plan target을 기준으로 해당 reference artifact를 실행하거나, server 없는 static/client page build에서는 verified HTML을 stdout으로 출력한다. `orv dev <file-or-orv.toml> --out <dir>`은 build, verify-build, run-build를 묶는 현재 dev bootstrap이고, `--hmr`은 `dev/session.json`에 watch source hash, bundle target, hot-reload/full-reload fallback 전략을, `dev/transport.json`과 `dev/hmr-client.js`에 reference EventSource browser transport 계약을 기록한다. `--watch`는 `dev/watch.json`에 poll loop/watch target/manifest transport 계약을 기록하며, `--watch-loop`는 poll loop를 실행하고 `dev/events.json` event manifest를 남기며, `--hmr --serve`는 `dev/server.json`과 HTTP/1 `/__orv/hmr/session`, `/__orv/hmr/events` endpoint를 제공한다. `orv reveal <dir> <origin-id>`는 build artifact의 origin id를 source span, ProjectGraph node, server route descriptor 또는 client bundle target으로 역추적한다. native 서버 바이너리, 실제 WASM/JS 코드젠은 로드맵이다. 현재 구조는 [ARCHITECTURE.md](ARCHITECTURE.md)를 참조.

DB PITR 세부: `orv db restore --archive <archive.json> --data <data.json> --at <RFC3339>`는 hash-verified WAL archive manifest를 지정 시점까지 재생해 `@db.save` 호환 snapshot으로 복원한다.

Package lock 세부: `orv lock`은 prerelease/build metadata 포함 exact version을 그대로 고정하고, local/file/HTTP/HTTPS registry `index.json`이 있으면 `*`, `x`, segment wildcard, caret, tilde, whitespace-AND comparator, `||` disjunction range를 exact version으로 해석하며 원래 range는 `requested_version`으로 보존한다. `auth_token_env`는 secret 대신 env var 이름만 lockfile에 보존하고 HTTP/HTTPS registry index/fetch 요청에 Bearer token을 붙인다.

Workspace package 세부: `orv workspace graph --view`는 member 간 path dependency edge에 target member name/version과 requested version을 기록하고, requested version이 target member version과 맞지 않으면 실패하며, JSON과 static HTML workspace graph view를 출력한다. `orv workspace lock [root] --out <dir>`은 workspace graph/path dependency edge를 재사용해 member를 dependency-first 순서로 lock하고, `members/<member>/orv.lock`과 top-level `workspace-lock.json` artifact를 출력한다. `orv workspace fetch [root] --out <dir>`은 그 lock artifact를 member별 dependency cache로 materialize하고 `workspace-fetch.json`을 출력한다.

LSP bootstrap은 `declaration`/`typeDefinition` navigation, project moniker, document colors, linked editing, type hierarchy, function call `signatureHelp`, parameter `inlayHint`까지 제공한다. DAP bootstrap은 현재 ProjectGraph/AST 기반 verified nested line breakpoint/goto target line, conditional/hit-condition breakpoint, non-stopping `logMessage` logpoint, function breakpoint, instruction breakpoint unverified response, source-frame pseudo `disassemble`, source-frame `readMemory`, data breakpoint, ORV diagnostics/runtime exception filters, source/module inventory와 snapshot-backed `source` content, debug-control thread id guard, reference-runtime debug frame capture, `launch.arguments.live=true` DebugStepper 기반 incremental runtime frame 실행, standard `attach` request를 통한 runtime attach, 장기 실행 `@server` launch의 non-blocking paused frame과 `continue`/`pause` event loop, `supportsOrvRuntimeAttach`/`supportsOrvRuntimeTracePath`/`supportsDisassembleRequest`/`supportsReadMemoryRequest` initialize capability, `launch.arguments.attachRuntime=true` child-process accept-loop attach, `attachRuntimeMode="inProcess"` in-process accept-loop attach, process STOP/CONT 또는 in-process shutdown/restart pause/resume, launch runtime JSON/variables/evaluate/completions async state/static/env listen endpoint/route inventory/transport process/address inspect 노출, in-process request count/last request/request frame/request trace JSON inspect와 `runtimeRequestTracePath` file flush 노출, runtime-owned `orv.production.trace` schema/file writer 재사용, `ORV_RUNTIME_REQUEST_TRACE_PATH` graceful server trace file capture, `/__orv/trace/events` live trace EventSource snapshot, runtime-evaluated Locals scope, captured function call-stack `stackTrace`, current-frame local-name `evaluate`/`completions`, Locals snapshot `setVariable`/`setExpression`, runtime-frame `next` step-over, captured `stepIn.targetId`, live targetId 거부 `stepIn`, `stepOut`, `stepBack`, `goto` progression with last-frame termination, next line/function/data breakpoint `continue`/`reverseContinue`, per-frame stdout `output` events, `pause` stopped event, live-mode 보존 `restart`, stderr `output` events, stdio `initialized`/`stopped`/`continued`/`terminated` events까지 제공한다. `orv editor snapshot`은 first-party editor Files/Routes/Schema/Domains 패널 입력과 source-hash live refresh watch set을 만들고, `orv editor reveal`은 build origin id를 editor focus/source/production navigation payload로 바꾸며, `orv editor runtime`은 runtime status/stdout/frames를 editor pane JSON으로 만든다. `orv editor export`는 이 state를 static HTML shell로 렌더링하고 ProjectGraph view, panel list, DAP launch config/breakpoint line, selectable runtime/trace detail, optional trace navigation state도 embed한다. `orv editor trace`는 captured request trace navigation과 stable server EventSource trace transport metadata를 제공하고, native editor live trace consumption과 continuous multi-event streaming은 다음 단계다.

## 빌드

```bash
cargo build
cargo test
cargo clippy
```

## 라이선스

MIT
