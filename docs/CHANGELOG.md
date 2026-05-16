# orv Changelog

Implementation deltas live here, not in [SPEC.md](SPEC.md). Keep entries factual and dated.

## 2026-05-16

- Added a reference HTTP bridge for PostgreSQL/MySQL `@db.connect` handles: configured `ORV_DB_ADAPTER_POSTGRES_ENDPOINT`, `ORV_DB_ADAPTER_MYSQL_ENDPOINT`, or `ORV_DB_ADAPTER_ENDPOINT` values turn external DB handles from explicit unsupported status into checked `http-json-v1` POST adapter calls, with optional bearer tokens from provider-specific or generic DB adapter auth envs.
- Surfaced PostgreSQL/MySQL DB bridge request shape and provider-specific endpoint/auth env knobs in `deploy/db-adapters.json`, generated Compose/env.example, preflight envs, and the deploy runbook; production deploy env checks now require the provider-specific bridge endpoint before launch while keeping bridge auth tokens optional.
- Strengthened generated production shop smoke tests so checkout/admin validation captures response bodies and checks checkout status, payment capture, shipment tracking, customer catalog/cart/session read models, and admin catalog/order/payment/shipment/audit read models.
- Added generated production smoke checks for `x-orv-origin-id` route headers so deployed route reachability also proves the ProjectGraph/HIR origin contract is exposed at runtime.
- Made `orv run-build <dir>` execute relative DB/WAL, `@serve`, `@fs`, and file-backed commerce adapter paths against the build directory so local deploy smoke runs do not leak persistence files into the caller's shell cwd.
- Strengthened `orv verify-build` so server route/listen/response origin ids must resolve through `origin-map.json` and server/deploy source snapshots must match `source-bundle.json`.
- Added `project-graph.json` verification for source-bundle file nodes, semantic origin-map mirrors, semantic origin edges, and origin-link drift.
- Made generated deploy smoke tests compare each `x-orv-origin-id` header against the exact route origin id from the server artifact instead of accepting any `ori_` value.
- Made DAP `setInstructionBreakpoints` verify `orv:frame:N` pseudo-instruction references after launch and stop `continue` on matching runtime frames.
- Exposed CSRF, session cookie, auth role, and default route rate-limit requirements as shared `runtime_features` across build, server, deploy, and native plan artifacts.
- Added explicit reference `@rateLimit key=... limit=... window=...` route policies plus `@rateLimit exempt`, with runtime enforcement, server artifact descriptors, and native route table fields.
- Added source-backed `@csrf exempt` so intentional CSRF bypasses can execute without a token while still appearing in route policy artifacts.
- Added generated `deploy/preflight.json` so verify-build, deploy-env-check, run-build, smoke-test, runtime features, security features, persistence, env requirements, and linked deploy artifacts share one checked preflight contract.
- Exposed the checked deploy preflight contract through reveal/editor/LSP production payloads and the native editor production panel.
- Added per-route security policy descriptors for source-backed auth/session/csrf domains and built-in rate-limit defaults, with verify-build origin containment checks and reveal production payload exposure.
- Surfaced preflight route-policy counts and kind summaries in editor export/native-host production payloads and the generated production panel.
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
