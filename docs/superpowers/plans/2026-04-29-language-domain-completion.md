# Language Domain Completion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring the currently documented language/domain surface closer to `docs/SPEC.md` before adding more e2e fixtures.

**Architecture:** Keep the existing pipeline order: lexer/parser accepts source surface, resolver/analyzer preserves or validates it conservatively, runtime implements only core executable domains. Advanced domains not implemented in this phase must produce explicit diagnostics or stable no-op behavior instead of parser failures.

**Tech Stack:** Rust workspace (`orv-syntax`, `orv-resolve`, `orv-analyzer`, `orv-runtime`), `rtk cargo test`, `orv-cli check` fixtures.

**Status (2026-04-30):** This implementation pass is complete and superseded by the current codebase state. Parser surface gaps, core server/web/db runtime paths, advanced-domain reference stubs, fixture checks, HIR origin map generation with `contains`/`calls` edges, AST ProjectGraph v1 plus source/semantic graph depth stats, `orv test` reference test block runner bootstrap, `orv lsp snapshot` editor bootstrap JSON, `orv lsp reveal` production-to-LSP-location JSON, `orv lsp serve --stdio` Content-Length JSON-RPC initialize/shutdown/documentSymbol/diagnostic/definition/references/hover/completion bootstrap with didOpen/didChange full-sync open-buffer cache, `orv dap serve --stdio` Debug Adapter Protocol initialize/launch/configurationDone/setBreakpoints/setFunctionBreakpoints/dataBreakpointInfo/setDataBreakpoints/threads/stackTrace/scopes/variables/loadedSources/continue/step/disconnect/terminate bootstrap with project-graph-backed entry stack frame, function/data breakpoint frames, project variables, and reference runtime stdout/status/error variables, `orv.toml` `[project].entry` source-entry resolution with project-directory input, `orv init <dir>` minimal scaffold generation, initial `orv build` manifest/bundle-plan/origin-map/project-graph/source-bundle/source-bundled server-runtime/launch artifacts with `runtime_features`, `orv build --prod` deploy manifest/reference server entrypoint, artifact verification, import-aware artifact source reanalysis, source-bundled artifact reference execution, build-directory launcher/static execution, HTML-only zero-runtime static page output, client page/JS/WASM bundle targets with `client/app.wasm` minimum-module placeholder, build directory verification, `orv reveal <dir> <origin-id>` build artifact route/client bundle origin reveal with source-bundle fallback, explicit `@db.save/load` JSON snapshots, `@db.wal(path)` JSONL append+fsync replay, `@db.checkpoint()` WAL snapshot compaction, explicit `@db.savepoint()`/`@db.rollback(point)` memory-state restoration, WAL-backed savepoint and transaction rollback replay preservation, torn final WAL record recovery, `orv db plan/verify` schema migration dry-run/drift check, `orv db apply/migrate` schema snapshot updates with optional migration history, `orv db squash` history action compaction, `orv db migrate --data` JSON snapshot add/drop field execution, `orv db rollback` schema/data snapshot restoration, `orv db backup/restore` local JSON data snapshot artifacts, and runtime `@ffi`/`@net` method enforcement inside `@unsafe` were implemented in the working tree. Remaining gaps are now higher-level compiler/platform work: native production codegen/bundling, real client WASM/JS codegen/glue, full LSP/DAP method set, editor UI production reveal, HTTP/2/H3/QUIC transport, persistent DB external adapters/WAL archive/PITR/full crash matrix, ABI signature/native FFI loading, and real plugin sandboxing.

**DAP delta (2026-05-02):** DAP bootstrap now verifies breakpoints against ProjectGraph selectable nodes plus nested AST statement lines, evaluates conditional and hit-count breakpoints against Locals values, accepts function breakpoints against captured runtime call-stack names, accepts data breakpoints against Locals value changes, accepts ORV diagnostics/runtime exception filters, captures reference-runtime debug frames after executed HIR statements with active function call stack and per-frame stdout deltas, exposes runtime-evaluated Locals through `scopes`/`variables`, makes current frame locals available to `evaluate` and `completions`, exposes direct function call choices through `stepInTargets`, advances `next`/`stepIn`/`stepOut` through captured runtime frames with optional `stepIn.targetId`, continues to the next verified line/function/data breakpoint when one remains, supports `restart`, emits stdout/stderr `output` events, and emits stdio `initialized`/`stopped`/`continued`/`terminated` event frames. Remaining DAP work is a live long-running interpreter pause/resume loop.

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
