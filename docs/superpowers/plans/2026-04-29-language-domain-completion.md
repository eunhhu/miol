# Language Domain Completion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring the currently documented language/domain surface closer to `docs/SPEC.md` before adding more e2e fixtures.

**Architecture:** Keep the existing pipeline order: lexer/parser accepts source surface, resolver/analyzer preserves or validates it conservatively, runtime implements only core executable domains. Advanced domains not implemented in this phase must produce explicit diagnostics or stable no-op behavior instead of parser failures.

**Tech Stack:** Rust workspace (`orv-syntax`, `orv-resolve`, `orv-analyzer`, `orv-runtime`), `rtk cargo test`, `orv-cli check` fixtures.

---

### Task 1: Parser Surface Gap Closure

**Files:**
- Modify: `crates/orv-syntax/src/parser.rs`
- Modify: `crates/orv-syntax/src/lexer.rs`
- Test: `crates/orv-syntax/src/parser.rs`

- [ ] Write failing parser tests for domain `key=value`, reserved prop names, shorthand lambdas, compound assignment, index assignment, optional chaining, inline object array types, and string/union type aliases.
- [ ] Run focused parser tests and verify they fail on current parser behavior.
- [ ] Implement the smallest AST-compatible parsing changes, preferring existing `ExprKind`/`TypeRefKind` where possible and preserving unsupported details as named/opaque forms.
- [ ] Run focused parser tests and fixture checks for `fixtures/default-syntax.orv`, `fixtures/plan/03-domains.orv`, `fixtures/plan/04-web.orv`, `fixtures/plan/05-server.orv`.

### Task 2: Analyzer/Runtime Core Domain Gap Closure

**Files:**
- Modify: `crates/orv-analyzer/src/lib.rs`
- Modify: `crates/orv-runtime/src/interp.rs`
- Modify: `crates/orv-runtime/src/server.rs`
- Test: `crates/orv-runtime/src/server.rs`

- [ ] Write failing checks for `@serve ./path`, `@db.find User { @where ... }`, `%data=...`, and HTML prop/event preservation.
- [ ] Implement conservative lowering/runtime adapters for core server/web/db syntax used by `plan/04` and `plan/05`.
- [ ] Leave advanced domains (`@ws`, `@wt`, `@webrtc`, `@storage`, `@mail`, `@net`) outside runtime execution with clear unsupported handling.
- [ ] Verify all current e2e fixtures still pass.

### Task 3: Fixture Gate

**Files:**
- Modify only if needed: `fixtures/plan/*.orv`, `fixtures/default-syntax.orv`
- Test: `orv-cli check`

- [ ] Run `orv-cli check` across `fixtures/default-syntax.orv`, `fixtures/plan/01-basics.orv` through `fixtures/plan/09-shopping-mall.orv`, and `fixtures/e2e/*.orv`.
- [ ] Classify any remaining failures as implementation gaps or intentionally future-only examples.
- [ ] Add narrow syntax/runtime tests for each remaining implementation gap before fixing it.
- [ ] Finish with `rtk timeout 120 cargo test`, `rtk cargo clippy --all-targets`, `rtk cargo fmt --check`, and `rtk git diff --check`.
