# orv MVP

이 문서는 "지금 되는 것"과 "MVP에서 의도적으로 하지 않는 것"만 적는다. 언어 의미론은 [SPEC.md](SPEC.md)가 기준이고, 구현 구조는 [ARCHITECTURE.md](ARCHITECTURE.md)가 기준이다.

## 현재 MVP 한 줄

현재 orv는 `.orv` 소스를 로드, 파싱, 이름 해석, 의미 분석한 뒤 HIR을 레퍼런스 tree-walking 런타임으로 실행하는 초기 플랫폼이다. `orv-compiler`는 origin map과 build/deploy artifact contract를 만들고, `orv-runtime`은 HTTP/1.1 서버, reference DB, reference commerce adapter를 실행한다.

이 문서의 목적은 "아이디어를 MVP로 줄이자"가 아니라, 이미 구현 중인 MVP의 제품 경계를 선명하게 잡는 것이다. 상세 상태와 계약 레벨은 [IMPLEMENTATION_MATRIX.md](IMPLEMENTATION_MATRIX.md)를 따른다.

## MVP 포함 범위

| 영역 | 현재 목표 |
|------|-----------|
| 프로젝트 생성 | `orv init <dir> --template basic|shop` |
| 개발 루프 | `orv check`, `orv run`, `orv dev`, `orv test` |
| 언어 프론트엔드 | lexer/parser/AST, import 기반 멀티파일 로드, name resolution, HIR lowering |
| 서버 | `@server`, `@listen`, `@route`, `@respond`, `@serve`, HTTP/1.1 reference server |
| 요청 데이터 | `@param`, `@query`, `@header`, `@body`, `@request.rawBody`, JSON/form-urlencoded body |
| UI | `@html` 정적 렌더, 일부 `let sig` 기반 client bundle artifact |
| DB | in-memory table map, JSON snapshot, WAL, SQLite row JSON reference adapter |
| Commerce | local/file payment and shipping reference adapter, HTTP checked stub, Stripe webhook verification reference path |
| Shop scaffold | member, cart, catalog, checkout, payment, shipping, audit rows, admin read models, deploy runbook |
| Build contract | source bundle, project graph, origin map, server runtime artifact, deploy manifest |
| Verification | `orv verify-build`, `orv deploy-env-check`, generated preflight artifact, generated smoke-test |
| Reveal | `orv origins`, `orv graph`, `orv reveal`, LSP/editor reveal payload |
| Editor bootstrap | static editor snapshot/export/runtime/debug artifacts, first-party native editor UI still roadmap |
| External tools | LSP/DAP bootstrap and Tree-sitter package |

## MVP Product Slice

The product MVP is not "all language features". It is the smallest slice that can make the 5-hour shop benchmark credible:

- `@server`
- `@route`
- `@html`
- `@form`
- `@db`
- `@auth`
- `@payment`
- `@shipping`
- `@design`
- `orv init <dir> --template shop`
- `orv dev`
- `orv build --prod`
- `orv deploy-env-check`
- generated preflight artifact
- generated smoke-test

Everything else must either support this path directly or stay outside the MVP.

## Milestone Tracks

| Milestone | 범위 | 안정화 기준 |
|-----------|------|-------------|
| M0 | compiler/runtime foundation: parse, resolve, analyze, HIR, ProjectGraph, origin, reference runtime, basic CLI | `Span -> AST -> HIR -> origin` 연결이 깨지지 않음 |
| M1 | web app foundation: `@server`, `@route`, `@html`, form/body parse, schema validation, SQLite reference adapter, static serve | template-independent web fixture가 `check/run/build/smoke`를 통과 |
| M2 | shop foundation: auth/session, cart, order, mock payment, mock shipping, admin page, deploy artifact | `orv init --template shop`에서 running shop smoke test 통과 |
| M3 | reveal/editor foundation: graph view, origin reveal, runtime trace, LSP/DAP/bootstrap, editor protocol | CLI/static graph view만으로 runtime event에서 source로 reveal 가능 |
| M4+ | native optimizer, custom DB engine, advanced editor, production providers, advanced deploy | 별도 roadmap/prod hardening gate 필요 |

## M1/M2 Acceptance Smoke Path

사람 대상 5시간 테스트 전에 먼저 template-to-running-shop smoke test가 기준이다.

```bash
orv init my-shop --template shop
cd my-shop
orv check .
orv build . --prod --out dist
orv verify-build dist
orv deploy-env-check dist
orv run-build dist
sh dist/deploy/smoke-test.sh
```

초기 acceptance는 mock/local payment와 mock/local shipping을 사용한다. Stripe와 실제 carrier provider는 이 경로가 안정화된 뒤 production adapter milestone에서 다룬다.

## Explicit Non-Goals For MVP

These remain roadmap until promoted by benchmark evidence:

- full native optimizer and production-grade native server binary
- custom DB optimizer, sharding, replication, and advanced storage engine work
- full self-hosted first-party editor
- full DAP/LSP method matrix beyond current bootstrap
- CRDT collaboration
- `@gpu`, `@net`, raw transport domains
- general FFI and broad `@unsafe` workflows
- advanced object storage and cloud provider hardening
- production provider SDK matrix for payment/shipping

## Success Gate

MVP work should be judged by [BENCHMARK_SHOP_5H.md](BENCHMARK_SHOP_5H.md), not by feature count. A new feature belongs in MVP only if it reduces benchmark time, removes unsafe manual work, or makes the resulting shop easier to verify.
