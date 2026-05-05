# orv Operational Surfaces

이 문서는 현재 CLI, editor/LSP/DAP, build/deploy, DB 운영 surface를 추적한다. 핵심 크레이트 구조와 데이터 흐름은 [ARCHITECTURE.md](ARCHITECTURE.md)에 둔다.

## CLI Surface

현재 `orv-cli`는 프로젝트 scaffold, 로드/해석/분석/실행, graph/origin 출력, editor/LSP/DAP bootstrap, build artifact 생성/검증/실행, lockfile 생성/검증, DB snapshot/migration workflow를 오케스트레이션한다.

주요 command:

- `orv init <dir> --name <name> [--template basic|shop]`
- `orv run/dev/check/dump/origins/graph/test`
- `orv editor snapshot/reveal/runtime/debug/run-debug/export/trace/trace-stream`
- `orv lsp snapshot/reveal/serve --stdio`
- `orv dap serve --stdio`
- `orv build <file-or-orv.toml> --out <dir> [--prod]`
- `orv add/remove`
- `orv lock [dir-or-orv.toml] [--check]`
- `orv fetch [dir-or-orv.toml] [--out <dir>]`
- `orv workspace new <member> [--root <dir>] [--name <name>]`
- `orv workspace graph [root] [--view] [--out <dir>]`
- `orv workspace lock [root] [--out <dir>]`
- `orv workspace fetch [root] [--out <dir>]`
- `orv workspace build [root] [--out <dir>] [--prod] [--incremental]`
- `orv verify-build/verify-artifact/check-artifact/check-build`
- `orv run-artifact/run-build/reveal`
- `orv db plan/verify/apply/migrate/rollback/backup/restore/recover/archive/squash`

Source-entry commands accept a single `.orv` file, an `orv.toml` with `[project].entry`, or a project directory containing `orv.toml`.

`orv graph <file> --view --out <dir>` writes `graph.json` and a static `index.html` ProjectGraph view with source/semantic depth stats, node/edge visualization, node search/kind filtering, and origin rows.

`orv dev --hmr` writes `dev/session.json`, `dev/transport.json`, and `dev/hmr-client.js` for the reference EventSource HMR browser transport; `orv dev --watch` writes `dev/watch.json`; `orv dev --watch-loop [--watch-iterations <n>]` runs the poll-loop build/verify/run path while writing `dev/events.json`; and `orv dev --hmr --serve [--serve-port <port>]` starts the reference HTTP/1 HMR endpoint with `dev/server.json`, `/__orv/hmr/session`, and `/__orv/hmr/events`.

`orv lock [dir-or-orv.toml]` reads `[project]`, `[dependencies]`, and `[dev-dependencies]` from `orv.toml`, writes deterministic JSON `orv.lock` entries sorted by package name, preserves registry/path sources, preserves optional `auth_token_env` names without storing secret tokens, resolves exact semver versions including prerelease/build metadata directly, resolves `*`, `x`, segment wildcards, caret ranges, tilde ranges, whitespace-AND comparator ranges, and `||` disjunction ranges from local/file/HTTP/HTTPS registry `index.json` into exact locked versions, preserves the original range as `requested_version`, and adds stable `fnv1a64` checksums. HTTP/HTTPS registry index requests use `Authorization: Bearer <token>` when `auth_token_env` is present. `--check` compares the existing lockfile without writing.

`orv fetch [dir-or-orv.toml] --out <dir>` verifies that `orv.lock` matches `orv.toml`, materializes path dependencies, local/file registry dependencies, and HTTP/HTTPS registry `/<package>/<version>/source-bundle.json` artifacts into source-bundle cache artifacts, sends `Authorization: Bearer <token>` for registry entries with `auth_token_env`, and writes `deps-manifest.json`.

`orv add <pkg> <version> [--dev] [--path <path>] [--registry <url>] [--manifest <dir-or-orv.toml>]` edits the selected dependency section and regenerates `orv.lock`; `orv remove <pkg> [--dev] [--manifest <dir-or-orv.toml>]` removes from the selected section and regenerates the lockfile.

`orv workspace new <member> [--root <dir>] [--name <name>]` creates a basic member project and records the relative member path in root `orv.toml` `[workspace].members` with resolver `2`.

