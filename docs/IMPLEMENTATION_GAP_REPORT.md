# orv Implementation Gap Report

작성일: 2026-05-18  
기준: `docs/README.md`, `docs/SPEC.md`, `docs/MVP.md`, `docs/IMPLEMENTATION_MATRIX.md`, `docs/OPERATIONAL_SURFACES.md`, `docs/ROADMAP.md`, `docs/SECURITY_MODEL.md`, `docs/BENCHMARK_SHOP_5H.md`, 현재 Rust workspace

이 문서는 상태표의 파생 분석이다. 기능별 authoritative 판정은 `docs/IMPLEMENTATION_MATRIX.md`에 남기고, 이 문서는 현재 진행률, 리스크, 다음 작업 순서를 요약한다.

## 요약

orv는 아이디어 단계가 아니라, compiler/runtime/CLI/build/reveal/shop scaffold가 실제로 움직이는 구현 중 프로젝트다. 현재 구현의 중심축은 이미 잡혀 있다.

```text
ProjectGraph + HIR Origin + Reference Runtime + Trace/Reveal
```

다만 전체 문서가 약속하는 범위는 이 중심축을 훨씬 넘는다. 전체 SPEC/ROADMAP 기준으로는 아직 production language platform 완성 단계가 아니다. 특히 native optimizer, full first-party editor UI, real provider adapters, custom DB engine, CRDT/GPU/media/network domains는 대부분 artifact, reference stub, 또는 non-binding roadmap 상태다.

## 진행률 추정

이 수치는 LOC 기준이 아니다. 문서가 약속한 제품 표면을 `implemented`, `reference`, `artifact only`, `planned`, `non-binding` 계약 레벨로 나눠 산정한 engineering estimate다.

| 기준 | 진행률 | 해석 |
|------|--------|------|
| M0 compiler/runtime foundation | 85-90% | parser/project/analyzer/HIR/runtime/origin/CLI 기본 경로가 구현됨. 안정 계약과 golden invariant가 남음 |
| M1 web app foundation | 75-80% | HTTP server, route, body/form/query binding, validation, HTML, SQLite reference path가 있음. dynamic client/runtime hardening 남음 |
| M2 shop foundation | 70-75% | shop scaffold, session/auth/csrf/rate-limit reference path, checkout/admin/smoke/preflight가 있음. human benchmark와 production provider path 남음 |
| M3 reveal/editor foundation | 65-70% | CLI/static reveal, origin map, trace, LSP/DAP/export artifact가 큼. native editor UX는 아직 없음 |
| M4+ production/advanced platform | 15-25% | native/source contracts와 stubs는 있으나 real optimizer, provider drivers, custom DB, CRDT/GPU/media/editor는 남음 |
| 5시간 쇼핑몰 MVP 경로 | 70-80% | automated template-to-running-shop path는 강함. 실제 비개발자 5시간 검증과 UX polish가 남음 |
| 전체 문서 대비 | 50-60% | MVP/contract layer는 많이 진행. 전체 SPEC의 advanced domains와 production platform까지 포함하면 절반 조금 넘는 수준 |

가장 정확한 현재 상태 표현은 다음이다.

> Reference MVP와 artifact contract는 많이 구현됐다. Production-grade platform과 first-party editor 제품은 아직 남아 있다.

## 근거

코드베이스 기준으로 확인한 구현 신호:

- Rust workspace crate 12개: `orv-syntax`, `orv-project`, `orv-resolve`, `orv-analyzer`, `orv-hir`, `orv-runtime`, `orv-compiler`, `orv-cli` 등.
- 테스트 attribute 수 1,239개.
- 큰 구현 파일: `orv-cli` build/deploy/editor/DAP/DB, `orv-runtime` interpreter/server/DB, `orv-compiler` native/server artifact 쪽이 이미 큰 비중을 차지.
- CLI command surface가 넓음: `run/check/test/build/dev/graph/origins/reveal`, `editor`, `lsp`, `dap`, `db`, `workspace`, `benchmark-report`, `verify-build`, `deploy-env-check`.
- `fixtures/e2e/shopping_mall.orv`가 shop north-star의 reference vertical slice로 존재.
- `docs/IMPLEMENTATION_MATRIX.md`가 현재 상태와 계약 레벨을 이미 기능별로 추적.

검증 제약:

- `rtk cargo test`는 약 8분 29초 동안 실행했으나 완료 출력 없이 계속되어 중단했다. 따라서 이 보고서는 전체 test pass를 주장하지 않는다.
- 대신 문서, CLI args, code search, fixture/test inventory를 근거로 분석했다.

