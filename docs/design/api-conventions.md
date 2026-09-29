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
  | `PropertiesView`, `RoomsView`, `InventoryView`, `RatesView`, `ReservationsView` | ✓ | ✓ | ✓ | ✓ | ✓ |
  | `PropertiesCreate` | ✓ | | | | |
  | `PropertiesManage` (property settings), `RoomsManage` (room types, rooms, sections, block reasons) | ✓ | ✓ | | | |
  | `InventoryBlock` (block and release rooms) | ✓ | ✓ | ✓ | | |
  | `RatesManage` (rate plans, prices, bulk changes, restrictions, meal supplements, cancellation policies) | ✓ | ✓ | | | |
  | `ReservationsManage` (guests, create reservations, cancel, assign and unassign rooms, modify a room, occupants, accounts) | ✓ | ✓ | ✓ | | |
  | `FrontDeskCheckIn` (check in, undo a same-day check-in, check out) | ✓ | ✓ | ✓ | | |
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
- Mount create routes in the `commands` router, which has `.route_layer(from_fn_with_state(state, idempotency::idempotent))`. So is any other command that is not safe to repeat: `POST …/rate-plans/{plan}/bulk-change` (a second "+10 %" would compound) answers 200 and replays like a create.
- Same key, same request: the stored first response is replayed, with its status, body, `Content-Type` and `ETag`. Same key, different request: 422. The request hash covers the user, method, path, query string and body, so a different user or query with the same key is a different request.
- First request still running: 409, and the client retries. A claim is abandoned once it has been unfinished for 60 s (`routes::ABANDONED_CLAIM_AFTER`, the 15 s timeout plus margin), for example after a timeout or a dropped connection, and the next request with that key takes it over and runs.
- A 5xx response, or one larger than 1 MiB, is not stored; its claim is released so the client may retry.
- The SPA's create forms take keys from `formKeys()` (`web/pms/src/lib/api/rest.ts`): the same body gets the same key (a double-click or a retry replays instead of creating twice); an edited body gets a new key; `reset()` after a success starts fresh; `failed(err)` after a failure rotates the key on a definitive 4xx (so an unchanged resubmit is a new request, not a replay of the stored error) and keeps it after a network error, a 5xx or a 409 "still in progress", when the first request may still succeed.

## Patterns to copy