`orv workspace graph [root] [--view] [--out <dir>]` reads root `[workspace].members`, loads each member entry through the shared ProjectGraph pipeline, records member graphs/files/dependencies, and emits path dependency edges between workspace members. Workspace path dependency edges include dependency package/section, target member name/version, requested version when present, and reject requested versions that do not match the target member version. With `--out`, it writes `workspace-graph.json`; with `--view`, it writes `workspace-graph.json` plus a static `index.html` workspace member/dependency graph view with member/edge search filtering.

`orv workspace lock [root] --out <dir>` reads the same workspace graph, orders members dependency-first from path dependency edges, writes per-member lockfiles under `members/<member>/orv.lock`, and emits `workspace-lock.json` with member project metadata, dependency lists, path dependency edges, lock order, and package counts without mutating member source directories.

`orv workspace fetch [root] --out <dir>` writes the same workspace graph and workspace lock artifacts, then materializes each member's lockfile into `members/<member>/deps/deps-manifest.json` and per-member dependency source-bundle caches. It emits `workspace-fetch.json` with dependency-first fetch order, member dependency manifests, and total package counts.

`orv workspace build [root] --out <dir> [--prod] [--incremental]` reuses the normal build pipeline for every workspace member, orders member builds dependency-first from path dependency edges, verifies each member build directory, writes member artifacts under `members/<member>`, and emits `workspace-build.json` plus `workspace-graph.json` as the top-level workspace build contract. With `--incremental`, unchanged member source-bundle input hashes are skipped when the previous verified build is still valid; rebuilt dependencies force dependent members to rebuild.

## Editor And LSP

`orv editor snapshot <file>` emits first-party editor bootstrap JSON with diagnostics, ProjectGraph, Files/Routes/Schema/Domains panel inputs, and source-hash watch sources. `orv editor reveal <dir> <origin-id>` maps build artifact origins to editor focus/source/production navigation payloads. `orv editor runtime <file>` reuses DAP trace/runtime helpers for runtime inspection pane JSON. `orv editor debug <file> --control <continue|pause|next|step-in|step-out|restart|disconnect>` runs an initialize/live-launch/control/stackTrace sequence over the same Content-Length DAP transport and emits the response/event frames for native editor wiring; repeated `--control` values run in order inside one session. `orv editor run-debug <state-or-runner.json> --control <...>` reads an exported `debug.session_runner` or `debug/session-runner.json` standalone artifact, replays the controls through one DAP session for the recorded program, and emits runner result JSON for native host execution. `orv editor export <file> --out <dir>` writes `state.json`, `debug/session-runner.json`, `native-host.json`, plus a static `index.html` shell with panel lists, ProjectGraph visualization, runtime frame inspection, DAP adapter launch/live/attach configuration wiring, live-control request payloads, native-host `session_runner` command metadata, executable breakpoint source lines, and optional trace navigation. `native-host.json` is the native editor host manifest: it points at the shell/state/debug runner artifacts, DAP adapter command, runner command, and trace/debug capabilities. `orv editor trace <dir> --trace <trace.json>` maps captured request frames back to source/production navigation and embeds the `/__orv/trace/events` open-ended EventSource transport URL when a server artifact has a stable listen endpoint. `orv editor trace-stream <dir> --events <trace-events.sse>` consumes a native-host EventSource body, extracts `orv:trace` snapshots plus `orv:trace.frame` deltas, and emits normalized editor trace stream JSON with latest trace payload and transport metadata.

Editor DAP control commands are now first-class in the export contract: every exported control carries the exact `orv editor run-debug debug/session-runner.json --control <name>` runner command, `native-host.json` mirrors those commands under `debug.control_commands`, and the static shell renders the selected runner command next to the DAP request payload.

Trace transport is also mirrored for native hosts: when an editor export includes trace state and the build has a stable server listen endpoint, `native-host.json` includes `trace.transport` with the `/__orv/trace/events` EventSource URL and the static shell renders the same transport in a Trace Transport pane.

`orv lsp serve --stdio` currently handles:

