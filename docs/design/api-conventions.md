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
- Authorisation: `ctx.require(Permission::X, Some(property_id))?` at the top of every handler. Add new actions to `identity::Permission` and to each role's `permits`, with a test (`modules/identity/tests/rbac.rs` pins the whole matrix). A grant with a role this build does not know is skipped with a warning.

  | Permission | Owner | Manager | Front desk | Housekeeping | Accountant |
  |---|---|---|---|---|---|
  | `PropertiesView`, `RoomsView`, `InventoryView` | ✓ | ✓ | ✓ | ✓ | ✓ |
  | `PropertiesCreate` | ✓ | | | | |
  | `PropertiesManage` (property settings), `RoomsManage` (room types, rooms, sections, block reasons) | ✓ | ✓ | | | |
  | `InventoryBlock` (block and release rooms) | ✓ | ✓ | ✓ | | |
- Data access: `db::begin(&state.pool, Scope::tenant(ctx.tenant))` and never anything else. RLS is the safety net, but queries still filter by `property_id` explicitly.

## CSRF

Every non-GET/HEAD/OPTIONS request must send `x-goodfolk-csrf: 1` (`crates/core-api/src/csrf.rs`). The SPA's clients add it automatically. No CORS is configured, so cross-site pages cannot send it. Webhooks are mounted outside this layer.

## Errors: RFC 9457 problem details

```json
{ "type": "about:blank", "title": "Conflict", "status": 409, "detail": "a property with this code already exists" }
```

`Content-Type: application/problem+json`, for every error the API returns, including malformed bodies and query strings, GraphQL request parse failures and timeouts. Construct errors with `ApiError::{bad_request, unauthenticated, invalid_credentials, forbidden, not_found, method_not_allowed, conflict, precondition_failed, precondition_required, unprocessable, too_many_requests, gateway_timeout, internal}`. Database errors are logged and become a bare 500, and internal details never reach the client. A problem may carry extension members added with `ApiError::with(name, value)`: a 409 for an overlapping room block lists the blocks in the way as `conflicts`.

