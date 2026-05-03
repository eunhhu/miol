# orv Operational Surfaces

이 문서는 현재 CLI, editor/LSP/DAP, build/deploy, DB 운영 surface를 추적한다. 핵심 크레이트 구조와 데이터 흐름은 [ARCHITECTURE.md](ARCHITECTURE.md)에 둔다.

## CLI Surface

현재 `orv-cli`는 프로젝트 scaffold, 로드/해석/분석/실행, graph/origin 출력, editor/LSP/DAP bootstrap, build artifact 생성/검증/실행, lockfile 생성/검증, DB snapshot/migration workflow를 오케스트레이션한다.

주요 command:

- `orv init <dir> --name <name> [--template basic|shop]`
- `orv run/dev/check/dump/origins/graph/test`
- `orv editor snapshot/reveal/runtime/export/trace`
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

`orv graph <file> --view --out <dir>` writes `graph.json` and a static `index.html` ProjectGraph view with source/semantic depth stats, node/edge visualization, and origin rows.

`orv dev --hmr` writes `dev/session.json`, `dev/transport.json`, and `dev/hmr-client.js` for the reference EventSource HMR browser transport; `orv dev --watch` writes `dev/watch.json`; `orv dev --watch-loop [--watch-iterations <n>]` runs the poll-loop build/verify/run path while writing `dev/events.json`; and `orv dev --hmr --serve [--serve-port <port>]` starts the reference HTTP/1 HMR endpoint with `dev/server.json`, `/__orv/hmr/session`, and `/__orv/hmr/events`.

`orv lock [dir-or-orv.toml]` reads `[project]`, `[dependencies]`, and `[dev-dependencies]` from `orv.toml`, writes deterministic JSON `orv.lock` entries sorted by package name, preserves registry/path sources, preserves optional `auth_token_env` names without storing secret tokens, resolves exact semver versions including prerelease/build metadata directly, resolves `*`, `x`, segment wildcards, caret ranges, tilde ranges, whitespace-AND comparator ranges, and `||` disjunction ranges from local/file/HTTP/HTTPS registry `index.json` into exact locked versions, preserves the original range as `requested_version`, and adds stable `fnv1a64` checksums. HTTP/HTTPS registry index requests use `Authorization: Bearer <token>` when `auth_token_env` is present. `--check` compares the existing lockfile without writing.

`orv fetch [dir-or-orv.toml] --out <dir>` verifies that `orv.lock` matches `orv.toml`, materializes path dependencies, local/file registry dependencies, and HTTP/HTTPS registry `/<package>/<version>/source-bundle.json` artifacts into source-bundle cache artifacts, sends `Authorization: Bearer <token>` for registry entries with `auth_token_env`, and writes `deps-manifest.json`.

`orv add <pkg> <version> [--dev] [--path <path>] [--registry <url>] [--manifest <dir-or-orv.toml>]` edits the selected dependency section and regenerates `orv.lock`; `orv remove <pkg> [--dev] [--manifest <dir-or-orv.toml>]` removes from the selected section and regenerates the lockfile.

`orv workspace new <member> [--root <dir>] [--name <name>]` creates a basic member project and records the relative member path in root `orv.toml` `[workspace].members` with resolver `2`.

`orv workspace graph [root] [--view] [--out <dir>]` reads root `[workspace].members`, loads each member entry through the shared ProjectGraph pipeline, records member graphs/files/dependencies, and emits path dependency edges between workspace members. Workspace path dependency edges include dependency package/section, target member name/version, requested version when present, and reject requested versions that do not match the target member version. With `--out`, it writes `workspace-graph.json`; with `--view`, it writes `workspace-graph.json` plus a static `index.html` workspace member/dependency graph view.

`orv workspace lock [root] --out <dir>` reads the same workspace graph, orders members dependency-first from path dependency edges, writes per-member lockfiles under `members/<member>/orv.lock`, and emits `workspace-lock.json` with member project metadata, dependency lists, path dependency edges, lock order, and package counts without mutating member source directories.

`orv workspace fetch [root] --out <dir>` writes the same workspace graph and workspace lock artifacts, then materializes each member's lockfile into `members/<member>/deps/deps-manifest.json` and per-member dependency source-bundle caches. It emits `workspace-fetch.json` with dependency-first fetch order, member dependency manifests, and total package counts.

`orv workspace build [root] --out <dir> [--prod] [--incremental]` reuses the normal build pipeline for every workspace member, orders member builds dependency-first from path dependency edges, verifies each member build directory, writes member artifacts under `members/<member>`, and emits `workspace-build.json` plus `workspace-graph.json` as the top-level workspace build contract. With `--incremental`, unchanged member source-bundle input hashes are skipped when the previous verified build is still valid; rebuilt dependencies force dependent members to rebuild.

## Editor And LSP

`orv editor snapshot <file>` emits first-party editor bootstrap JSON with diagnostics, ProjectGraph, Files/Routes/Schema/Domains panel inputs, and source-hash watch sources. `orv editor reveal <dir> <origin-id>` maps build artifact origins to editor focus/source/production navigation payloads. `orv editor runtime <file>` reuses DAP trace/runtime helpers for runtime inspection pane JSON. `orv editor export <file> --out <dir>` writes `state.json` plus a static `index.html` shell with panel lists, ProjectGraph visualization, runtime frame inspection, DAP adapter launch/live/attach configuration wiring, executable breakpoint source lines, and optional trace navigation. `orv editor trace <dir> --trace <trace.json>` maps captured request frames back to source/production navigation.

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

Runtime debug state comes from the reference runtime debug trace. Long-running `@server` launches start as a paused runtime frame and expose `continue`/`pause` event-loop control. `attachRuntime=true` or the standard `attach` request can attach a child `orv run <program>` process; `attachRuntimeMode="inProcess"` starts an attached server thread in the DAP process. In-process request frames are exposed through variables/evaluate/completions and can be flushed to `orv.production.trace` JSON via `runtimeRequestTracePath`, normal `orv run-artifact/run-build --trace <path>`, or `ORV_RUNTIME_REQUEST_TRACE_PATH`.

## Build And Deploy Artifacts

`orv build <file-or-orv.toml> --out <dir>` creates deterministic reference artifacts:

- `build-manifest.json`
- `bundle-plan.json`
- `origin-map.json`
- `project-graph.json`
- `source-bundle.json`
- `server/app.orv-runtime.json`
- `server/launch.json`
- `pages/index.html` for HTML-only zero-runtime entries
- `client/app.js` and `client/app.wasm` placeholders for interactive client entries

`orv build --prod` adds deploy artifacts:

- `deploy/manifest.json`
- `deploy/routes.json`
- `deploy/container.json`
- `deploy/Dockerfile`
- `deploy/compose.yaml`
- `deploy/README.md`
- `deploy/server.sh`

`source-bundle.json` records source path/content/hash snapshots so reveal, LSP reveal, artifact verification, artifact reanalysis, and reference artifact execution do not depend on the original source files.

## DB Operations

Runtime `@db` currently uses an in-memory execution model with explicit JSON snapshot and WAL APIs: `@db.save/load`, `@db.wal(path)`, `@db.checkpoint()`, `@db.savepoint()`, and `@db.rollback(point)`.

CLI DB commands provide schema/data dry-run, drift verification, apply/migrate with history, rollback, backup/restore, WAL recovery, archive manifest generation, archive restore, and history squash.