- lifecycle: `initialize`, `shutdown`, notifications, unknown-method errors
- document/project: `textDocument/documentSymbol`, `workspace/symbol`, `textDocument/diagnostic`, `workspace/diagnostic`
- editor affordances: `textDocument/codeLens`, `textDocument/codeAction`, `workspace/executeCommand`, `textDocument/documentLink`, `textDocument/documentColor`, `textDocument/colorPresentation`, `textDocument/foldingRange`, `textDocument/selectionRange`, `textDocument/semanticTokens/full`, `textDocument/linkedEditingRange`
- navigation: `textDocument/definition`, `textDocument/declaration`, `textDocument/typeDefinition`, `textDocument/implementation`, `textDocument/moniker`
- hierarchy: `textDocument/prepareCallHierarchy`, `callHierarchy/incomingCalls`, `callHierarchy/outgoingCalls`, `textDocument/prepareTypeHierarchy`, `typeHierarchy/supertypes`, `typeHierarchy/subtypes`
- editing/introspection: `textDocument/references`, `textDocument/documentHighlight`, `textDocument/prepareRename`, `textDocument/rename`, `textDocument/hover`, `textDocument/signatureHelp`, `textDocument/inlayHint`, `textDocument/completion`

The LSP session keeps `textDocument/didOpen` and full-sync `textDocument/didChange` buffers so later file URI requests can run against unsaved content.

## DAP

`orv dap serve --stdio` reuses the same project loader and ProjectGraph as CLI/LSP. It supports initialize/cancel/launch/attach/configurationDone, source/function/instruction/data breakpoints, breakpoint/goto/step-in target discovery, exception info, threads/stackTrace/scopes/variables, variable/expression mutation, evaluate/completions, loadedSources/modules/source, source-frame disassemble/readMemory, continue/reverseContinue/goto/stepIn/stepBack/restartFrame, disconnect/terminate/terminateThreads, and stdio lifecycle/output events.

Runtime debug state comes from the reference runtime debug trace. Long-running `@server` launches start as a paused runtime frame and expose `continue`/`pause` event-loop control. `attachRuntime=true` or the standard `attach` request can attach a child `orv run <program>` process; `attachRuntimeMode="inProcess"` starts an attached server thread in the DAP process and preserves the same DB handle across top-level prefix statements, server boot body, and route handlers. In-process request frames are exposed through variables/evaluate/completions and can be flushed to `orv.production.trace` JSON via `runtimeRequestTracePath`, normal `orv run-artifact/run-build --trace <path>`, or `ORV_RUNTIME_REQUEST_TRACE_PATH`. When request trace capture is enabled, the reference HTTP runtime also serves an EventSource-compatible snapshot at `/__orv/trace/events`.

## Build And Deploy Artifacts

`orv build <file-or-orv.toml> --out <dir>` creates deterministic reference artifacts:

- `build-manifest.json`
- `bundle-plan.json`
- `origin-map.json`
- `project-graph.json`
- `source-bundle.json`
- `server/app.orv-runtime.json`
- `server/launch.json`
- `server/native-server.json`
- `server/runtime-image.json`
- `server/native/Cargo.toml`
- `server/native/main.rs`
- `server/native/routes.rs`
- `server/native/router.rs`
- `pages/index.html` for HTML-only zero-runtime entries
- `client/manifest.json`, `client/reactive-plan.json`, `client/app.js`, and a source-bound `client/app.wasm` with initial-render memory exports for interactive client entries

`orv build --prod` adds deploy artifacts:

- `deploy/manifest.json`
- `deploy/routes.json`
- `deploy/container.json`
- `deploy/Dockerfile`
- `deploy/compose.yaml`
- `deploy/README.md`
- `deploy/server.sh`

`server/native-server.json` is a planned native server binary contract, not a final compiled binary. It records the current reference artifact, reference launcher, generated `server/native/Cargo.toml` launcher package, generated `server/native/main.rs` launcher source, generated `server/native/routes.rs` route table source, generated `server/native/router.rs` router dispatch source, `server/runtime-image.json` image plan, structured `commands.build`/`commands.run`, route/listen/runtime feature shape, planned `server/app` HTTP/1 target, and the `native-codegen`/`native-runtime-image` blockers. `server/runtime-image.json` records the reference runtime image, future OCI image target, native binary path, route/listen/runtime feature shape, and the same blockers without claiming a final image exists. `server/native/Cargo.toml`, `server/native/main.rs`, `server/native/routes.rs`, and `server/native/router.rs` form a generated Rust reference launcher package that checks the native plan and server artifact exist, links a typed route table with method/path/origin id constants, contained `@respond` origin ids, a `:param`/`:rest*`-aware `orv_native_match_route`, route param capture structs, `orv_native_param_value` lookup helper, and temporary `orv_native_dispatch` 501/404 dispatch contract, shells through `orv run-artifact`, and forwards process arguments; it is a codegen bridge, not the final zero-overhead native runtime. `deploy/README.md` documents the generated launcher path with `cargo build --manifest-path server/native/Cargo.toml --release` and `ORV_BUILD_DIR=. ./server/native/target/release/orv-native-server`; running from that generated path also infers the build directory, while `ORV_BUILD_DIR` remains an explicit override. The shop starter README emits the same workflow with `dist/`-prefixed paths. `deploy/manifest.json` references the plans through `server.native_plan`, `server.native_runtime_image_plan`, `server.native_routes_source`, and `server.native_router_source`; `orv verify-build` checks that the plan/package/source/routes-source/router-source/commands/image contract still match the server runtime artifact, and reveal/editor/LSP production payloads expose the matching native server target, build/run commands, route/router source summary, and runtime image plan summary for route origins.

