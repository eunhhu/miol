#![allow(clippy::redundant_pub_crate)]

use std::path::Path;

use super::{escape_toml_string, write_new_text_file};
use crate::args::InitTemplate;
const BASIC_INIT_TEMPLATE_SOURCE: &str =
    "@html { @body { @h1 \"Hello from orv\" @p \"Edit src/main.orv\" } }\n";
const SHOP_INIT_TEMPLATE_SOURCE: &str = include_str!("../../../fixtures/e2e/shopping_mall.orv");

pub(super) fn cmd_init(
    dir: &Path,
    name: Option<&str>,
    template: InitTemplate,
) -> anyhow::Result<()> {
    let project_name = name
        .map(str::to_string)
        .or_else(|| {
            dir.file_name()
                .and_then(std::ffi::OsStr::to_str)
                .map(str::to_string)
        })
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "orv-app".to_string());
    let src = dir.join("src");
    std::fs::create_dir_all(&src)
        .map_err(|e| anyhow::anyhow!("failed to create {}: {e}", src.display()))?;
    write_new_text_file(
        &dir.join("orv.toml"),
        &format!(
            "[project]\nname = \"{}\"\nversion = \"0.1.0\"\nentry = \"src/main.orv\"\n",
            escape_toml_string(&project_name)
        ),
    )?;
    let entry_source = match template {
        InitTemplate::Basic => BASIC_INIT_TEMPLATE_SOURCE.to_string(),
        InitTemplate::Shop => shop_init_template_source(),
    };
    write_new_text_file(&src.join("main.orv"), &entry_source)?;
    if template == InitTemplate::Shop {
        write_new_text_file(&dir.join("README.md"), &shop_init_readme(&project_name))?;
    }
    println!("init: {} created", dir.display());
    Ok(())
}

fn shop_init_template_source() -> String {
    SHOP_INIT_TEMPLATE_SOURCE.replace("@listen 0", "@listen 8080")
}

