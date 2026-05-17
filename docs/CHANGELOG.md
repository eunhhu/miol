# orv Changelog

Implementation deltas live here, not in [SPEC.md](SPEC.md). Keep entries factual and dated.

## 2026-05-18

- Fixed generated deploy smoke DAP source-bundle count checks to use the actual build source-bundle file count, so imported multi-file projects do not fail against a one-file expectation.
- Fixed generated deploy smoke DAP native route summary checks to use the actual server route count, so shop-scale builds with many routes do not fail against a one-route expectation.

## 2026-05-17

- Aligned `clippy.toml` with the workspace Rust MSRV and replaced the remaining newer API use in DAP hit conditions so `cargo clippy -- -D warnings` is warning-free on the declared toolchain floor.
- Added a checked `smoke_output_contract` to generated preflight and benchmark evidence artifacts so `deploy/preflight.json`, benchmark reports, reveal surfaces, and runbooks share the same required marker list.
- Added generated deploy smoke gates for the smoke-output required-marker contract across CLI/editor/LSP reveal payloads and DAP production context, and cached the DAP run-debug output so smoke does not rerun it for every grep.
- Added the generated smoke-output artifact and required marker list to the shop starter README so the starter guide matches generated deploy runbooks.
- Added the required smoke-output marker list to the generated deploy runbook and made verify-build reject runbook drift from the benchmark smoke marker contract.
- Mirrored benchmark smoke required-marker contracts into reveal/editor/native production preflight payloads.
- Split the large CLI and compiler implementation files into focused modules while keeping public command/artifact behavior unchanged.
- Made generated benchmark evidence record the required smoke-output marker list, including `dap_source_bundle`, and made `orv verify-build` reject evidence drift from that marker contract.
- Exposed the same required smoke-output marker list in benchmark report data and parsed smoke summaries so reveal/editor consumers can see the expected smoke contract alongside missing markers.

## 2026-05-16

