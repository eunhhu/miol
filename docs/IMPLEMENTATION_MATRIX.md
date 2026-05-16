# orv Implementation Matrix

이 문서는 구현 중인 orv의 **상태 + 계약 레벨 + 검증 기준 + 담당 crate**를 한 번에 보여준다. 단순히 "구현됨"인지보다 "제품 표면으로 안정 계약을 걸 수 있는지"를 구분하는 것이 목적이다.

언어 의미론은 [SPEC.md](SPEC.md), 현재 MVP 경계는 [MVP.md](MVP.md), 구현 구조는 [ARCHITECTURE.md](ARCHITECTURE.md), 운영 command/method 세부는 [OPERATIONAL_SURFACES.md](OPERATIONAL_SURFACES.md)를 따른다.

## Core Spine

현재 안정화의 중심축은 다음 네 가지다.

```text
ProjectGraph + HIR Origin + Reference Runtime + Trace/Reveal
```

이 축이 깨지면 editor, deploy, native optimizer, shop scaffold 모두 신뢰를 잃는다. 따라서 feature 추가보다 먼저 다음 연결을 안정 계약으로 올린다.

- `Span -> AST node -> HIR node -> runtime event -> origin id`
- `orv graph`, `orv origins`, `x-orv-origin-id`, `x-orv-response-origin-id`, trace JSON의 origin schema 정합성
- route, DB query, HTML node, function call, domain invocation의 동일 reveal 모델
- first-party editor 없이도 CLI/static graph view만으로 production output에서 source로 돌아가는 경로

## Status Terms

| Status | 의미 |
|--------|------|
| implemented | 현재 코드 경로가 동작하고 검증 대상으로 볼 수 있음 |
| reference stub | 레퍼런스 런타임/scaffold에서 제한적으로 동작함 |
| artifact only | 실행 기능보다 산출물/계약/manifest가 먼저 고정됨 |
| planned | 설계 방향은 있으나 구현 경로가 아직 없음 |
| not started | 문서상 아이디어 수준 |

## Contract Terms

| Contract | 의미 |
|----------|------|
| stable | 외부 사용자와 문서가 의존해도 되는 계약. 변경 시 migration/release note 필요 |
| stable-ish | MVP 내부 기준으로 안정화 중. 이름/JSON shape 변경 가능성 낮음 |
| experimental | 구현은 있으나 edge case와 문서 계약이 아직 흔들릴 수 있음 |
| reference | production provider가 아니라 reference/runtime/scaffold 기준 계약 |
| unstable | 개발 중인 내부 surface. 사용자는 직접 의존하지 않는 것이 좋음 |
| non-binding | 로드맵/디자인 방향. 구현 의무 없음 |

## Milestone Terms

| Milestone | 목적 |
|-----------|------|
| M0 | compiler/runtime foundation: parse, resolve, analyze, HIR, graph, origin, reference runtime, basic CLI |
| M1 | web app foundation: `@server`, `@route`, `@html`, form/body parse, schema validation, SQLite reference adapter, static serve, smoke test |
| M2 | shop foundation: auth/session, cart, order, mock payment, mock shipping, admin page, deploy artifact |
| M3 | reveal/editor foundation: graph view, origin reveal, runtime trace, LSP/DAP/bootstrap, editor protocol |
| M4+ | native optimizer, custom DB engine, advanced editor, production providers, advanced deploy |

## Matrix

