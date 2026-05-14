# orv Roadmap

이 문서는 미래 기능만 둔다. 현재 구현/계약 상태는 [IMPLEMENTATION_MATRIX.md](IMPLEMENTATION_MATRIX.md), MVP 경계는 [MVP.md](MVP.md)를 따른다.

Milestone 이름은 [IMPLEMENTATION_MATRIX.md](IMPLEMENTATION_MATRIX.md)의 M0~M4+ 정의를 따른다. 이미 구현 중인 기반은 "새로 시작할 MVP"가 아니라 안정화할 제품 축으로 본다.

## M0/M1: Foundation Hardening

- stabilize `Span -> AST -> HIR -> origin id` continuity
- keep `orv graph`, `orv origins`, route origin headers, and trace JSON on one origin schema
- add regression tests for route/html/db/function/domain reveal paths
- make typed body/form validation produce predictable HTTP errors
- keep ProjectGraph/HIR origin contracts independent from future native optimizer work

## M2: Shop MVP Hardening

- stabilize `orv init <dir> --template shop`
- make checkout flow transactional and auditable
- connect typed `@body`/`@form` validation to HTTP responses
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

## Later / Research

- custom orv-db storage engine and optimizer
- sharding, replication, and PITR operator UX beyond reference paths
- CRDT collaboration
- `@gpu`, `@net`, raw protocol domains
- broad FFI and `@unsafe` ecosystem
- self-hosted editor/runtime

## Promotion Rule

A roadmap item moves into MVP only when it directly improves [BENCHMARK_SHOP_5H.md](BENCHMARK_SHOP_5H.md) or closes a documented production safety gap in [SECURITY_MODEL.md](SECURITY_MODEL.md).
