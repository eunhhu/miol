# Language Domain Completion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring the currently documented language/domain surface closer to `docs/SPEC.md` before adding more e2e fixtures.

**Architecture:** Keep the existing pipeline order: lexer/parser accepts source surface, resolver/analyzer preserves or validates it conservatively, runtime implements only core executable domains. Advanced domains not implemented in this phase must produce explicit diagnostics or stable no-op behavior instead of parser failures.

**Tech Stack:** Rust workspace (`orv-syntax`, `orv-resolve`, `orv-analyzer`, `orv-runtime`), `rtk cargo test`, `orv-cli check` fixtures.

**Status (2026-04-30):** This implementation pass is complete and superseded by the current codebase state. Parser surface gaps, core server/web/db runtime paths, advanced-domain reference stubs, fixture checks, HIR origin map generation with `contains`/`calls` edges, AST ProjectGraph v1 plus source/semantic graph depth stats, `orv test` reference test block runner bootstrap, `orv lsp snapshot` editor bootstrap JSON, `orv lsp reveal` production-to-LSP-location JSON, `orv lsp serve --stdio` Content-Length JSON-RPC initialize/shutdown/documentSymbol/diagnostic/definition/references/hover/completion bootstrap with didOpen/didChange full-sync open-buffer cache, `orv dap serve --stdio` Debug Adapter Protocol initialize/launch/configurationDone/setBreakpoints/setFunctionBreakpoints/dataBreakpointInfo/setDataBreakpoints/threads/stackTrace/scopes/variables/loadedSources/continue/step/disconnect/terminate bootstrap with project-graph-backed entry stack frame, function/data breakpoint frames, project variables, and reference runtime stdout/status/error variables, `orv.toml` `[project].entry` source-entry resolution with project-directory input, `orv init <dir>` minimal scaffold generation, initial `orv build` manifest/bundle-plan/origin-map/project-graph/source-bundle/source-bundled server-runtime/launch artifacts with `runtime_features`, `orv build --prod` deploy manifest/reference server entrypoint, artifact verification, import-aware artifact source reanalysis, source-bundled artifact reference execution, build-directory launcher/static execution, HTML-only zero-runtime static page output, client page/JS/WASM bundle targets with `client/app.wasm` minimum-module placeholder, build directory verification, `orv reveal <dir> <origin-id>` build artifact route/client bundle origin reveal with source-bundle fallback, explicit `@db.save/load` JSON snapshots, `@db.wal(path)` JSONL append+fsync replay, `@db.checkpoint()` WAL snapshot compaction, explicit `@db.savepoint()`/`@db.rollback(point)` memory-state restoration, WAL-backed savepoint and transaction rollback replay preservation, torn final WAL record recovery, `orv db plan/verify` schema migration dry-run/drift check, `orv db apply/migrate` schema snapshot updates with optional migration history, `orv db squash` history action compaction, `orv db migrate --data` JSON snapshot add/drop field execution, `orv db rollback` schema/data snapshot restoration, `orv db backup/restore` local JSON data snapshot artifacts, and runtime `@ffi`/`@net` method enforcement inside `@unsafe` were implemented in the working tree. Remaining gaps are now higher-level compiler/platform work: native production codegen/bundling, real client WASM/JS codegen/glue, full LSP/DAP method set, editor UI production reveal, HTTP/2/H3/QUIC transport, persistent DB external adapters/WAL archive/PITR/full crash matrix, ABI signature/native FFI loading, and real plugin sandboxing.

