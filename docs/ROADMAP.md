# orv Roadmap

이 문서는 미래 기능, 현재 실행 우선순위, 진행 오버헤드 규칙을 둔다. 현재 구현/계약 상태는 [IMPLEMENTATION_MATRIX.md](IMPLEMENTATION_MATRIX.md), MVP 경계는 [MVP.md](MVP.md)를 따른다.

Milestone 이름은 [IMPLEMENTATION_MATRIX.md](IMPLEMENTATION_MATRIX.md)의 M0~M4+ 정의를 따른다. 이미 구현 중인 기반은 "새로 시작할 MVP"가 아니라 안정화할 제품 축으로 본다.

## Current Execution Order

[IMPLEMENTATION_GAP_REPORT.md](IMPLEMENTATION_GAP_REPORT.md)를 반영한 현재 실행 순서는 다음이다.

1. Contract freeze 후보를 먼저 닫는다: ProjectGraph JSON, origin-map JSON, runtime trace JSON, build/deploy/preflight/benchmark evidence schema, route/response origin header, validation error response.
2. Shop acceptance를 실제로 닫는다: fresh `orv init --template shop` automated smoke, benchmark evidence sample, `benchmark-report --require-pass`, 2-3명 human run.
3. Production DB/commerce boundary를 결정한다: direct Postgres/MySQL vs HTTP bridge official path, checkout transaction/compensation, provider webhook replay/idempotency, secret redaction.
4. Reveal coverage를 제품 차별점으로 고정한다: route/html/db/function/domain invocation/trace frame reveal golden tests and static graph view gates.
5. M4+ advanced domains는 `non-binding` 또는 `reference`로 유지한다. Shop benchmark나 security model을 직접 개선할 때만 MVP로 승격한다.

## Overhead Control

진행률을 올리기 위해 작업 단위를 작게 유지한다.

- 한 큐는 하나의 invariant, 하나의 narrow patch, 하나의 targeted verification을 기본값으로 한다.
- 큰 full-suite 테스트보다 `rtk cargo test -p <crate> <test-name>`, `rtk cargo check -p <crate>`, `rtk cargo clippy -p <crate> --tests -- -D warnings`를 먼저 사용한다.
- `build_deploy.rs`, `editor_lsp_dap.rs`, `runtime/interp.rs` 같은 대형 파일은 feature 추가와 분리해 pure helper/module seam부터 쪼갠다.
- 문서 변경은 `IMPLEMENTATION_MATRIX.md`를 먼저 맞추고, 사용자-facing 요약이 바뀔 때만 `README.md`, `MVP.md`, `IMPLEMENTATION_STATUS.md`를 갱신한다.
- 전체 test sweep은 release gate, shared schema 변경, runtime/security boundary 변경 때만 실행한다.

## M0/M1: Foundation Hardening

- stabilize `Span -> AST -> HIR -> origin id` continuity
- keep `orv graph`, `orv origins`, route origin headers, and trace JSON on one origin schema
- add regression tests for route/html/db/function/domain reveal paths
- stabilize typed body/form validation 400 error payloads with golden tests
- keep ProjectGraph/HIR origin contracts independent from future native optimizer work

## M2: Shop MVP Hardening

- stabilize `orv init <dir> --template shop`
- make checkout flow transactional and auditable
- keep shop forms covered by typed `@body`/`@form` validation response smoke/golden checks
- document safe auth/session/csrf/rate-limit defaults
- make `orv dev -> build --prod -> deploy-env-check -> smoke-test` one clear path
- run the 5-hour shop benchmark and publish results

## M4+: Production Adapters

- PostgreSQL adapter with schema/migration DSL and transaction semantics
- provider-grade payment/shipping adapters
- webhook replay protection and credential rotation hardening
- cloud archive/backup provider matrix
- deploy profile hardening for secrets, headers, and audit trail

## M4+: Compiler And Runtime

- semantic project graph expansion from HIR
- server native binary generation beyond direct-lowered route slices
- dynamic and optimized client WASM/JS codegen
- dead-code elimination by domain and runtime feature
- render strategy inference and route-level bundle splitting

## M3/M4+: First-Party Editor

- native editor shell consuming `native-host.json`
- source/production reveal UI
- route/schema/domain panels driven by the project graph
- inline value flow from dev runtime traces
- design editing mode for `@html` and `@design`
- AI autocomplete using parser-constrained context, spec/example RAG, validated synthetic data, fixed evals, and later local fine-tuned small code models

## Later / Research

- custom orv-db storage engine and optimizer
- sharding, replication, and PITR operator UX beyond reference paths
- CRDT collaboration
- `@gpu`, `@net`, raw protocol domains
- broad FFI and `@unsafe` ecosystem
- self-hosted editor/runtime

## Promotion Rule

A roadmap item moves into MVP only when it directly improves [BENCHMARK_SHOP_5H.md](BENCHMARK_SHOP_5H.md) or closes a documented production safety gap in [SECURITY_MODEL.md](SECURITY_MODEL.md).