- **Request extractors:** take bodies as `ApiJson<T>`, path parameters as `ApiPath<T>` and query strings as `ApiQuery<T>` (`crates/core-api/src/extract.rs`), never `axum::Json` / `Query`, so rejections are problem details (400 malformed, 422 wrong shape). `axum::Json` is still fine for responses.
- **Resolver errors:** in GraphQL resolvers, map every database call with `.map_err(internal)?` (`graphql.rs`), which logs the error and returns a bare `Internal error`. Never let `?` put `sqlx::Error` text into a GraphQL response.
- **Idempotency claim lifetime:** an unfinished claim is abandoned after 60 s (`ABANDONED_CLAIM_AFTER`) and taken over by the next request with its key; finalizing and releasing touch only the request's own claim (matched on `created_at`).
- **Live events:** the broadcast channel carries `events::LiveEvent` (`Invalidate(db::Event)` | `Resync`). The listener (`events::spawn_listener`, given a pool on the direct listen URL) sends `Resync` whenever its database connection drops, and again once it has reconnected if the first reconnect attempt failed, since changes committed in between were missed. Streams send it as a `resync` event, as they do when a subscriber lags.
- **`inventory_day` lock order:** every counter UPDATE is preceded, in its transaction, by one ordered lock of every row it will change: `rooms::inventory::lock_days` (`select … order by room_type_id, date for update`), called after the command's room and block row locks and before its first counter update. Counter UPDATEs are bounded to the counter window (`[business date, business date + 730 days)`) so they never write a row outside that lock. Rows are therefore locked in ascending `(room_type_id, date)` order, all at once, and two counter updaters cannot deadlock on `inventory_day` whatever order or plan their UPDATEs use afterwards. A new counter writer must call it the same way; locking a few extra days is fine. Reservations are a counter writer that takes no room or block locks first: `reservations::create_reservation` calls `rooms::extend_window` and then `lock_days` once for every requested room type over `[earliest check-in, latest check-out)`, reads the counters to check availability, and takes the `property_counter` row (the confirmation number) before it writes any `reservation_room` row and increments `sold`, so the counter row is always locked after the inventory rows, and before the rows and counters it protects are written. `reservations::cancel_room` locks its `reservation_room` row first, then `lock_days` for the nights it releases (`[max(check-in, business date), check-out)`), and bumps the `reservation` row's version last. Keep each UPDATE valid on its own for the counter constraints: `rooms::inventory::contribute` changes `physical` and `out_of_order` in one statement so `out_of_order <= physical` holds on every row. Window extension (`rooms::extend_window`, INSERT … ON CONFLICT DO NOTHING) is not covered: its SELECT is ordered by `(room_type_id, date)` so rows are inserted in the same order, but Postgres does not formally guarantee INSERT … SELECT insertion order, so it will move to the Phase 7 nightly job under a property-level lock (see the Phase 7 carry-over in [ROADMAP.md](../ROADMAP.md)).
- **The sellable rule (overbooking allowance):** a night of a room type is sellable when `physical - sold - out_of_order + overbooking > 0`, where `overbooking` (`room_type.overbooking`, 0–20, default 0, set through `RoomsManage`) is how many more rooms of the type may be sold than are physically available. Both places that decide whether a booking succeeds — `reservations::availability`'s `free` and `create_reservation`'s per-night check (`reservations::reservations::check_free`) — read this from the one shared SQL fragment (`reservations::SELLABLE`, `i.physical - i.sold - i.out_of_order + rt.overbooking` with `i` aliasing `inventory_day` and `rt` the joined `room_type`), so the rule can't drift between the two call sites. `rooms::InventoryDay::available()` is a different figure on purpose: it stays the plain physical count (`physical - sold - out_of_order`, no allowance), since `rooms` has no notion of a booking and the allowance is only ever applied where a night is actually sold.
- **Room assignment lock order:** `reservation_room` → `room` → `room_block` → `inventory_day`. `reservations::assign_room` locks its `reservation_room` row, then the `room` row (`select … for update`), and takes no counter lock; `unassign_room` locks only its `reservation_room` row. Room and block commands (`rooms::create_block`, `shorten_block`, `update_room`) lock the `room` row first and only read `reservation_room`, with plain SQL (`rooms` cannot depend on `reservations`), before any `room_block` or `inventory_day` lock. Nothing takes these locks in another order, so they cannot deadlock, and an assignment and a block or retype of the same room run one at a time: whichever gets the room second sees the other's committed rows. The room lock also serializes two assignments of one room, which would otherwise wait on each other inside the exclusion check and can deadlock. The exclusion constraint `reservation_room_no_double_booking` stays the final guard against double booking: `assign_room` runs its UPDATE in a savepoint so that, on a violation, it can still read which booking holds the room. Every reservation-room command takes the `reservation` row LAST: `cancel_room`, `assign_room` and `unassign_room` each lock `reservation_room` (and, for `assign_room`, the `room` row too) before bumping `reservation`'s version, never before. No command may lock `reservation` ahead of `reservation_room`.
- **Rates lock:** every write to a property's rate plans, prices and restrictions first takes `rates::lock_rates` (a transaction advisory lock on the property), then reads the plan tree. A change to one plan rewrites the plans derived from it level by level, so writers in one property run one at a time instead of following a lock order over `rate_day` rows. Reads (grid, quote) take no lock.
- **Startup RLS guard:** `serve` calls `db::assert_rls_applies(&pool)` and refuses to start as a superuser, a `BYPASSRLS` role or a role that owns (directly or through membership) a table in `public`.

## Optimistic concurrency

- Editable resources return `version` in their body and an `ETag: "<version>"` header: handlers return `concurrency::Versioned::{ok, created}(version, body)` (`crates/core-api/src/concurrency.rs`). Their `#[utoipa::path]` success response declares the header (`headers(("ETag" = String, …))`); `tests/openapi.rs` lists the operations that do. GraphQL nodes expose `version` too, which is where the SPA reads it.
- Updates take the `concurrency::IfMatch` extractor, so they must send `If-Match: "<version>"`: missing is 428, not a quoted number is 400. The module's `update … where id = $1 and version = $2 … returning` finds no row on mismatch; it then checks whether the row exists and returns a version-mismatch error (412) or not-found (404). The client refetches and shows what changed.
- An update that names no field to change is a 422 ("send at least one field to change"): update DTOs implement `error::Changes` and handlers check them with `error::validate_changes`, so an empty `PATCH` cannot bump the version.
- Reordering (`PUT …/order`) is not a concurrent edit of one resource: it takes no `If-Match` and does not bump versions.

## Change events

