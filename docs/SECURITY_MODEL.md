# orv Security Model

Security is a default scaffold behavior, not an optional library checklist. The App Authoring surface should let a beginner build a shop without manually handling bearer token slicing, cookie flags, CSRF details, webhook replay logic, or payment idempotency.

## Safe Defaults

| Area | Default expectation |
|------|---------------------|
| Sessions | HttpOnly, Secure in production, SameSite=Lax or Strict, rotation after login |
| Passwords | `hash.password` with approved parameters; no plaintext storage |
| CSRF | state-changing browser routes require CSRF token unless explicitly exempted |
| XSS | HTML text escapes by default; raw HTML requires an explicit unsafe escape hatch |
| Authz | admin routes require declarative role/policy checks |
| Rate limits | auth, checkout, webhook, and password reset routes get scaffolded limits |
| Secrets | `vault.get`/env contracts never expose values in runtime responses or artifacts |
| Webhooks | signature verification, timestamp tolerance, replay/idempotency key storage |
| Payments | stable idempotency keys per order/payment attempt |
| Audit | login, checkout, payment, shipping, admin mutation, and webhook events logged |
| Errors | route errors become safe 4xx/5xx responses without leaking secrets |

## App Authoring Surface

Beginner-facing code should prefer declarative security domains:

```orv
@route POST /checkout {
  @session required
  @csrf
  @rateLimit key=@session.userId limit=10 window=1m
  @CheckoutPolicy
  @body: CheckoutForm

  @checkout.capture
}

@route GET /admin/orders {
  @Auth required role="admin"
  @respond 200 await @db.find Order
}
```

Lower-level primitives such as `jwt.verify`, `hash.password`, `crypto.hmac`, and `vault.get` remain available for Systems Surface code, but scaffolds should not force beginners to wire them by hand.

## Shop Scaffold Requirements

The shop template should provide:

- protected admin routes
- member session cookie defaults
- signup/login password hashing
- checkout CSRF and rate limit hooks
- payment idempotency keys
- Stripe webhook signature/replay protection in provider mode
- audit records for checkout, payment, shipping, and admin mutations
- deploy env checks for required provider secrets

## Implementation Tracking

This file defines security expectations. Exact implementation/contract status is tracked in [IMPLEMENTATION_MATRIX.md](IMPLEMENTATION_MATRIX.md).