- Added a `dap_source_bundle=verified` marker to generated smoke output and benchmark-report parsing so source-bundled DAP panel coverage is recorded as benchmark evidence.
- Added generated deploy smoke and verify-build gates for `panels.debug.source_bundle` path/file-count/hash metadata from source-bundled DAP runs.
- Mirrored raw DAP source-bundle launch metadata into editor run-debug result JSON, `panels.debug.source_bundle`, session summaries, and the rendered debug result panel.
- Exposed DAP source-bundle launch metadata in raw launch/restart responses and made `restart` preserve the previous build `source-bundle.json` path when no program override is supplied.
- Mirrored the DAP production-summary gate into generated deploy preflight, benchmark evidence, and runbook commands as `orv editor run-debug . --control next`, with verify-build drift checks.
- Advertised raw DAP `sourceBundle` launch support and added a direct DAP regression that launches from build `source-bundle.json` after the original source file is removed.
- Added a `dap_summary=verified` marker to generated `deploy/smoke-output.txt` and `orv benchmark-report` parsing, so benchmark evidence records whether the source-bundled DAP production-summary gate passed.
- Added generated deploy smoke DAP gates: smoke tests now run `orv editor run-debug . --control next` from the build dir and assert graph/source-bundle, native, and client production summary counters.
- Let `orv editor run-debug <build-dir>` synthesize a DAP runner from `source-bundle.json`, so build-backed debug sessions can run and render production summaries even after the original source file is unavailable.
- Added client and static positive gates for `panels.debug.production_summary`, so DAP runner result tests now cover native, client bundle, and zero-runtime static production counters.
- Split build-backed DAP runner production context into a checked `panels.debug.production_summary` section and rendered debug-result metrics, so native/static/client/smoke summary counters stay visible in `orv editor run-debug` outputs.
- Extended generated deploy smoke tests so client-bundle builds assert CLI/editor/LSP client-origin reveal payloads carry client target, manifest, and capability summary counters.
- Tightened static production verification so `deploy/manifest.json` static targets must match the bundle-plan `static_page` target, and LSP/editor reveal tests now assert static summary counters.
- Extended generated deploy smoke tests so route-origin CLI/editor/LSP reveal payloads must carry native-server target and route summary counters.
- Added native-server and static-page production target summaries to reveal/editor/native-host production payloads, including native route/blocker counts, static verification counts, and Production panel sections.
- Threaded build-backed production graph and summary context into editor debug metadata, standalone DAP runner artifacts, native-host debug metadata, run-debug results, and debug result panels.
- Added graph-contract and production summary counters to `orv reveal`, `orv editor reveal`, and `orv lsp reveal` production payloads, and made generated deploy smoke checks assert the smoke-evidence summary counter is present across all three reveal surfaces.
- Added state/native-host/editor Production panel counters for preflight smoke evidence summaries, including present, missing, and missing-marker gap counts.
- Added reference-runtime `x-orv-response-origin-id` headers and request trace `response_origin_id` fields for executed `@respond` nodes, and wired editor/native-host trace payloads to expose separate response reveal navigation alongside route navigation.
- Extended generated deploy smoke tests to verify exact `x-orv-response-origin-id` headers for covered routes with one unambiguous response origin, and made verify-build reject response-origin smoke drift.
- Linked `@html` projection origins back to static page/client bundle artifacts and route-local HTML origins back to their containing route/native-server production targets in reveal/editor payloads.
- Linked generated DB adapter contracts back to the source `@db.connect` origin through `source_origin_id`, and made reveal production payloads expose `matched_adapters` for the selected origin.
- Linked generated commerce adapter contracts back to source `@payment.connect` and `@shipping.connect` origins through `source_origin_id`, with matching reveal `matched_adapters` payloads.
- Strengthened `orv verify-build` so DB and commerce adapter `source_origin_id(s)` must resolve to the expected connect call entries in `origin-map.json`.
- Added a reference HTTP bridge for PostgreSQL/MySQL `@db.connect` handles: configured `ORV_DB_ADAPTER_POSTGRES_ENDPOINT`, `ORV_DB_ADAPTER_MYSQL_ENDPOINT`, or `ORV_DB_ADAPTER_ENDPOINT` values turn external DB handles from explicit unsupported status into checked `http-json-v1` POST adapter calls, with optional bearer tokens from provider-specific or generic DB adapter auth envs.
- Made `@design` token lookup work inside HTML render attributes and added editable color/spacing/typography tokens to the shop starter home shell.
- Added an end-to-end editable product field path to the shop starter: `ProductInput.badge` now flows through the product form, `POST /products`, customer catalog, admin catalog, and generated smoke-test body checks.
- Surfaced PostgreSQL/MySQL DB bridge request shape, bounded transient retry policy, and provider-specific endpoint/auth env knobs in `deploy/db-adapters.json`, generated Compose/env.example, preflight envs, and the deploy runbook; production deploy env checks now require the provider-specific bridge endpoint before launch while keeping bridge auth tokens optional.
- Aligned deploy preflight and smoke tests with the runtime DB bridge fallback envs, so generic `ORV_DB_ADAPTER_ENDPOINT` and `ORV_DB_ADAPTER_AUTH_TOKEN` can satisfy shared bridge deployments when provider-specific values are unset.
- Extended generated deploy smoke tests so external DB bridge builds check `deploy/db-adapters.json` and POST a safe `schema` probe to each configured provider bridge endpoint.
- Strengthened generated production shop smoke tests so checkout/admin validation captures response bodies and checks checkout status, payment capture, shipment tracking, customer catalog/cart/session read models, and admin catalog/order/payment/shipment/audit read models.
- Added generated production smoke checks for `x-orv-origin-id` route headers so deployed route reachability also proves the ProjectGraph/HIR origin contract is exposed at runtime.
- Made `orv run-build <dir>` execute relative DB/WAL, `@serve`, `@fs`, and file-backed commerce adapter paths against the build directory so local deploy smoke runs do not leak persistence files into the caller's shell cwd.
- Strengthened `orv verify-build` so server route/listen/response origin ids must resolve through `origin-map.json` and server/deploy source snapshots must match `source-bundle.json`.
- Added `project-graph.json` verification for source-bundle file nodes, semantic origin-map mirrors, semantic origin edges, and origin-link drift.
- Made generated deploy smoke tests compare each `x-orv-origin-id` header against the exact route origin id from the server artifact instead of accepting any `ori_` value.
- Made DAP `setInstructionBreakpoints` verify `orv:frame:N` pseudo-instruction references after launch and stop `continue` on matching runtime frames.
- Surfaced DAP `loadedSources`/`source` request inventory and launch-time source snapshot responses through editor export, native-host debug metadata, and run-debug result panels, including imported source SHA256 checksums.
- Mirrored build graph contracts (`source-bundle.json`, `project-graph.json`, and `origin-map.json`) into editor production export/native-host/panel payloads with artifact hashes and source/origin counts.
- Made generated deploy smoke tests gate on the same build graph spine by checking `source-bundle.json`, `project-graph.json`, `origin-map.json`, and running `orv verify-build .` before live route checks.
- Mirrored graph artifacts into `deploy/preflight.json` so preflight, smoke, runbook, and verify-build all name the same source-bundle/project-graph/origin-map contract paths.
- Added a trace-enabled `orv run-build . --trace deploy/request-trace.json` preflight command and clearer trace-smoke failure guidance, aligning generated smoke with the runbook trace capture flow.
- Mirrored the 5-hour shop benchmark contract into `deploy/preflight.json`, including automated gate commands, success criteria, time budget, and data-to-record fields, with verify-build drift checks.
- Added checked `deploy/benchmark-evidence.json` generation so benchmark timing and observation records carry the same 5-hour shop contract, preflight hash, command list, linked artifacts, task budget, and data-to-record schema that `orv verify-build` validates.
- Added `orv benchmark-report <dir> [--require-pass]` to summarize recorded benchmark evidence as pass/fail/incomplete JSON and optionally fail CI when the human-run evidence is incomplete or over the 5-hour budget.
- Mirrored `orv benchmark-report .` and `orv benchmark-report . --require-pass` into generated deploy preflight/runbook contracts so benchmark reporting is a checked deploy gate instead of a standalone command.
- Added benchmark evidence report-status and missing-evidence counters to reveal/editor/native production preflight payloads, reusing the same pass/fail/incomplete calculation as `orv benchmark-report`.
- Added generated `deploy/smoke-output.txt` capture on successful smoke runs and let `orv benchmark-report` use it when benchmark evidence has not copied smoke output yet.
- Strengthened generated production shop smoke tests to fetch the admin dashboard and webhook read-model page, checking dashboard links/storage paths plus webhook/audit summary fields.
- Exposed CSRF, session cookie, auth role, and default route rate-limit requirements as shared `runtime_features` across build, server, deploy, and native plan artifacts.
- Added explicit reference `@rateLimit key=... limit=... window=...` route policies plus `@rateLimit exempt`, with runtime enforcement, server artifact descriptors, and native route table fields.
- Added source-backed `@csrf exempt` so intentional CSRF bypasses can execute without a token while still appearing in route policy artifacts.
- Added generated `deploy/preflight.json` so verify-build, deploy-env-check, run-build, smoke-test, runtime features, security features, persistence, env requirements, and linked deploy artifacts share one checked preflight contract.
- Exposed the checked deploy preflight contract through reveal/editor/LSP production payloads and the native editor production panel.
- Added per-route security policy descriptors for source-backed auth/session/csrf domains and built-in rate-limit defaults, with verify-build origin containment checks and reveal production payload exposure.
- Surfaced preflight route-policy counts and kind summaries in editor export/native-host production payloads and the generated production panel.
- Surfaced preflight command counts and checked benchmark-report commands through reveal/editor/native production payload tests and production panel summaries.
- Mirrored route security policy descriptors into generated native route table source so native artifacts carry the same source-backed policy contract.