## 영역별 상태

### 1. Compiler / Runtime Foundation

상태: 높음, 단 계약은 아직 mostly `stable-ish` 또는 `experimental`.

구현됨:

- source load, lex, parse, AST
- import DFS와 merged program/source map
- AST ProjectGraph v1
- name resolution
- semantic analysis와 HIR lowering
- span-backed diagnostics
- reference tree-walking runtime
- HIR origin map
- origin `contains`/`calls` edge
- `orv check`, `orv run`, `orv test`, `orv dump`, `orv origins`, `orv graph`

남은 기능:

- ProjectGraph/origin-map/trace JSON schema versioning과 migration policy
- `Span -> AST -> HIR -> runtime event -> origin id` 불변식에 대한 golden regression suite
- import visibility, cyclic import, package boundary, source bundle rehydration edge case hardening
- error signature inference의 full graph warning contract
- core language feature별 stable/experimental 분리

판단:

M0는 구현량 기준으로 거의 MVP 완성권이다. 다만 외부 사용자에게 stable contract라고 말하기에는 schema freeze와 invariant tests가 더 필요하다.

### 2. Language Core

상태: parser/runtime/analyzer 기능은 넓지만, Systems Surface는 아직 문서 목표가 더 큼.

구현됨 또는 부분 구현:

- primitive/collection/object/tuple/enum/union-ish type lowering
- pattern types, constraints, `where` 일부
- `.parse`, `.safeParse`, `.errors`, `.is`, `.validate`
- nullable/cast/runtime validation path
- function/control-flow/basic async surface
- custom domain parser/lowering/reference runtime path

남은 기능:

- RC ownership을 실제 제품 계약으로 만들기: `.move()`, `.copy()`, reference invalidation, borrow-like diagnostics
- `WeakRef<T>`, cycle detection, optional cycle collector
- `spawn` boundary Atomic RC promotion
- `Arena<T>` allocation semantics
- full generic type checking and monomorphization/lowering policy
- stable error model: route/job/spawn/main boundary별 propagation, rollback, audit, `@after` policy
- App Authoring Surface와 Systems Surface의 editor/documentation 분리

판단:

App Authoring에 필요한 schema validation은 강해지고 있다. Systems Surface의 memory/concurrency semantics는 아직 SPEC 목표에 가깝다.

### 3. Web / Server Foundation

상태: MVP web server는 강함. Advanced transports는 roadmap/reference.

구현됨:

- HTTP/1.1 `@server`, `@listen`, `@route`
- path param, query/header/body/rawBody request state
- JSON/form-urlencoded body parse
- named schema request binding: `@body: T`, `@query: T`, `@form: T`
- validation failure 400 `orv.validation.error` response
- `@respond`, `@serve`, static files
- route origin and response origin headers
- basic middleware/security domains in reference runtime

남은 기능:

- WebSocket `@ws` production runtime
- WebTransport `@wt`
- WebRTC signaling/runtime
- route group/policy composition hardening
- server boundary error/audit/transaction policy across all route shapes
- production TLS/H2/H3/QUIC story
- complete observability propagation over route/db/job/ws/sync

판단:

M1 server path is usable as reference runtime. Full server platform promised by SPEC is much wider and remains unfinished.

### 4. HTML / Client / Design

상태: static and artifact-heavy. Full interactive client runtime still not product-complete.

구현됨:

- `@html` static render path
- generated page artifacts
- client manifest/reactive-plan/JS/WASM bootstrap contracts
- signal/text/attr/event binding inventory
- verify-build checks over client manifest/reactive plan
- shop template uses editable `@design` tokens in starter path

남은 기능:

- full dynamic DOM diff runtime
- optimized client WASM/JS codegen beyond bootstrap
- partial hydration/island strategy
- production asset pipeline
- first-class design editing mode
- richer `@design` token tooling and visual affordances
- browser API domains as real product surfaces: `@media`, `@offline`, `@push`, `@textbuffer`

판단:

Client side is good as a checked contract and smoke target. It is not yet a complete framework/runtime replacement.

### 5. DB / Persistence

상태: reference persistence is strong. Production DB story remains incomplete.

구현됨:

