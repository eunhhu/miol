# 5-Hour Shop Benchmark

This benchmark is the product test for orv's north star: a non-developer can build and deploy a small shop without AI assistance in under 5 hours.

## Participant

- HTML/CSS/JS experience: 0 to 1 year.
- No professional backend, DB, deployment, or payment integration experience.
- Can read official orv docs and use built-in editor/help.
- Cannot use Copilot, Cursor, ChatGPT, or other AI assistance during the run.

## Starting Point

```bash
orv init my-shop --template shop
cd my-shop
orv dev
```

The primary benchmark uses local reference adapters:

- SQLite-backed shop DB via `SHOP_DATABASE_URL` default.
- Mock/local payment capture.
- Mock/local shipping booking.
- Local deploy/preflight artifacts and generated smoke-test.

Provider-backed Stripe/carrier runs are separate advanced variants.

## Acceptance Before Human Runs

Before recruiting participants, the generated shop template must pass an automated template-to-running-shop smoke path:

```bash
orv init my-shop --template shop
cd my-shop
orv check .
orv build . --prod --out dist
orv verify-build dist
orv deploy-env-check dist
orv run-build dist
sh dist/deploy/smoke-test.sh
orv benchmark-report dist
```

`orv run-build dist` keeps the reference server in the foreground. Keep that command running and execute the generated smoke test from a second terminal, or use the generated Docker Compose runbook for a detached server.

This gate proves the implementation path first. Human 5-hour runs then measure authoring UX, not whether the scaffold can boot.

Production builds mirror this benchmark contract into `deploy/preflight.json` under `benchmark`, and the checked preflight command list includes both `orv benchmark-report .` and `orv benchmark-report . --require-pass`. They also emit `deploy/benchmark-evidence.json`, a checked evidence template keyed to the same preflight hash. The evidence artifact carries the automated gate, success criteria, time budget, and data-to-record fields so benchmark reports stay tied to the same deploy preflight that `orv verify-build` checks.

After a human run, fill the recorded fields in `deploy/benchmark-evidence.json` and run `orv benchmark-report dist --require-pass` to turn elapsed task time, required observation data, and the 5-hour limit into a checked JSON report.

## Success Criteria

The participant must finish all items:

- edit the home page copy and theme tokens
- create 3 products
- add one product field and show it in catalog/admin
- sign up and log in as a member
- add an item to cart
- complete checkout
- capture mock payment
- book mock shipping
- view order/payment/shipment rows in admin
- run prod build
- pass deploy env check
- pass generated smoke-test
- reveal route/html/db-related execution output back to source through origin artifacts

## Failure Criteria

The run fails if:

- total elapsed time exceeds 5 hours
- AI assistance is used
- checkout cannot create an order, payment record, and shipment record
- smoke-test fails
- the participant must edit generated runtime/build artifacts by hand
- a required security step is manual and undocumented

## Time Budget

| Task | Target |
|------|--------|
| Project creation and first run | 15 min |
| First page/theme edit | 30 min |
| Product data entry | 30 min |
| Product field addition | 45 min |
| Form validation update | 45 min |
| Auth/member flow check | 30 min |
| Checkout/payment/shipping config | 60 min |
| Admin verification | 30 min |
| Prod build and env check | 30 min |
| Smoke-test and issue fixing | 45 min |

## Data To Record

- elapsed time per task
- number of docs/help lookups
- number of compiler/runtime errors
- time from first error to fix
- all manual config edits
- smoke-test output
- participant notes on confusing concepts

## Design Feedback Loop

Any step that repeatedly exceeds its time budget should produce one of:

- simpler App Authoring syntax
- better scaffold defaults
- better error message
- editor affordance
- documentation change
- removal from MVP scope