| Feature | Status | Contract | Milestone | Crate | Test / Fixture | CLI | Notes |
|---------|--------|----------|-----------|-------|----------------|-----|-------|
| Source load / import DFS | implemented | stable-ish | M0 | `orv-project` | `fixtures/e2e/hello.orv` | `orv check` | Merged program + source map |
| Lexer / parser / AST | implemented | stable-ish | M0 | `orv-syntax` | `fixtures/e2e/hello.orv` | `orv check` | Span-backed AST |
| Name resolution | implemented | experimental | M0 | `orv-resolve` | `fixtures/plan/models/*.orv` | `orv check` | Scope/binding map |
| Semantic analysis / HIR lowering | implemented | experimental | M0 | `orv-analyzer`, `orv-hir` | `fixtures/e2e/hello.orv` | `orv check`, `orv run` | Runtime/compiler consume HIR |
| Diagnostics | implemented | stable-ish | M0 | `orv-diagnostics` | compiler fixture suite | `orv check` | Span-backed structured diagnostics |
| AST ProjectGraph v1 | implemented | experimental | M0/M3 | `orv-project`, `orv-cli` | CLI graph tests | `orv graph` | File/import/declaration/domain graph |
| HIR origin map | implemented | experimental | M0/M3 | `orv-hir`, `orv-compiler` | origin/graph CLI tests | `orv origins`, `orv graph` | Contains/calls semantic edges |
| Reference tree-walking runtime | implemented | experimental | M0 | `orv-runtime` | `fixtures/e2e/hello.orv` | `orv run` | Main execution path |
| HTTP/1.1 `@server` / `@route` | implemented | experimental | M1 | `orv-runtime` | `fixtures/e2e/hello.orv`, `fixtures/e2e/path_param.orv` | `orv run` | Hyper reference server |
| Route origin header | implemented | experimental | M1/M3 | `orv-runtime`, `orv-compiler` | origin runtime tests | `orv run` | Emits `x-orv-origin-id` |
| Request body parsing | implemented | experimental | M1 | `orv-runtime` | `fixtures/e2e/shopping_mall.orv` | `orv run` | JSON/form-urlencoded into `@body`; raw body available |
| Typed body/form validation | implemented | experimental | M1 | `orv-syntax`, `orv-runtime` | request binding runtime tests, `fixtures/e2e/shopping_mall.orv` | `orv run`, `orv init` | `@body: T`, `@query: T`, `@form: T` named-schema bindings use runtime validators, normalize request-state values, and return 400 `validation_failed` payloads on failure |
| `@html` static render | implemented | experimental | M1 | `orv-runtime`, `orv-compiler` | `fixtures/e2e/shopping_mall.orv` | `orv run`, `orv build` | HTML page/static build path |
| Client reactive bundle | artifact only | unstable | M4+ | `orv-compiler`, `orv-cli` | build artifact tests | `orv build`, `orv verify-build` | Manifest/reactive plan/JS/WASM bootstrap; full DOM diff roadmap |
| In-memory `@db` | implemented | reference | M1 | `orv-runtime` | `fixtures/e2e/shopping_mall.orv` | `orv run` | CRUD/filter/sort/limit/reference aggregation |
| DB snapshot/WAL/checkpoint | implemented | reference | M1 | `orv-runtime`, `orv-cli` | DB CLI/runtime tests | `orv db *` | Reference persistence/recovery path |
| SQLite row JSON adapter | implemented | reference | M1/M2 | `orv-runtime` | `fixtures/e2e/shopping_mall.orv` | `orv run` | SQLite file with ORV metadata + row JSON |
| PostgreSQL/MySQL adapters | reference stub | reference | M4+ | `orv-runtime`, `orv-cli` | external DB adapter bridge runtime/deploy artifact/env-check/smoke tests | `orv run`, `orv build --prod`, `orv deploy-env-check`, `deploy/smoke-test.sh` | Default handles expose explicit unsupported status/fail query methods; when `ORV_DB_ADAPTER_POSTGRES_ENDPOINT`, `ORV_DB_ADAPTER_MYSQL_ENDPOINT`, or `ORV_DB_ADAPTER_ENDPOINT` is configured, query methods POST checked `http-json-v1` requests to the external bridge with bounded transient retry and return its JSON response. Prod artifacts expose the bridge request/retry shape plus provider-specific and generic endpoint/auth env knobs, deploy env check requires a provider-specific or generic bridge endpoint before launch, and generated smoke probes bridge `schema`. Direct provider drivers remain planned |
| Auth/member session scaffold | reference stub | reference | M2 | `orv-cli`, `orv-runtime` | `fixtures/e2e/shopping_mall.orv` | `orv init`, `orv run` | Member/session rows exist, signup stores Argon2 `passwordHash` through `hash.password`, login verifies with `hash.verify`, successful login emits `orv_session` plus role cookies with HttpOnly/SameSite/Secure defaults, `@session required` gates cookie-backed routes, and reference `@Auth required role="admin"` gates shop admin read models |
| CSRF/rate-limit/security defaults | partial | reference | M2 | `orv-runtime`, `orv-cli` | shopping fixture security assertions, rate-limit runtime test | `orv check`, `orv run` | Shop scaffold persists AuditEvent rows, emits reference login session cookies, gates account sessions with `@session required`, protects browser mutation routes with `@csrf`, reference server rate-limits login/checkout/webhook hotspots, and build/server/deploy/native artifacts expose matching `auth_roles`, `session_cookies`, `csrf_protection`, and `rate_limit` runtime features plus per-route `auth`/`session`/`csrf`/`rate_limit` policy descriptors; explicit `@csrf exempt`, `@rateLimit key=... limit=... window=...`, and `@rateLimit exempt` mark intentional exemptions/overrides |
| Payment/shipping local adapters | implemented | reference | M2 | `orv-runtime` | `fixtures/e2e/shopping_mall.orv` | `orv run` | Local/file capture and booking records |
| Payment/shipping HTTP adapters | reference stub | reference | M2/M4+ | `orv-runtime` | commerce adapter tests | `orv run` | Checked JSON POST contract |
| Stripe webhook verification | reference stub | reference | M2/M4+ | `orv-runtime`, `orv-cli` | shop scaffold tests | `orv run`, `orv deploy-env-check` | HMAC/idempotency reference path |
| Provider SDK matrix | planned | non-binding | M4+ | - | - | - | Production hardening later |
| `orv init <dir> --template shop` | implemented | experimental | M2 | `orv-cli` | `fixtures/e2e/shopping_mall.orv` | `orv init` | Catalog/cart/member/checkout/admin scaffold with editable `@design` color/spacing/typography tokens on the home shell and an end-to-end `ProductInput.badge` field path |
| Template-to-running-shop smoke path | implemented | experimental | M2 | `orv-cli`, `orv-runtime` | generated smoke-test | `orv init`, `orv build --prod`, smoke-test | First acceptance target before human 5h runs; generated prod smoke now gates on `source-bundle.json`/`project-graph.json`/`origin-map.json` through `orv verify-build .`, then checks route reachability, exact `x-orv-origin-id`/`x-orv-response-origin-id` headers, route/response/DB/commerce source reveal through CLI reveal, editor reveal, and LSP reveal, home copy/theme token rendering, CSRF/session/admin cookies, three product creates, checkout response markers, editable product field propagation, and customer/admin read-model body markers |
| Build artifacts | implemented | experimental | M1/M3 | `orv-compiler`, `orv-cli` | build artifact tests | `orv build`, `orv verify-build` | Manifest, bundle plan, origin map, graph, source bundle; verify-build rejects project-graph/source-bundle/origin-map drift plus server route/listen/response origin drift |
| Native server plan/source | artifact only | unstable | M4+ | `orv-compiler`, `orv-cli` | build artifact tests | `orv build` | Contract first; full native optimizer planned |
| Deploy artifacts | implemented | experimental | M2 | `orv-cli`, `orv-compiler` | deploy artifact tests | `orv build --prod`, `orv deploy-env-check` | Manifest/container/Compose/runbook/env/preflight/benchmark-evidence/smoke-test contracts; preflight names the same source-bundle/project-graph/origin-map graph artifacts that verify-build and generated smoke gate on, includes the 5-hour shop benchmark contract, links a checked `deploy/benchmark-evidence.json` template keyed to the preflight hash, plus trace-enabled run-build command for trace-stream smoke |
| `orv reveal` / editor reveal payload | implemented | experimental | M3 | `orv-cli`, `orv-compiler` | reveal CLI tests | `orv reveal`, `orv editor reveal` | Build origin to source/production payload |
| Runtime trace JSON / trace stream | implemented | experimental | M3 | `orv-runtime`, `orv-cli` | editor trace tests, optional generated smoke trace-stream check | `orv editor trace`, `orv editor trace-stream` | Shared trace schema for reveal; trace-enabled deploy smoke can capture `/__orv/trace/events` and normalize it through editor trace-stream |
| LSP bootstrap | implemented | experimental | M3 | `orv-cli` | LSP CLI tests | `orv lsp serve` | Symbols/diagnostics/navigation/format/completion subsets |
| DAP bootstrap | implemented | experimental | M3 | `orv-cli`, `orv-runtime` | DAP CLI tests, editor debug runner tests | `orv dap serve`, `orv editor debug`, `orv editor run-debug` | Runtime frame/locals/debug control subsets; launch-time `loadedSources`/`source` snapshots carry imported source checksums into editor/native-host debug payloads; `orv:frame:N` instruction breakpoints verify against pseudo-instruction frames |
| Static editor export | implemented | experimental | M3 | `orv-cli` | editor export tests | `orv editor export` | Graph/panel/trace HTML artifacts; production export mirrors source-bundle/project-graph/origin-map graph contract into `state.json`, `native-host.json`, and `production/panel.html` |
| First-party native editor UI | planned | non-binding | M4+ | - | - | - | Native shell and production reveal UI later |
| `@gpu` / `@net` / CRDT / broad FFI | reference stub | non-binding | M4+ | `orv-runtime`, `orv-analyzer` | `fixtures/e2e/domains.orv` | `orv run` | Syntax/design pressure, not MVP production path |

## Update Rule

When implementation changes, update this matrix first. Then adjust [MVP.md](MVP.md), [ROADMAP.md](ROADMAP.md), [CHANGELOG.md](CHANGELOG.md), or [SPEC.md](SPEC.md) only if the product boundary, future plan, dated delta, or language contract changed.