| Status | When |
|---|---|
| 400 | Malformed request (invalid JSON, wrong content type, unparseable query string), missing `Idempotency-Key` |
| 401 | No or expired session; bad credentials |
| 403 | Missing permission, no tenant selected, missing CSRF header |
| 404 | Not found **or not visible to this tenant** (never reveal existence), or no such route (the router's fallback) |
| 405 | The route exists but not for this method (the router's method-not-allowed fallback) |
| 409 | Uniqueness conflict, double booking (exclusion violation), idempotent request still running |
| 412 | `If-Match` version mismatch |
| 422 | Body of the wrong shape (missing or mistyped field), validation failed (`garde`), business rule violated, idempotency key reused for a different request |
| 428 | An update without `If-Match` |
| 429 | Sign-in throttled: 5 failed attempts for one email within 15 minutes (the same answer whether or not the account exists) |
| 504 | Handler exceeded the 15 s request timeout (`routes::REQUEST_TIMEOUT`) |

## Validation

Request DTOs derive `garde::Validate` and are checked with `error::validate(&body)?` before any work. Rules that need the database (unknown time zone, overlapping block) are enforced in the module and returned as typed errors, which handlers map to 409 or 422.

## Idempotency (every create command)

- The client sends `Idempotency-Key: <8–200 chars>`, one fresh UUID per user action, reused only when retrying that same action.
- Mount create routes in the `commands` router, which has `.route_layer(from_fn_with_state(state, idempotency::idempotent))`.
- Same key, same request: the stored first response is replayed. Same key, different request: 422. The request hash covers the user, method, path, query string and body, so a different user or query with the same key is a different request.
- First request still running: 409, and the client retries. A claim is abandoned once it has been unfinished for 60 s (`routes::ABANDONED_CLAIM_AFTER`, the 15 s timeout plus margin), for example after a timeout or a dropped connection, and the next request with that key takes it over and runs.
- A 5xx response, or one larger than 1 MiB, is not stored; its claim is released so the client may retry.
- The SPA's create forms take keys from `formKeys()` (`web/pms/src/lib/api/rest.ts`): the same body gets the same key (a double-click or a retry replays instead of creating twice); an edited body gets a new key; `reset()` after a success starts fresh; `failed(err)` after a failure rotates the key on a definitive 4xx (so an unchanged resubmit is a new request, not a replay of the stored error) and keeps it after a network error, a 5xx or a 409 "still in progress", when the first request may still succeed.

## Patterns to copy

- **Request extractors:** take bodies as `ApiJson<T>`, path parameters as `ApiPath<T>` and query strings as `ApiQuery<T>` (`crates/core-api/src/extract.rs`), never `axum::Json` / `Query`, so rejections are problem details (400 malformed, 422 wrong shape). `axum::Json` is still fine for responses.
- **Resolver errors:** in GraphQL resolvers, map every database call with `.map_err(internal)?` (`graphql.rs`), which logs the error and returns a bare `Internal error`. Never let `?` put `sqlx::Error` text into a GraphQL response.
- **Idempotency claim lifetime:** an unfinished claim is abandoned after 60 s (`ABANDONED_CLAIM_AFTER`) and taken over by the next request with its key; finalizing and releasing touch only the request's own claim (matched on `created_at`).
- **Live events:** the broadcast channel carries `events::LiveEvent` (`Invalidate(db::Event)` | `Resync`). The listener (`events::spawn_listener`, given a pool on the direct listen URL) sends `Resync` whenever its database connection drops, and again once it has reconnected if the first reconnect attempt failed, since changes committed in between were missed. Streams send it as a `resync` event, as they do when a subscriber lags.
- **`inventory_day` lock order:** every command that changes counters locks every row it will change in one statement, `rooms::inventory::lock_days` (`select … order by room_type_id, date for update`), after its room and block row locks and before its first counter update. Rows are therefore always locked in ascending `(room_type_id, date)` order, all at once, so two counter writers cannot deadlock on `inventory_day`, whatever order or plan their UPDATEs use afterwards. A new counter writer (reservations included) must call it the same way; locking a few extra days is fine. Keep each UPDATE valid on its own for the counter constraints: `rooms::inventory::contribute` changes `physical` and `out_of_order` in one statement so `out_of_order <= physical` holds on every row.
- **Startup RLS guard:** `serve` calls `db::assert_rls_applies(&pool)` and refuses to start as a superuser, a `BYPASSRLS` role or a role that owns (directly or through membership) a table in `public`.

## Optimistic concurrency

- Editable resources return `version` in their body and an `ETag: "<version>"` header: handlers return `concurrency::Versioned::{ok, created}(version, body)` (`crates/core-api/src/concurrency.rs`). GraphQL nodes expose `version` too, which is where the SPA reads it.
- Updates take the `concurrency::IfMatch` extractor, so they must send `If-Match: "<version>"`: missing is 428, not a quoted number is 400. The module's `update … where id = $1 and version = $2 … returning` finds no row on mismatch; it then checks whether the row exists and returns a version-mismatch error (412) or not-found (404). The client refetches and shows what changed.
- Reordering (`PUT …/order`) is not a concurrent edit of one resource: it takes no `If-Match` and does not bump versions.

## Change events

After a successful write, in the same transaction, call `db::notify(&mut tx, &Event { tenant_id, property_id, keys })`. Keys name **TanStack Query keys** the client should invalidate (`"properties"`, `"room-types:<property>"`, `"rooms:<property>"` (rooms, sections and block reasons), `"inventory:<property>:<yyyy-mm>"` (one per month a change touches), later `"tape:<property>:<tileStart>"` …). Events never carry data. Choose keys so that a change refetches only screens that show it; the SPA's query keys start with the same string (`web/pms/src/lib/rooms.ts`, `inventory.ts`).

Keep events small: `pg_notify` payloads must stay under 8000 bytes. Inventory month keys are clamped to the counter window (`[business date, business date + 730 days)`, `rooms::inventory::clamped_month_keys`, crate-internal), which caps a change at 25 month keys, and blocks may not end past the window. A new key family that grows with a date range needs the same kind of bound.

## GraphQL

- Schema in `crates/core-api/src/graphql.rs`: depth ≤ 8, complexity ≤ 500. Introspection returns `null` in production. Persisted-query allowlist from Phase 4.
- Resolvers read `PgPool` and `TenantContext` from the request context, check the permission for the `propertyId` argument (`graphql::scoped`), and batch relations with DataLoaders once the first nested relation exists (Phase 1 lists are flat: rooms carry `roomTypeId`, blocks carry `roomId`).
- Date-range queries are bounded: `inventory` spans at most 93 days, `blocks` at most 400.
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