**DAP delta (2026-05-02):** DAP bootstrap now verifies breakpoints against ProjectGraph selectable nodes plus nested AST statement lines, evaluates conditional and hit-count breakpoints against Locals values, accepts function breakpoints against captured runtime call-stack names, accepts data breakpoints against Locals value changes, accepts ORV diagnostics/runtime exception filters, captures reference-runtime debug frames after executed HIR statements with active function call stack and per-frame stdout deltas, exposes runtime-evaluated Locals through `scopes`/`variables`, makes current frame locals available to `evaluate` and `completions`, exposes direct function call choices through `stepInTargets`, advances `next`/`stepIn`/`stepOut` through captured runtime frames with optional captured `stepIn.targetId`, rewinds the current captured function frame with `restartFrame`, continues to the next verified line/function/data breakpoint when one remains, supports `restart`, emits stdout/stderr `output` events, and emits stdio `initialized`/`stopped`/`continued`/`terminated` event frames. 2026-05-03 adds opt-in `launch.arguments.live=true` DebugStepper progression so launch does not pre-run the full program, `next`/`stepIn`/`stepOut`/`continue` incrementally advance runtime frames, live `stepIn.targetId` is rejected instead of ignored, `restart` preserves live mode unless explicitly overridden, and `@server` launch opens a non-blocking paused long-running frame with `continue`/`pause` events instead of starting the HTTP accept loop by default. It also exposes long-running async runtime kind/state, static/env listen endpoint, route inventory, transport process state/id/address, and pause/resume counters through launch runtime JSON, `variables`, `evaluate`, and `completions`, exposes in-process request frame count/last/list/trace and request trace file path through `variables`, `evaluate`, and `completions`, serves DAP `source` content from the launch-time project source snapshot, rejects debug-control requests for unknown thread ids, advertises `supportsOrvRuntimeAttach` and `supportsOrvRuntimeTracePath`, and supports opt-in `launch.arguments.attachRuntime=true` child-process accept-loop attach plus `attachRuntimeMode="inProcess"` attached server thread transport. Remaining DAP work is richer editor UI wiring.

**Dev/HMR delta (2026-05-03):** `orv dev --hmr` now emits `dev/session.json` with source hash watch inputs, bundle targets, and hot-reload/full-reload fallback strategy, and `orv dev --watch` emits `dev/watch.json` with poll-loop/watch target contract. `orv verify-build` validates both dev manifests when present. Remaining HMR work is the persistent process loop and browser/server transport.

**DB archive delta (2026-05-03):** `orv db recover` now accepts `--archive <archive.json>` as an alternative to raw `--wal`, resolves file archive targets, and verifies archived WAL hash/byte count before replay. Remaining DB persistence work is remote archive targets, external adapters, and full crash matrix coverage.

**Shop scaffold delta (2026-05-03):** `orv init --template shop` now writes the shopping route scaffold with a browser `GET /` HTML home route, product/member/order/payment/shipment POST forms, form-urlencoded `@body` parsing, and a README with check/build/verify/run-build commands, browser home URL, generated deploy runbook/Compose launch guidance, and the member/payment/shipment route inventory. Remaining shop north-star work is native server deployment, real payment/shipping adapters, persistent external DB adapters, and richer storefront/admin UI.

**Deploy route inventory delta (2026-05-03):** `orv build --prod` now writes `deploy/routes.json` for server builds and `orv verify-build` checks that it matches the server runtime artifact, giving deploy/reveal tooling a standalone route inventory before native bundling.

**Deploy container contract delta (2026-05-03):** `orv build --prod` now writes `deploy/container.json`, `deploy/Dockerfile`, `deploy/compose.yaml`, and `deploy/README.md` for server builds, records reference `runtime_image` plus listen/port exposure in the container contract, emits Dockerfile `EXPOSE` and Compose port/environment wiring for static or env-default nonzero listen ports, includes request trace capture/editor trace commands in the deploy runbook, and `orv verify-build` checks that the container contract points at the same runtime artifact, route inventory, entrypoint, runtime image, compose file, runbook, listen descriptor, ports, reference server command, and trace runbook guidance. Remaining production deploy work is native server binaries and real runtime images.

**Server listen artifact delta (2026-05-03):** origin maps now include `listen` nodes, server runtime artifacts preserve listen origin/static/env port descriptors, `server/launch.json` must match that listen descriptor during `orv verify-build`, and prod builds reject static test-only `@listen 0`. Remaining deployment work is native runtime images.

**DAP exception filter delta (2026-05-03):** `orv dap serve --stdio` now stores `setExceptionBreakpoints` diagnostics/runtime selections and only marks launch stops as `exception` when the active filter covers the runtime status. Remaining DAP work is richer editor UI wiring.

**DAP cancel delta (2026-05-03):** `orv dap serve --stdio` now advertises and accepts DAP `cancel` requests as synchronous no-op success responses, avoiding client-visible unsupported-command errors for adapters that send cancellation frames.