When a server artifact contains static persistence paths such as `@db.wal("data/app.wal.jsonl")`, `@db.connect "file://data/app.wal.jsonl"`, or `@db.connect "sqlite://data/shop.sqlite"`, the deploy manifest and container contract record those WAL/SQLite paths and the generated Compose file mounts the parent directory, for example `../data:/app/data`.

`source-bundle.json` records source path/content/hash snapshots so reveal, LSP reveal, artifact verification, artifact reanalysis, and reference artifact execution do not depend on the original source files.

For interactive client entries, `client/manifest.json` binds the page, reactive plan, JS loader, WASM module, source bundle hash, WASM exports, initial render metadata, runtime features, and dynamic-client-codegen blockers into one checked artifact. `client/reactive-plan.json` records source-backed `let sig` origins, the initial-render binding, source bundle hash, and `reactive-dom-diff` blocker for the future DOM-diff codegen pass. The generated `client/app.js` fetches the manifest and reactive plan before loading the source bundle and WASM, rejects schema/hash/WASM/export/reactive-plan mismatches, and then records the resolved manifest URL plus reactive signal count on the client root. `orv verify-build` validates the manifest and reactive plan against the generated page/loader/WASM/source bundle, prod `deploy/manifest.json` references the manifest as `client.manifest`, and reveal/editor/LSP production payloads expose both client contracts for client origins.

## DB Operations

Runtime `@db` currently uses an in-memory execution model with explicit JSON snapshot and WAL APIs: `@db.save/load`, `@db.wal(path)`, `@db.checkpoint()`, `@db.savepoint()`, and `@db.rollback(point)`. `@server` boot body DB setup is carried into route handlers, so a server-level SQLite handle such as `let shopdb = @db.connect "sqlite://data/shop.sqlite"` can be captured by routes and persist product/member/order/payment/shipment mutations across runs. `@db.connect` accepts the reference `memory://` adapter, a local WAL-backed `file://path` adapter, and a SQLite-backed `sqlite://path` adapter that stores ORV table metadata plus row JSON in a real SQLite file while preserving current query semantics. PostgreSQL/MySQL adapter URLs are rejected until real provider implementations exist. Build/deploy/server runtime artifacts include the `db_adapter` runtime feature when source uses `@db.connect`. Production deploy persistence scans `@db.wal("relative/path.jsonl")`, `@db.connect "file://relative/path.jsonl"`, and `@db.connect "sqlite://relative/path.sqlite"`, records SQLite files as `db_paths`, and mounts eligible parent directories into Compose.

Runtime commerce adapters currently provide local reference handles: `@payment.connect("test://local").capture(...)` returns captured payment metadata and `@shipping.connect("test://local").book(...)` returns shipment booking metadata. The shop scaffold uses file-backed local handles, `@payment.connect("file://data/payments.jsonl")` and `@shipping.connect("file://data/shipments.jsonl")`, so capture/booking records are appended and synced to local JSONL files before matching DB rows are persisted. Prod deploy persistence records those relative file adapter paths as `record_paths` and mounts their parent directories into Compose. Builds that use these connections record `payment_adapter` and `shipping_adapter` runtime features in build, server runtime, and deploy artifacts. External payment/shipping adapter URLs are rejected until real provider integrations exist, so the scaffold does not silently pretend to support live providers.

CLI DB commands provide schema/data dry-run, drift verification, apply/migrate with history, rollback, local backup/restore, hash-verified WAL recovery, archive manifest generation, manifest-relative source WAL resolution, raw-WAL/archive point-in-time restore, and history squash.