After a successful write, in the same transaction, call `db::notify(&mut tx, &Event { tenant_id, property_id, keys })`. Keys name **TanStack Query keys** the client should invalidate (`"properties"`, `"room-types:<property>"`, `"rooms:<property>"` (rooms, sections and block reasons), `"inventory:<property>:<yyyy-mm>"` (one per month a change touches), `"rate-plans:<property>"` (rate plans, meal supplements and cancellation policies), `"rates:<property>:<plan>:<yyyy-mm>"` (one per plan and month a price or restriction change touches, derived plans included), `"reservations:<property>"` (the reservation list), `"reservation:<id>"` (one reservation's detail), later `"tape:<property>:<tileStart>"` …). Events never carry data. Choose keys so that a change refetches only screens that show it; the SPA's query keys start with the same string (`web/pms/src/lib/rooms.ts`, `inventory.ts`).

Keep events small: `pg_notify` payloads must stay under 8000 bytes. Inventory month keys are clamped to the counter window (`[business date, business date + 730 days)`, `rooms::inventory::clamped_month_keys`, crate-internal), which caps a change at 25 month keys, and blocks may not end past the window. A new key family that grows with a date range needs the same kind of bound. Rate keys grow with the number of plans too (a change to a plan with many derived plans), so `rates` writes only inside the same 730-day window and sends its keys in as many events as it takes to keep each payload under 6000 bytes of keys.

## GraphQL

- Schema in `crates/core-api/src/graphql.rs`: depth ≤ 8, complexity ≤ 500. Introspection returns `null` in production. Persisted-query allowlist from Phase 4.
- Resolvers read `PgPool` and `TenantContext` from the request context, check the permission for the `propertyId` argument (`graphql::scoped`), and batch relations with DataLoaders once the first nested relation exists (Phase 1 lists are flat: rooms carry `roomTypeId`, blocks carry `roomId`).
- Date-range queries are bounded: `inventory` and `rateGrid` span at most 93 days, `blocks` at most 400, a `quote` at most 90 nights, `availability` at most 30 nights, `freeRooms` at most 730 (the counter window, so it serves any bookable stay); `bulkChangePreview` returns the first 50 changed cells and the total. (`rates::load_quote` itself refuses stays over `rates::MAX_STAY_NIGHTS`, 730, and the pure `quote` reports them as an `INVALID_STAY` violation).
- Enums shared with REST are mirrored as GraphQL enums (`graphql::mirror_enum!`); GraphQL spells values in capitals (`FIT_F`, `NON_RESIDENT`, `PERCENT`), REST as the database does (`FIT_F`, `non_resident`, `percent`).
- Fields are camelCase; IDs are `UUID`; money is `{ amount: Int (minor units, as string if > 2^53), currency }`; dates are ISO `YYYY-MM-DD`.
- Lists use cursor pagination (`first`, `after` → `{ nodes, pageInfo { endCursor, hasNextPage } }`) once they can exceed a few hundred rows (reservations, guests). `reservations` is the model: keyset on (sort value, id), `first` 1–100, `totalCount` from a separate count run only when selected, and an opaque cursor (base64 JSON) that carries its sort, so a cursor used under another sort or direction is an error. Bind keyset values with their own types, never through a text cast: under row-level security only leakproof comparisons can be index conditions.
- **Checking whether a field was asked for:** use `graphql::selected(ctx, name)`, a one-line wrapper around `ctx.look_ahead().field(name).exists()`. async-graphql already drops selections left out by `@skip`/`@include` (on fields and fragments) before resolving, so `totalCount @include(if: false)` is not selected and look-ahead itself sees that. `selected` exists as a seam, not a workaround: a single place to change resolvers through if that ever stops being true, kept honest by an upgrade-guard unit test that sends both directives and checks the field is (and isn't) counted. Used for `reservations.totalCount` and the reservation detail's `history`; a new expensive, optional field should use it too.
- Under forced row-level security, Postgres evaluates a non-leakproof condition (`like`, pg_trgm's `%` and `<%`, functions such as `lower(daterange)`) only after the tenant filter, so no index serves it. Prefer leakproof forms (`starts_with` instead of `like 'X%'`, a stored generated column instead of an expression index) and check list queries with `EXPLAIN` as the application role.
- **Reading around RLS for index-only searches.** Some conditions (pg_trgm's `<%` is the case in this codebase, guest name search) can never be made leakproof, so no ordinary query can use their index under forced row-level security — and the target platform (Cloud SQL) has no superuser, no BYPASSRLS role and no way to mark a function leakproof, so bypassing RLS isn't an option either. The pattern: a second table holding only the columns the search needs (here, `guest_search`: a guest's id, tenant and lowercased name), with **no row-level security of its own and no privileges for the application role** (`revoke all ... from goodfolk_app`, checked directly against `alter default privileges`, which grants new tables to it automatically) — kept in step by a trigger — and a single `SECURITY DEFINER` function as the only door into it. That function must (a) filter by the tenant inside itself, using the same session setting row-level security itself trusts (`app.current_tenant()`, returning nothing when it is unset) — never trust an argument the caller could get wrong; and (b) return ids only, never any of the row's data, so the caller reads the actual records back from the real table, under row-level security as usual. See `migrations/0009_guest_search.sql` and `reservations::search_guests` for the worked example.

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