- in-memory DB
- JSON snapshot
- WAL/checkpoint/savepoint/rollback/recover/archive/crash-matrix paths
- SQLite row JSON adapter
- file adapter path
- DB build/deploy persistence artifacts
- external PostgreSQL/MySQL status handles
- optional HTTP bridge contract for external DB adapters
- DB adapter env/preflight/deploy smoke contracts

남은 기능:

- direct PostgreSQL driver
- direct MySQL driver
- production transaction semantics
- schema/migration DSL as stable user contract
- query planner/index model
- checkout stock/order/payment/shipping transaction boundary with real rollback/compensation
- custom orv-db storage engine
- sharding/replication/PITR operator UX beyond reference paths

판단:

DB is robust for reference/shop MVP. Production database credibility now depends on direct adapters and transaction semantics, not more artifact metadata.

### 6. Shop / Commerce / Security

상태: automated shop path is advanced. Production payment/shipping/security hardening still remains.

구현됨:

- `orv init <dir> --template shop`
- catalog, cart, member, checkout, admin read models
- editable product field path
- local/file payment and shipping records
- HTTP commerce adapter contract
- provider-mode Stripe/carrier reference handles
- Stripe-style webhook verification reference path
- session cookies, password hashing, login verification
- `@session required`, `@Auth required role="admin"`
- CSRF reference path
- route rate limits for hotspots
- audit rows for core shop operations
- deploy preflight, benchmark evidence, smoke output, benchmark report

남은 기능:

- actual human 5-hour benchmark runs and recorded evidence
- production-grade Stripe SDK adapter
- production carrier/shipping SDK adapter
- webhook timestamp tolerance/replay window/rotation policy as stable contract
- payment idempotency and compensation across retries/provider failures
- password reset/email verification/account recovery
- OAuth/provider auth if retained in product direction
- secrets/vault production handling and redaction tests
- XSS/raw HTML unsafe escape audit
- authorization policy model beyond simple role checks

판단:

M2 is close to credible automated demo. It is not yet proven as a non-developer product until real user runs and provider/error-path hardening exist.

### 7. Build / Deploy / Native

상태: artifact contract is very strong. Native production output is still mixed.

구현됨:

- build manifest, bundle plan, source bundle, origin map, project graph
- server runtime artifact
- deploy manifest, container/Compose/runbook/env/preflight/smoke/evidence artifacts
- `orv verify-build`
- `orv deploy-env-check`
- `orv run-build`
- generated native launcher package/source contracts
- direct native lowering for simple route response slices
- fallback to reference runtime for dynamic unsupported paths
- reveal/editor/LSP payloads for build/deploy/native artifacts

남은 기능:

- full native server binary generation for dynamic routes
- native DB-backed route lowering
- native runtime image as real production image, not mostly plan/contract
- route-level bundle splitting and render strategy inference
- DCE/runtime feature pruning measured by bundle size
- cloud deploy provider matrix
- artifact signing/provenance/release profile hardening
- benchmarked performance claims with hardware/payload/concurrency/TLS details

판단:

Build/deploy is one of the most complete areas as a contract system. It is not yet a production native compiler story.

### 8. Reveal / Editor / LSP / DAP

상태: CLI/static/editor artifact layer is deep. Native editor product is not there yet.

구현됨:

- `orv reveal`
- `orv editor reveal`
- `orv lsp reveal`
- runtime trace JSON and EventSource trace stream normalization
- static editor export with panels
- LSP bootstrap with many navigation/introspection methods
- DAP bootstrap with runtime frames, locals, controls, breakpoints, source snapshots
- debug runner artifacts and production context summaries

남은 기능:

- first-party native editor shell consuming `native-host.json`
- interactive source/production reveal UI
- route/schema/domain panels as polished product surfaces
- inline value flow from live runtime traces
- design edit mode for `@html`/`@design`
- CRDT collaboration UI
- production permission model for source reveal
- performance targets measured on real projects

판단:

M3 is far beyond a placeholder, but most value is still delivered as CLI/static artifacts. The user-facing editor product remains a major remaining product.

### 9. Advanced Domains

상태: mostly reference stubs/design pressure.

Some reference behavior exists for:

- `@storage`
- `@job`
- `@cron`
- `@sync.open`/`connect`
- `@mail.verify`
- `@media.camera`
- `@push`
- `@cache`
- `@offline.store`
- `@plugin`
- `@gpu`
- `@observability`
- `@net` gated by `@unsafe`
- `@ffi` gated by `@unsafe`

남은 기능:

