# API Conventions

How every endpoint behaves. Phase 0 implements each pattern once (see the named file), and later phases reuse it rather than inventing another.

Related: [ARCHITECTURE.md §5](../ARCHITECTURE.md), [data-model.md](data-model.md).

## Which protocol

| Use | Protocol | Where |
|---|---|---|
| Any change of state (create, update, cancel, check-in, post charge …) | **REST** `POST/PUT/PATCH/DELETE /api/v1/...` | `crates/core-api/src/routes/` |
| Reading data for a screen, especially lists, grids and anything with relations | **GraphQL** `POST /graphql` (queries only; there are no mutations) | `crates/core-api/src/graphql.rs` |
| Telling open screens what changed | **SSE** `GET /api/v1/events[?property=<id>]` | `crates/core-api/src/events.rs` |
| Service to service (media, channels, jobs) | **gRPC** (tonic), from Phase 6 | `proto/` |
| Inbound webhooks (payment gateways, Channex) | REST under `/webhooks/<provider>`, outside the CSRF layer, signature-verified | Phase 7–8 |

Simple single-object reads that bootstrap the app (`GET /api/v1/me`) may be REST.

## Authentication and tenancy

- Session cookie `gf_session`: opaque random token, `HttpOnly; SameSite=Lax; Path=/`, plus `Secure` in production, 14 h lifetime. The server stores only its SHA-256.
- Extractors (`crates/core-api/src/auth.rs`):
  - `Authenticated`: any signed-in user (401 otherwise).
  - `TenantContext`: signed-in user with a selected tenant, grants loaded (403 `no tenant selected` otherwise). Use this for every tenant-scoped endpoint.
  - Both are resolved once per request and cached in request extensions.
- Authorisation: `ctx.require(Permission::X, Some(property_id))?` at the top of every handler. Add new actions to `identity::Permission` and to each role's `permits`, with a test.
- Data access: `db::begin(&state.pool, Scope::tenant(ctx.tenant))` and never anything else. RLS is the safety net, but queries still filter by `property_id` explicitly.

## CSRF

Every non-GET/HEAD/OPTIONS request must send `x-goodfolk-csrf: 1` (`crates/core-api/src/csrf.rs`). The SPA's clients add it automatically. No CORS is configured, so cross-site pages cannot send it. Webhooks are mounted outside this layer.

## Errors: RFC 9457 problem details

```json
{ "type": "about:blank", "title": "Conflict", "status": 409, "detail": "a property with this code already exists" }
```

`Content-Type: application/problem+json`. Construct errors with `ApiError::{bad_request, unauthenticated, invalid_credentials, forbidden, conflict, unprocessable, internal}`. Database errors are logged and become a bare 500, and internal details never reach the client.

| Status | When |
|---|---|
| 400 | Malformed request, missing `Idempotency-Key` |
| 401 | No or expired session; bad credentials |
| 403 | Missing permission, no tenant selected, missing CSRF header |
| 404 | Not found **or not visible to this tenant** (never reveal existence) |
| 409 | Uniqueness conflict, double booking (exclusion violation), idempotent request still running |
| 412 | `If-Match` version mismatch (from Phase 1) |
| 422 | Validation failed (`garde`), business rule violated, idempotency key reused for a different request |
| 504 | Handler exceeded the 15 s request timeout |

## Validation

Request DTOs derive `garde::Validate` and are checked with `error::validate(&body)?` before any work. Rules that need the database (unknown time zone, overlapping block) are enforced in the module and returned as typed errors, which handlers map to 409 or 422.

## Idempotency (every create command)

- The client sends `Idempotency-Key: <8–200 chars>`, one fresh UUID per user action, reused only when retrying that same action.
- Mount create routes in the `commands` router, which has `.route_layer(from_fn_with_state(state, idempotency::idempotent))`.
- Same key, same request: the stored first response is replayed. Same key, different request: 422. First request still running: 409. A 5xx response is not stored, so the client may retry.

## Optimistic concurrency (from Phase 1)

- Editable resources return `version` in their body and an `ETag: "<version>"` header.
- Updates must send `If-Match: "<version>"`. The SQL `update … where id = $1 and version = $2` returns no row on mismatch, which maps to 412, and the client refetches and shows what changed.

## Change events

After a successful write, in the same transaction, call `db::notify(&mut tx, &Event { tenant_id, property_id, keys })`. Keys name **TanStack Query keys** the client should invalidate (`"properties"`, `"reservations"`, `"tape:<property>:<tileStart>"` …). Events never carry data. Choose keys so that a change refetches only screens that show it.

## GraphQL

- Schema in `crates/core-api/src/graphql.rs`: depth ≤ 8, complexity ≤ 500. Introspection returns `null` in production. Persisted-query allowlist from Phase 4.
- Resolvers read `PgPool` and `TenantContext` from the request context and batch relations with DataLoaders (from Phase 1, the first nested relation).
- Fields are camelCase; IDs are `UUID`; money is `{ amount: Int (minor units, as string if > 2^53), currency }`; dates are ISO `YYYY-MM-DD`.
- Lists use cursor pagination (`first`, `after` → `{ nodes, pageInfo { endCursor, hasNextPage } }`) once they can exceed a few hundred rows (reservations, guests).

## REST shapes

- JSON bodies with snake_case fields (as the DTOs serialize). Paths are plural nouns, with actions as sub-resources (`POST /api/v1/reservations/{id}/check-in`).
- Create returns 201 and the created resource. Commands with no useful body return 204.
- Every handler carries `#[utoipa::path]` and is listed in `crates/core-api/src/openapi.rs`. `tests/openapi.rs` pins the route list, so update it with each new route.

## Frontend contract

- `bun run api:schemas && bun run codegen` regenerates `web/pms/src/lib/api/{openapi.json,openapi.d.ts,schema.graphql,gql/}`. Commit the result; CI fails if it is stale.
- REST calls use `rest` (openapi-fetch), GraphQL uses `query(document)` with documents declared via `graphql(\`…\`)` in `src/lib/**/*.ts` (not in components).
- Query keys match event keys. `connectEvents` applies `invalidate` and `resync`.

## Versioning

REST is under `/api/v1`. A breaking change gets `/api/v2` for the affected routes only. GraphQL evolves additively: fields are deprecated with `#[graphql(deprecation = "...")]` and removed after the SPA stops using them.