**Editor snapshot/reveal/runtime/export/trace delta (2026-05-03):** `orv editor snapshot <file>` now emits first-party editor bootstrap JSON with diagnostics, shared ProjectGraph, graph-backed Files/Routes/Schema/Domains panel inputs, and source-hash live refresh watch sources. `orv editor reveal <dir> <origin-id>` now converts build artifact origins into first-party editor focus/source/production navigation payloads. `orv editor runtime <file>` now reuses DAP trace/runtime helpers to emit runtime status/stdout/frame inspection pane JSON. `orv editor export <file> --out <dir>` now writes `state.json` plus a static `index.html` editor shell artifact with rendered panel lists, selectable trace detail, and optional trace navigation state with `--build <dir> --trace <trace.json>`. `orv editor trace <dir> --trace <trace.json>` now expands captured request trace frames into editor source/production navigation payloads. Remaining editor work is interactive/native UI rendering and live production trace capture transport.

**DAP request trace JSON delta (2026-05-03):** In-process attached runtime request frames now expose `runtimeRequestTrace` through DAP variables/evaluate/completions as `orv.production.trace` JSON, so captured traffic can feed `orv editor trace` without scraping display strings. `launch.arguments.runtimeRequestTracePath` now flushes the same trace JSON file on pause/terminate/disconnect and exposes the path through variables/evaluate/completions. The trace JSON schema/file writer is now owned by `orv-runtime` and reused by DAP file/display surfaces instead of being duplicated inside the CLI.

**Runtime request trace file delta (2026-05-03):** Normal `@server` runtime execution now honors `orv run-artifact/run-build --trace <path>` and `ORV_RUNTIME_REQUEST_TRACE_PATH`, writing the same `orv.production.trace` file on graceful shutdown so run/build/deploy processes have a non-DAP production trace capture path. Remaining trace work is live streaming/transport and richer native editor consumption.

**Editor trace summary delta (2026-05-03):** `orv editor trace` now adds per-frame request labels, route/status classes, aggregate status buckets, and a trace-file-hash live refresh contract so the exported editor shell can render captured traffic without recomputing request summaries in UI code. The exported shell also renders status buckets and client-side trace filters for all/2xx/3xx/4xx/5xx/other traffic.

---

### Task 1: Parser Surface Gap Closure

**Files:**
- Modify: `crates/orv-syntax/src/parser.rs`
- Modify: `crates/orv-syntax/src/lexer.rs`
- Test: `crates/orv-syntax/src/parser.rs`

- [x] Write failing parser tests for domain `key=value`, reserved prop names, shorthand lambdas, compound assignment, index assignment, optional chaining, inline object array types, and string/union type aliases.
- [x] Run focused parser tests and verify they fail on current parser behavior.
- [x] Implement the smallest AST-compatible parsing changes, preferring existing `ExprKind`/`TypeRefKind` where possible and preserving unsupported details as named/opaque forms.
- [x] Run focused parser tests and fixture checks for `fixtures/default-syntax.orv`, `fixtures/plan/03-domains.orv`, `fixtures/plan/04-web.orv`, `fixtures/plan/05-server.orv`.

### Task 2: Analyzer/Runtime Core Domain Gap Closure

**Files:**
- Modify: `crates/orv-analyzer/src/lib.rs`
- Modify: `crates/orv-runtime/src/interp.rs`
- Modify: `crates/orv-runtime/src/server.rs`
- Test: `crates/orv-runtime/src/server.rs`

- [x] Write failing checks for `@serve ./path`, `@db.find User { @where ... }`, `%data=...`, and HTML prop/event preservation.
- [x] Implement conservative lowering/runtime adapters for core server/web/db syntax used by `plan/04` and `plan/05`.
- [x] Add stable reference behavior for advanced domains where useful (`@offline`, `@cache`, `@net`, `@plugin`, `@gpu`, `@observability`, `@ffi`) while keeping non-core guarantees documented as roadmap.
- [x] Verify all current e2e fixtures still pass.

### Task 3: Fixture Gate

**Files:**
- Modify only if needed: `fixtures/plan/*.orv`, `fixtures/default-syntax.orv`
- Test: `orv-cli check`

- [x] Run `orv-cli check` across `fixtures/default-syntax.orv`, `fixtures/plan/01-basics.orv` through `fixtures/plan/09-shopping-mall.orv`, and `fixtures/e2e/*.orv`.
- [x] Classify any remaining failures as implementation gaps or intentionally future-only examples.
- [x] Add narrow syntax/runtime tests for each remaining implementation gap before fixing it.
- [x] Finish with `rtk timeout 120 cargo test`, `rtk cargo clippy --all-targets`, `rtk cargo fmt --check`, and `rtk git diff --check`.
