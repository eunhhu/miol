# orv Changelog

Implementation deltas live here, not in [SPEC.md](SPEC.md). Keep entries factual and dated.

## 2026-05-16

- Strengthened generated production shop smoke tests so checkout/admin validation captures response bodies and checks checkout status, payment capture, shipment tracking, customer catalog/cart/session read models, and admin catalog/order/payment/shipment/audit read models.

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