- real GPU/WebGPU pipeline and shader asset model
- media encode/transcode/streaming pipeline
- ServiceWorker/IndexedDB/offline sync generation
- Web Push provider integration
- textbuffer rope/piece-table engine
- upload/storage provider drivers and range streaming
- raw TCP/UDP/TUN production runtime
- mail server/client production implementation
- durable job queue, worker restart, distributed cron leader election
- WASM plugin sandbox and capability model
- CRDT/OT state engine and sync transport
- observability export to real tracing/metrics backends
- native FFI ABI validation/loading

판단:

These domains prove syntax and reference intent. They should not be counted as product-ready unless promoted through matrix contract changes.

## 가장 큰 남은 리스크

1. **계약 안정도 리스크**  
   많은 기능이 implemented지만 contract는 `experimental`, `reference`, `unstable`이다. 문서가 외부 사용자에게 "된다"로 읽히면 신뢰 리스크가 생긴다.

2. **5시간 쇼핑몰 검증 리스크**  
   automated smoke path는 강하지만, 실제 비개발자 벤치마크 evidence가 아직 없다. 북극성 목표는 사람 테스트 없이는 증명되지 않는다.

3. **Production adapter 리스크**  
   SQLite/local/file/HTTP bridge는 충분히 유용하지만, Postgres/MySQL direct driver와 provider-grade Stripe/shipping이 없다. production commerce platform claim은 아직 이르다.

4. **Native/editor product 리스크**  
   native launcher/source contract와 editor export는 깊지만, 실제 native optimized server와 first-party editor UI가 없다. 차별점이 product UX로 보이려면 M3/M4가 필요하다.

5. **Advanced domain surface explosion**  
   SPEC의 GPU/media/offline/push/ws/wt/webrtc/upload/net/mail/job/plugin/sync/observability/FFI는 각각 제품 하나 수준이다. Matrix에서 계속 non-binding/reference로 묶어야 한다.

## 다음 작업 우선순위

### P0: Contract Freeze 후보 만들기

- ProjectGraph JSON schema
- origin-map JSON schema
- runtime trace JSON schema
- build/deploy/preflight/benchmark evidence schema
- route origin/response origin header contract
- validation error response contract

각 schema마다 golden fixture와 `verify-build` drift test를 붙인다.

### P1: Shop acceptance를 실제로 닫기

- `orv init --template shop` fresh project에서 generated smoke를 CI-style로 돌리는 script 추가
- `deploy/benchmark-evidence.json` 샘플 evidence 작성
- `orv benchmark-report --require-pass`를 gate로 사용
- 1차 human benchmark를 최소 2-3명으로 실행
- 실패 시간을 문법/scaffold/error/editor issue로 분류

### P2: Production DB/commerce boundary 결정

- Postgres direct driver를 할지, HTTP adapter bridge를 MVP production path로 공식화할지 결정
- checkout transaction model 정의: stock decrement, order create, payment capture, shipping booking, compensation
- provider webhook replay/timestamp/idempotency table schema 고정
- provider secrets/vault/env redaction tests 추가

### P3: Reveal coverage를 제품 차별점으로 고정

- route/html/db/function/domain invocation별 reveal golden tests
- trace frame에서 response origin, DB operation origin, commerce adapter origin까지 source reveal
- static graph view에서 같은 origin schema를 확인하는 smoke gate
- editor export가 아니라 native editor UI에서 최소 route/source/trace reveal one-loop 구현

### P4: M4+를 non-binding으로 계속 격리

- GPU/media/sync/plugin/net/FFI는 `fixtures/spec-pressure` 또는 matrix `non-binding`에 묶기
- SPEC 예제마다 contract badge를 붙이기
- advanced domain은 shop benchmark를 개선하는 경우에만 MVP로 승격

## 결론

현재 orv는 문서 대비 "reference MVP는 많이 구현됐고, production platform은 아직 중간"이다.

가장 강한 구현 자산은 다음이다.

- compiler/runtime pipeline
- HIR origin and ProjectGraph contracts
- build/deploy/reveal artifacts
- shop scaffold and generated smoke/preflight evidence
- LSP/DAP/editor export bootstrap

가장 큰 남은 제품 기능은 다음이다.

- 실제 5시간 benchmark evidence
- contract-stable ProjectGraph/origin/trace schema
- production DB/provider adapters
- native optimized server/client generation
- first-party editor UI
- advanced domains의 명확한 non-binding 격리

따라서 다음 마일스톤은 "기능 추가"보다 "M0-M3 계약 안정화 + shop benchmark evidence 확보"가 맞다.