fn shop_init_readme(project_name: &str) -> String {
    let smoke_required_markers = crate::deploy_benchmark::SMOKE_REQUIRED_MARKERS
        .iter()
        .map(|marker| format!("- `{marker}`\n"))
        .collect::<String>();
    format!(
        "# {project_name}\n\
\n\
Generated ORV shop starter.\n\
\n\
## Verify\n\
\n\
```sh\n\
orv check .\n\
orv build . --prod --out dist\n\
orv verify-build dist\n\
orv deploy-env-check dist\n\
orv benchmark-report dist\n\
```\n\
\n\
## Run\n\
\n\
```sh\n\
orv run-build dist\n\
```\n\
\n\
`orv run-build dist` keeps the local reference server in the foreground. Leave it running while you open the browser, or run the generated smoke test from a second terminal:\n\
\n\
```sh\n\
sh dist/deploy/smoke-test.sh\n\
```\n\
\n\
Browser home: http://localhost:8080/ provides product, member signup/login, order, one-step checkout, payment, and shipment forms.\n\
\n\
Theme tokens live in the starter `@design` block (`@colors`, `@spacing`, and `@typography`) and are used by the home page shell, so copy and visual theme edits stay in source instead of generated artifacts.\n\
\n\
Product field edits follow the starter `ProductInput.badge` path: form input, `POST /products` persistence, customer `/catalog`, admin `/admin/catalog`, and generated smoke checks all carry the field end to end.\n\
\n\
Admin dashboard: http://localhost:8080/admin shows catalog/order/payment/shipment/webhook/audit read-model links, operations summary, and persistent storage paths. Admin routes are protected by `@Auth required role=\"admin\"`; the starter seeds a reference admin member `admin` / `admin@example.test` for local smoke sessions.\n\
\n\
Signup stores an Argon2 `passwordHash` through `hash.password`; login uses `hash.verify` and never persists plaintext passwords.\n\
\n\
Successful `POST /members/login` responses set an `orv_session` cookie with `HttpOnly`, `SameSite=Lax`, `Secure`, `Path=/`, and one-day `Max-Age` defaults. When the session has a role, the reference runtime also sets an `orv_session_role` cookie for declarative `@Auth` role checks.\n\
\n\
The account sessions view requires that login cookie through `@session required` and reads the current session with `@session.id`.\n\
\n\
Browser mutation forms include a reference `_csrf` hidden token. Their POST routes use `@csrf`, which requires that token to match the `orv_csrf` cookie minted by HTML GET responses; the Stripe webhook route stays signature-verified as a provider callback.\n\
\n\
Persistent database: `data/shop.sqlite`. The runtime opens this SQLite adapter on startup and stores product, member, order, payment, shipment, webhook, and audit rows in the SQLite file.\n\
\n\
Database adapter override: set `SHOP_DATABASE_URL` before Compose launch to point the generated shop at a different supported DB adapter URL without editing source.\n\
\n\
Commerce records: `data/payments.jsonl`, `data/shipments.jsonl`. The default local payment and shipping adapters append capture and booking records before the DB rows are persisted.\n\
\n\
Commerce adapter overrides: set `PAYMENT_ADAPTER_URL` or `SHIPPING_ADAPTER_URL` before Compose launch to point the generated shop at external HTTP adapter endpoints or provider-mode adapters such as `stripe://local` and `carrier://local` without editing source.\n\
\n\
Provider-mode deploy artifacts expose endpoint and credential env placeholders such as `STRIPE_API_ENDPOINT`, `STRIPE_SECRET_KEY`, `STRIPE_WEBHOOK_SECRET`, `STRIPE_WEBHOOK_SECRET_PREVIOUS`, `CARRIER_API_ENDPOINT`, `CARRIER_API_KEY`, and `CARRIER_WEBHOOK_SECRET`. When `STRIPE_API_ENDPOINT` or `CARRIER_API_ENDPOINT` is configured, provider-mode capture/booking calls POST checked JSON to that endpoint with bearer credentials, stable idempotency keys, and bounded transient retry, then merges the provider JSON response without exposing secret values. The shop also exposes `POST /webhooks/stripe` for reference Stripe webhook signature verification with primary/previous webhook secret rotation, duplicate event handling, and Payment/Order status reconciliation from webhook payloads; production provider SDK hardening remains future work.\n\
\n\
Compose mounts `data/` into `/app/data`, so the generated production container keeps the shop database and commerce record logs outside the container layer.\n\
\n\
Back up `data/shop.sqlite` and commerce record logs with the mounted `data/` volume before deploy or backup rotation.\n\
\n\
## Deploy\n\
\n\
After `orv build . --prod --out dist`, use generated deploy runbook:\n\
\n\
```sh\n\
cd dist\n\
PORT=8080 docker compose -f deploy/compose.yaml up --build -d\n\
./deploy/smoke-test.sh\n\
orv benchmark-report .\n\
```\n\
\n\
The generated `deploy/benchmark-evidence.json` template records the 5-hour shop benchmark tasks and data-to-record fields against the same preflight hash that `orv verify-build` checks. After a human run, fill the evidence file and run `orv benchmark-report dist --require-pass` from the project root, or `orv benchmark-report . --require-pass` from `dist`.\n\
\n\
The generated smoke test writes `deploy/smoke-output.txt`. Benchmark reports require these smoke-output markers:\n\
\n\
{smoke_required_markers}\n\
\n\
## Native Launcher\n\
\n\
```sh\n\
cargo build --manifest-path dist/server/native/Cargo.toml --release\n\
ORV_BUILD_DIR=dist ./dist/server/native/target/release/orv-native-server\n\
```\n\
\n\
The generated launcher path can infer `dist`; `ORV_BUILD_DIR` is an explicit override.\n\
\n\
## Native Runtime Image\n\
\n\
```sh\n\
docker build -f dist/server/native/Dockerfile -t orv-native-server:latest dist\n\
```\n\
\n\
## Deploy artifacts\n\
\n\
- `deploy/manifest.json`\n\
- `deploy/container.json`\n\
- `deploy/Dockerfile`\n\
- `deploy/compose.yaml`\n\
- `deploy/env.example`\n\
- `deploy/db-adapters.json`\n\
- `deploy/commerce-adapters.json`\n\
- `deploy/preflight.json`\n\
- `deploy/benchmark-evidence.json`\n\
- `deploy/smoke-test.sh`\n\
- `deploy/smoke-output.txt`\n\
- `deploy/README.md`\n\
- `deploy/routes.json`\n\
- `deploy/server.sh`\n\
- `server/native-server.json`\n\
- `server/runtime-image.json`\n\
- `server/native/Dockerfile`\n\
- `server/native/Cargo.toml`\n\
- `server/native/main.rs`\n\
- `server/native/routes.rs`\n\
- `server/native/router.rs`\n\
- `server/native/handlers.rs`\n\
\n\
## Routes\n\
\n\
- `GET /`\n\
- `GET /catalog`\n\
- `GET /cart`\n\
- `GET /account/sessions`\n\
- `GET /admin`\n\
- `GET /admin/catalog`\n\
- `GET /admin/summary`\n\
- `GET /admin/orders`\n\
- `GET /admin/payments`\n\
- `GET /admin/shipments`\n\
- `GET /admin/webhooks`\n\
- `GET /admin/audit`\n\
- `GET /health`\n\
- `POST /products`\n\
- `GET /products`\n\
- `GET /products/:sku`\n\
- `POST /members`\n\
- `POST /members/login`\n\
- `GET /members/:handle`\n\
- `POST /orders`\n\
- `GET /orders/:customer`\n\
- `POST /checkout`\n\
- `POST /cart/items`\n\
- `POST /payments`\n\
- `POST /webhooks/stripe`\n\
- `POST /shipments`\n\
- `GET /shipments/:orderId`\n"
    )
}