## 2026-05-06

- Added shop scaffold coverage for persisted catalog, cart, member sessions, checkout, admin read models, payment records, shipment records, and webhook records.
- Added reference Stripe-style webhook verification with primary/previous secret handling, HMAC-SHA256 signature checks, duplicate event handling, and payment/order reconciliation hooks.
- Added DB archive upload/restore contracts for local file, HTTP, and S3-compatible targets, including hash/byte verification and bounded transient retries.
- Added DB crash-matrix verification for WAL replay, torn EOF recovery, corruption rejection, checkpoint replay, savepoint rollback, PITR cutoff, and archive hash mismatch.
- Added build/deploy artifacts for native server plan/source contracts, runtime image plan, generated Compose/runbook/env.example, DB adapter manifest, commerce adapter manifest, and smoke-test script.
- Added client bundle artifacts for static page, reactive plan, JS loader, WASM bootstrap, manifest capability inventory, blocker metadata, and verify-build checks.
- Expanded LSP/DAP bootstrap with source checksums, paging for stack/local windows, guarded request-domain references, debug runner commands, native-host export metadata, and trace transport payloads.

## Policy

- Date-stamped implementation notes go here.
- State/contract/crate/test tables go in [IMPLEMENTATION_MATRIX.md](IMPLEMENTATION_MATRIX.md).
- Future work goes in [ROADMAP.md](ROADMAP.md).
- Stable language behavior goes in [SPEC.md](SPEC.md).
