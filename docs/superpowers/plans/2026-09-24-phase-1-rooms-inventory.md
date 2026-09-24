# Phase 1: Rooms, Room Types and Inventory Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A manager defines room types and rooms, groups rooms into floors and housekeeping sections and reorders them, blocks rooms out of order or out of service with conflict detection, and sees per-type daily availability in a virtualized month grid. Phase 0's carry-overs land alongside: problem+json fallbacks, a second SSE resync after a delayed reconnect, the missing tests, `If-Match` concurrency, login throttling and Playwright end-to-end tests.

**Architecture:** A new `rooms` module owns room types, sections, rooms, block reasons, blocks and the `inventory_day` counters. Every command adjusts the counters incrementally inside its own transaction; `rooms::find_drift` recounts them from rooms and blocks, for a property-based test now and the nightly check later. `core-api` adds REST commands (creates behind the idempotency middleware, updates through the new `concurrency::{IfMatch, Versioned}`) and flat GraphQL reads. The SPA adds Room types, Rooms and Inventory screens; the inventory month grid is a reusable, horizontally virtualized `DateGrid` component that the Phase 4 tape chart will build on.

**Tech Stack:** as Phase 0 (Rust 1.97, axum 0.8, sqlx 0.9, async-graphql 7.2, utoipa 6, Postgres 17, SvelteKit 2 / Svelte 5, Bun 1.3, TanStack Query 6), plus `proptest` 1 (dev), `@playwright/test` 1.63.0, and the `macros` and `serde-human-readable` features of `time`.

**Spec:** [docs/specs/phase-1-rooms-inventory.md](../../specs/phase-1-rooms-inventory.md). Also binding: [data-model.md](../../design/data-model.md) (Phase 1 and the rules for every table), [api-conventions.md](../../design/api-conventions.md) (including "Patterns to copy"), [ARCHITECTURE.md](../../ARCHITECTURE.md) §6.5 and §7.3, and [ROADMAP.md](../../ROADMAP.md) Phase 1 with its Phase 0 carry-overs.

**Verified:** every task was executed in order on `main` at `e771231` (Phase 0 complete) in a throwaway worktree before this plan was written, and the code blocks below are rendered from those commits (new files in full, changes as the exact diffs). Every "verify it fails" step failed as described (Task 14 has none: it adds the harness, and its first tests cover existing behaviour), and every passing step passed; `cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D warnings` were clean after every task, and `bun run lint && bun run check && bun run test && bun run build` after every task that touches `web/pms`. At the end: 134 Rust tests pass (plus the ignored release-mode performance test, which passed at p95 3.7 ms against the 20 ms gate), 37 web unit tests, 4 Playwright tests, and the `@perf` Playwright test (month grid median render 38–40 ms against 50 ms, no slow frames while scrolling, 228 of 372 cells in the DOM). Generated API types matched the backend after every task.

Not verified here: the CI workflow itself (no GitHub runner; its steps were run by hand locally, except `cargo deny`, which is not installed here: the new crates `proptest`, `bit-set`, `bit-vec`, `fastrand`, `quick-error`, `rand_xorshift`, `rusty-fork`, `tempfile`, `unarray` and `wait-timeout` are all MIT or Apache-2.0, which `deny.toml` allows), and Chromium's sandbox (this machine cannot start it, so the local Playwright runs used `PLAYWRIGHT_NO_SANDBOX=1`; CI keeps it on, see Task 14). Migration 0004's backfill ran as a superuser; its "lift FORCE RLS while backfilling" step exists for a non-superuser owner such as Neon's and was not exercised with one.

## Global Constraints

- Everything in the Phase 0 plan's Global Constraints still holds: toolchain `1.97`, edition 2024, `unsafe_code = "forbid"`, clippy `all = deny`, rustfmt `max_width = 120`; all data access through `db::begin(pool, scope)`; UUIDv7 ids from Rust; `sqlx::query*` checked at run time and exercised by tests; problem+json errors; CSRF header on every state-changing request.
- Tests need a superuser URL: `export TEST_DATABASE_URL=postgres://goodfolk_owner:goodfolk_owner_dev@localhost:5432/goodfolk`. The Postgres server must also have the default `postgres` database (Task 2's reconnect test acts on the test database from there).
- **Every new tenant table has `tenant_id`, forced RLS and a case in `crates/db/tests/isolation.rs`.** `login_failure` is global (like `session`): no RLS and no `tenant_id` column.
- Permissions (user decision): Owner = everything. Manager = view + `PropertiesManage` + `RoomsManage` + `InventoryBlock`. FrontDesk = view + `InventoryBlock`. Housekeeping and Accountant = view only. Every role has `PropertiesView`, `RoomsView` and `InventoryView`.
- Login throttling (user decision): after 5 failed sign-ins for one email within 15 minutes, further attempts for that email get 429 problem+json until the window passes; stored in Postgres; no per-IP limit; a successful sign-in clears the email's failures; the answer is the same whether or not the account exists.
- Playwright (user decision): Chromium's sandbox is on by default and in CI. `PLAYWRIGHT_NO_SANDBOX=1` turns it off for local runs only (the config refuses it when `CI` is set); the suite only loads `localhost`. The suite stays small.
- Every create command is idempotent (mounted in the `commands` router); every update takes `If-Match: "<version>"` (missing 428, malformed 400, stale 412) and returns `ETag`. Reorders (`PUT …/order`) take no `If-Match` and do not bump versions.
- `inventory_day` rows exist for `[business_date, business_date + 730)`. Blocks are `[from, to)`; they cannot start before the business date; out-of-order blocks reduce availability, out-of-service blocks do not.
- Events: `room-types:<p>`, `rooms:<p>` (rooms, sections, block reasons), `inventory:<p>:<yyyy-mm>` (one per touched month), `properties`. The SPA's query keys start with the same strings.
- Performance gates: `inventory(month)` for 200 rooms / 12 types p95 < 20 ms server time; month grid render < 50 ms and 60 fps scrolling. Both are runnable tests (Tasks 12 and 16), run by hand, not in CI.
- Commit messages describe only the change (no tool or AI attribution).

## Decisions made while planning

Where the spec left a choice open, this plan decided as follows.

1. **Module layout:** one `rooms` crate for types, sections, rooms, reasons, blocks and counters (ARCHITECTURE §12 names `rooms`). Counters and blocks depend on rooms and vice versa, so splitting them would create a cycle.
2. **Counters are incremental, with an independent recount.** Commands add and subtract (`inventory::adjust`, `contribute`); `find_drift` recomputes from source tables. The property-based test compares the two, so it tests something real (a deliberately broken `contribute` fails it within a few sequences).
3. **Window:** `[business_date, business_date + 730)`, created by `extend_window`, which every writer calls first (cheap when nothing is missing) until the Phase 7 job exists. Days before the business date are never changed.
4. **A room's share moves with it:** retyping, deactivating or reactivating a room moves its physical count and the out-of-order days of its active blocks from the business date on. `out_of_order <= physical` is a check constraint. Inactive rooms cannot be blocked; a room type with active rooms cannot be deactivated (409).
5. **Releasing a block** is `PATCH …/blocks/{id} {"to": date}` with `business_date <= to < current end` (shorten only). If `to` is on or before the start, the block is cancelled (`released_at` set, period kept, it stops counting in the exclusion constraint). The UI's Release button sends the business date (or the start, for a future block).
6. **Overlap 409** is a problem with an extension member `conflicts` (the blocks in the way), added through `ApiError::with`. Conflicts are checked under a row lock on the room; the exclusion constraint stays the last line of defence.
7. **Composite foreign keys** `(tenant_id, property_id) → property(tenant_id, id)` and `(property_id, x_id) → x(property_id, id)`, so no row can point into another tenant or property even though foreign-key checks bypass RLS.
8. **Property settings:** `check_in_time`/`check_out_time` default `14:00`/`12:00`, exchanged as `HH:MM` strings; `PATCH /properties/{p}` changes `name`, check-in and check-out; timezone, currency, code and business date are not editable (night audit moves the business date). The settings screen is Phase 8; this phase shows them on the property overview.
9. **Existing properties** get their business date and the five block reasons from migration 0004 (random v4 ids: Postgres 17 has no UUIDv7 function). New properties get reasons from `rooms::seed_block_reasons` in the create-property transaction.
10. **Fields the spec did not define:** `bed_config` is `[{kind, count}]` (free-text kind, 1–10 beds), `amenities` free-text labels; room numbers `^[A-Za-z0-9-]{1,10}$`; `floor` optional text; room type code `^[A-Z0-9]{1,10}$`, immutable; bulk ranges are `prefix + first..=last`, at most 200 rooms, all-or-nothing, and a clash names the taken numbers.
11. **Versions** are added to `housekeeping_section`, `block_reason` and `room_block` (the data model listed them only for types and rooms), because every update takes `If-Match`. `block_reason` also gets `active`, so a reason can be retired without breaking old blocks.
12. **Login throttle mechanics:** one `login_failure` row per attempt, written *before* the password check under a per-email advisory lock, so concurrent attempts cannot exceed five; success deletes the email's rows. Unknown emails are counted the same way. No `Retry-After` header (not asked for).
13. **SSE reconnect:** `spawn_listener` now takes a pool on the listen URL and owns reconnection. When sqlx reports a lost connection it could not re-establish, streams resync, the listener reconnects every second, and streams resync again once it is back.
14. **GraphQL reads are flat** (`rooms` carry `roomTypeId`, blocks carry `roomId`), so no DataLoader is needed yet; `inventory` ranges are capped at 93 days and `blocks` at 400. A property-scoped user asking about another property gets a GraphQL error; another tenant's property id returns empty lists (RLS).
15. **Operation ids:** several handlers are named `create`/`update`, and utoipa's default operation ids collided, which silently merged operations in the generated TypeScript. Every Phase 1 route sets `operation_id`, and `tests/openapi.rs` pins uniqueness (found while verifying Task 15; fixed in Task 10).
16. **TanStack Query `notifyOnChangeProps: 'all'`:** by default a query only re-renders for fields read on its last render, and templates read fields conditionally (`a.data && b.data`), which left the inventory screen stuck on "Loading…" on a fresh load. The root layout's client now notifies on every change (Task 16).
17. **Property-based test without shrinking:** `#[sqlx::test]` runs on a single-threaded runtime, so the test draws 25 random sequences with proptest's `TestRunner` and asserts after every step, printing the failing sequence, instead of using `proptest!` (which would need a blocking runtime per case).
18. **Playwright** starts the API (`cargo run -p core-api`, port 18080) and `vite preview` (port 4173, with the same `/api` and `/graphql` proxy as `vite dev`) itself, against a database named by `E2E_DATABASE_URL`, so test accounts never land in development data. `@perf` tests are excluded unless `E2E_PERF=1`.
19. **Block reason management UI** (beyond using them in the block dialog) is left to Phase 8's settings screens; the API is complete. Sections get a minimal add form on the Rooms screen.
20. **`crates/db/build.rs`** reruns the build when `migrations/` changes: `sqlx::migrate!` embeds the files at compile time, and without it a new migration is silently missing from test binaries.

## How to read the code blocks

New files are shown in full. Changes to existing files are shown as unified diffs against the previous task's result; they are exact, so an engineer can apply them by hand or save one to a file and run `git apply`. Generated files are never shown: `Cargo.lock` (updated by any `cargo` command), `web/pms/bun.lock` (by `bun add`), and `web/pms/src/lib/api/{openapi.json,openapi.d.ts,schema.graphql,gql/}` (by `cd web/pms && bun run api:schemas && bun run codegen`); the steps say when to regenerate, and the result must be committed.

## File Structure

```
migrations/0003_login_throttle.sql      login_failure (global)
migrations/0004_rooms_inventory.sql     btree_gist, property settings, room_type, housekeeping_section, room,
                                        block_reason, room_block (exclusion constraint), inventory_day, backfill
crates/db/build.rs                      rebuild when migrations change
crates/db/tests/rooms_schema.rs         exclusion constraint, composite keys, backfill
modules/identity/src/rbac.rs            Phase 1 permissions; unknown-role warning
modules/identity/src/throttle.rs        reserve_login_attempt, clear_login_failures
modules/property/src/lib.rs             business date, check-in/out, update_property
modules/rooms/                          the new domain module
  src/lib.rs                            RoomsError, cache keys, audit/notify/reorder helpers
  src/inventory.rs                      window, adjust, contribute, find_drift, list_inventory, month_keys
  src/room_types.rs  src/sections.rs  src/rooms.rs  src/blocks.rs
  tests/{room_types,rooms,blocks,counters}.rs, tests/common/mod.rs
crates/core-api/src/concurrency.rs      IfMatch extractor, Versioned response (ETag)
crates/core-api/src/routes/{room_types,rooms,blocks}.rs   REST commands
crates/core-api/src/graphql.rs          roomTypes, rooms, sections, blockReasons, blocks, inventory
crates/core-api/src/events.rs           listener that owns reconnection
crates/core-api/tests/{routing,rooms,blocks,inventory,perf}.rs
web/pms/src/lib/{grid,rooms,inventory}.ts (+ .spec.ts)   layout math, queries, keys, pure helpers
web/pms/src/lib/components/DateGrid.svelte               virtualized rows × dates grid
web/pms/src/lib/components/BlockDialog.svelte
web/pms/src/routes/(app)/p/[property]/{+layout,room-types/+page,rooms/+page,inventory/+page}.svelte
web/pms/playwright.config.ts, web/pms/tests/e2e/{helpers,auth,rooms,inventory,perf}.ts
.github/workflows/ci.yml                e2e job against a real API and Postgres
```

## Tasks

### Task 1: Problem+json fallbacks and the Phase 0 tests still to write

Unmatched routes and wrong methods currently get axum's empty 404/405. This task adds a router fallback and a method-not-allowed fallback that answer with problem details, and writes the tests the Phase 0 reviews asked for: `/readyz` returning 503, the GraphQL depth and complexity limits, an HTTP tenant switch followed by a create that lands in the new tenant, and the `property.created` audit row. Those four pin behaviour Phase 0 already has, so they pass as soon as they are written; only the routing tests fail first.

**Files:**
- Modify: `crates/core-api/src/error.rs`
- Modify: `crates/core-api/src/routes/mod.rs`
- Modify: `docs/design/api-conventions.md`
- Test: `crates/core-api/tests/routing.rs` (new)
- Test: `crates/core-api/tests/common/mod.rs`
- Test: `crates/core-api/tests/graphql.rs`
- Test: `crates/core-api/tests/health.rs`
- Test: `crates/core-api/tests/properties.rs`

**Interfaces:**
- Consumes: `TestApp` (`crates/core-api/tests/common/mod.rs`), `build_schema`, `ApiError`.
- Produces: `ApiError::not_found(detail)`, `ApiError::method_not_allowed()`; `TestApp::with_pool(PgPool) -> TestApp` (used by later tests to build an app on any pool).

- [ ] **Step 1: Write the failing tests**

Modify `crates/core-api/tests/common/mod.rs`:

```diff
--- a/crates/core-api/tests/common/mod.rs
+++ b/crates/core-api/tests/common/mod.rs
@@ -23,7 +23,10 @@ pub struct TestResponse {
 
 impl TestApp {
     pub async fn new(opts: PgConnectOptions) -> Self {
-        let pool = db::testing::app_pool(opts, 5).await;
+        Self::with_pool(db::testing::app_pool(opts, 5).await)
+    }
+
+    pub fn with_pool(pool: PgPool) -> Self {
         let state = AppState::new(pool.clone(), false);
         Self { router: router(state.clone()), state, pool }
     }
```

Create `crates/core-api/tests/routing.rs`:

```rust
mod common;

use axum::http::{Method, StatusCode, header};
use common::TestApp;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_unknown_route_is_a_404_problem(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;

    let response = app.send(Method::GET, "/api/v1/no-such-thing", None, None).await;

    assert_eq!(response.status, StatusCode::NOT_FOUND);
    assert_eq!(response.headers[header::CONTENT_TYPE], "application/problem+json");
    assert_eq!(response.body["status"], 404);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_known_route_with_the_wrong_method_is_a_405_problem(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;

    let response = app.send(Method::DELETE, "/api/v1/me", None, None).await;

    assert_eq!(response.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(response.headers[header::CONTENT_TYPE], "application/problem+json");
    assert_eq!(response.body["status"], 405);
}
```

Modify `crates/core-api/tests/health.rs`:

```diff
--- a/crates/core-api/tests/health.rs
+++ b/crates/core-api/tests/health.rs
@@ -3,6 +3,7 @@ mod common;
 use axum::http::{Method, StatusCode};
 use common::TestApp;
 use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
+use std::time::Duration;
 
 #[sqlx::test(migrator = "db::MIGRATOR")]
 async fn liveness_and_readiness_report_ok(_: PgPoolOptions, opts: PgConnectOptions) {
@@ -20,3 +21,15 @@ async fn responses_carry_a_request_id(_: PgPoolOptions, opts: PgConnectOptions)
 
     assert!(response.headers.contains_key("x-request-id"));
 }
+
+#[tokio::test]
+async fn readiness_reports_503_when_the_database_is_unreachable() {
+    let unreachable = PgPoolOptions::new()
+        .acquire_timeout(Duration::from_millis(500))
+        .connect_lazy("postgres://nobody@127.0.0.1:1/nothing")
+        .unwrap();
+    let app = TestApp::with_pool(unreachable);
+
+    assert_eq!(app.send(Method::GET, "/healthz", None, None).await.status, StatusCode::NO_CONTENT);
+    assert_eq!(app.send(Method::GET, "/readyz", None, None).await.status, StatusCode::SERVICE_UNAVAILABLE);
+}
```

Modify `crates/core-api/tests/graphql.rs`:

```diff
--- a/crates/core-api/tests/graphql.rs
+++ b/crates/core-api/tests/graphql.rs
@@ -83,3 +83,22 @@ async fn production_hides_the_schema_from_introspection() {
     assert_eq!(development, json!({"__schema": {"queryType": {"name": "Query"}}}));
     assert_eq!(production, json!({"__schema": null}));
 }
+
+#[tokio::test]
+async fn queries_nested_deeper_than_eight_levels_are_rejected() {
+    let query = "{ __schema { types { fields { type { ofType { ofType { ofType { ofType { name } } } } } } } } }";
+
+    let response = build_schema(false).execute(query).await;
+
+    assert_eq!(response.errors[0].message, "Query is nested too deep.");
+}
+
+#[tokio::test]
+async fn queries_more_complex_than_500_are_rejected() {
+    let fields: Vec<String> = (0..260).map(|i| format!("p{i}: properties {{ id }}")).collect();
+    let query = format!("{{ {} }}", fields.join(" "));
+
+    let response = build_schema(false).execute(query.as_str()).await;
+
+    assert_eq!(response.errors[0].message, "Query is too complex.");
+}
```

Modify `crates/core-api/tests/properties.rs`:

```diff
--- a/crates/core-api/tests/properties.rs
+++ b/crates/core-api/tests/properties.rs
@@ -3,7 +3,9 @@ mod common;
 use axum::http::{Method, StatusCode, header};
 use common::{TestApp, TestResponse};
 use serde_json::{Value, json};
+use sqlx::PgPool;
 use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
+use uuid::Uuid;
 
 fn galle() -> Value {
     json!({"code": "GAL", "name": "Galle Fort Hotel", "timezone": "Asia/Colombo", "base_currency": "LKR"})
@@ -91,3 +93,62 @@ async fn a_replayed_error_keeps_its_problem_json_content_type(_: PgPoolOptions,
     assert_eq!(retry.headers.get(header::CONTENT_TYPE).unwrap(), "application/problem+json");
     assert_eq!(retry.body, first.body);
 }
+
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn creating_a_property_writes_an_audit_row(_: PgPoolOptions, opts: PgConnectOptions) {
+    let app = TestApp::new(opts.clone()).await;
+    let superuser = PgPool::connect_with(opts).await.unwrap();
+    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
+    let me = app.send(Method::GET, "/api/v1/me", Some(&owner), None).await.body;
+
+    let created = create(&app, &owner, "key-00000001", galle()).await;
+
+    let id = Uuid::parse_str(created.body["id"].as_str().unwrap()).unwrap();
+    let (action, entity, actor, data): (String, String, Uuid, Value) = sqlx::query_as(
+        "select action, entity, actor_user_id, data from audit_log where entity_id = $1 and action <> 'tenant.created'",
+    )
+    .bind(id)
+    .fetch_one(&superuser)
+    .await
+    .unwrap();
+    assert_eq!(action, "property.created");
+    assert_eq!(entity, "property");
+    assert_eq!(actor.to_string(), me["user_id"].as_str().unwrap());
+    assert_eq!(data, json!({"code": "GAL", "name": "Galle Fort Hotel"}));
+}
+
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn after_switching_tenant_a_create_lands_in_the_new_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
+    let app = TestApp::new(opts.clone()).await;
+    let superuser = PgPool::connect_with(opts).await.unwrap();
+    let alice = app.signup_owner("alice@example.com", "Alice Hotels").await;
+    let bob = app.signup_owner("bob@example.com", "Bob Hotels").await;
+    let alice_id = app.send(Method::GET, "/api/v1/me", Some(&alice), None).await.body["user_id"].clone();
+    let bobs_tenant = app.send(Method::GET, "/api/v1/me", Some(&bob), None).await.body["current_tenant"].clone();
+    let (alice_id, bobs_tenant) =
+        (Uuid::parse_str(alice_id.as_str().unwrap()).unwrap(), Uuid::parse_str(bobs_tenant.as_str().unwrap()).unwrap());
+    // Staff invitations arrive in Phase 8; until then a second membership is added directly.
+    sqlx::query("insert into membership (tenant_id, user_id) values ($1, $2)")
+        .bind(bobs_tenant)
+        .bind(alice_id)
+        .execute(&superuser)
+        .await
+        .unwrap();
+    sqlx::query("insert into role_grant (id, tenant_id, user_id, role) values ($1, $2, $3, 'owner')")
+        .bind(Uuid::now_v7())
+        .bind(bobs_tenant)
+        .bind(alice_id)
+        .execute(&superuser)
+        .await
+        .unwrap();
+
+    let switched =
+        app.send(Method::PUT, "/api/v1/session/tenant", Some(&alice), Some(json!({"tenant_id": bobs_tenant}))).await;
+    let created = create(&app, &alice, "key-00000001", galle()).await;
+
+    assert_eq!(switched.status, StatusCode::OK, "{:?}", switched.body);
+    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
+    let owner: Uuid =
+        sqlx::query_scalar("select tenant_id from property where code = 'GAL'").fetch_one(&superuser).await.unwrap();
+    assert_eq!(owner, bobs_tenant);
+}
```

- [ ] **Step 2: Run the tests to verify they fail**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p core-api --no-fail-fast --test routing --test health --test graphql --test properties
```

Expected: `routing` FAILS (2 tests: the 404 and 405 bodies are empty, not problem+json). `health`, `graphql` and `properties` pass, including the new readiness, limit, tenant-switch and audit tests: they cover existing behaviour.

- [ ] **Step 3: Implement the fallbacks**

Both fallbacks are set last on the top-level router, so they apply to every route above, and outside the CSRF layer (an unknown route is a 404 whether or not the header is present).

Modify `crates/core-api/src/error.rs`:

```diff
--- a/crates/core-api/src/error.rs
+++ b/crates/core-api/src/error.rs
@@ -42,6 +42,14 @@ impl ApiError {
         Self::new(StatusCode::FORBIDDEN, "Forbidden", Some(detail.into()))
     }
 
+    pub fn not_found(detail: impl Into<String>) -> Self {
+        Self::new(StatusCode::NOT_FOUND, "Not found", Some(detail.into()))
+    }
+
+    pub fn method_not_allowed() -> Self {
+        Self::new(StatusCode::METHOD_NOT_ALLOWED, "Method not allowed", None)
+    }
+
     pub fn conflict(detail: impl Into<String>) -> Self {
         Self::new(StatusCode::CONFLICT, "Conflict", Some(detail.into()))
     }
```

Modify `crates/core-api/src/routes/mod.rs`:

```diff
--- a/crates/core-api/src/routes/mod.rs
+++ b/crates/core-api/src/routes/mod.rs
@@ -48,6 +48,9 @@ pub fn router(state: AppState) -> Router {
         .layer(from_fn(csrf::require_csrf_header))
         .route("/healthz", get(health::live))
         .route("/readyz", get(health::ready))
+        // Set last, so they cover every route above.
+        .fallback(|| async { ApiError::not_found("no such route") })
+        .method_not_allowed_fallback(|| async { ApiError::method_not_allowed() })
         .layer(CompressionLayer::new())
         .layer(TraceLayer::new_for_http())
         .layer(PropagateRequestIdLayer::x_request_id())
```

Modify `docs/design/api-conventions.md`:

````diff
--- a/docs/design/api-conventions.md
+++ b/docs/design/api-conventions.md
@@ -36,14 +36,15 @@ Every non-GET/HEAD/OPTIONS request must send `x-goodfolk-csrf: 1` (`crates/core-
 { "type": "about:blank", "title": "Conflict", "status": 409, "detail": "a property with this code already exists" }
 ```
 
-`Content-Type: application/problem+json`, for every error the API returns, including malformed bodies and query strings, GraphQL request parse failures and timeouts. Construct errors with `ApiError::{bad_request, unauthenticated, invalid_credentials, forbidden, conflict, unprocessable, gateway_timeout, internal}`. Database errors are logged and become a bare 500, and internal details never reach the client.
+`Content-Type: application/problem+json`, for every error the API returns, including malformed bodies and query strings, GraphQL request parse failures and timeouts. Construct errors with `ApiError::{bad_request, unauthenticated, invalid_credentials, forbidden, not_found, method_not_allowed, conflict, unprocessable, gateway_timeout, internal}`. Database errors are logged and become a bare 500, and internal details never reach the client.
 
 | Status | When |
 |---|---|
 | 400 | Malformed request (invalid JSON, wrong content type, unparseable query string), missing `Idempotency-Key` |
 | 401 | No or expired session; bad credentials |
 | 403 | Missing permission, no tenant selected, missing CSRF header |
-| 404 | Not found **or not visible to this tenant** (never reveal existence) |
+| 404 | Not found **or not visible to this tenant** (never reveal existence), or no such route (the router's fallback) |
+| 405 | The route exists but not for this method (the router's method-not-allowed fallback) |
 | 409 | Uniqueness conflict, double booking (exclusion violation), idempotent request still running |
 | 412 | `If-Match` version mismatch (from Phase 1) |
 | 422 | Body of the wrong shape (missing or mistyped field), validation failed (`garde`), business rule violated, idempotency key reused for a different request |
````

- [ ] **Step 4: Run the tests to verify they pass**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p core-api
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS (`routing`: 2, `health`: 3, `graphql`: 7, `properties`: 7, and the rest unchanged). `fmt` and `clippy` print nothing.

- [ ] **Step 5: Commit**

```sh
git add -A
git commit -m "fix(core-api): answer unknown routes and wrong methods with problem details; cover readiness, GraphQL limits, tenant switch and audit rows"
```

### Task 2: Resync again when the event listener comes back after an outage

When the listener's connection drops and sqlx cannot re-establish it at once, streams are told to resync, but changes committed between that resync and the eventual reconnect were silently missed. `sqlx::PgListener` only reconnects inside `try_recv`, which then blocks waiting for a notification, so the listener cannot tell when it is back. `spawn_listener` therefore takes a pool on the listen URL and owns reconnection: after an error it drops the listener, retries every second, and sends a second `Resync` once subscribed again. The first subscription still happens before `spawn_listener` returns, so `serve` fails fast on a bad `DATABASE_LISTEN_URL` and tests never miss an early event.

The new test makes the outage real: from the `postgres` maintenance database it disallows connections to the test database, terminates the listener's session, checks that no second resync arrives while connections are refused, then allows them again.

**Files:**
- Modify: `crates/core-api/src/events.rs`
- Modify: `crates/core-api/src/main.rs`
- Modify: `docs/design/api-conventions.md`
- Test: `crates/core-api/tests/events.rs`

**Interfaces:**
- Consumes: `events::LiveEvent`, `db::{CHANNEL, Event, notify}`.
- Produces: `events::spawn_listener(pool: PgPool, events: broadcast::Sender<LiveEvent>) -> Result<JoinHandle<()>, sqlx::Error>` (async; replaces the `PgListener` argument).

- [ ] **Step 1: Write the failing test**

The existing tests move to the new signature, and the terminate-the-listener code becomes an `Admin` helper they share.

Modify `crates/core-api/tests/events.rs`:

```diff
--- a/crates/core-api/tests/events.rs
+++ b/crates/core-api/tests/events.rs
@@ -4,11 +4,14 @@ use axum::body::Body;
 use axum::http::{Method, Request, StatusCode, header};
 use common::TestApp;
 use core_api::events::{LiveEvent, spawn_listener};
+use db::{Event, Scope, TenantId};
 use http_body_util::BodyExt;
 use serde_json::json;
-use sqlx::postgres::{PgConnectOptions, PgListener, PgPoolOptions};
+use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
+use sqlx::{AssertSqlSafe, Connection, PgConnection, PgPool};
 use std::time::Duration;
 use tower::ServiceExt;
+use uuid::Uuid;
 
 /// Opens the event stream as `cookie` and reads its `ready` event.
 async fn open_stream(app: &TestApp, cookie: &str) -> Body {
@@ -19,12 +22,42 @@ async fn open_stream(app: &TestApp, cookie: &str) -> Body {
     body
 }
 
+/// A superuser connection to the maintenance database, so it can act on the test database from outside.
+struct Admin {
+    conn: PgConnection,
+    database: String,
+}
+
+impl Admin {
+    async fn connect(opts: &PgConnectOptions) -> Self {
+        let database = opts.get_database().expect("sqlx::test names the database").to_owned();
+        let conn = PgConnection::connect_with(&opts.clone().database("postgres")).await.unwrap();
+        Self { conn, database }
+    }
+
+    /// Ends the listener's database session, as a network failure or a database restart would.
+    async fn terminate_listener(&mut self) {
+        let terminated: Vec<bool> = sqlx::query_scalar(
+            "select pg_terminate_backend(pid) from pg_stat_activity where datname = $1 and query like 'LISTEN%'",
+        )
+        .bind(&self.database)
+        .fetch_all(&mut self.conn)
+        .await
+        .unwrap();
+        assert_eq!(terminated, vec![true]);
+    }
+
+    /// While disallowed, every new connection to the test database fails, as during an outage.
+    async fn allow_connections(&mut self, allow: bool) {
+        let sql = format!("alter database \"{}\" allow_connections {allow}", self.database);
+        sqlx::query(AssertSqlSafe(sql)).execute(&mut self.conn).await.unwrap();
+    }
+}
+
 #[sqlx::test(migrator = "db::MIGRATOR")]
 async fn creating_a_property_pushes_an_invalidation_to_the_tenants_stream(_: PgPoolOptions, opts: PgConnectOptions) {
     let app = TestApp::new(opts.clone()).await;
-    let mut listener = PgListener::connect_with(&sqlx::PgPool::connect_with(opts).await.unwrap()).await.unwrap();
-    listener.listen(db::CHANNEL).await.unwrap();
-    spawn_listener(listener, app.state.events.clone());
+    spawn_listener(PgPool::connect_with(opts).await.unwrap(), app.state.events.clone()).await.unwrap();
     let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
 
     let request = Request::builder().uri("/api/v1/events").header(header::COOKIE, &owner).body(Body::empty()).unwrap();
@@ -62,20 +95,11 @@ async fn a_malformed_query_is_a_400_problem(_: PgPoolOptions, opts: PgConnectOpt
 #[sqlx::test(migrator = "db::MIGRATOR")]
 async fn losing_the_listener_connection_tells_every_stream_to_resync(_: PgPoolOptions, opts: PgConnectOptions) {
     let app = TestApp::new(opts.clone()).await;
-    let superuser = sqlx::PgPool::connect_with(opts).await.unwrap();
-    let mut listener = PgListener::connect_with(&superuser).await.unwrap();
-    listener.listen(db::CHANNEL).await.unwrap();
+    let mut admin = Admin::connect(&opts).await;
     let mut events = app.state.events.subscribe();
-    spawn_listener(listener, app.state.events.clone());
+    spawn_listener(PgPool::connect_with(opts).await.unwrap(), app.state.events.clone()).await.unwrap();
 
-    let terminated: Vec<bool> = sqlx::query_scalar(
-        "select pg_terminate_backend(pid) from pg_stat_activity
-         where datname = current_database() and query like 'LISTEN%'",
-    )
-    .fetch_all(&superuser)
-    .await
-    .unwrap();
-    assert_eq!(terminated, vec![true]);
+    admin.terminate_listener().await;
 
     let received = tokio::time::timeout(Duration::from_secs(5), events.recv()).await.unwrap().unwrap();
     assert_eq!(received, LiveEvent::Resync);
@@ -96,9 +120,7 @@ async fn a_resync_reaches_the_stream_as_a_resync_event(_: PgPoolOptions, opts: P
 #[sqlx::test(migrator = "db::MIGRATOR")]
 async fn another_tenants_stream_stays_silent(_: PgPoolOptions, opts: PgConnectOptions) {
     let app = TestApp::new(opts.clone()).await;
-    let mut listener = PgListener::connect_with(&sqlx::PgPool::connect_with(opts).await.unwrap()).await.unwrap();
-    listener.listen(db::CHANNEL).await.unwrap();
-    spawn_listener(listener, app.state.events.clone());
+    spawn_listener(PgPool::connect_with(opts).await.unwrap(), app.state.events.clone()).await.unwrap();
     let alice = app.signup_owner("alice@example.com", "Alice Hotels").await;
     let bob = app.signup_owner("bob@example.com", "Bob Hotels").await;
     let mut alices_stream = open_stream(&app, &alice).await;
@@ -120,3 +142,29 @@ async fn another_tenants_stream_stays_silent(_: PgPoolOptions, opts: PgConnectOp
     let bobs = tokio::time::timeout(Duration::from_secs(1), bobs_stream.frame()).await;
     assert!(bobs.is_err(), "Bob's stream received {bobs:?}");
 }
+
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn a_delayed_reconnect_resyncs_again_once_events_flow(_: PgPoolOptions, opts: PgConnectOptions) {
+    let app = TestApp::new(opts.clone()).await;
+    let mut admin = Admin::connect(&opts).await;
+    let mut events = app.state.events.subscribe();
+    spawn_listener(PgPool::connect_with(opts).await.unwrap(), app.state.events.clone()).await.unwrap();
+
+    admin.allow_connections(false).await;
+    admin.terminate_listener().await;
+    let lost = tokio::time::timeout(Duration::from_secs(5), events.recv()).await.unwrap().unwrap();
+    let while_down = tokio::time::timeout(Duration::from_secs(2), events.recv()).await;
+    admin.allow_connections(true).await;
+    let back = tokio::time::timeout(Duration::from_secs(5), events.recv()).await.unwrap().unwrap();
+
+    assert_eq!(lost, LiveEvent::Resync);
+    assert!(while_down.is_err(), "resynced before the listener was back: {while_down:?}");
+    assert_eq!(back, LiveEvent::Resync);
+    let tenant = TenantId(Uuid::now_v7());
+    let event = Event { tenant_id: tenant, property_id: None, keys: vec!["properties".into()] };
+    let mut tx = db::begin(&app.pool, Scope::tenant(tenant)).await.unwrap();
+    db::notify(&mut tx, &event).await.unwrap();
+    tx.commit().await.unwrap();
+    let received = tokio::time::timeout(Duration::from_secs(5), events.recv()).await.unwrap().unwrap();
+    assert_eq!(received, LiveEvent::Invalidate(event));
+}
```

- [ ] **Step 2: Run the test to verify it fails**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p core-api --test events
```

Expected: FAIL to compile: `mismatched types` (four calls pass a `PgPool` where `spawn_listener` still takes a `PgListener`).

- [ ] **Step 3: Implement**

Modify `crates/core-api/src/events.rs`:

```diff
--- a/crates/core-api/src/events.rs
+++ b/crates/core-api/src/events.rs
@@ -6,6 +6,7 @@ use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
 use db::Event;
 use futures::Stream;
 use serde::Deserialize;
+use sqlx::PgPool;
 use sqlx::postgres::PgListener;
 use std::convert::Infallible;
 use std::time::Duration;
@@ -24,36 +25,68 @@ pub enum LiveEvent {
     Resync,
 }
 
-/// Forwards every `NOTIFY gf_events` from Postgres to this instance's subscribers.
-/// Every instance runs one, so every connected client hears about every change.
+/// How long the listener waits before each attempt to reconnect.
+const RECONNECT_DELAY: Duration = Duration::from_secs(1);
+
+/// Subscribes to `NOTIFY gf_events` on `pool` (a direct connection: poolers in transaction mode cannot
+/// LISTEN), then forwards every notification to this instance's subscribers. Every instance runs one,
+/// so every connected client hears about every change. Fails if the first subscription fails.
 ///
-/// Notifications sent while the connection is down are lost, so every stream is told to resync when it drops.
-pub fn spawn_listener(mut listener: PgListener, events: broadcast::Sender<LiveEvent>) -> tokio::task::JoinHandle<()> {
-    // Sending with no subscribers is normal, so send results are ignored.
-    tokio::spawn(async move {
-        loop {
-            match listener.try_recv().await {
-                Ok(Some(notification)) => match serde_json::from_str::<Event>(notification.payload()) {
-                    Ok(event) => {
-                        let _ = events.send(LiveEvent::Invalidate(event));
-                    }
-                    Err(err) => tracing::warn!(error = %err, "ignoring malformed event payload"),
-                },
-                Ok(None) => {
-                    // The listener has already reconnected and listens again.
-                    tracing::warn!("event listener connection lost; streams will resync");
-                    let _ = events.send(LiveEvent::Resync);
-                }
-                Err(err) => {
-                    // The connection may be gone; the next try_recv reconnects. Back off briefly, then
-                    // resync, since notifications may have been missed.
-                    tracing::warn!(error = %err, "event listener error; streams will resync");
-                    tokio::time::sleep(Duration::from_secs(1)).await;
-                    let _ = events.send(LiveEvent::Resync);
+/// Notifications sent while the connection is down are lost, so streams are told to resync when it
+/// drops, and again once the listener is back if reconnecting took more than one attempt.
+pub async fn spawn_listener(
+    pool: PgPool,
+    events: broadcast::Sender<LiveEvent>,
+) -> Result<tokio::task::JoinHandle<()>, sqlx::Error> {
+    let listener = subscribe(&pool).await?;
+    Ok(tokio::spawn(forward(pool, listener, events)))
+}
+
+async fn subscribe(pool: &PgPool) -> Result<PgListener, sqlx::Error> {
+    let mut listener = PgListener::connect_with(pool).await?;
+    listener.listen(db::CHANNEL).await?;
+    Ok(listener)
+}
+
+// Sending with no subscribers is normal, so send results are ignored.
+async fn forward(pool: PgPool, mut listener: PgListener, events: broadcast::Sender<LiveEvent>) {
+    loop {
+        match listener.try_recv().await {
+            Ok(Some(notification)) => match serde_json::from_str::<Event>(notification.payload()) {
+                Ok(event) => {
+                    let _ = events.send(LiveEvent::Invalidate(event));
                 }
+                Err(err) => tracing::warn!(error = %err, "ignoring malformed event payload"),
+            },
+            Ok(None) => {
+                // The connection dropped and sqlx has already reconnected and listens again.
+                tracing::warn!("event listener connection lost and re-established; streams will resync");
+                let _ = events.send(LiveEvent::Resync);
+            }
+            Err(err) => {
+                // The connection is lost and sqlx could not re-establish it. Streams resync now, but
+                // changes committed until the listener is back are missed too, so they resync again then.
+                tracing::warn!(error = %err, "event listener connection lost; streams will resync");
+                let _ = events.send(LiveEvent::Resync);
+                drop(listener);
+                listener = reconnect(&pool).await;
+                let _ = events.send(LiveEvent::Resync);
+            }
+        }
+    }
+}
+
+async fn reconnect(pool: &PgPool) -> PgListener {
+    loop {
+        tokio::time::sleep(RECONNECT_DELAY).await;
+        match subscribe(pool).await {
+            Ok(listener) => {
+                tracing::info!("event listener reconnected; streams will resync");
+                return listener;
             }
+            Err(err) => tracing::warn!(error = %err, "event listener could not reconnect; retrying"),
         }
-    })
+    }
 }
 
 #[derive(Debug, Deserialize)]
```

Modify `crates/core-api/src/main.rs`:

```diff
--- a/crates/core-api/src/main.rs
+++ b/crates/core-api/src/main.rs
@@ -1,7 +1,7 @@
 use anyhow::{Context, bail};
 use core_api::config::Config;
 use core_api::{AppState, events, router};
-use sqlx::postgres::PgListener;
+use sqlx::postgres::PgPoolOptions;
 use tracing_subscriber::EnvFilter;
 
 #[global_allocator]
@@ -22,9 +22,13 @@ async fn serve() -> anyhow::Result<()> {
     let pool = db::connect(&config.database_url, config.database_max_connections).await?;
     db::assert_rls_applies(&pool).await?;
     let state = AppState::new(pool, config.production);
-    let mut listener = PgListener::connect(&config.database_listen_url).await?;
-    listener.listen(db::CHANNEL).await?;
-    events::spawn_listener(listener, state.events.clone());
+    // One direct connection, kept open, used only for LISTEN (and to reconnect it).
+    let listen_pool = PgPoolOptions::new()
+        .max_connections(1)
+        .max_lifetime(None)
+        .idle_timeout(None)
+        .connect_lazy(&config.database_listen_url)?;
+    events::spawn_listener(listen_pool, state.events.clone()).await?;
 
     let tcp = tokio::net::TcpListener::bind(config.bind_addr).await?;
     tracing::info!(addr = %config.bind_addr, "listening");
```

Modify `docs/design/api-conventions.md`:

```diff
--- a/docs/design/api-conventions.md
+++ b/docs/design/api-conventions.md
@@ -67,7 +67,7 @@ Request DTOs derive `garde::Validate` and are checked with `error::validate(&bod
 - **Request extractors:** take bodies as `ApiJson<T>` and query strings as `ApiQuery<T>` (`crates/core-api/src/extract.rs`), never `axum::Json` / `Query`, so rejections are problem details (400 malformed, 422 wrong shape). `axum::Json` is still fine for responses.
 - **Resolver errors:** in GraphQL resolvers, map every database call with `.map_err(internal)?` (`graphql.rs`), which logs the error and returns a bare `Internal error`. Never let `?` put `sqlx::Error` text into a GraphQL response.
 - **Idempotency claim lifetime:** an unfinished claim is abandoned after 60 s (`ABANDONED_CLAIM_AFTER`) and taken over by the next request with its key; finalizing and releasing touch only the request's own claim (matched on `created_at`).
-- **Live events:** the broadcast channel carries `events::LiveEvent` (`Invalidate(db::Event)` | `Resync`). The listener sends `Resync` whenever its database connection drops, and streams send it as a `resync` event, as they do when a subscriber lags.
+- **Live events:** the broadcast channel carries `events::LiveEvent` (`Invalidate(db::Event)` | `Resync`). The listener (`events::spawn_listener`, given a pool on the direct listen URL) sends `Resync` whenever its database connection drops, and again once it has reconnected if the first reconnect attempt failed, since changes committed in between were missed. Streams send it as a `resync` event, as they do when a subscriber lags.
 - **Startup RLS guard:** `serve` calls `db::assert_rls_applies(&pool)` and refuses to start as a superuser, a `BYPASSRLS` role or a role that owns (directly or through membership) a table in `public`.
 
 ## Optimistic concurrency (from Phase 1)
```

- [ ] **Step 4: Run the tests to verify they pass**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p core-api --test events
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS: 6 tests (about 10 s: the delayed-reconnect test waits out a 2 s outage).

- [ ] **Step 5: Commit**

```sh
git add -A
git commit -m "fix(core-api): resync streams again when the event listener reconnects after an outage"
```

### Task 3: Room and inventory permissions; warn on unknown roles

Phase 1's actions become `identity::Permission` variants, granted per the user's matrix (Global Constraints). The test pins the whole matrix, so adding a permission later without deciding who gets it fails loudly. `load_grants` also logs a warning when it skips a role this build does not know (for example one written by a newer release), which the Phase 0 review asked for.

**Files:**
- Modify: `docs/design/api-conventions.md`
- Modify: `modules/identity/Cargo.toml`
- Modify: `modules/identity/src/rbac.rs`
- Test: `modules/identity/tests/accounts.rs`
- Test: `modules/identity/tests/rbac.rs`
- Regenerated, not shown: `Cargo.lock`

**Interfaces:**
- Consumes: `identity::{Grant, Role, allows, load_grants}`.
- Produces: `Permission::{PropertiesManage, RoomsView, RoomsManage, InventoryView, InventoryBlock}` (alongside `PropertiesView`, `PropertiesCreate`).

- [ ] **Step 1: Write the failing tests**

`identity` needs `tracing` (to log) and, for the test only, `tracing-subscriber` to capture the output.

Modify `modules/identity/Cargo.toml`:

```diff
--- a/modules/identity/Cargo.toml
+++ b/modules/identity/Cargo.toml
@@ -16,11 +16,13 @@ sqlx.workspace = true
 thiserror.workspace = true
 time.workspace = true
 tokio.workspace = true
+tracing.workspace = true
 utoipa.workspace = true
 uuid.workspace = true
 
 [dev-dependencies]
 db = { workspace = true, features = ["testing"] }
+tracing-subscriber.workspace = true
 
 [lints]
 workspace = true
```

Modify `modules/identity/tests/rbac.rs`:

```diff
--- a/modules/identity/tests/rbac.rs
+++ b/modules/identity/tests/rbac.rs
@@ -35,3 +35,28 @@ fn roles_round_trip_through_their_database_names() {
     }
     assert_eq!(Role::parse("root"), None);
 }
+
+#[test]
+fn each_role_has_exactly_its_permissions() {
+    use Permission::*;
+    let all =
+        [PropertiesView, PropertiesCreate, PropertiesManage, RoomsView, RoomsManage, InventoryView, InventoryBlock];
+    let expected: [(Role, &[Permission]); 5] = [
+        (Role::Owner, &all),
+        (Role::Manager, &[PropertiesView, PropertiesManage, RoomsView, RoomsManage, InventoryView, InventoryBlock]),
+        (Role::FrontDesk, &[PropertiesView, RoomsView, InventoryView, InventoryBlock]),
+        (Role::Housekeeping, &[PropertiesView, RoomsView, InventoryView]),
+        (Role::Accountant, &[PropertiesView, RoomsView, InventoryView]),
+    ];
+
+    for (role, permitted) in expected {
+        let grants = [Grant { property_id: Some(HOTEL), role }];
+        for permission in all {
+            assert_eq!(
+                allows(&grants, permission, Some(HOTEL)),
+                permitted.contains(&permission),
+                "{role:?} and {permission:?}"
+            );
+        }
+    }
+}
```

Modify `modules/identity/tests/accounts.rs`:

```diff
--- a/modules/identity/tests/accounts.rs
+++ b/modules/identity/tests/accounts.rs
@@ -1,9 +1,14 @@
 use db::testing::app_pool;
+use db::{Scope, begin};
 use identity::{
-    Permission, SignupError, SignupInput, allows, authenticate, create_session, default_tenant, delete_session,
-    load_profile, resolve_session, signup, switch_tenant,
+    Grant, Permission, Role, SignupError, SignupInput, allows, authenticate, create_session, default_tenant,
+    delete_session, load_grants, load_profile, resolve_session, signup, switch_tenant,
 };
+use sqlx::PgPool;
 use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
+use std::sync::{Arc, Mutex};
+use tracing_subscriber::fmt::MakeWriter;
+use uuid::Uuid;
 
 fn input(email: &str, tenant: &str) -> SignupInput {
     SignupInput {
@@ -75,3 +80,57 @@ async fn a_session_can_switch_only_to_tenants_the_user_belongs_to(_: PgPoolOptio
     assert!(switch_tenant(&pool, &session, tenant).await.unwrap());
     assert_eq!(resolve_session(&pool, &token).await.unwrap().unwrap().tenant, Some(tenant));
 }
+
+/// Collects log output so a test can check what was logged.
+#[derive(Clone, Default)]
+struct CapturedLogs(Arc<Mutex<Vec<u8>>>);
+
+impl CapturedLogs {
+    fn text(&self) -> String {
+        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
+    }
+}
+
+impl std::io::Write for CapturedLogs {
+    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
+        self.0.lock().unwrap().extend_from_slice(buf);
+        Ok(buf.len())
+    }
+
+    fn flush(&mut self) -> std::io::Result<()> {
+        Ok(())
+    }
+}
+
+impl<'a> MakeWriter<'a> for CapturedLogs {
+    type Writer = CapturedLogs;
+
+    fn make_writer(&'a self) -> Self::Writer {
+        self.clone()
+    }
+}
+
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn a_grant_with_an_unknown_role_is_skipped_with_a_warning(_: PgPoolOptions, opts: PgConnectOptions) {
+    let pool = app_pool(opts.clone(), 1).await;
+    let superuser = PgPool::connect_with(opts).await.unwrap();
+    let (user, tenant) = signup(&pool, input("owner@example.com", "A")).await.unwrap();
+    // A role written by a newer release (or by hand) that this build does not know.
+    sqlx::query("alter table role_grant drop constraint role_grant_role_check").execute(&superuser).await.unwrap();
+    sqlx::query("insert into role_grant (id, tenant_id, user_id, role) values ($1, $2, $3, 'night_auditor')")
+        .bind(Uuid::now_v7())
+        .bind(tenant.0)
+        .bind(user.0)
+        .execute(&superuser)
+        .await
+        .unwrap();
+    let logs = CapturedLogs::default();
+    let _guard = tracing::subscriber::set_default(tracing_subscriber::fmt().with_writer(logs.clone()).finish());
+
+    let mut tx = begin(&pool, Scope::tenant(tenant)).await.unwrap();
+    let grants = load_grants(&mut tx, user).await.unwrap();
+
+    assert_eq!(grants, vec![Grant { property_id: None, role: Role::Owner }]);
+    let logged = logs.text();
+    assert!(logged.contains("WARN") && logged.contains("night_auditor"), "logged: {logged}");
+}
```

- [ ] **Step 2: Run the tests to verify they fail**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p identity --no-fail-fast
```

Expected: `rbac` FAILS to compile (`cannot find value PropertiesManage`, …). `accounts` FAILS: `a_grant_with_an_unknown_role_is_skipped_with_a_warning` (nothing is logged).

- [ ] **Step 3: Implement**

Modify `modules/identity/src/rbac.rs`:

```diff
--- a/modules/identity/src/rbac.rs
+++ b/modules/identity/src/rbac.rs
@@ -29,13 +29,17 @@ impl Role {
             .find(|role| role.as_str() == value)
     }
 
+    /// Anything not listed is denied, so a new permission is granted only where it is added.
     fn permits(self, permission: Permission) -> bool {
         use Permission::*;
         match self {
             Role::Owner => true,
-            Role::Manager | Role::FrontDesk | Role::Housekeeping | Role::Accountant => {
-                matches!(permission, PropertiesView)
-            }
+            Role::Manager => matches!(
+                permission,
+                PropertiesView | PropertiesManage | RoomsView | RoomsManage | InventoryView | InventoryBlock
+            ),
+            Role::FrontDesk => matches!(permission, PropertiesView | RoomsView | InventoryView | InventoryBlock),
+            Role::Housekeeping | Role::Accountant => matches!(permission, PropertiesView | RoomsView | InventoryView),
         }
     }
 }
@@ -45,6 +49,16 @@ impl Role {
 pub enum Permission {
     PropertiesView,
     PropertiesCreate,
+    /// Change a property's settings (name, check-in and check-out times).
+    PropertiesManage,
+    /// See room types, rooms, sections and block reasons.
+    RoomsView,
+    /// Create, change, deactivate and reorder room types, rooms, sections and block reasons.
+    RoomsManage,
+    /// See inventory counts and room blocks.
+    InventoryView,
+    /// Block rooms and release or shorten blocks.
+    InventoryBlock,
 }
 
 /// A role held tenant-wide (`property_id: None`) or for one property.
@@ -71,6 +85,12 @@ pub async fn load_grants(tx: &mut Tx, user: UserId) -> Result<Vec<Grant>, sqlx::
             .await?;
     Ok(rows
         .into_iter()
-        .filter_map(|(property_id, role)| Role::parse(&role).map(|role| Grant { property_id, role }))
+        .filter_map(|(property_id, role)| match Role::parse(&role) {
+            Some(role) => Some(Grant { property_id, role }),
+            None => {
+                tracing::warn!(role, user = %user.0, "skipping a role grant with a role this build does not know");
+                None
+            }
+        })
         .collect())
 }
```

Modify `docs/design/api-conventions.md`:

```diff
--- a/docs/design/api-conventions.md
+++ b/docs/design/api-conventions.md
@@ -23,7 +23,14 @@ Simple single-object reads that bootstrap the app (`GET /api/v1/me`) may be REST
   - `Authenticated`: any signed-in user (401 otherwise).
   - `TenantContext`: signed-in user with a selected tenant, grants loaded (403 `no tenant selected` otherwise). Use this for every tenant-scoped endpoint.
   - Both are resolved once per request and cached in request extensions.
-- Authorisation: `ctx.require(Permission::X, Some(property_id))?` at the top of every handler. Add new actions to `identity::Permission` and to each role's `permits`, with a test.
+- Authorisation: `ctx.require(Permission::X, Some(property_id))?` at the top of every handler. Add new actions to `identity::Permission` and to each role's `permits`, with a test (`modules/identity/tests/rbac.rs` pins the whole matrix). A grant with a role this build does not know is skipped with a warning.
+
+  | Permission | Owner | Manager | Front desk | Housekeeping | Accountant |
+  |---|---|---|---|---|---|
+  | `PropertiesView`, `RoomsView`, `InventoryView` | ✓ | ✓ | ✓ | ✓ | ✓ |
+  | `PropertiesCreate` | ✓ | | | | |
+  | `PropertiesManage` (property settings), `RoomsManage` (room types, rooms, sections, block reasons) | ✓ | ✓ | | | |
+  | `InventoryBlock` (block and release rooms) | ✓ | ✓ | ✓ | | |
 - Data access: `db::begin(&state.pool, Scope::tenant(ctx.tenant))` and never anything else. RLS is the safety net, but queries still filter by `property_id` explicitly.
 
 ## CSRF
```

- [ ] **Step 4: Run the tests to verify they pass**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p identity
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS (`accounts`: 6, `rbac`: 5, `password`: 2).

- [ ] **Step 5: Commit**

```sh
git add -A
git commit -m "feat(identity): room and inventory permissions; warn when a grant has an unknown role"
```

### Task 4: Login throttling

After 5 failed sign-ins for one email within 15 minutes, further attempts get 429 until the window passes. Attempts are rows in a global `login_failure` table, reserved *before* the password is checked under a per-email advisory lock, so concurrent attempts cannot get more than five guesses. A successful sign-in deletes the email's rows. Unknown emails are counted the same way, so the 429 never reveals whether an account exists.

This is the first new migration since Phase 0, which exposes a build problem: `sqlx::migrate!` embeds `migrations/` at compile time, and nothing tells cargo to rebuild `db` when a file is added, so test binaries silently run without it. `crates/db/build.rs` fixes that.

**Files:**
- Create: `crates/db/build.rs`
- Create: `migrations/0003_login_throttle.sql`
- Create: `modules/identity/src/throttle.rs`
- Modify: `crates/core-api/src/error.rs`
- Modify: `crates/core-api/src/routes/auth.rs`
- Modify: `docs/design/api-conventions.md`
- Modify: `docs/design/data-model.md`
- Modify: `modules/identity/src/lib.rs`
- Test: `crates/core-api/tests/auth.rs`

**Interfaces:**
- Consumes: `db::{begin, Scope}`, `identity::authenticate`.
- Produces: `identity::{reserve_login_attempt(pool, email) -> Result<bool, sqlx::Error>, clear_login_failures(pool, email), MAX_LOGIN_FAILURES = 5, LOGIN_WINDOW_SECS = 900.0}`; `ApiError::too_many_requests(detail)`; table `login_failure(id, email citext, at)`.

- [ ] **Step 1: Write the failing tests**

Modify `crates/core-api/tests/auth.rs`:

```diff
--- a/crates/core-api/tests/auth.rs
+++ b/crates/core-api/tests/auth.rs
@@ -3,6 +3,7 @@ mod common;
 use axum::http::{Method, StatusCode, header};
 use common::{TestApp, session_cookie};
 use serde_json::json;
+use sqlx::PgPool;
 use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
 
 #[sqlx::test(migrator = "db::MIGRATOR")]
@@ -161,3 +162,67 @@ async fn a_session_whose_tenant_membership_was_removed_is_refused(_: PgPoolOptio
     assert_eq!(response.status, StatusCode::FORBIDDEN, "{:?}", response.body);
     assert_eq!(response.body["detail"], "no tenant selected");
 }
+
+async fn login(app: &TestApp, email: &str, password: &str) -> common::TestResponse {
+    app.send(Method::POST, "/api/v1/auth/login", None, Some(json!({"email": email, "password": password}))).await
+}
+
+/// Sends `count` sign-ins one after another and returns their statuses.
+async fn login_times(app: &TestApp, email: &str, password: &str, count: usize) -> Vec<StatusCode> {
+    let mut statuses = Vec::with_capacity(count);
+    for _ in 0..count {
+        statuses.push(login(app, email, password).await.status);
+    }
+    statuses
+}
+
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn five_failed_logins_lock_the_email_until_the_window_passes(_: PgPoolOptions, opts: PgConnectOptions) {
+    let app = TestApp::new(opts.clone()).await;
+    let superuser = PgPool::connect_with(opts).await.unwrap();
+    app.signup_owner("owner@example.com", "A").await;
+    app.signup_owner("other@example.com", "B").await;
+
+    let failures: Vec<StatusCode> = login_times(&app, "owner@example.com", "not the password", 5).await;
+    let locked = login(&app, "Owner@Example.com", "a long enough password").await;
+    let other = login(&app, "other@example.com", "a long enough password").await;
+    sqlx::query("update login_failure set at = at - interval '15 minutes'").execute(&superuser).await.unwrap();
+    let after_window = login(&app, "owner@example.com", "a long enough password").await;
+
+    assert_eq!(failures, vec![StatusCode::UNAUTHORIZED; 5]);
+    assert_eq!(locked.status, StatusCode::TOO_MANY_REQUESTS, "{:?}", locked.body);
+    assert!(is_problem_json(&locked), "{:?}", locked.headers);
+    assert_eq!(other.status, StatusCode::OK);
+    assert_eq!(after_window.status, StatusCode::OK, "{:?}", after_window.body);
+}
+
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn a_throttled_unknown_email_looks_like_a_throttled_account(_: PgPoolOptions, opts: PgConnectOptions) {
+    let app = TestApp::new(opts).await;
+    app.signup_owner("owner@example.com", "A").await;
+    login_times(&app, "owner@example.com", "wrong", 5).await;
+    login_times(&app, "nobody@example.com", "wrong", 5).await;
+
+    let known = login(&app, "owner@example.com", "wrong").await;
+    let unknown = login(&app, "nobody@example.com", "wrong").await;
+
+    assert_eq!(known.status, StatusCode::TOO_MANY_REQUESTS);
+    assert_eq!(unknown.status, known.status);
+    assert_eq!(unknown.body, known.body);
+}
+
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn a_successful_login_clears_earlier_failures(_: PgPoolOptions, opts: PgConnectOptions) {
+    let app = TestApp::new(opts).await;
+    app.signup_owner("owner@example.com", "A").await;
+
+    let before = login_times(&app, "owner@example.com", "wrong", 4).await;
+    let success = login(&app, "owner@example.com", "a long enough password").await;
+    let after = login_times(&app, "owner@example.com", "wrong", 4).await;
+    let still_allowed = login(&app, "owner@example.com", "a long enough password").await;
+
+    assert_eq!(before, vec![StatusCode::UNAUTHORIZED; 4]);
+    assert_eq!(success.status, StatusCode::OK);
+    assert_eq!(after, vec![StatusCode::UNAUTHORIZED; 4]);
+    assert_eq!(still_allowed.status, StatusCode::OK);
+}
```

- [ ] **Step 2: Run the tests to verify they fail**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p core-api --test auth
```

Expected: FAIL: `five_failed_logins_lock_the_email_until_the_window_passes` and `a_throttled_unknown_email_looks_like_a_throttled_account` (the sixth attempt is a 401 or 200, not 429). `a_successful_login_clears_earlier_failures` already passes, since nothing is throttled yet.

- [ ] **Step 3: Implement**

Create `migrations/0003_login_throttle.sql`:

```sql
-- Sign-in attempts per email, for throttling. Looked up before any tenant is known, so, like `session`,
-- it has no RLS and no tenant_id. Attempts for unknown emails are recorded too, so throttling never reveals
-- whether an account exists. A successful sign-in deletes its email's rows.
create table login_failure (
  id uuid primary key,
  email citext not null,
  at timestamptz not null default now()
);
create index login_failure_email_at_idx on login_failure (email, at);
```

Create `crates/db/build.rs`:

```rust
// `sqlx::migrate!` embeds the migrations at compile time; rebuild when one is added or changed.
fn main() {
    println!("cargo:rerun-if-changed=../../migrations");
}
```

Create `modules/identity/src/throttle.rs`:

```rust
use db::Scope;
use sqlx::PgPool;
use uuid::Uuid;

/// Sign-in attempts allowed per email within [`LOGIN_WINDOW_SECS`]; further attempts are refused until the
/// oldest of them is older than the window.
pub const MAX_LOGIN_FAILURES: i64 = 5;
pub const LOGIN_WINDOW_SECS: f64 = 15.0 * 60.0;

/// Reserves a sign-in attempt for `email`. Returns `false`, recording nothing, if [`MAX_LOGIN_FAILURES`]
/// attempts failed within the window. Otherwise the attempt counts as a failure until
/// [`clear_login_failures`] is called after a successful sign-in.
///
/// The attempt is recorded before the password is checked, under a per-email lock, so concurrent attempts
/// cannot exceed the limit.
pub async fn reserve_login_attempt(pool: &PgPool, email: &str) -> Result<bool, sqlx::Error> {
    let mut tx = db::begin(pool, Scope::default()).await?;
    sqlx::query("select pg_advisory_xact_lock(hashtextextended(lower($1::text), 0))")
        .bind(email)
        .execute(&mut *tx)
        .await?;
    let recent: i64 = sqlx::query_scalar(
        "select count(*) from login_failure where email = $1::citext and at > now() - make_interval(secs => $2)",
    )
    .bind(email)
    .bind(LOGIN_WINDOW_SECS)
    .fetch_one(&mut *tx)
    .await?;
    if recent >= MAX_LOGIN_FAILURES {
        tx.commit().await?;
        return Ok(false);
    }
    // Older attempts no longer count, so they are pruned here.
    sqlx::query("delete from login_failure where email = $1::citext and at <= now() - make_interval(secs => $2)")
        .bind(email)
        .bind(LOGIN_WINDOW_SECS)
        .execute(&mut *tx)
        .await?;
    sqlx::query("insert into login_failure (id, email) values ($1, $2::citext)")
        .bind(Uuid::now_v7())
        .bind(email)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(true)
}

/// Forgets `email`'s failed attempts, after a successful sign-in.
pub async fn clear_login_failures(pool: &PgPool, email: &str) -> Result<(), sqlx::Error> {
    sqlx::query("delete from login_failure where email = $1::citext").bind(email).execute(pool).await?;
    Ok(())
}
```

Modify `modules/identity/src/lib.rs`:

```diff
--- a/modules/identity/src/lib.rs
+++ b/modules/identity/src/lib.rs
@@ -4,6 +4,7 @@ mod account;
 mod password;
 mod rbac;
 mod session;
+mod throttle;
 
 pub use account::{
     Profile, SignupError, SignupInput, TenantSummary, authenticate, default_tenant, load_profile, signup,
@@ -11,3 +12,4 @@ pub use account::{
 pub use password::{hash_password, verify_password};
 pub use rbac::{Grant, Permission, Role, allows, load_grants};
 pub use session::{SESSION_TTL, SessionInfo, create_session, delete_session, resolve_session, switch_tenant};
+pub use throttle::{LOGIN_WINDOW_SECS, MAX_LOGIN_FAILURES, clear_login_failures, reserve_login_attempt};
```

Modify `crates/core-api/src/error.rs`:

```diff
--- a/crates/core-api/src/error.rs
+++ b/crates/core-api/src/error.rs
@@ -58,6 +58,10 @@ impl ApiError {
         Self::new(StatusCode::UNPROCESSABLE_ENTITY, "Invalid request", Some(detail.into()))
     }
 
+    pub fn too_many_requests(detail: impl Into<String>) -> Self {
+        Self::new(StatusCode::TOO_MANY_REQUESTS, "Too many requests", Some(detail.into()))
+    }
+
     pub fn gateway_timeout() -> Self {
         Self::new(StatusCode::GATEWAY_TIMEOUT, "Request timed out", None)
     }
```

Modify `crates/core-api/src/routes/auth.rs`:

```diff
--- a/crates/core-api/src/routes/auth.rs
+++ b/crates/core-api/src/routes/auth.rs
@@ -59,16 +59,22 @@ pub async fn signup(
     Ok((StatusCode::CREATED, jar.add(session_cookie(token, expires, state.production)), Json(profile)))
 }
 
+/// After 5 failed attempts for one email within 15 minutes, further attempts for that email are answered
+/// with 429 until the window passes, whether or not the account exists. A successful sign-in clears them.
 #[utoipa::path(post, path = "/api/v1/auth/login", request_body = LoginRequest,
-    responses((status = 200, body = Profile), (status = 401)))]
+    responses((status = 200, body = Profile), (status = 401), (status = 429)))]
 pub async fn login(
     State(state): State<AppState>,
     jar: CookieJar,
     ApiJson(body): ApiJson<LoginRequest>,
 ) -> Result<(CookieJar, Json<Profile>), ApiError> {
+    if !identity::reserve_login_attempt(&state.pool, &body.email).await? {
+        return Err(ApiError::too_many_requests("too many failed sign-in attempts; try again in 15 minutes"));
+    }
     let user = identity::authenticate(&state.pool, &body.email, &body.password)
         .await?
         .ok_or_else(ApiError::invalid_credentials)?;
+    identity::clear_login_failures(&state.pool, &body.email).await?;
     let tenant = identity::default_tenant(&state.pool, user).await?;
     let (token, expires) = identity::create_session(&state.pool, user, tenant).await?;
     let profile = identity::load_profile(&state.pool, user, tenant).await?;
```

Modify `docs/design/api-conventions.md`:

````diff
--- a/docs/design/api-conventions.md
+++ b/docs/design/api-conventions.md
@@ -43,7 +43,7 @@ Every non-GET/HEAD/OPTIONS request must send `x-goodfolk-csrf: 1` (`crates/core-
 { "type": "about:blank", "title": "Conflict", "status": 409, "detail": "a property with this code already exists" }
 ```
 
-`Content-Type: application/problem+json`, for every error the API returns, including malformed bodies and query strings, GraphQL request parse failures and timeouts. Construct errors with `ApiError::{bad_request, unauthenticated, invalid_credentials, forbidden, not_found, method_not_allowed, conflict, unprocessable, gateway_timeout, internal}`. Database errors are logged and become a bare 500, and internal details never reach the client.
+`Content-Type: application/problem+json`, for every error the API returns, including malformed bodies and query strings, GraphQL request parse failures and timeouts. Construct errors with `ApiError::{bad_request, unauthenticated, invalid_credentials, forbidden, not_found, method_not_allowed, conflict, unprocessable, too_many_requests, gateway_timeout, internal}`. Database errors are logged and become a bare 500, and internal details never reach the client.
 
 | Status | When |
 |---|---|
@@ -55,6 +55,7 @@ Every non-GET/HEAD/OPTIONS request must send `x-goodfolk-csrf: 1` (`crates/core-
 | 409 | Uniqueness conflict, double booking (exclusion violation), idempotent request still running |
 | 412 | `If-Match` version mismatch (from Phase 1) |
 | 422 | Body of the wrong shape (missing or mistyped field), validation failed (`garde`), business rule violated, idempotency key reused for a different request |
+| 429 | Sign-in throttled: 5 failed attempts for one email within 15 minutes (the same answer whether or not the account exists) |
 | 504 | Handler exceeded the 15 s request timeout (`routes::REQUEST_TIMEOUT`) |
 
 ## Validation
````

Modify `docs/design/data-model.md`:

```diff
--- a/docs/design/data-model.md
+++ b/docs/design/data-model.md
@@ -43,6 +43,10 @@ Related: [ARCHITECTURE.md](../ARCHITECTURE.md) (why), [api-conventions.md](api-c
 
 Extensions: `btree_gist` (needed by the exclusion constraints).
 
+| Global table | Key columns | Notes |
+|---|---|---|
+| `login_failure` | `id`, `email citext`, `at` | Sign-in throttling (`migrations/0003_login_throttle.sql`). No RLS and no `tenant_id`: looked up before a tenant is known. One row per attempt, written before the password is checked; a successful sign-in deletes the email's rows. The Phase 7 purge job deletes rows older than the window |
+
 ## Phase 2: Rates and meal plans
 
 | Table | Key columns | Constraints and indexes |
```

- [ ] **Step 4: Run the tests to verify they pass**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS (`auth`: 11). The schema guard still passes: `login_failure` has no `tenant_id`.

- [ ] **Step 5: Commit**

```sh
git add -A
git commit -m "feat(auth): throttle sign-in after five failed attempts per email in 15 minutes"
```

### Task 5: Rooms, blocks and inventory schema, with tenant isolation

Migration 0004 adds `btree_gist`, the property settings (`check_in_time`, `check_out_time`, `business_date`) and the six Phase 1 tables, all under forced RLS, with composite foreign keys that keep every row inside its tenant and property. `room_block_no_overlap` forbids overlapping active blocks on one room; `inventory_day` checks `out_of_order between 0 and physical`. Existing properties get their business date and the five default block reasons; `FORCE ROW LEVEL SECURITY` is lifted on `property` only for that backfill, because a non-superuser owner (Neon) would otherwise see no rows. `create_property` now sets the business date to the property's local today.

Each new table gets an isolation-suite case: another tenant sees none of its rows and cannot write one for the owner.

**Files:**
- Create: `migrations/0004_rooms_inventory.sql`
- Modify: `docs/design/data-model.md`
- Modify: `modules/property/src/lib.rs`
- Test: `crates/db/tests/rooms_schema.rs` (new)
- Test: `crates/db/tests/isolation.rs`

**Interfaces:**
- Consumes: `migrations/0001_foundation.sql` (`app.current_tenant()`, the tenant-isolation policy shape).
- Produces: tables `room_type`, `housekeeping_section`, `room`, `block_reason`, `room_block`, `inventory_day`; `property.{check_in_time, check_out_time, business_date}`; constraint names relied on later: `room_type_property_id_code_key`, `housekeeping_section_property_id_name_key`, `room_property_id_number_key`, `block_reason_property_id_code_key`, `room_block_no_overlap`.

- [ ] **Step 1: Write the failing tests**

`seed_tenant` now sets `business_date`, which becomes required.

Modify `crates/db/tests/isolation.rs`:

```diff
--- a/crates/db/tests/isolation.rs
+++ b/crates/db/tests/isolation.rs
@@ -2,8 +2,8 @@
 
 use db::testing::app_pool;
 use db::{Scope, TenantId, UserId, begin};
-use sqlx::PgPool;
 use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
+use sqlx::{AssertSqlSafe, PgPool};
 use uuid::Uuid;
 
 async fn seed_tenant(pool: &PgPool, name: &str) -> TenantId {
@@ -16,8 +16,8 @@ async fn seed_tenant(pool: &PgPool, name: &str) -> TenantId {
         .await
         .unwrap();
     sqlx::query(
-        "insert into property (id, tenant_id, code, name, timezone, base_currency)
-         values ($1, $2, 'MAIN', $3, 'Asia/Colombo', 'LKR')",
+        "insert into property (id, tenant_id, code, name, timezone, base_currency, business_date)
+         values ($1, $2, 'MAIN', $3, 'Asia/Colombo', 'LKR', current_date)",
     )
     .bind(Uuid::now_v7())
     .bind(tenant.0)
@@ -63,8 +63,8 @@ async fn writing_into_another_tenant_is_rejected(_: PgPoolOptions, opts: PgConne
 
     let mut tx = begin(&pool, Scope::tenant(a)).await.unwrap();
     let result = sqlx::query(
-        "insert into property (id, tenant_id, code, name, timezone, base_currency)
-         values ($1, $2, 'X1', 'Intruder', 'Asia/Colombo', 'LKR')",
+        "insert into property (id, tenant_id, code, name, timezone, base_currency, business_date)
+         values ($1, $2, 'X1', 'Intruder', 'Asia/Colombo', 'LKR', current_date)",
     )
     .bind(Uuid::now_v7())
     .bind(b.0)
@@ -213,3 +213,238 @@ async fn idempotency_keys_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConne
     let err = result.unwrap_err().to_string();
     assert!(err.contains("row-level security"), "unexpected error: {err}");
 }
+
+/// One row in every Phase 1 table, for tenant `a`'s property.
+struct RoomsSeed {
+    property: Uuid,
+    room_type: Uuid,
+    section: Uuid,
+    room: Uuid,
+    reason: Uuid,
+}
+
+async fn seed_rooms(pool: &PgPool, tenant: TenantId) -> RoomsSeed {
+    let mut tx = begin(pool, Scope::tenant(tenant)).await.unwrap();
+    let property: Uuid = sqlx::query_scalar("select id from property").fetch_one(&mut *tx).await.unwrap();
+    let seed = RoomsSeed {
+        property,
+        room_type: Uuid::now_v7(),
+        section: Uuid::now_v7(),
+        room: Uuid::now_v7(),
+        reason: Uuid::now_v7(),
+    };
+    sqlx::query(
+        "insert into room_type (id, tenant_id, property_id, code, name, base_occupancy, max_adults, max_children, max_occupancy)
+         values ($1, $2, $3, 'DLX', 'Deluxe', 2, 2, 1, 3)",
+    )
+    .bind(seed.room_type)
+    .bind(tenant.0)
+    .bind(property)
+    .execute(&mut *tx)
+    .await
+    .unwrap();
+    sqlx::query("insert into housekeeping_section (id, tenant_id, property_id, name) values ($1, $2, $3, 'East')")
+        .bind(seed.section)
+        .bind(tenant.0)
+        .bind(property)
+        .execute(&mut *tx)
+        .await
+        .unwrap();
+    sqlx::query(
+        "insert into room (id, tenant_id, property_id, room_type_id, number, section_id) values ($1, $2, $3, $4, '101', $5)",
+    )
+    .bind(seed.room)
+    .bind(tenant.0)
+    .bind(property)
+    .bind(seed.room_type)
+    .bind(seed.section)
+    .execute(&mut *tx)
+    .await
+    .unwrap();
+    sqlx::query(
+        "insert into block_reason (id, tenant_id, property_id, code, label, default_kind)
+         values ($1, $2, $3, 'LEAK', 'Leak', 'out_of_order')",
+    )
+    .bind(seed.reason)
+    .bind(tenant.0)
+    .bind(property)
+    .execute(&mut *tx)
+    .await
+    .unwrap();
+    sqlx::query(
+        "insert into room_block (id, tenant_id, property_id, room_id, period, kind, reason_id)
+         values ($1, $2, $3, $4, daterange(current_date, current_date + 3), 'out_of_order', $5)",
+    )
+    .bind(Uuid::now_v7())
+    .bind(tenant.0)
+    .bind(property)
+    .bind(seed.room)
+    .bind(seed.reason)
+    .execute(&mut *tx)
+    .await
+    .unwrap();
+    sqlx::query(
+        "insert into inventory_day (tenant_id, property_id, room_type_id, date, physical) values ($1, $2, $3, current_date, 1)",
+    )
+    .bind(tenant.0)
+    .bind(property)
+    .bind(seed.room_type)
+    .execute(&mut *tx)
+    .await
+    .unwrap();
+    tx.commit().await.unwrap();
+    seed
+}
+
+/// How many rows of `table` tenant `viewer` can see.
+async fn visible_rows(pool: &PgPool, viewer: TenantId, table: &str) -> i64 {
+    let mut tx = begin(pool, Scope::tenant(viewer)).await.unwrap();
+    sqlx::query_scalar(AssertSqlSafe(format!("select count(*) from {table}"))).fetch_one(&mut *tx).await.unwrap()
+}
+
+/// Runs `insert` (with `$1` bound to the owning tenant) as tenant `writer`, and returns the error message.
+async fn foreign_insert_error(pool: &PgPool, writer: TenantId, owner: TenantId, insert: &'static str) -> String {
+    let mut tx = begin(pool, Scope::tenant(writer)).await.unwrap();
+    sqlx::query(insert).bind(owner.0).execute(&mut *tx).await.unwrap_err().to_string()
+}
+
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn room_types_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
+    let pool = app_pool(opts, 1).await;
+    let a = seed_tenant(&pool, "A").await;
+    let b = seed_tenant(&pool, "B").await;
+    seed_rooms(&pool, a).await;
+
+    let seen = visible_rows(&pool, b, "room_type").await;
+    let err = foreign_insert_error(
+        &pool,
+        b,
+        a,
+        "insert into room_type (id, tenant_id, property_id, code, name, base_occupancy, max_adults, max_children, max_occupancy)
+         select gen_random_uuid(), $1, id, 'STD', 'Standard', 1, 1, 0, 1 from property limit 1",
+    )
+    .await;
+
+    assert_eq!(visible_rows(&pool, a, "room_type").await, 1);
+    assert_eq!(seen, 0);
+    assert!(err.contains("row-level security"), "unexpected error: {err}");
+}
+
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn housekeeping_sections_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
+    let pool = app_pool(opts, 1).await;
+    let a = seed_tenant(&pool, "A").await;
+    let b = seed_tenant(&pool, "B").await;
+    seed_rooms(&pool, a).await;
+
+    let seen = visible_rows(&pool, b, "housekeeping_section").await;
+    let err = foreign_insert_error(
+        &pool,
+        b,
+        a,
+        "insert into housekeeping_section (id, tenant_id, property_id, name)
+         select gen_random_uuid(), $1, id, 'West' from property limit 1",
+    )
+    .await;
+
+    assert_eq!(visible_rows(&pool, a, "housekeeping_section").await, 1);
+    assert_eq!(seen, 0);
+    assert!(err.contains("row-level security"), "unexpected error: {err}");
+}
+
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn rooms_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
+    let pool = app_pool(opts, 1).await;
+    let a = seed_tenant(&pool, "A").await;
+    let b = seed_tenant(&pool, "B").await;
+    seed_rooms(&pool, a).await;
+    let theirs = seed_rooms(&pool, b).await;
+
+    let seen = visible_rows(&pool, b, "room").await;
+    let mut tx = begin(&pool, Scope::tenant(b)).await.unwrap();
+    let err = sqlx::query(
+        "insert into room (id, tenant_id, property_id, room_type_id, number) values (gen_random_uuid(), $1, $2, $3, '102')",
+    )
+    .bind(a.0)
+    .bind(theirs.property)
+    .bind(theirs.room_type)
+    .execute(&mut *tx)
+    .await
+    .unwrap_err()
+    .to_string();
+
+    assert_eq!(seen, 1, "B sees only its own room");
+    assert!(err.contains("row-level security"), "unexpected error: {err}");
+}
+
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn block_reasons_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
+    let pool = app_pool(opts, 1).await;
+    let a = seed_tenant(&pool, "A").await;
+    let b = seed_tenant(&pool, "B").await;
+    seed_rooms(&pool, a).await;
+
+    let seen = visible_rows(&pool, b, "block_reason").await;
+    let err = foreign_insert_error(
+        &pool,
+        b,
+        a,
+        "insert into block_reason (id, tenant_id, property_id, code, label, default_kind)
+         select gen_random_uuid(), $1, id, 'PAINT', 'Painting', 'out_of_order' from property limit 1",
+    )
+    .await;
+
+    assert_eq!(seen, 0);
+    assert!(err.contains("row-level security"), "unexpected error: {err}");
+}
+
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn room_blocks_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
+    let pool = app_pool(opts, 1).await;
+    let a = seed_tenant(&pool, "A").await;
+    let b = seed_tenant(&pool, "B").await;
+    seed_rooms(&pool, a).await;
+    let theirs = seed_rooms(&pool, b).await;
+
+    let seen = visible_rows(&pool, b, "room_block").await;
+    let mut tx = begin(&pool, Scope::tenant(b)).await.unwrap();
+    let err = sqlx::query(
+        "insert into room_block (id, tenant_id, property_id, room_id, period, kind, reason_id)
+         values (gen_random_uuid(), $1, $2, $3, daterange(current_date + 10, current_date + 12), 'out_of_order', $4)",
+    )
+    .bind(a.0)
+    .bind(theirs.property)
+    .bind(theirs.room)
+    .bind(theirs.reason)
+    .execute(&mut *tx)
+    .await
+    .unwrap_err()
+    .to_string();
+
+    assert_eq!(seen, 1, "B sees only its own block");
+    assert!(err.contains("row-level security"), "unexpected error: {err}");
+}
+
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn inventory_days_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
+    let pool = app_pool(opts, 1).await;
+    let a = seed_tenant(&pool, "A").await;
+    let b = seed_tenant(&pool, "B").await;
+    let ours = seed_rooms(&pool, a).await;
+
+    let seen = visible_rows(&pool, b, "inventory_day").await;
+    let mut tx = begin(&pool, Scope::tenant(b)).await.unwrap();
+    let err = sqlx::query(
+        "insert into inventory_day (tenant_id, property_id, room_type_id, date) values ($1, $2, $3, current_date + 1)",
+    )
+    .bind(a.0)
+    .bind(ours.property)
+    .bind(ours.room_type)
+    .execute(&mut *tx)
+    .await
+    .unwrap_err()
+    .to_string();
+
+    assert_eq!(seen, 0);
+    assert!(err.contains("row-level security"), "unexpected error: {err}");
+}
```

Create `crates/db/tests/rooms_schema.rs`:

```rust
//! Constraints the Phase 1 migration puts on rooms and blocks, checked directly in the database.

use sqlx::PgPool;
use uuid::Uuid;

struct Hotel {
    tenant: Uuid,
    property: Uuid,
    room_type: Uuid,
    room: Uuid,
    reason: Uuid,
}

/// A tenant with one property, room type, room and block reason. Runs as the superuser (no RLS).
async fn hotel(pool: &PgPool, code: &str) -> Hotel {
    let hotel = Hotel {
        tenant: Uuid::now_v7(),
        property: Uuid::now_v7(),
        room_type: Uuid::now_v7(),
        room: Uuid::now_v7(),
        reason: Uuid::now_v7(),
    };
    sqlx::query("insert into tenant (id, name) values ($1, 'T')").bind(hotel.tenant).execute(pool).await.unwrap();
    sqlx::query(
        "insert into property (id, tenant_id, code, name, timezone, base_currency, business_date)
         values ($1, $2, $3, 'Hotel', 'Asia/Colombo', 'LKR', current_date)",
    )
    .bind(hotel.property)
    .bind(hotel.tenant)
    .bind(code)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "insert into room_type (id, tenant_id, property_id, code, name, base_occupancy, max_adults, max_children, max_occupancy)
         values ($1, $2, $3, 'DLX', 'Deluxe', 2, 2, 0, 2)",
    )
    .bind(hotel.room_type)
    .bind(hotel.tenant)
    .bind(hotel.property)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("insert into room (id, tenant_id, property_id, room_type_id, number) values ($1, $2, $3, $4, '101')")
        .bind(hotel.room)
        .bind(hotel.tenant)
        .bind(hotel.property)
        .bind(hotel.room_type)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "insert into block_reason (id, tenant_id, property_id, code, label, default_kind)
         values ($1, $2, $3, 'LEAK', 'Leak', 'out_of_order')",
    )
    .bind(hotel.reason)
    .bind(hotel.tenant)
    .bind(hotel.property)
    .execute(pool)
    .await
    .unwrap();
    hotel
}

/// Blocks `hotel`'s room for `[today + from, today + to)`.
async fn block(pool: &PgPool, hotel: &Hotel, from: i32, to: i32) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::now_v7();
    sqlx::query(
        "insert into room_block (id, tenant_id, property_id, room_id, period, kind, reason_id)
         values ($1, $2, $3, $4, daterange(current_date + $5, current_date + $6), 'out_of_order', $7)",
    )
    .bind(id)
    .bind(hotel.tenant)
    .bind(hotel.property)
    .bind(hotel.room)
    .bind(from)
    .bind(to)
    .bind(hotel.reason)
    .execute(pool)
    .await?;
    Ok(id)
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn overlapping_active_blocks_on_one_room_are_rejected(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;
    let first = block(&pool, &hotel, 0, 5).await.unwrap();

    let overlapping = block(&pool, &hotel, 4, 6).await.unwrap_err();
    let adjacent = block(&pool, &hotel, 5, 7).await;
    sqlx::query("update room_block set released_at = now() where id = $1").bind(first).execute(&pool).await.unwrap();
    let after_release = block(&pool, &hotel, 2, 4).await;

    let constraint = overlapping.as_database_error().and_then(|e| e.constraint()).map(str::to_owned);
    assert_eq!(constraint.as_deref(), Some("room_block_no_overlap"));
    assert!(adjacent.is_ok(), "[5, 7) touches [0, 5) without overlapping: {adjacent:?}");
    assert!(after_release.is_ok(), "{after_release:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_room_cannot_use_another_propertys_room_type(pool: PgPool) {
    let galle = hotel(&pool, "GAL").await;
    let kandy = hotel(&pool, "KAN").await;

    let result = sqlx::query(
        "insert into room (id, tenant_id, property_id, room_type_id, number) values ($1, $2, $3, $4, '102')",
    )
    .bind(Uuid::now_v7())
    .bind(galle.tenant)
    .bind(galle.property)
    .bind(kandy.room_type)
    .execute(&pool)
    .await;

    let err = result.unwrap_err();
    assert_eq!(err.as_database_error().and_then(|e| e.code()).as_deref(), Some("23503"), "{err}");
}

#[sqlx::test(migrations = false)]
async fn existing_properties_get_a_business_date_and_block_reasons(pool: PgPool) {
    db::MIGRATOR.run_to(3, &pool).await.unwrap();
    let tenant = Uuid::now_v7();
    let property = Uuid::now_v7();
    sqlx::query("insert into tenant (id, name) values ($1, 'T')").bind(tenant).execute(&pool).await.unwrap();
    sqlx::query(
        "insert into property (id, tenant_id, code, name, timezone, base_currency)
         values ($1, $2, 'GAL', 'Galle', 'Pacific/Kiritimati', 'LKR')",
    )
    .bind(property)
    .bind(tenant)
    .execute(&pool)
    .await
    .unwrap();

    db::MIGRATOR.run(&pool).await.unwrap();

    let (business_date_is_local_today, check_in, check_out): (bool, String, String) = sqlx::query_as(
        "select business_date = (now() at time zone 'Pacific/Kiritimati')::date,
                to_char(check_in_time, 'HH24:MI'), to_char(check_out_time, 'HH24:MI')
         from property where id = $1",
    )
    .bind(property)
    .fetch_one(&pool)
    .await
    .unwrap();
    let reasons: Vec<(String, String)> =
        sqlx::query_as("select code, default_kind from block_reason where property_id = $1 order by code")
            .bind(property)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert!(business_date_is_local_today);
    assert_eq!((check_in.as_str(), check_out.as_str()), ("14:00", "12:00"));
    assert_eq!(
        reasons,
        [
            ("CONSTRUCTION", "out_of_order"),
            ("DEEP_CLEAN", "out_of_service"),
            ("MAINTENANCE", "out_of_order"),
            ("OTHER", "out_of_order"),
            ("RENOVATION", "out_of_order"),
        ]
        .map(|(code, kind)| (code.to_owned(), kind.to_owned()))
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p db --no-fail-fast
```

Expected: `isolation` FAILS (every test: `column "business_date" of relation "property" does not exist`); `rooms_schema` FAILS (the tables do not exist; the backfill test finds no migration 4).

- [ ] **Step 3: Implement**

Create `migrations/0004_rooms_inventory.sql`:

```sql
-- Phase 1: room types, housekeeping sections, rooms, block reasons, room blocks and inventory counters.
create extension if not exists btree_gist;

-- Property settings used from Phase 3 on. business_date is the property's local "today" when it is created;
-- only night audit (Phase 7) moves it.
alter table property
  add column check_in_time time not null default '14:00',
  add column check_out_time time not null default '12:00',
  add column business_date date;
-- Lets child rows reference (tenant_id, property_id), so a row can never point at another tenant's property.
alter table property add constraint property_tenant_id_id_key unique (tenant_id, id);

create table room_type (
  id uuid primary key,
  tenant_id uuid not null,
  property_id uuid not null,
  code text not null check (code ~ '^[A-Z0-9]{1,10}$'),
  name text not null check (length(name) between 1 and 100),
  base_occupancy integer not null check (base_occupancy between 1 and 50),
  max_adults integer not null check (max_adults between 1 and 50),
  max_children integer not null check (max_children between 0 and 50),
  max_occupancy integer not null check (max_occupancy between 1 and 50),
  bed_config jsonb not null default '[]',
  amenities text[] not null default '{}',
  sort_order integer not null default 0,
  active boolean not null default true,
  version integer not null default 1,
  created_at timestamptz not null default now(),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade,
  unique (property_id, code),
  unique (property_id, id),
  check (base_occupancy <= max_occupancy),
  check (max_adults <= max_occupancy),
  check (max_occupancy <= max_adults + max_children)
);

create table housekeeping_section (
  id uuid primary key,
  tenant_id uuid not null,
  property_id uuid not null,
  name text not null check (length(name) between 1 and 100),
  version integer not null default 1,
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade,
  unique (property_id, name),
  unique (property_id, id)
);

-- Composite foreign keys keep a room's type and section in the room's own property.
create table room (
  id uuid primary key,
  tenant_id uuid not null,
  property_id uuid not null,
  room_type_id uuid not null,
  number text not null check (number ~ '^[A-Za-z0-9-]{1,10}$'),
  floor text check (length(floor) between 1 and 20),
  section_id uuid,
  active boolean not null default true,
  sort_order integer not null default 0,
  version integer not null default 1,
  created_at timestamptz not null default now(),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade,
  foreign key (property_id, room_type_id) references room_type (property_id, id),
  foreign key (property_id, section_id) references housekeeping_section (property_id, id),
  unique (property_id, number),
  unique (property_id, id)
);
create index room_room_type_idx on room (room_type_id);

create table block_reason (
  id uuid primary key,
  tenant_id uuid not null,
  property_id uuid not null,
  code text not null check (code ~ '^[A-Z0-9_]{1,20}$'),
  label text not null check (length(label) between 1 and 100),
  default_kind text not null check (default_kind in ('out_of_order', 'out_of_service')),
  active boolean not null default true,
  version integer not null default 1,
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade,
  unique (property_id, code),
  unique (property_id, id)
);

-- A room out of order (removed from inventory) or out of service (still sellable, flagged) for [from, to).
-- released_at is set when a block is cancelled before it starts; shortening a block moves upper(period).
create table room_block (
  id uuid primary key,
  tenant_id uuid not null,
  property_id uuid not null,
  room_id uuid not null,
  period daterange not null check (not isempty(period) and not lower_inf(period) and not upper_inf(period)),
  kind text not null check (kind in ('out_of_order', 'out_of_service')),
  reason_id uuid not null,
  note text not null default '' check (length(note) <= 500),
  created_by uuid references app_user (id) on delete set null,
  created_at timestamptz not null default now(),
  released_at timestamptz,
  version integer not null default 1,
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade,
  foreign key (property_id, room_id) references room (property_id, id) on delete cascade,
  foreign key (property_id, reason_id) references block_reason (property_id, id),
  constraint room_block_no_overlap exclude using gist (room_id with =, period with &&) where (released_at is null)
);
create index room_block_property_period_idx on room_block using gist (property_id, period);

-- One counter row per room type per day, from the business date to 730 days ahead, kept in step with rooms
-- and blocks in the same transaction. available = physical - sold - out_of_order.
create table inventory_day (
  tenant_id uuid not null,
  property_id uuid not null,
  room_type_id uuid not null,
  date date not null,
  physical integer not null default 0 check (physical >= 0),
  sold integer not null default 0 check (sold >= 0),
  out_of_order integer not null default 0 check (out_of_order between 0 and physical),
  primary key (property_id, room_type_id, date),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade,
  foreign key (property_id, room_type_id) references room_type (property_id, id) on delete cascade
);
create index inventory_day_property_date_idx on inventory_day (property_id, date);

-- Backfill existing properties. FORCE ROW LEVEL SECURITY applies policies to the table owner too, so it is
-- lifted while the owner running this migration updates every tenant's rows. The seeded ids are random
-- (Postgres 17 has no UUIDv7 function); new rows get UUIDv7 ids from Rust.
alter table property no force row level security;
update property set business_date = (now() at time zone timezone)::date;
insert into block_reason (id, tenant_id, property_id, code, label, default_kind)
select gen_random_uuid(), p.tenant_id, p.id, r.code, r.label, r.kind
from property p
cross join (values
  ('RENOVATION', 'Renovation', 'out_of_order'),
  ('CONSTRUCTION', 'Construction', 'out_of_order'),
  ('MAINTENANCE', 'Maintenance', 'out_of_order'),
  ('DEEP_CLEAN', 'Deep clean', 'out_of_service'),
  ('OTHER', 'Other', 'out_of_order')
) as r (code, label, kind);
alter table property force row level security;
alter table property alter column business_date set not null;

do $$
declare t text;
begin
  foreach t in array array['room_type', 'housekeeping_section', 'room', 'block_reason', 'room_block', 'inventory_day'] loop
    execute format('alter table %I enable row level security', t);
    execute format('alter table %I force row level security', t);
    execute format(
      'create policy tenant_isolation on %I for all using (tenant_id = (select app.current_tenant())) with check (tenant_id = (select app.current_tenant()))',
      t);
  end loop;
end $$;
```

Modify `modules/property/src/lib.rs`:

```diff
--- a/modules/property/src/lib.rs
+++ b/modules/property/src/lib.rs
@@ -49,8 +49,8 @@ pub async fn create_property(
         return Err(PropertyError::UnknownTimezone);
     }
     let inserted = sqlx::query_as::<_, Property>(
-        "insert into property (id, tenant_id, code, name, timezone, base_currency)
-         values ($1, $2, $3, $4, $5, $6)
+        "insert into property (id, tenant_id, code, name, timezone, base_currency, business_date)
+         values ($1, $2, $3, $4, $5, $6, (now() at time zone $5)::date)
          returning id, code, name, timezone, base_currency, version",
     )
     .bind(Uuid::now_v7())
```

Modify `docs/design/data-model.md`:

```diff
--- a/docs/design/data-model.md
+++ b/docs/design/data-model.md
@@ -1,6 +1,6 @@
 # Data Model
 
-The reference schema for every phase. Phase 0 tables exist in `migrations/0001_foundation.sql`. Tables for later phases are the target design: the migrations that create them are written in their phase and may refine columns, but must keep the rules below.
+The reference schema for every phase. Phase 0 tables exist in `migrations/0001_foundation.sql`, Phase 1 tables in `migrations/0003_login_throttle.sql` and `migrations/0004_rooms_inventory.sql`. Tables for later phases are the target design: the migrations that create them are written in their phase and may refine columns, but must keep the rules below.
 
 Related: [ARCHITECTURE.md](../ARCHITECTURE.md) (why), [api-conventions.md](api-conventions.md) (how data leaves the API).
 
@@ -28,18 +28,20 @@ Related: [ARCHITECTURE.md](../ARCHITECTURE.md) (why), [api-conventions.md](api-c
 | `audit_log` | `id`, `tenant_id`, `actor_user_id`, `action`, `entity`, `entity_id`, `data jsonb`, `at` | Append-only (no update/delete grant) |
 | `idempotency_key` | `(tenant_id, key)`, `request_hash`, `status_code`, `response_body` | Replay store for create commands. A cleanup job (Phase 7 `jobs-svc`) deletes keys older than 7 days |
 
-## Phase 1: Rooms and inventory
+## Phase 1: Rooms and inventory (implemented)
 
-`property` gains: `check_in_time time`, `check_out_time time`, `business_date date not null` (set to the property's local today when created; moved only by night audit).
+Created by `migrations/0004_rooms_inventory.sql`. Every table below carries `tenant_id` and references its property through `(tenant_id, property_id)`, and a room's type, section, blocks and reasons through `(property_id, id)`, so a row can never point into another tenant or property.
+
+`property` gains: `check_in_time time not null default '14:00'`, `check_out_time time not null default '12:00'`, `business_date date not null` (set to the property's local today when created; moved only by night audit).
 
 | Table | Key columns | Constraints and indexes |
 |---|---|---|
-| `room_type` | `id`, `tenant_id`, `property_id`, `code`, `name`, `base_occupancy`, `max_adults`, `max_children`, `max_occupancy`, `bed_config jsonb`, `amenities text[]`, `sort_order`, `active`, `version` | `unique (property_id, code)`; `check (base_occupancy <= max_occupancy)` |
-| `housekeeping_section` | `id`, `tenant_id`, `property_id`, `name` | `unique (property_id, name)` |
-| `room` | `id`, `tenant_id`, `property_id`, `room_type_id`, `number text`, `floor text`, `section_id null`, `active`, `sort_order`, `version` | `unique (property_id, number)` |
-| `block_reason` | `id`, `tenant_id`, `property_id`, `code`, `label`, `default_kind` | Seeded per property: `RENOVATION`, `CONSTRUCTION`, `MAINTENANCE`, `DEEP_CLEAN`, `OTHER` |
-| `room_block` | `id`, `tenant_id`, `property_id`, `room_id`, `period daterange`, `kind` (`out_of_order` \| `out_of_service`), `reason_id`, `note`, `created_by`, `released_at null` | `exclude using gist (room_id with =, period with &&) where (released_at is null)`; GiST `(property_id, period)` |
-| `inventory_day` | `(property_id, room_type_id, date)`, `tenant_id`, `physical`, `sold`, `out_of_order` | Counters updated in the same transaction as reservations and blocks. `available = physical − sold − out_of_order`. A nightly job recomputes them and alerts on drift |
+| `room_type` | `id`, `tenant_id`, `property_id`, `code`, `name`, `base_occupancy`, `max_adults`, `max_children`, `max_occupancy`, `bed_config jsonb` (`[{kind, count}]`), `amenities text[]`, `sort_order`, `active`, `version` | `unique (property_id, code)`; `check (base_occupancy <= max_occupancy)`, `max_adults <= max_occupancy <= max_adults + max_children` |
+| `housekeeping_section` | `id`, `tenant_id`, `property_id`, `name`, `version` | `unique (property_id, name)` |
+| `room` | `id`, `tenant_id`, `property_id`, `room_type_id`, `number text`, `floor text null`, `section_id null`, `active`, `sort_order`, `version` | `unique (property_id, number)` |
+| `block_reason` | `id`, `tenant_id`, `property_id`, `code`, `label`, `default_kind`, `active`, `version` | `unique (property_id, code)`. Seeded per property: `RENOVATION`, `CONSTRUCTION`, `MAINTENANCE`, `OTHER` (out of order), `DEEP_CLEAN` (out of service) |
+| `room_block` | `id`, `tenant_id`, `property_id`, `room_id`, `period daterange`, `kind` (`out_of_order` \| `out_of_service`), `reason_id`, `note`, `created_by`, `released_at null`, `version` | `room_block_no_overlap: exclude using gist (room_id with =, period with &&) where (released_at is null)`; GiST `(property_id, period)`. `released_at` marks a block cancelled before it started; shortening moves `upper(period)` |
+| `inventory_day` | `(property_id, room_type_id, date)`, `tenant_id`, `physical`, `sold`, `out_of_order` | `check (out_of_order between 0 and physical)`; index `(property_id, date)`. Counters updated in the same transaction as rooms, blocks and (from Phase 3) reservations. `available = physical − sold − out_of_order`. Rows exist from the business date for 730 days. A nightly job (Phase 7) recomputes them and alerts on drift (`rooms::find_drift`) |
 
 Extensions: `btree_gist` (needed by the exclusion constraints).
 
```

- [ ] **Step 4: Run the tests to verify they pass**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS (`isolation`: 14, `rooms_schema`: 3, `schema`: 2 — every new table has forced RLS).

- [ ] **Step 5: Commit**

```sh
git add -A
git commit -m "feat(db): rooms, blocks and inventory schema with tenant isolation"
```

### Task 6: Property settings with If-Match and ETag

The first update endpoint brings optimistic concurrency. `concurrency::IfMatch` extracts `If-Match: "<version>"` (missing → 428, malformed → 400) and `concurrency::Versioned` answers with the body and `ETag: "<version>"`. `property::update_property` updates `where id = $1 and version = $2`; no row means 412 if the property exists, 404 if not. `Property` now carries `check_in_time`, `check_out_time` (`HH:MM`), `business_date` and `version`, over REST and GraphQL. Dates serialize as `YYYY-MM-DD` through `time`'s `serde-human-readable` feature. Path parameters use a new `ApiPath` extractor, so a malformed id is a problem+json 400 like every other rejection.

A `TestApp::staff` helper makes a second user a member of the owner's tenant with a given role (invitations are Phase 8), for permission tests from here on.

**Files:**
- Create: `crates/core-api/src/concurrency.rs`
- Modify: `Cargo.toml`
- Modify: `crates/core-api/src/error.rs`
- Modify: `crates/core-api/src/extract.rs`
- Modify: `crates/core-api/src/graphql.rs`
- Modify: `crates/core-api/src/lib.rs`
- Modify: `crates/core-api/src/openapi.rs`
- Modify: `crates/core-api/src/routes/mod.rs`
- Modify: `crates/core-api/src/routes/properties.rs`
- Modify: `docs/design/api-conventions.md`
- Modify: `modules/property/Cargo.toml`
- Modify: `modules/property/src/lib.rs`
- Modify: `web/pms/codegen.ts`
- Test: `crates/core-api/tests/common/mod.rs`
- Test: `crates/core-api/tests/graphql.rs`
- Test: `crates/core-api/tests/openapi.rs`
- Test: `crates/core-api/tests/properties.rs`
- Regenerated, not shown: `Cargo.lock`, `web/pms/src/lib/api/openapi.d.ts`, `web/pms/src/lib/api/openapi.json`, `web/pms/src/lib/api/schema.graphql`

**Interfaces:**
- Consumes: `ApiJson`, `TenantContext::require`, `Permission::PropertiesManage`.
- Produces: `concurrency::{IfMatch(pub i32), Versioned::{ok, created}(version, body)}`; `extract::ApiPath<T>`; `ApiError::{precondition_failed(detail), precondition_required()}`; `property::{Property { …, check_in_time: String, check_out_time: String, business_date: time::Date, version }, PropertyChanges, update_property(tx, tenant, actor, id, expected_version, PropertyChanges) -> Result<Property, PropertyError>}`, `PropertyError::{NotFound, VersionMismatch}`; `PATCH /api/v1/properties/{property}`; GraphQL `PropertyNode.{checkInTime, checkOutTime, businessDate, version}`; tests: `TestApp::staff(&superuser, owner_cookie, email, role) -> cookie`, `common::uuid(&Value) -> Uuid`.

- [ ] **Step 1: Write the failing tests**

Modify `crates/core-api/tests/common/mod.rs`:

```diff
--- a/crates/core-api/tests/common/mod.rs
+++ b/crates/core-api/tests/common/mod.rs
@@ -112,6 +112,40 @@ impl TestApp {
     }
 }
 
+impl TestApp {
+    /// Signs up `email` and makes them `role` in the tenant of `owner_cookie`, with their session switched to it.
+    /// Staff invitations arrive in Phase 8; until then the membership is written directly as `superuser`.
+    pub async fn staff(&self, superuser: &PgPool, owner_cookie: &str, email: &str, role: &str) -> String {
+        let cookie = self.signup_owner(email, "Own tenant").await;
+        let user = self.send(Method::GET, "/api/v1/me", Some(&cookie), None).await.body["user_id"].clone();
+        let tenant =
+            self.send(Method::GET, "/api/v1/me", Some(owner_cookie), None).await.body["current_tenant"].clone();
+        let (user, tenant) = (uuid(&user), uuid(&tenant));
+        sqlx::query("insert into membership (tenant_id, user_id) values ($1, $2)")
+            .bind(tenant)
+            .bind(user)
+            .execute(superuser)
+            .await
+            .unwrap();
+        sqlx::query("insert into role_grant (id, tenant_id, user_id, role) values ($1, $2, $3, $4)")
+            .bind(uuid::Uuid::now_v7())
+            .bind(tenant)
+            .bind(user)
+            .bind(role)
+            .execute(superuser)
+            .await
+            .unwrap();
+        let switched =
+            self.send(Method::PUT, "/api/v1/session/tenant", Some(&cookie), Some(json!({"tenant_id": tenant}))).await;
+        assert_eq!(switched.status, StatusCode::OK, "{:?}", switched.body);
+        cookie
+    }
+}
+
+pub fn uuid(value: &Value) -> uuid::Uuid {
+    uuid::Uuid::parse_str(value.as_str().expect("a UUID string")).unwrap()
+}
+
 pub fn session_cookie(headers: &HeaderMap) -> String {
     let set_cookie = headers.get(header::SET_COOKIE).expect("Set-Cookie header").to_str().unwrap();
     set_cookie.split(';').next().unwrap().to_owned()
```

Modify `crates/core-api/tests/properties.rs`:

```diff
--- a/crates/core-api/tests/properties.rs
+++ b/crates/core-api/tests/properties.rs
@@ -152,3 +152,85 @@ async fn after_switching_tenant_a_create_lands_in_the_new_tenant(_: PgPoolOption
         sqlx::query_scalar("select tenant_id from property where code = 'GAL'").fetch_one(&superuser).await.unwrap();
     assert_eq!(owner, bobs_tenant);
 }
+
+async fn patch(app: &TestApp, cookie: &str, path: &str, if_match: Option<&str>, body: Value) -> TestResponse {
+    let mut headers = vec![("x-goodfolk-csrf", "1")];
+    if let Some(version) = if_match {
+        headers.push(("if-match", version));
+    }
+    app.send_with(Method::PATCH, path, Some(cookie), Some(body), &headers).await
+}
+
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn property_settings_are_updated_with_if_match(_: PgPoolOptions, opts: PgConnectOptions) {
+    let app = TestApp::new(opts).await;
+    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
+    let created = create(&app, &owner, "key-00000001", galle()).await;
+    let path = format!("/api/v1/properties/{}", created.body["id"].as_str().unwrap());
+
+    let updated =
+        patch(&app, &owner, &path, Some("\"1\""), json!({"check_in_time": "15:00", "name": "Galle Fort"})).await;
+
+    assert_eq!(created.headers[header::ETAG], "\"1\"");
+    assert_eq!(created.body["check_in_time"], "14:00");
+    assert_eq!(created.body["check_out_time"], "12:00");
+    assert_eq!(created.body["business_date"].as_str().unwrap().len(), "2026-09-24".len());
+    assert_eq!(updated.status, StatusCode::OK, "{:?}", updated.body);
+    assert_eq!(updated.headers[header::ETAG], "\"2\"");
+    assert_eq!(updated.body["version"], 2);
+    assert_eq!(updated.body["check_in_time"], "15:00");
+    assert_eq!(updated.body["check_out_time"], "12:00");
+    assert_eq!(updated.body["name"], "Galle Fort");
+}
+
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn a_stale_version_is_412_and_a_missing_one_428(_: PgPoolOptions, opts: PgConnectOptions) {
+    let app = TestApp::new(opts).await;
+    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
+    let created = create(&app, &owner, "key-00000001", galle()).await;
+    let path = format!("/api/v1/properties/{}", created.body["id"].as_str().unwrap());
+    patch(&app, &owner, &path, Some("\"1\""), json!({"name": "First edit"})).await;
+
+    let stale = patch(&app, &owner, &path, Some("\"1\""), json!({"name": "Second edit"})).await;
+    let missing = patch(&app, &owner, &path, None, json!({"name": "Third edit"})).await;
+    let malformed = patch(&app, &owner, &path, Some("2"), json!({"name": "Fourth edit"})).await;
+    let unknown =
+        patch(&app, &owner, &format!("/api/v1/properties/{}", Uuid::now_v7()), Some("\"1\""), json!({})).await;
+
+    assert_eq!(stale.status, StatusCode::PRECONDITION_FAILED, "{:?}", stale.body);
+    assert_eq!(stale.headers[header::CONTENT_TYPE], "application/problem+json");
+    assert_eq!(missing.status, StatusCode::PRECONDITION_REQUIRED, "{:?}", missing.body);
+    assert_eq!(malformed.status, StatusCode::BAD_REQUEST, "{:?}", malformed.body);
+    assert_eq!(unknown.status, StatusCode::NOT_FOUND, "{:?}", unknown.body);
+}
+
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn property_settings_are_validated(_: PgPoolOptions, opts: PgConnectOptions) {
+    let app = TestApp::new(opts).await;
+    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
+    let created = create(&app, &owner, "key-00000001", galle()).await;
+    let path = format!("/api/v1/properties/{}", created.body["id"].as_str().unwrap());
+
+    let bad_time = patch(&app, &owner, &path, Some("\"1\""), json!({"check_in_time": "25:00"})).await;
+    let empty_name = patch(&app, &owner, &path, Some("\"1\""), json!({"name": ""})).await;
+
+    assert_eq!(bad_time.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", bad_time.body);
+    assert_eq!(empty_name.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", empty_name.body);
+}
+
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn only_owners_and_managers_change_property_settings(_: PgPoolOptions, opts: PgConnectOptions) {
+    let app = TestApp::new(opts.clone()).await;
+    let superuser = PgPool::connect_with(opts).await.unwrap();
+    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
+    let created = create(&app, &owner, "key-00000001", galle()).await;
+    let path = format!("/api/v1/properties/{}", created.body["id"].as_str().unwrap());
+    let manager = app.staff(&superuser, &owner, "manager@example.com", "manager").await;
+    let front_desk = app.staff(&superuser, &owner, "desk@example.com", "front_desk").await;
+
+    let by_desk = patch(&app, &front_desk, &path, Some("\"1\""), json!({"check_out_time": "11:00"})).await;
+    let by_manager = patch(&app, &manager, &path, Some("\"1\""), json!({"check_out_time": "11:00"})).await;
+
+    assert_eq!(by_desk.status, StatusCode::FORBIDDEN);
+    assert_eq!(by_manager.status, StatusCode::OK, "{:?}", by_manager.body);
+}
```

Modify `crates/core-api/tests/graphql.rs`:

```diff
--- a/crates/core-api/tests/graphql.rs
+++ b/crates/core-api/tests/graphql.rs
@@ -102,3 +102,19 @@ async fn queries_more_complex_than_500_are_rejected() {
 
     assert_eq!(response.errors[0].message, "Query is too complex.");
 }
+
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn properties_include_their_settings(_: PgPoolOptions, opts: PgConnectOptions) {
+    let app = TestApp::new(opts).await;
+    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
+    create(&app, &owner, "GAL").await;
+
+    let query = json!({"query": "{ properties { checkInTime checkOutTime businessDate version } }"});
+    let response = app.send(Method::POST, "/graphql", Some(&owner), Some(query)).await;
+
+    let property = &response.body["data"]["properties"][0];
+    assert_eq!(property["checkInTime"], "14:00", "{:?}", response.body);
+    assert_eq!(property["checkOutTime"], "12:00");
+    assert_eq!(property["businessDate"].as_str().unwrap().len(), "2026-09-24".len());
+    assert_eq!(property["version"], 1);
+}
```

Modify `crates/core-api/tests/openapi.rs`:

```diff
--- a/crates/core-api/tests/openapi.rs
+++ b/crates/core-api/tests/openapi.rs
@@ -14,6 +14,7 @@ fn the_openapi_document_lists_every_rest_route() {
             "/api/v1/auth/signup",
             "/api/v1/me",
             "/api/v1/properties",
+            "/api/v1/properties/{property}",
             "/api/v1/session/tenant",
         ]
     );
```

- [ ] **Step 2: Run the tests to verify they fail**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p core-api --no-fail-fast --test properties --test graphql --test openapi
```

Expected: FAIL: the 4 new `properties` tests (there is no PATCH route yet, and create returns no `ETag` or settings), `graphql::properties_include_their_settings` (unknown fields), and the OpenAPI route list.

- [ ] **Step 3: Implement**

The workspace enables `serde-human-readable` on `time`, so `Date` serializes as `YYYY-MM-DD` in JSON. GraphQL codegen maps the new `Date` scalar to `string`.

Modify `Cargo.toml`:

```diff
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -29,7 +29,7 @@ serde_json = "1"
 sha2 = "0.11"
 sqlx = { version = "0.9", default-features = false, features = ["runtime-tokio", "tls-rustls", "postgres", "uuid", "time", "json", "migrate", "macros"] }
 thiserror = "2"
-time = { version = "0.3", features = ["serde", "serde-well-known"] }
+time = { version = "0.3", features = ["serde", "serde-well-known", "serde-human-readable"] }
 tokio = { version = "1", features = ["macros", "rt-multi-thread", "signal", "sync", "time"] }
 tokio-stream = { version = "0.1", features = ["sync"] }
 tower = { version = "0.5", features = ["util"] }
```

Modify `modules/property/Cargo.toml`:

```diff
--- a/modules/property/Cargo.toml
+++ b/modules/property/Cargo.toml
@@ -11,6 +11,7 @@ serde.workspace = true
 serde_json.workspace = true
 sqlx.workspace = true
 thiserror.workspace = true
+time.workspace = true
 utoipa.workspace = true
 uuid.workspace = true
 
```

Modify `modules/property/src/lib.rs`:

```diff
--- a/modules/property/src/lib.rs
+++ b/modules/property/src/lib.rs
@@ -2,6 +2,7 @@
 
 use db::{Event, TenantId, Tx, UserId};
 use serde::Serialize;
+use time::Date;
 use uuid::Uuid;
 
 #[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow, utoipa::ToSchema)]
@@ -11,9 +12,31 @@ pub struct Property {
     pub name: String,
     pub timezone: String,
     pub base_currency: String,
+    /// Local time guests may check in from, `HH:MM`.
+    pub check_in_time: String,
+    /// Local time guests check out by, `HH:MM`.
+    pub check_out_time: String,
+    /// The property's current trading day. Set to its local today when created; moved only by night audit.
+    pub business_date: Date,
     pub version: i32,
 }
 
+/// The `select` list that reads a [`Property`]. A macro, so queries can `concat!` it into a static string.
+macro_rules! property_columns {
+    () => {
+        "id, code, name, timezone, base_currency, to_char(check_in_time, 'HH24:MI') as check_in_time, \
+         to_char(check_out_time, 'HH24:MI') as check_out_time, business_date, version"
+    };
+}
+
+/// Settings changes; `None` leaves a field as it is. Times are `HH:MM`, validated by the caller.
+#[derive(Debug, Clone, Default)]
+pub struct PropertyChanges {
+    pub name: Option<String>,
+    pub check_in_time: Option<String>,
+    pub check_out_time: Option<String>,
+}
+
 pub struct NewProperty {
     pub code: String,
     pub name: String,
@@ -27,6 +50,10 @@ pub enum PropertyError {
     CodeTaken,
     #[error("unknown time zone")]
     UnknownTimezone,
+    #[error("property not found")]
+    NotFound,
+    #[error("the property was changed by someone else; reload and try again")]
+    VersionMismatch,
     #[error(transparent)]
     Database(#[from] sqlx::Error),
 }
@@ -48,11 +75,12 @@ pub async fn create_property(
     if !known_zone {
         return Err(PropertyError::UnknownTimezone);
     }
-    let inserted = sqlx::query_as::<_, Property>(
+    let inserted = sqlx::query_as::<_, Property>(concat!(
         "insert into property (id, tenant_id, code, name, timezone, base_currency, business_date)
          values ($1, $2, $3, $4, $5, $6, (now() at time zone $5)::date)
-         returning id, code, name, timezone, base_currency, version",
-    )
+         returning ",
+        property_columns!()
+    ))
     .bind(Uuid::now_v7())
     .bind(tenant.0)
     .bind(&input.code)
@@ -88,12 +116,64 @@ pub async fn create_property(
 
 /// Lists properties by code. `only` limits the result to those ids (for property-scoped staff).
 pub async fn list_properties(tx: &mut Tx, only: Option<&[Uuid]>) -> Result<Vec<Property>, sqlx::Error> {
-    sqlx::query_as(
-        "select id, code, name, timezone, base_currency, version from property
-         where $1::uuid[] is null or id = any($1)
-         order by code",
-    )
+    sqlx::query_as(concat!(
+        "select ",
+        property_columns!(),
+        " from property where $1::uuid[] is null or id = any($1) order by code"
+    ))
     .bind(only)
     .fetch_all(&mut **tx)
     .await
 }
+
+/// Applies `changes` if the property is still at `expected_version`, and bumps the version.
+/// `tx` must be scoped to `tenant`.
+pub async fn update_property(
+    tx: &mut Tx,
+    tenant: TenantId,
+    actor: UserId,
+    id: Uuid,
+    expected_version: i32,
+    changes: PropertyChanges,
+) -> Result<Property, PropertyError> {
+    let updated: Option<Property> = sqlx::query_as(concat!(
+        "update property set name = coalesce($3, name),
+                check_in_time = coalesce($4::time, check_in_time),
+                check_out_time = coalesce($5::time, check_out_time),
+                version = version + 1
+         where id = $1 and version = $2
+         returning ",
+        property_columns!()
+    ))
+    .bind(id)
+    .bind(expected_version)
+    .bind(&changes.name)
+    .bind(&changes.check_in_time)
+    .bind(&changes.check_out_time)
+    .fetch_optional(&mut **tx)
+    .await?;
+    let Some(property) = updated else {
+        let exists: bool = sqlx::query_scalar("select exists (select 1 from property where id = $1)")
+            .bind(id)
+            .fetch_one(&mut **tx)
+            .await?;
+        return Err(if exists { PropertyError::VersionMismatch } else { PropertyError::NotFound });
+    };
+    sqlx::query(
+        "insert into audit_log (id, tenant_id, actor_user_id, action, entity, entity_id, data)
+         values ($1, $2, $3, 'property.updated', 'property', $4, $5)",
+    )
+    .bind(Uuid::now_v7())
+    .bind(tenant.0)
+    .bind(actor.0)
+    .bind(id)
+    .bind(serde_json::json!({
+        "name": changes.name,
+        "check_in_time": changes.check_in_time,
+        "check_out_time": changes.check_out_time,
+    }))
+    .execute(&mut **tx)
+    .await?;
+    db::notify(tx, &Event { tenant_id: tenant, property_id: Some(id), keys: vec![PROPERTIES_KEY.into()] }).await?;
+    Ok(property)
+}
```

Create `crates/core-api/src/concurrency.rs`:

```rust
//! Optimistic concurrency. Editable resources carry a `version`, sent as `ETag: "<version>"`;
//! updates send it back in `If-Match` and are refused with 412 if the resource changed since.

use crate::error::ApiError;
use axum::Json;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

/// The version from `If-Match: "<version>"`. Missing is 428, malformed is 400.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IfMatch(pub i32);

impl<S: Send + Sync> FromRequestParts<S> for IfMatch {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        let value = parts.headers.get(header::IF_MATCH).ok_or_else(ApiError::precondition_required)?;
        value
            .to_str()
            .ok()
            .and_then(parse_version)
            .map(IfMatch)
            .ok_or_else(|| ApiError::bad_request("If-Match must be a quoted version, e.g. \"3\""))
    }
}

/// Parses a strong ETag holding a version: `"3"`.
fn parse_version(etag: &str) -> Option<i32> {
    etag.trim().strip_prefix('"')?.strip_suffix('"')?.parse().ok()
}

/// A JSON response with its version as the `ETag`.
pub struct Versioned<T> {
    status: StatusCode,
    version: i32,
    body: T,
}

impl<T> Versioned<T> {
    pub fn ok(version: i32, body: T) -> Self {
        Self { status: StatusCode::OK, version, body }
    }

    pub fn created(version: i32, body: T) -> Self {
        Self { status: StatusCode::CREATED, version, body }
    }
}

impl<T: Serialize> IntoResponse for Versioned<T> {
    fn into_response(self) -> Response {
        let etag = HeaderValue::from_str(&format!("\"{}\"", self.version)).expect("a number is a valid header value");
        (self.status, [(header::ETAG, etag)], Json(self.body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::parse_version;

    #[test]
    fn only_a_quoted_number_is_a_version() {
        assert_eq!(parse_version("\"3\""), Some(3));
        assert_eq!(parse_version(" \"12\" "), Some(12));
        assert_eq!(parse_version("3"), None);
        assert_eq!(parse_version("W/\"3\""), None);
        assert_eq!(parse_version("\"x\""), None);
    }
}
```

Modify `crates/core-api/src/error.rs`:

```diff
--- a/crates/core-api/src/error.rs
+++ b/crates/core-api/src/error.rs
@@ -54,6 +54,18 @@ impl ApiError {
         Self::new(StatusCode::CONFLICT, "Conflict", Some(detail.into()))
     }
 
+    pub fn precondition_failed(detail: impl Into<String>) -> Self {
+        Self::new(StatusCode::PRECONDITION_FAILED, "Precondition failed", Some(detail.into()))
+    }
+
+    pub fn precondition_required() -> Self {
+        Self::new(
+            StatusCode::PRECONDITION_REQUIRED,
+            "Precondition required",
+            Some("send If-Match with the version you edited, e.g. If-Match: \"3\"".into()),
+        )
+    }
+
     pub fn unprocessable(detail: impl Into<String>) -> Self {
         Self::new(StatusCode::UNPROCESSABLE_ENTITY, "Invalid request", Some(detail.into()))
     }
```

Modify `crates/core-api/src/extract.rs`:

```diff
--- a/crates/core-api/src/extract.rs
+++ b/crates/core-api/src/extract.rs
@@ -1,7 +1,7 @@
 //! Request extractors whose rejections are problem details, like every other API error.
 
 use crate::error::ApiError;
-use axum::extract::rejection::{JsonRejection, QueryRejection};
+use axum::extract::rejection::{JsonRejection, PathRejection, QueryRejection};
 use axum::extract::{FromRequest, FromRequestParts};
 
 /// `axum::Json` for request bodies: a malformed body is a 400 and a body of the wrong shape a 422.
@@ -14,6 +14,11 @@ pub struct ApiJson<T>(pub T);
 #[from_request(via(axum::extract::Query), rejection(ApiError))]
 pub struct ApiQuery<T>(pub T);
 
+/// `axum::extract::Path`: a path parameter that does not parse (such as an id that is not a UUID) is a 400.
+#[derive(Debug, FromRequestParts)]
+#[from_request(via(axum::extract::Path), rejection(ApiError))]
+pub struct ApiPath<T>(pub T);
+
 impl From<JsonRejection> for ApiError {
     fn from(rejection: JsonRejection) -> Self {
         match &rejection {
@@ -28,3 +33,9 @@ impl From<QueryRejection> for ApiError {
         ApiError::bad_request(rejection.body_text())
     }
 }
+
+impl From<PathRejection> for ApiError {
+    fn from(rejection: PathRejection) -> Self {
+        ApiError::bad_request(rejection.body_text())
+    }
+}
```

Modify `crates/core-api/src/lib.rs`:

```diff
--- a/crates/core-api/src/lib.rs
+++ b/crates/core-api/src/lib.rs
@@ -1,6 +1,7 @@
 //! HTTP layer: REST commands, GraphQL reads and server-sent events over the domain modules.
 
 pub mod auth;
+pub mod concurrency;
 pub mod config;
 pub mod csrf;
 pub mod error;
```

Modify `crates/core-api/src/routes/properties.rs`:

```diff
--- a/crates/core-api/src/routes/properties.rs
+++ b/crates/core-api/src/routes/properties.rs
@@ -1,16 +1,16 @@
 use crate::auth::TenantContext;
+use crate::concurrency::{IfMatch, Versioned};
 use crate::error::{ApiError, validate};
-use crate::extract::ApiJson;
+use crate::extract::{ApiJson, ApiPath};
 use crate::state::AppState;
-use axum::Json;
 use axum::extract::State;
-use axum::http::StatusCode;
 use db::Scope;
 use garde::Validate;
 use identity::Permission;
-use property::{NewProperty, Property, PropertyError};
+use property::{NewProperty, Property, PropertyChanges, PropertyError};
 use serde::Deserialize;
 use utoipa::ToSchema;
+use uuid::Uuid;
 
 #[derive(Debug, Deserialize, Validate, ToSchema)]
 pub struct CreatePropertyRequest {
@@ -27,6 +27,28 @@ pub struct CreatePropertyRequest {
     pub base_currency: String,
 }
 
+#[derive(Debug, Deserialize, Validate, ToSchema)]
+pub struct UpdatePropertyRequest {
+    #[garde(inner(length(chars, min = 1, max = 200)))]
+    pub name: Option<String>,
+    /// `HH:MM` (24-hour), local time.
+    #[garde(inner(pattern(r"^([01][0-9]|2[0-3]):[0-5][0-9]$")))]
+    pub check_in_time: Option<String>,
+    /// `HH:MM` (24-hour), local time.
+    #[garde(inner(pattern(r"^([01][0-9]|2[0-3]):[0-5][0-9]$")))]
+    pub check_out_time: Option<String>,
+}
+
+fn property_error(err: PropertyError) -> ApiError {
+    match err {
+        PropertyError::CodeTaken => ApiError::conflict(err.to_string()),
+        PropertyError::UnknownTimezone => ApiError::unprocessable(err.to_string()),
+        PropertyError::NotFound => ApiError::not_found(err.to_string()),
+        PropertyError::VersionMismatch => ApiError::precondition_failed(err.to_string()),
+        PropertyError::Database(db_err) => db_err.into(),
+    }
+}
+
 #[utoipa::path(post, path = "/api/v1/properties", request_body = CreatePropertyRequest,
     params(("Idempotency-Key" = String, Header)),
     responses((status = 201, body = Property), (status = 403), (status = 409), (status = 422)))]
@@ -34,17 +56,36 @@ pub async fn create(
     State(state): State<AppState>,
     ctx: TenantContext,
     ApiJson(body): ApiJson<CreatePropertyRequest>,
-) -> Result<(StatusCode, Json<Property>), ApiError> {
+) -> Result<Versioned<Property>, ApiError> {
     ctx.require(Permission::PropertiesCreate, None)?;
     validate(&body)?;
     let input =
         NewProperty { code: body.code, name: body.name, timezone: body.timezone, base_currency: body.base_currency };
     let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
-    let created = property::create_property(&mut tx, ctx.tenant, ctx.user, input).await.map_err(|err| match err {
-        PropertyError::CodeTaken => ApiError::conflict(err.to_string()),
-        PropertyError::UnknownTimezone => ApiError::unprocessable(err.to_string()),
-        PropertyError::Database(db_err) => db_err.into(),
-    })?;
+    let created = property::create_property(&mut tx, ctx.tenant, ctx.user, input).await.map_err(property_error)?;
+    tx.commit().await?;
+    Ok(Versioned::created(created.version, created))
+}
+
+/// Changes a property's settings. The business date is not editable: night audit moves it.
+#[utoipa::path(patch, path = "/api/v1/properties/{property}", request_body = UpdatePropertyRequest,
+    params(("property" = Uuid, Path), ("If-Match" = String, Header, description = "the version edited, e.g. \"3\"")),
+    responses((status = 200, body = Property), (status = 403), (status = 404), (status = 412), (status = 422), (status = 428)))]
+pub async fn update(
+    State(state): State<AppState>,
+    ctx: TenantContext,
+    ApiPath(property): ApiPath<Uuid>,
+    IfMatch(version): IfMatch,
+    ApiJson(body): ApiJson<UpdatePropertyRequest>,
+) -> Result<Versioned<Property>, ApiError> {
+    ctx.require(Permission::PropertiesManage, Some(property))?;
+    validate(&body)?;
+    let changes =
+        PropertyChanges { name: body.name, check_in_time: body.check_in_time, check_out_time: body.check_out_time };
+    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
+    let updated = property::update_property(&mut tx, ctx.tenant, ctx.user, property, version, changes)
+        .await
+        .map_err(property_error)?;
     tx.commit().await?;
-    Ok((StatusCode::CREATED, Json(created)))
+    Ok(Versioned::ok(updated.version, updated))
 }
```

Modify `crates/core-api/src/routes/mod.rs`:

```diff
--- a/crates/core-api/src/routes/mod.rs
+++ b/crates/core-api/src/routes/mod.rs
@@ -9,14 +9,14 @@ use axum::Router;
 use axum::extract::Request;
 use axum::middleware::{Next, from_fn, from_fn_with_state};
 use axum::response::{IntoResponse, Response};
-use axum::routing::{get, post, put};
+use axum::routing::{get, patch, post, put};
 use std::time::Duration;
 use tower_http::compression::CompressionLayer;
 use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
 use tower_http::trace::TraceLayer;
 
 pub use auth::{LoginRequest, SignupRequest, SwitchTenantRequest};
-pub use properties::CreatePropertyRequest;
+pub use properties::{CreatePropertyRequest, UpdatePropertyRequest};
 
 /// Longest a request may run before it is answered with 504.
 pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
@@ -35,6 +35,7 @@ pub fn router(state: AppState) -> Router {
         .route("/api/v1/auth/logout", post(auth::logout))
         .route("/api/v1/me", get(auth::me))
         .route("/api/v1/session/tenant", put(auth::switch_tenant))
+        .route("/api/v1/properties/{property}", patch(properties::update))
         .route("/graphql", post(graphql::handler))
         .merge(commands)
         .layer(from_fn(|request, next| deadline(REQUEST_TIMEOUT, request, next)));
```

Modify `crates/core-api/src/openapi.rs`:

```diff
--- a/crates/core-api/src/openapi.rs
+++ b/crates/core-api/src/openapi.rs
@@ -1,4 +1,4 @@
-use crate::routes::{CreatePropertyRequest, LoginRequest, SignupRequest, SwitchTenantRequest};
+use crate::routes::{CreatePropertyRequest, LoginRequest, SignupRequest, SwitchTenantRequest, UpdatePropertyRequest};
 use utoipa::OpenApi;
 
 #[derive(OpenApi)]
@@ -11,12 +11,14 @@ use utoipa::OpenApi;
         crate::routes::auth::me,
         crate::routes::auth::switch_tenant,
         crate::routes::properties::create,
+        crate::routes::properties::update,
     ),
     components(schemas(
         SignupRequest,
         LoginRequest,
         SwitchTenantRequest,
         CreatePropertyRequest,
+        UpdatePropertyRequest,
         identity::Profile,
         identity::TenantSummary,
         identity::Grant,
```

Modify `crates/core-api/src/graphql.rs`:

```diff
--- a/crates/core-api/src/graphql.rs
+++ b/crates/core-api/src/graphql.rs
@@ -9,6 +9,7 @@ use async_graphql_axum::{GraphQLRequest, GraphQLResponse};
 use axum::extract::State;
 use db::Scope;
 use sqlx::PgPool;
+use time::Date;
 use uuid::Uuid;
 
 pub type GqlSchema = Schema<Query, EmptyMutation, EmptySubscription>;
@@ -41,6 +42,13 @@ pub struct PropertyNode {
     pub name: String,
     pub timezone: String,
     pub base_currency: String,
+    /// `HH:MM`, local time.
+    pub check_in_time: String,
+    /// `HH:MM`, local time.
+    pub check_out_time: String,
+    pub business_date: Date,
+    /// Send as `If-Match: "<version>"` when updating.
+    pub version: i32,
 }
 
 pub struct Query;
@@ -63,6 +71,10 @@ impl Query {
                 name: p.name,
                 timezone: p.timezone,
                 base_currency: p.base_currency,
+                check_in_time: p.check_in_time,
+                check_out_time: p.check_out_time,
+                business_date: p.business_date,
+                version: p.version,
             })
             .collect())
     }
```

Modify `web/pms/codegen.ts`:

```diff
--- a/web/pms/codegen.ts
+++ b/web/pms/codegen.ts
@@ -12,7 +12,7 @@ const config: CodegenConfig = {
 				// Documents are plain strings: no GraphQL parser is shipped to the browser.
 				documentMode: 'string',
 				useTypeImports: true,
-				scalars: { UUID: 'string' }
+				scalars: { UUID: 'string', Date: 'string' }
 			}
 		}
 	}
```

Modify `docs/design/api-conventions.md`:

````diff
--- a/docs/design/api-conventions.md
+++ b/docs/design/api-conventions.md
@@ -43,7 +43,7 @@ Every non-GET/HEAD/OPTIONS request must send `x-goodfolk-csrf: 1` (`crates/core-
 { "type": "about:blank", "title": "Conflict", "status": 409, "detail": "a property with this code already exists" }
 ```
 
-`Content-Type: application/problem+json`, for every error the API returns, including malformed bodies and query strings, GraphQL request parse failures and timeouts. Construct errors with `ApiError::{bad_request, unauthenticated, invalid_credentials, forbidden, not_found, method_not_allowed, conflict, unprocessable, too_many_requests, gateway_timeout, internal}`. Database errors are logged and become a bare 500, and internal details never reach the client.
+`Content-Type: application/problem+json`, for every error the API returns, including malformed bodies and query strings, GraphQL request parse failures and timeouts. Construct errors with `ApiError::{bad_request, unauthenticated, invalid_credentials, forbidden, not_found, method_not_allowed, conflict, precondition_failed, precondition_required, unprocessable, too_many_requests, gateway_timeout, internal}`. Database errors are logged and become a bare 500, and internal details never reach the client.
 
 | Status | When |
 |---|---|
@@ -53,8 +53,9 @@ Every non-GET/HEAD/OPTIONS request must send `x-goodfolk-csrf: 1` (`crates/core-
 | 404 | Not found **or not visible to this tenant** (never reveal existence), or no such route (the router's fallback) |
 | 405 | The route exists but not for this method (the router's method-not-allowed fallback) |
 | 409 | Uniqueness conflict, double booking (exclusion violation), idempotent request still running |
-| 412 | `If-Match` version mismatch (from Phase 1) |
+| 412 | `If-Match` version mismatch |
 | 422 | Body of the wrong shape (missing or mistyped field), validation failed (`garde`), business rule violated, idempotency key reused for a different request |
+| 428 | An update without `If-Match` |
 | 429 | Sign-in throttled: 5 failed attempts for one email within 15 minutes (the same answer whether or not the account exists) |
 | 504 | Handler exceeded the 15 s request timeout (`routes::REQUEST_TIMEOUT`) |
 
@@ -72,16 +73,17 @@ Request DTOs derive `garde::Validate` and are checked with `error::validate(&bod
 
 ## Patterns to copy
 
-- **Request extractors:** take bodies as `ApiJson<T>` and query strings as `ApiQuery<T>` (`crates/core-api/src/extract.rs`), never `axum::Json` / `Query`, so rejections are problem details (400 malformed, 422 wrong shape). `axum::Json` is still fine for responses.
+- **Request extractors:** take bodies as `ApiJson<T>`, path parameters as `ApiPath<T>` and query strings as `ApiQuery<T>` (`crates/core-api/src/extract.rs`), never `axum::Json` / `Query`, so rejections are problem details (400 malformed, 422 wrong shape). `axum::Json` is still fine for responses.
 - **Resolver errors:** in GraphQL resolvers, map every database call with `.map_err(internal)?` (`graphql.rs`), which logs the error and returns a bare `Internal error`. Never let `?` put `sqlx::Error` text into a GraphQL response.
 - **Idempotency claim lifetime:** an unfinished claim is abandoned after 60 s (`ABANDONED_CLAIM_AFTER`) and taken over by the next request with its key; finalizing and releasing touch only the request's own claim (matched on `created_at`).
 - **Live events:** the broadcast channel carries `events::LiveEvent` (`Invalidate(db::Event)` | `Resync`). The listener (`events::spawn_listener`, given a pool on the direct listen URL) sends `Resync` whenever its database connection drops, and again once it has reconnected if the first reconnect attempt failed, since changes committed in between were missed. Streams send it as a `resync` event, as they do when a subscriber lags.
 - **Startup RLS guard:** `serve` calls `db::assert_rls_applies(&pool)` and refuses to start as a superuser, a `BYPASSRLS` role or a role that owns (directly or through membership) a table in `public`.
 
-## Optimistic concurrency (from Phase 1)
+## Optimistic concurrency
 
-- Editable resources return `version` in their body and an `ETag: "<version>"` header.
-- Updates must send `If-Match: "<version>"`. The SQL `update … where id = $1 and version = $2` returns no row on mismatch, which maps to 412, and the client refetches and shows what changed.
+- Editable resources return `version` in their body and an `ETag: "<version>"` header: handlers return `concurrency::Versioned::{ok, created}(version, body)` (`crates/core-api/src/concurrency.rs`). GraphQL nodes expose `version` too, which is where the SPA reads it.
+- Updates take the `concurrency::IfMatch` extractor, so they must send `If-Match: "<version>"`: missing is 428, not a quoted number is 400. The module's `update … where id = $1 and version = $2 … returning` finds no row on mismatch; it then checks whether the row exists and returns a version-mismatch error (412) or not-found (404). The client refetches and shows what changed.
+- Reordering (`PUT …/order`) is not a concurrent edit of one resource: it takes no `If-Match` and does not bump versions.
 
 ## Change events
 
````

- [ ] **Step 4: Regenerate the API types**

```sh
cd web/pms && bun run api:schemas && bun run codegen
```

- [ ] **Step 5: Run the tests to verify they pass**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cd web/pms
bun run lint && bun run check && bun run test && bun run build
```

Expected: PASS (`properties`: 11, `graphql`: 8, `openapi`: 1, `concurrency` unit test). Web checks clean.

- [ ] **Step 6: Commit**

```sh
git add -A
git commit -m "feat(properties): update check-in, check-out and name with If-Match; expose the business date"
```

### Task 7: The rooms module: room types, sections and the counter window

A new domain crate. This task gives it room types and housekeeping sections, and the start of the inventory counters: `extend_window` creates each room type's rows from the business date for 730 days (computing `physical` and `out_of_order` from rooms and blocks, so it is also how the window grows later), and `find_drift` recomputes every row from the source tables. Every function takes the caller's tenant-scoped transaction, checks the property exists (`NotFound("property")`), records an audit entry and queues change events.

Creating a room type emits `room-types:<p>` and the 25 or 26 month keys of the window, because the calendar gains a row. Room type codes never change; capacities are checked in Rust first for a readable message, and again by the database.

**Files:**
- Create: `modules/rooms/Cargo.toml`
- Create: `modules/rooms/src/inventory.rs`
- Create: `modules/rooms/src/lib.rs`
- Create: `modules/rooms/src/room_types.rs`
- Create: `modules/rooms/src/sections.rs`
- Modify: `Cargo.toml`
- Test: `modules/rooms/tests/common/mod.rs` (new)
- Test: `modules/rooms/tests/room_types.rs` (new)
- Regenerated, not shown: `Cargo.lock`

**Interfaces:**
- Consumes: `db::{Tx, TenantId, UserId, Event, notify}`, `property::create_property` (tests).
- Produces: `rooms::RoomsError::{NotFound(&'static str), VersionMismatch(&'static str), Conflict(String), Invalid(String), Database}`; `rooms::{room_types_key(p), rooms_key(p), month_keys(p, from, to) -> Vec<String>}`; `rooms::{Bed { kind, count }, RoomType, NewRoomType, RoomTypeChanges, create_room_type, update_room_type, reorder_room_types, list_room_types}`; `rooms::{Section, create_section, rename_section, list_sections}`; `rooms::{WINDOW_DAYS = 730, InventoryDay { room_type_id, date, physical, sold, out_of_order } + available(), InventoryDrift, extend_window, find_drift, list_inventory}`. Test helper `common::Hotel` (`new`, `tx`, `day(offset)`, `room_type(code)`, `counters(room_type)`, `drift()`).

- [ ] **Step 1: Write the failing tests**

The crate manifest comes first so the tests can build against it; the workspace adds `rooms` to its path dependencies and enables `time`'s `macros` (for `date!` in unit tests).

Modify `Cargo.toml`:

```diff
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -11,6 +11,7 @@ publish = false
 db = { path = "crates/db" }
 identity = { path = "modules/identity" }
 property = { path = "modules/property" }
+rooms = { path = "modules/rooms" }
 
 anyhow = "1"
 argon2 = "0.6"
@@ -29,7 +30,7 @@ serde_json = "1"
 sha2 = "0.11"
 sqlx = { version = "0.9", default-features = false, features = ["runtime-tokio", "tls-rustls", "postgres", "uuid", "time", "json", "migrate", "macros"] }
 thiserror = "2"
-time = { version = "0.3", features = ["serde", "serde-well-known", "serde-human-readable"] }
+time = { version = "0.3", features = ["macros", "serde", "serde-well-known", "serde-human-readable"] }
 tokio = { version = "1", features = ["macros", "rt-multi-thread", "signal", "sync", "time"] }
 tokio-stream = { version = "0.1", features = ["sync"] }
 tower = { version = "0.5", features = ["util"] }
```

Create `modules/rooms/Cargo.toml`:

```toml
[package]
name = "rooms"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
publish.workspace = true

[dependencies]
db.workspace = true
serde.workspace = true
serde_json.workspace = true
sqlx.workspace = true
thiserror.workspace = true
time.workspace = true
utoipa.workspace = true
uuid.workspace = true

[dev-dependencies]
db = { workspace = true, features = ["testing"] }
property.workspace = true
tokio.workspace = true

[lints]
workspace = true
```

Create `modules/rooms/tests/common/mod.rs`:

```rust
#![allow(dead_code)] // each test binary uses a different subset

use db::testing::app_pool;
use db::{Scope, TenantId, Tx, UserId, begin};
use rooms::{InventoryDay, NewRoomType, RoomType, WINDOW_DAYS};
use sqlx::PgPool;
use sqlx::postgres::PgConnectOptions;
use time::{Date, Duration};
use uuid::Uuid;

/// A tenant with one user and one property, as the API's sign-up and create-property commands leave it.
pub struct Hotel {
    pub pool: PgPool,
    pub tenant: TenantId,
    pub user: UserId,
    pub property: Uuid,
    pub business_date: Date,
}

impl Hotel {
    pub async fn new(opts: PgConnectOptions) -> Self {
        let pool = app_pool(opts, 2).await;
        let tenant = TenantId(Uuid::now_v7());
        let user = UserId(Uuid::now_v7());
        let mut tx = begin(&pool, Scope::tenant(tenant)).await.unwrap();
        sqlx::query("insert into tenant (id, name) values ($1, 'T')").bind(tenant.0).execute(&mut *tx).await.unwrap();
        sqlx::query("insert into app_user (id, email, password_hash, display_name) values ($1, $2, 'x', 'U')")
            .bind(user.0)
            .bind(format!("{}@example.com", user.0))
            .execute(&mut *tx)
            .await
            .unwrap();
        let hotel = property::NewProperty {
            code: "GAL".into(),
            name: "Galle".into(),
            timezone: "Asia/Colombo".into(),
            base_currency: "LKR".into(),
        };
        let property = property::create_property(&mut tx, tenant, user, hotel).await.unwrap();
        tx.commit().await.unwrap();
        Self { pool, tenant, user, property: property.id, business_date: property.business_date }
    }

    pub async fn tx(&self) -> Tx {
        begin(&self.pool, Scope::tenant(self.tenant)).await.unwrap()
    }

    /// The business date plus `days`.
    pub fn day(&self, days: i64) -> Date {
        self.business_date + Duration::days(days)
    }

    pub async fn room_type(&self, code: &str) -> RoomType {
        let mut tx = self.tx().await;
        let created = rooms::create_room_type(&mut tx, self.tenant, self.user, self.property, room_type(code)).await;
        tx.commit().await.unwrap();
        created.unwrap()
    }

    /// `room_type`'s counters over the whole window, by date.
    pub async fn counters(&self, room_type: Uuid) -> Vec<InventoryDay> {
        let mut tx = self.tx().await;
        let days = rooms::list_inventory(&mut tx, self.property, self.day(0), self.day(WINDOW_DAYS)).await.unwrap();
        days.into_iter().filter(|day| day.room_type_id == room_type).collect()
    }

    /// Counter rows that disagree with rooms and blocks; empty when the counters are right.
    pub async fn drift(&self) -> Vec<rooms::InventoryDrift> {
        let mut tx = self.tx().await;
        rooms::find_drift(&mut tx, self.property).await.unwrap()
    }
}

/// A double room for two adults and one child.
pub fn room_type(code: &str) -> NewRoomType {
    NewRoomType {
        code: code.into(),
        name: format!("Room type {code}"),
        base_occupancy: 2,
        max_adults: 2,
        max_children: 1,
        max_occupancy: 3,
        bed_config: vec![rooms::Bed { kind: "queen".into(), count: 1 }],
        amenities: vec!["Air conditioning".into()],
    }
}
```

Create `modules/rooms/tests/room_types.rs`:

```rust
mod common;

use common::{Hotel, room_type};
use rooms::{RoomTypeChanges, RoomsError, WINDOW_DAYS};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn room_types_are_listed_in_the_order_they_were_created(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let std = hotel.room_type("STD").await;
    let dlx = hotel.room_type("DLX").await;

    let mut tx = hotel.tx().await;
    let listed = rooms::list_room_types(&mut tx, hotel.property).await.unwrap();

    assert_eq!(listed, vec![std.clone(), dlx.clone()]);
    assert_eq!((std.sort_order, dlx.sort_order), (0, 1));
    assert_eq!(dlx.bed_config, vec![rooms::Bed { kind: "queen".into(), count: 1 }]);
    assert!(dlx.active);
    assert_eq!(dlx.version, 1);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_new_room_type_has_zero_counters_for_the_whole_window(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;

    let dlx = hotel.room_type("DLX").await;

    let counters = hotel.counters(dlx.id).await;
    assert_eq!(counters.len(), usize::try_from(WINDOW_DAYS).unwrap());
    assert_eq!(counters[0].date, hotel.day(0));
    assert_eq!(counters.last().unwrap().date, hotel.day(WINDOW_DAYS - 1));
    assert!(counters.iter().all(|day| day.physical == 0 && day.out_of_order == 0 && day.sold == 0));
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn codes_are_unique_within_a_property(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    hotel.room_type("DLX").await;

    let mut tx = hotel.tx().await;
    let again = rooms::create_room_type(&mut tx, hotel.tenant, hotel.user, hotel.property, room_type("DLX")).await;

    assert!(matches!(again, Err(RoomsError::Conflict(_))), "{again:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn capacities_must_add_up(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let mut tx = hotel.tx().await;

    let base_over_max = rooms::NewRoomType { base_occupancy: 4, ..room_type("A") };
    let max_over_people = rooms::NewRoomType { max_occupancy: 4, base_occupancy: 2, ..room_type("B") };
    let first = rooms::create_room_type(&mut tx, hotel.tenant, hotel.user, hotel.property, base_over_max).await;
    let second = rooms::create_room_type(&mut tx, hotel.tenant, hotel.user, hotel.property, max_over_people).await;

    assert!(matches!(first, Err(RoomsError::Invalid(_))), "{first:?}");
    assert!(matches!(second, Err(RoomsError::Invalid(_))), "{second:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_unknown_property_is_not_found(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let mut tx = hotel.tx().await;

    let result = rooms::create_room_type(&mut tx, hotel.tenant, hotel.user, Uuid::now_v7(), room_type("DLX")).await;

    assert!(matches!(result, Err(RoomsError::NotFound("property"))), "{result:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn updates_need_the_current_version(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let dlx = hotel.room_type("DLX").await;
    let rename = |name: &str| RoomTypeChanges { name: Some(name.into()), ..RoomTypeChanges::default() };

    let mut tx = hotel.tx().await;
    let renamed =
        rooms::update_room_type(&mut tx, hotel.tenant, hotel.user, hotel.property, dlx.id, 1, rename("Deluxe"))
            .await
            .unwrap();
    let stale =
        rooms::update_room_type(&mut tx, hotel.tenant, hotel.user, hotel.property, dlx.id, 1, rename("Deluxe Sea"))
            .await;
    let unknown =
        rooms::update_room_type(&mut tx, hotel.tenant, hotel.user, hotel.property, Uuid::now_v7(), 1, rename("X"))
            .await;

    assert_eq!((renamed.name.as_str(), renamed.version), ("Deluxe", 2));
    assert_eq!(renamed.code, "DLX");
    assert!(matches!(stale, Err(RoomsError::VersionMismatch("room type"))), "{stale:?}");
    assert!(matches!(unknown, Err(RoomsError::NotFound("room type"))), "{unknown:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn reordering_must_list_every_room_type_once(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let std = hotel.room_type("STD").await;
    let dlx = hotel.room_type("DLX").await;

    let mut tx = hotel.tx().await;
    let missing = rooms::reorder_room_types(&mut tx, hotel.tenant, hotel.property, &[dlx.id]).await;
    let repeated = rooms::reorder_room_types(&mut tx, hotel.tenant, hotel.property, &[dlx.id, dlx.id]).await;
    rooms::reorder_room_types(&mut tx, hotel.tenant, hotel.property, &[dlx.id, std.id]).await.unwrap();
    let listed = rooms::list_room_types(&mut tx, hotel.property).await.unwrap();

    assert!(matches!(missing, Err(RoomsError::Invalid(_))), "{missing:?}");
    assert!(matches!(repeated, Err(RoomsError::Invalid(_))), "{repeated:?}");
    assert_eq!(listed.iter().map(|t| t.code.as_str()).collect::<Vec<_>>(), ["DLX", "STD"]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn sections_are_unique_by_name_and_renamed_with_their_version(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let mut tx = hotel.tx().await;
    let east = rooms::create_section(&mut tx, hotel.tenant, hotel.user, hotel.property, "East wing").await.unwrap();
    tx.commit().await.unwrap();

    // A failed statement aborts its transaction, so each attempt that can fail gets its own.
    let duplicate =
        rooms::create_section(&mut hotel.tx().await, hotel.tenant, hotel.user, hotel.property, "East wing").await;
    let mut tx = hotel.tx().await;
    let renamed =
        rooms::rename_section(&mut tx, hotel.tenant, hotel.user, hotel.property, east.id, 1, "East").await.unwrap();
    let stale = rooms::rename_section(&mut tx, hotel.tenant, hotel.user, hotel.property, east.id, 1, "E").await;
    let listed = rooms::list_sections(&mut tx, hotel.property).await.unwrap();

    assert!(matches!(duplicate, Err(RoomsError::Conflict(_))), "{duplicate:?}");
    assert_eq!((renamed.name.as_str(), renamed.version), ("East", 2));
    assert!(matches!(stale, Err(RoomsError::VersionMismatch("section"))), "{stale:?}");
    assert_eq!(listed, vec![renamed]);
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p rooms
```

Expected: FAIL to compile: `unresolved import rooms` (the crate has no library yet).

- [ ] **Step 3: Implement**

Create `modules/rooms/src/lib.rs`:

```rust
//! Room types, housekeeping sections, rooms, room blocks, and the inventory counters they keep up to date.
//!
//! Every function takes a transaction scoped to the caller's tenant and checks that ids belong to the
//! given property. Writes record an audit entry and queue change events in the same transaction.

mod inventory;
mod room_types;
mod sections;

pub use inventory::{InventoryDay, InventoryDrift, WINDOW_DAYS, extend_window, find_drift, list_inventory, month_keys};
pub use room_types::{
    Bed, NewRoomType, RoomType, RoomTypeChanges, create_room_type, list_room_types, reorder_room_types,
    update_room_type,
};
pub use sections::{Section, create_section, list_sections, rename_section};

use db::{Event, TenantId, Tx, UserId};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum RoomsError {
    /// The property or the named resource does not exist in this tenant.
    #[error("{0} not found")]
    NotFound(&'static str),
    /// `If-Match` named an older version of the named resource.
    #[error("the {0} was changed by someone else; reload and try again")]
    VersionMismatch(&'static str),
    /// A uniqueness rule, such as a duplicate code or room number.
    #[error("{0}")]
    Conflict(String),
    /// A business rule, such as a capacity that does not add up or an unknown room type.
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// Cache key for a property's room types.
pub fn room_types_key(property: Uuid) -> String {
    format!("room-types:{property}")
}

/// Cache key for a property's rooms, sections and block reasons.
pub fn rooms_key(property: Uuid) -> String {
    format!("rooms:{property}")
}

/// Whether `err` violated the named constraint.
fn violates(err: &sqlx::Error, constraint: &str) -> bool {
    err.as_database_error().and_then(|db_err| db_err.constraint()).is_some_and(|name| name == constraint)
}

async fn audit(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    action: &str,
    entity: &str,
    id: Uuid,
    data: serde_json::Value,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into audit_log (id, tenant_id, actor_user_id, action, entity, entity_id, data)
         values ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(actor.0)
    .bind(action)
    .bind(entity)
    .bind(id)
    .bind(data)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn notify(tx: &mut Tx, tenant: TenantId, property: Uuid, keys: Vec<String>) -> Result<(), sqlx::Error> {
    db::notify(tx, &Event { tenant_id: tenant, property_id: Some(property), keys }).await
}

/// Checks that every one of the property's `table` rows is listed exactly once, then stores the listed order.
async fn reorder(tx: &mut Tx, table: &'static str, property: Uuid, ids: &[Uuid]) -> Result<(), RoomsError> {
    let (existing, listed): (i64, i64) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select (select count(*) from {table} where property_id = $1),
                (select count(distinct t.id) from {table} t where t.property_id = $1 and t.id = any($2))"
    )))
    .bind(property)
    .bind(ids)
    .fetch_one(&mut **tx)
    .await?;
    if existing != listed || usize::try_from(listed).ok() != Some(ids.len()) {
        return Err(RoomsError::Invalid(format!(
            "list every {} of the property exactly once",
            table.replace('_', " ")
        )));
    }
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "update {table} t set sort_order = o.position::integer
         from unnest($2::uuid[]) with ordinality as o (id, position)
         where t.id = o.id and t.property_id = $1"
    )))
    .bind(property)
    .bind(ids)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
```

Create `modules/rooms/src/inventory.rs`:

```rust
//! `inventory_day` counters: one row per room type per day, from the business date for [`WINDOW_DAYS`].
//!
//! `physical` counts a type's active rooms; `out_of_order` counts its active rooms under an active
//! out-of-order block that day. Writers adjust the counters in their own transaction; [`find_drift`]
//! recomputes them from rooms and blocks, for tests and the nightly check (Phase 7).

use crate::RoomsError;
use db::Tx;
use serde::Serialize;
use time::{Date, Duration};
use uuid::Uuid;

/// Counter rows exist for `[business date, business date + WINDOW_DAYS)`.
pub const WINDOW_DAYS: i64 = 730;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub struct InventoryDay {
    pub room_type_id: Uuid,
    pub date: Date,
    pub physical: i32,
    pub sold: i32,
    pub out_of_order: i32,
}

impl InventoryDay {
    /// Rooms of this type that can still be sold that day.
    pub fn available(&self) -> i32 {
        self.physical - self.sold - self.out_of_order
    }
}

/// A counter row that disagrees with a recomputation from rooms and blocks. `None` means the row is
/// missing (expected) or should not exist (actual).
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct InventoryDrift {
    pub room_type_id: Uuid,
    pub date: Date,
    pub expected_physical: Option<i32>,
    pub actual_physical: Option<i32>,
    pub expected_out_of_order: Option<i32>,
    pub actual_out_of_order: Option<i32>,
}

/// The property's business date. `NotFound` if the property is not in this tenant.
pub(crate) async fn business_date(tx: &mut Tx, property: Uuid) -> Result<Date, RoomsError> {
    sqlx::query_scalar("select business_date from property where id = $1")
        .bind(property)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(RoomsError::NotFound("property"))
}

/// Creates missing counter rows up to the end of the window, computed from rooms and blocks. Cheap when
/// nothing is missing. Until the nightly job exists (Phase 7), writers call it before changing counters.
pub async fn extend_window(tx: &mut Tx, property: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into inventory_day (tenant_id, property_id, room_type_id, date, physical, out_of_order)
         select rt.tenant_id, rt.property_id, rt.id, day.date,
                (select count(*) from room r where r.room_type_id = rt.id and r.active),
                (select count(*) from room_block b join room r on r.id = b.room_id
                 where r.room_type_id = rt.id and r.active and b.kind = 'out_of_order'
                   and b.released_at is null and b.period @> day.date)
         from room_type rt
         join property p on p.id = rt.property_id
         cross join lateral (select max(i.date) as last from inventory_day i
                             where i.property_id = rt.property_id and i.room_type_id = rt.id) existing
         cross join lateral (select p.business_date + offset_days as date
                             from generate_series(greatest(0, existing.last - p.business_date + 1), $2 - 1) offset_days) day
         where rt.property_id = $1
         on conflict do nothing",
    )
    .bind(property)
    .bind(i32::try_from(WINDOW_DAYS).expect("window fits in i32"))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Counter rows from the business date on that differ from a recomputation from rooms and blocks.
/// `sold` is expected to be 0 until reservations exist (Phase 3).
pub async fn find_drift(tx: &mut Tx, property: Uuid) -> Result<Vec<InventoryDrift>, sqlx::Error> {
    sqlx::query_as(
        "with expected as (
             select rt.id as room_type_id, day.date,
                    (select count(*) from room r where r.room_type_id = rt.id and r.active)::integer as physical,
                    (select count(*) from room_block b join room r on r.id = b.room_id
                     where r.room_type_id = rt.id and r.active and b.kind = 'out_of_order'
                       and b.released_at is null and b.period @> day.date)::integer as out_of_order
             from room_type rt
             join property p on p.id = rt.property_id
             cross join lateral (select p.business_date + offset_days as date
                                 from generate_series(0, $2 - 1) offset_days) day
             where rt.property_id = $1
         ),
         actual as (
             select i.room_type_id, i.date, i.physical, i.sold, i.out_of_order
             from inventory_day i join property p on p.id = i.property_id
             where i.property_id = $1 and i.date >= p.business_date
         )
         select coalesce(e.room_type_id, a.room_type_id) as room_type_id, coalesce(e.date, a.date) as date,
                e.physical as expected_physical, a.physical as actual_physical,
                e.out_of_order as expected_out_of_order, a.out_of_order as actual_out_of_order
         from expected e full join actual a on a.room_type_id = e.room_type_id and a.date = e.date
         where e.physical is distinct from a.physical or e.out_of_order is distinct from a.out_of_order
            or a.sold is distinct from 0
         order by 2, 1",
    )
    .bind(property)
    .bind(i32::try_from(WINDOW_DAYS).expect("window fits in i32"))
    .fetch_all(&mut **tx)
    .await
}

/// Counters for every room type on each day in `[from, to)`, by date then room type.
pub async fn list_inventory(
    tx: &mut Tx,
    property: Uuid,
    from: Date,
    to: Date,
) -> Result<Vec<InventoryDay>, sqlx::Error> {
    sqlx::query_as(
        "select room_type_id, date, physical, sold, out_of_order from inventory_day
         where property_id = $1 and date >= $2 and date < $3
         order by date, room_type_id",
    )
    .bind(property)
    .bind(from)
    .bind(to)
    .fetch_all(&mut **tx)
    .await
}

/// Cache keys `inventory:<property>:<yyyy-mm>` for every month that `[from, to)` touches.
pub fn month_keys(property: Uuid, from: Date, to: Date) -> Vec<String> {
    let mut keys = Vec::new();
    let mut month = from.replace_day(1).expect("every month has a first day");
    while from < to && month < to {
        keys.push(format!("inventory:{property}:{:04}-{:02}", month.year(), u8::from(month.month())));
        month += Duration::days(i64::from(month.month().length(month.year())));
    }
    keys
}

/// Month keys for the whole counter window, for changes that touch every day from the business date on.
pub(crate) fn window_keys(property: Uuid, business_date: Date) -> Vec<String> {
    month_keys(property, business_date, business_date + Duration::days(WINDOW_DAYS))
}

#[cfg(test)]
mod tests {
    use super::month_keys;
    use time::macros::date;
    use uuid::Uuid;

    #[test]
    fn month_keys_cover_each_touched_month_once() {
        let p = Uuid::nil();

        assert_eq!(
            month_keys(p, date!(2026 - 01 - 30), date!(2026 - 03 - 01)),
            vec![format!("inventory:{p}:2026-01"), format!("inventory:{p}:2026-02")]
        );
        assert_eq!(month_keys(p, date!(2026 - 12 - 31), date!(2027 - 01 - 02)).len(), 2);
        assert_eq!(month_keys(p, date!(2026 - 05 - 10), date!(2026 - 05 - 10)), Vec::<String>::new());
    }
}
```

Create `modules/rooms/src/room_types.rs`:

```rust
use crate::inventory::{business_date, extend_window, window_keys};
use crate::{RoomsError, audit, notify, reorder, room_types_key, violates};
use db::{TenantId, Tx, UserId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Beds of one kind in a room type, e.g. `{ "kind": "king", "count": 1 }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Bed {
    pub kind: String,
    pub count: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct RoomType {
    pub id: Uuid,
    pub property_id: Uuid,
    pub code: String,
    pub name: String,
    pub base_occupancy: i32,
    pub max_adults: i32,
    pub max_children: i32,
    pub max_occupancy: i32,
    #[sqlx(json)]
    pub bed_config: Vec<Bed>,
    pub amenities: Vec<String>,
    pub sort_order: i32,
    pub active: bool,
    pub version: i32,
}

#[derive(Debug, Clone)]
pub struct NewRoomType {
    pub code: String,
    pub name: String,
    pub base_occupancy: i32,
    pub max_adults: i32,
    pub max_children: i32,
    pub max_occupancy: i32,
    pub bed_config: Vec<Bed>,
    pub amenities: Vec<String>,
}

/// `None` leaves a field unchanged. The code never changes: channels and reports refer to it.
#[derive(Debug, Clone, Default)]
pub struct RoomTypeChanges {
    pub name: Option<String>,
    pub base_occupancy: Option<i32>,
    pub max_adults: Option<i32>,
    pub max_children: Option<i32>,
    pub max_occupancy: Option<i32>,
    pub bed_config: Option<Vec<Bed>>,
    pub amenities: Option<Vec<String>>,
    pub active: Option<bool>,
}

const COLUMNS: &str = "id, property_id, code, name, base_occupancy, max_adults, max_children, max_occupancy, \
                       bed_config, amenities, sort_order, active, version";

/// The capacity rules the database also enforces, checked first for a readable message.
fn check_capacity(base: i32, adults: i32, children: i32, max: i32) -> Result<(), RoomsError> {
    if base > max {
        Err(RoomsError::Invalid("base occupancy cannot exceed maximum occupancy".into()))
    } else if adults > max {
        Err(RoomsError::Invalid("maximum adults cannot exceed maximum occupancy".into()))
    } else if max > adults + children {
        Err(RoomsError::Invalid("maximum occupancy cannot exceed maximum adults plus maximum children".into()))
    } else {
        Ok(())
    }
}

pub async fn create_room_type(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    input: NewRoomType,
) -> Result<RoomType, RoomsError> {
    let today = business_date(tx, property).await?;
    check_capacity(input.base_occupancy, input.max_adults, input.max_children, input.max_occupancy)?;
    let inserted = sqlx::query_as::<_, RoomType>(sqlx::AssertSqlSafe(format!(
        "insert into room_type (id, tenant_id, property_id, code, name, base_occupancy, max_adults, max_children,
                                max_occupancy, bed_config, amenities, sort_order)
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11,
                 (select coalesce(max(sort_order) + 1, 0) from room_type where property_id = $3))
         returning {COLUMNS}"
    )))
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(property)
    .bind(&input.code)
    .bind(&input.name)
    .bind(input.base_occupancy)
    .bind(input.max_adults)
    .bind(input.max_children)
    .bind(input.max_occupancy)
    .bind(sqlx::types::Json(&input.bed_config))
    .bind(&input.amenities)
    .fetch_one(&mut **tx)
    .await;
    let room_type = match inserted {
        Ok(room_type) => room_type,
        Err(err) if violates(&err, "room_type_property_id_code_key") => {
            return Err(RoomsError::Conflict(format!("a room type with code {} already exists", input.code)));
        }
        Err(err) => return Err(err.into()),
    };
    extend_window(tx, property).await?;
    audit(tx, tenant, actor, "room_type.created", "room_type", room_type.id, serde_json::json!({ "code": input.code }))
        .await?;
    let mut keys = vec![room_types_key(property)];
    keys.extend(window_keys(property, today));
    notify(tx, tenant, property, keys).await?;
    Ok(room_type)
}

pub async fn update_room_type(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
    changes: RoomTypeChanges,
) -> Result<RoomType, RoomsError> {
    let current: RoomType = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {COLUMNS} from room_type where id = $1 and property_id = $2 for update"
    )))
    .bind(id)
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(RoomsError::NotFound("room type"))?;
    if current.version != expected_version {
        return Err(RoomsError::VersionMismatch("room type"));
    }
    check_capacity(
        changes.base_occupancy.unwrap_or(current.base_occupancy),
        changes.max_adults.unwrap_or(current.max_adults),
        changes.max_children.unwrap_or(current.max_children),
        changes.max_occupancy.unwrap_or(current.max_occupancy),
    )?;
    if changes.active == Some(false) && current.active {
        let rooms: i64 = sqlx::query_scalar("select count(*) from room where room_type_id = $1 and active")
            .bind(id)
            .fetch_one(&mut **tx)
            .await?;
        if rooms > 0 {
            return Err(RoomsError::Conflict(format!(
                "{rooms} active rooms have this type; move or deactivate them first"
            )));
        }
    }
    let updated: RoomType = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "update room_type set name = coalesce($3, name), base_occupancy = coalesce($4, base_occupancy),
                max_adults = coalesce($5, max_adults), max_children = coalesce($6, max_children),
                max_occupancy = coalesce($7, max_occupancy), bed_config = coalesce($8, bed_config),
                amenities = coalesce($9, amenities), active = coalesce($10, active), version = version + 1
         where id = $1 and version = $2
         returning {COLUMNS}"
    )))
    .bind(id)
    .bind(expected_version)
    .bind(&changes.name)
    .bind(changes.base_occupancy)
    .bind(changes.max_adults)
    .bind(changes.max_children)
    .bind(changes.max_occupancy)
    .bind(changes.bed_config.as_ref().map(sqlx::types::Json))
    .bind(&changes.amenities)
    .bind(changes.active)
    .fetch_one(&mut **tx)
    .await?;
    audit(tx, tenant, actor, "room_type.updated", "room_type", id, serde_json::json!({ "active": changes.active }))
        .await?;
    notify(tx, tenant, property, vec![room_types_key(property)]).await?;
    Ok(updated)
}

/// Sets the display order. `ids` must list every room type of the property exactly once.
pub async fn reorder_room_types(tx: &mut Tx, tenant: TenantId, property: Uuid, ids: &[Uuid]) -> Result<(), RoomsError> {
    business_date(tx, property).await?;
    reorder(tx, "room_type", property, ids).await?;
    notify(tx, tenant, property, vec![room_types_key(property)]).await?;
    Ok(())
}

/// Every room type of the property, active or not, in display order.
pub async fn list_room_types(tx: &mut Tx, property: Uuid) -> Result<Vec<RoomType>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {COLUMNS} from room_type where property_id = $1 order by sort_order, code"
    )))
    .bind(property)
    .fetch_all(&mut **tx)
    .await
}
```

Create `modules/rooms/src/sections.rs`:

```rust
use crate::inventory::business_date;
use crate::{RoomsError, audit, notify, rooms_key, violates};
use db::{TenantId, Tx, UserId};
use serde::Serialize;
use uuid::Uuid;

/// A housekeeping section: a group of rooms one housekeeper looks after.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct Section {
    pub id: Uuid,
    pub property_id: Uuid,
    pub name: String,
    pub version: i32,
}

fn name_error(err: sqlx::Error, name: &str) -> RoomsError {
    if violates(&err, "housekeeping_section_property_id_name_key") {
        RoomsError::Conflict(format!("a section named {name} already exists"))
    } else {
        err.into()
    }
}

pub async fn create_section(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    name: &str,
) -> Result<Section, RoomsError> {
    business_date(tx, property).await?;
    let section: Section = sqlx::query_as(
        "insert into housekeeping_section (id, tenant_id, property_id, name) values ($1, $2, $3, $4)
         returning id, property_id, name, version",
    )
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(property)
    .bind(name)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| name_error(err, name))?;
    audit(
        tx,
        tenant,
        actor,
        "section.created",
        "housekeeping_section",
        section.id,
        serde_json::json!({ "name": name }),
    )
    .await?;
    notify(tx, tenant, property, vec![rooms_key(property)]).await?;
    Ok(section)
}

pub async fn rename_section(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
    name: &str,
) -> Result<Section, RoomsError> {
    let updated: Option<Section> = sqlx::query_as(
        "update housekeeping_section set name = $4, version = version + 1
         where id = $1 and property_id = $2 and version = $3
         returning id, property_id, name, version",
    )
    .bind(id)
    .bind(property)
    .bind(expected_version)
    .bind(name)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|err| name_error(err, name))?;
    let Some(section) = updated else {
        let exists: bool =
            sqlx::query_scalar("select exists (select 1 from housekeeping_section where id = $1 and property_id = $2)")
                .bind(id)
                .bind(property)
                .fetch_one(&mut **tx)
                .await?;
        return Err(if exists { RoomsError::VersionMismatch("section") } else { RoomsError::NotFound("section") });
    };
    audit(tx, tenant, actor, "section.renamed", "housekeeping_section", id, serde_json::json!({ "name": name }))
        .await?;
    notify(tx, tenant, property, vec![rooms_key(property)]).await?;
    Ok(section)
}

pub async fn list_sections(tx: &mut Tx, property: Uuid) -> Result<Vec<Section>, sqlx::Error> {
    sqlx::query_as(
        "select id, property_id, name, version from housekeeping_section where property_id = $1 order by name",
    )
    .bind(property)
    .fetch_all(&mut **tx)
    .await
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p rooms
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS: 8 `room_types` tests and the `month_keys` unit test.

- [ ] **Step 5: Commit**

```sh
git add -A
git commit -m "feat(rooms): room types, housekeeping sections and the inventory counter window"
```

### Task 8: Rooms, bulk ranges and reordering, with physical counts

Rooms are created one at a time or as a range (`101`–`120`, at most 200, all or nothing; taken numbers are named in the 409), updated with their version, and reordered. A room's share of its type's counters — one physical room from the business date on, plus one out-of-order room on each day of its active out-of-order blocks — is added when it is created or reactivated, removed when it is deactivated, and moved when it is retyped (`inventory::contribute`). Removal takes the blocks off before the physical count, so `out_of_order <= physical` holds after every statement. The room row is locked (`for update`) while it changes, which later serializes it against blocks on the same room.

**Files:**
- Create: `modules/rooms/src/rooms.rs`
- Modify: `modules/rooms/src/inventory.rs`
- Modify: `modules/rooms/src/lib.rs`
- Test: `modules/rooms/tests/rooms.rs` (new)

**Interfaces:**
- Consumes: Task 7's `business_date`, `extend_window`, `window_keys`, `reorder`, `audit`, `notify`.
- Produces: `rooms::{Room, NewRoom, RoomRange { room_type_id, prefix, first, last, floor, section_id }, RoomChanges { room_type_id, number, floor: Option<Option<String>>, section_id: Option<Option<Uuid>>, active }, MAX_ROOMS_PER_RANGE = 200, create_room, create_rooms, update_room, reorder_rooms, list_rooms(tx, property, room_type: Option<Uuid>)}`; crate-internal `inventory::{adjust, contribute}`.

- [ ] **Step 1: Write the failing tests**

Create `modules/rooms/tests/rooms.rs`:

```rust
mod common;

use common::Hotel;
use rooms::{NewRoom, Room, RoomChanges, RoomRange, RoomTypeChanges, RoomsError};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

impl Hotel {
    async fn room(&self, room_type: Uuid, number: &str) -> Room {
        let input = NewRoom { room_type_id: room_type, number: number.into(), floor: None, section_id: None };
        let mut tx = self.tx().await;
        let room = rooms::create_room(&mut tx, self.tenant, self.user, self.property, input).await.unwrap();
        tx.commit().await.unwrap();
        room
    }

    async fn update_room(&self, room: &Room, changes: RoomChanges) -> Result<Room, RoomsError> {
        let mut tx = self.tx().await;
        let updated =
            rooms::update_room(&mut tx, self.tenant, self.user, self.property, room.id, room.version, changes).await?;
        tx.commit().await.unwrap();
        Ok(updated)
    }

    /// `room_type`'s physical count on every day of the window, collapsed to the distinct values.
    async fn physical(&self, room_type: Uuid) -> Vec<i32> {
        let mut values: Vec<i32> = self.counters(room_type).await.iter().map(|day| day.physical).collect();
        values.dedup();
        values
    }
}

fn range(room_type: Uuid, first: u32, last: u32) -> RoomRange {
    RoomRange { room_type_id: room_type, prefix: String::new(), first, last, floor: Some("1".into()), section_id: None }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_new_room_counts_on_every_day_of_the_window(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let dlx = hotel.room_type("DLX").await;

    let room = hotel.room(dlx.id, "101").await;

    assert_eq!((room.number.as_str(), room.active, room.version), ("101", true, 1));
    assert_eq!(hotel.physical(dlx.id).await, vec![1]);
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_range_creates_numbered_rooms_in_order(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let dlx = hotel.room_type("DLX").await;
    let mut tx = hotel.tx().await;

    let created =
        rooms::create_rooms(&mut tx, hotel.tenant, hotel.user, hotel.property, range(dlx.id, 101, 105)).await.unwrap();
    tx.commit().await.unwrap();

    let numbers: Vec<&str> = created.iter().map(|room| room.number.as_str()).collect();
    assert_eq!(numbers, ["101", "102", "103", "104", "105"]);
    assert_eq!(created.iter().map(|room| room.sort_order).collect::<Vec<_>>(), [0, 1, 2, 3, 4]);
    assert!(created.iter().all(|room| room.floor.as_deref() == Some("1")));
    assert_eq!(hotel.physical(dlx.id).await, vec![5]);
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn existing_numbers_are_a_conflict_that_names_them(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let dlx = hotel.room_type("DLX").await;
    hotel.room(dlx.id, "103").await;
    let mut tx = hotel.tx().await;

    let result = rooms::create_rooms(&mut tx, hotel.tenant, hotel.user, hotel.property, range(dlx.id, 101, 105)).await;

    let Err(RoomsError::Conflict(message)) = result else { panic!("expected a conflict, got {result:?}") };
    assert_eq!(message, "room 103 already exists");
    assert_eq!(rooms::list_rooms(&mut tx, hotel.property, None).await.unwrap().len(), 1);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_range_must_be_in_order_and_at_most_200_rooms(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let dlx = hotel.room_type("DLX").await;
    let mut tx = hotel.tx().await;

    let backwards =
        rooms::create_rooms(&mut tx, hotel.tenant, hotel.user, hotel.property, range(dlx.id, 120, 101)).await;
    let too_many = rooms::create_rooms(&mut tx, hotel.tenant, hotel.user, hotel.property, range(dlx.id, 1, 201)).await;

    assert!(matches!(backwards, Err(RoomsError::Invalid(_))), "{backwards:?}");
    assert!(matches!(too_many, Err(RoomsError::Invalid(_))), "{too_many:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn retyping_and_deactivating_move_the_counts(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let std = hotel.room_type("STD").await;
    let dlx = hotel.room_type("DLX").await;
    let room = hotel.room(std.id, "101").await;

    let retyped =
        hotel.update_room(&room, RoomChanges { room_type_id: Some(dlx.id), ..RoomChanges::default() }).await.unwrap();
    let after_retype = (hotel.physical(std.id).await, hotel.physical(dlx.id).await);
    let deactivated =
        hotel.update_room(&retyped, RoomChanges { active: Some(false), ..RoomChanges::default() }).await.unwrap();
    let after_deactivate = hotel.physical(dlx.id).await;
    let reactivated =
        hotel.update_room(&deactivated, RoomChanges { active: Some(true), ..RoomChanges::default() }).await.unwrap();

    assert_eq!(after_retype, (vec![0], vec![1]));
    assert_eq!(after_deactivate, vec![0]);
    assert_eq!(hotel.physical(dlx.id).await, vec![1]);
    assert_eq!((reactivated.version, reactivated.room_type_id), (4, dlx.id));
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_room_needs_an_active_room_type_of_its_property(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let dlx = hotel.room_type("DLX").await;
    let std = hotel.room_type("STD").await;
    hotel.room(dlx.id, "101").await;
    let mut tx = hotel.tx().await;
    let retired = RoomTypeChanges { active: Some(false), ..RoomTypeChanges::default() };
    rooms::update_room_type(&mut tx, hotel.tenant, hotel.user, hotel.property, std.id, 1, retired.clone())
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let mut tx = hotel.tx().await;
    let unknown = NewRoom { room_type_id: Uuid::now_v7(), number: "102".into(), floor: None, section_id: None };
    let unknown = rooms::create_room(&mut tx, hotel.tenant, hotel.user, hotel.property, unknown).await;
    let inactive = NewRoom { room_type_id: std.id, number: "103".into(), floor: None, section_id: None };
    let inactive = rooms::create_room(&mut tx, hotel.tenant, hotel.user, hotel.property, inactive).await;
    let retire_in_use =
        rooms::update_room_type(&mut tx, hotel.tenant, hotel.user, hotel.property, dlx.id, 1, retired).await;

    assert!(matches!(unknown, Err(RoomsError::Invalid(_))), "{unknown:?}");
    assert!(matches!(inactive, Err(RoomsError::Invalid(_))), "{inactive:?}");
    assert!(matches!(retire_in_use, Err(RoomsError::Conflict(_))), "{retire_in_use:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn rooms_are_listed_in_display_order_and_filtered_by_type(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let std = hotel.room_type("STD").await;
    let dlx = hotel.room_type("DLX").await;
    let first = hotel.room(std.id, "101").await;
    let second = hotel.room(dlx.id, "102").await;
    let third = hotel.room(std.id, "103").await;

    let mut tx = hotel.tx().await;
    rooms::reorder_rooms(&mut tx, hotel.tenant, hotel.property, &[third.id, first.id, second.id]).await.unwrap();
    let all = rooms::list_rooms(&mut tx, hotel.property, None).await.unwrap();
    let standard = rooms::list_rooms(&mut tx, hotel.property, Some(std.id)).await.unwrap();
    let partial = rooms::reorder_rooms(&mut tx, hotel.tenant, hotel.property, &[third.id]).await;

    assert_eq!(all.iter().map(|room| room.number.as_str()).collect::<Vec<_>>(), ["103", "101", "102"]);
    assert_eq!(standard.iter().map(|room| room.number.as_str()).collect::<Vec<_>>(), ["103", "101"]);
    assert!(matches!(partial, Err(RoomsError::Invalid(_))), "{partial:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn room_updates_need_the_current_version(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let dlx = hotel.room_type("DLX").await;
    let room = hotel.room(dlx.id, "101").await;
    let section = {
        let mut tx = hotel.tx().await;
        let section = rooms::create_section(&mut tx, hotel.tenant, hotel.user, hotel.property, "East").await.unwrap();
        tx.commit().await.unwrap();
        section
    };

    let moved = hotel
        .update_room(
            &room,
            RoomChanges {
                number: Some("101A".into()),
                floor: Some(Some("2".into())),
                section_id: Some(Some(section.id)),
                ..RoomChanges::default()
            },
        )
        .await
        .unwrap();
    let stale = hotel.update_room(&room, RoomChanges { floor: Some(None), ..RoomChanges::default() }).await;
    let cleared = hotel.update_room(&moved, RoomChanges { floor: Some(None), ..RoomChanges::default() }).await.unwrap();

    assert_eq!(
        (moved.number.as_str(), moved.floor.as_deref(), moved.section_id),
        ("101A", Some("2"), Some(section.id))
    );
    assert!(matches!(stale, Err(RoomsError::VersionMismatch("room"))), "{stale:?}");
    assert_eq!((cleared.floor, cleared.section_id, cleared.version), (None, Some(section.id), 3));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p rooms --test rooms
```

Expected: FAIL to compile: `unresolved imports rooms::NewRoom, rooms::Room, rooms::RoomChanges, rooms::RoomRange`.

- [ ] **Step 3: Implement**

Modify `modules/rooms/src/inventory.rs`:

```diff
--- a/modules/rooms/src/inventory.rs
+++ b/modules/rooms/src/inventory.rs
@@ -76,6 +76,69 @@ pub async fn extend_window(tx: &mut Tx, property: Uuid) -> Result<(), sqlx::Erro
     Ok(())
 }
 
+/// Adds the deltas to `room_type`'s counters on each day in `[from, to)` (`to: None` = to the end of the
+/// window). `from` must not be before the business date: past days are history and never change.
+pub(crate) async fn adjust(
+    tx: &mut Tx,
+    property: Uuid,
+    room_type: Uuid,
+    from: Date,
+    to: Option<Date>,
+    physical: i32,
+    out_of_order: i32,
+) -> Result<(), sqlx::Error> {
+    sqlx::query(
+        "update inventory_day set physical = physical + $5, out_of_order = out_of_order + $6
+         where property_id = $1 and room_type_id = $2 and date >= $3 and ($4::date is null or date < $4)",
+    )
+    .bind(property)
+    .bind(room_type)
+    .bind(from)
+    .bind(to)
+    .bind(physical)
+    .bind(out_of_order)
+    .execute(&mut **tx)
+    .await?;
+    Ok(())
+}
+
+/// Adds (`sign = 1`) or removes (`sign = -1`) one room's share of its type's counters from `today` on:
+/// one physical room, plus one out-of-order room on each day of its active out-of-order blocks.
+/// Inactive rooms have no share. Removal takes the blocks off first so `out_of_order <= physical` holds.
+pub(crate) async fn contribute(
+    tx: &mut Tx,
+    property: Uuid,
+    today: Date,
+    room: Uuid,
+    room_type: Uuid,
+    active: bool,
+    sign: i32,
+) -> Result<(), sqlx::Error> {
+    if !active {
+        return Ok(());
+    }
+    if sign > 0 {
+        adjust(tx, property, room_type, today, None, sign, 0).await?;
+    }
+    sqlx::query(
+        "update inventory_day i set out_of_order = i.out_of_order + $5
+         from room_block b
+         where b.room_id = $3 and b.released_at is null and b.kind = 'out_of_order' and b.period @> i.date
+           and i.property_id = $1 and i.room_type_id = $4 and i.date >= $2",
+    )
+    .bind(property)
+    .bind(today)
+    .bind(room)
+    .bind(room_type)
+    .bind(sign)
+    .execute(&mut **tx)
+    .await?;
+    if sign < 0 {
+        adjust(tx, property, room_type, today, None, sign, 0).await?;
+    }
+    Ok(())
+}
+
 /// Counter rows from the business date on that differ from a recomputation from rooms and blocks.
 /// `sold` is expected to be 0 until reservations exist (Phase 3).
 pub async fn find_drift(tx: &mut Tx, property: Uuid) -> Result<Vec<InventoryDrift>, sqlx::Error> {
```

Create `modules/rooms/src/rooms.rs`:

```rust
use crate::inventory::{adjust, business_date, contribute, extend_window, window_keys};
use crate::{RoomsError, audit, notify, reorder, rooms_key, violates};
use db::{TenantId, Tx, UserId};
use serde::Serialize;
use time::Date;
use uuid::Uuid;

/// Most rooms one range may create.
pub const MAX_ROOMS_PER_RANGE: u32 = 200;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct Room {
    pub id: Uuid,
    pub property_id: Uuid,
    pub room_type_id: Uuid,
    pub number: String,
    pub floor: Option<String>,
    pub section_id: Option<Uuid>,
    pub active: bool,
    pub sort_order: i32,
    pub version: i32,
}

#[derive(Debug, Clone)]
pub struct NewRoom {
    pub room_type_id: Uuid,
    pub number: String,
    pub floor: Option<String>,
    pub section_id: Option<Uuid>,
}

/// Rooms `{prefix}{first}` to `{prefix}{last}`, e.g. 101 to 120, all of one type, floor and section.
#[derive(Debug, Clone)]
pub struct RoomRange {
    pub room_type_id: Uuid,
    pub prefix: String,
    pub first: u32,
    pub last: u32,
    pub floor: Option<String>,
    pub section_id: Option<Uuid>,
}

/// `None` leaves a field unchanged; `Some(None)` clears an optional one.
#[derive(Debug, Clone, Default)]
pub struct RoomChanges {
    pub room_type_id: Option<Uuid>,
    pub number: Option<String>,
    pub floor: Option<Option<String>>,
    pub section_id: Option<Option<Uuid>>,
    pub active: Option<bool>,
}

const COLUMNS: &str = "id, property_id, room_type_id, number, floor, section_id, active, sort_order, version";

/// Letters, digits and `-`, 1 to 10 characters (the database checks the same).
fn check_number(number: &str) -> Result<(), RoomsError> {
    let valid = (1..=10).contains(&number.len()) && number.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    if valid { Ok(()) } else { Err(RoomsError::Invalid(format!("{number:?} is not a valid room number"))) }
}

/// The room type must be an active type of this property, and the section (if any) a section of it.
async fn check_references(
    tx: &mut Tx,
    property: Uuid,
    room_type: Option<Uuid>,
    section: Option<Uuid>,
) -> Result<(), RoomsError> {
    if let Some(room_type) = room_type {
        let active: Option<bool> =
            sqlx::query_scalar("select active from room_type where id = $1 and property_id = $2")
                .bind(room_type)
                .bind(property)
                .fetch_optional(&mut **tx)
                .await?;
        match active {
            None => return Err(RoomsError::Invalid("no such room type in this property".into())),
            Some(false) => return Err(RoomsError::Invalid("the room type is inactive".into())),
            Some(true) => {}
        }
    }
    if let Some(section) = section {
        let exists: bool =
            sqlx::query_scalar("select exists (select 1 from housekeeping_section where id = $1 and property_id = $2)")
                .bind(section)
                .bind(property)
                .fetch_one(&mut **tx)
                .await?;
        if !exists {
            return Err(RoomsError::Invalid("no such housekeeping section in this property".into()));
        }
    }
    Ok(())
}

fn numbers_taken(numbers: &[String]) -> RoomsError {
    match numbers {
        [one] => RoomsError::Conflict(format!("room {one} already exists")),
        many => RoomsError::Conflict(format!("rooms {} already exist", many.join(", "))),
    }
}

pub async fn create_room(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    input: NewRoom,
) -> Result<Room, RoomsError> {
    let mut created = insert_rooms(
        tx,
        tenant,
        actor,
        property,
        input.room_type_id,
        vec![input.number],
        input.floor,
        input.section_id,
    )
    .await?;
    Ok(created.remove(0))
}

/// Creates every room in `range`, or none if any number is taken.
pub async fn create_rooms(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    range: RoomRange,
) -> Result<Vec<Room>, RoomsError> {
    if range.first > range.last {
        return Err(RoomsError::Invalid("the first room number must not be after the last".into()));
    }
    if range.last - range.first >= MAX_ROOMS_PER_RANGE {
        return Err(RoomsError::Invalid(format!("add at most {MAX_ROOMS_PER_RANGE} rooms at a time")));
    }
    let numbers = (range.first..=range.last).map(|n| format!("{}{n}", range.prefix)).collect();
    insert_rooms(tx, tenant, actor, property, range.room_type_id, numbers, range.floor, range.section_id).await
}

#[allow(clippy::too_many_arguments)]
async fn insert_rooms(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    room_type: Uuid,
    numbers: Vec<String>,
    floor: Option<String>,
    section: Option<Uuid>,
) -> Result<Vec<Room>, RoomsError> {
    let today = business_date(tx, property).await?;
    numbers.iter().try_for_each(|number| check_number(number))?;
    check_references(tx, property, Some(room_type), section).await?;
    let taken: Vec<String> =
        sqlx::query_scalar("select number from room where property_id = $1 and number = any($2) order by number")
            .bind(property)
            .bind(&numbers)
            .fetch_all(&mut **tx)
            .await?;
    if !taken.is_empty() {
        return Err(numbers_taken(&taken));
    }
    extend_window(tx, property).await?;
    let ids: Vec<Uuid> = numbers.iter().map(|_| Uuid::now_v7()).collect();
    let inserted = sqlx::query_as::<_, Room>(sqlx::AssertSqlSafe(format!(
        "insert into room (id, tenant_id, property_id, room_type_id, number, floor, section_id, sort_order)
         select n.id, $1, $2, $3, n.number, $4, $5,
                (select coalesce(max(sort_order) + 1, 0) from room where property_id = $2) + n.position::integer - 1
         from unnest($6::uuid[], $7::text[]) with ordinality as n (id, number, position)
         returning {COLUMNS}"
    )))
    .bind(tenant.0)
    .bind(property)
    .bind(room_type)
    .bind(&floor)
    .bind(section)
    .bind(&ids)
    .bind(&numbers)
    .fetch_all(&mut **tx)
    .await;
    let mut created = match inserted {
        Ok(created) => created,
        // Another transaction took a number after the check above.
        Err(err) if violates(&err, "room_property_id_number_key") => return Err(numbers_taken(&numbers)),
        Err(err) => return Err(err.into()),
    };
    created.sort_by_key(|room| room.sort_order);
    let count = i32::try_from(created.len()).expect("at most MAX_ROOMS_PER_RANGE rooms");
    adjust(tx, property, room_type, today, None, count, 0).await?;
    // One entry for the batch, on the room type the rooms were added to.
    audit(tx, tenant, actor, "rooms.created", "room_type", room_type, serde_json::json!({ "numbers": numbers }))
        .await?;
    let mut keys = vec![rooms_key(property)];
    keys.extend(window_keys(property, today));
    notify(tx, tenant, property, keys).await?;
    Ok(created)
}

/// Changes a room. Retyping, deactivating or reactivating moves its share of the inventory counters
/// (including its out-of-order blocks) from the business date on.
pub async fn update_room(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
    changes: RoomChanges,
) -> Result<Room, RoomsError> {
    let today: Date = business_date(tx, property).await?;
    let current: Room = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {COLUMNS} from room where id = $1 and property_id = $2 for update"
    )))
    .bind(id)
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(RoomsError::NotFound("room"))?;
    if current.version != expected_version {
        return Err(RoomsError::VersionMismatch("room"));
    }
    if let Some(number) = &changes.number {
        check_number(number)?;
    }
    let room_type = changes.room_type_id.unwrap_or(current.room_type_id);
    let active = changes.active.unwrap_or(current.active);
    let type_to_check = (changes.room_type_id.is_some() || (active && !current.active)).then_some(room_type);
    check_references(tx, property, type_to_check, changes.section_id.flatten()).await?;
    let moves_counts = room_type != current.room_type_id || active != current.active;
    if moves_counts {
        extend_window(tx, property).await?;
        contribute(tx, property, today, id, current.room_type_id, current.active, -1).await?;
    }
    let updated = sqlx::query_as::<_, Room>(sqlx::AssertSqlSafe(format!(
        "update room set room_type_id = $2, number = coalesce($3, number),
                floor = case when $4 then $5 else floor end,
                section_id = case when $6 then $7 else section_id end,
                active = $8, version = version + 1
         where id = $1
         returning {COLUMNS}"
    )))
    .bind(id)
    .bind(room_type)
    .bind(&changes.number)
    .bind(changes.floor.is_some())
    .bind(changes.floor.clone().flatten())
    .bind(changes.section_id.is_some())
    .bind(changes.section_id.flatten())
    .bind(active)
    .fetch_one(&mut **tx)
    .await;
    let updated = match updated {
        Ok(updated) => updated,
        Err(err) if violates(&err, "room_property_id_number_key") => {
            return Err(numbers_taken(&[changes.number.unwrap_or_default()]));
        }
        Err(err) => return Err(err.into()),
    };
    let mut keys = vec![rooms_key(property)];
    if moves_counts {
        contribute(tx, property, today, id, updated.room_type_id, updated.active, 1).await?;
        keys.extend(window_keys(property, today));
    }
    audit(
        tx,
        tenant,
        actor,
        "room.updated",
        "room",
        id,
        serde_json::json!({ "room_type_id": room_type, "active": active, "number": updated.number }),
    )
    .await?;
    notify(tx, tenant, property, keys).await?;
    Ok(updated)
}

/// Sets the display order. `ids` must list every room of the property exactly once.
pub async fn reorder_rooms(tx: &mut Tx, tenant: TenantId, property: Uuid, ids: &[Uuid]) -> Result<(), RoomsError> {
    business_date(tx, property).await?;
    reorder(tx, "room", property, ids).await?;
    notify(tx, tenant, property, vec![rooms_key(property)]).await?;
    Ok(())
}

/// The property's rooms, active or not, in display order; only those of `room_type` if given.
pub async fn list_rooms(tx: &mut Tx, property: Uuid, room_type: Option<Uuid>) -> Result<Vec<Room>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {COLUMNS} from room where property_id = $1 and ($2::uuid is null or room_type_id = $2)
         order by sort_order, number"
    )))
    .bind(property)
    .bind(room_type)
    .fetch_all(&mut **tx)
    .await
}
```

Modify `modules/rooms/src/lib.rs`:

```diff
--- a/modules/rooms/src/lib.rs
+++ b/modules/rooms/src/lib.rs
@@ -5,6 +5,7 @@
 
 mod inventory;
 mod room_types;
+mod rooms;
 mod sections;
 
 pub use inventory::{InventoryDay, InventoryDrift, WINDOW_DAYS, extend_window, find_drift, list_inventory, month_keys};
@@ -12,6 +13,10 @@ pub use room_types::{
     Bed, NewRoomType, RoomType, RoomTypeChanges, create_room_type, list_room_types, reorder_room_types,
     update_room_type,
 };
+pub use rooms::{
+    MAX_ROOMS_PER_RANGE, NewRoom, Room, RoomChanges, RoomRange, create_room, create_rooms, list_rooms, reorder_rooms,
+    update_room,
+};
 pub use sections::{Section, create_section, list_sections, rename_section};
 
 use db::{Event, TenantId, Tx, UserId};
```

- [ ] **Step 4: Run the tests to verify they pass**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p rooms
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS (`room_types`: 8, `rooms`: 8). Every test that changes counts also asserts `drift()` is empty.

- [ ] **Step 5: Commit**

```sh
git add -A
git commit -m "feat(rooms): rooms, bulk ranges and reordering, with physical inventory counts"
```

### Task 9: Block reasons and room blocks, and the property-based counter test

Blocks are `[from, to)` on one room: they cannot start before the business date, must end after they start, need an active room and an active reason, and may not overlap another active block on the room (`RoomsError::Overlap` lists the blocks in the way; the check runs under the room's row lock, and the exclusion constraint backs it up). Out-of-order blocks add one to their type's `out_of_order` on each covered day in the window. `shorten_block` ends a block early (the room is back from `to`, which may not be before the business date) and restores the counters for the days it no longer covers; a `to` on or before the start cancels the block. Each property starts with five block reasons (`seed_block_reasons`); properties can add and retire their own.

`counters.rs` is the spec's property-based test: 25 random sequences (1–29 steps each) of room creation, activation, retyping, blocking and shortening, with `find_drift` asserted empty after every step. Operations the rules refuse are skipped, as a user would see them fail. Each run draws new sequences; a failure prints the sequence that broke the counters.

**Files:**
- Create: `modules/rooms/src/blocks.rs`
- Modify: `Cargo.toml`
- Modify: `modules/rooms/Cargo.toml`
- Modify: `modules/rooms/src/lib.rs`
- Test: `modules/rooms/tests/blocks.rs` (new)
- Test: `modules/rooms/tests/counters.rs` (new)
- Test: `modules/rooms/tests/common/mod.rs`
- Test: `modules/rooms/tests/rooms.rs`
- Regenerated, not shown: `Cargo.lock`

**Interfaces:**
- Consumes: Tasks 7–8 (`adjust`, `extend_window`, `month_keys`, the room lock pattern).
- Produces: `rooms::{BlockKind::{OutOfOrder, OutOfService} + as_str/parse, BlockReason, NewBlockReason, BlockReasonChanges, DEFAULT_BLOCK_REASONS, seed_block_reasons(tx, tenant, property), create_block_reason, update_block_reason, list_block_reasons}`; `rooms::{Block { id, property_id, room_id, from, to, kind, reason_id, note, released, version }, NewBlock, create_block, shorten_block(tx, tenant, actor, property, id, expected_version, to), list_blocks(tx, property, from, to)}`; `RoomsError::Overlap(Vec<Block>)`. Test helpers `Hotel::{room(type, number), reason(code)}`; `Hotel::new` now seeds the default reasons, as creating a property over the API will.

- [ ] **Step 1: Write the failing tests**

`proptest` becomes a workspace dependency (dev-only for `rooms`). The shared `room` helper moves from `tests/rooms.rs` into `tests/common`.

Modify `Cargo.toml`:

```diff
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -25,6 +25,7 @@ garde = { version = "0.23", features = ["derive", "email"] }
 getrandom = "0.3"
 http-body-util = "0.1"
 mimalloc = "0.1"
+proptest = "1"
 serde = { version = "1", features = ["derive"] }
 serde_json = "1"
 sha2 = "0.11"
```

Modify `modules/rooms/Cargo.toml`:

```diff
--- a/modules/rooms/Cargo.toml
+++ b/modules/rooms/Cargo.toml
@@ -18,6 +18,7 @@ uuid.workspace = true
 [dev-dependencies]
 db = { workspace = true, features = ["testing"] }
 property.workspace = true
+proptest.workspace = true
 tokio.workspace = true
 
 [lints]
```

Modify `modules/rooms/tests/common/mod.rs`:

```diff
--- a/modules/rooms/tests/common/mod.rs
+++ b/modules/rooms/tests/common/mod.rs
@@ -2,7 +2,7 @@
 
 use db::testing::app_pool;
 use db::{Scope, TenantId, Tx, UserId, begin};
-use rooms::{InventoryDay, NewRoomType, RoomType, WINDOW_DAYS};
+use rooms::{BlockReason, InventoryDay, NewRoom, NewRoomType, Room, RoomType, WINDOW_DAYS};
 use sqlx::PgPool;
 use sqlx::postgres::PgConnectOptions;
 use time::{Date, Duration};
@@ -37,6 +37,7 @@ impl Hotel {
             base_currency: "LKR".into(),
         };
         let property = property::create_property(&mut tx, tenant, user, hotel).await.unwrap();
+        rooms::seed_block_reasons(&mut tx, tenant, property.id).await.unwrap();
         tx.commit().await.unwrap();
         Self { pool, tenant, user, property: property.id, business_date: property.business_date }
     }
@@ -50,6 +51,21 @@ impl Hotel {
         self.business_date + Duration::days(days)
     }
 
+    pub async fn room(&self, room_type: Uuid, number: &str) -> Room {
+        let input = NewRoom { room_type_id: room_type, number: number.into(), floor: None, section_id: None };
+        let mut tx = self.tx().await;
+        let room = rooms::create_room(&mut tx, self.tenant, self.user, self.property, input).await.unwrap();
+        tx.commit().await.unwrap();
+        room
+    }
+
+    /// The seeded block reason with `code`.
+    pub async fn reason(&self, code: &str) -> BlockReason {
+        let mut tx = self.tx().await;
+        let reasons = rooms::list_block_reasons(&mut tx, self.property).await.unwrap();
+        reasons.into_iter().find(|reason| reason.code == code).expect("a seeded reason")
+    }
+
     pub async fn room_type(&self, code: &str) -> RoomType {
         let mut tx = self.tx().await;
         let created = rooms::create_room_type(&mut tx, self.tenant, self.user, self.property, room_type(code)).await;
```

Modify `modules/rooms/tests/rooms.rs`:

```diff
--- a/modules/rooms/tests/rooms.rs
+++ b/modules/rooms/tests/rooms.rs
@@ -6,14 +6,6 @@ use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
 use uuid::Uuid;
 
 impl Hotel {
-    async fn room(&self, room_type: Uuid, number: &str) -> Room {
-        let input = NewRoom { room_type_id: room_type, number: number.into(), floor: None, section_id: None };
-        let mut tx = self.tx().await;
-        let room = rooms::create_room(&mut tx, self.tenant, self.user, self.property, input).await.unwrap();
-        tx.commit().await.unwrap();
-        room
-    }
-
     async fn update_room(&self, room: &Room, changes: RoomChanges) -> Result<Room, RoomsError> {
         let mut tx = self.tx().await;
         let updated =
```

Create `modules/rooms/tests/blocks.rs`:

```rust
mod common;

use common::Hotel;
use rooms::{
    Block, BlockKind, BlockReasonChanges, NewBlock, NewBlockReason, Room, RoomChanges, RoomsError, WINDOW_DAYS,
};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

impl Hotel {
    /// Blocks `room` for `[business date + from, business date + to)`.
    async fn block(&self, room: &Room, from: i64, to: i64, kind: BlockKind) -> Result<Block, RoomsError> {
        let reason = self.reason("MAINTENANCE").await;
        let input = NewBlock {
            room_id: room.id,
            from: self.day(from),
            to: self.day(to),
            kind,
            reason_id: reason.id,
            note: "Leaking pipe".into(),
        };
        let mut tx = self.tx().await;
        let block = rooms::create_block(&mut tx, self.tenant, self.user, self.property, input).await?;
        tx.commit().await.unwrap();
        Ok(block)
    }

    async fn shorten(&self, block: &Block, to: i64) -> Result<Block, RoomsError> {
        let mut tx = self.tx().await;
        let shortened =
            rooms::shorten_block(&mut tx, self.tenant, self.user, self.property, block.id, block.version, self.day(to))
                .await?;
        tx.commit().await.unwrap();
        Ok(shortened)
    }

    /// The days (as offsets from the business date) on which `room_type` has rooms out of order.
    async fn out_of_order_days(&self, room_type: Uuid) -> Vec<i64> {
        let counters = self.counters(room_type).await;
        (0..WINDOW_DAYS).zip(counters).filter(|(_, day)| day.out_of_order > 0).map(|(offset, _)| offset).collect()
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_property_starts_with_the_default_block_reasons(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let mut tx = hotel.tx().await;

    let reasons = rooms::list_block_reasons(&mut tx, hotel.property).await.unwrap();

    let codes: Vec<(&str, BlockKind)> = reasons.iter().map(|r| (r.code.as_str(), r.default_kind)).collect();
    assert_eq!(
        codes,
        [
            ("CONSTRUCTION", BlockKind::OutOfOrder),
            ("DEEP_CLEAN", BlockKind::OutOfService),
            ("MAINTENANCE", BlockKind::OutOfOrder),
            ("OTHER", BlockKind::OutOfOrder),
            ("RENOVATION", BlockKind::OutOfOrder),
        ]
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_out_of_order_block_reduces_availability_on_its_days_only(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let dlx = hotel.room_type("DLX").await;
    let room = hotel.room(dlx.id, "101").await;
    hotel.room(dlx.id, "102").await;

    let block = hotel.block(&room, 2, 5, BlockKind::OutOfOrder).await.unwrap();

    assert_eq!((block.from, block.to, block.version, block.released), (hotel.day(2), hotel.day(5), 1, false));
    assert_eq!(hotel.out_of_order_days(dlx.id).await, vec![2, 3, 4]);
    let counters = hotel.counters(dlx.id).await;
    assert_eq!(counters[2].available(), 1);
    assert_eq!(counters[5].available(), 2);
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn an_out_of_service_block_leaves_availability_alone(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let dlx = hotel.room_type("DLX").await;
    let room = hotel.room(dlx.id, "101").await;

    hotel.block(&room, 0, 3, BlockKind::OutOfService).await.unwrap();

    assert_eq!(hotel.out_of_order_days(dlx.id).await, Vec::<i64>::new());
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn overlapping_blocks_conflict_and_name_the_block_in_the_way(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let dlx = hotel.room_type("DLX").await;
    let room = hotel.room(dlx.id, "101").await;
    let first = hotel.block(&room, 2, 5, BlockKind::OutOfOrder).await.unwrap();

    let overlapping = hotel.block(&room, 4, 6, BlockKind::OutOfService).await;
    let adjacent = hotel.block(&room, 5, 7, BlockKind::OutOfOrder).await;

    let Err(RoomsError::Overlap(conflicts)) = overlapping else { panic!("expected an overlap, got {overlapping:?}") };
    assert_eq!(conflicts, vec![first]);
    assert!(adjacent.is_ok(), "[5, 7) starts the day [2, 5) ends: {adjacent:?}");
    assert_eq!(hotel.out_of_order_days(dlx.id).await, vec![2, 3, 4, 5, 6]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn blocks_start_on_or_after_the_business_date_and_end_after_they_start(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let dlx = hotel.room_type("DLX").await;
    let room = hotel.room(dlx.id, "101").await;

    let in_the_past = hotel.block(&room, -1, 2, BlockKind::OutOfOrder).await;
    let empty = hotel.block(&room, 3, 3, BlockKind::OutOfOrder).await;

    assert!(matches!(in_the_past, Err(RoomsError::Invalid(_))), "{in_the_past:?}");
    assert!(matches!(empty, Err(RoomsError::Invalid(_))), "{empty:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn releasing_early_restores_the_remaining_days(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let dlx = hotel.room_type("DLX").await;
    let room = hotel.room(dlx.id, "101").await;
    let block = hotel.block(&room, 0, 10, BlockKind::OutOfOrder).await.unwrap();

    let released = hotel.shorten(&block, 4).await.unwrap();
    let stale = hotel.shorten(&block, 2).await;
    let longer = hotel.shorten(&released, 6).await;

    assert_eq!(
        (released.from, released.to, released.version, released.released),
        (hotel.day(0), hotel.day(4), 2, false)
    );
    assert_eq!(hotel.out_of_order_days(dlx.id).await, vec![0, 1, 2, 3]);
    assert!(matches!(stale, Err(RoomsError::VersionMismatch("block"))), "{stale:?}");
    assert!(matches!(longer, Err(RoomsError::Invalid(_))), "{longer:?}");
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn cancelling_before_the_start_frees_every_day(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let dlx = hotel.room_type("DLX").await;
    let room = hotel.room(dlx.id, "101").await;
    let block = hotel.block(&room, 5, 8, BlockKind::OutOfOrder).await.unwrap();

    let cancelled = hotel.shorten(&block, 1).await.unwrap();
    let again = hotel.block(&room, 5, 8, BlockKind::OutOfOrder).await;
    let mut tx = hotel.tx().await;
    let listed = rooms::list_blocks(&mut tx, hotel.property, hotel.day(0), hotel.day(30)).await.unwrap();

    assert!(cancelled.released);
    assert_eq!((cancelled.from, cancelled.to), (hotel.day(5), hotel.day(8)));
    let again = again.unwrap();
    assert_eq!(listed, vec![again]);
    assert_eq!(hotel.out_of_order_days(dlx.id).await, vec![5, 6, 7]);
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_blocked_room_takes_its_blocks_along_when_retyped_or_deactivated(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let std = hotel.room_type("STD").await;
    let dlx = hotel.room_type("DLX").await;
    let room = hotel.room(std.id, "101").await;
    hotel.block(&room, 1, 3, BlockKind::OutOfOrder).await.unwrap();

    let mut tx = hotel.tx().await;
    let retype = RoomChanges { room_type_id: Some(dlx.id), ..RoomChanges::default() };
    let retyped =
        rooms::update_room(&mut tx, hotel.tenant, hotel.user, hotel.property, room.id, 1, retype).await.unwrap();
    tx.commit().await.unwrap();
    let moved = (hotel.out_of_order_days(std.id).await, hotel.out_of_order_days(dlx.id).await);
    let mut tx = hotel.tx().await;
    let deactivate = RoomChanges { active: Some(false), ..RoomChanges::default() };
    rooms::update_room(&mut tx, hotel.tenant, hotel.user, hotel.property, room.id, retyped.version, deactivate)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(moved, (vec![], vec![1, 2]));
    assert_eq!(hotel.out_of_order_days(dlx.id).await, Vec::<i64>::new());
    assert_eq!(hotel.drift().await, vec![]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn inactive_rooms_and_unknown_reasons_cannot_be_blocked(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let dlx = hotel.room_type("DLX").await;
    let room = hotel.room(dlx.id, "101").await;
    let mut tx = hotel.tx().await;
    let deactivate = RoomChanges { active: Some(false), ..RoomChanges::default() };
    let inactive =
        rooms::update_room(&mut tx, hotel.tenant, hotel.user, hotel.property, room.id, 1, deactivate).await.unwrap();
    tx.commit().await.unwrap();
    let active = hotel.room(dlx.id, "102").await;

    let on_inactive = hotel.block(&inactive, 0, 2, BlockKind::OutOfOrder).await;
    let unknown_reason = NewBlock {
        room_id: active.id,
        from: hotel.day(0),
        to: hotel.day(2),
        kind: BlockKind::OutOfOrder,
        reason_id: Uuid::now_v7(),
        note: String::new(),
    };
    let mut tx = hotel.tx().await;
    let unknown_reason = rooms::create_block(&mut tx, hotel.tenant, hotel.user, hotel.property, unknown_reason).await;

    assert!(matches!(on_inactive, Err(RoomsError::Invalid(_))), "{on_inactive:?}");
    assert!(matches!(unknown_reason, Err(RoomsError::Invalid(_))), "{unknown_reason:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_property_adds_and_retires_its_own_block_reasons(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let dlx = hotel.room_type("DLX").await;
    let room = hotel.room(dlx.id, "101").await;
    let pest =
        NewBlockReason { code: "PEST".into(), label: "Pest control".into(), default_kind: BlockKind::OutOfOrder };

    let mut tx = hotel.tx().await;
    let created =
        rooms::create_block_reason(&mut tx, hotel.tenant, hotel.user, hotel.property, pest.clone()).await.unwrap();
    let retire =
        BlockReasonChanges { active: Some(false), label: Some("Pests".into()), ..BlockReasonChanges::default() };
    let retired = rooms::update_block_reason(&mut tx, hotel.tenant, hotel.user, hotel.property, created.id, 1, retire)
        .await
        .unwrap();
    let stale = rooms::update_block_reason(
        &mut tx,
        hotel.tenant,
        hotel.user,
        hotel.property,
        created.id,
        1,
        BlockReasonChanges::default(),
    )
    .await;
    tx.commit().await.unwrap();
    let duplicate =
        rooms::create_block_reason(&mut hotel.tx().await, hotel.tenant, hotel.user, hotel.property, pest).await;
    let use_retired = NewBlock {
        room_id: room.id,
        from: hotel.day(0),
        to: hotel.day(1),
        kind: BlockKind::OutOfOrder,
        reason_id: retired.id,
        note: String::new(),
    };
    let use_retired =
        rooms::create_block(&mut hotel.tx().await, hotel.tenant, hotel.user, hotel.property, use_retired).await;

    assert_eq!((retired.label.as_str(), retired.active, retired.version), ("Pests", false, 2));
    assert!(matches!(stale, Err(RoomsError::VersionMismatch("block reason"))), "{stale:?}");
    assert!(matches!(duplicate, Err(RoomsError::Conflict(_))), "{duplicate:?}");
    assert!(matches!(use_retired, Err(RoomsError::Invalid(_))), "{use_retired:?}");
}
```

Create `modules/rooms/tests/counters.rs`:

```rust
//! Property-based check of the inventory counters: random sequences of room and block changes must leave
//! `inventory_day` equal to a recomputation from rooms and blocks (`rooms::find_drift`).

mod common;

use common::Hotel;
use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::TestRunner;
use rooms::{Block, BlockKind, NewBlock, NewRoom, Room, RoomChanges, RoomType};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

/// Sequences tried per run. Each run draws new ones; a failure prints the sequence that broke the counters.
const SEQUENCES: usize = 25;

#[derive(Debug, Clone)]
enum Op {
    CreateRoom { room_type: usize },
    SetActive { room: usize, active: bool },
    Retype { room: usize, room_type: usize },
    Block { room: usize, start: i64, days: i64, out_of_order: bool },
    Shorten { block: usize, to: i64 },
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (0..3usize).prop_map(|room_type| Op::CreateRoom { room_type }),
        (0..8usize, any::<bool>()).prop_map(|(room, active)| Op::SetActive { room, active }),
        (0..8usize, 0..3usize).prop_map(|(room, room_type)| Op::Retype { room, room_type }),
        (0..8usize, 0..40i64, 1..20i64, any::<bool>()).prop_map(|(room, start, days, out_of_order)| Op::Block {
            room,
            start,
            days,
            out_of_order
        }),
        (0..8usize, 0..40i64).prop_map(|(block, to)| Op::Shorten { block, to }),
    ]
}

/// What one sequence has created so far, so operations can refer to rooms and blocks by index.
struct Sequence<'a> {
    hotel: &'a Hotel,
    types: &'a [RoomType],
    name: usize,
    rooms: Vec<Room>,
    blocks: Vec<Block>,
}

impl Sequence<'_> {
    /// Applies `op` in its own transaction. Operations the rules refuse (an overlapping block, a room that is
    /// inactive) are rolled back and skipped, as they would be for a user.
    async fn apply(&mut self, op: &Op) {
        let hotel = self.hotel;
        let mut tx = hotel.tx().await;
        let (tenant, user, property) = (hotel.tenant, hotel.user, hotel.property);
        match *op {
            Op::CreateRoom { room_type } => {
                let number = format!("{}-{}", self.name, self.rooms.len());
                let input = NewRoom { room_type_id: self.types[room_type].id, number, floor: None, section_id: None };
                if let Ok(room) = rooms::create_room(&mut tx, tenant, user, property, input).await {
                    self.rooms.push(room);
                }
            }
            Op::SetActive { room, active } => {
                let Some(current) = self.rooms.get(room).cloned() else { return };
                let changes = RoomChanges { active: Some(active), ..RoomChanges::default() };
                if let Ok(updated) =
                    rooms::update_room(&mut tx, tenant, user, property, current.id, current.version, changes).await
                {
                    self.rooms[room] = updated;
                }
            }
            Op::Retype { room, room_type } => {
                let Some(current) = self.rooms.get(room).cloned() else { return };
                let changes = RoomChanges { room_type_id: Some(self.types[room_type].id), ..RoomChanges::default() };
                if let Ok(updated) =
                    rooms::update_room(&mut tx, tenant, user, property, current.id, current.version, changes).await
                {
                    self.rooms[room] = updated;
                }
            }
            Op::Block { room, start, days, out_of_order } => {
                let Some(current) = self.rooms.get(room) else { return };
                let reason = hotel.reason("OTHER").await;
                let input = NewBlock {
                    room_id: current.id,
                    from: hotel.day(start),
                    to: hotel.day(start + days),
                    kind: if out_of_order { BlockKind::OutOfOrder } else { BlockKind::OutOfService },
                    reason_id: reason.id,
                    note: String::new(),
                };
                if let Ok(block) = rooms::create_block(&mut tx, tenant, user, property, input).await {
                    self.blocks.push(block);
                }
            }
            Op::Shorten { block, to } => {
                let Some(current) = self.blocks.get(block).cloned() else { return };
                if let Ok(shortened) =
                    rooms::shorten_block(&mut tx, tenant, user, property, current.id, current.version, hotel.day(to))
                        .await
                {
                    self.blocks[block] = shortened;
                }
            }
        }
        tx.commit().await.unwrap();
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn counters_match_a_recount_after_any_sequence_of_changes(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let types = [hotel.room_type("A").await, hotel.room_type("B").await, hotel.room_type("C").await];
    let sequences = proptest::collection::vec(op(), 1..30);
    let mut runner = TestRunner::default();

    for name in 0..SEQUENCES {
        let ops = sequences.new_tree(&mut runner).unwrap().current();
        let mut sequence = Sequence { hotel: &hotel, types: &types, name, rooms: Vec::new(), blocks: Vec::new() };
        for (step, op) in ops.iter().enumerate() {
            sequence.apply(op).await;
            let drift = hotel.drift().await;
            assert!(drift.is_empty(), "counters drifted after step {step} of {ops:?}: {drift:?}");
        }
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p rooms
```

Expected: FAIL to compile: `unresolved import rooms::BlockReason`, `cannot find function seed_block_reasons in crate rooms`, …

- [ ] **Step 3: Implement**

Create `modules/rooms/src/blocks.rs`:

```rust
use crate::inventory::{adjust, business_date, extend_window, month_keys};
use crate::{RoomsError, audit, notify, rooms_key, violates};
use db::{TenantId, Tx, UserId};
use serde::{Deserialize, Serialize};
use time::Date;
use uuid::Uuid;

/// `OutOfOrder` takes the room out of inventory (renovation, construction); `OutOfService` leaves it
/// sellable and only flags it (a short fix, a deep clean).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum BlockKind {
    OutOfOrder,
    OutOfService,
}

impl BlockKind {
    pub fn as_str(self) -> &'static str {
        match self {
            BlockKind::OutOfOrder => "out_of_order",
            BlockKind::OutOfService => "out_of_service",
        }
    }

    pub fn parse(value: &str) -> Option<BlockKind> {
        [BlockKind::OutOfOrder, BlockKind::OutOfService].into_iter().find(|kind| kind.as_str() == value)
    }
}

impl TryFrom<String> for BlockKind {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        BlockKind::parse(&value).ok_or_else(|| format!("unknown block kind {value:?}"))
    }
}

/// Why a room is blocked. Each property starts with [`DEFAULT_BLOCK_REASONS`] and may add its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct BlockReason {
    pub id: Uuid,
    pub property_id: Uuid,
    pub code: String,
    pub label: String,
    #[sqlx(try_from = "String")]
    pub default_kind: BlockKind,
    pub active: bool,
    pub version: i32,
}

/// Reasons every new property starts with (`migrations/0004_rooms_inventory.sql` seeds existing ones).
pub const DEFAULT_BLOCK_REASONS: [(&str, &str, BlockKind); 5] = [
    ("RENOVATION", "Renovation", BlockKind::OutOfOrder),
    ("CONSTRUCTION", "Construction", BlockKind::OutOfOrder),
    ("MAINTENANCE", "Maintenance", BlockKind::OutOfOrder),
    ("DEEP_CLEAN", "Deep clean", BlockKind::OutOfService),
    ("OTHER", "Other", BlockKind::OutOfOrder),
];

#[derive(Debug, Clone)]
pub struct NewBlockReason {
    pub code: String,
    pub label: String,
    pub default_kind: BlockKind,
}

/// `None` leaves a field unchanged. The code never changes.
#[derive(Debug, Clone, Default)]
pub struct BlockReasonChanges {
    pub label: Option<String>,
    pub default_kind: Option<BlockKind>,
    pub active: Option<bool>,
}

/// A room blocked for `[from, to)`: `to` is the first day it is back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct Block {
    pub id: Uuid,
    pub property_id: Uuid,
    pub room_id: Uuid,
    pub from: Date,
    pub to: Date,
    #[sqlx(try_from = "String")]
    pub kind: BlockKind,
    pub reason_id: Uuid,
    pub note: String,
    /// Cancelled before it started. Released blocks no longer block anything.
    pub released: bool,
    pub version: i32,
}

#[derive(Debug, Clone)]
pub struct NewBlock {
    pub room_id: Uuid,
    pub from: Date,
    pub to: Date,
    pub kind: BlockKind,
    pub reason_id: Uuid,
    pub note: String,
}

const REASON_COLUMNS: &str = "id, property_id, code, label, default_kind, active, version";
const BLOCK_COLUMNS: &str = "id, property_id, room_id, lower(period) as \"from\", upper(period) as \"to\", kind, \
                             reason_id, note, released_at is not null as released, version";

/// Gives a new property the default block reasons, in the property's creation transaction.
pub async fn seed_block_reasons(tx: &mut Tx, tenant: TenantId, property: Uuid) -> Result<(), sqlx::Error> {
    for (code, label, kind) in DEFAULT_BLOCK_REASONS {
        sqlx::query(
            "insert into block_reason (id, tenant_id, property_id, code, label, default_kind)
             values ($1, $2, $3, $4, $5, $6)",
        )
        .bind(Uuid::now_v7())
        .bind(tenant.0)
        .bind(property)
        .bind(code)
        .bind(label)
        .bind(kind.as_str())
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

pub async fn create_block_reason(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    input: NewBlockReason,
) -> Result<BlockReason, RoomsError> {
    business_date(tx, property).await?;
    let inserted = sqlx::query_as::<_, BlockReason>(sqlx::AssertSqlSafe(format!(
        "insert into block_reason (id, tenant_id, property_id, code, label, default_kind)
         values ($1, $2, $3, $4, $5, $6)
         returning {REASON_COLUMNS}"
    )))
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(property)
    .bind(&input.code)
    .bind(&input.label)
    .bind(input.default_kind.as_str())
    .fetch_one(&mut **tx)
    .await;
    let reason = match inserted {
        Ok(reason) => reason,
        Err(err) if violates(&err, "block_reason_property_id_code_key") => {
            return Err(RoomsError::Conflict(format!("a block reason with code {} already exists", input.code)));
        }
        Err(err) => return Err(err.into()),
    };
    audit(
        tx,
        tenant,
        actor,
        "block_reason.created",
        "block_reason",
        reason.id,
        serde_json::json!({ "code": input.code }),
    )
    .await?;
    notify(tx, tenant, property, vec![rooms_key(property)]).await?;
    Ok(reason)
}

pub async fn update_block_reason(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
    changes: BlockReasonChanges,
) -> Result<BlockReason, RoomsError> {
    let updated: Option<BlockReason> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "update block_reason set label = coalesce($4, label), default_kind = coalesce($5, default_kind),
                active = coalesce($6, active), version = version + 1
         where id = $1 and property_id = $2 and version = $3
         returning {REASON_COLUMNS}"
    )))
    .bind(id)
    .bind(property)
    .bind(expected_version)
    .bind(&changes.label)
    .bind(changes.default_kind.map(BlockKind::as_str))
    .bind(changes.active)
    .fetch_optional(&mut **tx)
    .await?;
    let Some(reason) = updated else {
        let exists: bool =
            sqlx::query_scalar("select exists (select 1 from block_reason where id = $1 and property_id = $2)")
                .bind(id)
                .bind(property)
                .fetch_one(&mut **tx)
                .await?;
        return Err(if exists {
            RoomsError::VersionMismatch("block reason")
        } else {
            RoomsError::NotFound("block reason")
        });
    };
    audit(
        tx,
        tenant,
        actor,
        "block_reason.updated",
        "block_reason",
        id,
        serde_json::json!({ "active": changes.active }),
    )
    .await?;
    notify(tx, tenant, property, vec![rooms_key(property)]).await?;
    Ok(reason)
}

/// Every block reason of the property, active or not, by code.
pub async fn list_block_reasons(tx: &mut Tx, property: Uuid) -> Result<Vec<BlockReason>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {REASON_COLUMNS} from block_reason where property_id = $1 order by code"
    )))
    .bind(property)
    .fetch_all(&mut **tx)
    .await
}

/// Active blocks of `room` that overlap `[from, to)`.
async fn overlapping(tx: &mut Tx, room: Uuid, from: Date, to: Date) -> Result<Vec<Block>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {BLOCK_COLUMNS} from room_block
         where room_id = $1 and released_at is null and period && daterange($2, $3)
         order by lower(period)"
    )))
    .bind(room)
    .bind(from)
    .bind(to)
    .fetch_all(&mut **tx)
    .await
}

/// Locks the room (serializing its blocks and retyping) and returns its type and whether it is active.
async fn lock_room(tx: &mut Tx, property: Uuid, room: Uuid) -> Result<(Uuid, bool), RoomsError> {
    sqlx::query_as("select room_type_id, active from room where id = $1 and property_id = $2 for update")
        .bind(room)
        .bind(property)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(RoomsError::NotFound("room"))
}

/// Blocks a room for `[from, to)`. Out-of-order blocks take it out of its type's availability on those days.
/// Fails with [`RoomsError::Overlap`], listing the blocks in the way, if the room is already blocked then.
pub async fn create_block(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    input: NewBlock,
) -> Result<Block, RoomsError> {
    let today = business_date(tx, property).await?;
    if input.to <= input.from {
        return Err(RoomsError::Invalid("a block must end after it starts".into()));
    }
    if input.from < today {
        return Err(RoomsError::Invalid(format!("blocks cannot start before the business date ({today})")));
    }
    let (room_type, active) = lock_room(tx, property, input.room_id).await?;
    if !active {
        return Err(RoomsError::Invalid("the room is inactive".into()));
    }
    let reason_active: Option<bool> =
        sqlx::query_scalar("select active from block_reason where id = $1 and property_id = $2")
            .bind(input.reason_id)
            .bind(property)
            .fetch_optional(&mut **tx)
            .await?;
    if reason_active != Some(true) {
        return Err(RoomsError::Invalid("no such active block reason in this property".into()));
    }
    let conflicts = overlapping(tx, input.room_id, input.from, input.to).await?;
    if !conflicts.is_empty() {
        return Err(RoomsError::Overlap(conflicts));
    }
    extend_window(tx, property).await?;
    let inserted = sqlx::query_as::<_, Block>(sqlx::AssertSqlSafe(format!(
        "insert into room_block (id, tenant_id, property_id, room_id, period, kind, reason_id, note, created_by)
         values ($1, $2, $3, $4, daterange($5, $6), $7, $8, $9, $10)
         returning {BLOCK_COLUMNS}"
    )))
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(property)
    .bind(input.room_id)
    .bind(input.from)
    .bind(input.to)
    .bind(input.kind.as_str())
    .bind(input.reason_id)
    .bind(&input.note)
    .bind(actor.0)
    .fetch_one(&mut **tx)
    .await;
    let block = match inserted {
        Ok(block) => block,
        // The room lock above makes this unreachable in practice; the constraint is the last line of defence.
        Err(err) if violates(&err, "room_block_no_overlap") => return Err(RoomsError::Overlap(Vec::new())),
        Err(err) => return Err(err.into()),
    };
    if block.kind == BlockKind::OutOfOrder {
        adjust(tx, property, room_type, block.from, Some(block.to), 0, 1).await?;
    }
    audit(
        tx,
        tenant,
        actor,
        "room_block.created",
        "room_block",
        block.id,
        serde_json::json!({ "room_id": block.room_id, "from": block.from, "to": block.to, "kind": block.kind }),
    )
    .await?;
    notify(tx, tenant, property, month_keys(property, block.from, block.to)).await?;
    Ok(block)
}

/// Ends a block early: the room is back from `to`, which may not be before the business date and must be
/// before the block's current end. If `to` is on or before the start, the block is cancelled (released).
/// Counters are restored for the days the block no longer covers.
pub async fn shorten_block(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
    to: Date,
) -> Result<Block, RoomsError> {
    let today = business_date(tx, property).await?;
    let room: Option<Uuid> =
        sqlx::query_scalar("select room_id from room_block where id = $1 and property_id = $2 and released_at is null")
            .bind(id)
            .bind(property)
            .fetch_optional(&mut **tx)
            .await?;
    let room = room.ok_or(RoomsError::NotFound("block"))?;
    let (room_type, room_active) = lock_room(tx, property, room).await?;
    let current: Block =
        sqlx::query_as(sqlx::AssertSqlSafe(format!("select {BLOCK_COLUMNS} from room_block where id = $1 for update")))
            .bind(id)
            .fetch_one(&mut **tx)
            .await?;
    if current.version != expected_version {
        return Err(RoomsError::VersionMismatch("block"));
    }
    if to < today || to >= current.to {
        return Err(RoomsError::Invalid(format!(
            "a block can only be shortened, to end between the business date ({today}) and {}",
            current.to
        )));
    }
    let cancelled = to <= current.from;
    let block: Block = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "update room_block set
             period = case when $2 then period else daterange(lower(period), $3) end,
             released_at = case when $2 then now() end,
             version = version + 1
         where id = $1
         returning {BLOCK_COLUMNS}"
    )))
    .bind(id)
    .bind(cancelled)
    .bind(to)
    .fetch_one(&mut **tx)
    .await?;
    // Days the block covered from the business date on, and no longer does.
    let restored_from = if cancelled { current.from.max(today) } else { to };
    if current.kind == BlockKind::OutOfOrder && room_active {
        adjust(tx, property, room_type, restored_from, Some(current.to), 0, -1).await?;
    }
    audit(
        tx,
        tenant,
        actor,
        if cancelled { "room_block.cancelled" } else { "room_block.shortened" },
        "room_block",
        id,
        serde_json::json!({ "to": to }),
    )
    .await?;
    notify(tx, tenant, property, month_keys(property, restored_from, current.to)).await?;
    Ok(block)
}

/// Active blocks overlapping `[from, to)`, by start date.
pub async fn list_blocks(tx: &mut Tx, property: Uuid, from: Date, to: Date) -> Result<Vec<Block>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {BLOCK_COLUMNS} from room_block
         where property_id = $1 and released_at is null and period && daterange($2, $3)
         order by lower(period), room_id"
    )))
    .bind(property)
    .bind(from)
    .bind(to)
    .fetch_all(&mut **tx)
    .await
}
```

Modify `modules/rooms/src/lib.rs`:

```diff
--- a/modules/rooms/src/lib.rs
+++ b/modules/rooms/src/lib.rs
@@ -3,11 +3,16 @@
 //! Every function takes a transaction scoped to the caller's tenant and checks that ids belong to the
 //! given property. Writes record an audit entry and queue change events in the same transaction.
 
+mod blocks;
 mod inventory;
 mod room_types;
 mod rooms;
 mod sections;
 
+pub use blocks::{
+    Block, BlockKind, BlockReason, BlockReasonChanges, DEFAULT_BLOCK_REASONS, NewBlock, NewBlockReason, create_block,
+    create_block_reason, list_block_reasons, list_blocks, seed_block_reasons, shorten_block, update_block_reason,
+};
 pub use inventory::{InventoryDay, InventoryDrift, WINDOW_DAYS, extend_window, find_drift, list_inventory, month_keys};
 pub use room_types::{
     Bed, NewRoomType, RoomType, RoomTypeChanges, create_room_type, list_room_types, reorder_room_types,
@@ -36,6 +41,9 @@ pub enum RoomsError {
     /// A business rule, such as a capacity that does not add up or an unknown room type.
     #[error("{0}")]
     Invalid(String),
+    /// The room is already blocked on some of the dates; lists the blocks in the way.
+    #[error("the room is already blocked on some of these dates")]
+    Overlap(Vec<Block>),
     #[error(transparent)]
     Database(#[from] sqlx::Error),
 }
```

- [ ] **Step 4: Run the tests to verify they pass**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p rooms
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: PASS (`blocks`: 10, `counters`: 1 — about 15 s, `room_types`: 8, `rooms`: 8). To see the property test bite, make `contribute` skip removals (`let sign = if sign < 0 { 0 } else { sign };`): `counters` fails within a few steps and prints the sequence; undo the change.

- [ ] **Step 5: Commit**

```sh
git add -A
git commit -m "feat(rooms): block reasons and room blocks with out-of-order counts, checked by a property-based test"
```

### Task 10: REST commands for room types, rooms and sections

The module's commands over HTTP. Creates are mounted in the `commands` router (idempotency keys) and answer 201 with `ETag`; updates take `IfMatch`; reorders are `PUT …/order` with every id in the new order and answer 204. `RoomsError` maps to problems in one place (`routes::rooms::rooms_error`): not found 404, version mismatch 412, conflict 409, business rule 422. `PATCH …/rooms/{room}` tells a field sent as `null` (clear it) from one left out (keep it). All require `RoomsManage` for the path's property.

Each Phase 1 route sets `operation_id`: several handlers share names (`create`, `update`), and utoipa's default ids (the function name) collided, which made the generated TypeScript merge different operations. A test now pins that every operation id is unique.

**Files:**
- Create: `crates/core-api/src/routes/room_types.rs`
- Create: `crates/core-api/src/routes/rooms.rs`
- Modify: `crates/core-api/Cargo.toml`
- Modify: `crates/core-api/src/openapi.rs`
- Modify: `crates/core-api/src/routes/mod.rs`
- Modify: `crates/core-api/src/routes/properties.rs`
- Test: `crates/core-api/tests/rooms.rs` (new)
- Test: `crates/core-api/tests/openapi.rs`
- Regenerated, not shown: `Cargo.lock`, `web/pms/src/lib/api/openapi.d.ts`, `web/pms/src/lib/api/openapi.json`

**Interfaces:**
- Consumes: `rooms::*` (Tasks 7–9), `concurrency::{IfMatch, Versioned}`, `extract::ApiPath`.
- Produces: `POST /api/v1/properties/{property}/room-types`, `PATCH …/room-types/{room_type}`, `PUT …/room-types/order`, `POST …/rooms`, `POST …/rooms/bulk`, `PATCH …/rooms/{room}`, `PUT …/rooms/order`, `POST …/sections`, `PATCH …/sections/{section}`; operation ids `create_property`, `update_property`, `create_room_type`, `update_room_type`, `reorder_room_types`, `create_room`, `create_rooms`, `update_room`, `reorder_rooms`, `create_section`, `rename_section`; `routes::rooms::{rooms_error, ReorderRequest}`.

- [ ] **Step 1: Write the failing tests**

Create `crates/core-api/tests/rooms.rs`:

```rust
mod common;

use axum::http::{Method, StatusCode, header};
use common::{TestApp, TestResponse, uuid};
use core_api::events::{LiveEvent, spawn_listener};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::time::Duration;
use uuid::Uuid;

/// Sends a create command with a fresh idempotency key.
async fn post(app: &TestApp, cookie: &str, path: &str, body: Value) -> TestResponse {
    let key = Uuid::now_v7().to_string();
    app.send_with(Method::POST, path, Some(cookie), Some(body), &[("x-goodfolk-csrf", "1"), ("idempotency-key", &key)])
        .await
}

async fn patch(app: &TestApp, cookie: &str, path: &str, version: i64, body: Value) -> TestResponse {
    let if_match = format!("\"{version}\"");
    app.send_with(Method::PATCH, path, Some(cookie), Some(body), &[("x-goodfolk-csrf", "1"), ("if-match", &if_match)])
        .await
}

/// An owner with one property; returns the owner's cookie and the property's API path.
async fn hotel(app: &TestApp) -> (String, String) {
    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
    let property = json!({"code": "GAL", "name": "Galle", "timezone": "Asia/Colombo", "base_currency": "LKR"});
    let created = post(app, &owner, "/api/v1/properties", property).await;
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    (owner, format!("/api/v1/properties/{}", created.body["id"].as_str().unwrap()))
}

fn deluxe() -> Value {
    json!({
        "code": "DLX", "name": "Deluxe", "base_occupancy": 2, "max_adults": 2, "max_children": 1, "max_occupancy": 3,
        "bed_config": [{"kind": "king", "count": 1}], "amenities": ["Sea view"],
    })
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_room_type_is_created_updated_and_reordered(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let superuser = PgPool::connect_with(opts).await.unwrap();
    let (owner, property) = hotel(&app).await;

    let created = post(&app, &owner, &format!("{property}/room-types"), deluxe()).await;
    let standard = json!({"code": "STD", "name": "Standard", "base_occupancy": 1, "max_adults": 2,
                          "max_children": 0, "max_occupancy": 2});
    let standard = post(&app, &owner, &format!("{property}/room-types"), standard).await;
    let dlx = format!("{property}/room-types/{}", created.body["id"].as_str().unwrap());
    let updated = patch(&app, &owner, &dlx, 1, json!({"name": "Deluxe Sea View", "amenities": []})).await;
    let order = json!({"ids": [standard.body["id"], created.body["id"]]});
    let reordered = app.send(Method::PUT, &format!("{property}/room-types/order"), Some(&owner), Some(order)).await;

    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    assert_eq!(created.headers[header::ETAG], "\"1\"");
    assert_eq!(created.body["bed_config"], json!([{"kind": "king", "count": 1}]));
    assert_eq!(updated.status, StatusCode::OK, "{:?}", updated.body);
    assert_eq!(updated.headers[header::ETAG], "\"2\"");
    assert_eq!(
        (updated.body["name"].as_str(), updated.body["amenities"].clone()),
        (Some("Deluxe Sea View"), json!([]))
    );
    assert_eq!(reordered.status, StatusCode::NO_CONTENT, "{:?}", reordered.body);
    let codes: Vec<String> =
        sqlx::query_scalar("select code from room_type order by sort_order").fetch_all(&superuser).await.unwrap();
    assert_eq!(codes, ["STD", "DLX"]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn room_type_rules_are_problems(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;
    let (owner, property) = hotel(&app).await;
    let path = format!("{property}/room-types");
    post(&app, &owner, &path, deluxe()).await;

    let duplicate = post(&app, &owner, &path, deluxe()).await;
    let too_many_guests = post(
        &app,
        &owner,
        &path,
        json!({"code": "TWN", "name": "Twin", "base_occupancy": 2,
        "max_adults": 2, "max_children": 0, "max_occupancy": 3}),
    )
    .await;
    let lower_case = post(
        &app,
        &owner,
        &path,
        json!({"code": "dlx", "name": "D", "base_occupancy": 1,
        "max_adults": 1, "max_children": 0, "max_occupancy": 1}),
    )
    .await;
    let unknown_property =
        post(&app, &owner, &format!("/api/v1/properties/{}/room-types", Uuid::now_v7()), deluxe()).await;

    assert_eq!(duplicate.status, StatusCode::CONFLICT, "{:?}", duplicate.body);
    assert_eq!(too_many_guests.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", too_many_guests.body);
    assert_eq!(lower_case.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", lower_case.body);
    assert_eq!(unknown_property.status, StatusCode::NOT_FOUND, "{:?}", unknown_property.body);
    assert_eq!(duplicate.headers[header::CONTENT_TYPE], "application/problem+json");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn rooms_are_added_one_at_a_time_or_as_a_range(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;
    let (owner, property) = hotel(&app).await;
    let room_type = post(&app, &owner, &format!("{property}/room-types"), deluxe()).await.body["id"].clone();
    let section = post(&app, &owner, &format!("{property}/sections"), json!({"name": "East"})).await;

    let single = post(
        &app,
        &owner,
        &format!("{property}/rooms"),
        json!({"room_type_id": room_type, "number": "100", "floor": "G", "section_id": section.body["id"]}),
    )
    .await;
    let range = json!({"room_type_id": room_type, "first": 101, "last": 105, "floor": "1"});
    let key = [("x-goodfolk-csrf", "1"), ("idempotency-key", "bulk-rooms-0001")];
    let bulk =
        app.send_with(Method::POST, &format!("{property}/rooms/bulk"), Some(&owner), Some(range.clone()), &key).await;
    let retry =
        app.send_with(Method::POST, &format!("{property}/rooms/bulk"), Some(&owner), Some(range.clone()), &key).await;
    let overlapping = post(
        &app,
        &owner,
        &format!("{property}/rooms/bulk"),
        json!({"room_type_id": room_type, "first": 105, "last": 106}),
    )
    .await;
    let no_key = app
        .send(
            Method::POST,
            &format!("{property}/rooms"),
            Some(&owner),
            Some(json!({"room_type_id": room_type, "number": "200"})),
        )
        .await;

    assert_eq!(section.status, StatusCode::CREATED, "{:?}", section.body);
    assert_eq!(single.status, StatusCode::CREATED, "{:?}", single.body);
    assert_eq!(single.body["section_id"], section.body["id"]);
    assert_eq!(bulk.status, StatusCode::CREATED, "{:?}", bulk.body);
    let numbers: Vec<&str> = bulk.body.as_array().unwrap().iter().map(|r| r["number"].as_str().unwrap()).collect();
    assert_eq!(numbers, ["101", "102", "103", "104", "105"]);
    assert_eq!(retry.body, bulk.body);
    assert_eq!(overlapping.status, StatusCode::CONFLICT, "{:?}", overlapping.body);
    assert_eq!(overlapping.body["detail"], "room 105 already exists");
    assert_eq!(no_key.status, StatusCode::BAD_REQUEST);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_room_update_needs_the_current_version(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts).await;
    let (owner, property) = hotel(&app).await;
    let room_type = post(&app, &owner, &format!("{property}/room-types"), deluxe()).await.body["id"].clone();
    let room = post(
        &app,
        &owner,
        &format!("{property}/rooms"),
        json!({"room_type_id": room_type, "number": "101", "floor": "1"}),
    )
    .await;
    let path = format!("{property}/rooms/{}", room.body["id"].as_str().unwrap());

    let cleared = patch(&app, &owner, &path, 1, json!({"floor": null, "number": "101A"})).await;
    let stale = patch(&app, &owner, &path, 1, json!({"active": false})).await;
    let missing = app.send(Method::PATCH, &path, Some(&owner), Some(json!({"active": false}))).await;

    assert_eq!(cleared.status, StatusCode::OK, "{:?}", cleared.body);
    assert_eq!((cleared.body["floor"].clone(), cleared.body["number"].clone()), (Value::Null, json!("101A")));
    assert_eq!(stale.status, StatusCode::PRECONDITION_FAILED, "{:?}", stale.body);
    assert_eq!(missing.status, StatusCode::PRECONDITION_REQUIRED, "{:?}", missing.body);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn only_owners_and_managers_manage_rooms(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let superuser = PgPool::connect_with(opts).await.unwrap();
    let (owner, property) = hotel(&app).await;
    let manager = app.staff(&superuser, &owner, "manager@example.com", "manager").await;
    let front_desk = app.staff(&superuser, &owner, "desk@example.com", "front_desk").await;
    let housekeeping = app.staff(&superuser, &owner, "hk@example.com", "housekeeping").await;

    let by_manager = post(&app, &manager, &format!("{property}/room-types"), deluxe()).await;
    let by_desk = post(&app, &front_desk, &format!("{property}/sections"), json!({"name": "East"})).await;
    let by_housekeeping = post(&app, &housekeeping, &format!("{property}/room-types"), deluxe()).await;

    assert_eq!(by_manager.status, StatusCode::CREATED, "{:?}", by_manager.body);
    assert_eq!(by_desk.status, StatusCode::FORBIDDEN);
    assert_eq!(by_housekeeping.status, StatusCode::FORBIDDEN);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_new_room_tells_screens_to_refetch_rooms_and_inventory(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    spawn_listener(PgPool::connect_with(opts).await.unwrap(), app.state.events.clone()).await.unwrap();
    let (owner, property) = hotel(&app).await;
    let room_type = post(&app, &owner, &format!("{property}/room-types"), deluxe()).await.body["id"].clone();
    let mut events = app.state.events.subscribe();

    post(&app, &owner, &format!("{property}/rooms"), json!({"room_type_id": room_type, "number": "101"})).await;

    let property_id = uuid(&json!(property.trim_start_matches("/api/v1/properties/")));
    let LiveEvent::Invalidate(event) =
        tokio::time::timeout(Duration::from_secs(5), events.recv()).await.unwrap().unwrap()
    else {
        panic!("expected an invalidation")
    };
    assert_eq!(event.property_id, Some(property_id));
    assert_eq!(event.keys[0], format!("rooms:{property_id}"));
    // Physical counts change from the business date for the whole 730-day window: 25 or 26 months.
    let months = event.keys.iter().filter(|key| key.starts_with(&format!("inventory:{property_id}:"))).count();
    assert!((25..=26).contains(&months), "{:?}", event.keys);
}
```

Modify `crates/core-api/tests/openapi.rs`:

```diff
--- a/crates/core-api/tests/openapi.rs
+++ b/crates/core-api/tests/openapi.rs
@@ -1,4 +1,5 @@
 use core_api::openapi::ApiDoc;
+use std::collections::BTreeSet;
 use utoipa::OpenApi;
 
 #[test]
@@ -15,7 +16,33 @@ fn the_openapi_document_lists_every_rest_route() {
             "/api/v1/me",
             "/api/v1/properties",
             "/api/v1/properties/{property}",
+            "/api/v1/properties/{property}/room-types",
+            "/api/v1/properties/{property}/room-types/order",
+            "/api/v1/properties/{property}/room-types/{room_type}",
+            "/api/v1/properties/{property}/rooms",
+            "/api/v1/properties/{property}/rooms/bulk",
+            "/api/v1/properties/{property}/rooms/order",
+            "/api/v1/properties/{property}/rooms/{room}",
+            "/api/v1/properties/{property}/sections",
+            "/api/v1/properties/{property}/sections/{section}",
             "/api/v1/session/tenant",
         ]
     );
 }
+
+/// Generated TypeScript names operations by id, so a repeated id would silently merge two operations.
+#[test]
+fn every_operation_has_its_own_id() {
+    let doc = ApiDoc::openapi();
+    let ids: Vec<String> = doc
+        .paths
+        .paths
+        .values()
+        .flat_map(|item| [&item.get, &item.put, &item.post, &item.delete, &item.patch])
+        .flatten()
+        .map(|operation| operation.operation_id.clone().expect("every operation has an id"))
+        .collect();
+
+    let unique: BTreeSet<&String> = ids.iter().collect();
+    assert_eq!(unique.len(), ids.len(), "repeated operation ids in {ids:?}");
+}
```

- [ ] **Step 2: Run the tests to verify they fail**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p core-api --no-fail-fast --test rooms --test openapi
```

Expected: FAIL: all 6 `rooms` tests (the routes answer the 404 problem) and `the_openapi_document_lists_every_rest_route`.

- [ ] **Step 3: Implement**

Modify `crates/core-api/Cargo.toml`:

```diff
--- a/crates/core-api/Cargo.toml
+++ b/crates/core-api/Cargo.toml
@@ -18,6 +18,7 @@ garde.workspace = true
 identity.workspace = true
 mimalloc.workspace = true
 property.workspace = true
+rooms.workspace = true
 serde.workspace = true
 serde_json.workspace = true
 sha2.workspace = true
```

Create `crates/core-api/src/routes/room_types.rs`:

```rust
use crate::auth::TenantContext;
use crate::concurrency::{IfMatch, Versioned};
use crate::error::{ApiError, validate};
use crate::extract::{ApiJson, ApiPath};
use crate::routes::rooms::{ReorderRequest, rooms_error};
use crate::state::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use db::Scope;
use garde::Validate;
use identity::Permission;
use rooms::{Bed, NewRoomType, RoomType, RoomTypeChanges};
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct BedRequest {
    /// e.g. `king`, `queen`, `twin`, `sofa bed`.
    #[garde(length(chars, min = 1, max = 40))]
    pub kind: String,
    #[garde(range(min = 1, max = 10))]
    pub count: i32,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreateRoomTypeRequest {
    /// 1 to 10 capital letters or digits, unique within the property. Cannot be changed later.
    #[garde(pattern(r"^[A-Z0-9]{1,10}$"))]
    pub code: String,
    #[garde(length(chars, min = 1, max = 100))]
    pub name: String,
    #[garde(range(min = 1, max = 50))]
    pub base_occupancy: i32,
    #[garde(range(min = 1, max = 50))]
    pub max_adults: i32,
    #[garde(range(min = 0, max = 50))]
    pub max_children: i32,
    #[garde(range(min = 1, max = 50))]
    pub max_occupancy: i32,
    #[serde(default)]
    #[garde(length(max = 10), dive)]
    pub bed_config: Vec<BedRequest>,
    #[serde(default)]
    #[garde(length(max = 50), inner(length(chars, min = 1, max = 60)))]
    pub amenities: Vec<String>,
}

/// Fields left out stay as they are.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdateRoomTypeRequest {
    #[garde(inner(length(chars, min = 1, max = 100)))]
    pub name: Option<String>,
    #[garde(inner(range(min = 1, max = 50)))]
    pub base_occupancy: Option<i32>,
    #[garde(inner(range(min = 1, max = 50)))]
    pub max_adults: Option<i32>,
    #[garde(inner(range(min = 0, max = 50)))]
    pub max_children: Option<i32>,
    #[garde(inner(range(min = 1, max = 50)))]
    pub max_occupancy: Option<i32>,
    #[garde(length(max = 10), dive)]
    pub bed_config: Option<Vec<BedRequest>>,
    #[garde(inner(length(max = 50), inner(length(chars, min = 1, max = 60))))]
    pub amenities: Option<Vec<String>>,
    /// `false` retires the type; it must have no active rooms.
    #[garde(skip)]
    pub active: Option<bool>,
}

fn beds(requests: Vec<BedRequest>) -> Vec<Bed> {
    requests.into_iter().map(|bed| Bed { kind: bed.kind, count: bed.count }).collect()
}

#[utoipa::path(post, operation_id = "create_room_type", path = "/api/v1/properties/{property}/room-types", request_body = CreateRoomTypeRequest,
    params(("property" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 201, body = RoomType), (status = 403), (status = 404), (status = 409), (status = 422)))]
pub async fn create(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
    ApiJson(body): ApiJson<CreateRoomTypeRequest>,
) -> Result<Versioned<RoomType>, ApiError> {
    ctx.require(Permission::RoomsManage, Some(property))?;
    validate(&body)?;
    let input = NewRoomType {
        code: body.code,
        name: body.name,
        base_occupancy: body.base_occupancy,
        max_adults: body.max_adults,
        max_children: body.max_children,
        max_occupancy: body.max_occupancy,
        bed_config: beds(body.bed_config),
        amenities: body.amenities,
    };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let created = rooms::create_room_type(&mut tx, ctx.tenant, ctx.user, property, input).await.map_err(rooms_error)?;
    tx.commit().await?;
    Ok(Versioned::created(created.version, created))
}

#[utoipa::path(patch, operation_id = "update_room_type", path = "/api/v1/properties/{property}/room-types/{room_type}", request_body = UpdateRoomTypeRequest,
    params(("property" = Uuid, Path), ("room_type" = Uuid, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = RoomType), (status = 403), (status = 404), (status = 409), (status = 412), (status = 422), (status = 428)))]
pub async fn update(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, room_type)): ApiPath<(Uuid, Uuid)>,
    IfMatch(version): IfMatch,
    ApiJson(body): ApiJson<UpdateRoomTypeRequest>,
) -> Result<Versioned<RoomType>, ApiError> {
    ctx.require(Permission::RoomsManage, Some(property))?;
    validate(&body)?;
    let changes = RoomTypeChanges {
        name: body.name,
        base_occupancy: body.base_occupancy,
        max_adults: body.max_adults,
        max_children: body.max_children,
        max_occupancy: body.max_occupancy,
        bed_config: body.bed_config.map(beds),
        amenities: body.amenities,
        active: body.active,
    };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let updated = rooms::update_room_type(&mut tx, ctx.tenant, ctx.user, property, room_type, version, changes)
        .await
        .map_err(rooms_error)?;
    tx.commit().await?;
    Ok(Versioned::ok(updated.version, updated))
}

#[utoipa::path(put, operation_id = "reorder_room_types", path = "/api/v1/properties/{property}/room-types/order", request_body = ReorderRequest,
    params(("property" = Uuid, Path)),
    responses((status = 204), (status = 403), (status = 404), (status = 422)))]
pub async fn reorder(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
    ApiJson(body): ApiJson<ReorderRequest>,
) -> Result<StatusCode, ApiError> {
    ctx.require(Permission::RoomsManage, Some(property))?;
    validate(&body)?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    rooms::reorder_room_types(&mut tx, ctx.tenant, property, &body.ids).await.map_err(rooms_error)?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
```

Create `crates/core-api/src/routes/rooms.rs`:

```rust
use crate::auth::TenantContext;
use crate::concurrency::{IfMatch, Versioned};
use crate::error::{ApiError, validate};
use crate::extract::{ApiJson, ApiPath};
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use db::Scope;
use garde::Validate;
use identity::Permission;
use rooms::{NewRoom, Room, RoomChanges, RoomRange, RoomsError, Section};
use serde::{Deserialize, Deserializer};
use utoipa::ToSchema;
use uuid::Uuid;

/// Maps the rooms module's errors to problem details.
pub(crate) fn rooms_error(err: RoomsError) -> ApiError {
    match err {
        RoomsError::NotFound(_) => ApiError::not_found(err.to_string()),
        RoomsError::VersionMismatch(_) => ApiError::precondition_failed(err.to_string()),
        RoomsError::Conflict(message) => ApiError::conflict(message),
        RoomsError::Invalid(message) => ApiError::unprocessable(message),
        RoomsError::Overlap(_) => ApiError::conflict(err.to_string()),
        RoomsError::Database(db_err) => db_err.into(),
    }
}

/// Tells a field sent as `null` (`Some(None)`: clear it) from one left out (`None`: keep it).
fn present<'de, T: Deserialize<'de>, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct ReorderRequest {
    /// Every id of the collection, in the new display order.
    #[garde(length(min = 1, max = 2000))]
    pub ids: Vec<Uuid>,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreateRoomRequest {
    #[garde(skip)]
    pub room_type_id: Uuid,
    /// Letters, digits and `-`, up to 10 characters, unique within the property.
    #[garde(pattern(r"^[A-Za-z0-9-]{1,10}$"))]
    pub number: String,
    #[garde(inner(length(chars, min = 1, max = 20)))]
    pub floor: Option<String>,
    #[garde(skip)]
    pub section_id: Option<Uuid>,
}

/// Rooms `{prefix}{first}` to `{prefix}{last}`: `{"first": 101, "last": 120}` adds 101 to 120.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreateRoomRangeRequest {
    #[garde(skip)]
    pub room_type_id: Uuid,
    #[serde(default)]
    #[garde(pattern(r"^[A-Za-z0-9-]{0,5}$"))]
    pub prefix: String,
    #[garde(range(max = 99999))]
    pub first: u32,
    #[garde(range(max = 99999))]
    pub last: u32,
    #[garde(inner(length(chars, min = 1, max = 20)))]
    pub floor: Option<String>,
    #[garde(skip)]
    pub section_id: Option<Uuid>,
}

/// Fields left out stay as they are; `floor` and `section_id` sent as `null` are cleared.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdateRoomRequest {
    #[garde(skip)]
    pub room_type_id: Option<Uuid>,
    #[garde(inner(pattern(r"^[A-Za-z0-9-]{1,10}$")))]
    pub number: Option<String>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<String>, nullable)]
    #[garde(inner(inner(length(chars, min = 1, max = 20))))]
    pub floor: Option<Option<String>>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<Uuid>, nullable)]
    #[garde(skip)]
    pub section_id: Option<Option<Uuid>>,
    #[garde(skip)]
    pub active: Option<bool>,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct SectionRequest {
    #[garde(length(chars, min = 1, max = 100))]
    pub name: String,
}

#[utoipa::path(post, operation_id = "create_room", path = "/api/v1/properties/{property}/rooms", request_body = CreateRoomRequest,
    params(("property" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 201, body = Room), (status = 403), (status = 404), (status = 409), (status = 422)))]
pub async fn create(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
    ApiJson(body): ApiJson<CreateRoomRequest>,
) -> Result<Versioned<Room>, ApiError> {
    ctx.require(Permission::RoomsManage, Some(property))?;
    validate(&body)?;
    let input = NewRoom {
        room_type_id: body.room_type_id,
        number: body.number,
        floor: body.floor,
        section_id: body.section_id,
    };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let created = rooms::create_room(&mut tx, ctx.tenant, ctx.user, property, input).await.map_err(rooms_error)?;
    tx.commit().await?;
    Ok(Versioned::created(created.version, created))
}

#[utoipa::path(post, operation_id = "create_rooms", path = "/api/v1/properties/{property}/rooms/bulk", request_body = CreateRoomRangeRequest,
    params(("property" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 201, body = Vec<Room>), (status = 403), (status = 404), (status = 409), (status = 422)))]
pub async fn create_range(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
    ApiJson(body): ApiJson<CreateRoomRangeRequest>,
) -> Result<(StatusCode, Json<Vec<Room>>), ApiError> {
    ctx.require(Permission::RoomsManage, Some(property))?;
    validate(&body)?;
    let range = RoomRange {
        room_type_id: body.room_type_id,
        prefix: body.prefix,
        first: body.first,
        last: body.last,
        floor: body.floor,
        section_id: body.section_id,
    };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let created = rooms::create_rooms(&mut tx, ctx.tenant, ctx.user, property, range).await.map_err(rooms_error)?;
    tx.commit().await?;
    Ok((StatusCode::CREATED, Json(created)))
}

#[utoipa::path(patch, operation_id = "update_room", path = "/api/v1/properties/{property}/rooms/{room}", request_body = UpdateRoomRequest,
    params(("property" = Uuid, Path), ("room" = Uuid, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = Room), (status = 403), (status = 404), (status = 409), (status = 412), (status = 422), (status = 428)))]
pub async fn update(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, room)): ApiPath<(Uuid, Uuid)>,
    IfMatch(version): IfMatch,
    ApiJson(body): ApiJson<UpdateRoomRequest>,
) -> Result<Versioned<Room>, ApiError> {
    ctx.require(Permission::RoomsManage, Some(property))?;
    validate(&body)?;
    let changes = RoomChanges {
        room_type_id: body.room_type_id,
        number: body.number,
        floor: body.floor,
        section_id: body.section_id,
        active: body.active,
    };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let updated = rooms::update_room(&mut tx, ctx.tenant, ctx.user, property, room, version, changes)
        .await
        .map_err(rooms_error)?;
    tx.commit().await?;
    Ok(Versioned::ok(updated.version, updated))
}

#[utoipa::path(put, operation_id = "reorder_rooms", path = "/api/v1/properties/{property}/rooms/order", request_body = ReorderRequest,
    params(("property" = Uuid, Path)),
    responses((status = 204), (status = 403), (status = 404), (status = 422)))]
pub async fn reorder(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
    ApiJson(body): ApiJson<ReorderRequest>,
) -> Result<StatusCode, ApiError> {
    ctx.require(Permission::RoomsManage, Some(property))?;
    validate(&body)?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    rooms::reorder_rooms(&mut tx, ctx.tenant, property, &body.ids).await.map_err(rooms_error)?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, operation_id = "create_section", path = "/api/v1/properties/{property}/sections", request_body = SectionRequest,
    params(("property" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 201, body = Section), (status = 403), (status = 404), (status = 409), (status = 422)))]
pub async fn create_section(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
    ApiJson(body): ApiJson<SectionRequest>,
) -> Result<Versioned<Section>, ApiError> {
    ctx.require(Permission::RoomsManage, Some(property))?;
    validate(&body)?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let created =
        rooms::create_section(&mut tx, ctx.tenant, ctx.user, property, &body.name).await.map_err(rooms_error)?;
    tx.commit().await?;
    Ok(Versioned::created(created.version, created))
}

#[utoipa::path(patch, operation_id = "rename_section", path = "/api/v1/properties/{property}/sections/{section}", request_body = SectionRequest,
    params(("property" = Uuid, Path), ("section" = Uuid, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = Section), (status = 403), (status = 404), (status = 409), (status = 412), (status = 422), (status = 428)))]
pub async fn rename_section(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, section)): ApiPath<(Uuid, Uuid)>,
    IfMatch(version): IfMatch,
    ApiJson(body): ApiJson<SectionRequest>,
) -> Result<Versioned<Section>, ApiError> {
    ctx.require(Permission::RoomsManage, Some(property))?;
    validate(&body)?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let renamed = rooms::rename_section(&mut tx, ctx.tenant, ctx.user, property, section, version, &body.name)
        .await
        .map_err(rooms_error)?;
    tx.commit().await?;
    Ok(Versioned::ok(renamed.version, renamed))
}
```

Modify `crates/core-api/src/routes/mod.rs`:

```diff
--- a/crates/core-api/src/routes/mod.rs
+++ b/crates/core-api/src/routes/mod.rs
@@ -1,6 +1,8 @@
 pub(crate) mod auth;
 mod health;
 pub(crate) mod properties;
+pub(crate) mod room_types;
+pub(crate) mod rooms;
 
 use crate::error::ApiError;
 use crate::state::AppState;
@@ -17,6 +19,8 @@ use tower_http::trace::TraceLayer;
 
 pub use auth::{LoginRequest, SignupRequest, SwitchTenantRequest};
 pub use properties::{CreatePropertyRequest, UpdatePropertyRequest};
+pub use room_types::{BedRequest, CreateRoomTypeRequest, UpdateRoomTypeRequest};
+pub use rooms::{CreateRoomRangeRequest, CreateRoomRequest, ReorderRequest, SectionRequest, UpdateRoomRequest};
 
 /// Longest a request may run before it is answered with 504.
 pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
@@ -25,8 +29,13 @@ pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
 pub const ABANDONED_CLAIM_AFTER: Duration = Duration::from_secs(60);
 
 pub fn router(state: AppState) -> Router {
+    const PROPERTY: &str = "/api/v1/properties/{property}";
     let commands = Router::new()
         .route("/api/v1/properties", post(properties::create))
+        .route(&format!("{PROPERTY}/room-types"), post(room_types::create))
+        .route(&format!("{PROPERTY}/rooms"), post(rooms::create))
+        .route(&format!("{PROPERTY}/rooms/bulk"), post(rooms::create_range))
+        .route(&format!("{PROPERTY}/sections"), post(rooms::create_section))
         .route_layer(from_fn_with_state(state.clone(), idempotency::idempotent));
 
     let requests = Router::new()
@@ -35,7 +44,12 @@ pub fn router(state: AppState) -> Router {
         .route("/api/v1/auth/logout", post(auth::logout))
         .route("/api/v1/me", get(auth::me))
         .route("/api/v1/session/tenant", put(auth::switch_tenant))
-        .route("/api/v1/properties/{property}", patch(properties::update))
+        .route(PROPERTY, patch(properties::update))
+        .route(&format!("{PROPERTY}/room-types/order"), put(room_types::reorder))
+        .route(&format!("{PROPERTY}/room-types/{{room_type}}"), patch(room_types::update))
+        .route(&format!("{PROPERTY}/rooms/order"), put(rooms::reorder))
+        .route(&format!("{PROPERTY}/rooms/{{room}}"), patch(rooms::update))
+        .route(&format!("{PROPERTY}/sections/{{section}}"), patch(rooms::rename_section))
         .route("/graphql", post(graphql::handler))
         .merge(commands)
         .layer(from_fn(|request, next| deadline(REQUEST_TIMEOUT, request, next)));
```

Modify `crates/core-api/src/routes/properties.rs`:

```diff
--- a/crates/core-api/src/routes/properties.rs
+++ b/crates/core-api/src/routes/properties.rs
@@ -49,7 +49,7 @@ fn property_error(err: PropertyError) -> ApiError {
     }
 }
 
-#[utoipa::path(post, path = "/api/v1/properties", request_body = CreatePropertyRequest,
+#[utoipa::path(post, operation_id = "create_property", path = "/api/v1/properties", request_body = CreatePropertyRequest,
     params(("Idempotency-Key" = String, Header)),
     responses((status = 201, body = Property), (status = 403), (status = 409), (status = 422)))]
 pub async fn create(
@@ -68,7 +68,7 @@ pub async fn create(
 }
 
 /// Changes a property's settings. The business date is not editable: night audit moves it.
-#[utoipa::path(patch, path = "/api/v1/properties/{property}", request_body = UpdatePropertyRequest,
+#[utoipa::path(patch, operation_id = "update_property", path = "/api/v1/properties/{property}", request_body = UpdatePropertyRequest,
     params(("property" = Uuid, Path), ("If-Match" = String, Header, description = "the version edited, e.g. \"3\"")),
     responses((status = 200, body = Property), (status = 403), (status = 404), (status = 412), (status = 422), (status = 428)))]
 pub async fn update(
```

Modify `crates/core-api/src/openapi.rs`:

```diff
--- a/crates/core-api/src/openapi.rs
+++ b/crates/core-api/src/openapi.rs
@@ -1,4 +1,8 @@
-use crate::routes::{CreatePropertyRequest, LoginRequest, SignupRequest, SwitchTenantRequest, UpdatePropertyRequest};
+use crate::routes::{
+    BedRequest, CreatePropertyRequest, CreateRoomRangeRequest, CreateRoomRequest, CreateRoomTypeRequest, LoginRequest,
+    ReorderRequest, SectionRequest, SignupRequest, SwitchTenantRequest, UpdatePropertyRequest, UpdateRoomRequest,
+    UpdateRoomTypeRequest,
+};
 use utoipa::OpenApi;
 
 #[derive(OpenApi)]
@@ -12,6 +16,15 @@ use utoipa::OpenApi;
         crate::routes::auth::switch_tenant,
         crate::routes::properties::create,
         crate::routes::properties::update,
+        crate::routes::room_types::create,
+        crate::routes::room_types::update,
+        crate::routes::room_types::reorder,
+        crate::routes::rooms::create,
+        crate::routes::rooms::create_range,
+        crate::routes::rooms::update,
+        crate::routes::rooms::reorder,
+        crate::routes::rooms::create_section,
+        crate::routes::rooms::rename_section,
     ),
     components(schemas(
         SignupRequest,
@@ -19,11 +32,23 @@ use utoipa::OpenApi;
         SwitchTenantRequest,
         CreatePropertyRequest,
         UpdatePropertyRequest,
+        BedRequest,
+        CreateRoomTypeRequest,
+        UpdateRoomTypeRequest,
+        ReorderRequest,
+        CreateRoomRequest,
+        CreateRoomRangeRequest,
+        UpdateRoomRequest,
+        SectionRequest,
         identity::Profile,
         identity::TenantSummary,
         identity::Grant,
         identity::Role,
         property::Property,
+        rooms::Bed,
+        rooms::RoomType,
+        rooms::Room,
+        rooms::Section,
     ))
 )]
 pub struct ApiDoc;
```

- [ ] **Step 4: Regenerate the API types**

```sh
cd web/pms && bun run api:schemas && bun run codegen
```

- [ ] **Step 5: Run the tests to verify they pass**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cd web/pms
bun run lint && bun run check && bun run test && bun run build
```

Expected: PASS (`rooms`: 6, `openapi`: 2). Web checks clean.

- [ ] **Step 6: Commit**

```sh
git add -A
git commit -m "feat(api): room type, room and section commands"
```

### Task 11: REST commands for block reasons and room blocks

Blocks over HTTP: `POST …/rooms/{room}/blocks` (needs `InventoryBlock`, idempotent) and `PATCH …/blocks/{block}` with `{"to": "YYYY-MM-DD"}` (release or shorten, `If-Match`). An overlap is a 409 whose problem carries `conflicts`, the blocks in the way, through a new `ApiError::with(name, value)` for extension members. Block reasons are managed with `RoomsManage`. Creating a property now seeds its five default block reasons in the same transaction.

**Files:**
- Create: `crates/core-api/src/routes/blocks.rs`
- Modify: `crates/core-api/src/error.rs`
- Modify: `crates/core-api/src/openapi.rs`
- Modify: `crates/core-api/src/routes/mod.rs`
- Modify: `crates/core-api/src/routes/properties.rs`
- Modify: `crates/core-api/src/routes/rooms.rs`
- Modify: `docs/design/api-conventions.md`
- Test: `crates/core-api/tests/blocks.rs` (new)
- Test: `crates/core-api/tests/openapi.rs`
- Regenerated, not shown: `web/pms/src/lib/api/openapi.d.ts`, `web/pms/src/lib/api/openapi.json`

**Interfaces:**
- Consumes: `rooms::{create_block, shorten_block, create_block_reason, update_block_reason, seed_block_reasons, RoomsError::Overlap}`.
- Produces: `ApiError::with(name: &str, value: impl Serialize) -> ApiError`; `POST …/block-reasons` (`create_block_reason`), `PATCH …/block-reasons/{reason}` (`update_block_reason`), `POST …/rooms/{room}/blocks` (`create_block`), `PATCH …/blocks/{block}` (`shorten_block`); 409 body `{ …, "conflicts": [Block] }`.

- [ ] **Step 1: Write the failing tests**

Create `crates/core-api/tests/blocks.rs`:

```rust
mod common;

use axum::http::{Method, StatusCode, header};
use common::{TestApp, TestResponse};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use time::{Date, Duration, format_description::well_known::Iso8601};
use uuid::Uuid;

async fn post(app: &TestApp, cookie: &str, path: &str, body: Value) -> TestResponse {
    let key = Uuid::now_v7().to_string();
    app.send_with(Method::POST, path, Some(cookie), Some(body), &[("x-goodfolk-csrf", "1"), ("idempotency-key", &key)])
        .await
}

async fn patch(app: &TestApp, cookie: &str, path: &str, version: i64, body: Value) -> TestResponse {
    let if_match = format!("\"{version}\"");
    app.send_with(Method::PATCH, path, Some(cookie), Some(body), &[("x-goodfolk-csrf", "1"), ("if-match", &if_match)])
        .await
}

/// A property with one room type and rooms 101 and 102, set up by its owner.
struct Hotel {
    owner: String,
    superuser: PgPool,
    path: String,
    business_date: Date,
    room_type: Uuid,
    rooms: Vec<Uuid>,
}

impl Hotel {
    async fn new(app: &TestApp, opts: PgConnectOptions) -> Self {
        let superuser = PgPool::connect_with(opts).await.unwrap();
        let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
        let property = json!({"code": "GAL", "name": "Galle", "timezone": "Asia/Colombo", "base_currency": "LKR"});
        let property = post(app, &owner, "/api/v1/properties", property).await.body;
        let path = format!("/api/v1/properties/{}", property["id"].as_str().unwrap());
        let business_date = Date::parse(property["business_date"].as_str().unwrap(), &Iso8601::DEFAULT).unwrap();
        let room_type = json!({"code": "DLX", "name": "Deluxe", "base_occupancy": 2, "max_adults": 2,
                               "max_children": 0, "max_occupancy": 2});
        let room_type = post(app, &owner, &format!("{path}/room-types"), room_type).await.body["id"].clone();
        let range = json!({"room_type_id": room_type, "first": 101, "last": 102});
        let rooms = post(app, &owner, &format!("{path}/rooms/bulk"), range).await.body;
        let rooms = rooms.as_array().unwrap().iter().map(|room| common::uuid(&room["id"])).collect();
        Self { owner, superuser, path, business_date, room_type: common::uuid(&room_type), rooms }
    }

    fn day(&self, offset: i64) -> String {
        (self.business_date + Duration::days(offset)).to_string()
    }

    async fn reason(&self, code: &str) -> Uuid {
        sqlx::query_scalar("select id from block_reason where code = $1")
            .bind(code)
            .fetch_one(&self.superuser)
            .await
            .unwrap()
    }

    async fn block(&self, app: &TestApp, cookie: &str, room: usize, from: i64, to: i64) -> TestResponse {
        let body = json!({"from": self.day(from), "to": self.day(to), "kind": "out_of_order",
                          "reason_id": self.reason("MAINTENANCE").await, "note": "Leaking pipe"});
        post(app, cookie, &format!("{}/rooms/{}/blocks", self.path, self.rooms[room]), body).await
    }

    /// Rooms out of order on the business date plus `offset`.
    async fn out_of_order(&self, offset: i64) -> i32 {
        sqlx::query_scalar("select out_of_order from inventory_day where room_type_id = $1 and date = $2")
            .bind(self.room_type)
            .bind(self.business_date + Duration::days(offset))
            .fetch_one(&self.superuser)
            .await
            .unwrap()
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn front_desk_blocks_a_room_and_an_overlap_is_a_409_naming_the_block(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let front_desk = app.staff(&hotel.superuser, &hotel.owner, "desk@example.com", "front_desk").await;

    let created = hotel.block(&app, &front_desk, 0, 1, 4).await;
    let overlapping = hotel.block(&app, &front_desk, 0, 3, 5).await;
    let other_room = hotel.block(&app, &front_desk, 1, 3, 5).await;

    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    assert_eq!(created.headers[header::ETAG], "\"1\"");
    assert_eq!(
        (created.body["from"].as_str(), created.body["to"].as_str()),
        (Some(hotel.day(1).as_str()), Some(hotel.day(4).as_str()))
    );
    assert_eq!(overlapping.status, StatusCode::CONFLICT, "{:?}", overlapping.body);
    assert_eq!(overlapping.headers[header::CONTENT_TYPE], "application/problem+json");
    assert_eq!(overlapping.body["conflicts"][0]["id"], created.body["id"]);
    assert_eq!(overlapping.body["conflicts"].as_array().unwrap().len(), 1);
    assert_eq!(other_room.status, StatusCode::CREATED, "{:?}", other_room.body);
    assert_eq!((hotel.out_of_order(0).await, hotel.out_of_order(3).await, hotel.out_of_order(4).await), (0, 2, 1));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_block_cannot_start_before_the_business_date(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;

    let past = hotel.block(&app, &hotel.owner, 0, -1, 2).await;
    let backwards = hotel.block(&app, &hotel.owner, 0, 3, 1).await;

    assert_eq!(past.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", past.body);
    assert_eq!(backwards.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", backwards.body);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn releasing_a_block_early_restores_availability(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let block = hotel.block(&app, &hotel.owner, 0, 0, 5).await;
    let path = format!("{}/blocks/{}", hotel.path, block.body["id"].as_str().unwrap());

    let released = patch(&app, &hotel.owner, &path, 1, json!({"to": hotel.day(2)})).await;
    let stale = patch(&app, &hotel.owner, &path, 1, json!({"to": hotel.day(1)})).await;
    let in_the_past = patch(&app, &hotel.owner, &path, 2, json!({"to": hotel.day(-1)})).await;

    assert_eq!(released.status, StatusCode::OK, "{:?}", released.body);
    assert_eq!(released.headers[header::ETAG], "\"2\"");
    assert_eq!(released.body["to"].as_str(), Some(hotel.day(2).as_str()));
    assert_eq!((hotel.out_of_order(1).await, hotel.out_of_order(2).await), (1, 0));
    assert_eq!(stale.status, StatusCode::PRECONDITION_FAILED, "{:?}", stale.body);
    assert_eq!(in_the_past.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", in_the_past.body);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn blocking_and_reasons_follow_the_role_permissions(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let manager = app.staff(&hotel.superuser, &hotel.owner, "manager@example.com", "manager").await;
    let front_desk = app.staff(&hotel.superuser, &hotel.owner, "desk@example.com", "front_desk").await;
    let housekeeping = app.staff(&hotel.superuser, &hotel.owner, "hk@example.com", "housekeeping").await;
    let pest = json!({"code": "PEST", "label": "Pest control", "default_kind": "out_of_order"});

    let by_housekeeping = hotel.block(&app, &housekeeping, 0, 0, 1).await;
    let reason_by_desk = post(&app, &front_desk, &format!("{}/block-reasons", hotel.path), pest.clone()).await;
    let reason_by_manager = post(&app, &manager, &format!("{}/block-reasons", hotel.path), pest).await;
    let reason_path = format!("{}/block-reasons/{}", hotel.path, reason_by_manager.body["id"].as_str().unwrap());
    let retired = patch(&app, &manager, &reason_path, 1, json!({"active": false})).await;

    assert_eq!(by_housekeeping.status, StatusCode::FORBIDDEN);
    assert_eq!(reason_by_desk.status, StatusCode::FORBIDDEN);
    assert_eq!(reason_by_manager.status, StatusCode::CREATED, "{:?}", reason_by_manager.body);
    assert_eq!(retired.status, StatusCode::OK, "{:?}", retired.body);
    assert_eq!(retired.body["active"], false);
}
```

Modify `crates/core-api/tests/openapi.rs`:

```diff
--- a/crates/core-api/tests/openapi.rs
+++ b/crates/core-api/tests/openapi.rs
@@ -16,6 +16,9 @@ fn the_openapi_document_lists_every_rest_route() {
             "/api/v1/me",
             "/api/v1/properties",
             "/api/v1/properties/{property}",
+            "/api/v1/properties/{property}/block-reasons",
+            "/api/v1/properties/{property}/block-reasons/{reason}",
+            "/api/v1/properties/{property}/blocks/{block}",
             "/api/v1/properties/{property}/room-types",
             "/api/v1/properties/{property}/room-types/order",
             "/api/v1/properties/{property}/room-types/{room_type}",
@@ -23,6 +26,7 @@ fn the_openapi_document_lists_every_rest_route() {
             "/api/v1/properties/{property}/rooms/bulk",
             "/api/v1/properties/{property}/rooms/order",
             "/api/v1/properties/{property}/rooms/{room}",
+            "/api/v1/properties/{property}/rooms/{room}/blocks",
             "/api/v1/properties/{property}/sections",
             "/api/v1/properties/{property}/sections/{section}",
             "/api/v1/session/tenant",
```

- [ ] **Step 2: Run the tests to verify they fail**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p core-api --no-fail-fast --test blocks --test openapi
```

Expected: FAIL: all 4 `blocks` tests (no block reasons are seeded, and the routes are 404s) and the OpenAPI route list.

- [ ] **Step 3: Implement**

Modify `crates/core-api/src/error.rs`:

```diff
--- a/crates/core-api/src/error.rs
+++ b/crates/core-api/src/error.rs
@@ -9,6 +9,8 @@ pub struct ApiError {
     status: StatusCode,
     title: &'static str,
     detail: Option<String>,
+    /// Extension members, such as the blocks in the way of a new one.
+    extensions: serde_json::Map<String, serde_json::Value>,
 }
 
 #[derive(Serialize)]
@@ -19,11 +21,20 @@ struct Problem<'a> {
     status: u16,
     #[serde(skip_serializing_if = "Option::is_none")]
     detail: Option<&'a str>,
+    #[serde(flatten)]
+    extensions: &'a serde_json::Map<String, serde_json::Value>,
 }
 
 impl ApiError {
     fn new(status: StatusCode, title: &'static str, detail: Option<String>) -> Self {
-        Self { status, title, detail }
+        Self { status, title, detail, extensions: serde_json::Map::new() }
+    }
+
+    /// Adds an extension member to the problem, e.g. `conflicts: [...]`.
+    pub fn with(mut self, name: &str, value: impl Serialize) -> Self {
+        let value = serde_json::to_value(value).expect("extension members serialize");
+        self.extensions.insert(name.to_owned(), value);
+        self
     }
 
     pub fn bad_request(detail: impl Into<String>) -> Self {
@@ -101,6 +112,7 @@ impl IntoResponse for ApiError {
             title: self.title,
             status: self.status.as_u16(),
             detail: self.detail.as_deref(),
+            extensions: &self.extensions,
         };
         let mut response = (self.status, Json(body)).into_response();
         response
```

Create `crates/core-api/src/routes/blocks.rs`:

```rust
use crate::auth::TenantContext;
use crate::concurrency::{IfMatch, Versioned};
use crate::error::{ApiError, validate};
use crate::extract::{ApiJson, ApiPath};
use crate::routes::rooms::rooms_error;
use crate::state::AppState;
use axum::extract::State;
use db::Scope;
use garde::Validate;
use identity::Permission;
use rooms::{Block, BlockKind, BlockReason, BlockReasonChanges, NewBlock, NewBlockReason};
use serde::Deserialize;
use time::Date;
use utoipa::ToSchema;
use uuid::Uuid;

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreateBlockReasonRequest {
    /// Capital letters, digits and `_`, unique within the property. Cannot be changed later.
    #[garde(pattern(r"^[A-Z0-9_]{1,20}$"))]
    pub code: String,
    #[garde(length(chars, min = 1, max = 100))]
    pub label: String,
    /// The kind the block dialog suggests for this reason.
    #[garde(skip)]
    pub default_kind: BlockKind,
}

/// Fields left out stay as they are.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdateBlockReasonRequest {
    #[garde(inner(length(chars, min = 1, max = 100)))]
    pub label: Option<String>,
    #[garde(skip)]
    pub default_kind: Option<BlockKind>,
    /// `false` retires the reason: existing blocks keep it, new blocks cannot use it.
    #[garde(skip)]
    pub active: Option<bool>,
}

/// Blocks the room for `[from, to)`: `to` is the first day it is back in service.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreateBlockRequest {
    /// `YYYY-MM-DD`, on or after the property's business date.
    #[garde(skip)]
    pub from: Date,
    /// `YYYY-MM-DD`, after `from`.
    #[garde(skip)]
    pub to: Date,
    #[garde(skip)]
    pub kind: BlockKind,
    #[garde(skip)]
    pub reason_id: Uuid,
    #[serde(default)]
    #[garde(length(chars, max = 500))]
    pub note: String,
}

/// Ends the block early: the room is back from `to`. Sending the business date releases it now; a date on
/// or before the block's start cancels it.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct ShortenBlockRequest {
    /// `YYYY-MM-DD`, on or after the business date and before the block's current end.
    #[garde(skip)]
    pub to: Date,
}

#[utoipa::path(post, operation_id = "create_block_reason", path = "/api/v1/properties/{property}/block-reasons", request_body = CreateBlockReasonRequest,
    params(("property" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 201, body = BlockReason), (status = 403), (status = 404), (status = 409), (status = 422)))]
pub async fn create_reason(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
    ApiJson(body): ApiJson<CreateBlockReasonRequest>,
) -> Result<Versioned<BlockReason>, ApiError> {
    ctx.require(Permission::RoomsManage, Some(property))?;
    validate(&body)?;
    let input = NewBlockReason { code: body.code, label: body.label, default_kind: body.default_kind };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let created =
        rooms::create_block_reason(&mut tx, ctx.tenant, ctx.user, property, input).await.map_err(rooms_error)?;
    tx.commit().await?;
    Ok(Versioned::created(created.version, created))
}

#[utoipa::path(patch, operation_id = "update_block_reason", path = "/api/v1/properties/{property}/block-reasons/{reason}",
    request_body = UpdateBlockReasonRequest,
    params(("property" = Uuid, Path), ("reason" = Uuid, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = BlockReason), (status = 403), (status = 404), (status = 412), (status = 422), (status = 428)))]
pub async fn update_reason(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, reason)): ApiPath<(Uuid, Uuid)>,
    IfMatch(version): IfMatch,
    ApiJson(body): ApiJson<UpdateBlockReasonRequest>,
) -> Result<Versioned<BlockReason>, ApiError> {
    ctx.require(Permission::RoomsManage, Some(property))?;
    validate(&body)?;
    let changes = BlockReasonChanges { label: body.label, default_kind: body.default_kind, active: body.active };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let updated = rooms::update_block_reason(&mut tx, ctx.tenant, ctx.user, property, reason, version, changes)
        .await
        .map_err(rooms_error)?;
    tx.commit().await?;
    Ok(Versioned::ok(updated.version, updated))
}

/// A 409 lists the blocks in the way as `conflicts` (each with `id`, `room_id`, `from`, `to`, `kind`).
#[utoipa::path(post, operation_id = "create_block", path = "/api/v1/properties/{property}/rooms/{room}/blocks", request_body = CreateBlockRequest,
    params(("property" = Uuid, Path), ("room" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 201, body = Block), (status = 403), (status = 404), (status = 409), (status = 422)))]
pub async fn create(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, room)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<CreateBlockRequest>,
) -> Result<Versioned<Block>, ApiError> {
    ctx.require(Permission::InventoryBlock, Some(property))?;
    validate(&body)?;
    let input = NewBlock {
        room_id: room,
        from: body.from,
        to: body.to,
        kind: body.kind,
        reason_id: body.reason_id,
        note: body.note,
    };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let created = rooms::create_block(&mut tx, ctx.tenant, ctx.user, property, input).await.map_err(rooms_error)?;
    tx.commit().await?;
    Ok(Versioned::created(created.version, created))
}

#[utoipa::path(patch, operation_id = "shorten_block", path = "/api/v1/properties/{property}/blocks/{block}", request_body = ShortenBlockRequest,
    params(("property" = Uuid, Path), ("block" = Uuid, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = Block), (status = 403), (status = 404), (status = 412), (status = 422), (status = 428)))]
pub async fn shorten(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, block)): ApiPath<(Uuid, Uuid)>,
    IfMatch(version): IfMatch,
    ApiJson(body): ApiJson<ShortenBlockRequest>,
) -> Result<Versioned<Block>, ApiError> {
    ctx.require(Permission::InventoryBlock, Some(property))?;
    validate(&body)?;
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let updated = rooms::shorten_block(&mut tx, ctx.tenant, ctx.user, property, block, version, body.to)
        .await
        .map_err(rooms_error)?;
    tx.commit().await?;
    Ok(Versioned::ok(updated.version, updated))
}
```

Modify `crates/core-api/src/routes/rooms.rs`:

```diff
--- a/crates/core-api/src/routes/rooms.rs
+++ b/crates/core-api/src/routes/rooms.rs
@@ -21,7 +21,7 @@ pub(crate) fn rooms_error(err: RoomsError) -> ApiError {
         RoomsError::VersionMismatch(_) => ApiError::precondition_failed(err.to_string()),
         RoomsError::Conflict(message) => ApiError::conflict(message),
         RoomsError::Invalid(message) => ApiError::unprocessable(message),
-        RoomsError::Overlap(_) => ApiError::conflict(err.to_string()),
+        RoomsError::Overlap(ref blocks) => ApiError::conflict(err.to_string()).with("conflicts", blocks),
         RoomsError::Database(db_err) => db_err.into(),
     }
 }
```

Modify `crates/core-api/src/routes/properties.rs`:

```diff
--- a/crates/core-api/src/routes/properties.rs
+++ b/crates/core-api/src/routes/properties.rs
@@ -63,6 +63,7 @@ pub async fn create(
         NewProperty { code: body.code, name: body.name, timezone: body.timezone, base_currency: body.base_currency };
     let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
     let created = property::create_property(&mut tx, ctx.tenant, ctx.user, input).await.map_err(property_error)?;
+    rooms::seed_block_reasons(&mut tx, ctx.tenant, created.id).await?;
     tx.commit().await?;
     Ok(Versioned::created(created.version, created))
 }
```

Modify `crates/core-api/src/routes/mod.rs`:

```diff
--- a/crates/core-api/src/routes/mod.rs
+++ b/crates/core-api/src/routes/mod.rs
@@ -1,4 +1,5 @@
 pub(crate) mod auth;
+pub(crate) mod blocks;
 mod health;
 pub(crate) mod properties;
 pub(crate) mod room_types;
@@ -18,6 +19,7 @@ use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetReques
 use tower_http::trace::TraceLayer;
 
 pub use auth::{LoginRequest, SignupRequest, SwitchTenantRequest};
+pub use blocks::{CreateBlockReasonRequest, CreateBlockRequest, ShortenBlockRequest, UpdateBlockReasonRequest};
 pub use properties::{CreatePropertyRequest, UpdatePropertyRequest};
 pub use room_types::{BedRequest, CreateRoomTypeRequest, UpdateRoomTypeRequest};
 pub use rooms::{CreateRoomRangeRequest, CreateRoomRequest, ReorderRequest, SectionRequest, UpdateRoomRequest};
@@ -36,6 +38,8 @@ pub fn router(state: AppState) -> Router {
         .route(&format!("{PROPERTY}/rooms"), post(rooms::create))
         .route(&format!("{PROPERTY}/rooms/bulk"), post(rooms::create_range))
         .route(&format!("{PROPERTY}/sections"), post(rooms::create_section))
+        .route(&format!("{PROPERTY}/block-reasons"), post(blocks::create_reason))
+        .route(&format!("{PROPERTY}/rooms/{{room}}/blocks"), post(blocks::create))
         .route_layer(from_fn_with_state(state.clone(), idempotency::idempotent));
 
     let requests = Router::new()
@@ -50,6 +54,8 @@ pub fn router(state: AppState) -> Router {
         .route(&format!("{PROPERTY}/rooms/order"), put(rooms::reorder))
         .route(&format!("{PROPERTY}/rooms/{{room}}"), patch(rooms::update))
         .route(&format!("{PROPERTY}/sections/{{section}}"), patch(rooms::rename_section))
+        .route(&format!("{PROPERTY}/block-reasons/{{reason}}"), patch(blocks::update_reason))
+        .route(&format!("{PROPERTY}/blocks/{{block}}"), patch(blocks::shorten))
         .route("/graphql", post(graphql::handler))
         .merge(commands)
         .layer(from_fn(|request, next| deadline(REQUEST_TIMEOUT, request, next)));
```

Modify `crates/core-api/src/openapi.rs`:

```diff
--- a/crates/core-api/src/openapi.rs
+++ b/crates/core-api/src/openapi.rs
@@ -1,6 +1,7 @@
 use crate::routes::{
-    BedRequest, CreatePropertyRequest, CreateRoomRangeRequest, CreateRoomRequest, CreateRoomTypeRequest, LoginRequest,
-    ReorderRequest, SectionRequest, SignupRequest, SwitchTenantRequest, UpdatePropertyRequest, UpdateRoomRequest,
+    BedRequest, CreateBlockReasonRequest, CreateBlockRequest, CreatePropertyRequest, CreateRoomRangeRequest,
+    CreateRoomRequest, CreateRoomTypeRequest, LoginRequest, ReorderRequest, SectionRequest, ShortenBlockRequest,
+    SignupRequest, SwitchTenantRequest, UpdateBlockReasonRequest, UpdatePropertyRequest, UpdateRoomRequest,
     UpdateRoomTypeRequest,
 };
 use utoipa::OpenApi;
@@ -25,6 +26,10 @@ use utoipa::OpenApi;
         crate::routes::rooms::reorder,
         crate::routes::rooms::create_section,
         crate::routes::rooms::rename_section,
+        crate::routes::blocks::create_reason,
+        crate::routes::blocks::update_reason,
+        crate::routes::blocks::create,
+        crate::routes::blocks::shorten,
     ),
     components(schemas(
         SignupRequest,
@@ -40,6 +45,10 @@ use utoipa::OpenApi;
         CreateRoomRangeRequest,
         UpdateRoomRequest,
         SectionRequest,
+        CreateBlockReasonRequest,
+        UpdateBlockReasonRequest,
+        CreateBlockRequest,
+        ShortenBlockRequest,
         identity::Profile,
         identity::TenantSummary,
         identity::Grant,
@@ -49,6 +58,9 @@ use utoipa::OpenApi;
         rooms::RoomType,
         rooms::Room,
         rooms::Section,
+        rooms::BlockKind,
+        rooms::BlockReason,
+        rooms::Block,
     ))
 )]
 pub struct ApiDoc;
```

Modify `docs/design/api-conventions.md`:

````diff
--- a/docs/design/api-conventions.md
+++ b/docs/design/api-conventions.md
@@ -43,7 +43,7 @@ Every non-GET/HEAD/OPTIONS request must send `x-goodfolk-csrf: 1` (`crates/core-
 { "type": "about:blank", "title": "Conflict", "status": 409, "detail": "a property with this code already exists" }
 ```
 
-`Content-Type: application/problem+json`, for every error the API returns, including malformed bodies and query strings, GraphQL request parse failures and timeouts. Construct errors with `ApiError::{bad_request, unauthenticated, invalid_credentials, forbidden, not_found, method_not_allowed, conflict, precondition_failed, precondition_required, unprocessable, too_many_requests, gateway_timeout, internal}`. Database errors are logged and become a bare 500, and internal details never reach the client.
+`Content-Type: application/problem+json`, for every error the API returns, including malformed bodies and query strings, GraphQL request parse failures and timeouts. Construct errors with `ApiError::{bad_request, unauthenticated, invalid_credentials, forbidden, not_found, method_not_allowed, conflict, precondition_failed, precondition_required, unprocessable, too_many_requests, gateway_timeout, internal}`. Database errors are logged and become a bare 500, and internal details never reach the client. A problem may carry extension members added with `ApiError::with(name, value)`: a 409 for an overlapping room block lists the blocks in the way as `conflicts`.
 
 | Status | When |
 |---|---|
````

- [ ] **Step 4: Regenerate the API types**

```sh
cd web/pms && bun run api:schemas && bun run codegen
```

- [ ] **Step 5: Run the tests to verify they pass**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cd web/pms
bun run lint && bun run check && bun run test && bun run build
```

Expected: PASS (`blocks`: 4, `openapi`: 2). Web checks clean.

- [ ] **Step 6: Commit**

```sh
git add -A
git commit -m "feat(api): block reasons and room blocks, with the conflicting blocks in a 409"
```

### Task 12: GraphQL reads and the inventory performance gate

The screens read through GraphQL: `roomTypes`, `rooms(roomTypeId?)`, `sections`, `blockReasons`, `blocks(from, to)` and `inventory(from, to)`, each checking its permission for the `propertyId` argument (`graphql::scoped`) and mapping every database error through `internal`. Ranges are bounded (93 days for `inventory`, 400 for `blocks`). The lists are flat, so no DataLoader is needed yet.

`tests/perf.rs` is the spec's server-time gate: a 200-room, 12-type property with blocks, 200 timed month queries through the router after 20 warm-up queries, p95 under 20 ms. It is `#[ignore]`d because debug builds are several times slower; run it in release mode.

**Files:**
- Modify: `crates/core-api/src/graphql.rs`
- Test: `crates/core-api/tests/inventory.rs` (new)
- Test: `crates/core-api/tests/perf.rs` (new)
- Regenerated, not shown: `web/pms/src/lib/api/schema.graphql`

**Interfaces:**
- Consumes: `rooms::{list_room_types, list_rooms, list_sections, list_block_reasons, list_blocks, list_inventory}`.
- Produces: GraphQL `roomTypes(propertyId)`, `rooms(propertyId, roomTypeId)`, `sections(propertyId)`, `blockReasons(propertyId)`, `blocks(propertyId, from, to)`, `inventory(propertyId, from, to) { date roomTypeId physical sold outOfOrder available }`; enum `BlockKind { OUT_OF_ORDER, OUT_OF_SERVICE }`; scalar `Date`.

- [ ] **Step 1: Write the failing tests**

Create `crates/core-api/tests/inventory.rs`:

```rust
mod common;

use axum::http::{Method, StatusCode};
use common::{TestApp, TestResponse};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use time::{Date, Duration, format_description::well_known::Iso8601};
use uuid::Uuid;

async fn post(app: &TestApp, cookie: &str, path: &str, body: Value) -> TestResponse {
    let key = Uuid::now_v7().to_string();
    app.send_with(Method::POST, path, Some(cookie), Some(body), &[("x-goodfolk-csrf", "1"), ("idempotency-key", &key)])
        .await
}

async fn graphql(app: &TestApp, cookie: &str, query: &str, variables: Value) -> Value {
    let response =
        app.send(Method::POST, "/graphql", Some(cookie), Some(json!({"query": query, "variables": variables}))).await;
    assert_eq!(response.status, StatusCode::OK, "{:?}", response.body);
    response.body
}

/// Galle with types STD (rooms 101, 102) and DLX (room 201), and room 101 out of order for two days from
/// the business date plus one.
struct Galle {
    owner: String,
    id: String,
    business_date: Date,
    std: Value,
    dlx: Value,
}

impl Galle {
    async fn new(app: &TestApp, superuser: &PgPool) -> Self {
        let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
        let property = json!({"code": "GAL", "name": "Galle", "timezone": "Asia/Colombo", "base_currency": "LKR"});
        let property = post(app, &owner, "/api/v1/properties", property).await.body;
        let id = property["id"].as_str().unwrap().to_owned();
        let path = format!("/api/v1/properties/{id}");
        let business_date = Date::parse(property["business_date"].as_str().unwrap(), &Iso8601::DEFAULT).unwrap();
        let room_type = |code: &str| json!({"code": code, "name": code, "base_occupancy": 2, "max_adults": 2, "max_children": 0, "max_occupancy": 2});
        let std = post(app, &owner, &format!("{path}/room-types"), room_type("STD")).await.body;
        let dlx = post(app, &owner, &format!("{path}/room-types"), room_type("DLX")).await.body;
        let rooms = post(
            app,
            &owner,
            &format!("{path}/rooms/bulk"),
            json!({"room_type_id": std["id"], "first": 101, "last": 102}),
        )
        .await
        .body;
        post(app, &owner, &format!("{path}/rooms"), json!({"room_type_id": dlx["id"], "number": "201", "floor": "2"}))
            .await;
        post(app, &owner, &format!("{path}/sections"), json!({"name": "East"})).await;
        let reason: Uuid = sqlx::query_scalar("select id from block_reason where code = 'RENOVATION'")
            .fetch_one(superuser)
            .await
            .unwrap();
        let block = json!({"from": (business_date + Duration::days(1)).to_string(),
                           "to": (business_date + Duration::days(3)).to_string(),
                           "kind": "out_of_order", "reason_id": reason});
        let blocked =
            post(app, &owner, &format!("{path}/rooms/{}/blocks", rooms[0]["id"].as_str().unwrap()), block).await;
        assert_eq!(blocked.status, StatusCode::CREATED, "{:?}", blocked.body);
        Self { owner, id, business_date, std, dlx }
    }

    fn day(&self, offset: i64) -> String {
        (self.business_date + Duration::days(offset)).to_string()
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn room_types_rooms_sections_and_reasons_are_listed(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let galle = Galle::new(&app, &PgPool::connect_with(opts).await.unwrap()).await;

    let body = graphql(
        &app,
        &galle.owner,
        "query ($p: UUID!, $dlx: UUID) {
           roomTypes(propertyId: $p) { code maxOccupancy active version }
           rooms(propertyId: $p) { number floor active }
           dlxRooms: rooms(propertyId: $p, roomTypeId: $dlx) { number }
           sections(propertyId: $p) { name }
           blockReasons(propertyId: $p) { code defaultKind }
         }",
        json!({"p": galle.id, "dlx": galle.dlx["id"]}),
    )
    .await;

    let data = &body["data"];
    assert_eq!(
        data["roomTypes"],
        json!([{"code": "STD", "maxOccupancy": 2, "active": true, "version": 1},
               {"code": "DLX", "maxOccupancy": 2, "active": true, "version": 1}])
    );
    assert_eq!(
        data["rooms"],
        json!([{"number": "101", "floor": null, "active": true}, {"number": "102", "floor": null, "active": true},
               {"number": "201", "floor": "2", "active": true}])
    );
    assert_eq!(data["dlxRooms"], json!([{"number": "201"}]));
    assert_eq!(data["sections"], json!([{"name": "East"}]));
    assert_eq!(data["blockReasons"][0], json!({"code": "CONSTRUCTION", "defaultKind": "OUT_OF_ORDER"}));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn the_inventory_calendar_counts_rooms_per_type_per_day(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let galle = Galle::new(&app, &PgPool::connect_with(opts).await.unwrap()).await;

    let body = graphql(
        &app,
        &galle.owner,
        "query ($p: UUID!, $from: Date!, $to: Date!) {
           inventory(propertyId: $p, from: $from, to: $to) { date roomTypeId physical sold outOfOrder available }
           blocks(propertyId: $p, from: $from, to: $to) { from to kind note }
         }",
        json!({"p": galle.id, "from": galle.day(0), "to": galle.day(4)}),
    )
    .await;

    let days = body["data"]["inventory"].as_array().unwrap();
    assert_eq!(days.len(), 8, "4 days x 2 room types: {days:?}");
    let std_available: Vec<i64> = days
        .iter()
        .filter(|day| day["roomTypeId"] == galle.std["id"])
        .map(|day| day["available"].as_i64().unwrap())
        .collect();
    assert_eq!(std_available, [2, 1, 1, 2]);
    let dlx = days.iter().find(|day| day["roomTypeId"] == galle.dlx["id"]).unwrap();
    assert_eq!(
        dlx,
        &json!({"date": galle.day(0), "roomTypeId": galle.dlx["id"], "physical": 1, "sold": 0,
                            "outOfOrder": 0, "available": 1})
    );
    assert_eq!(
        body["data"]["blocks"],
        json!([{"from": galle.day(1), "to": galle.day(3), "kind": "OUT_OF_ORDER", "note": ""}])
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_calendar_range_is_limited_to_93_days(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let galle = Galle::new(&app, &PgPool::connect_with(opts).await.unwrap()).await;

    let body = graphql(
        &app,
        &galle.owner,
        "query ($p: UUID!, $from: Date!, $to: Date!) { inventory(propertyId: $p, from: $from, to: $to) { date } }",
        json!({"p": galle.id, "from": galle.day(0), "to": galle.day(94)}),
    )
    .await;

    assert_eq!(body["errors"][0]["message"], "the range must be 1 to 93 days");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn another_tenant_sees_no_rooms_or_inventory(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let galle = Galle::new(&app, &PgPool::connect_with(opts).await.unwrap()).await;
    let stranger = app.signup_owner("stranger@example.com", "Other Hotels").await;

    let body = graphql(
        &app,
        &stranger,
        "query ($p: UUID!, $from: Date!, $to: Date!) {
           roomTypes(propertyId: $p) { id } rooms(propertyId: $p) { id }
           inventory(propertyId: $p, from: $from, to: $to) { date } blocks(propertyId: $p, from: $from, to: $to) { id }
         }",
        json!({"p": galle.id, "from": galle.day(0), "to": galle.day(30)}),
    )
    .await;

    assert_eq!(body["data"], json!({"roomTypes": [], "rooms": [], "inventory": [], "blocks": []}));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn property_scoped_staff_see_only_their_property(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let superuser = PgPool::connect_with(opts).await.unwrap();
    let galle = Galle::new(&app, &superuser).await;
    let kandy = json!({"code": "KAN", "name": "Kandy", "timezone": "Asia/Colombo", "base_currency": "LKR"});
    let kandy = post(&app, &galle.owner, "/api/v1/properties", kandy).await.body["id"].clone();
    let desk = app.staff(&superuser, &galle.owner, "desk@example.com", "front_desk").await;
    // Narrow the tenant-wide grant `staff` gives to Kandy only.
    sqlx::query("update role_grant set property_id = $1 where role = 'front_desk'")
        .bind(common::uuid(&kandy))
        .execute(&superuser)
        .await
        .unwrap();

    let response = app
        .send(
            Method::POST,
            "/graphql",
            Some(&desk),
            Some(json!({"query": "query ($p: UUID!) { roomTypes(propertyId: $p) { id } }", "variables": {"p": galle.id}})),
        )
        .await;

    assert_eq!(response.body["errors"][0]["message"], "you do not have permission for this property");
}
```

Create `crates/core-api/tests/perf.rs`:

````rust
//! Phase 1 performance gate: one month of `inventory` for a 200-room, 12-type property is answered in
//! under 20 ms at p95. Timed in-process through the router, so it is server time: authentication, the
//! indexed range scan of at most 372 rows, and JSON, without the network.
//!
//! Ignored by default because debug builds are several times slower. Run it in release mode:
//!
//! ```sh
//! DATABASE_URL=$TEST_DATABASE_URL cargo test --release -p core-api --test perf -- --ignored --nocapture
//! ```

mod common;

use axum::http::{Method, StatusCode};
use common::{TestApp, TestResponse};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::time::{Duration, Instant};
use time::format_description::well_known::Iso8601;
use uuid::Uuid;

const ROOM_TYPES: u32 = 12;
const ROOMS: u32 = 200;
const SAMPLES: usize = 200;

async fn post(app: &TestApp, cookie: &str, path: &str, body: Value) -> TestResponse {
    let key = Uuid::now_v7().to_string();
    let response = app
        .send_with(Method::POST, path, Some(cookie), Some(body), &[("x-goodfolk-csrf", "1"), ("idempotency-key", &key)])
        .await;
    assert_eq!(response.status, StatusCode::CREATED, "{:?}", response.body);
    response
}

#[sqlx::test(migrator = "db::MIGRATOR")]
#[ignore = "performance gate; run in release mode (see the module docs)"]
async fn a_month_of_inventory_for_200_rooms_is_served_under_20ms_at_p95(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let superuser = PgPool::connect_with(opts).await.unwrap();
    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
    let property = json!({"code": "BIG", "name": "Big Hotel", "timezone": "Asia/Colombo", "base_currency": "LKR"});
    let property = post(&app, &owner, "/api/v1/properties", property).await.body;
    let path = format!("/api/v1/properties/{}", property["id"].as_str().unwrap());
    let business_date = time::Date::parse(property["business_date"].as_str().unwrap(), &Iso8601::DEFAULT).unwrap();
    let reason: Uuid = sqlx::query_scalar("select id from block_reason where code = 'RENOVATION'")
        .fetch_one(&superuser)
        .await
        .unwrap();
    for index in 0..ROOM_TYPES {
        let room_type = json!({"code": format!("T{index}"), "name": format!("Type {index}"), "base_occupancy": 2,
                               "max_adults": 2, "max_children": 1, "max_occupancy": 3});
        let room_type = post(&app, &owner, &format!("{path}/room-types"), room_type).await.body["id"].clone();
        let count = ROOMS / ROOM_TYPES + u32::from(index < ROOMS % ROOM_TYPES);
        let first = (index + 1) * 100 + 1;
        let range = json!({"room_type_id": room_type, "first": first, "last": first + count - 1});
        let rooms = post(&app, &owner, &format!("{path}/rooms/bulk"), range).await.body;
        // Two blocks per type inside the measured month.
        for (offset, room) in rooms.as_array().unwrap().iter().take(2).enumerate() {
            let from = business_date + time::Duration::days(i64::try_from(offset).unwrap() * 7 + 3);
            let block = json!({"from": from.to_string(), "to": (from + time::Duration::days(4)).to_string(),
                               "kind": "out_of_order", "reason_id": reason});
            post(&app, &owner, &format!("{path}/rooms/{}/blocks", room["id"].as_str().unwrap()), block).await;
        }
    }
    let query = json!({
        "query": "query ($p: UUID!, $from: Date!, $to: Date!) {
                    inventory(propertyId: $p, from: $from, to: $to) { date roomTypeId physical sold outOfOrder available }
                  }",
        "variables": {"p": property["id"], "from": business_date.to_string(),
                      "to": (business_date + time::Duration::days(31)).to_string()},
    });

    let mut samples: Vec<Duration> = Vec::with_capacity(SAMPLES);
    for round in 0..SAMPLES + 20 {
        let started = Instant::now();
        let response = app.send(Method::POST, "/graphql", Some(&owner), Some(query.clone())).await;
        let elapsed = started.elapsed();
        assert_eq!(response.body["data"]["inventory"].as_array().map(Vec::len), Some(31 * 12), "{:?}", response.body);
        // The first 20 warm the connection pool and Postgres' caches.
        if round >= 20 {
            samples.push(elapsed);
        }
    }

    samples.sort();
    let p50 = samples[SAMPLES / 2];
    let p95 = samples[SAMPLES * 95 / 100 - 1];
    println!("inventory(month), {ROOMS} rooms / {ROOM_TYPES} types: p50 {p50:?}, p95 {p95:?}");
    assert!(p95 < Duration::from_millis(20), "p95 {p95:?} is over the 20 ms gate");
}
````

- [ ] **Step 2: Run the tests to verify they fail**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p core-api --test inventory
```

Expected: FAIL: 5 tests (the queries name fields the schema does not have yet).

- [ ] **Step 3: Implement**

Modify `crates/core-api/src/graphql.rs`:

```diff
--- a/crates/core-api/src/graphql.rs
+++ b/crates/core-api/src/graphql.rs
@@ -3,13 +3,14 @@
 use crate::auth::TenantContext;
 use crate::error::ApiError;
 use crate::state::AppState;
-use async_graphql::{Context, EmptyMutation, EmptySubscription, Object, Schema, SimpleObject};
+use async_graphql::{Context, EmptyMutation, EmptySubscription, Enum, Object, Schema, SimpleObject};
 use async_graphql_axum::rejection::GraphQLRejection;
 use async_graphql_axum::{GraphQLRequest, GraphQLResponse};
 use axum::extract::State;
-use db::Scope;
+use db::{Scope, Tx};
+use identity::Permission;
 use sqlx::PgPool;
-use time::Date;
+use time::{Date, Duration};
 use uuid::Uuid;
 
 pub type GqlSchema = Schema<Query, EmptyMutation, EmptySubscription>;
@@ -51,6 +52,152 @@ pub struct PropertyNode {
     pub version: i32,
 }
 
+#[derive(SimpleObject)]
+pub struct BedNode {
+    pub kind: String,
+    pub count: i32,
+}
+
+#[derive(SimpleObject)]
+pub struct RoomTypeNode {
+    pub id: Uuid,
+    pub code: String,
+    pub name: String,
+    pub base_occupancy: i32,
+    pub max_adults: i32,
+    pub max_children: i32,
+    pub max_occupancy: i32,
+    pub beds: Vec<BedNode>,
+    pub amenities: Vec<String>,
+    pub sort_order: i32,
+    pub active: bool,
+    pub version: i32,
+}
+
+impl From<rooms::RoomType> for RoomTypeNode {
+    fn from(t: rooms::RoomType) -> Self {
+        Self {
+            id: t.id,
+            code: t.code,
+            name: t.name,
+            base_occupancy: t.base_occupancy,
+            max_adults: t.max_adults,
+            max_children: t.max_children,
+            max_occupancy: t.max_occupancy,
+            beds: t.bed_config.into_iter().map(|bed| BedNode { kind: bed.kind, count: bed.count }).collect(),
+            amenities: t.amenities,
+            sort_order: t.sort_order,
+            active: t.active,
+            version: t.version,
+        }
+    }
+}
+
+#[derive(SimpleObject)]
+pub struct RoomNode {
+    pub id: Uuid,
+    pub room_type_id: Uuid,
+    pub number: String,
+    pub floor: Option<String>,
+    pub section_id: Option<Uuid>,
+    pub active: bool,
+    pub sort_order: i32,
+    pub version: i32,
+}
+
+impl From<rooms::Room> for RoomNode {
+    fn from(r: rooms::Room) -> Self {
+        Self {
+            id: r.id,
+            room_type_id: r.room_type_id,
+            number: r.number,
+            floor: r.floor,
+            section_id: r.section_id,
+            active: r.active,
+            sort_order: r.sort_order,
+            version: r.version,
+        }
+    }
+}
+
+#[derive(SimpleObject)]
+pub struct SectionNode {
+    pub id: Uuid,
+    pub name: String,
+    pub version: i32,
+}
+
+#[derive(Enum, Clone, Copy, PartialEq, Eq)]
+#[graphql(name = "BlockKind")]
+pub enum BlockKindNode {
+    /// Out of inventory: reduces availability.
+    OutOfOrder,
+    /// Still sellable; shown only.
+    OutOfService,
+}
+
+impl From<rooms::BlockKind> for BlockKindNode {
+    fn from(kind: rooms::BlockKind) -> Self {
+        match kind {
+            rooms::BlockKind::OutOfOrder => BlockKindNode::OutOfOrder,
+            rooms::BlockKind::OutOfService => BlockKindNode::OutOfService,
+        }
+    }
+}
+
+#[derive(SimpleObject)]
+pub struct BlockReasonNode {
+    pub id: Uuid,
+    pub code: String,
+    pub label: String,
+    pub default_kind: BlockKindNode,
+    pub active: bool,
+    pub version: i32,
+}
+
+/// A room blocked for `[from, to)`: `to` is the first day it is back.
+#[derive(SimpleObject)]
+pub struct BlockNode {
+    pub id: Uuid,
+    pub room_id: Uuid,
+    pub from: Date,
+    pub to: Date,
+    pub kind: BlockKindNode,
+    pub reason_id: Uuid,
+    pub note: String,
+    pub version: i32,
+}
+
+/// One room type on one day. `available = physical - sold - outOfOrder`.
+#[derive(SimpleObject)]
+pub struct InventoryDayNode {
+    pub date: Date,
+    pub room_type_id: Uuid,
+    pub physical: i32,
+    pub sold: i32,
+    pub out_of_order: i32,
+    pub available: i32,
+}
+
+/// Checks `permission` for `property` and opens a transaction in the caller's tenant.
+async fn scoped(ctx: &Context<'_>, permission: Permission, property: Uuid) -> async_graphql::Result<Tx> {
+    let pool = ctx.data::<PgPool>()?;
+    let tenant = ctx.data::<TenantContext>()?;
+    tenant
+        .require(permission, Some(property))
+        .map_err(|_| async_graphql::Error::new("you do not have permission for this property"))?;
+    db::begin(pool, Scope::tenant(tenant.tenant)).await.map_err(internal)
+}
+
+/// `[from, to)` must span 1 to `max_days` days.
+fn check_range(from: Date, to: Date, max_days: i64) -> async_graphql::Result<()> {
+    if to > from && to - from <= Duration::days(max_days) {
+        Ok(())
+    } else {
+        Err(async_graphql::Error::new(format!("the range must be 1 to {max_days} days")))
+    }
+}
+
 pub struct Query;
 
 #[Object]
@@ -78,6 +225,106 @@ impl Query {
             })
             .collect())
     }
+
+    /// The property's room types, active or not, in display order.
+    async fn room_types(&self, ctx: &Context<'_>, property_id: Uuid) -> async_graphql::Result<Vec<RoomTypeNode>> {
+        let mut tx = scoped(ctx, Permission::RoomsView, property_id).await?;
+        let types = rooms::list_room_types(&mut tx, property_id).await.map_err(internal)?;
+        tx.commit().await.map_err(internal)?;
+        Ok(types.into_iter().map(RoomTypeNode::from).collect())
+    }
+
+    /// The property's rooms, active or not, in display order; only one type's if `roomTypeId` is given.
+    async fn rooms(
+        &self,
+        ctx: &Context<'_>,
+        property_id: Uuid,
+        room_type_id: Option<Uuid>,
+    ) -> async_graphql::Result<Vec<RoomNode>> {
+        let mut tx = scoped(ctx, Permission::RoomsView, property_id).await?;
+        let rooms = rooms::list_rooms(&mut tx, property_id, room_type_id).await.map_err(internal)?;
+        tx.commit().await.map_err(internal)?;
+        Ok(rooms.into_iter().map(RoomNode::from).collect())
+    }
+
+    /// Housekeeping sections, by name.
+    async fn sections(&self, ctx: &Context<'_>, property_id: Uuid) -> async_graphql::Result<Vec<SectionNode>> {
+        let mut tx = scoped(ctx, Permission::RoomsView, property_id).await?;
+        let sections = rooms::list_sections(&mut tx, property_id).await.map_err(internal)?;
+        tx.commit().await.map_err(internal)?;
+        Ok(sections.into_iter().map(|s| SectionNode { id: s.id, name: s.name, version: s.version }).collect())
+    }
+
+    /// Reasons a room can be blocked for, active or not, by code.
+    async fn block_reasons(&self, ctx: &Context<'_>, property_id: Uuid) -> async_graphql::Result<Vec<BlockReasonNode>> {
+        let mut tx = scoped(ctx, Permission::RoomsView, property_id).await?;
+        let reasons = rooms::list_block_reasons(&mut tx, property_id).await.map_err(internal)?;
+        tx.commit().await.map_err(internal)?;
+        Ok(reasons
+            .into_iter()
+            .map(|r| BlockReasonNode {
+                id: r.id,
+                code: r.code,
+                label: r.label,
+                default_kind: r.default_kind.into(),
+                active: r.active,
+                version: r.version,
+            })
+            .collect())
+    }
+
+    /// Active room blocks overlapping `[from, to)` (at most 400 days), by start date.
+    async fn blocks(
+        &self,
+        ctx: &Context<'_>,
+        property_id: Uuid,
+        from: Date,
+        to: Date,
+    ) -> async_graphql::Result<Vec<BlockNode>> {
+        check_range(from, to, 400)?;
+        let mut tx = scoped(ctx, Permission::InventoryView, property_id).await?;
+        let blocks = rooms::list_blocks(&mut tx, property_id, from, to).await.map_err(internal)?;
+        tx.commit().await.map_err(internal)?;
+        Ok(blocks
+            .into_iter()
+            .map(|b| BlockNode {
+                id: b.id,
+                room_id: b.room_id,
+                from: b.from,
+                to: b.to,
+                kind: b.kind.into(),
+                reason_id: b.reason_id,
+                note: b.note,
+                version: b.version,
+            })
+            .collect())
+    }
+
+    /// Counts per room type per day for `[from, to)` (at most 93 days), by date then room type. Days before
+    /// the business date that were never counted, and days past the 730-day window, have no rows.
+    async fn inventory(
+        &self,
+        ctx: &Context<'_>,
+        property_id: Uuid,
+        from: Date,
+        to: Date,
+    ) -> async_graphql::Result<Vec<InventoryDayNode>> {
+        check_range(from, to, 93)?;
+        let mut tx = scoped(ctx, Permission::InventoryView, property_id).await?;
+        let days = rooms::list_inventory(&mut tx, property_id, from, to).await.map_err(internal)?;
+        tx.commit().await.map_err(internal)?;
+        Ok(days
+            .into_iter()
+            .map(|d| InventoryDayNode {
+                date: d.date,
+                room_type_id: d.room_type_id,
+                physical: d.physical,
+                sold: d.sold,
+                out_of_order: d.out_of_order,
+                available: d.available(),
+            })
+            .collect())
+    }
 }
 
 #[cfg(test)]
```

- [ ] **Step 4: Regenerate the GraphQL schema**

```sh
cd web/pms && bun run api:schemas && bun run codegen
```

- [ ] **Step 5: Run the tests and the performance gate**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
DATABASE_URL=$TEST_DATABASE_URL cargo test --release -p core-api --test perf -- --ignored --nocapture
```

Expected: PASS (`inventory`: 5; `perf` ignored in the normal run). The release run prints about `inventory(month), 200 rooms / 12 types: p50 3.2ms, p95 3.7ms` and passes.

- [ ] **Step 6: Commit**

```sh
git add -A
git commit -m "feat(graphql): room types, rooms, sections, block reasons, blocks and the inventory calendar, with a performance gate"
```

### Task 13: Frontend data layer: queries, keys and grid math

Pure, tested building blocks for the screens. `rooms.ts` and `inventory.ts` declare the GraphQL documents and query keys that match the server's events (`room-types:<p>`, `rooms:<p>`, `inventory:<p>:<yyyy-mm>`), plus helpers: reordering, grouping rooms by type or floor, bulk-range previews, month and date arithmetic on `YYYY-MM-DD` strings, and looking up counts and blocks for a day. `grid.ts` is the virtualization math the grid (and later the tape chart) uses: which columns intersect the viewport plus overscan, the scroll position that reveals a column, and keyboard movement. `rest.ts` gains `unwrap` (data, or the problem as an `ApiError`) and `ifMatch(version)`; `session.ts` gains `can(profile, action, propertyId)` as a UI hint mirroring the server's permissions.

**Files:**
- Create: `web/pms/src/lib/grid.ts`
- Create: `web/pms/src/lib/inventory.ts`
- Create: `web/pms/src/lib/rooms.ts`
- Modify: `web/pms/src/lib/api/rest.ts`
- Modify: `web/pms/src/lib/session.ts`
- Test: `web/pms/src/lib/api/rest.spec.ts` (new)
- Test: `web/pms/src/lib/grid.spec.ts` (new)
- Test: `web/pms/src/lib/inventory.spec.ts` (new)
- Test: `web/pms/src/lib/rooms.spec.ts` (new)
- Test: `web/pms/src/lib/session.spec.ts`
- Regenerated, not shown: `web/pms/src/lib/api/gql/gql.ts`, `web/pms/src/lib/api/gql/graphql.ts`

**Interfaces:**
- Consumes: the generated `paths`/`components` and GraphQL types.
- Produces: `grid.ts`: `visibleColumns(scrollLeft, viewportWidth, columnWidth, count, overscan = 2) -> { start, end }`, `revealColumn(column, scrollLeft, viewportWidth, columnWidth) -> number`, `moveFocus(cell, key, size) -> Cell | null`; `rooms.ts`: `RoomTypesDocument`, `RoomsDocument` (rooms, sections, blockReasons), `roomTypesKey`, `roomsKey`, `fetchRoomTypes`, `fetchRooms`, `moveItem`, `groupRooms`, `rangeNumbers`, types `RoomType`, `Room`, `Section`, `BlockReason`; `inventory.ts`: `InventoryDocument`, `inventoryKey`, `fetchMonth`, `addDays`, `monthOf`, `shiftMonth`, `monthDays`, `indexInventory`, `blocksOn`, types `InventoryDay`, `Block`; `rest.ts`: `unwrap`, `ifMatch`; `session.ts`: `can(profile, 'manageRooms' | 'blockRooms', propertyId)`.

- [ ] **Step 1: Write the failing tests**

Create `web/pms/src/lib/api/rest.spec.ts`:

```ts
import { describe, expect, it } from 'vitest';
import { ApiError } from './problem';
import { ifMatch, unwrap } from './rest';

describe('unwrap', () => {
	it('returns the data of a successful response', () => {
		expect(unwrap({ data: { id: 'x' }, response: new Response(null, { status: 201 }) })).toEqual({
			id: 'x'
		});
	});

	it('throws the problem of a failed response as an ApiError', () => {
		const problem = { type: 'about:blank', title: 'Conflict', status: 409, detail: 'taken' };

		const failed = () => unwrap({ error: problem, response: new Response(null, { status: 409 }) });

		expect(failed).toThrow(ApiError);
		expect(failed).toThrow('taken');
	});
});

describe('ifMatch', () => {
	it('quotes the version as a strong ETag', () => {
		expect(ifMatch(3)).toEqual({ 'If-Match': '"3"' });
	});
});
```

Create `web/pms/src/lib/grid.spec.ts`:

```ts
import { describe, expect, it } from 'vitest';
import { moveFocus, revealColumn, visibleColumns } from './grid';

describe('visibleColumns', () => {
	it('covers the columns in view plus overscan on both sides', () => {
		// 56 px columns, scrolled 3.5 columns in, 5 columns wide.
		expect(visibleColumns(196, 280, 56, 31, 1)).toEqual({ start: 2, end: 10 });
	});

	it('is clamped to the columns that exist', () => {
		expect(visibleColumns(0, 280, 56, 31, 2)).toEqual({ start: 0, end: 7 });
		expect(visibleColumns(56 * 29, 280, 56, 31, 2)).toEqual({ start: 27, end: 31 });
		expect(visibleColumns(0, 280, 56, 0, 2)).toEqual({ start: 0, end: 0 });
	});
});

describe('revealColumn', () => {
	it('scrolls just enough to show a column that is out of view', () => {
		expect(revealColumn(10, 0, 280, 56)).toBe(11 * 56 - 280);
		expect(revealColumn(1, 5 * 56, 280, 56)).toBe(56);
	});

	it('leaves the scroll position alone when the column is visible', () => {
		expect(revealColumn(3, 56, 280, 56)).toBe(56);
	});
});

describe('moveFocus', () => {
	const size = { rows: 3, columns: 31 };

	it('moves one cell with the arrow keys and stops at the edges', () => {
		expect(moveFocus({ row: 1, column: 5 }, 'ArrowRight', size)).toEqual({ row: 1, column: 6 });
		expect(moveFocus({ row: 1, column: 5 }, 'ArrowUp', size)).toEqual({ row: 0, column: 5 });
		expect(moveFocus({ row: 0, column: 0 }, 'ArrowLeft', size)).toEqual({ row: 0, column: 0 });
		expect(moveFocus({ row: 2, column: 30 }, 'ArrowDown', size)).toEqual({ row: 2, column: 30 });
	});

	it('jumps to the first or last day with Home and End', () => {
		expect(moveFocus({ row: 1, column: 5 }, 'Home', size)).toEqual({ row: 1, column: 0 });
		expect(moveFocus({ row: 1, column: 5 }, 'End', size)).toEqual({ row: 1, column: 30 });
	});

	it('ignores other keys', () => {
		expect(moveFocus({ row: 1, column: 5 }, 'a', size)).toBeNull();
	});
});
```

Create `web/pms/src/lib/inventory.spec.ts`:

```ts
import { describe, expect, it } from 'vitest';
import {
	addDays,
	blocksOn,
	indexInventory,
	inventoryKey,
	monthDays,
	monthOf,
	shiftMonth
} from './inventory';

describe('month helpers', () => {
	it('lists every day of a month as YYYY-MM-DD', () => {
		const days = monthDays('2028-02');

		expect(days).toHaveLength(29);
		expect(days[0]).toBe('2028-02-01');
		expect(days.at(-1)).toBe('2028-02-29');
	});

	it('moves between months across years', () => {
		expect(shiftMonth('2026-12', 1)).toBe('2027-01');
		expect(shiftMonth('2026-01', -1)).toBe('2025-12');
		expect(monthOf('2026-09-24')).toBe('2026-09');
	});

	it('adds days across month ends', () => {
		expect(addDays('2026-09-30', 1)).toBe('2026-10-01');
		expect(addDays('2026-03-01', -1)).toBe('2026-02-28');
	});

	it('keys a month the way the server names its events', () => {
		expect(inventoryKey('p1', '2026-09')).toEqual(['inventory:p1:2026-09']);
	});
});

describe('indexInventory', () => {
	it('finds a room type on a day', () => {
		const day = {
			date: '2026-09-24',
			roomTypeId: 't1',
			physical: 5,
			sold: 0,
			outOfOrder: 1,
			available: 4
		};

		const index = indexInventory([day]);

		expect(index.get('t1', '2026-09-24')).toBe(day);
		expect(index.get('t1', '2026-09-25')).toBeUndefined();
	});
});

describe('blocksOn', () => {
	const block = (id: string, roomId: string, from: string, to: string) => ({
		id,
		roomId,
		from,
		to,
		kind: 'OUT_OF_ORDER' as const,
		reasonId: 'r',
		note: '',
		version: 1
	});

	it('includes the first day of a block but not the day it ends', () => {
		const blocks = [block('b1', 'r101', '2026-09-24', '2026-09-26')];

		expect(blocksOn(blocks, '2026-09-24')).toHaveLength(1);
		expect(blocksOn(blocks, '2026-09-25')).toHaveLength(1);
		expect(blocksOn(blocks, '2026-09-26')).toHaveLength(0);
	});

	it('keeps only the given rooms', () => {
		const blocks = [
			block('b1', 'r101', '2026-09-24', '2026-09-26'),
			block('b2', 'r201', '2026-09-24', '2026-09-26')
		];

		expect(blocksOn(blocks, '2026-09-24', new Set(['r201'])).map((b) => b.id)).toEqual(['b2']);
	});
});
```

Create `web/pms/src/lib/rooms.spec.ts`:

```ts
import { describe, expect, it } from 'vitest';
import { groupRooms, moveItem, rangeNumbers, roomTypesKey, roomsKey } from './rooms';

const room = (id: string, number: string, roomTypeId: string, floor: string | null) => ({
	id,
	number,
	roomTypeId,
	floor,
	sectionId: null,
	active: true,
	sortOrder: 0,
	version: 1
});

describe('moveItem', () => {
	it('moves an item to a new position without changing the input', () => {
		const items = ['a', 'b', 'c', 'd'];

		expect(moveItem(items, 0, 2)).toEqual(['b', 'c', 'a', 'd']);
		expect(moveItem(items, 3, 0)).toEqual(['d', 'a', 'b', 'c']);
		expect(items).toEqual(['a', 'b', 'c', 'd']);
	});

	it('ignores moves past either end', () => {
		expect(moveItem(['a', 'b'], 0, -1)).toEqual(['a', 'b']);
		expect(moveItem(['a', 'b'], 1, 2)).toEqual(['a', 'b']);
	});
});

describe('groupRooms', () => {
	const types = [
		{ id: 'std', code: 'STD', name: 'Standard' },
		{ id: 'dlx', code: 'DLX', name: 'Deluxe' }
	];
	const rooms = [
		room('1', '201', 'dlx', '2'),
		room('2', '101', 'std', '1'),
		room('3', 'G1', 'std', null)
	];

	it('groups by room type in type order, keeping room order within a group', () => {
		const groups = groupRooms(rooms, types, 'type');

		expect(groups.map((g) => g.label)).toEqual(['STD · Standard', 'DLX · Deluxe']);
		expect(groups[0].rooms.map((r) => r.number)).toEqual(['101', 'G1']);
	});

	it('groups by floor, rooms without a floor last', () => {
		const groups = groupRooms(rooms, types, 'floor');

		expect(groups.map((g) => g.label)).toEqual(['Floor 1', 'Floor 2', 'No floor']);
	});
});

describe('rangeNumbers', () => {
	it('lists the numbers a range will create', () => {
		expect(rangeNumbers('', 101, 104)).toEqual(['101', '102', '103', '104']);
		expect(rangeNumbers('A', 1, 2)).toEqual(['A1', 'A2']);
	});

	it('is empty for a backwards range', () => {
		expect(rangeNumbers('', 5, 4)).toEqual([]);
	});
});

describe('query keys', () => {
	it('match the server event keys', () => {
		expect(roomTypesKey('p1')).toEqual(['room-types:p1']);
		expect(roomsKey('p1')).toEqual(['rooms:p1']);
	});
});
```

Modify `web/pms/src/lib/session.spec.ts`:

```diff
--- a/web/pms/src/lib/session.spec.ts
+++ b/web/pms/src/lib/session.spec.ts
@@ -1,5 +1,5 @@
 import { describe, expect, it } from 'vitest';
-import { isTenantOwner, type Profile } from './session';
+import { can, isTenantOwner, type Profile } from './session';
 
 function profile(grants: Profile['grants']): Profile {
 	return { user_id: 'u', email: 'a@b.lk', display_name: 'A', tenants: [], grants };
@@ -12,3 +12,17 @@ describe('isTenantOwner', () => {
 		expect(isTenantOwner(profile([{ role: 'manager' }]))).toBe(false);
 	});
 });
+
+describe('can', () => {
+	it('lets owners and managers manage rooms, and front desk block them', () => {
+		const manager = profile([{ role: 'manager', property_id: 'p1' }]);
+		const desk = profile([{ role: 'front_desk' }]);
+		const housekeeping = profile([{ role: 'housekeeping' }]);
+
+		expect(can(manager, 'manageRooms', 'p1')).toBe(true);
+		expect(can(manager, 'manageRooms', 'p2')).toBe(false);
+		expect(can(desk, 'manageRooms', 'p1')).toBe(false);
+		expect(can(desk, 'blockRooms', 'p1')).toBe(true);
+		expect(can(housekeeping, 'blockRooms', 'p1')).toBe(false);
+	});
+});
```

- [ ] **Step 2: Run the tests to verify they fail**

```sh
cd web/pms && bun run test
```

Expected: FAIL: `grid.spec.ts`, `inventory.spec.ts` and `rooms.spec.ts` cannot import their modules; `rest.spec.ts` and `session.spec.ts` fail on the missing `unwrap`, `ifMatch` and `can`.

- [ ] **Step 3: Implement**

Then run `bun run codegen`, which generates the typed documents for the new `graphql(...)` queries.

Modify `web/pms/src/lib/api/rest.ts`:

```diff
--- a/web/pms/src/lib/api/rest.ts
+++ b/web/pms/src/lib/api/rest.ts
@@ -1,5 +1,6 @@
 import createClient from 'openapi-fetch';
 import type { paths } from './openapi';
+import { toApiError } from './problem';
 
 /** REST client for commands. Same origin, so the session cookie is sent automatically. */
 export const rest = createClient<paths>({
@@ -11,3 +12,14 @@ export const rest = createClient<paths>({
 export function idempotencyKey(): string {
 	return crypto.randomUUID();
 }
+
+/** The data of an openapi-fetch result, or its problem thrown as an ApiError. */
+export function unwrap<T>(result: { data?: T; error?: unknown; response: Response }): T {
+	if (!result.response.ok) throw toApiError(result.error, result.response.status);
+	return result.data as T;
+}
+
+/** The `If-Match` header for an update of the resource at `version`. */
+export function ifMatch(version: number) {
+	return { 'If-Match': `"${version}"` };
+}
```

Create `web/pms/src/lib/grid.ts`:

```ts
/**
 * Layout math for horizontally virtualized date grids: the inventory calendar now, the tape chart later.
 * Only the columns returned by `visibleColumns` are rendered as DOM nodes.
 */

/** Columns `[start, end)` to render. */
export interface ColumnWindow {
	start: number;
	end: number;
}

/** The columns intersecting the viewport, plus `overscan` columns on each side. */
export function visibleColumns(
	scrollLeft: number,
	viewportWidth: number,
	columnWidth: number,
	columnCount: number,
	overscan = 2
): ColumnWindow {
	const first = Math.floor(scrollLeft / columnWidth);
	const last = Math.ceil((scrollLeft + viewportWidth) / columnWidth);
	return {
		start: Math.max(0, Math.min(columnCount, first - overscan)),
		end: Math.max(0, Math.min(columnCount, last + overscan))
	};
}

/** The scroll position that shows `column` with as little movement as possible. */
export function revealColumn(
	column: number,
	scrollLeft: number,
	viewportWidth: number,
	columnWidth: number
): number {
	const left = column * columnWidth;
	const right = left + columnWidth;
	if (left < scrollLeft) return left;
	if (right > scrollLeft + viewportWidth) return right - viewportWidth;
	return scrollLeft;
}

export interface Cell {
	row: number;
	column: number;
}

/** The cell a navigation key moves to, or `null` if the key does not navigate. */
export function moveFocus(
	cell: Cell,
	key: string,
	size: { rows: number; columns: number }
): Cell | null {
	const clamp = (value: number, count: number) => Math.max(0, Math.min(count - 1, value));
	switch (key) {
		case 'ArrowLeft':
			return { ...cell, column: clamp(cell.column - 1, size.columns) };
		case 'ArrowRight':
			return { ...cell, column: clamp(cell.column + 1, size.columns) };
		case 'ArrowUp':
			return { ...cell, row: clamp(cell.row - 1, size.rows) };
		case 'ArrowDown':
			return { ...cell, row: clamp(cell.row + 1, size.rows) };
		case 'Home':
			return { ...cell, column: 0 };
		case 'End':
			return { ...cell, column: size.columns - 1 };
		default:
			return null;
	}
}
```

Create `web/pms/src/lib/inventory.ts`:

```ts
import { graphql } from './api/gql';
import type { InventoryQuery } from './api/gql/graphql';
import { query } from './api/graphql';

export const InventoryDocument = graphql(`
	query Inventory($propertyId: UUID!, $from: Date!, $to: Date!) {
		inventory(propertyId: $propertyId, from: $from, to: $to) {
			date
			roomTypeId
			physical
			sold
			outOfOrder
			available
		}
		blocks(propertyId: $propertyId, from: $from, to: $to) {
			id
			roomId
			from
			to
			kind
			reasonId
			note
			version
		}
	}
`);

export type InventoryDay = InventoryQuery['inventory'][number];
export type Block = InventoryQuery['blocks'][number];

/** Query key shared with the server's `inventory:<property>:<yyyy-mm>` event. */
export function inventoryKey(propertyId: string, month: string) {
	return [`inventory:${propertyId}:${month}`] as const;
}

/** A month's counts and blocks. `month` is `YYYY-MM`. */
export async function fetchMonth(propertyId: string, month: string, signal?: AbortSignal) {
	const days = monthDays(month);
	return query(
		InventoryDocument,
		{ propertyId, from: days[0], to: addDays(days[days.length - 1], 1) },
		signal
	);
}

/** `YYYY-MM-DD` plus `days`, in calendar days (no time zones involved). */
export function addDays(date: string, days: number): string {
	const moved = new Date(`${date}T00:00:00Z`);
	moved.setUTCDate(moved.getUTCDate() + days);
	return moved.toISOString().slice(0, 10);
}

/** The month (`YYYY-MM`) a date is in. */
export function monthOf(date: string): string {
	return date.slice(0, 7);
}

/** `YYYY-MM` moved by `delta` months. */
export function shiftMonth(month: string, delta: number): string {
	const [year, index] = month.split('-').map(Number);
	const moved = new Date(Date.UTC(year, index - 1 + delta, 1));
	return moved.toISOString().slice(0, 7);
}

/** Every day of `month` as `YYYY-MM-DD`. */
export function monthDays(month: string): string[] {
	const days: string[] = [];
	for (let day = `${month}-01`; monthOf(day) === month; day = addDays(day, 1)) days.push(day);
	return days;
}

/** Looks up a room type's counts on a day. */
export function indexInventory(rows: InventoryDay[]) {
	const byKey = new Map(rows.map((row) => [`${row.roomTypeId}|${row.date}`, row]));
	return {
		get: (roomTypeId: string, date: string) => byKey.get(`${roomTypeId}|${date}`)
	};
}

/** Blocks covering `date` (from its first day up to, not including, `to`), optionally only for `rooms`. */
export function blocksOn(blocks: Block[], date: string, rooms?: Set<string>): Block[] {
	return blocks.filter(
		(block) => block.from <= date && date < block.to && (!rooms || rooms.has(block.roomId))
	);
}
```

Create `web/pms/src/lib/rooms.ts`:

```ts
import { graphql } from './api/gql';
import type { RoomsQuery, RoomTypesQuery } from './api/gql/graphql';
import { query } from './api/graphql';

export const RoomTypesDocument = graphql(`
	query RoomTypes($propertyId: UUID!) {
		roomTypes(propertyId: $propertyId) {
			id
			code
			name
			baseOccupancy
			maxAdults
			maxChildren
			maxOccupancy
			amenities
			sortOrder
			active
			version
		}
	}
`);

export const RoomsDocument = graphql(`
	query Rooms($propertyId: UUID!) {
		rooms(propertyId: $propertyId) {
			id
			roomTypeId
			number
			floor
			sectionId
			active
			sortOrder
			version
		}
		sections(propertyId: $propertyId) {
			id
			name
			version
		}
		blockReasons(propertyId: $propertyId) {
			id
			code
			label
			defaultKind
			active
		}
	}
`);

export type RoomType = RoomTypesQuery['roomTypes'][number];
export type Room = RoomsQuery['rooms'][number];
export type Section = RoomsQuery['sections'][number];
export type BlockReason = RoomsQuery['blockReasons'][number];

/** Query keys shared with the server's events, so an `invalidate` refetches exactly these. */
export function roomTypesKey(propertyId: string) {
	return [`room-types:${propertyId}`] as const;
}

/** Rooms, sections and block reasons change together under the `rooms:<property>` event. */
export function roomsKey(propertyId: string) {
	return [`rooms:${propertyId}`] as const;
}

export async function fetchRoomTypes(propertyId: string, signal?: AbortSignal) {
	return (await query(RoomTypesDocument, { propertyId }, signal)).roomTypes;
}

export async function fetchRooms(propertyId: string, signal?: AbortSignal) {
	return query(RoomsDocument, { propertyId }, signal);
}

/** `items` with the one at `from` moved to `to`; unchanged if either is out of range. */
export function moveItem<T>(items: readonly T[], from: number, to: number): T[] {
	const moved = [...items];
	if (from < 0 || from >= items.length || to < 0 || to >= items.length) return moved;
	const [item] = moved.splice(from, 1);
	moved.splice(to, 0, item);
	return moved;
}

export interface RoomGroup {
	key: string;
	label: string;
	rooms: Room[];
}

/** Rooms grouped by type (in the types' display order) or by floor (rooms without a floor last). */
export function groupRooms(
	rooms: readonly Room[],
	roomTypes: readonly Pick<RoomType, 'id' | 'code' | 'name'>[],
	by: 'type' | 'floor'
): RoomGroup[] {
	if (by === 'type') {
		return roomTypes
			.map((type) => ({
				key: type.id,
				label: `${type.code} · ${type.name}`,
				rooms: rooms.filter((room) => room.roomTypeId === type.id)
			}))
			.filter((group) => group.rooms.length > 0);
	}
	const floors = [...new Set(rooms.map((room) => room.floor ?? ''))].sort((a, b) =>
		a === '' ? 1 : b === '' ? -1 : a.localeCompare(b, undefined, { numeric: true })
	);
	return floors.map((floor) => ({
		key: floor,
		label: floor ? `Floor ${floor}` : 'No floor',
		rooms: rooms.filter((room) => (room.floor ?? '') === floor)
	}));
}

/** The room numbers a bulk range creates: `prefix` + each number from `first` to `last`. */
export function rangeNumbers(prefix: string, first: number, last: number): string[] {
	const numbers: string[] = [];
	for (let n = first; n <= last; n++) numbers.push(`${prefix}${n}`);
	return numbers;
}
```

Modify `web/pms/src/lib/session.ts`:

```diff
--- a/web/pms/src/lib/session.ts
+++ b/web/pms/src/lib/session.ts
@@ -14,3 +14,20 @@ export async function fetchMe(): Promise<Profile> {
 export function isTenantOwner(profile: Profile): boolean {
 	return profile.grants.some((grant) => grant.role === 'owner' && grant.property_id == null);
 }
+
+type Role = Profile['grants'][number]['role'];
+
+/** Roles allowed each action, mirroring `identity::Permission` on the server. */
+const ACTIONS = {
+	manageRooms: ['owner', 'manager'],
+	blockRooms: ['owner', 'manager', 'front_desk']
+} satisfies Record<string, Role[]>;
+
+/** UI hint only; the API enforces permissions. A grant counts tenant-wide or for `propertyId`. */
+export function can(profile: Profile, action: keyof typeof ACTIONS, propertyId: string): boolean {
+	const roles: Role[] = ACTIONS[action];
+	return profile.grants.some(
+		(grant) =>
+			roles.includes(grant.role) && (grant.property_id == null || grant.property_id === propertyId)
+	);
+}
```

```sh
cd web/pms && bun run codegen
```

- [ ] **Step 4: Run the tests to verify they pass**

```sh
cd web/pms
bun run lint && bun run check && bun run test && bun run build
```

Expected: PASS: 35 tests in 8 files; lint, svelte-check and build clean.

- [ ] **Step 5: Commit**

```sh
git add -A
git commit -m "feat(web): room, inventory and grid data layer with query keys matching server events"
```

### Task 14: Playwright end-to-end tests, locally and in CI

The end-to-end harness. `playwright.config.ts` starts the API (`cargo run -p core-api` on port 18080, as `goodfolk_api` against `E2E_DATABASE_URL`) and the production build (`vite preview` on port 4173, proxying `/api` and `/graphql` like `vite dev`), and refuses to run without a database. Chromium's sandbox is on unless `PLAYWRIGHT_NO_SANDBOX=1`, which the config rejects under `CI`. The first two tests sign up, sign out and sign back in, and lock an email after five wrong passwords; they cover existing behaviour, so they pass once the harness runs. CI gains an `e2e` job against a Postgres service; Ubuntu 24.04 blocks the unprivileged user namespaces Chromium's sandbox needs, so the job allows them rather than disabling the sandbox.

**Files:**
- Create: `web/pms/playwright.config.ts`
- Modify: `.github/workflows/ci.yml`
- Modify: `README.md`
- Modify: `web/pms/.gitignore`
- Modify: `web/pms/package.json`
- Modify: `web/pms/vite.config.ts`
- Test: `web/pms/tests/e2e/auth.spec.ts` (new)
- Test: `web/pms/tests/e2e/helpers.ts` (new)
- Regenerated, not shown: `web/pms/bun.lock`

**Interfaces:**
- Consumes: the sign-up, sign-in and property screens from Phase 0; Task 4's throttling.
- Produces: `bun run test:e2e`; `tests/e2e/helpers.ts`: `PASSWORD`, `signUp(page) -> { email }`, `createProperty(page, code)`; env vars `E2E_DATABASE_URL`, `PLAYWRIGHT_NO_SANDBOX`.

- [ ] **Step 1: Add Playwright and its configuration**

Install the pinned package and Chromium (it is cached in `~/.cache/ms-playwright`):

```sh
cd web/pms
bun add -d --exact @playwright/test@1.63.0
bunx playwright install chromium
```

Then add the script, ignore Playwright's output directories, give `vite preview` the dev proxy, and create the config.

Modify `web/pms/package.json`:

```diff
--- a/web/pms/package.json
+++ b/web/pms/package.json
@@ -15,12 +15,14 @@
 		"test:unit": "vitest",
 		"test": "npm run test:unit -- --run",
 		"api:schemas": "cd ../.. && cargo run -q -p core-api --bin export-schemas -- web/pms/src/lib/api",
-		"codegen": "openapi-typescript src/lib/api/openapi.json -o src/lib/api/openapi.d.ts && graphql-codegen"
+		"codegen": "openapi-typescript src/lib/api/openapi.json -o src/lib/api/openapi.d.ts && graphql-codegen",
+		"test:e2e": "playwright test"
 	},
 	"devDependencies": {
 		"@eslint/js": "^10.0.1",
 		"@graphql-codegen/cli": "7.4.2",
 		"@graphql-codegen/client-preset": "6.2.0",
+		"@playwright/test": "1.63.0",
 		"@sveltejs/adapter-static": "^3.0.10",
 		"@sveltejs/kit": "^2.63.0",
 		"@sveltejs/vite-plugin-svelte": "^7.1.2",
```

Modify `web/pms/.gitignore`:

```diff
--- a/web/pms/.gitignore
+++ b/web/pms/.gitignore
@@ -1,6 +1,9 @@
 node_modules
 
 # Output
+/test-results
+/playwright-report
+/blob-report
 .output
 .vercel
 .netlify
```

Modify `web/pms/vite.config.ts`:

```diff
--- a/web/pms/vite.config.ts
+++ b/web/pms/vite.config.ts
@@ -22,6 +22,13 @@ export default defineConfig({
 			'/graphql': api
 		}
 	},
+	// `vite preview` serves the production build the same way (end-to-end tests use it).
+	preview: {
+		proxy: {
+			'/api': api,
+			'/graphql': api
+		}
+	},
 	test: {
 		expect: { requireAssertions: true },
 		projects: [
```

Create `web/pms/playwright.config.ts`:

```ts
import { defineConfig, devices } from '@playwright/test';

// End-to-end tests drive the built SPA against a real API and Postgres, all on localhost.
// E2E_DATABASE_URL is a migrated database, as the API role (see README "End-to-end tests").
const database = process.env.E2E_DATABASE_URL;
if (!database) throw new Error('set E2E_DATABASE_URL to a migrated database, as goodfolk_api');

// Chromium's sandbox stays on, as in CI. PLAYWRIGHT_NO_SANDBOX=1 turns it off for local machines that
// cannot start it (no unprivileged user namespaces); the suite only ever loads localhost pages.
const noSandbox = process.env.PLAYWRIGHT_NO_SANDBOX === '1';
if (noSandbox && process.env.CI) throw new Error('PLAYWRIGHT_NO_SANDBOX is for local runs only');

const API_PORT = 18080;
const WEB_PORT = 4173;

export default defineConfig({
	testDir: 'tests/e2e',
	fullyParallel: true,
	forbidOnly: !!process.env.CI,
	retries: process.env.CI ? 1 : 0,
	reporter: process.env.CI ? [['github'], ['list']] : 'list',
	use: {
		baseURL: `http://localhost:${WEB_PORT}`,
		trace: 'retain-on-failure'
	},
	projects: [
		{
			name: 'chromium',
			use: { ...devices['Desktop Chrome'], launchOptions: { chromiumSandbox: !noSandbox } }
		}
	],
	webServer: [
		{
			command: 'cargo run -q -p core-api',
			cwd: '../..',
			url: `http://localhost:${API_PORT}/readyz`,
			env: { PORT: String(API_PORT), DATABASE_URL: database, RUST_LOG: 'warn' },
			reuseExistingServer: !process.env.CI,
			timeout: 600_000
		},
		{
			command: `bun run build && bun run preview --port ${WEB_PORT} --strictPort`,
			url: `http://localhost:${WEB_PORT}`,
			env: { GOODFOLK_API: `http://localhost:${API_PORT}` },
			reuseExistingServer: !process.env.CI,
			timeout: 180_000
		}
	]
});
```

- [ ] **Step 2: Create a database for the tests**

Use a database of its own, so test accounts never mix with development data (any Postgres client works; `psql` shown):

```sh
psql postgres://goodfolk_owner:goodfolk_owner_dev@localhost:5432/goodfolk -c 'create database goodfolk_e2e'
DATABASE_OWNER_URL=postgres://goodfolk_owner:goodfolk_owner_dev@localhost:5432/goodfolk_e2e cargo run -p core-api -- migrate
export E2E_DATABASE_URL=postgres://goodfolk_api:goodfolk_api_dev@localhost:5432/goodfolk_e2e
```

- [ ] **Step 3: Write the tests**

Create `web/pms/tests/e2e/helpers.ts`:

```ts
import { expect, type Page } from '@playwright/test';

export const PASSWORD = 'a long enough password';

/** Signs up a new owner with a unique email and waits for the property list. */
export async function signUp(page: Page): Promise<{ email: string }> {
	const email = `e2e-${Date.now()}-${Math.random().toString(36).slice(2, 8)}@example.com`;
	await page.goto('/signup');
	await page.getByLabel('Your name').fill('Nimal Perera');
	await page.getByLabel('Hotel or group name').fill('Lagoon Hotels');
	await page.getByLabel('Email').fill(email);
	await page.getByLabel('Password').fill(PASSWORD);
	await page.getByRole('button', { name: 'Create account' }).click();
	await expect(page.getByRole('heading', { name: 'Properties' })).toBeVisible();
	return { email };
}

/** Creates a property and waits for its page. */
export async function createProperty(page: Page, code: string): Promise<void> {
	await page.getByRole('link', { name: 'Add a property' }).click();
	await page.getByLabel('Code').fill(code);
	await page.getByLabel('Name').fill(`Hotel ${code}`);
	await page.getByRole('button', { name: 'Create property' }).click();
	await expect(page.getByRole('heading', { name: `Hotel ${code}` })).toBeVisible();
}
```

Create `web/pms/tests/e2e/auth.spec.ts`:

```ts
import { expect, test } from '@playwright/test';
import { PASSWORD, signUp } from './helpers';

test('a new user signs up, signs out and signs back in', async ({ page }) => {
	const { email } = await signUp(page);

	await page.getByRole('button', { name: 'Sign out' }).click();
	await expect(page).toHaveURL(/\/login$/);
	await page.getByLabel('Email').fill(email);
	await page.getByLabel('Password').fill(PASSWORD);
	await page.getByRole('button', { name: 'Sign in' }).click();

	await expect(page.getByRole('heading', { name: 'Properties' })).toBeVisible();
});

test('repeated wrong passwords lock sign-in for the email', async ({ page }) => {
	const { email } = await signUp(page);
	await page.getByRole('button', { name: 'Sign out' }).click();

	for (let attempt = 1; attempt <= 5; attempt++) {
		await page.getByLabel('Email').fill(email);
		await page.getByLabel('Password').fill('not the password');
		await page.getByRole('button', { name: 'Sign in' }).click();
		await expect(page.getByRole('alert')).toHaveText('Invalid email or password');
	}
	await page.getByLabel('Password').fill(PASSWORD);
	await page.getByRole('button', { name: 'Sign in' }).click();

	await expect(page.getByRole('alert')).toContainText('too many failed sign-in attempts');
});
```

- [ ] **Step 4: Run them**

```sh
cd web/pms && bun run test:e2e
```

Expected: PASS: 2 tests. On a machine that cannot start Chromium's sandbox the tests fail at launch with `No usable sandbox!`; run `PLAYWRIGHT_NO_SANDBOX=1 bun run test:e2e` there (local only). Playwright stops both servers when it finishes.

- [ ] **Step 5: Run them in CI and document the setup**

Modify `.github/workflows/ci.yml`:

```diff
--- a/.github/workflows/ci.yml
+++ b/.github/workflows/ci.yml
@@ -55,3 +55,49 @@ jobs:
       - run: bun run check
       - run: bun run test
       - run: bun run build
+
+  e2e:
+    runs-on: ubuntu-latest
+    services:
+      postgres:
+        image: postgres:17
+        env:
+          POSTGRES_USER: goodfolk_owner
+          POSTGRES_PASSWORD: goodfolk_owner_dev
+          POSTGRES_DB: goodfolk
+        ports: ["5432:5432"]
+        options: >-
+          --health-cmd "pg_isready -U goodfolk_owner"
+          --health-interval 2s --health-timeout 5s --health-retries 30
+    env:
+      DATABASE_OWNER_URL: postgres://goodfolk_owner:goodfolk_owner_dev@localhost:5432/goodfolk
+      # Playwright starts the API as the API role against this database.
+      E2E_DATABASE_URL: postgres://goodfolk_api:goodfolk_api_dev@localhost:5432/goodfolk
+    defaults:
+      run:
+        working-directory: web/pms
+    steps:
+      - uses: actions/checkout@v5
+      - run: rustup show
+      - uses: Swatinem/rust-cache@v2
+      - uses: oven-sh/setup-bun@v2
+        with:
+          bun-version: 1.3.14
+      - name: Create database roles and apply migrations
+        run: |
+          psql "$DATABASE_OWNER_URL" -f ../../deploy/dev/postgres-init.sql
+          cargo run -q -p core-api -- migrate
+      - name: Build the API that Playwright starts
+        run: cargo build -p core-api
+      - run: bun install --frozen-lockfile
+      # Ubuntu 24.04 blocks the unprivileged user namespaces Chromium's sandbox needs; allow them so the
+      # sandbox stays on.
+      - name: Allow Chromium's sandbox
+        run: sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0
+      - run: bunx playwright install --with-deps chromium
+      - run: bun run test:e2e
+      - uses: actions/upload-artifact@v4
+        if: failure()
+        with:
+          name: playwright-traces
+          path: web/pms/test-results
```

Modify `README.md`:

````diff
--- a/README.md
+++ b/README.md
@@ -43,6 +43,22 @@ bun run api:schemas && bun run codegen   # after any API change; commit the resu
 bun run lint && bun run check && bun run test && bun run build
 ```
 
+### End-to-end tests
+
+Playwright (`web/pms/tests/e2e`) drives the production build against a real API and Postgres. It starts both itself: the API on port 18080 (`cargo run -p core-api`) and `vite preview` on port 4173. Give it a migrated database of its own, so test accounts never land in your development data:
+
+```sh
+# once: create and migrate a database for the tests (psql, or any Postgres client)
+psql "$DATABASE_OWNER_URL" -c 'create database goodfolk_e2e'
+DATABASE_OWNER_URL=postgres://goodfolk_owner:goodfolk_owner_dev@localhost:5432/goodfolk_e2e cargo run -p core-api -- migrate
+
+cd web/pms
+bunx playwright install chromium   # once
+E2E_DATABASE_URL=postgres://goodfolk_api:goodfolk_api_dev@localhost:5432/goodfolk_e2e bun run test:e2e
+```
+
+Chromium runs with its sandbox on, as in CI. If your machine cannot start it ("No usable sandbox!", for example on distributions that restrict unprivileged user namespaces), add `PLAYWRIGHT_NO_SANDBOX=1` for local runs; the config refuses it when `CI` is set.
+
 ### Configuration
 
 The API (`core-api serve`) reads:
````

- [ ] **Step 6: Run the web checks**

```sh
cd web/pms
bun run lint && bun run check && bun run test && bun run build
```

Expected: clean (svelte-check now also type-checks `tests/e2e`).

- [ ] **Step 7: Commit**

```sh
git add -A
git commit -m "test(web): Playwright end-to-end tests for sign-up, sign-in and throttling, run in CI against a real API"
```

### Task 15: Room types and rooms screens

Two screens under a property sub-navigation. **Room types**: a table with inline edit (name and capacities), drag-and-drop reordering with ↑/↓ buttons as the keyboard alternative, activate/deactivate, and an add form. **Rooms**: grouped by room type or floor, with inline type, floor and section changes, activate/deactivate, move up/down, an add-one form, a section form, and the bulk-add dialog ("101–120, type DLX, floor 1") with a live preview. Every command shows its problem detail; after a 412 the screen refetches, so the user sees what changed. The property overview shows the business date and check-in/out times. Controls to change things appear only for roles that may (`can`), while the API enforces it.

**Files:**
- Create: `web/pms/src/routes/(app)/p/[property]/+layout.svelte`
- Create: `web/pms/src/routes/(app)/p/[property]/room-types/+page.svelte`
- Create: `web/pms/src/routes/(app)/p/[property]/rooms/+page.svelte`
- Modify: `web/pms/src/app.css`
- Modify: `web/pms/src/lib/properties.ts`
- Modify: `web/pms/src/routes/(app)/p/[property]/+page.svelte`
- Test: `web/pms/tests/e2e/rooms.spec.ts` (new)
- Test: `web/pms/tests/e2e/helpers.ts`
- Regenerated, not shown: `web/pms/src/lib/api/gql/gql.ts`, `web/pms/src/lib/api/gql/graphql.ts`

**Interfaces:**
- Consumes: Task 13's `rooms.ts`, `session.can`, `rest.{unwrap, ifMatch, idempotencyKey}`.
- Produces: routes `/p/[property]/room-types`, `/p/[property]/rooms`, the property layout's navigation; e2e helpers `addRoomType(page, code, name)`, `addRooms(page, code, first, last)`; accessible names the tests rely on (`Move DLX up`, `Edit DLX`, `Name of DLX`, `Save DLX`, `Add rooms…`, dialog `Add rooms`, `Deactivate room 105`).

- [ ] **Step 1: Write the failing end-to-end test**

Modify `web/pms/tests/e2e/helpers.ts`:

```diff
--- a/web/pms/tests/e2e/helpers.ts
+++ b/web/pms/tests/e2e/helpers.ts
@@ -23,3 +23,26 @@ export async function createProperty(page: Page, code: string): Promise<void> {
 	await page.getByRole('button', { name: 'Create property' }).click();
 	await expect(page.getByRole('heading', { name: `Hotel ${code}` })).toBeVisible();
 }
+
+/** Adds a room type on the Room types page. */
+export async function addRoomType(page: Page, code: string, name: string): Promise<void> {
+	const form = page.getByRole('form', { name: 'New room type' });
+	await form.getByLabel('Code').fill(code);
+	await form.getByLabel('Name').fill(name);
+	await form.getByRole('button', { name: 'Add room type' }).click();
+	await expect(page.getByRole('cell', { name: code, exact: true })).toBeVisible();
+}
+
+/** Adds rooms `first` to `last` of one type with the bulk dialog on the Rooms page. */
+export async function addRooms(page: Page, code: string, first: number, last: number) {
+	await page.getByRole('button', { name: 'Add rooms…' }).click();
+	const dialog = page.getByRole('dialog', { name: 'Add rooms' });
+	await dialog.getByLabel('Room type').selectOption({ label: code });
+	await dialog.getByLabel('First number').fill(String(first));
+	await dialog.getByLabel('Last number').fill(String(last));
+	await dialog.getByLabel('Floor').fill(String(first).slice(0, 1));
+	const count = last - first + 1;
+	await dialog.getByRole('button', { name: `Add ${count} rooms` }).click();
+	await expect(dialog).toBeHidden();
+	await expect(page.getByRole('cell', { name: String(last), exact: true })).toBeVisible();
+}
```

Create `web/pms/tests/e2e/rooms.spec.ts`:

```ts
import { expect, test } from '@playwright/test';
import { addRooms, addRoomType, createProperty, signUp } from './helpers';

test('an owner sets up room types and rooms', async ({ page }) => {
	await signUp(page);
	await createProperty(page, 'GAL');

	await page.getByRole('link', { name: 'Room types' }).click();
	await addRoomType(page, 'STD', 'Standard');
	await addRoomType(page, 'DLX', 'Deluxe');
	await page.getByRole('button', { name: 'Move DLX up' }).click();
	await expect(page.getByRole('row').nth(1)).toContainText('DLX');
	await page.getByRole('button', { name: 'Edit DLX' }).click();
	await page.getByLabel('Name of DLX').fill('Deluxe Sea View');
	await page.getByRole('button', { name: 'Save DLX' }).click();
	await expect(page.getByRole('cell', { name: 'Deluxe Sea View' })).toBeVisible();

	await page.getByRole('link', { name: 'Rooms', exact: true }).click();
	await addRooms(page, 'DLX', 101, 105);
	await addRooms(page, 'STD', 201, 202);
	await expect(page.getByRole('heading', { name: 'DLX · Deluxe Sea View' })).toBeVisible();

	await page.getByLabel('Group by').selectOption('floor');
	await expect(page.getByRole('heading', { name: 'Floor 1' })).toBeVisible();
	await expect(page.getByRole('heading', { name: 'Floor 2' })).toBeVisible();

	await page.getByRole('button', { name: 'Deactivate room 105' }).click();
	await expect(page.getByRole('button', { name: 'Activate room 105' })).toBeVisible();
});
```

- [ ] **Step 2: Run it to verify it fails**

```sh
cd web/pms && bun run test:e2e rooms
```

Expected: FAIL: `an owner sets up room types and rooms` times out waiting for the `Room types` link.

- [ ] **Step 3: Implement**

Then run `bun run codegen` for the changed `Properties` document.

Modify `web/pms/src/lib/properties.ts`:

```diff
--- a/web/pms/src/lib/properties.ts
+++ b/web/pms/src/lib/properties.ts
@@ -9,6 +9,10 @@ export const PropertiesDocument = graphql(`
 			name
 			timezone
 			baseCurrency
+			businessDate
+			checkInTime
+			checkOutTime
+			version
 		}
 	}
 `);
```

Modify `web/pms/src/app.css`:

```diff
--- a/web/pms/src/app.css
+++ b/web/pms/src/app.css
@@ -77,3 +77,65 @@ label {
 .error {
 	color: var(--danger);
 }
+
+button.secondary {
+	background: var(--bg);
+	border-color: var(--border);
+	color: var(--text);
+}
+
+table {
+	border-collapse: collapse;
+	margin: var(--space) 0;
+}
+
+th,
+td {
+	text-align: left;
+	padding: 0.35rem 0.6rem;
+	border-bottom: 1px solid var(--border);
+}
+
+tr.inactive td {
+	color: var(--muted);
+}
+
+td input {
+	width: 6rem;
+}
+
+.actions {
+	display: flex;
+	gap: 0.25rem;
+}
+
+.inline-form {
+	display: flex;
+	flex-wrap: wrap;
+	align-items: end;
+	gap: var(--space);
+}
+
+.inline-form input[type='number'] {
+	width: 5rem;
+}
+
+.hint {
+	color: var(--muted);
+}
+
+.visually-hidden {
+	position: absolute;
+	width: 1px;
+	height: 1px;
+	overflow: hidden;
+	clip-path: inset(50%);
+	white-space: nowrap;
+}
+
+dialog {
+	border: 1px solid var(--border);
+	border-radius: var(--radius);
+	background: var(--bg);
+	color: var(--text);
+}
```

Create `web/pms/src/routes/(app)/p/[property]/+layout.svelte`:

```svelte
<script lang="ts">
	import { resolve } from '$app/paths';
	import { page } from '$app/state';

	let { children } = $props();

	const property = $derived(page.params.property ?? '');
	const links = $derived([
		{ href: resolve('/(app)/p/[property]', { property }), label: 'Overview' },
		{ href: resolve('/(app)/p/[property]/room-types', { property }), label: 'Room types' },
		{ href: resolve('/(app)/p/[property]/rooms', { property }), label: 'Rooms' }
	]);
</script>

<nav class="property-nav" aria-label="Property">
	{#each links as link (link.href)}
		<a href={link.href} aria-current={page.url.pathname === link.href ? 'page' : undefined}
			>{link.label}</a
		>
	{/each}
</nav>
{@render children()}

<style>
	.property-nav {
		display: flex;
		gap: var(--space);
		margin-bottom: var(--space);
	}
	.property-nav a[aria-current='page'] {
		font-weight: 600;
	}
</style>
```

Modify `web/pms/src/routes/(app)/p/[property]/+page.svelte`:

```diff
--- a/web/pms/src/routes/(app)/p/[property]/+page.svelte
+++ b/web/pms/src/routes/(app)/p/[property]/+page.svelte
@@ -19,6 +19,10 @@
 		<dd>{property.timezone}</dd>
 		<dt>Base currency</dt>
 		<dd>{property.baseCurrency}</dd>
+		<dt>Business date</dt>
+		<dd>{property.businessDate}</dd>
+		<dt>Check-in / check-out</dt>
+		<dd>{property.checkInTime} / {property.checkOutTime}</dd>
 	</dl>
 {:else if properties.isSuccess}
 	<p>Property not found.</p>
```

Create `web/pms/src/routes/(app)/p/[property]/room-types/+page.svelte`:

```svelte
<script lang="ts">
	import { page } from '$app/state';
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { ApiError, errorMessage } from '$lib/api/problem';
	import { idempotencyKey, ifMatch, rest, unwrap } from '$lib/api/rest';
	import { fetchRoomTypes, moveItem, roomTypesKey, type RoomType } from '$lib/rooms';
	import { can, fetchMe } from '$lib/session';

	const propertyId = $derived(page.params.property ?? '');
	const client = useQueryClient();
	const me = createQuery(() => ({ queryKey: ['me'], queryFn: fetchMe }));
	const roomTypes = createQuery(() => ({
		queryKey: roomTypesKey(propertyId),
		queryFn: ({ signal }) => fetchRoomTypes(propertyId, signal)
	}));
	const manage = $derived(!!me.data && can(me.data, 'manageRooms', propertyId));

	type Capacity = Pick<RoomType, 'baseOccupancy' | 'maxAdults' | 'maxChildren' | 'maxOccupancy'>;
	const emptyDraft = () => ({
		code: '',
		name: '',
		baseOccupancy: 2,
		maxAdults: 2,
		maxChildren: 0,
		maxOccupancy: 2
	});
	let draft = $state(emptyDraft());
	let editing = $state<({ id: string; version: number; name: string } & Capacity) | null>(null);
	let dragged = $state<number | null>(null);
	let error = $state('');
	let busy = $state(false);
	// Reused for retries of the same form; a new one after a success or an edit of the form.
	let createKey = idempotencyKey();
	let lastBody = '';

	function capacity(values: Capacity) {
		return {
			base_occupancy: values.baseOccupancy,
			max_adults: values.maxAdults,
			max_children: values.maxChildren,
			max_occupancy: values.maxOccupancy
		};
	}

	/** Runs a command, shows its problem if it fails, and refetches after a version conflict. */
	async function run(command: () => Promise<unknown>) {
		busy = true;
		error = '';
		try {
			await command();
			await client.invalidateQueries({ queryKey: roomTypesKey(propertyId) });
		} catch (err) {
			error = errorMessage(err);
			if (err instanceof ApiError && err.status === 412) {
				editing = null;
				await client.invalidateQueries({ queryKey: roomTypesKey(propertyId) });
			}
		} finally {
			busy = false;
		}
	}

	function create(event: SubmitEvent) {
		event.preventDefault();
		const body = { code: draft.code.toUpperCase(), name: draft.name, ...capacity(draft) };
		const serialized = JSON.stringify(body);
		if (lastBody && serialized !== lastBody) createKey = idempotencyKey();
		lastBody = serialized;
		return run(async () => {
			unwrap(
				await rest.POST('/api/v1/properties/{property}/room-types', {
					params: { path: { property: propertyId }, header: { 'Idempotency-Key': createKey } },
					body
				})
			);
			draft = emptyDraft();
			createKey = idempotencyKey();
			lastBody = '';
		});
	}

	function update(type: RoomType, body: { name?: string; active?: boolean } & object) {
		return run(async () => {
			unwrap(
				await rest.PATCH('/api/v1/properties/{property}/room-types/{room_type}', {
					params: {
						path: { property: propertyId, room_type: type.id },
						header: ifMatch(type.version)
					},
					body
				})
			);
			editing = null;
		});
	}

	function reorder(from: number, to: number) {
		const current = roomTypes.data ?? [];
		const moved = moveItem(current, from, to);
		if (moved.every((type, index) => type.id === current[index].id)) return;
		// Show the new order at once; the refetch after the command confirms it.
		client.setQueryData(roomTypesKey(propertyId), moved);
		return run(async () => {
			unwrap(
				await rest.PUT('/api/v1/properties/{property}/room-types/order', {
					params: { path: { property: propertyId } },
					body: { ids: moved.map((type) => type.id) }
				})
			);
		});
	}
</script>

<h1>Room types</h1>
{#if error}<p class="error" role="alert">{error}</p>{/if}

{#if roomTypes.error}
	<p class="error" role="alert">{errorMessage(roomTypes.error)}</p>
{:else if roomTypes.data}
	<table>
		<thead>
			<tr>
				<th>Code</th>
				<th>Name</th>
				<th>Base</th>
				<th>Adults</th>
				<th>Children</th>
				<th>Max</th>
				<th>Status</th>
				{#if manage}<th><span class="visually-hidden">Actions</span></th>{/if}
			</tr>
		</thead>
		<tbody>
			{#each roomTypes.data as type, index (type.id)}
				<tr
					draggable={manage && !editing}
					class:inactive={!type.active}
					ondragstart={() => (dragged = index)}
					ondragover={(event) => event.preventDefault()}
					ondrop={() => {
						if (dragged !== null) void reorder(dragged, index);
						dragged = null;
					}}
				>
					<td>{type.code}</td>
					{#if editing?.id === type.id}
						<td><input aria-label="Name of {type.code}" bind:value={editing.name} /></td>
						<td
							><input
								aria-label="Base occupancy of {type.code}"
								type="number"
								min="1"
								bind:value={editing.baseOccupancy}
							/></td
						>
						<td
							><input
								aria-label="Maximum adults of {type.code}"
								type="number"
								min="1"
								bind:value={editing.maxAdults}
							/></td
						>
						<td
							><input
								aria-label="Maximum children of {type.code}"
								type="number"
								min="0"
								bind:value={editing.maxChildren}
							/></td
						>
						<td
							><input
								aria-label="Maximum occupancy of {type.code}"
								type="number"
								min="1"
								bind:value={editing.maxOccupancy}
							/></td
						>
					{:else}
						<td>{type.name}</td>
						<td>{type.baseOccupancy}</td>
						<td>{type.maxAdults}</td>
						<td>{type.maxChildren}</td>
						<td>{type.maxOccupancy}</td>
					{/if}
					<td>{type.active ? 'Active' : 'Inactive'}</td>
					{#if manage}
						<td class="actions">
							{#if editing?.id === type.id}
								<button
									disabled={busy}
									aria-label="Save {type.code}"
									onclick={() =>
										editing && update(type, { name: editing.name, ...capacity(editing) })}
									>Save</button
								>
								<button class="secondary" onclick={() => (editing = null)}>Cancel</button>
							{:else}
								<button
									class="secondary"
									aria-label="Move {type.code} up"
									disabled={busy || index === 0}
									onclick={() => reorder(index, index - 1)}>↑</button
								>
								<button
									class="secondary"
									aria-label="Move {type.code} down"
									disabled={busy || index === roomTypes.data.length - 1}
									onclick={() => reorder(index, index + 1)}>↓</button
								>
								<button
									class="secondary"
									aria-label="Edit {type.code}"
									disabled={busy}
									onclick={() => (editing = { ...type })}>Edit</button
								>
								<button
									class="secondary"
									disabled={busy}
									aria-label="{type.active ? 'Deactivate' : 'Activate'} {type.code}"
									onclick={() => update(type, { active: !type.active })}
									>{type.active ? 'Deactivate' : 'Activate'}</button
								>
							{/if}
						</td>
					{/if}
				</tr>
			{:else}
				<tr><td colspan="8">No room types yet.</td></tr>
			{/each}
		</tbody>
	</table>
	{#if manage}
		<p class="hint">Drag a row, or use the arrows, to change the order rooms are listed in.</p>
		<form class="inline-form" aria-label="New room type" onsubmit={create}>
			<label>Code <input required pattern={'[A-Za-z0-9]{1,10}'} bind:value={draft.code} /></label>
			<label>Name <input required maxlength="100" bind:value={draft.name} /></label>
			<label>Base <input type="number" min="1" max="50" bind:value={draft.baseOccupancy} /></label>
			<label>Adults <input type="number" min="1" max="50" bind:value={draft.maxAdults} /></label>
			<label>Children <input type="number" min="0" max="50" bind:value={draft.maxChildren} /></label
			>
			<label>Max <input type="number" min="1" max="50" bind:value={draft.maxOccupancy} /></label>
			<button disabled={busy}>Add room type</button>
		</form>
	{/if}
{:else}
	<p>Loading…</p>
{/if}
```

Create `web/pms/src/routes/(app)/p/[property]/rooms/+page.svelte`:

```svelte
<script lang="ts">
	import { page } from '$app/state';
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { ApiError, errorMessage } from '$lib/api/problem';
	import { idempotencyKey, ifMatch, rest, unwrap } from '$lib/api/rest';
	import {
		fetchRooms,
		fetchRoomTypes,
		groupRooms,
		moveItem,
		rangeNumbers,
		roomsKey,
		roomTypesKey,
		type Room
	} from '$lib/rooms';
	import { can, fetchMe } from '$lib/session';

	const MAX_RANGE = 200;
	const propertyId = $derived(page.params.property ?? '');
	const client = useQueryClient();
	const me = createQuery(() => ({ queryKey: ['me'], queryFn: fetchMe }));
	const roomTypes = createQuery(() => ({
		queryKey: roomTypesKey(propertyId),
		queryFn: ({ signal }) => fetchRoomTypes(propertyId, signal)
	}));
	const rooms = createQuery(() => ({
		queryKey: roomsKey(propertyId),
		queryFn: ({ signal }) => fetchRooms(propertyId, signal)
	}));
	const manage = $derived(!!me.data && can(me.data, 'manageRooms', propertyId));
	const activeTypes = $derived((roomTypes.data ?? []).filter((type) => type.active));
	const typeCode = $derived(new Map((roomTypes.data ?? []).map((type) => [type.id, type.code])));

	let groupBy = $state<'type' | 'floor'>('type');
	const groups = $derived(groupRooms(rooms.data?.rooms ?? [], roomTypes.data ?? [], groupBy));

	let error = $state('');
	let busy = $state(false);
	let bulkDialog = $state<HTMLDialogElement>();
	let bulk = $state({
		roomTypeId: '',
		prefix: '',
		first: 101,
		last: 110,
		floor: '',
		sectionId: ''
	});
	const bulkNumbers = $derived(rangeNumbers(bulk.prefix, bulk.first, bulk.last));
	let single = $state({ roomTypeId: '', number: '', floor: '' });
	let sectionName = $state('');
	// One key per user action: kept for a retry of the same form, replaced after a success.
	const keys = { bulk: idempotencyKey(), single: idempotencyKey(), section: idempotencyKey() };

	/** Runs a command; shows its problem if it fails, and refetches after a version conflict. */
	async function run(command: () => Promise<unknown>): Promise<boolean> {
		busy = true;
		error = '';
		try {
			await command();
			await client.invalidateQueries({ queryKey: roomsKey(propertyId) });
			return true;
		} catch (err) {
			error = errorMessage(err);
			if (err instanceof ApiError && err.status === 412) {
				await client.invalidateQueries({ queryKey: roomsKey(propertyId) });
			}
			return false;
		} finally {
			busy = false;
		}
	}

	function openBulk() {
		bulk.roomTypeId ||= activeTypes[0]?.id ?? '';
		error = '';
		bulkDialog?.showModal();
	}

	async function addRange(event: SubmitEvent) {
		event.preventDefault();
		const added = await run(async () =>
			unwrap(
				await rest.POST('/api/v1/properties/{property}/rooms/bulk', {
					params: { path: { property: propertyId }, header: { 'Idempotency-Key': keys.bulk } },
					body: {
						room_type_id: bulk.roomTypeId,
						prefix: bulk.prefix,
						first: bulk.first,
						last: bulk.last,
						floor: bulk.floor || null,
						section_id: bulk.sectionId || null
					}
				})
			)
		);
		if (added) {
			keys.bulk = idempotencyKey();
			bulkDialog?.close();
		}
	}

	async function addRoom(event: SubmitEvent) {
		event.preventDefault();
		const added = await run(async () =>
			unwrap(
				await rest.POST('/api/v1/properties/{property}/rooms', {
					params: { path: { property: propertyId }, header: { 'Idempotency-Key': keys.single } },
					body: {
						room_type_id: single.roomTypeId || activeTypes[0]?.id,
						number: single.number,
						floor: single.floor || null
					}
				})
			)
		);
		if (added) {
			keys.single = idempotencyKey();
			single.number = '';
		}
	}

	async function addSection(event: SubmitEvent) {
		event.preventDefault();
		const added = await run(async () =>
			unwrap(
				await rest.POST('/api/v1/properties/{property}/sections', {
					params: { path: { property: propertyId }, header: { 'Idempotency-Key': keys.section } },
					body: { name: sectionName }
				})
			)
		);
		if (added) {
			keys.section = idempotencyKey();
			sectionName = '';
		}
	}

	function update(
		room: Room,
		body: {
			room_type_id?: string;
			floor?: string | null;
			section_id?: string | null;
			active?: boolean;
		}
	) {
		return run(async () =>
			unwrap(
				await rest.PATCH('/api/v1/properties/{property}/rooms/{room}', {
					params: { path: { property: propertyId, room: room.id }, header: ifMatch(room.version) },
					body
				})
			)
		);
	}

	/** Moves `room` one place up or down in the property's overall room order. */
	function move(room: Room, delta: number) {
		const all = rooms.data?.rooms ?? [];
		const from = all.findIndex((r) => r.id === room.id);
		const moved = moveItem(all, from, from + delta);
		if (moved.every((r, index) => r.id === all[index].id)) return;
		return run(async () =>
			unwrap(
				await rest.PUT('/api/v1/properties/{property}/rooms/order', {
					params: { path: { property: propertyId } },
					body: { ids: moved.map((r) => r.id) }
				})
			)
		);
	}
</script>

<h1>Rooms</h1>
{#if error}<p class="error" role="alert">{error}</p>{/if}

{#if rooms.error || roomTypes.error}
	<p class="error" role="alert">{errorMessage(rooms.error ?? roomTypes.error)}</p>
{:else if rooms.data && roomTypes.data}
	<div class="inline-form">
		<label>
			Group by
			<select bind:value={groupBy}>
				<option value="type">Room type</option>
				<option value="floor">Floor</option>
			</select>
		</label>
		{#if manage}
			<button disabled={busy || activeTypes.length === 0} onclick={openBulk}>Add rooms…</button>
		{/if}
	</div>
	{#if activeTypes.length === 0}
		<p>Add a room type first.</p>
	{/if}

	{#each groups as group (group.key)}
		<h2>{group.label}</h2>
		<table>
			<thead>
				<tr>
					<th>Number</th>
					<th>Type</th>
					<th>Floor</th>
					<th>Section</th>
					<th>Status</th>
					{#if manage}<th><span class="visually-hidden">Actions</span></th>{/if}
				</tr>
			</thead>
			<tbody>
				{#each group.rooms as room (room.id)}
					<tr class:inactive={!room.active}>
						<td>{room.number}</td>
						{#if manage}
							<td>
								<select
									aria-label="Type of room {room.number}"
									value={room.roomTypeId}
									disabled={busy}
									onchange={(event) => update(room, { room_type_id: event.currentTarget.value })}
								>
									{#each roomTypes.data as type (type.id)}
										<option value={type.id} disabled={!type.active}>{type.code}</option>
									{/each}
								</select>
							</td>
							<td>
								<input
									aria-label="Floor of room {room.number}"
									value={room.floor ?? ''}
									disabled={busy}
									onchange={(event) => update(room, { floor: event.currentTarget.value || null })}
								/>
							</td>
							<td>
								<select
									aria-label="Section of room {room.number}"
									value={room.sectionId ?? ''}
									disabled={busy}
									onchange={(event) =>
										update(room, { section_id: event.currentTarget.value || null })}
								>
									<option value="">None</option>
									{#each rooms.data.sections as section (section.id)}
										<option value={section.id}>{section.name}</option>
									{/each}
								</select>
							</td>
						{:else}
							<td>{typeCode.get(room.roomTypeId)}</td>
							<td>{room.floor ?? ''}</td>
							<td>{rooms.data.sections.find((s) => s.id === room.sectionId)?.name ?? ''}</td>
						{/if}
						<td>{room.active ? 'Active' : 'Inactive'}</td>
						{#if manage}
							<td class="actions">
								<button
									class="secondary"
									aria-label="Move room {room.number} up"
									disabled={busy}
									onclick={() => move(room, -1)}>↑</button
								>
								<button
									class="secondary"
									aria-label="Move room {room.number} down"
									disabled={busy}
									onclick={() => move(room, 1)}>↓</button
								>
								<button
									class="secondary"
									disabled={busy}
									aria-label="{room.active ? 'Deactivate' : 'Activate'} room {room.number}"
									onclick={() => update(room, { active: !room.active })}
									>{room.active ? 'Deactivate' : 'Activate'}</button
								>
							</td>
						{/if}
					</tr>
				{/each}
			</tbody>
		</table>
	{:else}
		<p>No rooms yet.</p>
	{/each}

	{#if manage && activeTypes.length > 0}
		<h2>Add a room</h2>
		<form class="inline-form" aria-label="New room" onsubmit={addRoom}>
			<label
				>Number <input required pattern={'[A-Za-z0-9-]{1,10}'} bind:value={single.number} /></label
			>
			<label>
				Type
				<select bind:value={single.roomTypeId}>
					{#each activeTypes as type (type.id)}<option value={type.id}>{type.code}</option>{/each}
				</select>
			</label>
			<label>Floor <input maxlength="20" bind:value={single.floor} /></label>
			<button disabled={busy}>Add room</button>
		</form>

		<h2>Housekeeping sections</h2>
		<p>{rooms.data.sections.map((section) => section.name).join(', ') || 'None yet.'}</p>
		<form class="inline-form" aria-label="New section" onsubmit={addSection}>
			<label>Name <input required maxlength="100" bind:value={sectionName} /></label>
			<button disabled={busy}>Add section</button>
		</form>
	{/if}
{:else}
	<p>Loading…</p>
{/if}

<dialog bind:this={bulkDialog} aria-labelledby="bulk-title">
	<form class="form" onsubmit={addRange}>
		<h2 id="bulk-title">Add rooms</h2>
		<label>
			Room type
			<select required bind:value={bulk.roomTypeId}>
				{#each activeTypes as type (type.id)}<option value={type.id}>{type.code}</option>{/each}
			</select>
		</label>
		<label>Prefix (optional) <input maxlength="5" bind:value={bulk.prefix} /></label>
		<label>First number <input type="number" min="0" required bind:value={bulk.first} /></label>
		<label>Last number <input type="number" min="0" required bind:value={bulk.last} /></label>
		<label>Floor <input maxlength="20" bind:value={bulk.floor} /></label>
		<label>
			Section
			<select bind:value={bulk.sectionId}>
				<option value="">None</option>
				{#each rooms.data?.sections ?? [] as section (section.id)}
					<option value={section.id}>{section.name}</option>
				{/each}
			</select>
		</label>
		{#if bulkNumbers.length > MAX_RANGE}
			<p class="error">At most {MAX_RANGE} rooms at a time.</p>
		{:else if bulkNumbers.length > 0}
			<p>{bulkNumbers[0]} – {bulkNumbers.at(-1)} ({bulkNumbers.length} rooms)</p>
		{/if}
		{#if error}<p class="error" role="alert">{error}</p>{/if}
		<div class="actions">
			<button disabled={busy || bulkNumbers.length === 0 || bulkNumbers.length > MAX_RANGE}
				>Add {bulkNumbers.length} rooms</button
			>
			<button type="button" class="secondary" onclick={() => bulkDialog?.close()}>Cancel</button>
		</div>
	</form>
</dialog>
```

```sh
cd web/pms && bun run codegen
```

- [ ] **Step 4: Run the checks and the end-to-end tests**

```sh
cd web/pms
bun run lint && bun run check && bun run test && bun run build
bun run test:e2e
```

Expected: clean; Playwright: 3 passed.

- [ ] **Step 5: Commit**

```sh
git add -A
git commit -m "feat(web): room types and rooms screens with inline edit, reordering and bulk add"
```

### Task 16: Inventory month grid and block dialog

The inventory screen: room types × the days of a month, each cell showing rooms available (and out-of-order rooms), with previous/next month. It is built on `DateGrid`, a reusable component: one scroll container, sticky date header and row labels, day lines drawn with a CSS gradient, and only the columns in view (plus two of overscan each side, plus the active cell) rendered, positioned with `transform`. Arrow keys, Home and End move the active cell (announced with `aria-activedescendant`); Enter, Space or a click opens that day's blocks for the type, where a block can be released. **Block a room** opens `BlockDialog` (room, from, until, reason — which suggests the kind —, kind, note); a 409 lists the blocks in the way and keeps the dialog open.

Two fixes found while verifying belong here. `Problem` allows extension members (`conflicts`). And the root `QueryClient` sets `notifyOnChangeProps: 'all'`: TanStack Query otherwise re-renders a query only for the fields read on its last render, and `inventory.data && roomTypes.data` skips reading `roomTypes.data` while the first is loading, so the screen stayed on "Loading…" when opened directly.

`tests/e2e/perf.spec.ts` is the spec's client gate (`@perf`, run on request): a 200-room, 12-type property; the median of ten switches between two loaded months must render in under 50 ms, at most 3 of 90 frames may take over 25 ms while scrolling, and the DOM must hold only the columns in view.

**Files:**
- Create: `web/pms/src/lib/components/BlockDialog.svelte`
- Create: `web/pms/src/lib/components/DateGrid.svelte`
- Create: `web/pms/src/routes/(app)/p/[property]/inventory/+page.svelte`
- Modify: `web/pms/playwright.config.ts`
- Modify: `web/pms/src/lib/api/problem.ts`
- Modify: `web/pms/src/lib/inventory.ts`
- Modify: `web/pms/src/routes/(app)/p/[property]/+layout.svelte`
- Modify: `web/pms/src/routes/+layout.svelte`
- Test: `web/pms/tests/e2e/inventory.spec.ts` (new)
- Test: `web/pms/tests/e2e/perf.spec.ts` (new)
- Test: `web/pms/src/lib/inventory.spec.ts`

**Interfaces:**
- Consumes: Task 13's `grid.ts`, `inventory.ts`, `rooms.ts`; Task 12's GraphQL reads; Task 11's block commands.
- Produces: `DateGrid.svelte` props `{ label, rows: { id, label }[], columns: string[], cell: Snippet<[Row, string]>, header: Snippet<[string]>, cellLabel(row, column), initialColumn?, onactivate?(row, column), columnWidth = 64, rowHeight = 44, railWidth = 160, overscan = 2 }`; `BlockDialog.svelte` props `{ propertyId, businessDate, rooms, roomTypes, reasons, open (bindable), from }`; `inventory.conflictMessages(error, roomNumber) -> string[]`; route `/p/[property]/inventory`; accessible names the tests rely on (grid `Availability`, cells `<CODE> <date>: <n> available, …`, `Block a room`, `Release room <n>`, region `Blocks on <date>`, test id `business-date`).

- [ ] **Step 1: Write the failing tests**

`@perf` tests are excluded from the default run.

Modify `web/pms/src/lib/inventory.spec.ts`:

```diff
--- a/web/pms/src/lib/inventory.spec.ts
+++ b/web/pms/src/lib/inventory.spec.ts
@@ -1,7 +1,9 @@
 import { describe, expect, it } from 'vitest';
+import { ApiError } from './api/problem';
 import {
 	addDays,
 	blocksOn,
+	conflictMessages,
 	indexInventory,
 	inventoryKey,
 	monthDays,
@@ -81,3 +83,24 @@ describe('blocksOn', () => {
 		expect(blocksOn(blocks, '2026-09-24', new Set(['r201'])).map((b) => b.id)).toEqual(['b2']);
 	});
 });
+
+describe('conflictMessages', () => {
+	it('names each block in the way of a 409', () => {
+		const conflict = new ApiError({
+			type: 'about:blank',
+			title: 'Conflict',
+			status: 409,
+			conflicts: [
+				{ id: 'b1', room_id: 'r101', from: '2026-09-24', to: '2026-09-26', kind: 'out_of_order' }
+			]
+		});
+
+		expect(conflictMessages(conflict, () => '101')).toEqual([
+			'Room 101 is already blocked from 2026-09-24 until 2026-09-26 (out of order).'
+		]);
+	});
+
+	it('is empty for any other error', () => {
+		expect(conflictMessages(new TypeError('Failed to fetch'), () => '101')).toEqual([]);
+	});
+});
```

Create `web/pms/tests/e2e/inventory.spec.ts`:

```ts
import { expect, test } from '@playwright/test';
import { addRooms, addRoomType, createProperty, signUp } from './helpers';

/** `YYYY-MM-DD` plus `days`. */
function addDays(date: string, days: number): string {
	const moved = new Date(`${date}T00:00:00Z`);
	moved.setUTCDate(moved.getUTCDate() + days);
	return moved.toISOString().slice(0, 10);
}

test('blocking a room reduces availability on the calendar until it is released', async ({
	page
}) => {
	await signUp(page);
	await createProperty(page, 'GAL');
	await page.getByRole('link', { name: 'Room types' }).click();
	await addRoomType(page, 'DLX', 'Deluxe');
	await page.getByRole('link', { name: 'Rooms', exact: true }).click();
	await addRooms(page, 'DLX', 101, 105);

	await page.getByRole('link', { name: 'Inventory' }).click();
	const today = (await page.getByTestId('business-date').textContent())!.trim();
	const grid = page.getByRole('grid', { name: 'Availability' });
	await expect(grid.getByRole('gridcell', { name: `DLX ${today}: 5 available` })).toBeVisible();

	await page.getByRole('button', { name: 'Block a room' }).click();
	const dialog = page.getByRole('dialog', { name: 'Block a room' });
	await dialog.getByLabel('Room').selectOption({ label: '101 · DLX' });
	await dialog.getByLabel('From').fill(today);
	await dialog.getByLabel('Until (first day back)').fill(addDays(today, 2));
	await dialog.getByLabel('Reason').selectOption({ label: 'Maintenance' });
	await dialog.getByLabel('Note').fill('Leaking pipe');
	await dialog.getByRole('button', { name: 'Block room' }).click();
	await expect(dialog).toBeHidden();

	await expect(grid.getByRole('gridcell', { name: `DLX ${today}: 4 available` })).toBeVisible();
	await expect(
		grid.getByRole('gridcell', { name: `DLX ${addDays(today, 1)}: 4 available` })
	).toBeVisible();

	// A second block over the same days is refused and names the block in the way.
	await page.getByRole('button', { name: 'Block a room' }).click();
	await dialog.getByLabel('Room').selectOption({ label: '101 · DLX' });
	await dialog.getByLabel('From').fill(addDays(today, 1));
	await dialog.getByLabel('Until (first day back)').fill(addDays(today, 3));
	await dialog.getByLabel('Reason').selectOption({ label: 'Renovation' });
	await dialog.getByRole('button', { name: 'Block room' }).click();
	await expect(dialog.getByRole('alert')).toContainText(
		`Room 101 is already blocked from ${today} until ${addDays(today, 2)}`
	);
	await dialog.getByRole('button', { name: 'Cancel' }).click();

	// Keyboard: go to the first day of the month, walk right to the business date and open it.
	await grid.focus();
	await page.keyboard.press('Home');
	for (let day = 1; day < Number(today.slice(8)); day++) await page.keyboard.press('ArrowRight');
	await page.keyboard.press('Enter');
	const day = page.getByRole('region', { name: `Blocks on ${today}` });
	await expect(day).toContainText('101');
	await expect(day).toContainText('Leaking pipe');
	await day.getByRole('button', { name: 'Release room 101' }).click();

	await expect(grid.getByRole('gridcell', { name: `DLX ${today}: 5 available` })).toBeVisible();
});
```

Create `web/pms/tests/e2e/perf.spec.ts`:

```ts
import { expect, test, type APIRequestContext } from '@playwright/test';
import { createProperty, signUp } from './helpers';

// Phase 1 gate: the inventory month grid of a 200-room, 12-type property renders in under 50 ms and
// scrolls at 60 fps, with only the columns in view in the DOM. Timings on shared CI runners are noise,
// so this test is left out of the default run. Run it locally with:
//   E2E_PERF=1 bun run test:e2e --grep @perf

const ROOM_TYPES = 12;
const ROOMS = 200;

async function post(api: APIRequestContext, path: string, data: object) {
	const response = await api.post(path, {
		headers: { 'x-goodfolk-csrf': '1', 'Idempotency-Key': crypto.randomUUID() },
		data
	});
	expect(response.status(), await response.text()).toBe(201);
	return response.json();
}

test('the month grid renders under 50 ms and scrolls at 60 fps @perf', async ({ page }) => {
	await signUp(page);
	await createProperty(page, 'BIG');
	const property = `/api/v1/properties/${page.url().split('/p/')[1]}`;
	for (let index = 0; index < ROOM_TYPES; index++) {
		const type = await post(page.request, `${property}/room-types`, {
			code: `T${index}`,
			name: `Type ${index}`,
			base_occupancy: 2,
			max_adults: 2,
			max_children: 1,
			max_occupancy: 3
		});
		const count = Math.floor(ROOMS / ROOM_TYPES) + (index < ROOMS % ROOM_TYPES ? 1 : 0);
		const first = (index + 1) * 100 + 1;
		await post(page.request, `${property}/rooms/bulk`, {
			room_type_id: type.id,
			first,
			last: first + count - 1
		});
	}
	await page.getByRole('link', { name: 'Inventory' }).click();
	const grid = page.getByRole('grid', { name: 'Availability' });
	await expect(grid.getByRole('row')).toHaveCount(ROOM_TYPES + 1);
	// Load next month once and come back, so the timing below measures rendering, not the network.
	await page.getByRole('button', { name: 'Next month' }).click();
	await expect(grid.getByRole('gridcell', { name: /available/ }).first()).toBeVisible();
	await page.getByRole('button', { name: 'Previous month' }).click();
	await expect(grid.getByRole('gridcell', { name: /available/ }).first()).toBeVisible();

	// Median of ten switches between the two loaded months.
	const renderMs = await page.evaluate(async () => {
		const timings: number[] = [];
		for (let switches = 0; switches < 10; switches++) {
			const label = switches % 2 === 0 ? 'Next month' : 'Previous month';
			const button = document.querySelector<HTMLButtonElement>(`button[aria-label="${label}"]`)!;
			const start = performance.now();
			button.click();
			// Svelte applies the update in a microtask; reading the grid's size forces style and layout.
			await new Promise((resolve) => setTimeout(resolve, 0));
			document.querySelector('[role="grid"]')!.getBoundingClientRect();
			timings.push(performance.now() - start);
			await new Promise((resolve) => setTimeout(resolve, 100));
		}
		timings.sort((a, b) => a - b);
		return timings[5];
	});
	const frames = await page.evaluate(async () => {
		const viewport = document.querySelector<HTMLElement>('[role="grid"]')!;
		const gaps: number[] = [];
		let last = performance.now();
		for (let frame = 0; frame < 90; frame++) {
			viewport.scrollLeft += 16;
			await new Promise((resolve) => requestAnimationFrame(resolve));
			const now = performance.now();
			gaps.push(now - last);
			last = now;
		}
		return gaps;
	});
	const cells = await grid.getByRole('gridcell').count();
	// Columns that fit beside the 160 px row labels at 64 px each, plus a partial one.
	const columnsInView = await page.evaluate(
		() => Math.ceil((document.querySelector('[role="grid"]')!.clientWidth - 160) / 64) + 1
	);

	const slow = frames.filter((gap) => gap > 25).length;
	console.log(
		`month grid: render ${renderMs.toFixed(1)} ms, ${slow}/90 slow frames, ${cells} cells`
	);
	expect(renderMs).toBeLessThan(50);
	expect(slow).toBeLessThanOrEqual(3);
	// Only the columns in view, two of overscan on each side and the active one are in the DOM.
	expect(cells).toBeLessThanOrEqual(ROOM_TYPES * (columnsInView + 2 * 2 + 1));
});
```

Modify `web/pms/playwright.config.ts`:

```diff
--- a/web/pms/playwright.config.ts
+++ b/web/pms/playwright.config.ts
@@ -17,6 +17,8 @@ export default defineConfig({
 	testDir: 'tests/e2e',
 	fullyParallel: true,
 	forbidOnly: !!process.env.CI,
+	// Performance checks (@perf) run only when asked for: E2E_PERF=1.
+	grepInvert: process.env.E2E_PERF === '1' ? undefined : /@perf/,
 	retries: process.env.CI ? 1 : 0,
 	reporter: process.env.CI ? [['github'], ['list']] : 'list',
 	use: {
```

- [ ] **Step 2: Run them to verify they fail**

```sh
cd web/pms && bun run test
bun run test:e2e inventory
```

Expected: unit tests FAIL (`conflictMessages` is not exported); the end-to-end test FAILS waiting for the `Inventory` link.

- [ ] **Step 3: Implement**

Modify `web/pms/src/lib/api/problem.ts`:

```diff
--- a/web/pms/src/lib/api/problem.ts
+++ b/web/pms/src/lib/api/problem.ts
@@ -4,6 +4,8 @@ export interface Problem {
 	title: string;
 	status: number;
 	detail?: string;
+	/** Extension members, such as `conflicts` on a 409 for an overlapping room block. */
+	[extension: string]: unknown;
 }
 
 export class ApiError extends Error {
```

Modify `web/pms/src/lib/inventory.ts`:

```diff
--- a/web/pms/src/lib/inventory.ts
+++ b/web/pms/src/lib/inventory.ts
@@ -1,6 +1,8 @@
 import { graphql } from './api/gql';
 import type { InventoryQuery } from './api/gql/graphql';
 import { query } from './api/graphql';
+import type { components } from './api/openapi';
+import { ApiError } from './api/problem';
 
 export const InventoryDocument = graphql(`
 	query Inventory($propertyId: UUID!, $from: Date!, $to: Date!) {
@@ -83,3 +85,14 @@ export function blocksOn(blocks: Block[], date: string, rooms?: Set<string>): Bl
 		(block) => block.from <= date && date < block.to && (!rooms || rooms.has(block.roomId))
 	);
 }
+
+type BlockConflict = Pick<components['schemas']['Block'], 'room_id' | 'from' | 'to' | 'kind'>;
+
+/** One sentence per block listed in a 409's `conflicts`; empty for any other error. */
+export function conflictMessages(error: unknown, roomNumber: (roomId: string) => string): string[] {
+	if (!(error instanceof ApiError) || !Array.isArray(error.problem.conflicts)) return [];
+	return (error.problem.conflicts as BlockConflict[]).map(
+		(block) =>
+			`Room ${roomNumber(block.room_id)} is already blocked from ${block.from} until ${block.to} (${block.kind.replaceAll('_', ' ')}).`
+	);
+}
```

Create `web/pms/src/lib/components/DateGrid.svelte`:

```svelte
<!--
	A grid of rows by dates that renders only the columns in view (horizontal virtualization).
	One scroll container; the date header and the row labels stay put with `position: sticky`; the day
	lines are a CSS gradient, so there are no per-cell background nodes. The inventory calendar uses it
	now and the tape chart will later.

	Keyboard: arrow keys move between cells, Home and End go to the first and last date, Enter or Space
	activates the cell. The active cell is announced through `aria-activedescendant`.
-->
<script lang="ts" generics="Row extends { id: string; label: string }">
	import type { Snippet } from 'svelte';
	import { moveFocus, revealColumn, visibleColumns, type Cell } from '$lib/grid';

	interface Props {
		/** Accessible name of the grid. */
		label: string;
		rows: Row[];
		/** Dates as `YYYY-MM-DD`, one column each. */
		columns: string[];
		/** Content of a cell. */
		cell: Snippet<[Row, string]>;
		/** Content of a column header. */
		header: Snippet<[string]>;
		/** Accessible name of a cell. */
		cellLabel: (row: Row, column: string) => string;
		/** The column that is active and scrolled into view first. */
		initialColumn?: number;
		onactivate?: (row: Row, column: string) => void;
		columnWidth?: number;
		rowHeight?: number;
		railWidth?: number;
		overscan?: number;
	}

	let {
		label,
		rows,
		columns,
		cell,
		header,
		cellLabel,
		initialColumn = 0,
		onactivate,
		columnWidth = 64,
		rowHeight = 44,
		railWidth = 160,
		overscan = 2
	}: Props = $props();

	const id = $props.id();
	let viewport = $state<HTMLDivElement>();
	let scrollLeft = $state(0);
	let width = $state(0);
	let active = $state<Cell>({ row: 0, column: 0 });

	// Start on `initialColumn`, scrolled to the left edge, whenever the columns change (a new month).
	$effect(() => {
		const column = Math.min(initialColumn, Math.max(0, columns.length - 1));
		active = { row: 0, column };
		if (viewport) viewport.scrollLeft = column * columnWidth;
	});

	// Columns in view, plus the active one wherever it is, so `aria-activedescendant` names a node.
	const rendered = $derived.by(() => {
		const visible = visibleColumns(
			scrollLeft,
			width - railWidth,
			columnWidth,
			columns.length,
			overscan
		);
		const inView = Array.from({ length: visible.end - visible.start }, (_, i) => visible.start + i);
		const outside = active.column < visible.start || active.column >= visible.end;
		return outside && active.column < columns.length ? [active.column, ...inView] : inView;
	});

	function cellId(cellAt: Cell): string {
		return `${id}-${cellAt.row}-${cellAt.column}`;
	}

	function activate(cellAt: Cell) {
		active = cellAt;
		const row = rows[cellAt.row];
		if (row) onactivate?.(row, columns[cellAt.column]);
	}

	function keydown(event: KeyboardEvent) {
		if (event.key === 'Enter' || event.key === ' ') {
			event.preventDefault();
			activate(active);
			return;
		}
		const next = moveFocus(active, event.key, { rows: rows.length, columns: columns.length });
		if (!next || !viewport) return;
		event.preventDefault();
		active = next;
		viewport.scrollLeft = revealColumn(
			next.column,
			viewport.scrollLeft,
			viewport.clientWidth - railWidth,
			columnWidth
		);
	}
</script>

<div
	class="viewport"
	role="grid"
	tabindex="0"
	aria-label={label}
	aria-rowcount={rows.length + 1}
	aria-colcount={columns.length + 1}
	aria-activedescendant={rows.length > 0 ? cellId(active) : undefined}
	bind:this={viewport}
	bind:clientWidth={width}
	onscroll={() => (scrollLeft = viewport?.scrollLeft ?? 0)}
	onkeydown={keydown}
>
	<div
		class="canvas"
		style:width="{railWidth + columns.length * columnWidth}px"
		style:--column="{columnWidth}px"
		style:--row="{rowHeight}px"
		style:--rail="{railWidth}px"
	>
		<div class="row header" role="row" aria-rowindex={1}>
			<div class="rail" role="columnheader" aria-colindex={1}></div>
			{#each rendered as column (columns[column])}
				<div
					class="cell"
					role="columnheader"
					aria-colindex={column + 2}
					style:transform="translateX({railWidth + column * columnWidth}px)"
				>
					{@render header(columns[column])}
				</div>
			{/each}
		</div>
		{#each rows as row, r (row.id)}
			<div class="row" role="row" aria-rowindex={r + 2}>
				<div class="rail" role="rowheader" aria-colindex={1}>{row.label}</div>
				{#each rendered as column (columns[column])}
					<div
						id={cellId({ row: r, column })}
						class="cell"
						class:active={active.row === r && active.column === column}
						role="gridcell"
						tabindex="-1"
						aria-colindex={column + 2}
						aria-label={cellLabel(row, columns[column])}
						style:transform="translateX({railWidth + column * columnWidth}px)"
						onclick={() => activate({ row: r, column })}
						onkeydown={keydown}
					>
						{@render cell(row, columns[column])}
					</div>
				{/each}
			</div>
		{/each}
	</div>
</div>

<style>
	.viewport {
		overflow: auto;
		max-height: 70vh;
		border: 1px solid var(--border);
		border-radius: var(--radius);
	}
	.viewport:focus-visible {
		outline: 2px solid var(--accent);
	}
	.canvas {
		position: relative;
		/* Day lines, one per column after the row labels. */
		background-image: linear-gradient(to right, var(--border) 1px, transparent 1px);
		background-size: var(--column) 100%;
		background-position: var(--rail) 0;
	}
	.row {
		position: relative;
		height: var(--row);
		border-bottom: 1px solid var(--border);
	}
	.header {
		position: sticky;
		top: 0;
		z-index: 2;
		background: var(--surface);
	}
	.rail {
		position: sticky;
		left: 0;
		z-index: 1;
		width: var(--rail);
		height: 100%;
		display: flex;
		align-items: center;
		padding: 0 0.5rem;
		background: var(--surface);
		border-right: 1px solid var(--border);
	}
	.cell {
		position: absolute;
		top: 0;
		left: 0;
		width: var(--column);
		height: var(--row);
		display: grid;
		place-items: center;
		cursor: pointer;
	}
	.cell.active {
		outline: 2px solid var(--accent);
		outline-offset: -2px;
	}
</style>
```

Create `web/pms/src/lib/components/BlockDialog.svelte`:

```svelte
<!--
	Blocks a room for [from, until): until is the first day the room is back. A 409 lists the blocks in
	the way, and the dialog stays open so the dates can be changed.
-->
<script lang="ts">
	import { useQueryClient } from '@tanstack/svelte-query';
	import { errorMessage } from '$lib/api/problem';
	import { idempotencyKey, rest, unwrap } from '$lib/api/rest';
	import { addDays, conflictMessages, inventoryKey, monthOf, shiftMonth } from '$lib/inventory';
	import type { BlockReason, Room, RoomType } from '$lib/rooms';

	type Kind = 'out_of_order' | 'out_of_service';

	interface Props {
		propertyId: string;
		businessDate: string;
		rooms: Room[];
		roomTypes: RoomType[];
		reasons: BlockReason[];
		/** Set to open the dialog; cleared when it closes. */
		open: boolean;
		/** The first day suggested when the dialog opens. */
		from: string;
	}

	let {
		propertyId,
		businessDate,
		rooms,
		roomTypes,
		reasons,
		open = $bindable(),
		from
	}: Props = $props();

	const client = useQueryClient();
	let dialog = $state<HTMLDialogElement>();
	let form = $state({
		roomId: '',
		from: '',
		until: '',
		kind: 'out_of_order' as Kind,
		reasonId: '',
		note: ''
	});
	let problems = $state<string[]>([]);
	let busy = $state(false);
	let key = idempotencyKey();

	const activeRooms = $derived(rooms.filter((room) => room.active));
	const activeReasons = $derived(reasons.filter((reason) => reason.active));
	const typeCode = $derived(new Map(roomTypes.map((type) => [type.id, type.code])));
	const roomNumber = (roomId: string) => rooms.find((room) => room.id === roomId)?.number ?? '?';

	$effect(() => {
		if (!dialog) return;
		if (open && !dialog.open) {
			const start = from < businessDate ? businessDate : from;
			form = {
				roomId: form.roomId || (activeRooms[0]?.id ?? ''),
				from: start,
				until: addDays(start, 1),
				kind: 'out_of_order',
				reasonId: '',
				note: ''
			};
			problems = [];
			key = idempotencyKey();
			dialog.showModal();
		} else if (!open && dialog.open) {
			dialog.close();
		}
	});

	function chooseReason(reasonId: string) {
		form.reasonId = reasonId;
		const reason = reasons.find((r) => r.id === reasonId);
		if (reason)
			form.kind = reason.defaultKind === 'OUT_OF_ORDER' ? 'out_of_order' : 'out_of_service';
	}

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		busy = true;
		problems = [];
		try {
			unwrap(
				await rest.POST('/api/v1/properties/{property}/rooms/{room}/blocks', {
					params: {
						path: { property: propertyId, room: form.roomId },
						header: { 'Idempotency-Key': key }
					},
					body: {
						from: form.from,
						to: form.until,
						kind: form.kind,
						reason_id: form.reasonId,
						note: form.note
					}
				})
			);
			// The server's events refetch these too; invalidating now shows the change at once.
			const last = monthOf(addDays(form.until, -1));
			for (let month = monthOf(form.from); month <= last; month = shiftMonth(month, 1)) {
				void client.invalidateQueries({ queryKey: inventoryKey(propertyId, month) });
			}
			open = false;
		} catch (err) {
			const conflicts = conflictMessages(err, roomNumber);
			problems = conflicts.length > 0 ? conflicts : [errorMessage(err)];
			// A new attempt with changed dates is a new request.
			key = idempotencyKey();
		} finally {
			busy = false;
		}
	}
</script>

<dialog bind:this={dialog} aria-labelledby="block-title" onclose={() => (open = false)}>
	<form class="form" onsubmit={submit}>
		<h2 id="block-title">Block a room</h2>
		<label>
			Room
			<select required bind:value={form.roomId}>
				{#each activeRooms as room (room.id)}
					<option value={room.id}>{room.number} · {typeCode.get(room.roomTypeId)}</option>
				{/each}
			</select>
		</label>
		<label>From <input type="date" required min={businessDate} bind:value={form.from} /></label>
		<label>
			Until (first day back)
			<input
				type="date"
				required
				min={form.from ? addDays(form.from, 1) : businessDate}
				bind:value={form.until}
			/>
		</label>
		<label>
			Reason
			<select
				required
				value={form.reasonId}
				onchange={(event) => chooseReason(event.currentTarget.value)}
			>
				<option value="" disabled>Choose a reason</option>
				{#each activeReasons as reason (reason.id)}
					<option value={reason.id}>{reason.label}</option>
				{/each}
			</select>
		</label>
		<label>
			Kind
			<select bind:value={form.kind}>
				<option value="out_of_order">Out of order (not sellable)</option>
				<option value="out_of_service">Out of service (sellable, flagged)</option>
			</select>
		</label>
		<label>Note <textarea maxlength="500" bind:value={form.note}></textarea></label>
		{#if problems.length > 0}
			<div class="error" role="alert">
				{#each problems as problem, index (index)}<p>{problem}</p>{/each}
			</div>
		{/if}
		<div class="actions">
			<button disabled={busy}>Block room</button>
			<button type="button" class="secondary" onclick={() => (open = false)}>Cancel</button>
		</div>
	</form>
</dialog>
```

Create `web/pms/src/routes/(app)/p/[property]/inventory/+page.svelte`:

```svelte
<script lang="ts">
	import { page } from '$app/state';
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { ApiError, errorMessage } from '$lib/api/problem';
	import { ifMatch, rest, unwrap } from '$lib/api/rest';
	import BlockDialog from '$lib/components/BlockDialog.svelte';
	import DateGrid from '$lib/components/DateGrid.svelte';
	import {
		addDays,
		blocksOn,
		fetchMonth,
		indexInventory,
		inventoryKey,
		monthDays,
		monthOf,
		shiftMonth,
		type Block
	} from '$lib/inventory';
	import { fetchProperties, propertiesKey } from '$lib/properties';
	import { fetchRooms, fetchRoomTypes, roomsKey, roomTypesKey } from '$lib/rooms';
	import { can, fetchMe } from '$lib/session';

	const propertyId = $derived(page.params.property ?? '');
	const client = useQueryClient();
	const me = createQuery(() => ({ queryKey: ['me'], queryFn: fetchMe }));
	const properties = createQuery(() => ({
		queryKey: propertiesKey,
		queryFn: ({ signal }) => fetchProperties(signal)
	}));
	const roomTypes = createQuery(() => ({
		queryKey: roomTypesKey(propertyId),
		queryFn: ({ signal }) => fetchRoomTypes(propertyId, signal)
	}));
	const rooms = createQuery(() => ({
		queryKey: roomsKey(propertyId),
		queryFn: ({ signal }) => fetchRooms(propertyId, signal)
	}));

	const businessDate = $derived(
		properties.data?.find((property) => property.id === propertyId)?.businessDate ?? ''
	);
	let chosenMonth = $state<string | null>(null);
	const month = $derived(chosenMonth ?? (businessDate ? monthOf(businessDate) : ''));
	const days = $derived(month ? monthDays(month) : []);
	const inventory = createQuery(() => ({
		queryKey: inventoryKey(propertyId, month),
		queryFn: ({ signal }) => fetchMonth(propertyId, month, signal),
		enabled: !!month
	}));
	const counts = $derived(indexInventory(inventory.data?.inventory ?? []));
	const rows = $derived(
		(roomTypes.data ?? [])
			.filter((type) => type.active)
			.map((type) => ({ id: type.id, label: `${type.code} · ${type.name}`, code: type.code }))
	);
	const mayBlock = $derived(!!me.data && can(me.data, 'blockRooms', propertyId));
	const monthLabel = $derived(
		month
			? new Date(`${month}-01T00:00:00Z`).toLocaleDateString(undefined, {
					month: 'long',
					year: 'numeric',
					timeZone: 'UTC'
				})
			: ''
	);

	let selected = $state<{ date: string; roomTypeId: string; code: string } | null>(null);
	const selectedBlocks = $derived.by(() => {
		if (!selected) return [];
		const typeId = selected.roomTypeId;
		const ofType = (rooms.data?.rooms ?? []).filter((room) => room.roomTypeId === typeId);
		return blocksOn(inventory.data?.blocks ?? [], selected.date, new Set(ofType.map((r) => r.id)));
	});
	let blocking = $state(false);
	let error = $state('');
	let busy = $state(false);

	const WEEKDAYS = ['Sun', 'Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat'];

	function weekday(date: string): string {
		return WEEKDAYS[new Date(`${date}T00:00:00Z`).getUTCDay()];
	}

	function roomNumber(roomId: string): string {
		return rooms.data?.rooms.find((room) => room.id === roomId)?.number ?? '?';
	}

	function reasonLabel(reasonId: string): string {
		return rooms.data?.blockReasons.find((reason) => reason.id === reasonId)?.label ?? '';
	}

	function cellLabel(row: { id: string; code: string }, date: string): string {
		const day = counts.get(row.id, date);
		return day
			? `${row.code} ${date}: ${day.available} available, ${day.sold} sold, ${day.outOfOrder} out of order`
			: `${row.code} ${date}: not counted`;
	}

	/** Ends a block as of the business date, or cancels it if it has not started. */
	async function release(block: Block) {
		busy = true;
		error = '';
		try {
			unwrap(
				await rest.PATCH('/api/v1/properties/{property}/blocks/{block}', {
					params: {
						path: { property: propertyId, block: block.id },
						header: ifMatch(block.version)
					},
					body: { to: block.from > businessDate ? block.from : businessDate }
				})
			);
			for (let m = monthOf(block.from); m <= monthOf(addDays(block.to, -1)); m = shiftMonth(m, 1)) {
				void client.invalidateQueries({ queryKey: inventoryKey(propertyId, m) });
			}
		} catch (err) {
			error = errorMessage(err);
			if (err instanceof ApiError && err.status === 412) {
				void client.invalidateQueries({ queryKey: inventoryKey(propertyId, month) });
			}
		} finally {
			busy = false;
		}
	}
</script>

<h1>Inventory</h1>
<div class="inline-form">
	<button
		class="secondary"
		aria-label="Previous month"
		onclick={() => (chosenMonth = shiftMonth(month, -1))}>←</button
	>
	<h2 class="month">{monthLabel}</h2>
	<button
		class="secondary"
		aria-label="Next month"
		onclick={() => (chosenMonth = shiftMonth(month, 1))}>→</button
	>
	<span class="hint">Business date: <span data-testid="business-date">{businessDate}</span></span>
	{#if mayBlock && rooms.data && roomTypes.data}
		<button onclick={() => (blocking = true)}>Block a room</button>
	{/if}
</div>
{#if error}<p class="error" role="alert">{error}</p>{/if}

{#if inventory.error || roomTypes.error}
	<p class="error" role="alert">{errorMessage(inventory.error ?? roomTypes.error)}</p>
{:else if inventory.data && roomTypes.data}
	{#if rows.length === 0}
		<p>No active room types yet.</p>
	{:else}
		<DateGrid
			label="Availability"
			{rows}
			columns={days}
			{cellLabel}
			initialColumn={Math.max(0, days.indexOf(businessDate))}
			onactivate={(row, date) => (selected = { date, roomTypeId: row.id, code: row.code })}
		>
			{#snippet header(date)}
				<span class="day" class:today={date === businessDate}>
					<small>{weekday(date)}</small>
					{Number(date.slice(8))}
				</span>
			{/snippet}
			{#snippet cell(row, date)}
				{@const day = counts.get(row.id, date)}
				{#if day}
					<span class="count" class:full={day.available <= 0}>{day.available}</span>
					{#if day.outOfOrder > 0}<small class="blocked">{day.outOfOrder} OOO</small>{/if}
				{:else}
					<span class="hint">–</span>
				{/if}
			{/snippet}
		</DateGrid>
		<p class="hint">
			Rooms available per room type and day. Click a day, or use the arrow keys and Enter, to see
			its blocks.
		</p>
	{/if}

	{#if selected}
		<section aria-label="Blocks on {selected.date}">
			<h2>{selected.code} on {selected.date}</h2>
			{#if selectedBlocks.length === 0}
				<p>No rooms of this type are blocked on this day.</p>
			{:else}
				<ul>
					{#each selectedBlocks as block (block.id)}
						<li>
							Room {roomNumber(block.roomId)} · {block.kind === 'OUT_OF_ORDER'
								? 'Out of order'
								: 'Out of service'} · {reasonLabel(block.reasonId)} · {block.from} until {block.to}
							{#if block.note}· {block.note}{/if}
							{#if mayBlock && block.to > businessDate}
								<button
									class="secondary"
									disabled={busy}
									aria-label="Release room {roomNumber(block.roomId)}"
									onclick={() => release(block)}>Release</button
								>
							{/if}
						</li>
					{/each}
				</ul>
			{/if}
		</section>
	{/if}
{:else}
	<p>Loading…</p>
{/if}

{#if mayBlock && rooms.data && roomTypes.data && businessDate}
	<BlockDialog
		{propertyId}
		{businessDate}
		rooms={rooms.data.rooms}
		roomTypes={roomTypes.data}
		reasons={rooms.data.blockReasons}
		from={selected?.date ?? businessDate}
		bind:open={blocking}
	/>
{/if}

<style>
	.month {
		margin: 0;
		min-width: 12rem;
		text-align: center;
	}
	.day {
		display: grid;
		text-align: center;
		line-height: 1.1;
	}
	.today {
		color: var(--accent);
		font-weight: 600;
	}
	.count {
		font-weight: 600;
	}
	.count.full,
	.blocked {
		color: var(--danger);
	}
</style>
```

Modify `web/pms/src/routes/(app)/p/[property]/+layout.svelte`:

```diff
--- a/web/pms/src/routes/(app)/p/[property]/+layout.svelte
+++ b/web/pms/src/routes/(app)/p/[property]/+layout.svelte
@@ -8,7 +8,8 @@
 	const links = $derived([
 		{ href: resolve('/(app)/p/[property]', { property }), label: 'Overview' },
 		{ href: resolve('/(app)/p/[property]/room-types', { property }), label: 'Room types' },
-		{ href: resolve('/(app)/p/[property]/rooms', { property }), label: 'Rooms' }
+		{ href: resolve('/(app)/p/[property]/rooms', { property }), label: 'Rooms' },
+		{ href: resolve('/(app)/p/[property]/inventory', { property }), label: 'Inventory' }
 	]);
 </script>
 
```

Modify `web/pms/src/routes/+layout.svelte`:

```diff
--- a/web/pms/src/routes/+layout.svelte
+++ b/web/pms/src/routes/+layout.svelte
@@ -9,6 +9,10 @@
 		defaultOptions: {
 			queries: {
 				staleTime: 30_000,
+				// By default a query only re-renders for the fields read on its last render. Templates read
+				// fields conditionally (`a.data && b.data` skips `b.data` while `a` loads), so a field first
+				// read after it changed would stay stale; notify on every change instead.
+				notifyOnChangeProps: 'all',
 				// Client errors (401, 403, 404…) will not succeed on retry.
 				retry: (count, error) => !(error instanceof ApiError && error.status < 500) && count < 2
 			}
```

- [ ] **Step 4: Run the checks, the end-to-end tests and the grid gate**

```sh
cd web/pms
bun run lint && bun run check && bun run test && bun run build
bun run test:e2e
E2E_PERF=1 bun run test:e2e --grep @perf
```

Expected: 37 unit tests pass; Playwright: 4 passed; the `@perf` run prints about `month grid: render 39 ms, 0/90 slow frames, 228 cells` and passes.

- [ ] **Step 5: Commit**

```sh
git add -A
git commit -m "feat(web): inventory month grid with horizontal virtualization, keyboard navigation and the block dialog"
```

### Task 17: Documentation and the Phase 1 gate

The remaining documentation: event keys and GraphQL bounds in the API conventions, the corrected `inventory_day` columns in ARCHITECTURE §7.3, how to run the performance gates in the README, and the roadmap's Phase 1 links and the carry-overs that stay open (the SSE `?property=` check and the purge job). Then the whole suite, as CI and the gates run it.

**Files:**
- Modify: `README.md`
- Modify: `docs/ARCHITECTURE.md`
- Modify: `docs/ROADMAP.md`
- Modify: `docs/design/api-conventions.md`

**Interfaces:**
- Consumes: everything above.
- Produces: documentation only.

- [ ] **Step 1: Update the documentation**

Modify `docs/design/api-conventions.md`:

```diff
--- a/docs/design/api-conventions.md
+++ b/docs/design/api-conventions.md
@@ -87,12 +87,13 @@ Request DTOs derive `garde::Validate` and are checked with `error::validate(&bod
 
 ## Change events
 
-After a successful write, in the same transaction, call `db::notify(&mut tx, &Event { tenant_id, property_id, keys })`. Keys name **TanStack Query keys** the client should invalidate (`"properties"`, `"reservations"`, `"tape:<property>:<tileStart>"` …). Events never carry data. Choose keys so that a change refetches only screens that show it.
+After a successful write, in the same transaction, call `db::notify(&mut tx, &Event { tenant_id, property_id, keys })`. Keys name **TanStack Query keys** the client should invalidate (`"properties"`, `"room-types:<property>"`, `"rooms:<property>"` (rooms, sections and block reasons), `"inventory:<property>:<yyyy-mm>"` (one per month a change touches), later `"tape:<property>:<tileStart>"` …). Events never carry data. Choose keys so that a change refetches only screens that show it; the SPA's query keys start with the same string (`web/pms/src/lib/rooms.ts`, `inventory.ts`).
 
 ## GraphQL
 
 - Schema in `crates/core-api/src/graphql.rs`: depth ≤ 8, complexity ≤ 500. Introspection returns `null` in production. Persisted-query allowlist from Phase 4.
-- Resolvers read `PgPool` and `TenantContext` from the request context and batch relations with DataLoaders (from Phase 1, the first nested relation).
+- Resolvers read `PgPool` and `TenantContext` from the request context, check the permission for the `propertyId` argument (`graphql::scoped`), and batch relations with DataLoaders once the first nested relation exists (Phase 1 lists are flat: rooms carry `roomTypeId`, blocks carry `roomId`).
+- Date-range queries are bounded: `inventory` spans at most 93 days, `blocks` at most 400.
 - Fields are camelCase; IDs are `UUID`; money is `{ amount: Int (minor units, as string if > 2^53), currency }`; dates are ISO `YYYY-MM-DD`.
 - Lists use cursor pagination (`first`, `after` → `{ nodes, pageInfo { endCursor, hasNextPage } }`) once they can exceed a few hundred rows (reservations, guests).
 
```

Modify `docs/ARCHITECTURE.md`:

```diff
--- a/docs/ARCHITECTURE.md
+++ b/docs/ARCHITECTURE.md
@@ -327,7 +327,7 @@ query TapeTile($property: ID!, $from: Date!, $to: Date!, $rooms: [ID!]) {
 
 ### 7.3 Inventory
 
-- `inventory_day(property_id, room_type_id, date, physical, sold, blocked, …)` keeps a counter row per room type per day, updated in the same transaction as the reservation or block. Availability reads are then O(days), with no counting over reservations.
+- `inventory_day(property_id, room_type_id, date, physical, sold, out_of_order)` keeps a counter row per room type per day, from the business date for 730 days, updated in the same transaction as the room, reservation or block. Availability reads are then O(days), with no counting over reservations.
 - Correctness guard: a nightly job recomputes counters from the source tables and alerts on drift.
 - **Room blocks**: `room_block(room_id, range, reason, kind, note)` with `kind = out_of_order` (removed from inventory, for renovation or construction) or `out_of_service` (still sellable, but flagged). Reason codes are configurable per property. Blocks show on the tape chart and reduce availability. They cannot overlap existing stays unless the overlapping stays are moved first; the server returns the list of conflicts.
 - Inventory calendar screen (GraphQL): room types × dates grid showing free/sold/blocked, plus restrictions, windowed by month.
```

Modify `docs/ROADMAP.md`:

```diff
--- a/docs/ROADMAP.md
+++ b/docs/ROADMAP.md
@@ -9,6 +9,7 @@ Companion to [ARCHITECTURE.md](ARCHITECTURE.md). Each phase ends in something th
 | [design/data-model.md](design/data-model.md) | Every table, all phases, and the rules each must follow |
 | [design/api-conventions.md](design/api-conventions.md) | Auth, CSRF, errors, idempotency, concurrency, events, GraphQL and REST shapes |
 | [superpowers/plans/2026-09-23-phase-0-foundations.md](superpowers/plans/2026-09-23-phase-0-foundations.md) | **Phase 0 implementation plan**: 15 test-first tasks with complete code, run in order on a clean repository before the plan was written |
+| [superpowers/plans/2026-09-24-phase-1-rooms-inventory.md](superpowers/plans/2026-09-24-phase-1-rooms-inventory.md) | **Phase 1 implementation plan**: 17 tasks with complete code, executed in order on the Phase 0 code before the plan was written |
 | [specs/](specs/) | Phases 1–9: scope, data, API, UI, rules, required tests and performance gates |
 
 Each later phase gets its step-by-step implementation plan at the start of that phase, written and verified against the code as it stands then (the same method as Phase 0). Writing code-level plans for Phase 7 now would mean guessing at code that Phases 1–6 have not written yet.
@@ -28,7 +29,7 @@ Each later phase gets its step-by-step implementation plan at the start of that
 
 Moved out of Phase 0 during planning (nothing used them yet): outbox → Pub/Sub, `/proto`, MinIO (Phase 6); `If-Match`, login throttling, Playwright (Phase 1); persisted GraphQL queries (Phase 4); staff invitations (Phase 8).
 
-## Phase 1 — Rooms, room types, inventory base ([spec](specs/phase-1-rooms-inventory.md))
+## Phase 1 — Rooms, room types, inventory base ([spec](specs/phase-1-rooms-inventory.md), [plan](superpowers/plans/2026-09-24-phase-1-rooms-inventory.md))
 
 - Room types, rooms, floors and sections. CRUD over REST, lists over GraphQL.
 - `inventory_day` counters and the room block model (out of order / out of service, reason codes).
@@ -44,8 +45,7 @@ Moved out of Phase 0 during planning (nothing used them yet): outbox → Pub/Sub
     - an HTTP-level tenant switch followed by a create;
     - the `property.created` audit row.
   - Log a warning when `load_grants` skips a role it doesn't recognise.
-  - Check the SSE `?property=` filter against the user's grants once property-scoped roles exist.
-  - A purge job for expired sessions and old idempotency keys (runs in `jobs-svc`, Phase 7).
+  - Still open, not in the Phase 1 plan: check the SSE `?property=` filter against the user's grants (property-scoped grants exist now, but the stream only carries cache keys); a purge job for expired sessions, old idempotency keys and old `login_failure` rows (runs in `jobs-svc`, Phase 7).
 
 ## Phase 2 — Rates and meal plans ([spec](specs/phase-2-rates-meal-plans.md))
 
```

Modify `README.md`:

````diff
--- a/README.md
+++ b/README.md
@@ -11,7 +11,7 @@ Multi-tenant, cloud-hosted hotel property management system.
 |---|---|
 | `crates/core-api` | axum HTTP API (REST commands, GraphQL reads, server-sent events) |
 | `crates/db` | Postgres pool, migrations, tenant-scoped transactions, change events |
-| `modules/*` | Domain modules (`identity`, `property`, …) |
+| `modules/*` | Domain modules (`identity`, `property`, `rooms`, …) |
 | `migrations/` | SQL migrations, applied by `core-api migrate` |
 | `web/pms` | SvelteKit staff app (single-page) |
 
@@ -43,6 +43,18 @@ bun run api:schemas && bun run codegen   # after any API change; commit the resu
 bun run lint && bun run check && bun run test && bun run build
 ```
 
+### Performance gates
+
+Phase 1 sets two, both run by hand because shared CI machines make timings noisy:
+
+```sh
+# inventory(month) for a 200-room, 12-type property: p95 under 20 ms server time
+DATABASE_URL=$DATABASE_OWNER_URL cargo test --release -p core-api --test perf -- --ignored --nocapture
+
+# the month grid for the same property renders in under 50 ms and scrolls at 60 fps (see End-to-end tests)
+cd web/pms && E2E_PERF=1 E2E_DATABASE_URL=... bun run test:e2e --grep @perf
+```
+
 ### End-to-end tests
 
 Playwright (`web/pms/tests/e2e`) drives the production build against a real API and Postgres. It starts both itself: the API on port 18080 (`cargo run -p core-api`) and `vite preview` on port 4173. Give it a migrated database of its own, so test accounts never land in your development data:
````

- [ ] **Step 2: Run everything**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
DATABASE_URL=$TEST_DATABASE_URL cargo test --release -p core-api --test perf -- --ignored --nocapture
cd web/pms
bun run api:schemas && bun run codegen && git diff --exit-code -- src/lib/api
bun run lint && bun run check && bun run test && bun run build
bun run test:e2e
E2E_PERF=1 bun run test:e2e --grep @perf
```

Expected: 134 Rust tests pass (1 ignored: the release-mode gate, which passes when run explicitly); no generated-type drift; 37 web tests; Playwright 4 passed; both performance gates pass.

- [ ] **Step 3: Commit**

```sh
git add -A
git commit -m "docs: Phase 1 events, GraphQL bounds, performance gates and roadmap status"
```

## Phase 1 done when

- A manager defines room types and rooms, groups rooms by floor and housekeeping section, and reorders both (Tasks 7–8, 10, 15; e2e `rooms.spec.ts`).
- They block a room for a date range with a reason, see conflicts, and release a block early (Tasks 9, 11, 16; e2e `inventory.spec.ts`).
- The inventory calendar shows physical / sold / out-of-order / available per room type per day for any month (Tasks 12, 16), inside the gates: server p95 < 20 ms (`tests/perf.rs`), grid render < 50 ms and 60 fps (`perf.spec.ts @perf`).
- CI is green: fmt, clippy, Rust tests, cargo-deny, generated types, lint, svelte-check, web tests, build, and the new `e2e` job.

## Self-review

**Spec coverage.**

| Spec item | Where |
|---|---|
| Tables `room_type`, `housekeeping_section`, `room`, `block_reason`, `room_block`, `inventory_day`, property settings, `btree_gist`; isolation case per table | Task 5 |
| REST commands and permissions (`rooms.manage` → `RoomsManage`, `inventory.block` → `InventoryBlock`, `properties.manage` → `PropertiesManage`); creates idempotent; updates `If-Match` | Tasks 3, 6, 10, 11 |
| GraphQL `roomTypes`, `rooms`, `blocks`, `inventory` | Task 12 |
| Events `room-types:<p>`, `rooms:<p>`, `inventory:<p>:<yyyy-mm>` | Tasks 7–9 (emitted), 10 (tested over SSE), 13 (client keys) |
| `physical` = active rooms; create/deactivate/retype adjust from the business date, in one transaction | Task 8 |
| Rows for business date → +730 days; extended on property creation (no types yet) and room changes | Task 7 (`extend_window`, called by every writer) |
| Blocks `[from, to)`, exclusion constraint, 409 listing the conflict, out-of-order vs out-of-service, no start before the business date | Tasks 5, 9, 11 |
| Early release restores counters for the remaining days | Tasks 9, 11, 16 |
| UI: room types (inline edit, drag to reorder, deactivate); rooms (grouped by type or floor, bulk add); inventory month grid (virtualized, reusable, click → day's blocks, keyboard); block dialog with conflicts | Tasks 15, 16 |
| Tests: isolation cases; property-based counter test; 409 / release / past; `If-Match` 412; Playwright type → rooms → block → reduced availability | Tasks 5, 9, 11, 6, 16 |
| Performance gates | Tasks 12, 16 |
| Carry-overs: `If-Match`, login throttling, Playwright | Tasks 6, 4, 14 |
| Phase 0 review: fallbacks; second resync; `/readyz` 503, GraphQL limits, tenant switch + create, `property.created` audit test; unknown-role warning | Tasks 1, 2, 3 |
| Deferred by decision (not in this plan): SSE `?property=` vs grants; purge jobs | ROADMAP (Task 17) |

**Placeholder scan.** No "TBD", "similar to", or undefined names: every file is shown in full or as its exact diff, and each task's Interfaces list the names later tasks use.

**Type consistency.** Names were checked by compiling and running each task in order: for example `rooms::shorten_block(tx, tenant, actor, property, id, expected_version, to)` (Task 9) is what `routes::blocks::shorten` (Task 11) calls, and the SPA's `inventoryKey(p, month)` produces the `inventory:<p>:<yyyy-mm>` strings `rooms::month_keys` emits.

## Execution handoff

Plan complete and saved to `docs/superpowers/plans/2026-09-24-phase-1-rooms-inventory.md`. Two execution options:

1. **Subagent-driven (recommended):** a fresh subagent per task, with a review between tasks (superpowers:subagent-driven-development).
2. **Inline execution:** execute the tasks in one session with checkpoints (superpowers:executing-plans).
