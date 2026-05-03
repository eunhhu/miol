# orv Operational Surfaces

이 문서는 현재 CLI, editor/LSP/DAP, build/deploy, DB 운영 surface를 추적한다. 핵심 크레이트 구조와 데이터 흐름은 [ARCHITECTURE.md](ARCHITECTURE.md)에 둔다.

## CLI Surface

현재 `orv-cli`는 프로젝트 scaffold, 로드/해석/분석/실행, graph/origin 출력, editor/LSP/DAP bootstrap, build artifact 생성/검증/실행, DB snapshot/migration workflow를 오케스트레이션한다.

주요 command:

- `orv init <dir> --name <name> [--template basic|shop]`
- `orv run/dev/check/dump/origins/graph/test`
- `orv editor snapshot/reveal/runtime/export/trace`
- `orv lsp snapshot/reveal/serve --stdio`
- `orv dap serve --stdio`
- `orv build <file-or-orv.toml> --out <dir> [--prod]`
- `orv verify-build/verify-artifact/check-artifact/check-build`
- `orv run-artifact/run-build/reveal`
- `orv db plan/verify/apply/migrate/rollback/backup/restore/recover/archive/squash`

Source-entry commands accept a single `.orv` file, an `orv.toml` with `[project].entry`, or a project directory containing `orv.toml`.

## Editor And LSP

`orv editor snapshot <file>` emits first-party editor bootstrap JSON with diagnostics, ProjectGraph, Files/Routes/Schema/Domains panel inputs, and source-hash watch sources. `orv editor reveal <dir> <origin-id>` maps build artifact origins to editor focus/source/production navigation payloads. `orv editor runtime <file>` reuses DAP trace/runtime helpers for runtime inspection pane JSON. `orv editor export <file> --out <dir>` writes `state.json` plus a static `index.html` shell with panel lists, runtime frame inspection, and optional trace navigation. `orv editor trace <dir> --trace <trace.json>` maps captured request frames back to source/production navigation.

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
