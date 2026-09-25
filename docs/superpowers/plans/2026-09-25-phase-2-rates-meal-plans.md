# Phase 2: Rate Plans and Meal Plans Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A revenue manager creates standard, derived and custom rate plans tagged FIT-F, FIT-L, OTA, TA or IBE, each in its own currency; edits prices per room type, date and occupancy in a grid; applies previewed bulk changes ("+10 % on July weekends") that reprice derived plans in the same transaction; sets restrictions that derived plans can inherit; prices BB/HB/FB as per-person supplements; and quotes a stay with every reason it cannot be sold. The Phase 1 review follow-ups land alongside.

**Architecture:** A new `rates` module owns rate plans (a tree checked for currency, depth ≤ 3 and cycles), resolved prices (`rate_day`) and restrictions (`rate_restriction`), meal supplements, cancellation policies and the quote. Writes to a property's rates take one advisory lock, change the plan's rows, then rewrite the derived plans level by level with set-based SQL through `app.derive_amount` (half-up rounding in integer arithmetic), so reads never derive anything. `quote` is a pure function over rows `load_quote` reads, ready for reservations (Phase 3) and the booking engine (Phase 9). `core-api` adds REST commands (creates and bulk changes behind the idempotency middleware, updates with `If-Match`) and GraphQL reads; the SPA adds Rate plans, Rates (the Phase 1 `DateGrid` with editable cells, a bulk-change dialog with a server preview, restrictions and a quote panel) and Meal plans screens.

**Tech Stack:** as Phase 1 (Rust 1.97, axum 0.8, sqlx 0.9, async-graphql 7.2, utoipa 6, garde, proptest 1, Postgres 17, SvelteKit 2 / Svelte 5, Bun 1.3, TanStack Query 6, Playwright 1.63). No new dependencies: `db` gains `proptest` as a dev-dependency, `core-api` depends on the new `rates` crate.

**Spec:** [docs/specs/phase-2-rates-meal-plans.md](../../specs/phase-2-rates-meal-plans.md). Also binding: [data-model.md](../../design/data-model.md) (Phase 2 and the rules for every table), [api-conventions.md](../../design/api-conventions.md) (including "Patterns to copy"), [ARCHITECTURE.md](../../ARCHITECTURE.md) §7.2, and [ROADMAP.md](../../ROADMAP.md) Phase 2 with its Phase 1 carry-overs.

**Verified:** every task was executed in order on `main` at `63679d8` (Phase 1 complete) in a throwaway worktree before this plan was written, and the code blocks below are rendered from those commits (new files in full, changes as the exact diffs). Every "see them fail" step was run and failed as described: for Tasks 1–5, 12–14 before the implementation was written, and for Tasks 6–11, 15 and 16 by replaying the task's test files on the previous task's commit. Every passing step passed; `cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D warnings` were clean after every task, and `bun run lint && bun run check && bun run test && bun run build` after every task that touches `web/pms`, with no drift in the generated API types. At the end: 205 Rust tests pass (141 before this phase) plus 3 ignored release-mode performance gates, 61 web unit tests (48 before), 8 Playwright tests (4 before), and Phase 1's `@perf` grid test (38 ms render, no slow frames). The property-based tests were also checked against deliberate bugs: dropping the depth rule or the rounding step, or skipping the reprice after a formula change, each makes them fail within one run; the concurrent sign-in test fails about one run in three without the throttle's advisory lock.

Performance gates, release build, in-process server time, measured on the laptop this plan was verified on (Postgres 17, CPU governor `powersave`), run one at a time:

| Gate | Spec | Measured |
|---|---|---|
| `rateGrid`, 1 plan × 12 types × 62 days × 2 occupancies (1488 prices + 744 restrictions) | p95 < 30 ms | p95 11.5–20 ms (p50 10–13 ms) |
| Bulk change, 1 year × 12 types × 2 occupancies, 2 derived levels (26 280 rows) | < 300 ms | median 178–322 ms over runs; the gate asserts the median of 10 |
| Phase 1 `inventory(month)` (unchanged) | p95 < 20 ms | p95 4.7–8.5 ms |

The bulk-change gate has little margin on this machine: its median passed in most runs and failed in some (up to 322 ms), with the same code, when the machine was in a slower state (every timing, including Phase 1's, roughly doubled then). Getting there took three changes found by profiling, all in the plan: `app.derive_amount` is not `STRICT` (so Postgres inlines it: 900 → 590 ms), `rate_day` leaves half of each page free (fillfactor 50, so a rewrite of every price is a HOT update: → 280 ms), and existing prices are rewritten with `UPDATE` rather than `INSERT … ON CONFLICT DO UPDATE` (→ 180–200 ms). Check it on the production database class before relying on it.

Not verified here: the CI workflow itself (its steps were run by hand); `cargo deny` (not installed; no new third-party crates); Chromium's sandbox (local Playwright runs used `PLAYWRIGHT_NO_SANDBOX=1`, CI keeps it on); the migrations as a non-superuser owner such as Neon's (they create tables and one function; no backfill).

## Global Constraints

- Everything in the Phase 0 and Phase 1 plans' Global Constraints still holds: toolchain `1.97`, edition 2024, `unsafe_code = "forbid"`, clippy `all = deny`, rustfmt `max_width = 120`; all data access through `db::begin(pool, scope)`; UUIDv7 ids from Rust; problem+json errors; CSRF header on every state-changing request; `ApiJson`/`ApiPath`/`ApiQuery` extractors; `.map_err(internal)` in resolvers.
- Tests need a superuser URL: `export TEST_DATABASE_URL=postgres://goodfolk_owner:goodfolk_owner_dev@localhost:5432/goodfolk`. End-to-end tests need a migrated database of their own as the API role: `export E2E_DATABASE_URL=postgres://goodfolk_api:goodfolk_api_dev@localhost:5432/goodfolk_e2e` (migrate it with the owner URL after Task 4 and Task 11: `DATABASE_OWNER_URL=postgres://goodfolk_owner:goodfolk_owner_dev@localhost:5432/goodfolk_e2e cargo run -p core-api -- migrate`).
- **Every new tenant table has `tenant_id`, `property_id`, forced RLS, composite foreign keys into its property, and a case in `crates/db/tests/isolation.rs`.**
- Permissions (user decision): Owner = everything. Manager = everything in Phase 1 plus `RatesManage` (rate plans, prices, bulk changes, restrictions, meal supplements, cancellation policies). Front desk, housekeeping and accountant view rates only. Every role has `RatesView`.
- Rates (user decision): resident (FIT-L) prices are set by hand; derivation only within the same currency (another currency is rejected); derivation depth ≤ 3 below a standard plan; no cycles; rounding half-up to `rounding_step`.
- Money is `bigint` minor units in the plan's (or supplement's) currency, at most 100 000 000 000; GraphQL sends it as `Int`. Dates are `YYYY-MM-DD`; ranges are `[from, to)` in the API (the SPA's "Through" fields add a day).
- Rate writes stay inside `[business date, business date + 730 days)`, the Phase 1 counter window.
- Every create command and `bulk-change` is idempotent (the `commands` router); every update takes `If-Match: "<version>"` and returns `ETag`; an update that names no field is a 422. `PUT …/prices` and `PUT …/restrictions` are not versioned (the last write to a cell wins).
- Events: `rate-plans:<p>` (plans, meal supplements, cancellation policies) and `rates:<p>:<plan>:<yyyy-mm>` (one per plan and month a price or restriction change touches, derived plans included). The SPA's query keys start with the same strings.
- Performance gates: `rateGrid` 62 days × 12 types × 2 occupancies p95 < 30 ms; bulk change of 1 year × 12 types with 2 derived levels < 300 ms. Both are ignored release-mode tests in `crates/core-api/tests/perf.rs`, run by hand with `--test-threads=1`.
- Commit messages describe only the change (no tool or AI attribution).

## Decisions made while planning

Where the spec left a choice open, this plan decided as follows.

1. **Empty PATCH is a 422** ("send at least one field to change"), through an `error::Changes` trait and `validate_changes`, for every update DTO, old and new; it no longer bumps the version. The alternative, a no-op 200, would hide client bugs.
2. **Replayed creates carry their `ETag`:** `idempotency_key` gains an `etag` column (migration 0005), stored with the response and replayed with it. Versioned responses declare the header in `#[utoipa::path]`; `tests/openapi.rs` pins which operations do.
3. **Permissions** `RatesView` (every role) and `RatesManage` (owner, manager) are new `identity::Permission` variants; the matrix test lists all nine.
4. **Restrictions have their own table**, `rate_restriction (rate_plan_id, date, room_type_id)`, instead of columns on `rate_day`: they are set per plan, room type and date (not per occupancy), and a day can be restricted before it is priced. `rateGrid` returns `{ prices, restrictions }`. data-model.md records the refinement.
5. **Occupancy counts adults** (1 to the room type's maximum occupancy, as the spec says; the grid shows rows up to the type's maximum adults). Children pay the child meal supplement. A missing occupancy falls back to the nearest lower one plus the plan's `extra_adult_amount` per adult.
6. **Plan kinds:** standard plans are priced by hand and may be parents; custom plans are priced by hand and may not; derived plans have a parent (standard or derived), a formula (`percent` in basis points, −100 % to +1000 %, or `amount` in minor units) and use the parent's currency and a subset of its room types. Code, kind and currency never change; a derived plan may move to another parent (its prices, and inherited restrictions, are rebuilt from the new one). A cycle is also always "too deep" with a limit of 3, but it keeps its own, clearer message.
7. **FIT segments fix residency:** `FIT_F` plans are `non_resident`, `FIT_L` plans `resident` (a check constraint); left out, the segment's residency is filled in; other segments may be restricted either way or sold to anyone.
8. **Derivation and bulk changes share one SQL function**, `app.derive_amount(base, mode, value, step)`: integer arithmetic, never below 0, half-up to a multiple of `step`. A bulk change rounds to the plan's own `rounding_step`; `set` writes the value as given. The function is not `STRICT` so Postgres inlines it.
9. **Recalculation per level:** one `UPDATE … FROM` rewrites a level's existing prices, and one `INSERT … SELECT … ON CONFLICT DO NOTHING` adds cells its parents gained (skipped for percent and amount bulk changes, which add none); a moved plan first drops prices its new parent lacks. The spec's "one `insert … on conflict do update` per level" was measured about a third slower for the bulk gate (Task 11).
10. **Prices are never deleted;** to stop selling a date, close it. So derived plans never keep a price their parent lost, except after a move (handled above).
11. **One lock per property for rate writes** (`rates::lock_rates`, a transaction advisory lock): a change cascades through many plans' rows, and a single lock is simpler and safe where a row lock order would be fragile. Reads take no lock.
12. **Event keys are chunked:** `rates:<p>:<plan>:<month>` keys grow with plans × months, so `rates` sends them in as many NOTIFY events as needed, each under 6000 bytes of keys; writes stay in the 730-day window, so one plan contributes at most 25 keys. Month iteration is shared with rooms (`rooms::months`).
13. **Bulk change is a POST through the idempotency middleware** (a retried "+10 %" must not apply twice) and answers `200 {"changed": n}`; its selection is `from`, `to`, ISO `weekdays`, `room_type_ids` and `occupancies` (empty = all), and `change: {mode: percent|amount|set, value}`.
14. **The bulk preview is a GraphQL read**, `bulkChangePreview`, computed by the same SQL as the change: the first 50 cells (before → after) and the total, so the dialog can preview a year it has not loaded.
15. **Restrictions are set over a range** (`PUT …/restrictions` with `from`, `to`, `weekdays`, `room_type_ids`); fields left out are unchanged and `min_stay`/`max_stay: null` removes them. Plans that inherit restrictions refuse their own (422); turning inheritance on replaces the plan's rows with the parent's, turning it off keeps the copy.
16. **Quote rules:** `restrictions_ok` is true when `violations` is empty. Closed and minimum/maximum stay apply to every night of the stay; closed-to-arrival only to the check-in date; closed-to-departure only to the check-out date. The quote needs the guest's `residency` (the spec's signature lacks it; the residency rule needs it). GraphQL caps a quote at 90 nights.
17. **Meal supplements** exist for BB, HB and FB only (RO is always 0, so it has no rows); `valid` may be open-ended; overlaps per meal plan and currency are a 409.
18. **Cancellation policies** are API-complete (rules sorted furthest from arrival first, one rule per day count, penalties by nights, basis points or minor units) and selectable on a rate plan; a screen to manage them is left to Phase 8's settings, like block reasons in Phase 1.
19. **Enums:** the module's text enums are generated by `text_enum!` with `$text:tt` values; with `:literal`, utoipa did not see the `serde(rename)` and documented variant names (found by svelte-check in Task 14, fixed where the macro is introduced). GraphQL mirrors them with `mirror_enum!` (values in capitals).
20. **Rate grid UI:** one month at a time like the inventory grid; rows are room type × adults plus a restrictions row; a click or Enter opens an input in the cell, and edits are saved in batches (`batcher`, 400 ms after the last edit, last value per cell wins) with drafts shown in italics until the refetch; a quote first flushes pending edits. Derived plans are read-only and show their formula.
21. **Phase 1 UI follow-ups:** the rooms page uses a `Pending` set keyed by row or form instead of one page-wide busy flag; `DateGrid` commits its clamped active cell when rows shrink, so it does not jump back when they return; and the room types page clears its form as soon as a create succeeds (before the refetch), a race the new e2e test hit.

## How to read the code blocks

New files are shown in full. Changes to existing files are shown as unified diffs against the previous task's result; they are exact, so an engineer can apply them by hand or save one to a file and run `git apply`. Generated files are never shown: `Cargo.lock` (updated by any `cargo` command) and `web/pms/src/lib/api/{openapi.json,openapi.d.ts,schema.graphql,gql/}` (by `cd web/pms && bun run api:schemas && bun run codegen`, which each task that changes the API runs in its checks; commit the result).

Migration `0006_rates.sql` is created in Task 4 and changed in Task 11 (two performance fixes). Until this phase ships it only reaches throwaway test databases; if you migrated another database with the Task 4 version, drop and recreate it.

## File Structure

```
migrations/0005_idempotency_etag.sql     idempotency_key.etag
migrations/0006_rates.sql                app.derive_amount, cancellation_policy, rate_plan, rate_plan_room_type,
                                         rate_day, rate_restriction, meal_supplement, RLS
crates/db/tests/rates_schema.rs          constraints, and derive_amount against exact arithmetic (property-based)
modules/identity/src/rbac.rs             RatesView, RatesManage
modules/rooms/src/inventory.rs           months() shared by cache keys
modules/rates/                           the new domain module
  src/lib.rs                             RatesError, text_enum!, keys, lock, audit, chunked notify
  src/plans.rs                           plan tree, derivation rules, create/update/list
  src/prices.rs                          set, derive down the tree, bulk change and preview, list
  src/restrictions.rs                    set, inherit, list
  src/meals.rs  src/policies.rs          meal supplements, cancellation policies
  src/quote.rs                           pure quote over loaded rows, and its loader
  tests/{plans,plan_tree,supplements,prices,derivation,restrictions,quote}.rs, tests/common/mod.rs
crates/core-api/src/error.rs             Changes, validate_changes
crates/core-api/src/idempotency.rs       stores and replays ETag
crates/core-api/src/routes/rates.rs      REST commands
crates/core-api/src/graphql.rs           ratePlans, rateGrid, bulkChangePreview, mealSupplements,
                                         cancellationPolicies, quote
crates/core-api/tests/{rates,rate_reads,perf}.rs
web/pms/src/lib/pending.svelte.ts        commands in flight by key
web/pms/src/lib/rates.ts                 queries, keys, money, grid rows, batcher
web/pms/src/routes/(app)/p/[property]/{rate-plans,rates,meal-plans}/+page.svelte
web/pms/tests/e2e/rates.spec.ts
```

## Tasks

### Task 1: Refuse empty updates; test concurrent sign-in throttling and cross-tenant room creates

Three Phase 1 review follow-ups. An update whose body names no field still bumped the resource's version; it
is now a 422 for every update DTO (property, room type, room, block reason, and the Phase 2 ones later). Two
tests the reviews asked for pin behaviour Phase 1 already has, so they pass as soon as they are written:
twelve concurrent failed sign-ins for one email yield exactly five 401s and seven 429s (without the
throttle's advisory lock this fails about one run in three), and an intruder cannot add rooms, a room range
or a section to another tenant's property, nor a room of another tenant's type to their own.

**Files:**
- Modify: `crates/core-api/src/error.rs`
- Modify: `crates/core-api/src/routes/blocks.rs`
- Modify: `crates/core-api/src/routes/properties.rs`
- Modify: `crates/core-api/src/routes/room_types.rs`
- Modify: `crates/core-api/src/routes/rooms.rs`
- Test: `crates/core-api/tests/auth.rs`
- Test: `crates/core-api/tests/properties.rs`
- Test: `crates/core-api/tests/rooms.rs`
- Modify: `docs/design/api-conventions.md`

**Interfaces:**
- Consumes: `ApiError::unprocessable`, `error::validate`, `TestApp::{send_with, staff}`.
- Produces: `error::Changes` (`fn is_empty(&self) -> bool`) and `error::validate_changes(&T)`; later update DTOs implement `Changes` and call `validate_changes`.

- [ ] **Step 1: Write the failing tests**

Modify `crates/core-api/tests/auth.rs`:

```diff
diff --git a/crates/core-api/tests/auth.rs b/crates/core-api/tests/auth.rs
index dcc3872..9fb0996 100644
--- a/crates/core-api/tests/auth.rs
+++ b/crates/core-api/tests/auth.rs
@@ -226,3 +226,23 @@ async fn a_successful_login_clears_earlier_failures(_: PgPoolOptions, opts: PgCo
     assert_eq!(after, vec![StatusCode::UNAUTHORIZED; 4]);
     assert_eq!(still_allowed.status, StatusCode::OK);
 }
+
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn concurrent_failed_logins_cannot_exceed_the_limit(_: PgPoolOptions, opts: PgConnectOptions) {
+    let app = TestApp::new(opts.clone()).await;
+    let superuser = PgPool::connect_with(opts).await.unwrap();
+    app.signup_owner("owner@example.com", "A").await;
+
+    let attempts = (0..12).map(|_| login(&app, "owner@example.com", "not the password"));
+    let statuses: Vec<StatusCode> =
+        futures::future::join_all(attempts).await.into_iter().map(|response| response.status).collect();
+
+    let rejected = statuses.iter().filter(|status| **status == StatusCode::UNAUTHORIZED).count();
+    let throttled = statuses.iter().filter(|status| **status == StatusCode::TOO_MANY_REQUESTS).count();
+    assert_eq!((rejected, throttled), (5, 7), "{statuses:?}");
+    let recorded: i64 = sqlx::query_scalar("select count(*) from login_failure where email = 'owner@example.com'")
+        .fetch_one(&superuser)
+        .await
+        .unwrap();
+    assert_eq!(recorded, 5);
+}
```

Modify `crates/core-api/tests/properties.rs`:

```diff
diff --git a/crates/core-api/tests/properties.rs b/crates/core-api/tests/properties.rs
index 4769573..0a4bfbb 100644
--- a/crates/core-api/tests/properties.rs
+++ b/crates/core-api/tests/properties.rs
@@ -195,7 +195,8 @@ async fn a_stale_version_is_412_and_a_missing_one_428(_: PgPoolOptions, opts: Pg
     let missing = patch(&app, &owner, &path, None, json!({"name": "Third edit"})).await;
     let malformed = patch(&app, &owner, &path, Some("2"), json!({"name": "Fourth edit"})).await;
     let unknown =
-        patch(&app, &owner, &format!("/api/v1/properties/{}", Uuid::now_v7()), Some("\"1\""), json!({})).await;
+        patch(&app, &owner, &format!("/api/v1/properties/{}", Uuid::now_v7()), Some("\"1\""), json!({"name": "X"}))
+            .await;
 
     assert_eq!(stale.status, StatusCode::PRECONDITION_FAILED, "{:?}", stale.body);
     assert_eq!(stale.headers[header::CONTENT_TYPE], "application/problem+json");
@@ -204,6 +205,22 @@ async fn a_stale_version_is_412_and_a_missing_one_428(_: PgPoolOptions, opts: Pg
     assert_eq!(unknown.status, StatusCode::NOT_FOUND, "{:?}", unknown.body);
 }
 
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn an_empty_update_is_a_422_and_keeps_the_version(_: PgPoolOptions, opts: PgConnectOptions) {
+    let app = TestApp::new(opts).await;
+    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
+    let created = create(&app, &owner, "key-00000001", galle()).await;
+    let path = format!("/api/v1/properties/{}", created.body["id"].as_str().unwrap());
+
+    let empty = patch(&app, &owner, &path, Some("\"1\""), json!({})).await;
+    let then = patch(&app, &owner, &path, Some("\"1\""), json!({"name": "Galle Fort"})).await;
+
+    assert_eq!(empty.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", empty.body);
+    assert_eq!(empty.body["detail"], "send at least one field to change");
+    assert_eq!(then.status, StatusCode::OK, "{:?}", then.body);
+    assert_eq!(then.body["version"], 2);
+}
+
 #[sqlx::test(migrator = "db::MIGRATOR")]
 async fn property_settings_are_validated(_: PgPoolOptions, opts: PgConnectOptions) {
     let app = TestApp::new(opts).await;
```

Modify `crates/core-api/tests/rooms.rs`:

```diff
diff --git a/crates/core-api/tests/rooms.rs b/crates/core-api/tests/rooms.rs
index 1d94631..31f4838 100644
--- a/crates/core-api/tests/rooms.rs
+++ b/crates/core-api/tests/rooms.rs
@@ -214,3 +214,66 @@ async fn a_new_room_tells_screens_to_refetch_rooms_and_inventory(_: PgPoolOption
     let months = event.keys.iter().filter(|key| key.starts_with(&format!("inventory:{property_id}:"))).count();
     assert!((25..=26).contains(&months), "{:?}", event.keys);
 }
+
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn rooms_and_sections_cannot_be_added_to_another_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
+    let app = TestApp::new(opts.clone()).await;
+    let superuser = PgPool::connect_with(opts).await.unwrap();
+    let (owner, property) = hotel(&app).await;
+    let room_type = post(&app, &owner, &format!("{property}/room-types"), deluxe()).await.body["id"].clone();
+    let intruder = app.signup_owner("intruder@example.com", "Other Hotels").await;
+    let own = json!({"code": "KAN", "name": "Kandy", "timezone": "Asia/Colombo", "base_currency": "LKR"});
+    let own = post(&app, &intruder, "/api/v1/properties", own).await.body["id"].as_str().unwrap().to_owned();
+    let room = json!({"room_type_id": room_type, "number": "666"});
+    let range = json!({"room_type_id": room_type, "first": 600, "last": 601});
+
+    let into_their_property = post(&app, &intruder, &format!("{property}/rooms"), room.clone()).await;
+    let range_into_their_property = post(&app, &intruder, &format!("{property}/rooms/bulk"), range).await;
+    let section_in_their_property =
+        post(&app, &intruder, &format!("{property}/sections"), json!({"name": "Intruders"})).await;
+    let with_their_room_type = post(&app, &intruder, &format!("/api/v1/properties/{own}/rooms"), room).await;
+
+    assert_eq!(into_their_property.status, StatusCode::NOT_FOUND, "{:?}", into_their_property.body);
+    assert_eq!(range_into_their_property.status, StatusCode::NOT_FOUND, "{:?}", range_into_their_property.body);
+    assert_eq!(section_in_their_property.status, StatusCode::NOT_FOUND, "{:?}", section_in_their_property.body);
+    assert_eq!(with_their_room_type.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", with_their_room_type.body);
+    let (rooms, sections): (i64, i64) =
+        sqlx::query_as("select (select count(*) from room), (select count(*) from housekeeping_section)")
+            .fetch_one(&superuser)
+            .await
+            .unwrap();
+    assert_eq!((rooms, sections), (0, 0));
+}
+
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn an_update_that_changes_nothing_is_a_422_and_keeps_the_version(_: PgPoolOptions, opts: PgConnectOptions) {
+    let app = TestApp::new(opts.clone()).await;
+    let superuser = PgPool::connect_with(opts).await.unwrap();
+    let (owner, property) = hotel(&app).await;
+    let room_type = post(&app, &owner, &format!("{property}/room-types"), deluxe()).await.body["id"].clone();
+    let room = post(&app, &owner, &format!("{property}/rooms"), json!({"room_type_id": room_type, "number": "101"}))
+        .await
+        .body["id"]
+        .clone();
+    let reason: Uuid =
+        sqlx::query_scalar("select id from block_reason where code = 'OTHER'").fetch_one(&superuser).await.unwrap();
+
+    let responses = [
+        patch(&app, &owner, &format!("{property}/room-types/{}", room_type.as_str().unwrap()), 1, json!({})).await,
+        patch(&app, &owner, &format!("{property}/rooms/{}", room.as_str().unwrap()), 1, json!({})).await,
+        patch(&app, &owner, &format!("{property}/block-reasons/{reason}"), 1, json!({"label": null})).await,
+    ];
+
+    for response in responses {
+        assert_eq!(response.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", response.body);
+        assert_eq!(response.body["detail"], "send at least one field to change");
+    }
+    let versions: (i32, i32, i32) = sqlx::query_as(
+        "select (select version from room_type), (select version from room),
+                (select version from block_reason where code = 'OTHER')",
+    )
+    .fetch_one(&superuser)
+    .await
+    .unwrap();
+    assert_eq!(versions, (1, 1, 1));
+}
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `DATABASE_URL=$TEST_DATABASE_URL cargo test -p core-api --no-fail-fast --test auth --test rooms --test properties`

Expected: `an_empty_update_is_a_422_and_keeps_the_version` and `an_update_that_changes_nothing_is_a_422_and_keeps_the_version` FAIL (`left: 200, right: 422`); `concurrent_failed_logins_cannot_exceed_the_limit` and `rooms_and_sections_cannot_be_added_to_another_tenant` pass (they pin existing behaviour).

- [ ] **Step 3: Implement**

Modify `crates/core-api/src/error.rs`:

```diff
diff --git a/crates/core-api/src/error.rs b/crates/core-api/src/error.rs
index 29b68b0..1bb47c6 100644
--- a/crates/core-api/src/error.rs
+++ b/crates/core-api/src/error.rs
@@ -126,3 +126,16 @@ impl IntoResponse for ApiError {
 pub fn validate<T: garde::Validate<Context = ()>>(value: &T) -> Result<(), ApiError> {
     value.validate().map_err(|report| ApiError::unprocessable(report.to_string()))
 }
+
+/// An update request whose fields are all optional.
+pub trait Changes {
+    /// True when the request names nothing to change.
+    fn is_empty(&self) -> bool;
+}
+
+/// Validates an update request like [`validate`], and refuses one that changes nothing, so an empty
+/// `PATCH` cannot bump the resource's version.
+pub fn validate_changes<T: garde::Validate<Context = ()> + Changes>(value: &T) -> Result<(), ApiError> {
+    validate(value)?;
+    if value.is_empty() { Err(ApiError::unprocessable("send at least one field to change")) } else { Ok(()) }
+}
```

Modify `crates/core-api/src/routes/blocks.rs`:

```diff
diff --git a/crates/core-api/src/routes/blocks.rs b/crates/core-api/src/routes/blocks.rs
index 125fa19..081863b 100644
--- a/crates/core-api/src/routes/blocks.rs
+++ b/crates/core-api/src/routes/blocks.rs
@@ -1,6 +1,6 @@
 use crate::auth::TenantContext;
 use crate::concurrency::{IfMatch, Versioned};
-use crate::error::{ApiError, validate};
+use crate::error::{ApiError, Changes, validate, validate_changes};
 use crate::extract::{ApiJson, ApiPath};
 use crate::routes::rooms::rooms_error;
 use crate::state::AppState;
@@ -38,6 +38,12 @@ pub struct UpdateBlockReasonRequest {
     pub active: Option<bool>,
 }
 
+impl Changes for UpdateBlockReasonRequest {
+    fn is_empty(&self) -> bool {
+        self.label.is_none() && self.default_kind.is_none() && self.active.is_none()
+    }
+}
+
 /// Blocks the room for `[from, to)`: `to` is the first day it is back in service.
 #[derive(Debug, Deserialize, Validate, ToSchema)]
 pub struct CreateBlockRequest {
@@ -96,7 +102,7 @@ pub async fn update_reason(
     ApiJson(body): ApiJson<UpdateBlockReasonRequest>,
 ) -> Result<Versioned<BlockReason>, ApiError> {
     ctx.require(Permission::RoomsManage, Some(property))?;
-    validate(&body)?;
+    validate_changes(&body)?;
     let changes = BlockReasonChanges { label: body.label, default_kind: body.default_kind, active: body.active };
     let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
     let updated = rooms::update_block_reason(&mut tx, ctx.tenant, ctx.user, property, reason, version, changes)
```

Modify `crates/core-api/src/routes/properties.rs`:

```diff
diff --git a/crates/core-api/src/routes/properties.rs b/crates/core-api/src/routes/properties.rs
index 9c73370..903ab3e 100644
--- a/crates/core-api/src/routes/properties.rs
+++ b/crates/core-api/src/routes/properties.rs
@@ -1,6 +1,6 @@
 use crate::auth::TenantContext;
 use crate::concurrency::{IfMatch, Versioned};
-use crate::error::{ApiError, validate};
+use crate::error::{ApiError, Changes, validate, validate_changes};
 use crate::extract::{ApiJson, ApiPath};
 use crate::state::AppState;
 use axum::extract::State;
@@ -39,6 +39,12 @@ pub struct UpdatePropertyRequest {
     pub check_out_time: Option<String>,
 }
 
+impl Changes for UpdatePropertyRequest {
+    fn is_empty(&self) -> bool {
+        self.name.is_none() && self.check_in_time.is_none() && self.check_out_time.is_none()
+    }
+}
+
 fn property_error(err: PropertyError) -> ApiError {
     match err {
         PropertyError::CodeTaken => ApiError::conflict(err.to_string()),
@@ -80,7 +86,7 @@ pub async fn update(
     ApiJson(body): ApiJson<UpdatePropertyRequest>,
 ) -> Result<Versioned<Property>, ApiError> {
     ctx.require(Permission::PropertiesManage, Some(property))?;
-    validate(&body)?;
+    validate_changes(&body)?;
     let changes =
         PropertyChanges { name: body.name, check_in_time: body.check_in_time, check_out_time: body.check_out_time };
     let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
```

Modify `crates/core-api/src/routes/room_types.rs`:

```diff
diff --git a/crates/core-api/src/routes/room_types.rs b/crates/core-api/src/routes/room_types.rs
index 5729f46..7973201 100644
--- a/crates/core-api/src/routes/room_types.rs
+++ b/crates/core-api/src/routes/room_types.rs
@@ -1,6 +1,6 @@
 use crate::auth::TenantContext;
 use crate::concurrency::{IfMatch, Versioned};
-use crate::error::{ApiError, validate};
+use crate::error::{ApiError, Changes, validate, validate_changes};
 use crate::extract::{ApiJson, ApiPath};
 use crate::routes::rooms::{ReorderRequest, rooms_error};
 use crate::state::AppState;
@@ -68,6 +68,19 @@ pub struct UpdateRoomTypeRequest {
     pub active: Option<bool>,
 }
 
+impl Changes for UpdateRoomTypeRequest {
+    fn is_empty(&self) -> bool {
+        self.name.is_none()
+            && self.base_occupancy.is_none()
+            && self.max_adults.is_none()
+            && self.max_children.is_none()
+            && self.max_occupancy.is_none()
+            && self.bed_config.is_none()
+            && self.amenities.is_none()
+            && self.active.is_none()
+    }
+}
+
 fn beds(requests: Vec<BedRequest>) -> Vec<Bed> {
     requests.into_iter().map(|bed| Bed { kind: bed.kind, count: bed.count }).collect()
 }
@@ -110,7 +123,7 @@ pub async fn update(
     ApiJson(body): ApiJson<UpdateRoomTypeRequest>,
 ) -> Result<Versioned<RoomType>, ApiError> {
     ctx.require(Permission::RoomsManage, Some(property))?;
-    validate(&body)?;
+    validate_changes(&body)?;
     let changes = RoomTypeChanges {
         name: body.name,
         base_occupancy: body.base_occupancy,
```

Modify `crates/core-api/src/routes/rooms.rs`:

```diff
diff --git a/crates/core-api/src/routes/rooms.rs b/crates/core-api/src/routes/rooms.rs
index b7bb17d..a4f7e87 100644
--- a/crates/core-api/src/routes/rooms.rs
+++ b/crates/core-api/src/routes/rooms.rs
@@ -1,6 +1,6 @@
 use crate::auth::TenantContext;
 use crate::concurrency::{IfMatch, Versioned};
-use crate::error::{ApiError, validate};
+use crate::error::{ApiError, Changes, validate, validate_changes};
 use crate::extract::{ApiJson, ApiPath};
 use crate::state::AppState;
 use axum::Json;
@@ -88,6 +88,16 @@ pub struct UpdateRoomRequest {
     pub active: Option<bool>,
 }
 
+impl Changes for UpdateRoomRequest {
+    fn is_empty(&self) -> bool {
+        self.room_type_id.is_none()
+            && self.number.is_none()
+            && self.floor.is_none()
+            && self.section_id.is_none()
+            && self.active.is_none()
+    }
+}
+
 #[derive(Debug, Deserialize, Validate, ToSchema)]
 pub struct SectionRequest {
     #[garde(length(chars, min = 1, max = 100))]
@@ -153,7 +163,7 @@ pub async fn update(
     ApiJson(body): ApiJson<UpdateRoomRequest>,
 ) -> Result<Versioned<Room>, ApiError> {
     ctx.require(Permission::RoomsManage, Some(property))?;
-    validate(&body)?;
+    validate_changes(&body)?;
     let changes = RoomChanges {
         room_type_id: body.room_type_id,
         number: body.number,
```

Modify `docs/design/api-conventions.md`:

```diff
diff --git a/docs/design/api-conventions.md b/docs/design/api-conventions.md
index c6368de..2eb3b18 100644
--- a/docs/design/api-conventions.md
+++ b/docs/design/api-conventions.md
@@ -85,6 +85,7 @@ Request DTOs derive `garde::Validate` and are checked with `error::validate(&bod
 
 - Editable resources return `version` in their body and an `ETag: "<version>"` header: handlers return `concurrency::Versioned::{ok, created}(version, body)` (`crates/core-api/src/concurrency.rs`). GraphQL nodes expose `version` too, which is where the SPA reads it.
 - Updates take the `concurrency::IfMatch` extractor, so they must send `If-Match: "<version>"`: missing is 428, not a quoted number is 400. The module's `update … where id = $1 and version = $2 … returning` finds no row on mismatch; it then checks whether the row exists and returns a version-mismatch error (412) or not-found (404). The client refetches and shows what changed.
+- An update that names no field to change is a 422 ("send at least one field to change"): update DTOs implement `error::Changes` and handlers check them with `error::validate_changes`, so an empty `PATCH` cannot bump the version.
 - Reordering (`PUT …/order`) is not a concurrent edit of one resource: it takes no `If-Match` and does not bump versions.
 
 ## Change events
```

- [ ] **Step 4: Run the checks**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p core-api
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: every core-api test passes; fmt and clippy are clean.

- [ ] **Step 5: Commit**

```sh
git add -A
git commit -m "fix(api): refuse an update that changes nothing; test concurrent sign-in throttling and cross-tenant room creates"
```

### Task 2: Replay the ETag of an idempotent create; declare ETag in the OpenAPI document

A retried create replayed its status, body and content type but not its `ETag`, so a client that read the
version from the header saw none. The response's `ETag` is now stored with it (migration 0005) and replayed.
Every versioned response also declares the header in its `#[utoipa::path]`, and a test pins which
operations do.

**Files:**
- Modify: `crates/core-api/src/idempotency.rs`
- Modify: `crates/core-api/src/routes/blocks.rs`
- Modify: `crates/core-api/src/routes/properties.rs`
- Modify: `crates/core-api/src/routes/room_types.rs`
- Modify: `crates/core-api/src/routes/rooms.rs`
- Test: `crates/core-api/tests/idempotency.rs`
- Test: `crates/core-api/tests/openapi.rs`
- Modify: `docs/design/api-conventions.md`
- Create: `migrations/0005_idempotency_etag.sql`

**Interfaces:**
- Consumes: `idempotency::idempotent`, `concurrency::Versioned`.
- Produces: column `idempotency_key.etag`; the header declaration `headers(("ETag" = String, description = "the version, e.g. \"1\"; send it back as If-Match"))` that Phase 2 routes copy; the list in `versioned_responses_declare_their_etag`, which later tasks extend.

- [ ] **Step 1: Write the failing tests**

Modify `crates/core-api/tests/idempotency.rs`:

```diff
diff --git a/crates/core-api/tests/idempotency.rs b/crates/core-api/tests/idempotency.rs
index 6d67a69..26b5288 100644
--- a/crates/core-api/tests/idempotency.rs
+++ b/crates/core-api/tests/idempotency.rs
@@ -1,6 +1,6 @@
 mod common;
 
-use axum::http::{Method, StatusCode, Uri};
+use axum::http::{Method, StatusCode, Uri, header};
 use common::{TestApp, TestResponse};
 use core_api::idempotency::request_hash;
 use db::{Scope, TenantId, UserId};
@@ -99,3 +99,16 @@ async fn a_server_error_is_not_stored_so_the_request_can_be_retried(_: PgPoolOpt
     assert_eq!(stored, 0);
     assert_eq!(retry.status, StatusCode::CREATED, "{:?}", retry.body);
 }
+
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn a_replayed_create_carries_the_etag_of_the_first_response(_: PgPoolOptions, opts: PgConnectOptions) {
+    let app = TestApp::new(opts).await;
+    let (cookie, _, _) = owner(&app).await;
+
+    let first = create(&app, &cookie, "/api/v1/properties", "key-00000001", galle()).await;
+    let replay = create(&app, &cookie, "/api/v1/properties", "key-00000001", galle()).await;
+
+    assert_eq!(first.headers[header::ETAG], "\"1\"");
+    assert_eq!(replay.status, StatusCode::CREATED);
+    assert_eq!(replay.headers.get(header::ETAG), first.headers.get(header::ETAG));
+}
```

Modify `crates/core-api/tests/openapi.rs`:

```diff
diff --git a/crates/core-api/tests/openapi.rs b/crates/core-api/tests/openapi.rs
index 1a77d0f..98c6c16 100644
--- a/crates/core-api/tests/openapi.rs
+++ b/crates/core-api/tests/openapi.rs
@@ -50,3 +50,42 @@ fn every_operation_has_its_own_id() {
     let unique: BTreeSet<&String> = ids.iter().collect();
     assert_eq!(unique.len(), ids.len(), "repeated operation ids in {ids:?}");
 }
+
+/// Responses that return a versioned resource send its version as `ETag`, and the document says so.
+#[test]
+fn versioned_responses_declare_their_etag() {
+    let doc = ApiDoc::openapi();
+    let mut declared: Vec<String> = doc
+        .paths
+        .paths
+        .values()
+        .flat_map(|item| [&item.get, &item.put, &item.post, &item.delete, &item.patch])
+        .flatten()
+        .filter(|operation| {
+            ["200", "201"].iter().any(|status| match operation.responses.responses.get(*status) {
+                Some(utoipa::openapi::RefOr::T(response)) => response.headers.contains_key("ETag"),
+                _ => false,
+            })
+        })
+        .map(|operation| operation.operation_id.clone().expect("every operation has an id"))
+        .collect();
+    declared.sort();
+
+    assert_eq!(
+        declared,
+        [
+            "create_block",
+            "create_block_reason",
+            "create_property",
+            "create_room",
+            "create_room_type",
+            "create_section",
+            "rename_section",
+            "shorten_block",
+            "update_block_reason",
+            "update_property",
+            "update_room",
+            "update_room_type",
+        ]
+    );
+}
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `DATABASE_URL=$TEST_DATABASE_URL cargo test -p core-api --no-fail-fast --test idempotency --test openapi`

Expected: `a_replayed_create_carries_the_etag_of_the_first_response` FAILS (`left: None, right: Some("\"1\"")`) and `versioned_responses_declare_their_etag` FAILS (`left: []`).

- [ ] **Step 3: Implement**

Modify `crates/core-api/src/idempotency.rs`:

```diff
diff --git a/crates/core-api/src/idempotency.rs b/crates/core-api/src/idempotency.rs
index 3079366..b7286ed 100644
--- a/crates/core-api/src/idempotency.rs
+++ b/crates/core-api/src/idempotency.rs
@@ -65,7 +65,7 @@ pub async fn idempotent(State(state): State<AppState>, request: Request, next: N
     .await?;
     let Some(claimed_at) = claimed_at else {
         let stored: Option<StoredClaim> = sqlx::query_as(
-            "select request_hash, status_code, content_type, response_body
+            "select request_hash, status_code, content_type, etag, response_body
              from idempotency_key where tenant_id = $1 and key = $2",
         )
         .bind(ctx.tenant.0)
@@ -89,7 +89,7 @@ pub async fn idempotent(State(state): State<AppState>, request: Request, next: N
     let stored = async {
         let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
         sqlx::query(
-            "update idempotency_key set status_code = $4, content_type = $5, response_body = $6
+            "update idempotency_key set status_code = $4, content_type = $5, etag = $6, response_body = $7
              where tenant_id = $1 and key = $2 and created_at = $3",
         )
         .bind(ctx.tenant.0)
@@ -97,6 +97,7 @@ pub async fn idempotent(State(state): State<AppState>, request: Request, next: N
         .bind(claimed_at)
         .bind(i16::try_from(response_parts.status.as_u16()).expect("HTTP status codes fit in i16"))
         .bind(response_parts.headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()))
+        .bind(response_parts.headers.get(header::ETAG).and_then(|v| v.to_str().ok()))
         .bind(body.to_vec())
         .execute(&mut *tx)
         .await?;
@@ -111,14 +112,14 @@ pub async fn idempotent(State(state): State<AppState>, request: Request, next: N
 }
 
 /// A claimed key's request hash, and its response once the request has finished:
-/// `(request_hash, status_code, content_type, response_body)`.
-type StoredClaim = (Vec<u8>, Option<i16>, Option<String>, Option<Vec<u8>>);
+/// `(request_hash, status_code, content_type, etag, response_body)`.
+type StoredClaim = (Vec<u8>, Option<i16>, Option<String>, Option<String>, Option<Vec<u8>>);
 
 /// Answers a request whose key is already claimed: replays the stored response, or reports why it cannot.
 fn replay(stored: Option<StoredClaim>, request_hash: &[u8]) -> Result<Response, ApiError> {
     let in_progress = || ApiError::conflict("a request with this Idempotency-Key is still in progress; retry shortly");
     // The claim vanished between the insert and the lookup: its request failed and released it.
-    let Some((stored_hash, status, content_type, body)) = stored else {
+    let Some((stored_hash, status, content_type, etag, body)) = stored else {
         return Err(in_progress());
     };
     if stored_hash != request_hash {
@@ -130,8 +131,10 @@ fn replay(stored: Option<StoredClaim>, request_hash: &[u8]) -> Result<Response,
     let status =
         u16::try_from(status).ok().and_then(|s| StatusCode::from_u16(s).ok()).ok_or_else(ApiError::internal)?;
     let mut replay = (status, body).into_response();
-    if let Some(content_type) = content_type.and_then(|v| header::HeaderValue::from_str(&v).ok()) {
-        replay.headers_mut().insert(header::CONTENT_TYPE, content_type);
+    for (name, value) in [(header::CONTENT_TYPE, content_type), (header::ETAG, etag)] {
+        if let Some(value) = value.and_then(|v| header::HeaderValue::from_str(&v).ok()) {
+            replay.headers_mut().insert(name, value);
+        }
     }
     Ok(replay)
 }
```

Modify `crates/core-api/src/routes/blocks.rs`:

```diff
diff --git a/crates/core-api/src/routes/blocks.rs b/crates/core-api/src/routes/blocks.rs
index 081863b..ad2c997 100644
--- a/crates/core-api/src/routes/blocks.rs
+++ b/crates/core-api/src/routes/blocks.rs
@@ -73,7 +73,8 @@ pub struct ShortenBlockRequest {
 
 #[utoipa::path(post, operation_id = "create_block_reason", path = "/api/v1/properties/{property}/block-reasons", request_body = CreateBlockReasonRequest,
     params(("property" = Uuid, Path), ("Idempotency-Key" = String, Header)),
-    responses((status = 201, body = BlockReason), (status = 403), (status = 404), (status = 409), (status = 422)))]
+    responses((status = 201, body = BlockReason,
+        headers(("ETag" = String, description = "the version, e.g. \"1\"; send it back as If-Match"))), (status = 403), (status = 404), (status = 409), (status = 422)))]
 pub async fn create_reason(
     State(state): State<AppState>,
     ctx: TenantContext,
@@ -93,7 +94,8 @@ pub async fn create_reason(
 #[utoipa::path(patch, operation_id = "update_block_reason", path = "/api/v1/properties/{property}/block-reasons/{reason}",
     request_body = UpdateBlockReasonRequest,
     params(("property" = Uuid, Path), ("reason" = Uuid, Path), ("If-Match" = String, Header)),
-    responses((status = 200, body = BlockReason), (status = 403), (status = 404), (status = 412), (status = 422), (status = 428)))]
+    responses((status = 200, body = BlockReason,
+        headers(("ETag" = String, description = "the version, e.g. \"1\"; send it back as If-Match"))), (status = 403), (status = 404), (status = 412), (status = 422), (status = 428)))]
 pub async fn update_reason(
     State(state): State<AppState>,
     ctx: TenantContext,
@@ -115,7 +117,8 @@ pub async fn update_reason(
 /// A 409 lists the blocks in the way as `conflicts`: full `Block` objects, as this endpoint returns them.
 #[utoipa::path(post, operation_id = "create_block", path = "/api/v1/properties/{property}/rooms/{room}/blocks", request_body = CreateBlockRequest,
     params(("property" = Uuid, Path), ("room" = Uuid, Path), ("Idempotency-Key" = String, Header)),
-    responses((status = 201, body = Block), (status = 403), (status = 404), (status = 409), (status = 422)))]
+    responses((status = 201, body = Block,
+        headers(("ETag" = String, description = "the version, e.g. \"1\"; send it back as If-Match"))), (status = 403), (status = 404), (status = 409), (status = 422)))]
 pub async fn create(
     State(state): State<AppState>,
     ctx: TenantContext,
@@ -140,7 +143,8 @@ pub async fn create(
 
 #[utoipa::path(patch, operation_id = "shorten_block", path = "/api/v1/properties/{property}/blocks/{block}", request_body = ShortenBlockRequest,
     params(("property" = Uuid, Path), ("block" = Uuid, Path), ("If-Match" = String, Header)),
-    responses((status = 200, body = Block), (status = 403), (status = 404), (status = 412), (status = 422), (status = 428)))]
+    responses((status = 200, body = Block,
+        headers(("ETag" = String, description = "the version, e.g. \"1\"; send it back as If-Match"))), (status = 403), (status = 404), (status = 412), (status = 422), (status = 428)))]
 pub async fn shorten(
     State(state): State<AppState>,
     ctx: TenantContext,
```

Modify `crates/core-api/src/routes/properties.rs`:

```diff
diff --git a/crates/core-api/src/routes/properties.rs b/crates/core-api/src/routes/properties.rs
index 903ab3e..34f9e23 100644
--- a/crates/core-api/src/routes/properties.rs
+++ b/crates/core-api/src/routes/properties.rs
@@ -57,7 +57,8 @@ fn property_error(err: PropertyError) -> ApiError {
 
 #[utoipa::path(post, operation_id = "create_property", path = "/api/v1/properties", request_body = CreatePropertyRequest,
     params(("Idempotency-Key" = String, Header)),
-    responses((status = 201, body = Property), (status = 403), (status = 409), (status = 422)))]
+    responses((status = 201, body = Property,
+        headers(("ETag" = String, description = "the version, e.g. \"1\"; send it back as If-Match"))), (status = 403), (status = 409), (status = 422)))]
 pub async fn create(
     State(state): State<AppState>,
     ctx: TenantContext,
@@ -77,7 +78,8 @@ pub async fn create(
 /// Changes a property's settings. The business date is not editable: night audit moves it.
 #[utoipa::path(patch, operation_id = "update_property", path = "/api/v1/properties/{property}", request_body = UpdatePropertyRequest,
     params(("property" = Uuid, Path), ("If-Match" = String, Header, description = "the version edited, e.g. \"3\"")),
-    responses((status = 200, body = Property), (status = 403), (status = 404), (status = 412), (status = 422), (status = 428)))]
+    responses((status = 200, body = Property,
+        headers(("ETag" = String, description = "the version, e.g. \"1\"; send it back as If-Match"))), (status = 403), (status = 404), (status = 412), (status = 422), (status = 428)))]
 pub async fn update(
     State(state): State<AppState>,
     ctx: TenantContext,
```

Modify `crates/core-api/src/routes/room_types.rs`:

```diff
diff --git a/crates/core-api/src/routes/room_types.rs b/crates/core-api/src/routes/room_types.rs
index 7973201..8bf1782 100644
--- a/crates/core-api/src/routes/room_types.rs
+++ b/crates/core-api/src/routes/room_types.rs
@@ -87,7 +87,8 @@ fn beds(requests: Vec<BedRequest>) -> Vec<Bed> {
 
 #[utoipa::path(post, operation_id = "create_room_type", path = "/api/v1/properties/{property}/room-types", request_body = CreateRoomTypeRequest,
     params(("property" = Uuid, Path), ("Idempotency-Key" = String, Header)),
-    responses((status = 201, body = RoomType), (status = 403), (status = 404), (status = 409), (status = 422)))]
+    responses((status = 201, body = RoomType,
+        headers(("ETag" = String, description = "the version, e.g. \"1\"; send it back as If-Match"))), (status = 403), (status = 404), (status = 409), (status = 422)))]
 pub async fn create(
     State(state): State<AppState>,
     ctx: TenantContext,
@@ -114,7 +115,8 @@ pub async fn create(
 
 #[utoipa::path(patch, operation_id = "update_room_type", path = "/api/v1/properties/{property}/room-types/{room_type}", request_body = UpdateRoomTypeRequest,
     params(("property" = Uuid, Path), ("room_type" = Uuid, Path), ("If-Match" = String, Header)),
-    responses((status = 200, body = RoomType), (status = 403), (status = 404), (status = 409), (status = 412), (status = 422), (status = 428)))]
+    responses((status = 200, body = RoomType,
+        headers(("ETag" = String, description = "the version, e.g. \"1\"; send it back as If-Match"))), (status = 403), (status = 404), (status = 409), (status = 412), (status = 422), (status = 428)))]
 pub async fn update(
     State(state): State<AppState>,
     ctx: TenantContext,
```

Modify `crates/core-api/src/routes/rooms.rs`:

```diff
diff --git a/crates/core-api/src/routes/rooms.rs b/crates/core-api/src/routes/rooms.rs
index a4f7e87..fd2a274 100644
--- a/crates/core-api/src/routes/rooms.rs
+++ b/crates/core-api/src/routes/rooms.rs
@@ -106,7 +106,8 @@ pub struct SectionRequest {
 
 #[utoipa::path(post, operation_id = "create_room", path = "/api/v1/properties/{property}/rooms", request_body = CreateRoomRequest,
     params(("property" = Uuid, Path), ("Idempotency-Key" = String, Header)),
-    responses((status = 201, body = Room), (status = 403), (status = 404), (status = 409), (status = 422)))]
+    responses((status = 201, body = Room,
+        headers(("ETag" = String, description = "the version, e.g. \"1\"; send it back as If-Match"))), (status = 403), (status = 404), (status = 409), (status = 422)))]
 pub async fn create(
     State(state): State<AppState>,
     ctx: TenantContext,
@@ -154,7 +155,8 @@ pub async fn create_range(
 
 #[utoipa::path(patch, operation_id = "update_room", path = "/api/v1/properties/{property}/rooms/{room}", request_body = UpdateRoomRequest,
     params(("property" = Uuid, Path), ("room" = Uuid, Path), ("If-Match" = String, Header)),
-    responses((status = 200, body = Room), (status = 403), (status = 404), (status = 409), (status = 412), (status = 422), (status = 428)))]
+    responses((status = 200, body = Room,
+        headers(("ETag" = String, description = "the version, e.g. \"1\"; send it back as If-Match"))), (status = 403), (status = 404), (status = 409), (status = 412), (status = 422), (status = 428)))]
 pub async fn update(
     State(state): State<AppState>,
     ctx: TenantContext,
@@ -198,7 +200,8 @@ pub async fn reorder(
 
 #[utoipa::path(post, operation_id = "create_section", path = "/api/v1/properties/{property}/sections", request_body = SectionRequest,
     params(("property" = Uuid, Path), ("Idempotency-Key" = String, Header)),
-    responses((status = 201, body = Section), (status = 403), (status = 404), (status = 409), (status = 422)))]
+    responses((status = 201, body = Section,
+        headers(("ETag" = String, description = "the version, e.g. \"1\"; send it back as If-Match"))), (status = 403), (status = 404), (status = 409), (status = 422)))]
 pub async fn create_section(
     State(state): State<AppState>,
     ctx: TenantContext,
@@ -216,7 +219,8 @@ pub async fn create_section(
 
 #[utoipa::path(patch, operation_id = "rename_section", path = "/api/v1/properties/{property}/sections/{section}", request_body = SectionRequest,
     params(("property" = Uuid, Path), ("section" = Uuid, Path), ("If-Match" = String, Header)),
-    responses((status = 200, body = Section), (status = 403), (status = 404), (status = 409), (status = 412), (status = 422), (status = 428)))]
+    responses((status = 200, body = Section,
+        headers(("ETag" = String, description = "the version, e.g. \"1\"; send it back as If-Match"))), (status = 403), (status = 404), (status = 409), (status = 412), (status = 422), (status = 428)))]
 pub async fn rename_section(
     State(state): State<AppState>,
     ctx: TenantContext,
```

Modify `docs/design/api-conventions.md`:

```diff
diff --git a/docs/design/api-conventions.md b/docs/design/api-conventions.md
index 2eb3b18..e26d493 100644
--- a/docs/design/api-conventions.md
+++ b/docs/design/api-conventions.md
@@ -67,7 +67,7 @@ Request DTOs derive `garde::Validate` and are checked with `error::validate(&bod
 
 - The client sends `Idempotency-Key: <8–200 chars>`, one fresh UUID per user action, reused only when retrying that same action.
 - Mount create routes in the `commands` router, which has `.route_layer(from_fn_with_state(state, idempotency::idempotent))`.
-- Same key, same request: the stored first response is replayed. Same key, different request: 422. The request hash covers the user, method, path, query string and body, so a different user or query with the same key is a different request.
+- Same key, same request: the stored first response is replayed, with its status, body, `Content-Type` and `ETag`. Same key, different request: 422. The request hash covers the user, method, path, query string and body, so a different user or query with the same key is a different request.
 - First request still running: 409, and the client retries. A claim is abandoned once it has been unfinished for 60 s (`routes::ABANDONED_CLAIM_AFTER`, the 15 s timeout plus margin), for example after a timeout or a dropped connection, and the next request with that key takes it over and runs.
 - A 5xx response, or one larger than 1 MiB, is not stored; its claim is released so the client may retry.
 - The SPA's create forms take keys from `formKeys()` (`web/pms/src/lib/api/rest.ts`): the same body gets the same key (a double-click or a retry replays instead of creating twice); an edited body gets a new key; `reset()` after a success starts fresh; `failed(err)` after a failure rotates the key on a definitive 4xx (so an unchanged resubmit is a new request, not a replay of the stored error) and keeps it after a network error, a 5xx or a 409 "still in progress", when the first request may still succeed.
@@ -83,7 +83,7 @@ Request DTOs derive `garde::Validate` and are checked with `error::validate(&bod
 
 ## Optimistic concurrency
 
-- Editable resources return `version` in their body and an `ETag: "<version>"` header: handlers return `concurrency::Versioned::{ok, created}(version, body)` (`crates/core-api/src/concurrency.rs`). GraphQL nodes expose `version` too, which is where the SPA reads it.
+- Editable resources return `version` in their body and an `ETag: "<version>"` header: handlers return `concurrency::Versioned::{ok, created}(version, body)` (`crates/core-api/src/concurrency.rs`). Their `#[utoipa::path]` success response declares the header (`headers(("ETag" = String, …))`); `tests/openapi.rs` lists the operations that do. GraphQL nodes expose `version` too, which is where the SPA reads it.
 - Updates take the `concurrency::IfMatch` extractor, so they must send `If-Match: "<version>"`: missing is 428, not a quoted number is 400. The module's `update … where id = $1 and version = $2 … returning` finds no row on mismatch; it then checks whether the row exists and returns a version-mismatch error (412) or not-found (404). The client refetches and shows what changed.
 - An update that names no field to change is a 422 ("send at least one field to change"): update DTOs implement `error::Changes` and handlers check them with `error::validate_changes`, so an empty `PATCH` cannot bump the version.
 - Reordering (`PUT …/order`) is not a concurrent edit of one resource: it takes no `If-Match` and does not bump versions.
```

Create `migrations/0005_idempotency_etag.sql`:

```sql
-- The original response's ETag, so a replayed create carries the created resource's version like the first response.
alter table idempotency_key add column etag text;
```

- [ ] **Step 4: Run the checks**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p core-api
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cd web/pms
bun run api:schemas && bun run codegen && git diff --exit-code -- src/lib/api
bun run lint && bun run check && bun run test && bun run build
```

Expected: all pass. `openapi.json` and `openapi.d.ts` change (the headers); commit them.

- [ ] **Step 5: Commit**

```sh
git add -A
git commit -m "fix(api): replay the ETag of an idempotent create and declare ETag headers in the OpenAPI document"
```

### Task 3: Rate permissions

Adds `RatesView` (every role) and `RatesManage` (owners and managers), and extends the permission-matrix test
and the table in api-conventions.md.

**Files:**
- Modify: `docs/design/api-conventions.md`
- Modify: `modules/identity/src/rbac.rs`
- Test: `modules/identity/tests/rbac.rs`

**Interfaces:**
- Produces: `identity::Permission::{RatesView, RatesManage}`, used by every rates route and resolver.

- [ ] **Step 1: Write the failing tests**

Modify `modules/identity/tests/rbac.rs`:

```diff
diff --git a/modules/identity/tests/rbac.rs b/modules/identity/tests/rbac.rs
index 9098d24..66051dc 100644
--- a/modules/identity/tests/rbac.rs
+++ b/modules/identity/tests/rbac.rs
@@ -39,14 +39,35 @@ fn roles_round_trip_through_their_database_names() {
 #[test]
 fn each_role_has_exactly_its_permissions() {
     use Permission::*;
-    let all =
-        [PropertiesView, PropertiesCreate, PropertiesManage, RoomsView, RoomsManage, InventoryView, InventoryBlock];
+    let all = [
+        PropertiesView,
+        PropertiesCreate,
+        PropertiesManage,
+        RoomsView,
+        RoomsManage,
+        InventoryView,
+        InventoryBlock,
+        RatesView,
+        RatesManage,
+    ];
     let expected: [(Role, &[Permission]); 5] = [
         (Role::Owner, &all),
-        (Role::Manager, &[PropertiesView, PropertiesManage, RoomsView, RoomsManage, InventoryView, InventoryBlock]),
-        (Role::FrontDesk, &[PropertiesView, RoomsView, InventoryView, InventoryBlock]),
-        (Role::Housekeeping, &[PropertiesView, RoomsView, InventoryView]),
-        (Role::Accountant, &[PropertiesView, RoomsView, InventoryView]),
+        (
+            Role::Manager,
+            &[
+                PropertiesView,
+                PropertiesManage,
+                RoomsView,
+                RoomsManage,
+                InventoryView,
+                InventoryBlock,
+                RatesView,
+                RatesManage,
+            ],
+        ),
+        (Role::FrontDesk, &[PropertiesView, RoomsView, InventoryView, InventoryBlock, RatesView]),
+        (Role::Housekeeping, &[PropertiesView, RoomsView, InventoryView, RatesView]),
+        (Role::Accountant, &[PropertiesView, RoomsView, InventoryView, RatesView]),
     ];
 
     for (role, permitted) in expected {
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `DATABASE_URL=$TEST_DATABASE_URL cargo test -p identity --test rbac`

Expected: fails to compile: ``cannot find value `RatesView` in this scope``.

- [ ] **Step 3: Implement**

Modify `docs/design/api-conventions.md`:

```diff
diff --git a/docs/design/api-conventions.md b/docs/design/api-conventions.md
index e26d493..f0426ea 100644
--- a/docs/design/api-conventions.md
+++ b/docs/design/api-conventions.md
@@ -27,10 +27,11 @@ Simple single-object reads that bootstrap the app (`GET /api/v1/me`) may be REST
 
   | Permission | Owner | Manager | Front desk | Housekeeping | Accountant |
   |---|---|---|---|---|---|
-  | `PropertiesView`, `RoomsView`, `InventoryView` | ✓ | ✓ | ✓ | ✓ | ✓ |
+  | `PropertiesView`, `RoomsView`, `InventoryView`, `RatesView` | ✓ | ✓ | ✓ | ✓ | ✓ |
   | `PropertiesCreate` | ✓ | | | | |
   | `PropertiesManage` (property settings), `RoomsManage` (room types, rooms, sections, block reasons) | ✓ | ✓ | | | |
   | `InventoryBlock` (block and release rooms) | ✓ | ✓ | ✓ | | |
+  | `RatesManage` (rate plans, prices, bulk changes, restrictions, meal supplements, cancellation policies) | ✓ | ✓ | | | |
 - Data access: `db::begin(&state.pool, Scope::tenant(ctx.tenant))` and never anything else. RLS is the safety net, but queries still filter by `property_id` explicitly.
 
 ## CSRF
```

Modify `modules/identity/src/rbac.rs`:

```diff
diff --git a/modules/identity/src/rbac.rs b/modules/identity/src/rbac.rs
index 2c86b9c..bed0b68 100644
--- a/modules/identity/src/rbac.rs
+++ b/modules/identity/src/rbac.rs
@@ -36,10 +36,21 @@ impl Role {
             Role::Owner => true,
             Role::Manager => matches!(
                 permission,
-                PropertiesView | PropertiesManage | RoomsView | RoomsManage | InventoryView | InventoryBlock
+                PropertiesView
+                    | PropertiesManage
+                    | RoomsView
+                    | RoomsManage
+                    | InventoryView
+                    | InventoryBlock
+                    | RatesView
+                    | RatesManage
             ),
-            Role::FrontDesk => matches!(permission, PropertiesView | RoomsView | InventoryView | InventoryBlock),
-            Role::Housekeeping | Role::Accountant => matches!(permission, PropertiesView | RoomsView | InventoryView),
+            Role::FrontDesk => {
+                matches!(permission, PropertiesView | RoomsView | InventoryView | InventoryBlock | RatesView)
+            }
+            Role::Housekeeping | Role::Accountant => {
+                matches!(permission, PropertiesView | RoomsView | InventoryView | RatesView)
+            }
         }
     }
 }
@@ -59,6 +70,10 @@ pub enum Permission {
     InventoryView,
     /// Block rooms and release or shorten blocks.
     InventoryBlock,
+    /// See rate plans, prices, restrictions, meal supplements and cancellation policies, and quote stays.
+    RatesView,
+    /// Create and change rate plans, prices, restrictions, meal supplements and cancellation policies.
+    RatesManage,
 }
 
 /// A role held tenant-wide (`property_id: None`) or for one property.
```

- [ ] **Step 4: Run the checks**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p identity
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: 5 rbac tests pass.

- [ ] **Step 5: Commit**

```sh
git add -A
git commit -m "feat(identity): rate permissions: every role views rates, owners and managers manage them"
```

### Task 4: Rates schema, the derivation function and tenant isolation

Migration 0006 creates `cancellation_policy`, `rate_plan`, `rate_plan_room_type`, `rate_day`,
`rate_restriction` and `meal_supplement`, each with forced RLS and composite keys into its property, and the
function `app.derive_amount`. Schema tests check the constraints (a derived plan and only a derived plan has a
parent and a formula; FIT segments carry their residency; a parent must be in the same property; prices and
restrictions exist only for sold room types and go with them; meal supplements cannot overlap; RO has no
rows), the function on worked examples, and the function against exact `i128` arithmetic on 5000 random
inputs per run. Six isolation cases join the suite. data-model.md records the Phase 2 tables as built.

**Files:**
- Modify: `crates/db/Cargo.toml`
- Test: `crates/db/tests/isolation.rs`
- Test: `crates/db/tests/rates_schema.rs` (new)
- Modify: `docs/design/data-model.md`
- Create: `migrations/0006_rates.sql`

**Interfaces:**
- Produces: the tables and constraint names used later (`rate_plan_property_id_code_key`, `meal_supplement_no_overlap`, `cancellation_policy_property_id_name_key`, `rate_restriction_check`); `app.derive_amount(base bigint, mode text, value bigint, step bigint) returns bigint`.

- [ ] **Step 1: Write the failing tests**

Add `proptest` as a dev-dependency of `db` (the workspace already has it), then the tests.

Modify `crates/db/Cargo.toml`:

```diff
diff --git a/crates/db/Cargo.toml b/crates/db/Cargo.toml
index 7b5710c..aabda18 100644
--- a/crates/db/Cargo.toml
+++ b/crates/db/Cargo.toml
@@ -17,6 +17,7 @@ testing = []
 
 [dev-dependencies]
 db = { workspace = true, features = ["testing"] }
+proptest.workspace = true
 tokio.workspace = true
 
 [lints]
```

Modify `crates/db/tests/isolation.rs`:

```diff
diff --git a/crates/db/tests/isolation.rs b/crates/db/tests/isolation.rs
index 05f876a..81eaaf0 100644
--- a/crates/db/tests/isolation.rs
+++ b/crates/db/tests/isolation.rs
@@ -448,3 +448,117 @@ async fn inventory_days_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnect
     assert_eq!(seen, 0);
     assert!(err.contains("row-level security"), "unexpected error: {err}");
 }
+
+/// One row in every Phase 2 table, on top of [`seed_rooms`], for `tenant`'s property.
+async fn seed_rates(pool: &PgPool, tenant: TenantId) {
+    let rooms = seed_rooms(pool, tenant).await;
+    let (policy, plan) = (Uuid::now_v7(), Uuid::now_v7());
+    let mut tx = begin(pool, Scope::tenant(tenant)).await.unwrap();
+    let statements = [
+        "insert into cancellation_policy (id, tenant_id, property_id, name, rules, no_show)
+         values ($4, $1, $2, 'Flexible', '[]', '{\"kind\": \"nights\", \"value\": 1}')",
+        "insert into rate_plan (id, tenant_id, property_id, code, name, kind, segment, currency, cancellation_policy_id)
+         values ($5, $1, $2, 'BAR', 'Best available', 'standard', 'IBE', 'USD', $4)",
+        "insert into rate_plan_room_type (tenant_id, property_id, rate_plan_id, room_type_id) values ($1, $2, $5, $3)",
+        "insert into rate_day (tenant_id, property_id, rate_plan_id, room_type_id, date, occupancy, amount)
+         values ($1, $2, $5, $3, current_date, 2, 12000)",
+        "insert into rate_restriction (tenant_id, property_id, rate_plan_id, room_type_id, date, min_stay)
+         values ($1, $2, $5, $3, current_date, 2)",
+        "insert into meal_supplement (id, tenant_id, property_id, meal_plan, currency, adult_amount, child_amount, valid)
+         values (gen_random_uuid(), $1, $2, 'BB', 'USD', 1500, 750, daterange(current_date, null))",
+    ];
+    for statement in statements {
+        sqlx::query(statement)
+            .bind(tenant.0)
+            .bind(rooms.property)
+            .bind(rooms.room_type)
+            .bind(policy)
+            .bind(plan)
+            .execute(&mut *tx)
+            .await
+            .unwrap();
+    }
+    tx.commit().await.unwrap();
+}
+
+/// Tenant `b` sees none of `a`'s rows in `table`, only its own, and cannot insert a row owned by `a` (`insert`
+/// binds `$1` to `a` and takes every other id from `b`'s own rows).
+async fn assert_rates_table_isolated(opts: PgConnectOptions, table: &str, insert: &'static str) {
+    let pool = app_pool(opts, 1).await;
+    let a = seed_tenant(&pool, "A").await;
+    let b = seed_tenant(&pool, "B").await;
+    seed_rates(&pool, a).await;
+    seed_rates(&pool, b).await;
+
+    let seen = visible_rows(&pool, b, table).await;
+    let err = foreign_insert_error(&pool, b, a, insert).await;
+
+    assert_eq!(seen, 1, "B sees only its own {table} row");
+    assert!(err.contains("row-level security"), "unexpected error: {err}");
+}
+
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn cancellation_policies_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
+    assert_rates_table_isolated(
+        opts,
+        "cancellation_policy",
+        "insert into cancellation_policy (id, tenant_id, property_id, name, rules, no_show)
+         select gen_random_uuid(), $1, id, 'Strict', '[]', '{}' from property",
+    )
+    .await;
+}
+
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn rate_plans_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
+    assert_rates_table_isolated(
+        opts,
+        "rate_plan",
+        "insert into rate_plan (id, tenant_id, property_id, code, name, kind, segment, currency)
+         select gen_random_uuid(), $1, id, 'OTA', 'OTA', 'custom', 'OTA', 'USD' from property",
+    )
+    .await;
+}
+
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn rate_plan_room_types_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
+    assert_rates_table_isolated(
+        opts,
+        "rate_plan_room_type",
+        "insert into rate_plan_room_type (tenant_id, property_id, rate_plan_id, room_type_id)
+         select $1, p.property_id, p.id, t.id from rate_plan p, room_type t",
+    )
+    .await;
+}
+
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn rate_days_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
+    assert_rates_table_isolated(
+        opts,
+        "rate_day",
+        "insert into rate_day (tenant_id, property_id, rate_plan_id, room_type_id, date, occupancy, amount)
+         select $1, property_id, rate_plan_id, room_type_id, current_date + 1, 1, 9000 from rate_plan_room_type",
+    )
+    .await;
+}
+
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn rate_restrictions_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
+    assert_rates_table_isolated(
+        opts,
+        "rate_restriction",
+        "insert into rate_restriction (tenant_id, property_id, rate_plan_id, room_type_id, date, closed)
+         select $1, property_id, rate_plan_id, room_type_id, current_date + 1, true from rate_plan_room_type",
+    )
+    .await;
+}
+
+#[sqlx::test(migrator = "db::MIGRATOR")]
+async fn meal_supplements_are_isolated_by_tenant(_: PgPoolOptions, opts: PgConnectOptions) {
+    assert_rates_table_isolated(
+        opts,
+        "meal_supplement",
+        "insert into meal_supplement (id, tenant_id, property_id, meal_plan, currency, adult_amount, child_amount, valid)
+         select gen_random_uuid(), $1, id, 'HB', 'USD', 3000, 1500, daterange(current_date, null) from property",
+    )
+    .await;
+}
```

Create `crates/db/tests/rates_schema.rs`:

```rust
//! Constraints the Phase 2 migration puts on rate plans, prices and meal supplements, and the price
//! derivation function, checked directly in the database.

use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::TestRunner;
use sqlx::PgPool;
use uuid::Uuid;

struct Hotel {
    tenant: Uuid,
    property: Uuid,
    room_type: Uuid,
}

/// A tenant with one property and one room type. Runs as the superuser (no RLS).
async fn hotel(pool: &PgPool, code: &str) -> Hotel {
    let hotel = Hotel { tenant: Uuid::now_v7(), property: Uuid::now_v7(), room_type: Uuid::now_v7() };
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
    hotel
}

/// Inserts a rate plan; `parent` makes it a derived plan (+10%).
async fn plan(
    pool: &PgPool,
    hotel: &Hotel,
    code: &str,
    kind: &str,
    parent: Option<Uuid>,
    segment: &str,
    residency: Option<&str>,
) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::now_v7();
    let (mode, value) = if kind == "derived" { (Some("percent"), Some(1000_i64)) } else { (None, None) };
    sqlx::query(
        "insert into rate_plan (id, tenant_id, property_id, code, name, kind, segment, residency, currency, parent_id,
                                derive_mode, derive_value)
         values ($1, $2, $3, $4, $4, $5, $6, $7, 'USD', $8, $9, $10)",
    )
    .bind(id)
    .bind(hotel.tenant)
    .bind(hotel.property)
    .bind(code)
    .bind(kind)
    .bind(segment)
    .bind(residency)
    .bind(parent)
    .bind(mode)
    .bind(value)
    .execute(pool)
    .await?;
    Ok(id)
}

fn constraint(result: Result<impl std::fmt::Debug, sqlx::Error>) -> String {
    let err = result.unwrap_err();
    err.as_database_error().and_then(|db_err| db_err.constraint()).unwrap_or_default().to_owned()
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_derived_plan_and_only_a_derived_plan_has_a_parent(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;
    let bar = plan(&pool, &hotel, "BAR", "standard", None, "IBE", None).await.unwrap();

    let orphan = plan(&pool, &hotel, "OTA", "derived", None, "OTA", None).await;
    let standard_with_parent = plan(&pool, &hotel, "TA", "standard", Some(bar), "TA", None).await;
    let derived = plan(&pool, &hotel, "OTA", "derived", Some(bar), "OTA", None).await;

    assert_eq!(constraint(orphan), "rate_plan_derivation_check");
    assert_eq!(constraint(standard_with_parent), "rate_plan_derivation_check");
    assert!(derived.is_ok(), "{derived:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn fit_segments_carry_their_residency(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;

    let foreign_for_residents = plan(&pool, &hotel, "FITF", "custom", None, "FIT_F", Some("resident")).await;
    let local_for_anyone = plan(&pool, &hotel, "FITL", "custom", None, "FIT_L", None).await;
    let foreign = plan(&pool, &hotel, "FITF", "custom", None, "FIT_F", Some("non_resident")).await;
    let local = plan(&pool, &hotel, "FITL", "custom", None, "FIT_L", Some("resident")).await;

    assert_eq!(constraint(foreign_for_residents), "rate_plan_segment_residency_check");
    assert_eq!(constraint(local_for_anyone), "rate_plan_segment_residency_check");
    assert!(foreign.is_ok() && local.is_ok());
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_plan_cannot_derive_from_another_propertys_plan(pool: PgPool) {
    let galle = hotel(&pool, "GAL").await;
    let kandy = hotel(&pool, "KAN").await;
    let theirs = plan(&pool, &kandy, "BAR", "standard", None, "IBE", None).await.unwrap();

    let derived = plan(&pool, &galle, "OTA", "derived", Some(theirs), "OTA", None).await;

    assert_eq!(constraint(derived), "rate_plan_property_id_parent_id_fkey");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn prices_and_restrictions_exist_only_for_room_types_the_plan_sells(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;
    let bar = plan(&pool, &hotel, "BAR", "standard", None, "IBE", None).await.unwrap();
    let price = "insert into rate_day (tenant_id, property_id, rate_plan_id, room_type_id, date, occupancy, amount)
                 values ($1, $2, $3, $4, current_date, 2, 15000)";
    let restriction = "insert into rate_restriction (tenant_id, property_id, rate_plan_id, room_type_id, date, closed)
                       values ($1, $2, $3, $4, current_date, true)";
    let insert = |sql: &'static str| {
        sqlx::query(sql).bind(hotel.tenant).bind(hotel.property).bind(bar).bind(hotel.room_type).execute(&pool)
    };

    let unsold_price = insert(price).await;
    let unsold_restriction = insert(restriction).await;
    sqlx::query(
        "insert into rate_plan_room_type (tenant_id, property_id, rate_plan_id, room_type_id) values ($1, $2, $3, $4)",
    )
    .bind(hotel.tenant)
    .bind(hotel.property)
    .bind(bar)
    .bind(hotel.room_type)
    .execute(&pool)
    .await
    .unwrap();
    insert(price).await.unwrap();
    insert(restriction).await.unwrap();
    sqlx::query("delete from rate_plan_room_type").execute(&pool).await.unwrap();
    let left: (i64, i64) =
        sqlx::query_as("select (select count(*) from rate_day), (select count(*) from rate_restriction)")
            .fetch_one(&pool)
            .await
            .unwrap();

    assert_eq!(constraint(unsold_price), "rate_day_property_id_rate_plan_id_room_type_id_fkey");
    assert_eq!(constraint(unsold_restriction), "rate_restriction_property_id_rate_plan_id_room_type_id_fkey");
    assert_eq!(left, (0, 0), "no longer selling a room type removes its prices and restrictions");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn meal_supplements_for_one_meal_plan_and_currency_cannot_overlap(pool: PgPool) {
    let hotel = hotel(&pool, "GAL").await;
    let supplement = |meal_plan: &'static str, currency: &'static str, from: i32, to: Option<i32>| {
        sqlx::query(
            "insert into meal_supplement (id, tenant_id, property_id, meal_plan, currency, adult_amount, child_amount, valid)
             values ($1, $2, $3, $4, $5, 1500, 750, daterange(current_date + $6, current_date + $7))",
        )
        .bind(Uuid::now_v7())
        .bind(hotel.tenant)
        .bind(hotel.property)
        .bind(meal_plan)
        .bind(currency)
        .bind(from)
        .bind(to)
        .execute(&pool)
    };
    supplement("BB", "USD", 0, None).await.unwrap();

    let overlapping = supplement("BB", "USD", 30, Some(60)).await;
    let other_currency = supplement("BB", "LKR", 30, Some(60)).await;
    let other_meal_plan = supplement("HB", "USD", 30, Some(60)).await;
    let room_only = supplement("RO", "USD", 0, None).await;

    assert_eq!(constraint(overlapping), "meal_supplement_no_overlap");
    assert!(other_currency.is_ok() && other_meal_plan.is_ok());
    assert_eq!(constraint(room_only), "meal_supplement_meal_plan_check");
}

/// `app.derive_amount(base, mode, value, step)` computed exactly: `base` plus `value` basis points
/// (`percent`) or minor units (`amount`), at least 0, rounded half-up to a multiple of `step`.
fn derived(base: i64, mode: &str, value: i64, step: i64) -> i64 {
    let (numerator, denominator) = match mode {
        "percent" => (i128::from(base) * (10_000 + i128::from(value)), 10_000_i128),
        _ => (i128::from(base) + i128::from(value), 1),
    };
    let unit = denominator * i128::from(step);
    let (whole, rest) = (numerator.max(0) / unit, numerator.max(0) % unit);
    let steps = if 2 * rest >= unit { whole + 1 } else { whole };
    i64::try_from(steps * i128::from(step)).unwrap()
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn derive_amount_rounds_half_up_to_the_step(pool: PgPool) {
    let cases: [(i64, &str, i64, i64, i64); 8] = [
        (10_000, "percent", 1_500, 1, 11_500),   // +15%
        (10_050, "percent", 1_500, 100, 11_600), // 115.575 rounds to 116.00
        (1_000, "percent", 500, 100, 1_100),     // 10.50 is a half: up to 11.00
        (10_000, "percent", -500, 1, 9_500),     // -5%
        (10_000, "percent", -10_000, 1, 0),      // -100%
        (10_000, "amount", -2_550, 100, 7_500),  // 74.50 is a half: up to 75.00
        (1_000, "amount", -5_000, 1, 0),         // never below zero
        (0, "amount", 0, 100, 0),
    ];
    for (base, mode, value, step, expected) in cases {
        let actual: i64 = sqlx::query_scalar("select app.derive_amount($1, $2, $3, $4)")
            .bind(base)
            .bind(mode)
            .bind(value)
            .bind(step)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(actual, expected, "{base} {mode} {value} step {step}");
        assert_eq!(derived(base, mode, value, step), expected, "the test's own model");
    }
}

/// Random inputs across the whole allowed range, compared with exact arithmetic. Each run draws new ones.
#[sqlx::test(migrator = "db::MIGRATOR")]
async fn derive_amount_matches_exact_arithmetic(pool: PgPool) {
    let step = prop_oneof![Just(1_i64), Just(5), Just(10), Just(50), Just(100), Just(1_000), 1..=1_000_000_i64];
    let case = prop_oneof![
        (0..=100_000_000_000_i64, -10_000..=100_000_i64, step.clone()).prop_map(|(b, v, s)| (b, "percent", v, s)),
        (0..=100_000_000_000_i64, -100_000_000_000..=100_000_000_000_i64, step)
            .prop_map(|(b, v, s)| (b, "amount", v, s)),
    ];
    let mut runner = TestRunner::default();
    let cases: Vec<(i64, &str, i64, i64)> = (0..5_000).map(|_| case.new_tree(&mut runner).unwrap().current()).collect();

    let actual: Vec<i64> = sqlx::query_scalar(
        "select app.derive_amount(c.base, c.mode, c.value, c.step)
         from unnest($1::bigint[], $2::text[], $3::bigint[], $4::bigint[]) with ordinality as c (base, mode, value, step, n)
         order by c.n",
    )
    .bind(cases.iter().map(|c| c.0).collect::<Vec<_>>())
    .bind(cases.iter().map(|c| c.1).collect::<Vec<_>>())
    .bind(cases.iter().map(|c| c.2).collect::<Vec<_>>())
    .bind(cases.iter().map(|c| c.3).collect::<Vec<_>>())
    .fetch_all(&pool)
    .await
    .unwrap();

    for (case, actual) in cases.iter().zip(actual) {
        assert_eq!(actual, derived(case.0, case.1, case.2, case.3), "app.derive_amount{case:?}");
    }
}
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `DATABASE_URL=$TEST_DATABASE_URL cargo test -p db --test rates_schema --test isolation`

Expected: all 7 `rates_schema` tests and the 6 new isolation cases FAIL: the tables and `app.derive_amount` do not exist (`relation "rate_plan" does not exist`, `relation "cancellation_policy" does not exist`, `function app.derive_amount(bigint, text, bigint, bigint) does not exist`); the 14 existing isolation cases pass.

- [ ] **Step 3: Implement**

Modify `docs/design/data-model.md`:

```diff
diff --git a/docs/design/data-model.md b/docs/design/data-model.md
index e6694a4..ae74c08 100644
--- a/docs/design/data-model.md
+++ b/docs/design/data-model.md
@@ -1,6 +1,6 @@
 # Data Model
 
-The reference schema for every phase. Phase 0 tables exist in `migrations/0001_foundation.sql`, Phase 1 tables in `migrations/0003_login_throttle.sql` and `migrations/0004_rooms_inventory.sql`. Tables for later phases are the target design: the migrations that create them are written in their phase and may refine columns, but must keep the rules below.
+The reference schema for every phase. Phase 0 tables exist in `migrations/0001_foundation.sql`, Phase 1 tables in `migrations/0003_login_throttle.sql` and `migrations/0004_rooms_inventory.sql`, Phase 2 tables in `migrations/0006_rates.sql`. Tables for later phases are the target design: the migrations that create them are written in their phase and may refine columns, but must keep the rules below.
 
 Related: [ARCHITECTURE.md](../ARCHITECTURE.md) (why), [api-conventions.md](api-conventions.md) (how data leaves the API).
 
@@ -49,15 +49,20 @@ Extensions: `btree_gist` (needed by the exclusion constraints).
 |---|---|---|
 | `login_failure` | `id`, `email citext`, `at` | Sign-in throttling (`migrations/0003_login_throttle.sql`). No RLS and no `tenant_id`: looked up before a tenant is known. One row per attempt, written before the password is checked; a successful sign-in deletes the email's rows. The Phase 7 purge job deletes rows older than the window |
 
-## Phase 2: Rates and meal plans
+## Phase 2: Rates and meal plans (implemented)
+
+Created by `migrations/0006_rates.sql` (`0005_idempotency_etag.sql` adds `idempotency_key.etag`, so a replayed create carries its `ETag`). Every table carries `tenant_id` and `property_id` and references its property through `(tenant_id, property_id)`; prices and restrictions reference the plan's room type through `(property_id, rate_plan_id, room_type_id)`. Amounts are minor units in the plan's (or supplement's) currency, at most 100 000 000 000.
 
 | Table | Key columns | Constraints and indexes |
 |---|---|---|
-| `rate_plan` | `id`, `tenant_id`, `property_id`, `code`, `name`, `kind` (`standard` \| `derived` \| `custom`), `segment` (`FIT_F` \| `FIT_L` \| `OTA` \| `TA` \| `IBE`), `residency` (`resident` \| `non_resident` \| null = any), `currency`, `parent_id null`, `derive_mode` (`percent` \| `amount`), `derive_value bigint` (basis points or minor units), `rounding_step bigint`, `inherit_restrictions bool`, `allowed_meal_plans text[]`, `cancellation_policy_id`, `active`, `version` | `unique (property_id, code)`; `check ((kind = 'derived') = (parent_id is not null))`. Same currency as the parent (enforced in the module, tested). Derivation depth ≤ 3; no cycles |
-| `rate_plan_room_type` | `(rate_plan_id, room_type_id)`, `tenant_id` | Which room types a plan sells |
-| `rate_day` | `(rate_plan_id, room_type_id, date, occupancy)`, `tenant_id`, `property_id`, `amount bigint`, `closed bool`, `min_stay`, `max_stay`, `closed_to_arrival`, `closed_to_departure` | **Resolved** prices, derived plans included. Recomputed in the same transaction as the parent change. Index `(property_id, date)` for grids and search |
-| `meal_supplement` | `id`, `tenant_id`, `property_id`, `meal_plan` (`RO` \| `BB` \| `HB` \| `FB`), `currency`, `adult_amount`, `child_amount`, `valid daterange` | Per person per night on top of the room price. `RO` is always 0. Exclusion: no overlapping `valid` for the same `(property_id, meal_plan, currency)` |
-| `cancellation_policy` | `id`, `tenant_id`, `property_id`, `name`, `rules jsonb` | Rules: list of `{days_before_arrival, penalty: {kind: nights\|percent\|amount, value}}`; `no_show` penalty |
+| `rate_plan` | `id`, `tenant_id`, `property_id`, `code`, `name`, `kind` (`standard` \| `derived` \| `custom`), `segment` (`FIT_F` \| `FIT_L` \| `OTA` \| `TA` \| `IBE`), `residency` (`resident` \| `non_resident` \| null = any), `currency`, `parent_id null`, `derive_mode` (`percent` \| `amount`), `derive_value bigint` (basis points or minor units), `rounding_step bigint`, `extra_adult_amount bigint`, `inherit_restrictions bool`, `allowed_meal_plans text[]`, `cancellation_policy_id null`, `active`, `version` | `unique (property_id, code)`; `rate_plan_derivation_check`: a plan has a parent and a formula exactly when it is derived, percent within −100 % … +1000 %; `rate_plan_segment_residency_check`: `FIT_F` is `non_resident`, `FIT_L` is `resident`. Same currency as the parent, derivation depth ≤ 3, no cycles, and only standard or derived plans as parents: enforced in the `rates` module, tested |
+| `rate_plan_room_type` | `(rate_plan_id, room_type_id)`, `tenant_id`, `property_id` | Which room types a plan sells. A derived plan sells a subset of its parent's. Removing a type deletes the plan's prices and restrictions for it (`on delete cascade`) |
+| `rate_day` | `(rate_plan_id, date, room_type_id, occupancy)`, `tenant_id`, `property_id`, `amount bigint` | **Resolved** prices, derived plans included; `occupancy` is the number of adults (1 … 50). Recomputed in the same transaction as the parent change (one `insert … select … on conflict do update` per level, through `app.derive_amount`). Index `(property_id, date)` for grids and search |
+| `rate_restriction` | `(rate_plan_id, date, room_type_id)`, `tenant_id`, `property_id`, `closed bool`, `min_stay null`, `max_stay null`, `closed_to_arrival`, `closed_to_departure` | Restrictions per plan, room type and date (the target design kept them on `rate_day`; they apply to every occupancy, so they have their own row). **Resolved**: a derived plan with `inherit_restrictions` holds a copy of its parent's rows, rewritten with them. Index `(property_id, date)` |
+| `meal_supplement` | `id`, `tenant_id`, `property_id`, `meal_plan` (`BB` \| `HB` \| `FB`), `currency`, `adult_amount`, `child_amount`, `valid daterange`, `version` | Per person per night on top of the room price. `RO` is always 0 and has no rows. `valid` has a lower bound; no upper bound means until further notice. Exclusion `meal_supplement_no_overlap`: no overlapping `valid` for the same `(property_id, meal_plan, currency)` |
+| `cancellation_policy` | `id`, `tenant_id`, `property_id`, `name`, `rules jsonb`, `no_show jsonb`, `version` | `unique (property_id, name)`. `rules`: list of `{days_before_arrival, penalty: {kind: nights\|percent\|amount, value}}` (percent in basis points); `no_show`: one penalty |
+
+`app.derive_amount(base, mode, value, step)` changes a price by `value` basis points (`percent`) or minor units (`amount`), never below 0, rounded half-up to a multiple of `step`, in integer arithmetic. Derived plans apply their formula to the parent's price with it, and bulk changes apply theirs to a plan's own prices.
 
 ## Phase 3: Reservations and guests
 
```

Create `migrations/0006_rates.sql`:

```sql
-- Phase 2: rate plans, resolved daily prices and restrictions, meal supplements and cancellation policies.
-- Amounts are bigint minor units in the plan's (or supplement's) currency, at most 100 000 000 000.

-- A price changed by `value` basis points (`percent`) or minor units (`amount`), either possibly negative,
-- never below 0, rounded half-up to a multiple of `step`. Derived plans apply their formula to the parent's
-- price with it, and bulk changes apply theirs to a plan's own prices. Integer arithmetic only: every input
-- is bounded by the checks below, so nothing overflows. Mirrored in web/pms/src/lib/rates.ts for previews.
create function app.derive_amount(base bigint, mode text, value bigint, step bigint) returns bigint
language sql immutable strict parallel safe
as $$
  select case mode
    when 'percent' then (2 * greatest(base * (10000 + value), 0) + 10000 * step) / (20000 * step) * step
    when 'amount' then (2 * greatest(base + value, 0) + step) / (2 * step) * step
  end
$$;

-- Penalties for cancelling: `rules` is a list of {"days_before_arrival": n, "penalty": {"kind", "value"}},
-- `no_show` one penalty. Kinds: `nights` (value = nights), `percent` (basis points of the stay), `amount`
-- (minor units). The rates module checks the shape; reservations (Phase 3) apply them.
create table cancellation_policy (
  id uuid primary key,
  tenant_id uuid not null,
  property_id uuid not null,
  name text not null check (length(name) between 1 and 100),
  rules jsonb not null check (jsonb_typeof(rules) = 'array'),
  no_show jsonb not null check (jsonb_typeof(no_show) = 'object'),
  version integer not null default 1,
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade,
  unique (property_id, name),
  unique (property_id, id)
);

-- Standard plans are priced by hand and may have derived plans; derived plans are priced from their parent
-- (same currency, at most 3 levels below a standard plan, no cycles: checked by the rates module); custom plans
-- are priced by hand and stand alone.
create table rate_plan (
  id uuid primary key,
  tenant_id uuid not null,
  property_id uuid not null,
  code text not null check (code ~ '^[A-Z0-9_-]{1,20}$'),
  name text not null check (length(name) between 1 and 100),
  kind text not null check (kind in ('standard', 'derived', 'custom')),
  segment text not null check (segment in ('FIT_F', 'FIT_L', 'OTA', 'TA', 'IBE')),
  residency text check (residency in ('resident', 'non_resident')),
  currency char(3) not null check (currency ~ '^[A-Z]{3}$'),
  parent_id uuid,
  derive_mode text check (derive_mode in ('percent', 'amount')),
  derive_value bigint check (derive_value between -100000000000 and 100000000000),
  rounding_step bigint not null default 1 check (rounding_step between 1 and 100000000),
  extra_adult_amount bigint not null default 0 check (extra_adult_amount between 0 and 100000000000),
  inherit_restrictions boolean not null default false,
  allowed_meal_plans text[] not null default '{RO}'
    check (cardinality(allowed_meal_plans) > 0 and allowed_meal_plans <@ array['RO', 'BB', 'HB', 'FB']),
  cancellation_policy_id uuid,
  active boolean not null default true,
  version integer not null default 1,
  created_at timestamptz not null default now(),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade,
  foreign key (property_id, parent_id) references rate_plan (property_id, id),
  foreign key (property_id, cancellation_policy_id) references cancellation_policy (property_id, id),
  unique (property_id, code),
  unique (property_id, id),
  constraint rate_plan_derivation_check check (
    (kind = 'derived') = (parent_id is not null)
    and (kind = 'derived') = (derive_mode is not null and derive_value is not null)
    and parent_id is distinct from id
    and (derive_mode is distinct from 'percent' or derive_value between -10000 and 100000)
  ),
  -- FIT-F is sold only to non-residents and FIT-L only to residents.
  constraint rate_plan_segment_residency_check check (
    (segment <> 'FIT_F' or residency is not distinct from 'non_resident')
    and (segment <> 'FIT_L' or residency is not distinct from 'resident')
  )
);
create index rate_plan_parent_idx on rate_plan (parent_id);

-- The room types a plan sells. Prices and restrictions exist only for these, and go when a type is removed.
create table rate_plan_room_type (
  tenant_id uuid not null,
  property_id uuid not null,
  rate_plan_id uuid not null,
  room_type_id uuid not null,
  primary key (rate_plan_id, room_type_id),
  unique (property_id, rate_plan_id, room_type_id),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade,
  foreign key (property_id, rate_plan_id) references rate_plan (property_id, id) on delete cascade,
  foreign key (property_id, room_type_id) references room_type (property_id, id) on delete cascade
);

-- Resolved prices per plan, room type, date and occupancy (adults), derived plans included: a write to a
-- plan's prices recomputes its descendants' rows in the same transaction, so reads never derive anything.
create table rate_day (
  tenant_id uuid not null,
  property_id uuid not null,
  rate_plan_id uuid not null,
  room_type_id uuid not null,
  date date not null,
  occupancy integer not null check (occupancy between 1 and 50),
  amount bigint not null check (amount between 0 and 100000000000),
  primary key (rate_plan_id, date, room_type_id, occupancy),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade,
  foreign key (property_id, rate_plan_id, room_type_id)
    references rate_plan_room_type (property_id, rate_plan_id, room_type_id) on delete cascade
);
create index rate_day_property_date_idx on rate_day (property_id, date);

-- Resolved restrictions per plan, room type and date. A derived plan that inherits restrictions gets a copy
-- of its parent's rows in the same transaction as every change to them.
create table rate_restriction (
  tenant_id uuid not null,
  property_id uuid not null,
  rate_plan_id uuid not null,
  room_type_id uuid not null,
  date date not null,
  closed boolean not null default false,
  min_stay integer check (min_stay between 1 and 365),
  max_stay integer check (max_stay between 1 and 365),
  closed_to_arrival boolean not null default false,
  closed_to_departure boolean not null default false,
  primary key (rate_plan_id, date, room_type_id),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade,
  foreign key (property_id, rate_plan_id, room_type_id)
    references rate_plan_room_type (property_id, rate_plan_id, room_type_id) on delete cascade,
  check (min_stay is null or max_stay is null or min_stay <= max_stay)
);
create index rate_restriction_property_date_idx on rate_restriction (property_id, date);

-- Per person per night on top of the room price, for [lower(valid), upper(valid)); no upper bound means
-- until further notice. Room only (RO) is always 0, so it has no rows.
create table meal_supplement (
  id uuid primary key,
  tenant_id uuid not null,
  property_id uuid not null,
  meal_plan text not null check (meal_plan in ('BB', 'HB', 'FB')),
  currency char(3) not null check (currency ~ '^[A-Z]{3}$'),
  adult_amount bigint not null check (adult_amount between 0 and 100000000000),
  child_amount bigint not null check (child_amount between 0 and 100000000000),
  valid daterange not null check (not isempty(valid) and not lower_inf(valid)),
  version integer not null default 1,
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade,
  constraint meal_supplement_no_overlap
    exclude using gist (property_id with =, meal_plan with =, currency with =, valid with &&)
);

do $$
declare t text;
begin
  foreach t in array array['cancellation_policy', 'rate_plan', 'rate_plan_room_type', 'rate_day', 'rate_restriction',
                           'meal_supplement'] loop
    execute format('alter table %I enable row level security', t);
    execute format('alter table %I force row level security', t);
    execute format(
      'create policy tenant_isolation on %I for all using (tenant_id = (select app.current_tenant())) with check (tenant_id = (select app.current_tenant()))',
      t);
  end loop;
end $$;
```

- [ ] **Step 4: Run the checks**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p db
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: `rates_schema` 7 passed, `isolation` 20 passed, and the schema guard (`every_tenant_table_has_forced_row_level_security`) still passes.

- [ ] **Step 5: Commit**

```sh
git add -A
git commit -m "feat(db): rate plans, resolved prices and restrictions, meal supplements and cancellation policies, with tenant isolation"
```

### Task 5: The rates module: rate plans as a checked tree

The `rates` crate starts with rate plans: create, update and list as a tree (each standard or custom plan by
code, then the plans derived from it, depth first, with `depth`). The rules: only derived plans have a
parent, a formula and inherited restrictions; a parent is standard or derived, in the same currency, at most
3 levels below a standard plan, and never the plan itself or one of its descendants; a derived plan sells a
subset of its parent's room types, and a parent cannot drop a type a child still sells; FIT segments fix the
residency. Writes take `lock_rates` and check the rules on the tree held in memory. `rooms::months` is
extracted so rate keys reuse the month iteration. Besides example tests, a property-based test draws random
sequences of "derive" and "move" and checks each is accepted exactly when a small model of the rules says so.

**Files:**
- Modify: `Cargo.toml`
- Create: `modules/rates/Cargo.toml`
- Create: `modules/rates/src/lib.rs`
- Create: `modules/rates/src/plans.rs`
- Test: `modules/rates/tests/common/mod.rs` (new)
- Test: `modules/rates/tests/plan_tree.rs` (new)
- Test: `modules/rates/tests/plans.rs` (new)
- Modify: `modules/rooms/src/inventory.rs`
- Modify: `modules/rooms/src/lib.rs`

**Interfaces:**
- Consumes: `rooms::WINDOW_DAYS`, `rooms::months`, `property::create_property` and `rooms::create_room_type` (tests).
- Produces: `rates::{PlanKind, Segment, Residency, ChangeMode, MealPlan}` (text enums with `as_str`/`parse`), `RatePlan` (with `depth`, `room_type_ids`), `NewRatePlan`, `RatePlanChanges`, `create_rate_plan(tx, tenant, actor, property, NewRatePlan)`, `update_rate_plan(tx, tenant, actor, property, id, expected_version, RatePlanChanges)`, `list_rate_plans(tx, property)`, `RatesError::{NotFound, VersionMismatch, Conflict, Invalid, Database}`, `rate_plans_key`, `rates_keys`, `MAX_DEPTH`; crate-internal `Tree`, `lock_rates`, `notify` (chunked).

- [ ] **Step 1: Write the failing tests**

Register the crate (workspace dependency and manifest) and create an empty `modules/rates/src/lib.rs`, so the tests compile far enough to fail; then the tests.

Modify `Cargo.toml`:

```diff
diff --git a/Cargo.toml b/Cargo.toml
index b5da363..451ff57 100644
--- a/Cargo.toml
+++ b/Cargo.toml
@@ -11,6 +11,7 @@ publish = false
 db = { path = "crates/db" }
 identity = { path = "modules/identity" }
 property = { path = "modules/property" }
+rates = { path = "modules/rates" }
 rooms = { path = "modules/rooms" }
 
 anyhow = "1"
```

Create `modules/rates/Cargo.toml`:

```toml
[package]
name = "rates"
version = "0.1.0"
edition.workspace = true
rust-version.workspace = true
publish.workspace = true

[dependencies]
db.workspace = true
rooms.workspace = true
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
proptest.workspace = true
tokio.workspace = true

[lints]
workspace = true
```

Create `modules/rates/tests/common/mod.rs`:

```rust
#![allow(dead_code)] // each test binary uses a different subset

use db::testing::app_pool;
use db::{Scope, TenantId, Tx, UserId, begin};
use rates::{ChangeMode, MealPlan, NewRatePlan, PlanKind, RatePlan, RatesError, Segment};
use rooms::{NewRoomType, RoomType};
use sqlx::PgPool;
use sqlx::postgres::PgConnectOptions;
use time::{Date, Duration};
use uuid::Uuid;

/// A tenant with one user, one property and two room types: `DLX` (up to 2 adults and 1 child) and `STD`
/// (up to 2 adults).
pub struct Hotel {
    pub pool: PgPool,
    pub tenant: TenantId,
    pub user: UserId,
    pub property: Uuid,
    pub business_date: Date,
    pub deluxe: RoomType,
    pub standard: RoomType,
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
        let deluxe = NewRoomType {
            code: "DLX".into(),
            name: "Deluxe".into(),
            base_occupancy: 2,
            max_adults: 2,
            max_children: 1,
            max_occupancy: 3,
            bed_config: vec![],
            amenities: vec![],
        };
        let standard = NewRoomType {
            code: "STD".into(),
            name: "Standard".into(),
            base_occupancy: 1,
            max_adults: 2,
            max_children: 0,
            max_occupancy: 2,
            ..deluxe.clone()
        };
        let deluxe = rooms::create_room_type(&mut tx, tenant, user, property.id, deluxe).await.unwrap();
        let standard = rooms::create_room_type(&mut tx, tenant, user, property.id, standard).await.unwrap();
        tx.commit().await.unwrap();
        Self { pool, tenant, user, property: property.id, business_date: property.business_date, deluxe, standard }
    }

    pub async fn tx(&self) -> Tx {
        begin(&self.pool, Scope::tenant(self.tenant)).await.unwrap()
    }

    /// The business date plus `days`.
    pub fn day(&self, days: i64) -> Date {
        self.business_date + Duration::days(days)
    }

    /// A standard plan in `currency` selling both room types.
    pub fn standard_plan(&self, code: &str, currency: &str) -> NewRatePlan {
        NewRatePlan {
            code: code.into(),
            name: format!("Plan {code}"),
            kind: PlanKind::Standard,
            segment: Segment::Ibe,
            residency: None,
            currency: currency.into(),
            parent_id: None,
            derive_mode: None,
            derive_value: None,
            rounding_step: 1,
            extra_adult_amount: 0,
            inherit_restrictions: false,
            allowed_meal_plans: vec![MealPlan::Ro, MealPlan::Bb],
            cancellation_policy_id: None,
            room_type_ids: vec![self.deluxe.id, self.standard.id],
        }
    }

    /// A plan derived from `parent` by `value` of `mode`, in the parent's currency and room types.
    pub fn derived_plan(&self, code: &str, parent: &RatePlan, mode: ChangeMode, value: i64) -> NewRatePlan {
        NewRatePlan {
            kind: PlanKind::Derived,
            segment: Segment::Ota,
            parent_id: Some(parent.id),
            derive_mode: Some(mode),
            derive_value: Some(value),
            room_type_ids: parent.room_type_ids.clone(),
            ..self.standard_plan(code, &parent.currency)
        }
    }

    /// Creates a plan in its own transaction, committed if it succeeds.
    pub async fn try_plan(&self, input: NewRatePlan) -> Result<RatePlan, RatesError> {
        let mut tx = self.tx().await;
        let created = rates::create_rate_plan(&mut tx, self.tenant, self.user, self.property, input).await?;
        tx.commit().await.unwrap();
        Ok(created)
    }

    pub async fn plan(&self, input: NewRatePlan) -> RatePlan {
        self.try_plan(input).await.unwrap()
    }

    /// Changes a plan in its own transaction, committed if it succeeds.
    pub async fn try_update(&self, plan: &RatePlan, changes: rates::RatePlanChanges) -> Result<RatePlan, RatesError> {
        let mut tx = self.tx().await;
        let updated =
            rates::update_rate_plan(&mut tx, self.tenant, self.user, self.property, plan.id, plan.version, changes)
                .await?;
        tx.commit().await.unwrap();
        Ok(updated)
    }

    pub async fn plans(&self) -> Vec<RatePlan> {
        rates::list_rate_plans(&mut self.tx().await, self.property).await.unwrap()
    }
}
```

Create `modules/rates/tests/plan_tree.rs`:

```rust
//! Property-based check of the derivation rules: random sequences of "derive a plan" and "move a plan to
//! another parent" must be accepted exactly when a simple model of the rules says so (same currency, a
//! standard or derived parent, at most 3 levels, no cycles), and every accepted tree must keep the rules.

mod common;

use common::Hotel;
use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::TestRunner;
use rates::{ChangeMode, NewRatePlan, PlanKind, RatePlan, RatePlanChanges, RatesError};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::collections::HashMap;
use uuid::Uuid;

/// Sequences tried per run. Each run draws new ones; a failure prints the sequence that broke a rule.
const SEQUENCES: usize = 12;

#[derive(Debug, Clone)]
enum Op {
    /// Derive a new plan from plan `parent`, in its currency or deliberately in the other one.
    Derive { parent: usize, other_currency: bool },
    /// Move plan `plan` under plan `parent`.
    Move { plan: usize, parent: usize },
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (0..12usize, prop::bool::weighted(0.2))
            .prop_map(|(parent, other_currency)| Op::Derive { parent, other_currency }),
        (0..12usize, 0..12usize).prop_map(|(plan, parent)| Op::Move { plan, parent }),
    ]
}

/// The rules, on plans held in memory: id to (kind, currency, parent).
struct Model(HashMap<Uuid, (PlanKind, String, Option<Uuid>)>);

impl Model {
    fn depth(&self, id: Uuid) -> usize {
        let mut depth = 0;
        let mut current = self.0[&id].2;
        while let Some(parent) = current {
            depth += 1;
            current = self.0[&parent].2;
        }
        depth
    }

    /// Levels of derived plans below `id`.
    fn height(&self, id: Uuid) -> usize {
        self.0
            .iter()
            .filter(|(_, plan)| plan.2 == Some(id))
            .map(|(child, _)| 1 + self.height(*child))
            .max()
            .unwrap_or(0)
    }

    fn is_below(&self, id: Uuid, ancestor: Uuid) -> bool {
        let mut current = Some(id);
        while let Some(plan) = current {
            if plan == ancestor {
                return true;
            }
            current = self.0[&plan].2;
        }
        false
    }

    fn may_derive(&self, parent: Uuid, currency: &str, height: usize) -> bool {
        let (kind, parent_currency, _) = &self.0[&parent];
        *kind != PlanKind::Custom && parent_currency == currency && self.depth(parent) + 1 + height <= 3
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn random_derivations_and_moves_follow_the_rules(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let roots = [
        hotel.plan(hotel.standard_plan("USD1", "USD")).await,
        hotel.plan(hotel.standard_plan("USD2", "USD")).await,
        hotel.plan(hotel.standard_plan("LKR1", "LKR")).await,
        hotel.plan(NewRatePlan { kind: PlanKind::Custom, ..hotel.standard_plan("CUSTOM", "USD") }).await,
    ];
    let mut runner = TestRunner::default();

    for sequence in 0..SEQUENCES {
        let ops = prop::collection::vec(op(), 1..16).new_tree(&mut runner).unwrap().current();
        let mut plans: Vec<RatePlan> = roots.to_vec();
        plans.extend(hotel.plans().await.into_iter().filter(|plan| plan.kind == PlanKind::Derived));
        let mut model = Model(plans.iter().map(|p| (p.id, (p.kind, p.currency.clone(), p.parent_id))).collect());

        for (step, op) in ops.iter().enumerate() {
            let context = format!("sequence {sequence}, step {step} of {ops:?}");
            match *op {
                Op::Derive { parent, other_currency } => {
                    let parent = plans[parent % plans.len()].clone();
                    let currency = match (other_currency, parent.currency.as_str()) {
                        (false, currency) => currency.to_owned(),
                        (true, "USD") => "LKR".into(),
                        (true, _) => "USD".into(),
                    };
                    let expected = model.may_derive(parent.id, &currency, 0);
                    let code = format!("D{sequence}-{step}");
                    let input =
                        NewRatePlan { currency, ..hotel.derived_plan(&code, &parent, ChangeMode::Percent, 500) };
                    match hotel.try_plan(input).await {
                        Ok(created) => {
                            assert!(expected, "accepted against the rules: {context}");
                            model.0.insert(created.id, (created.kind, created.currency.clone(), created.parent_id));
                            plans.push(created);
                        }
                        Err(RatesError::Invalid(_)) => assert!(!expected, "refused a valid derivation: {context}"),
                        Err(err) => panic!("{err:?}: {context}"),
                    }
                }
                Op::Move { plan, parent } => {
                    let (plan, parent) = (plans[plan % plans.len()].clone(), plans[parent % plans.len()].clone());
                    if plan.kind != PlanKind::Derived {
                        continue;
                    }
                    let expected = !model.is_below(parent.id, plan.id)
                        && model.may_derive(parent.id, &plan.currency, model.height(plan.id));
                    let changes = RatePlanChanges { parent_id: Some(parent.id), ..RatePlanChanges::default() };
                    match hotel.try_update(&plan, changes).await {
                        Ok(moved) => {
                            assert!(expected, "accepted against the rules: {context}");
                            model.0.get_mut(&moved.id).unwrap().2 = moved.parent_id;
                        }
                        Err(RatesError::Invalid(_)) => assert!(!expected, "refused a valid move: {context}"),
                        Err(err) => panic!("{err:?}: {context}"),
                    }
                    // Versions change with every move; refresh them.
                    let fresh = hotel.plans().await;
                    for plan in &mut plans {
                        *plan = fresh.iter().find(|p| p.id == plan.id).unwrap().clone();
                    }
                }
            }

            let listed = hotel.plans().await;
            assert_eq!(listed.len(), model.0.len(), "every plan is in the tree: {context}");
            for plan in listed {
                assert!(plan.depth <= 3, "{} is {} levels deep: {context}", plan.code, plan.depth);
                assert_eq!(usize::try_from(plan.depth).unwrap(), model.depth(plan.id), "{context}");
                if let Some(parent) = plan.parent_id {
                    assert_eq!(model.0[&parent].1, plan.currency, "currency of {}: {context}", plan.code);
                    assert_ne!(model.0[&parent].0, PlanKind::Custom, "{context}");
                }
            }
        }
    }
}
```

Create `modules/rates/tests/plans.rs`:

```rust
mod common;

use common::Hotel;
use rates::{ChangeMode, PlanKind, RatePlanChanges, RatesError, Residency, Segment};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

fn invalid<T: std::fmt::Debug>(result: Result<T, RatesError>) -> String {
    match result {
        Err(RatesError::Invalid(message)) => message,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn plans_are_listed_as_a_tree_with_their_room_types(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let bar = hotel.plan(hotel.standard_plan("BAR", "USD")).await;
    let ota = hotel.plan(hotel.derived_plan("OTA", &bar, ChangeMode::Percent, 1_500)).await;
    let ota_nr = hotel.plan(hotel.derived_plan("OTA-NR", &ota, ChangeMode::Amount, -1_000)).await;
    let custom = rates::NewRatePlan {
        kind: PlanKind::Custom,
        segment: Segment::Ta,
        room_type_ids: vec![hotel.deluxe.id],
        ..hotel.standard_plan("TA", "USD")
    };
    hotel.plan(custom).await;
    let local = rates::NewRatePlan { segment: Segment::FitL, ..hotel.standard_plan("FITL", "LKR") };
    let local = hotel.plan(local).await;

    let listed = hotel.plans().await;

    let tree: Vec<(&str, i32)> = listed.iter().map(|plan| (plan.code.as_str(), plan.depth)).collect();
    assert_eq!(tree, [("BAR", 0), ("OTA", 1), ("OTA-NR", 2), ("FITL", 0), ("TA", 0)]);
    assert_eq!(ota_nr.parent_id, Some(ota.id));
    assert_eq!(ota_nr.derive_mode, Some(ChangeMode::Amount));
    assert_eq!(ota_nr.derive_value, Some(-1_000));
    assert_eq!(listed[4].room_type_ids, [hotel.deluxe.id]);
    assert_eq!(bar.room_type_ids, [hotel.deluxe.id, hotel.standard.id]);
    assert_eq!(local.residency, Some(Residency::Resident), "FIT-L is sold to residents");
    assert_eq!((bar.version, bar.active), (1, true));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn derivation_is_at_most_three_levels_deep(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let bar = hotel.plan(hotel.standard_plan("BAR", "USD")).await;
    let one = hotel.plan(hotel.derived_plan("L1", &bar, ChangeMode::Percent, 1_000)).await;
    let two = hotel.plan(hotel.derived_plan("L2", &one, ChangeMode::Percent, 1_000)).await;
    let three = hotel.plan(hotel.derived_plan("L3", &two, ChangeMode::Percent, 1_000)).await;

    let four = hotel.try_plan(hotel.derived_plan("L4", &three, ChangeMode::Percent, 1_000)).await;

    assert_eq!(three.depth, 3);
    assert_eq!(invalid(four), "derived plans are at most 3 levels below a standard plan");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_derived_plan_uses_its_parents_currency(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let bar = hotel.plan(hotel.standard_plan("BAR", "USD")).await;
    let local = hotel.plan(hotel.standard_plan("LOCAL", "LKR")).await;
    let ota = hotel.plan(hotel.derived_plan("OTA", &bar, ChangeMode::Percent, 1_500)).await;

    let in_rupees = hotel.try_plan(rates::NewRatePlan {
        currency: "LKR".into(),
        ..hotel.derived_plan("OTA-LKR", &bar, ChangeMode::Percent, 1_500)
    });
    let moved_to_rupees =
        hotel.try_update(&ota, RatePlanChanges { parent_id: Some(local.id), ..RatePlanChanges::default() });

    let expected =
        "a derived plan uses its parent's currency (USD); resident prices in another currency are set by hand";
    assert_eq!(invalid(in_rupees.await), expected);
    assert!(invalid(moved_to_rupees.await).starts_with("a derived plan uses its parent's currency (LKR)"));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn re_parenting_cannot_make_a_cycle_or_go_too_deep(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let bar = hotel.plan(hotel.standard_plan("BAR", "USD")).await;
    let rack = hotel.plan(hotel.standard_plan("RACK", "USD")).await;
    let ota = hotel.plan(hotel.derived_plan("OTA", &bar, ChangeMode::Percent, 1_500)).await;
    let ota_nr = hotel.plan(hotel.derived_plan("OTA-NR", &ota, ChangeMode::Percent, -500)).await;
    let ibe = hotel.plan(hotel.derived_plan("IBE", &rack, ChangeMode::Percent, -500)).await;
    let ibe_2 = hotel.plan(hotel.derived_plan("IBE2", &ibe, ChangeMode::Percent, -500)).await;
    let reparent = |to: Uuid| RatePlanChanges { parent_id: Some(to), ..RatePlanChanges::default() };

    let under_its_child = hotel.try_update(&ota, reparent(ota_nr.id)).await;
    let under_itself = hotel.try_update(&ota, reparent(ota.id)).await;
    let too_deep = hotel.try_update(&ota, reparent(ibe_2.id)).await;
    let moved = hotel.try_update(&ota, reparent(ibe.id)).await.unwrap();

    assert_eq!(invalid(under_its_child), "OTA cannot derive from OTA-NR: that would make a cycle");
    assert_eq!(invalid(under_itself), "OTA cannot derive from OTA: that would make a cycle");
    assert_eq!(invalid(too_deep), "derived plans are at most 3 levels below a standard plan");
    assert_eq!((moved.parent_id, moved.depth, moved.version), (Some(ibe.id), 2, 2));
    let depths: Vec<(String, i32)> = hotel.plans().await.into_iter().map(|p| (p.code, p.depth)).collect();
    assert!(depths.contains(&("OTA-NR".into(), 3)), "{depths:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn custom_plans_stand_alone(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let custom = rates::NewRatePlan { kind: PlanKind::Custom, ..hotel.standard_plan("CORP", "USD") };
    let custom = hotel.plan(custom).await;

    let from_custom = hotel.try_plan(hotel.derived_plan("CORP-10", &custom, ChangeMode::Percent, -1_000)).await;
    let custom_with_parent = hotel
        .try_plan(rates::NewRatePlan {
            kind: PlanKind::Custom,
            ..hotel.derived_plan("CORP-5", &custom, ChangeMode::Percent, -500)
        })
        .await;
    let inheriting_standard =
        hotel.try_plan(rates::NewRatePlan { inherit_restrictions: true, ..hotel.standard_plan("BAR", "USD") }).await;

    assert_eq!(invalid(from_custom), "CORP is a custom plan; only standard and derived plans have derived plans");
    assert_eq!(invalid(custom_with_parent), "only derived plans have a parent and a formula");
    assert_eq!(invalid(inheriting_standard), "only derived plans inherit restrictions");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn fit_segments_set_and_check_residency(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let foreign = rates::NewRatePlan { segment: Segment::FitF, ..hotel.standard_plan("FITF", "USD") };
    let foreign = hotel.plan(foreign).await;
    let bar = hotel.plan(hotel.standard_plan("BAR", "LKR")).await;

    let residents_on_fit_f = hotel
        .try_plan(rates::NewRatePlan {
            segment: Segment::FitF,
            residency: Some(Residency::Resident),
            ..hotel.standard_plan("FITF2", "USD")
        })
        .await;
    let now_local =
        hotel.try_update(&bar, RatePlanChanges { segment: Some(Segment::FitL), ..RatePlanChanges::default() }).await;

    assert_eq!(foreign.residency, Some(Residency::NonResident));
    assert_eq!(invalid(residents_on_fit_f), "FIT_F plans are sold to non-residents only");
    assert_eq!(now_local.unwrap().residency, Some(Residency::Resident));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_derived_plan_sells_only_room_types_its_parent_sells(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let bar = hotel.plan(hotel.standard_plan("BAR", "USD")).await;
    let ota = hotel.plan(rates::NewRatePlan {
        room_type_ids: vec![hotel.deluxe.id],
        ..hotel.derived_plan("OTA", &bar, ChangeMode::Percent, 1_500)
    });
    let ota = ota.await;
    let deluxe_only = RatePlanChanges { room_type_ids: Some(vec![hotel.deluxe.id]), ..RatePlanChanges::default() };
    let standard_only = RatePlanChanges { room_type_ids: Some(vec![hotel.standard.id]), ..RatePlanChanges::default() };

    let bar = hotel.try_update(&bar, deluxe_only).await.unwrap();
    let dropping_a_type_the_child_sells = hotel.try_update(&bar, standard_only.clone()).await;
    let child_adds_an_unsold_type = hotel.try_update(&ota, standard_only).await;
    let unknown_type = hotel
        .try_plan(rates::NewRatePlan { room_type_ids: vec![Uuid::now_v7()], ..hotel.standard_plan("X", "USD") })
        .await;

    assert_eq!(bar.room_type_ids, [hotel.deluxe.id]);
    assert!(
        matches!(&dropping_a_type_the_child_sells, Err(RatesError::Conflict(m)) if m == "OTA still sells DLX; remove it there first"),
        "{dropping_a_type_the_child_sells:?}"
    );
    assert_eq!(invalid(child_adds_an_unsold_type), "OTA can only sell room types its parent sells, not STD");
    assert_eq!(invalid(unknown_type), "no such active room type in this property");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn codes_are_unique_and_updates_need_the_current_version(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let bar = hotel.plan(hotel.standard_plan("BAR", "USD")).await;
    let rename = |name: &str| RatePlanChanges { name: Some(name.into()), ..RatePlanChanges::default() };

    let duplicate = hotel.try_plan(hotel.standard_plan("BAR", "LKR")).await;
    let renamed = hotel.try_update(&bar, rename("Best available")).await.unwrap();
    let stale = hotel.try_update(&bar, rename("Rack")).await;
    let unknown = hotel.try_update(&rates::RatePlan { id: Uuid::now_v7(), ..bar.clone() }, rename("X")).await;
    let mut tx = hotel.tx().await;
    let elsewhere =
        rates::create_rate_plan(&mut tx, hotel.tenant, hotel.user, Uuid::now_v7(), hotel.standard_plan("X", "USD"))
            .await;

    assert!(matches!(&duplicate, Err(RatesError::Conflict(m)) if m == "a rate plan with code BAR already exists"));
    assert_eq!((renamed.name.as_str(), renamed.version, renamed.code.as_str()), ("Best available", 2, "BAR"));
    assert!(matches!(stale, Err(RatesError::VersionMismatch("rate plan"))), "{stale:?}");
    assert!(matches!(unknown, Err(RatesError::NotFound("rate plan"))), "{unknown:?}");
    assert!(matches!(elsewhere, Err(RatesError::NotFound("property"))), "{elsewhere:?}");
}
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `DATABASE_URL=$TEST_DATABASE_URL cargo test -p rates`

Expected: fails to compile: ``unresolved imports `rates::ChangeMode`, `rates::MealPlan`, `rates::NewRatePlan` …`` and ``cannot find function `create_rate_plan` in crate `rates` ``.

- [ ] **Step 3: Implement**

Replace the empty `lib.rs`:

Create `modules/rates/src/lib.rs`:

```rust
//! Rate plans with their resolved daily prices and restrictions, meal supplements and cancellation policies.
//!
//! Every function takes a transaction scoped to the caller's tenant and checks that ids belong to the given
//! property. Writes record an audit entry and queue change events in the same transaction. Writes to a
//! property's plans, prices and restrictions first take [`lock_rates`]: a change to one plan cascades to the
//! plans derived from it, and one lock per property is simpler than a lock order across all their rows.

/// A `text` column with a fixed set of values, mirrored as an enum with `as_str` and `parse`.
/// Values are `tt`, not `literal`: a `literal` fragment reaches derive macros wrapped, so utoipa would miss
/// the `serde(rename)` and document the variant names instead of the values.
macro_rules! text_enum {
    ($(#[$meta:meta])* $name:ident { $($(#[$variant_meta:meta])* $variant:ident = $text:tt),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, utoipa::ToSchema)]
        pub enum $name {
            $($(#[$variant_meta])* #[serde(rename = $text)] $variant),+
        }

        impl $name {
            pub fn as_str(self) -> &'static str {
                match self {
                    $($name::$variant => $text),+
                }
            }

            pub fn parse(value: &str) -> Option<$name> {
                match value {
                    $($text => Some($name::$variant),)+
                    _ => None,
                }
            }
        }
    };
}

mod plans;

pub use plans::{
    ChangeMode, MealPlan, NewRatePlan, PlanKind, RatePlan, RatePlanChanges, Residency, Segment, create_rate_plan,
    list_rate_plans, update_rate_plan,
};

use db::{Event, TenantId, Tx, UserId};
use time::Date;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum RatesError {
    /// The property or the named resource does not exist in this tenant.
    #[error("{0} not found")]
    NotFound(&'static str),
    /// `If-Match` named an older version of the named resource.
    #[error("the {0} was changed by someone else; reload and try again")]
    VersionMismatch(&'static str),
    /// A uniqueness rule, such as a duplicate code, or a change that other data depends on.
    #[error("{0}")]
    Conflict(String),
    /// A business rule, such as a derivation in another currency or a date outside the rate window.
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// Cache key for a property's rate plans, meal supplements and cancellation policies.
pub fn rate_plans_key(property: Uuid) -> String {
    format!("rate-plans:{property}")
}

/// Cache keys `rates:<property>:<plan>:<yyyy-mm>` for every month of `[from, to)`.
pub fn rates_keys(property: Uuid, plan: Uuid, from: Date, to: Date) -> Vec<String> {
    rooms::months(from, to).into_iter().map(|month| format!("rates:{property}:{plan}:{month}")).collect()
}

/// The property's business date. `NotFound` if the property is not in this tenant.
async fn business_date(tx: &mut Tx, property: Uuid) -> Result<Date, RatesError> {
    sqlx::query_scalar("select business_date from property where id = $1")
        .bind(property)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(RatesError::NotFound("property"))
}

/// Serializes writes to one property's rate plans, prices and restrictions until the transaction ends.
async fn lock_rates(tx: &mut Tx, property: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("select pg_advisory_xact_lock(hashtextextended('rates:' || $1::text, 0))")
        .bind(property)
        .execute(&mut **tx)
        .await?;
    Ok(())
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

/// Bytes of keys per event, well under Postgres' 8000-byte NOTIFY payload with the ids around them.
const EVENT_KEY_BYTES: usize = 6000;

/// `keys` split into batches of at most [`EVENT_KEY_BYTES`] (counting quotes and commas), in order.
fn event_batches(keys: Vec<String>) -> Vec<Vec<String>> {
    let mut batches: Vec<Vec<String>> = Vec::new();
    let mut size = 0;
    for key in keys {
        let key_size = key.len() + 3;
        match batches.last_mut() {
            Some(batch) if size + key_size <= EVENT_KEY_BYTES => batch.push(key),
            _ => {
                size = 0;
                batches.push(vec![key]);
            }
        }
        size += key_size;
    }
    batches
}

/// Queues change events for `keys`. A change to a plan with many derived plans touches many plan-months, so
/// the keys are sent in as many events as it takes to keep each one under the NOTIFY payload limit.
async fn notify(tx: &mut Tx, tenant: TenantId, property: Uuid, keys: Vec<String>) -> Result<(), sqlx::Error> {
    for keys in event_batches(keys) {
        db::notify(tx, &Event { tenant_id: tenant, property_id: Some(property), keys }).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{EVENT_KEY_BYTES, event_batches, rates_keys};
    use time::macros::date;
    use uuid::Uuid;

    #[test]
    fn rate_keys_name_the_plan_and_each_month() {
        let (p, plan) = (Uuid::nil(), Uuid::max());

        assert_eq!(
            rates_keys(p, plan, date!(2026 - 07 - 31), date!(2026 - 08 - 02)),
            [format!("rates:{p}:{plan}:2026-07"), format!("rates:{p}:{plan}:2026-08")]
        );
    }

    #[test]
    fn many_keys_are_split_into_events_that_fit_a_notify_payload() {
        let keys: Vec<String> =
            (0..200).map(|n| format!("rates:{}:{}:2026-{n:03}", Uuid::nil(), Uuid::max())).collect();

        let batches = event_batches(keys.clone());

        assert!(batches.len() > 1);
        assert!(batches.iter().all(|batch| batch.iter().map(|key| key.len() + 3).sum::<usize>() <= EVENT_KEY_BYTES));
        assert_eq!(batches.concat(), keys);
        assert_eq!(event_batches(vec![]), Vec::<Vec<String>>::new());
    }
}
```

Create `modules/rates/src/plans.rs`:

```rust
use crate::{RatesError, audit, business_date, lock_rates, notify, rate_plans_key, violates};
use db::{TenantId, Tx, UserId};
use serde::Serialize;
use sqlx::Row;
use sqlx::postgres::PgRow;
use uuid::Uuid;

/// Most levels of derived plans below a standard plan.
pub const MAX_DEPTH: i32 = 3;

text_enum!(
    /// `Standard` plans are priced by hand and may have derived plans; `Derived` plans are priced from their
    /// parent by a formula; `Custom` plans are priced by hand and stand alone.
    PlanKind { Standard = "standard", Derived = "derived", Custom = "custom" }
);

text_enum!(
    /// Where a plan is sold: foreign (`FIT_F`) or local (`FIT_L`) independent travellers, online travel
    /// agents, travel agent contracts, or the hotel's own booking engine.
    Segment { FitF = "FIT_F", FitL = "FIT_L", Ota = "OTA", Ta = "TA", Ibe = "IBE" }
);

text_enum!(
    /// Which guests a plan may be sold to.
    Residency { Resident = "resident", NonResident = "non_resident" }
);

text_enum!(
    /// How a price is changed: by basis points (`percent`, 1500 = +15 %) or by minor units (`amount`).
    ChangeMode { Percent = "percent", Amount = "amount" }
);

text_enum!(
    /// Room only, bed and breakfast, half board, full board.
    MealPlan { Ro = "RO", Bb = "BB", Hb = "HB", Fb = "FB" }
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct RatePlan {
    pub id: Uuid,
    pub property_id: Uuid,
    pub code: String,
    pub name: String,
    pub kind: PlanKind,
    pub segment: Segment,
    /// `None`: any guest.
    pub residency: Option<Residency>,
    pub currency: String,
    pub parent_id: Option<Uuid>,
    /// Levels of plans above this one: 0 for standard and custom plans.
    pub depth: i32,
    pub derive_mode: Option<ChangeMode>,
    /// Basis points (`percent`) or minor units (`amount`); may be negative.
    pub derive_value: Option<i64>,
    /// Derived and bulk-changed prices are rounded half-up to a multiple of this, in minor units.
    pub rounding_step: i64,
    /// Added per adult above the highest occupancy priced below a stay's.
    pub extra_adult_amount: i64,
    /// Copy the parent's restrictions instead of setting its own.
    pub inherit_restrictions: bool,
    pub allowed_meal_plans: Vec<MealPlan>,
    pub cancellation_policy_id: Option<Uuid>,
    /// The room types it sells, in their display order.
    pub room_type_ids: Vec<Uuid>,
    pub active: bool,
    pub version: i32,
}

#[derive(Debug, Clone)]
pub struct NewRatePlan {
    pub code: String,
    pub name: String,
    pub kind: PlanKind,
    pub segment: Segment,
    /// `None` on a `FIT_F` or `FIT_L` plan means the segment's residency.
    pub residency: Option<Residency>,
    pub currency: String,
    pub parent_id: Option<Uuid>,
    pub derive_mode: Option<ChangeMode>,
    pub derive_value: Option<i64>,
    pub rounding_step: i64,
    pub extra_adult_amount: i64,
    pub inherit_restrictions: bool,
    pub allowed_meal_plans: Vec<MealPlan>,
    pub cancellation_policy_id: Option<Uuid>,
    pub room_type_ids: Vec<Uuid>,
}

/// `None` leaves a field unchanged; `Some(None)` clears an optional one. The code, kind and currency never
/// change: prices, channels and reports refer to them.
#[derive(Debug, Clone, Default)]
pub struct RatePlanChanges {
    pub name: Option<String>,
    pub segment: Option<Segment>,
    pub residency: Option<Option<Residency>>,
    /// Moves a derived plan under another parent.
    pub parent_id: Option<Uuid>,
    pub derive_mode: Option<ChangeMode>,
    pub derive_value: Option<i64>,
    pub rounding_step: Option<i64>,
    pub extra_adult_amount: Option<i64>,
    pub inherit_restrictions: Option<bool>,
    pub allowed_meal_plans: Option<Vec<MealPlan>>,
    pub cancellation_policy_id: Option<Option<Uuid>>,
    pub room_type_ids: Option<Vec<Uuid>>,
    pub active: Option<bool>,
}

fn decode_error(column: &str, value: &str) -> sqlx::Error {
    sqlx::Error::ColumnDecode { index: column.into(), source: format!("unknown value {value:?}").into() }
}

/// Reads a `text` column into one of this module's enums.
fn parsed<T>(row: &PgRow, column: &str, parse: fn(&str) -> Option<T>) -> Result<T, sqlx::Error> {
    let text: String = row.try_get(column)?;
    parse(&text).ok_or_else(|| decode_error(column, &text))
}

impl sqlx::FromRow<'_, PgRow> for RatePlan {
    fn from_row(row: &PgRow) -> Result<Self, sqlx::Error> {
        let residency: Option<String> = row.try_get("residency")?;
        let derive_mode: Option<String> = row.try_get("derive_mode")?;
        let meal_plans: Vec<String> = row.try_get("allowed_meal_plans")?;
        Ok(RatePlan {
            id: row.try_get("id")?,
            property_id: row.try_get("property_id")?,
            code: row.try_get("code")?,
            name: row.try_get("name")?,
            kind: parsed(row, "kind", PlanKind::parse)?,
            segment: parsed(row, "segment", Segment::parse)?,
            residency: residency
                .map(|value| Residency::parse(&value).ok_or_else(|| decode_error("residency", &value)))
                .transpose()?,
            currency: row.try_get("currency")?,
            parent_id: row.try_get("parent_id")?,
            depth: row.try_get("depth")?,
            derive_mode: derive_mode
                .map(|value| ChangeMode::parse(&value).ok_or_else(|| decode_error("derive_mode", &value)))
                .transpose()?,
            derive_value: row.try_get("derive_value")?,
            rounding_step: row.try_get("rounding_step")?,
            extra_adult_amount: row.try_get("extra_adult_amount")?,
            inherit_restrictions: row.try_get("inherit_restrictions")?,
            allowed_meal_plans: meal_plans
                .iter()
                .map(|value| MealPlan::parse(value).ok_or_else(|| decode_error("allowed_meal_plans", value)))
                .collect::<Result<_, _>>()?,
            cancellation_policy_id: row.try_get("cancellation_policy_id")?,
            room_type_ids: row.try_get("room_type_ids")?,
            active: row.try_get("active")?,
            version: row.try_get("version")?,
        })
    }
}

/// The property's rate plans as a tree: each standard or custom plan (by code) followed by the plans derived
/// from it, depth first.
pub async fn list_rate_plans(tx: &mut Tx, property: Uuid) -> Result<Vec<RatePlan>, sqlx::Error> {
    sqlx::query_as(
        "with recursive tree as (
             select id, 0 as depth, array[code collate \"C\"] as path
             from rate_plan where property_id = $1 and parent_id is null
             union all
             select c.id, t.depth + 1, t.path || (c.code collate \"C\")
             from rate_plan c join tree t on c.parent_id = t.id
         )
         select p.id, p.property_id, p.code, p.name, p.kind, p.segment, p.residency, p.currency::text as currency,
                p.parent_id, t.depth, p.derive_mode, p.derive_value, p.rounding_step, p.extra_adult_amount,
                p.inherit_restrictions, p.allowed_meal_plans, p.cancellation_policy_id, p.active, p.version,
                array(select s.room_type_id from rate_plan_room_type s join room_type rt on rt.id = s.room_type_id
                      where s.rate_plan_id = p.id order by rt.sort_order, rt.code) as room_type_ids
         from tree t join rate_plan p on p.id = t.id
         order by t.path",
    )
    .bind(property)
    .fetch_all(&mut **tx)
    .await
}

/// The property's plans held in memory, to check derivation rules. Consistent while [`lock_rates`] is held.
pub(crate) struct Tree(pub(crate) Vec<RatePlan>);

impl Tree {
    pub(crate) fn get(&self, id: Uuid) -> Option<&RatePlan> {
        self.0.iter().find(|plan| plan.id == id)
    }

    fn children(&self, id: Uuid) -> impl Iterator<Item = &RatePlan> {
        self.0.iter().filter(move |plan| plan.parent_id == Some(id))
    }

    /// Levels of derived plans below `id`.
    fn height(&self, id: Uuid) -> i32 {
        self.children(id).map(|child| 1 + self.height(child.id)).max().unwrap_or(0)
    }

    /// Whether `id` is `ancestor` or one of the plans derived from it, at any depth.
    fn is_within(&self, id: Uuid, ancestor: Uuid) -> bool {
        let mut current = Some(id);
        while let Some(plan) = current {
            if plan == ancestor {
                return true;
            }
            current = self.get(plan).and_then(|plan| plan.parent_id);
        }
        false
    }
}

/// The residency a plan in `segment` is sold to: `FIT_F` and `FIT_L` fix it.
fn residency_for(segment: Segment, residency: Option<Residency>) -> Result<Option<Residency>, RatesError> {
    match (segment, residency) {
        (Segment::FitF, None) => Ok(Some(Residency::NonResident)),
        (Segment::FitL, None) => Ok(Some(Residency::Resident)),
        (Segment::FitF, Some(Residency::Resident)) => {
            Err(RatesError::Invalid("FIT_F plans are sold to non-residents only".into()))
        }
        (Segment::FitL, Some(Residency::NonResident)) => {
            Err(RatesError::Invalid("FIT_L plans are sold to residents only".into()))
        }
        (_, residency) => Ok(residency),
    }
}

/// Only derived plans have a parent, a formula and inherited restrictions; a formula stays in range.
fn check_formula(
    kind: PlanKind,
    has_parent: bool,
    mode: Option<ChangeMode>,
    value: Option<i64>,
    inherit_restrictions: bool,
) -> Result<(), RatesError> {
    if kind != PlanKind::Derived {
        if has_parent || mode.is_some() || value.is_some() {
            return Err(RatesError::Invalid("only derived plans have a parent and a formula".into()));
        }
        if inherit_restrictions {
            return Err(RatesError::Invalid("only derived plans inherit restrictions".into()));
        }
        return Ok(());
    }
    match (mode, value) {
        (Some(ChangeMode::Percent), Some(value)) if !(-10_000..=100_000).contains(&value) => {
            Err(RatesError::Invalid("a percentage change is between -100% and +1000%".into()))
        }
        (Some(ChangeMode::Amount), Some(value)) if value.abs() > 100_000_000_000 => {
            Err(RatesError::Invalid("an amount change is at most 100000000000 minor units".into()))
        }
        (Some(_), Some(_)) if has_parent => Ok(()),
        _ => Err(RatesError::Invalid("a derived plan needs a parent and a formula".into())),
    }
}

/// `parent` may be the parent of `code` (`plan`, if it already exists) in `currency`, with `height` levels of
/// derived plans below it.
fn check_parent<'a>(
    tree: &'a Tree,
    code: &str,
    plan: Option<Uuid>,
    parent: Uuid,
    currency: &str,
    height: i32,
) -> Result<&'a RatePlan, RatesError> {
    let parent =
        tree.get(parent).ok_or_else(|| RatesError::Invalid("no such parent rate plan in this property".into()))?;
    if plan.is_some_and(|plan| tree.is_within(parent.id, plan)) {
        return Err(RatesError::Invalid(format!("{code} cannot derive from {}: that would make a cycle", parent.code)));
    }
    if parent.kind == PlanKind::Custom {
        return Err(RatesError::Invalid(format!(
            "{} is a custom plan; only standard and derived plans have derived plans",
            parent.code
        )));
    }
    if parent.currency != currency {
        return Err(RatesError::Invalid(format!(
            "a derived plan uses its parent's currency ({}); resident prices in another currency are set by hand",
            parent.currency
        )));
    }
    if parent.depth + 1 + height > MAX_DEPTH {
        return Err(RatesError::Invalid(format!("derived plans are at most {MAX_DEPTH} levels below a standard plan")));
    }
    Ok(parent)
}

/// The property's room types: id, code and whether it is active.
async fn room_types(tx: &mut Tx, property: Uuid) -> Result<Vec<(Uuid, String, bool)>, sqlx::Error> {
    sqlx::query_as("select id, code, active from room_type where property_id = $1 order by sort_order, code")
        .bind(property)
        .fetch_all(&mut **tx)
        .await
}

fn codes(room_types: &[(Uuid, String, bool)], ids: &[Uuid]) -> String {
    room_types
        .iter()
        .filter(|(id, _, _)| ids.contains(id))
        .map(|(_, code, _)| code.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

/// `ids` without repeats, each a room type of the property; ones not already sold (`sold`) must be active.
fn check_room_types(room_types: &[(Uuid, String, bool)], ids: &[Uuid], sold: &[Uuid]) -> Result<Vec<Uuid>, RatesError> {
    let mut unique: Vec<Uuid> = Vec::new();
    for id in ids {
        let known = room_types.iter().any(|(type_id, _, active)| type_id == id && (*active || sold.contains(id)));
        if !known {
            return Err(RatesError::Invalid("no such active room type in this property".into()));
        }
        if !unique.contains(id) {
            unique.push(*id);
        }
    }
    if unique.is_empty() {
        return Err(RatesError::Invalid("a rate plan sells at least one room type".into()));
    }
    Ok(unique)
}

async fn check_policy(tx: &mut Tx, property: Uuid, policy: Option<Uuid>) -> Result<(), RatesError> {
    let Some(policy) = policy else { return Ok(()) };
    let exists: bool =
        sqlx::query_scalar("select exists (select 1 from cancellation_policy where id = $1 and property_id = $2)")
            .bind(policy)
            .bind(property)
            .fetch_one(&mut **tx)
            .await?;
    if exists { Ok(()) } else { Err(RatesError::Invalid("no such cancellation policy in this property".into())) }
}

/// Makes `types` the room types `plan` sells. Removing one deletes the plan's prices and restrictions for it.
async fn set_room_types(
    tx: &mut Tx,
    tenant: TenantId,
    property: Uuid,
    plan: Uuid,
    types: &[Uuid],
) -> Result<(), sqlx::Error> {
    sqlx::query("delete from rate_plan_room_type where rate_plan_id = $1 and room_type_id <> all($2)")
        .bind(plan)
        .bind(types)
        .execute(&mut **tx)
        .await?;
    sqlx::query(
        "insert into rate_plan_room_type (tenant_id, property_id, rate_plan_id, room_type_id)
         select $1, $2, $3, unnest($4::uuid[])
         on conflict do nothing",
    )
    .bind(tenant.0)
    .bind(property)
    .bind(plan)
    .bind(types)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

fn meal_plan_texts(meal_plans: &[MealPlan]) -> Vec<&'static str> {
    let mut texts: Vec<&'static str> = Vec::new();
    for meal_plan in meal_plans {
        if !texts.contains(&meal_plan.as_str()) {
            texts.push(meal_plan.as_str());
        }
    }
    texts
}

/// Reads one plan back after a write, from the same tree query the list uses.
async fn load(tx: &mut Tx, property: Uuid, id: Uuid) -> Result<RatePlan, RatesError> {
    let plans = list_rate_plans(tx, property).await?;
    plans.into_iter().find(|plan| plan.id == id).ok_or(RatesError::NotFound("rate plan"))
}

pub async fn create_rate_plan(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    input: NewRatePlan,
) -> Result<RatePlan, RatesError> {
    business_date(tx, property).await?;
    lock_rates(tx, property).await?;
    let tree = Tree(list_rate_plans(tx, property).await?);
    let residency = residency_for(input.segment, input.residency)?;
    check_formula(
        input.kind,
        input.parent_id.is_some(),
        input.derive_mode,
        input.derive_value,
        input.inherit_restrictions,
    )?;
    let room_types = room_types(tx, property).await?;
    let types = check_room_types(&room_types, &input.room_type_ids, &[])?;
    if let Some(parent) = input.parent_id {
        let parent = check_parent(&tree, &input.code, None, parent, &input.currency, 0)?;
        check_subset(&room_types, &input.code, &types, &parent.room_type_ids)?;
    }
    check_policy(tx, property, input.cancellation_policy_id).await?;
    let id = Uuid::now_v7();
    let inserted = sqlx::query(
        "insert into rate_plan (id, tenant_id, property_id, code, name, kind, segment, residency, currency, parent_id,
                                derive_mode, derive_value, rounding_step, extra_adult_amount, inherit_restrictions,
                                allowed_meal_plans, cancellation_policy_id)
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17)",
    )
    .bind(id)
    .bind(tenant.0)
    .bind(property)
    .bind(&input.code)
    .bind(&input.name)
    .bind(input.kind.as_str())
    .bind(input.segment.as_str())
    .bind(residency.map(Residency::as_str))
    .bind(&input.currency)
    .bind(input.parent_id)
    .bind(input.derive_mode.map(ChangeMode::as_str))
    .bind(input.derive_value)
    .bind(input.rounding_step)
    .bind(input.extra_adult_amount)
    .bind(input.inherit_restrictions)
    .bind(meal_plan_texts(&input.allowed_meal_plans))
    .bind(input.cancellation_policy_id)
    .execute(&mut **tx)
    .await;
    match inserted {
        Ok(_) => {}
        Err(err) if violates(&err, "rate_plan_property_id_code_key") => {
            return Err(RatesError::Conflict(format!("a rate plan with code {} already exists", input.code)));
        }
        Err(err) => return Err(err.into()),
    }
    set_room_types(tx, tenant, property, id, &types).await?;
    audit(tx, tenant, actor, "rate_plan.created", "rate_plan", id, serde_json::json!({ "code": input.code })).await?;
    notify(tx, tenant, property, vec![rate_plans_key(property)]).await?;
    load(tx, property, id).await
}

/// A derived plan sells only room types its parent sells.
fn check_subset(
    room_types: &[(Uuid, String, bool)],
    code: &str,
    types: &[Uuid],
    parent_types: &[Uuid],
) -> Result<(), RatesError> {
    let unsold: Vec<Uuid> = types.iter().filter(|id| !parent_types.contains(id)).copied().collect();
    if unsold.is_empty() {
        Ok(())
    } else {
        Err(RatesError::Invalid(format!(
            "{code} can only sell room types its parent sells, not {}",
            codes(room_types, &unsold)
        )))
    }
}

pub async fn update_rate_plan(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
    changes: RatePlanChanges,
) -> Result<RatePlan, RatesError> {
    business_date(tx, property).await?;
    lock_rates(tx, property).await?;
    let tree = Tree(list_rate_plans(tx, property).await?);
    let current = tree.get(id).ok_or(RatesError::NotFound("rate plan"))?;
    if current.version != expected_version {
        return Err(RatesError::VersionMismatch("rate plan"));
    }
    let segment = changes.segment.unwrap_or(current.segment);
    let residency = match changes.residency {
        Some(residency) => residency,
        // A plan moved to a FIT segment takes that segment's residency.
        None if changes.segment.is_some_and(|s| matches!(s, Segment::FitF | Segment::FitL)) => None,
        None => current.residency,
    };
    let residency = residency_for(segment, residency)?;
    check_formula(
        current.kind,
        current.parent_id.is_some() || changes.parent_id.is_some(),
        changes.derive_mode.or(current.derive_mode),
        changes.derive_value.or(current.derive_value),
        changes.inherit_restrictions.unwrap_or(current.inherit_restrictions),
    )?;
    let room_types = room_types(tx, property).await?;
    let types = check_room_types(
        &room_types,
        changes.room_type_ids.as_deref().unwrap_or(&current.room_type_ids),
        &current.room_type_ids,
    )?;
    if let Some(parent) = changes.parent_id.or(current.parent_id) {
        let parent = if Some(parent) == current.parent_id {
            tree.get(parent).ok_or(RatesError::NotFound("rate plan"))?
        } else {
            check_parent(&tree, &current.code, Some(id), parent, &current.currency, tree.height(id))?
        };
        check_subset(&room_types, &current.code, &types, &parent.room_type_ids)?;
    }
    let removed: Vec<Uuid> = current.room_type_ids.iter().filter(|t| !types.contains(t)).copied().collect();
    for child in tree.children(id) {
        let lost: Vec<Uuid> = child.room_type_ids.iter().filter(|t| removed.contains(t)).copied().collect();
        if !lost.is_empty() {
            return Err(RatesError::Conflict(format!(
                "{} still sells {}; remove it there first",
                child.code,
                codes(&room_types, &lost)
            )));
        }
    }
    if let Some(policy) = changes.cancellation_policy_id {
        check_policy(tx, property, policy).await?;
    }
    sqlx::query(
        "update rate_plan set name = coalesce($3, name), segment = $4, residency = $5,
                parent_id = coalesce($6, parent_id), derive_mode = coalesce($7, derive_mode),
                derive_value = coalesce($8, derive_value), rounding_step = coalesce($9, rounding_step),
                extra_adult_amount = coalesce($10, extra_adult_amount),
                inherit_restrictions = coalesce($11, inherit_restrictions),
                allowed_meal_plans = coalesce($12, allowed_meal_plans),
                cancellation_policy_id = case when $13 then $14 else cancellation_policy_id end,
                active = coalesce($15, active), version = version + 1
         where id = $1 and version = $2",
    )
    .bind(id)
    .bind(expected_version)
    .bind(&changes.name)
    .bind(segment.as_str())
    .bind(residency.map(Residency::as_str))
    .bind(changes.parent_id)
    .bind(changes.derive_mode.map(ChangeMode::as_str))
    .bind(changes.derive_value)
    .bind(changes.rounding_step)
    .bind(changes.extra_adult_amount)
    .bind(changes.inherit_restrictions)
    .bind(changes.allowed_meal_plans.as_deref().map(meal_plan_texts))
    .bind(changes.cancellation_policy_id.is_some())
    .bind(changes.cancellation_policy_id.flatten())
    .bind(changes.active)
    .execute(&mut **tx)
    .await?;
    set_room_types(tx, tenant, property, id, &types).await?;
    audit(
        tx,
        tenant,
        actor,
        "rate_plan.updated",
        "rate_plan",
        id,
        serde_json::json!({ "parent_id": changes.parent_id, "active": changes.active }),
    )
    .await?;
    notify(tx, tenant, property, vec![rate_plans_key(property)]).await?;
    load(tx, property, id).await
}
```

Modify `modules/rooms/src/inventory.rs`:

```diff
diff --git a/modules/rooms/src/inventory.rs b/modules/rooms/src/inventory.rs
index f63a9ec..b315163 100644
--- a/modules/rooms/src/inventory.rs
+++ b/modules/rooms/src/inventory.rs
@@ -223,15 +223,20 @@ pub async fn list_inventory(
     .await
 }
 
-/// Cache keys `inventory:<property>:<yyyy-mm>` for every month that `[from, to)` touches.
-pub fn month_keys(property: Uuid, from: Date, to: Date) -> Vec<String> {
-    let mut keys = Vec::new();
+/// Every month (`yyyy-mm`) that `[from, to)` touches, in order. Cache keys for date ranges are built from it.
+pub fn months(from: Date, to: Date) -> Vec<String> {
+    let mut months = Vec::new();
     let mut month = from.replace_day(1).expect("every month has a first day");
     while from < to && month < to {
-        keys.push(format!("inventory:{property}:{:04}-{:02}", month.year(), u8::from(month.month())));
+        months.push(format!("{:04}-{:02}", month.year(), u8::from(month.month())));
         month += Duration::days(i64::from(month.month().length(month.year())));
     }
-    keys
+    months
+}
+
+/// Cache keys `inventory:<property>:<yyyy-mm>` for every month that `[from, to)` touches.
+pub fn month_keys(property: Uuid, from: Date, to: Date) -> Vec<String> {
+    months(from, to).into_iter().map(|month| format!("inventory:{property}:{month}")).collect()
 }
 
 /// Month keys for the days of `[from, to)` inside the counter window. Only those days have counters to
```

Modify `modules/rooms/src/lib.rs`:

```diff
diff --git a/modules/rooms/src/lib.rs b/modules/rooms/src/lib.rs
index 86fdb30..be5c11a 100644
--- a/modules/rooms/src/lib.rs
+++ b/modules/rooms/src/lib.rs
@@ -13,7 +13,9 @@ pub use blocks::{
     Block, BlockKind, BlockReason, BlockReasonChanges, DEFAULT_BLOCK_REASONS, NewBlock, NewBlockReason, create_block,
     create_block_reason, list_block_reasons, list_blocks, seed_block_reasons, shorten_block, update_block_reason,
 };
-pub use inventory::{InventoryDay, InventoryDrift, WINDOW_DAYS, extend_window, find_drift, list_inventory, month_keys};
+pub use inventory::{
+    InventoryDay, InventoryDrift, WINDOW_DAYS, extend_window, find_drift, list_inventory, month_keys, months,
+};
 pub use room_types::{
     Bed, NewRoomType, RoomType, RoomTypeChanges, create_room_type, list_room_types, reorder_room_types,
     update_room_type,
```

- [ ] **Step 4: Run the checks**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p rates -p rooms
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: `rates`: 2 unit tests, `plans` 8, `plan_tree` 1; rooms unchanged.

- [ ] **Step 5: Commit**

```sh
git add -A
git commit -m "feat(rates): rate plans as a tree of standard, derived and custom plans, with the derivation rules checked by a property-based test"
```

### Task 6: Meal supplements and cancellation policies

Meal supplements (BB, HB, FB per currency, adult and child amounts, a validity period that may be
open-ended, no overlaps) and cancellation policies (rules by days before arrival, a no-show penalty) with
create, update (`If-Match` versions) and list. A rate plan may name a policy of its property.

**Files:**
- Modify: `modules/rates/src/lib.rs`
- Create: `modules/rates/src/meals.rs`
- Create: `modules/rates/src/policies.rs`
- Test: `modules/rates/tests/supplements.rs` (new)

**Interfaces:**
- Produces: `MealSupplement`, `NewMealSupplement`, `MealSupplementChanges` (`to: Option<Option<Date>>`), `create_meal_supplement`, `update_meal_supplement`, `list_meal_supplements`; `PenaltyKind`, `Penalty`, `CancellationRule`, `CancellationPolicy`, `NewCancellationPolicy`, `CancellationPolicyChanges`, `create_cancellation_policy`, `update_cancellation_policy`, `list_cancellation_policies`. All writes notify `rate-plans:<p>`.

- [ ] **Step 1: Write the failing tests**

Create `modules/rates/tests/supplements.rs`:

```rust
mod common;

use common::Hotel;
use rates::{
    CancellationPolicyChanges, CancellationRule, MealPlan, MealSupplementChanges, NewCancellationPolicy,
    NewMealSupplement, Penalty, PenaltyKind, RatesError,
};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

fn breakfast(hotel: &Hotel, currency: &str, from: i64, to: Option<i64>) -> NewMealSupplement {
    NewMealSupplement {
        meal_plan: MealPlan::Bb,
        currency: currency.into(),
        adult_amount: 1_500,
        child_amount: 750,
        from: hotel.day(from),
        to: to.map(|days| hotel.day(days)),
    }
}

async fn add(hotel: &Hotel, input: NewMealSupplement) -> Result<rates::MealSupplement, RatesError> {
    let mut tx = hotel.tx().await;
    let created = rates::create_meal_supplement(&mut tx, hotel.tenant, hotel.user, hotel.property, input).await?;
    tx.commit().await.unwrap();
    Ok(created)
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn supplements_are_per_meal_plan_and_currency_without_overlaps(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let usd = add(&hotel, breakfast(&hotel, "USD", 0, Some(180))).await.unwrap();
    let lkr = add(&hotel, NewMealSupplement { adult_amount: 450_000, ..breakfast(&hotel, "LKR", 0, None) }).await;
    let half_board = add(&hotel, NewMealSupplement { meal_plan: MealPlan::Hb, ..breakfast(&hotel, "USD", 0, None) });
    let half_board = half_board.await.unwrap();

    let overlapping = add(&hotel, breakfast(&hotel, "USD", 90, None)).await;
    let room_only = add(&hotel, NewMealSupplement { meal_plan: MealPlan::Ro, ..breakfast(&hotel, "USD", 0, None) });
    let backwards = add(&hotel, breakfast(&hotel, "EUR", 10, Some(10))).await;
    let next_season = add(&hotel, breakfast(&hotel, "USD", 180, None)).await.unwrap();
    let listed = rates::list_meal_supplements(&mut hotel.tx().await, hotel.property).await.unwrap();

    assert_eq!((usd.from, usd.to, usd.version), (hotel.day(0), Some(hotel.day(180)), 1));
    assert_eq!(lkr.unwrap().to, None, "open-ended");
    assert!(
        matches!(&overlapping, Err(RatesError::Conflict(m)) if m == "a BB supplement in USD already covers some of these dates"),
        "{overlapping:?}"
    );
    assert!(
        matches!(&room_only.await, Err(RatesError::Invalid(m)) if m == "room only (RO) has no supplement"),
        "RO is always 0"
    );
    assert!(matches!(&backwards, Err(RatesError::Invalid(m)) if m == "a supplement ends after it starts"));
    let order: Vec<(&str, MealPlan, time::Date)> =
        listed.iter().map(|s| (s.currency.as_str(), s.meal_plan, s.from)).collect();
    assert_eq!(
        order,
        [
            ("LKR", MealPlan::Bb, hotel.day(0)),
            ("USD", MealPlan::Bb, hotel.day(0)),
            ("USD", MealPlan::Bb, hotel.day(180)),
            ("USD", MealPlan::Hb, hotel.day(0)),
        ]
    );
    assert_eq!(next_season.from, hotel.day(180));
    assert_eq!(half_board.meal_plan, MealPlan::Hb);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_supplement_is_changed_with_its_version(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let usd = add(&hotel, breakfast(&hotel, "USD", 0, None)).await.unwrap();
    add(&hotel, breakfast(&hotel, "USD", -60, Some(0))).await.unwrap();
    let change = |changes: MealSupplementChanges, version: i32| {
        let hotel = &hotel;
        async move {
            let mut tx = hotel.tx().await;
            let updated = rates::update_meal_supplement(
                &mut tx,
                hotel.tenant,
                hotel.user,
                hotel.property,
                usd.id,
                version,
                changes,
            )
            .await?;
            tx.commit().await.unwrap();
            Ok::<_, RatesError>(updated)
        }
    };

    let raised = change(
        MealSupplementChanges {
            adult_amount: Some(1_800),
            to: Some(Some(hotel.day(365))),
            ..MealSupplementChanges::default()
        },
        1,
    )
    .await
    .unwrap();
    let stale = change(MealSupplementChanges { child_amount: Some(900), ..MealSupplementChanges::default() }, 1).await;
    let into_last_season =
        change(MealSupplementChanges { from: Some(hotel.day(-30)), ..MealSupplementChanges::default() }, 2).await;
    let reopened = change(MealSupplementChanges { to: Some(None), ..MealSupplementChanges::default() }, 2).await;

    assert_eq!(
        (raised.adult_amount, raised.child_amount, raised.to, raised.version),
        (1_800, 750, Some(hotel.day(365)), 2)
    );
    assert!(matches!(stale, Err(RatesError::VersionMismatch("meal supplement"))), "{stale:?}");
    assert!(matches!(into_last_season, Err(RatesError::Conflict(_))), "{into_last_season:?}");
    assert_eq!(reopened.unwrap().to, None);
}

fn penalty(kind: PenaltyKind, value: i64) -> Penalty {
    Penalty { kind, value }
}

fn flexible() -> NewCancellationPolicy {
    NewCancellationPolicy {
        name: "Flexible".into(),
        rules: vec![
            CancellationRule { days_before_arrival: 2, penalty: penalty(PenaltyKind::Nights, 1) },
            CancellationRule { days_before_arrival: 14, penalty: penalty(PenaltyKind::Percent, 2_500) },
        ],
        no_show: penalty(PenaltyKind::Percent, 10_000),
    }
}

async fn add_policy(hotel: &Hotel, input: NewCancellationPolicy) -> Result<rates::CancellationPolicy, RatesError> {
    let mut tx = hotel.tx().await;
    let created = rates::create_cancellation_policy(&mut tx, hotel.tenant, hotel.user, hotel.property, input).await?;
    tx.commit().await.unwrap();
    Ok(created)
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn cancellation_policies_keep_their_rules_in_order(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let policy = add_policy(&hotel, flexible()).await.unwrap();

    let duplicate = add_policy(&hotel, flexible()).await;
    let same_day_twice = add_policy(
        &hotel,
        NewCancellationPolicy {
            name: "Twice".into(),
            rules: vec![
                CancellationRule { days_before_arrival: 3, penalty: penalty(PenaltyKind::Nights, 1) },
                CancellationRule { days_before_arrival: 3, penalty: penalty(PenaltyKind::Nights, 2) },
            ],
            ..flexible()
        },
    )
    .await;
    let too_much = add_policy(
        &hotel,
        NewCancellationPolicy { name: "Harsh".into(), no_show: penalty(PenaltyKind::Percent, 20_000), ..flexible() },
    )
    .await;
    let mut tx = hotel.tx().await;
    let strict = CancellationPolicyChanges {
        name: Some("Strict".into()),
        rules: Some(vec![CancellationRule { days_before_arrival: 30, penalty: penalty(PenaltyKind::Amount, 50_000) }]),
        no_show: None,
    };
    let renamed =
        rates::update_cancellation_policy(&mut tx, hotel.tenant, hotel.user, hotel.property, policy.id, 1, strict)
            .await
            .unwrap();
    let listed = rates::list_cancellation_policies(&mut tx, hotel.property).await.unwrap();

    let days: Vec<i32> = policy.rules.iter().map(|rule| rule.days_before_arrival).collect();
    assert_eq!(days, [14, 2], "furthest from arrival first");
    assert!(
        matches!(&duplicate, Err(RatesError::Conflict(m)) if m == "a cancellation policy named Flexible already exists")
    );
    assert!(
        matches!(&same_day_twice, Err(RatesError::Invalid(m)) if m == "each rule needs its own number of days before arrival")
    );
    assert!(
        matches!(&too_much, Err(RatesError::Invalid(m)) if m == "a percent penalty is 1 to 10000 basis points"),
        "{too_much:?}"
    );
    assert_eq!((renamed.name.as_str(), renamed.version, renamed.no_show), ("Strict", 2, policy.no_show));
    assert_eq!(listed, [renamed]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_rate_plan_names_a_cancellation_policy_of_its_property(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let policy = add_policy(&hotel, flexible()).await.unwrap();

    let with_policy =
        rates::NewRatePlan { cancellation_policy_id: Some(policy.id), ..hotel.standard_plan("BAR", "USD") };
    let with_policy = hotel.try_plan(with_policy).await.unwrap();
    let with_unknown =
        rates::NewRatePlan { cancellation_policy_id: Some(Uuid::now_v7()), ..hotel.standard_plan("RACK", "USD") };
    let with_unknown = hotel.try_plan(with_unknown).await;

    assert_eq!(with_policy.cancellation_policy_id, Some(policy.id));
    assert!(
        matches!(&with_unknown, Err(RatesError::Invalid(m)) if m == "no such cancellation policy in this property"),
        "{with_unknown:?}"
    );
}
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `DATABASE_URL=$TEST_DATABASE_URL cargo test -p rates --test supplements`

Expected: fails to compile: ``unresolved imports `rates::CancellationPolicyChanges`, … `rates::NewMealSupplement` …``, ``cannot find function `update_meal_supplement` in crate `rates` ``.

- [ ] **Step 3: Implement**

Modify `modules/rates/src/lib.rs`:

```diff
diff --git a/modules/rates/src/lib.rs b/modules/rates/src/lib.rs
index 817465c..d5a5052 100644
--- a/modules/rates/src/lib.rs
+++ b/modules/rates/src/lib.rs
@@ -30,15 +30,33 @@ macro_rules! text_enum {
                 }
             }
         }
+
+        impl TryFrom<String> for $name {
+            type Error = String;
+
+            fn try_from(value: String) -> Result<Self, Self::Error> {
+                $name::parse(&value).ok_or_else(|| format!("unknown {} {value:?}", stringify!($name)))
+            }
+        }
     };
 }
 
+mod meals;
 mod plans;
+mod policies;
 
+pub use meals::{
+    MealSupplement, MealSupplementChanges, NewMealSupplement, create_meal_supplement, list_meal_supplements,
+    update_meal_supplement,
+};
 pub use plans::{
     ChangeMode, MealPlan, NewRatePlan, PlanKind, RatePlan, RatePlanChanges, Residency, Segment, create_rate_plan,
     list_rate_plans, update_rate_plan,
 };
+pub use policies::{
+    CancellationPolicy, CancellationPolicyChanges, CancellationRule, NewCancellationPolicy, Penalty, PenaltyKind,
+    create_cancellation_policy, list_cancellation_policies, update_cancellation_policy,
+};
 
 use db::{Event, TenantId, Tx, UserId};
 use time::Date;
```

Create `modules/rates/src/meals.rs`:

```rust
use crate::{MealPlan, RatesError, audit, business_date, notify, rate_plans_key, violates};
use db::{TenantId, Tx, UserId};
use serde::Serialize;
use time::Date;
use uuid::Uuid;

/// What a meal plan costs per person per night on top of the room price, in one currency, for the nights
/// from `from` until `to` (the first night it no longer applies; `None`: until further notice).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct MealSupplement {
    pub id: Uuid,
    pub property_id: Uuid,
    #[sqlx(try_from = "String")]
    pub meal_plan: MealPlan,
    pub currency: String,
    pub adult_amount: i64,
    pub child_amount: i64,
    pub from: Date,
    pub to: Option<Date>,
    pub version: i32,
}

#[derive(Debug, Clone)]
pub struct NewMealSupplement {
    pub meal_plan: MealPlan,
    pub currency: String,
    pub adult_amount: i64,
    pub child_amount: i64,
    pub from: Date,
    pub to: Option<Date>,
}

/// `None` leaves a field unchanged; `to: Some(None)` makes the supplement open-ended.
#[derive(Debug, Clone, Default)]
pub struct MealSupplementChanges {
    pub adult_amount: Option<i64>,
    pub child_amount: Option<i64>,
    pub from: Option<Date>,
    pub to: Option<Option<Date>>,
}

const COLUMNS: &str = "id, property_id, meal_plan, currency::text as currency, adult_amount, child_amount, \
                       lower(valid) as \"from\", upper(valid) as \"to\", version";

fn check_period(from: Date, to: Option<Date>) -> Result<(), RatesError> {
    if to.is_some_and(|to| to <= from) {
        Err(RatesError::Invalid("a supplement ends after it starts".into()))
    } else {
        Ok(())
    }
}

fn overlap_error(err: sqlx::Error, meal_plan: MealPlan, currency: &str) -> RatesError {
    if violates(&err, "meal_supplement_no_overlap") {
        RatesError::Conflict(format!(
            "a {} supplement in {currency} already covers some of these dates",
            meal_plan.as_str()
        ))
    } else {
        err.into()
    }
}

pub async fn create_meal_supplement(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    input: NewMealSupplement,
) -> Result<MealSupplement, RatesError> {
    business_date(tx, property).await?;
    if input.meal_plan == MealPlan::Ro {
        return Err(RatesError::Invalid("room only (RO) has no supplement".into()));
    }
    check_period(input.from, input.to)?;
    let created: MealSupplement = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "insert into meal_supplement (id, tenant_id, property_id, meal_plan, currency, adult_amount, child_amount, valid)
         values ($1, $2, $3, $4, $5, $6, $7, daterange($8, $9))
         returning {COLUMNS}"
    )))
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(property)
    .bind(input.meal_plan.as_str())
    .bind(&input.currency)
    .bind(input.adult_amount)
    .bind(input.child_amount)
    .bind(input.from)
    .bind(input.to)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| overlap_error(err, input.meal_plan, &input.currency))?;
    audit(
        tx,
        tenant,
        actor,
        "meal_supplement.created",
        "meal_supplement",
        created.id,
        serde_json::json!({ "meal_plan": input.meal_plan.as_str(), "currency": input.currency }),
    )
    .await?;
    notify(tx, tenant, property, vec![rate_plans_key(property)]).await?;
    Ok(created)
}

pub async fn update_meal_supplement(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
    changes: MealSupplementChanges,
) -> Result<MealSupplement, RatesError> {
    let current: MealSupplement = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {COLUMNS} from meal_supplement where id = $1 and property_id = $2 for update"
    )))
    .bind(id)
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(RatesError::NotFound("meal supplement"))?;
    if current.version != expected_version {
        return Err(RatesError::VersionMismatch("meal supplement"));
    }
    let (from, to) = (changes.from.unwrap_or(current.from), changes.to.unwrap_or(current.to));
    check_period(from, to)?;
    let updated: MealSupplement = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "update meal_supplement set adult_amount = coalesce($2, adult_amount), child_amount = coalesce($3, child_amount),
                valid = daterange($4, $5), version = version + 1
         where id = $1
         returning {COLUMNS}"
    )))
    .bind(id)
    .bind(changes.adult_amount)
    .bind(changes.child_amount)
    .bind(from)
    .bind(to)
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| overlap_error(err, current.meal_plan, &current.currency))?;
    audit(
        tx,
        tenant,
        actor,
        "meal_supplement.updated",
        "meal_supplement",
        id,
        serde_json::json!({ "adult_amount": updated.adult_amount, "child_amount": updated.child_amount }),
    )
    .await?;
    notify(tx, tenant, property, vec![rate_plans_key(property)]).await?;
    Ok(updated)
}

/// Every supplement of the property, by currency, meal plan and start.
pub async fn list_meal_supplements(tx: &mut Tx, property: Uuid) -> Result<Vec<MealSupplement>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {COLUMNS} from meal_supplement where property_id = $1
         order by currency, array_position(array['BB', 'HB', 'FB'], meal_plan), lower(valid)"
    )))
    .bind(property)
    .fetch_all(&mut **tx)
    .await
}
```

Create `modules/rates/src/policies.rs`:

```rust
use crate::{RatesError, audit, business_date, notify, rate_plans_key, violates};
use db::{TenantId, Tx, UserId};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

text_enum!(
    /// `nights`: that many nights' room price; `percent`: basis points of the stay; `amount`: minor units in
    /// the plan's currency.
    PenaltyKind { Nights = "nights", Percent = "percent", Amount = "amount" }
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Penalty {
    pub kind: PenaltyKind,
    pub value: i64,
}

/// Cancelling `days_before_arrival` days or fewer before arrival costs `penalty`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct CancellationRule {
    pub days_before_arrival: i32,
    pub penalty: Penalty,
}

/// What cancelling or not arriving costs. Reservations (Phase 3) apply it; rate plans name one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct CancellationPolicy {
    pub id: Uuid,
    pub property_id: Uuid,
    pub name: String,
    /// Furthest from arrival first; the rule with the fewest days at or above the days left applies.
    #[sqlx(json)]
    pub rules: Vec<CancellationRule>,
    #[sqlx(json)]
    pub no_show: Penalty,
    pub version: i32,
}

#[derive(Debug, Clone)]
pub struct NewCancellationPolicy {
    pub name: String,
    pub rules: Vec<CancellationRule>,
    pub no_show: Penalty,
}

/// `None` leaves a field unchanged; `rules` replaces every rule.
#[derive(Debug, Clone, Default)]
pub struct CancellationPolicyChanges {
    pub name: Option<String>,
    pub rules: Option<Vec<CancellationRule>>,
    pub no_show: Option<Penalty>,
}

const COLUMNS: &str = "id, property_id, name, rules, no_show, version";

fn check_penalty(penalty: Penalty) -> Result<(), RatesError> {
    let (range, message) = match penalty.kind {
        PenaltyKind::Nights => (1..=30, "a nights penalty is 1 to 30 nights"),
        PenaltyKind::Percent => (1..=10_000, "a percent penalty is 1 to 10000 basis points"),
        PenaltyKind::Amount => (1..=100_000_000_000, "an amount penalty is 1 to 100000000000 minor units"),
    };
    if range.contains(&penalty.value) { Ok(()) } else { Err(RatesError::Invalid(message.into())) }
}

/// The rules, checked and sorted furthest from arrival first.
fn checked_rules(mut rules: Vec<CancellationRule>, no_show: Penalty) -> Result<Vec<CancellationRule>, RatesError> {
    check_penalty(no_show)?;
    for rule in &rules {
        if !(0..=365).contains(&rule.days_before_arrival) {
            return Err(RatesError::Invalid("days before arrival are 0 to 365".into()));
        }
        check_penalty(rule.penalty)?;
    }
    rules.sort_by_key(|rule| std::cmp::Reverse(rule.days_before_arrival));
    if rules.windows(2).any(|pair| pair[0].days_before_arrival == pair[1].days_before_arrival) {
        return Err(RatesError::Invalid("each rule needs its own number of days before arrival".into()));
    }
    Ok(rules)
}

fn name_error(err: sqlx::Error, name: &str) -> RatesError {
    if violates(&err, "cancellation_policy_property_id_name_key") {
        RatesError::Conflict(format!("a cancellation policy named {name} already exists"))
    } else {
        err.into()
    }
}

pub async fn create_cancellation_policy(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    input: NewCancellationPolicy,
) -> Result<CancellationPolicy, RatesError> {
    business_date(tx, property).await?;
    let rules = checked_rules(input.rules, input.no_show)?;
    let created: CancellationPolicy = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "insert into cancellation_policy (id, tenant_id, property_id, name, rules, no_show)
         values ($1, $2, $3, $4, $5, $6)
         returning {COLUMNS}"
    )))
    .bind(Uuid::now_v7())
    .bind(tenant.0)
    .bind(property)
    .bind(&input.name)
    .bind(sqlx::types::Json(&rules))
    .bind(sqlx::types::Json(input.no_show))
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| name_error(err, &input.name))?;
    audit(
        tx,
        tenant,
        actor,
        "cancellation_policy.created",
        "cancellation_policy",
        created.id,
        serde_json::json!({ "name": input.name }),
    )
    .await?;
    notify(tx, tenant, property, vec![rate_plans_key(property)]).await?;
    Ok(created)
}

pub async fn update_cancellation_policy(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    id: Uuid,
    expected_version: i32,
    changes: CancellationPolicyChanges,
) -> Result<CancellationPolicy, RatesError> {
    let current: CancellationPolicy = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {COLUMNS} from cancellation_policy where id = $1 and property_id = $2 for update"
    )))
    .bind(id)
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(RatesError::NotFound("cancellation policy"))?;
    if current.version != expected_version {
        return Err(RatesError::VersionMismatch("cancellation policy"));
    }
    let no_show = changes.no_show.unwrap_or(current.no_show);
    let rules = checked_rules(changes.rules.unwrap_or(current.rules), no_show)?;
    let name = changes.name.unwrap_or(current.name);
    let updated: CancellationPolicy = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "update cancellation_policy set name = $2, rules = $3, no_show = $4, version = version + 1
         where id = $1
         returning {COLUMNS}"
    )))
    .bind(id)
    .bind(&name)
    .bind(sqlx::types::Json(&rules))
    .bind(sqlx::types::Json(no_show))
    .fetch_one(&mut **tx)
    .await
    .map_err(|err| name_error(err, &name))?;
    audit(
        tx,
        tenant,
        actor,
        "cancellation_policy.updated",
        "cancellation_policy",
        id,
        serde_json::json!({ "name": name }),
    )
    .await?;
    notify(tx, tenant, property, vec![rate_plans_key(property)]).await?;
    Ok(updated)
}

/// The property's cancellation policies, by name.
pub async fn list_cancellation_policies(tx: &mut Tx, property: Uuid) -> Result<Vec<CancellationPolicy>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {COLUMNS} from cancellation_policy where property_id = $1 order by name"
    )))
    .bind(property)
    .fetch_all(&mut **tx)
    .await
}
```

- [ ] **Step 4: Run the checks**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p rates
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: `supplements` 4 passed; earlier rates tests still pass.

- [ ] **Step 5: Commit**

```sh
git add -A
git commit -m "feat(rates): meal supplements per meal plan and currency, and cancellation policies"
```

### Task 7: Prices, derivation down the tree, and bulk changes

Standard and custom plans take prices cell by cell (`set_prices`) and in bulk (`bulk_change`: a date range,
weekdays, room types and occupancies, changed by percent, amount or set, rounded to the plan's step). Either
reprices every derived plan below in the same transaction, level by level, through `app.derive_amount`.
Creating a derived plan, moving it, or changing its formula, rounding or room types reprices it and its
descendants. `preview_bulk_change` runs the same selection without writing. Besides example tests (including
that the grandchild's rows exist before the transaction commits), a property-based test builds random chains
of up to 3 derived plans with random formulas and applies random prices, bulk changes and reformulations,
checking every level against exact arithmetic after each step.

**Files:**
- Modify: `modules/rates/src/lib.rs`
- Modify: `modules/rates/src/plans.rs`
- Create: `modules/rates/src/prices.rs`
- Test: `modules/rates/tests/common/mod.rs`
- Test: `modules/rates/tests/derivation.rs` (new)
- Test: `modules/rates/tests/prices.rs` (new)

**Interfaces:**
- Consumes: `Tree`, `lock_rates`, `rates_keys`, `app.derive_amount`.
- Produces: `Price { room_type_id, date, occupancy, amount }`, `PriceChangeMode::{Percent, Amount, Set}`, `PriceChange`, `BulkChange { from, to, weekdays: Vec<time::Weekday>, room_type_ids, occupancies, change }`, `PriceChangeCell`, `BulkPreview { total, cells }`, `set_prices(tx, tenant, actor, property, plan, &[Price])`, `bulk_change(…, &BulkChange) -> u64`, `preview_bulk_change(tx, property, plan, &BulkChange, limit) -> BulkPreview`, `list_prices(tx, property, plan, from, to)`, `MAX_AMOUNT`; crate-internal `derive_prices`, `check_range`, `hand_priced`, `sold_types`, `tree_keys`.

- [ ] **Step 1: Write the failing tests**

Modify `modules/rates/tests/common/mod.rs`:

```diff
diff --git a/modules/rates/tests/common/mod.rs b/modules/rates/tests/common/mod.rs
index 9f53684..9754488 100644
--- a/modules/rates/tests/common/mod.rs
+++ b/modules/rates/tests/common/mod.rs
@@ -135,3 +135,37 @@ impl Hotel {
         rates::list_rate_plans(&mut self.tx().await, self.property).await.unwrap()
     }
 }
+
+/// `app.derive_amount` computed exactly: `base` plus `value` basis points (`percent`) or minor units
+/// (`amount`), at least 0, rounded half-up to a multiple of `step`.
+pub fn derived(base: i64, mode: ChangeMode, value: i64, step: i64) -> i64 {
+    let (numerator, denominator) = match mode {
+        ChangeMode::Percent => (i128::from(base) * (10_000 + i128::from(value)), 10_000_i128),
+        ChangeMode::Amount => (i128::from(base) + i128::from(value), 1),
+    };
+    let unit = denominator * i128::from(step);
+    let (whole, rest) = (numerator.max(0) / unit, numerator.max(0) % unit);
+    let steps = if 2 * rest >= unit { whole + 1 } else { whole };
+    i64::try_from(steps * i128::from(step)).unwrap()
+}
+
+impl Hotel {
+    /// Sets `plan`'s prices in their own transaction, committed if it succeeds.
+    pub async fn try_prices(&self, plan: &RatePlan, prices: &[rates::Price]) -> Result<(), RatesError> {
+        let mut tx = self.tx().await;
+        rates::set_prices(&mut tx, self.tenant, self.user, self.property, plan.id, prices).await?;
+        tx.commit().await.unwrap();
+        Ok(())
+    }
+
+    /// `amount` for `occupancy` adults in `room_type` on each day in `[from, to)`.
+    pub fn prices(&self, room_type: Uuid, from: i64, to: i64, occupancy: i32, amount: i64) -> Vec<rates::Price> {
+        (from..to).map(|day| rates::Price { room_type_id: room_type, date: self.day(day), occupancy, amount }).collect()
+    }
+
+    /// `plan`'s prices over the whole rate window, by date, room type and occupancy.
+    pub async fn stored(&self, plan: &RatePlan) -> Vec<rates::Price> {
+        let window = (self.day(0), self.day(rooms::WINDOW_DAYS));
+        rates::list_prices(&mut self.tx().await, self.property, plan.id, window.0, window.1).await.unwrap()
+    }
+}
```

Create `modules/rates/tests/derivation.rs`:

```rust
//! Property-based check of price derivation: random chains of up to 3 derived plans with random formulas
//! (percent or amount, negative values, rounding steps), random prices and random bulk changes on the
//! standard plan. After every step each derived plan must hold exactly its parent's cells, each equal to the
//! parent's price through its formula, computed here with exact arithmetic.

mod common;

use common::{Hotel, derived};
use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::TestRunner;
use rates::{BulkChange, ChangeMode, Price, PriceChange, PriceChangeMode, RatePlan, RatePlanChanges};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

/// Chains tried per run. Each run draws new ones; a failure prints the chain and step.
const CHAINS: usize = 10;

#[derive(Debug, Clone, Copy)]
struct Formula {
    mode: ChangeMode,
    value: i64,
    step: i64,
}

fn formula() -> impl Strategy<Value = Formula> {
    let step = prop_oneof![Just(1_i64), Just(5), Just(50), Just(100), Just(1_000)];
    prop_oneof![
        (-10_000..=100_000_i64, step.clone()).prop_map(|(value, step)| Formula {
            mode: ChangeMode::Percent,
            value,
            step
        }),
        (-2_000_000..=2_000_000_i64, step).prop_map(|(value, step)| Formula { mode: ChangeMode::Amount, value, step }),
    ]
}

#[derive(Debug, Clone)]
enum Op {
    /// Set `amount` for `occupancy` adults of DLX on days `[day, day + days)`.
    SetPrices { day: i64, days: i64, occupancy: i32, amount: i64 },
    /// Change DLX on `[day, day + days)` by `mode` and `value`.
    Bulk { day: i64, days: i64, mode: PriceChangeMode, value: i64 },
    /// Give plan `level` (1-based) a new formula.
    Reformulate { level: usize, formula: Formula },
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (0..40_i64, 1..10_i64, 1..=3_i32, 0..=100_000_000_i64)
            .prop_map(|(day, days, occupancy, amount)| Op::SetPrices { day, days, occupancy, amount }),
        (0..40_i64, 1..20_i64, -5_000..=5_000_i64).prop_map(|(day, days, value)| Op::Bulk {
            day,
            days,
            mode: PriceChangeMode::Percent,
            value
        }),
        (0..40_i64, 1..20_i64, -500_000..=500_000_i64).prop_map(|(day, days, value)| Op::Bulk {
            day,
            days,
            mode: PriceChangeMode::Amount,
            value
        }),
        (1..=3_usize, formula()).prop_map(|(level, formula)| Op::Reformulate { level, formula }),
    ]
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn derived_prices_follow_their_formulas_down_the_chain(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let mut runner = TestRunner::default();

    for chain in 0..CHAINS {
        let formulas = prop::collection::vec(formula(), 1..=3).new_tree(&mut runner).unwrap().current();
        let ops = prop::collection::vec(op(), 1..12).new_tree(&mut runner).unwrap().current();
        let root = hotel.plan(hotel.standard_plan(&format!("ROOT{chain}"), "USD")).await;
        let mut plans: Vec<RatePlan> = vec![root.clone()];
        for (level, formula) in formulas.iter().enumerate() {
            let input = rates::NewRatePlan {
                rounding_step: formula.step,
                ..hotel.derived_plan(&format!("C{chain}L{level}"), &plans[level], formula.mode, formula.value)
            };
            plans.push(hotel.plan(input).await);
        }

        for (step, op) in ops.iter().enumerate() {
            let context = format!("chain {chain} {formulas:?}, step {step} of {ops:?}");
            let mut tx = hotel.tx().await;
            let (tenant, user, property) = (hotel.tenant, hotel.user, hotel.property);
            match op.clone() {
                Op::SetPrices { day, days, occupancy, amount } => {
                    let prices = hotel.prices(hotel.deluxe.id, day, day + days, occupancy, amount);
                    rates::set_prices(&mut tx, tenant, user, property, root.id, &prices).await.unwrap();
                }
                Op::Bulk { day, days, mode, value } => {
                    let change = BulkChange {
                        from: hotel.day(day),
                        to: hotel.day(day + days),
                        weekdays: vec![],
                        room_type_ids: vec![hotel.deluxe.id],
                        occupancies: vec![],
                        change: PriceChange { mode, value },
                    };
                    rates::bulk_change(&mut tx, tenant, user, property, root.id, &change).await.unwrap();
                }
                Op::Reformulate { level, formula } => {
                    let Some(plan) = plans.get(level) else { continue };
                    let changes = RatePlanChanges {
                        derive_mode: Some(formula.mode),
                        derive_value: Some(formula.value),
                        rounding_step: Some(formula.step),
                        ..RatePlanChanges::default()
                    };
                    let updated =
                        rates::update_rate_plan(&mut tx, tenant, user, property, plan.id, plan.version, changes)
                            .await
                            .unwrap();
                    plans[level] = updated;
                }
            }
            tx.commit().await.unwrap();

            for pair in plans.windows(2) {
                let (parent, child) = (&pair[0], &pair[1]);
                let expected: Vec<Price> = hotel
                    .stored(parent)
                    .await
                    .into_iter()
                    .map(|price| Price {
                        amount: derived(
                            price.amount,
                            child.derive_mode.unwrap(),
                            child.derive_value.unwrap(),
                            child.rounding_step,
                        ),
                        ..price
                    })
                    .collect();
                assert_eq!(hotel.stored(child).await, expected, "{} from {}: {context}", child.code, parent.code);
            }
        }
    }
}
```

Create `modules/rates/tests/prices.rs`:

```rust
mod common;

use common::Hotel;
use rates::{BulkChange, ChangeMode, Price, PriceChange, PriceChangeMode, RatePlanChanges, RatesError};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use time::Weekday;
use uuid::Uuid;

fn invalid(result: Result<impl std::fmt::Debug, RatesError>) -> String {
    match result {
        Err(RatesError::Invalid(message)) => message,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn derived_plans_are_repriced_in_the_same_transaction(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let bar = hotel.plan(hotel.standard_plan("BAR", "USD")).await;
    let ota = hotel
        .plan(rates::NewRatePlan { rounding_step: 100, ..hotel.derived_plan("OTA", &bar, ChangeMode::Percent, 1_500) });
    let ota = ota.await;
    let ota_nr = hotel.plan(hotel.derived_plan("OTA-NR", &ota, ChangeMode::Amount, -1_050)).await;

    let mut tx = hotel.tx().await;
    let prices = [Price { room_type_id: hotel.deluxe.id, date: hotel.day(3), occupancy: 2, amount: 10_050 }];
    rates::set_prices(&mut tx, hotel.tenant, hotel.user, hotel.property, bar.id, &prices).await.unwrap();
    let inside = rates::list_prices(&mut tx, hotel.property, ota_nr.id, hotel.day(0), hotel.day(7)).await.unwrap();
    tx.rollback().await.unwrap();
    hotel.try_prices(&bar, &prices).await.unwrap();

    assert_eq!(inside.len(), 1, "the grandchild's row is written before the transaction commits");
    // 100.50 + 15% = 115.575, rounded to 116.00; then -10.50 = 105.50.
    assert_eq!(hotel.stored(&ota).await[0].amount, 11_600);
    assert_eq!(hotel.stored(&ota_nr).await, [Price { amount: 10_550, ..prices[0] }]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn only_hand_priced_plans_take_prices_for_what_they_sell(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let bar =
        hotel.plan(rates::NewRatePlan { room_type_ids: vec![hotel.deluxe.id], ..hotel.standard_plan("BAR", "USD") });
    let bar = bar.await;
    let ota = hotel.plan(hotel.derived_plan("OTA", &bar, ChangeMode::Percent, 1_500)).await;
    let price = |room_type: Uuid, day: i64, occupancy: i32| {
        [Price { room_type_id: room_type, date: hotel.day(day), occupancy, amount: 100 }]
    };

    let derived_plan = hotel.try_prices(&ota, &price(hotel.deluxe.id, 0, 1)).await;
    let unsold_type = hotel.try_prices(&bar, &price(hotel.standard.id, 0, 1)).await;
    let too_many_adults = hotel.try_prices(&bar, &price(hotel.deluxe.id, 0, 4)).await;
    let yesterday = hotel.try_prices(&bar, &price(hotel.deluxe.id, -1, 1)).await;
    let past_the_window = hotel.try_prices(&bar, &price(hotel.deluxe.id, rooms::WINDOW_DAYS, 1)).await;
    let mut tx = hotel.tx().await;
    let unknown_plan = rates::set_prices(
        &mut tx,
        hotel.tenant,
        hotel.user,
        hotel.property,
        Uuid::now_v7(),
        &price(hotel.deluxe.id, 0, 1),
    )
    .await;

    assert_eq!(invalid(derived_plan), "OTA is derived from BAR; change BAR's prices instead");
    assert_eq!(invalid(unsold_type), "BAR does not sell this room type");
    assert_eq!(invalid(too_many_adults), "DLX takes 1 to 3 guests");
    let window = "prices and restrictions are set from the business date for 730 days";
    assert_eq!((invalid(yesterday), invalid(past_the_window)), (window.to_owned(), window.to_owned()));
    assert!(matches!(unknown_plan, Err(RatesError::NotFound("rate plan"))), "{unknown_plan:?}");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_new_or_changed_derived_plan_is_priced_from_its_parent(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let bar = hotel.plan(hotel.standard_plan("BAR", "USD")).await;
    let rack = hotel.plan(hotel.standard_plan("RACK", "USD")).await;
    hotel.try_prices(&bar, &hotel.prices(hotel.deluxe.id, 0, 10, 2, 10_000)).await.unwrap();
    hotel.try_prices(&rack, &hotel.prices(hotel.deluxe.id, 5, 10, 2, 20_000)).await.unwrap();

    let ota = hotel.plan(hotel.derived_plan("OTA", &bar, ChangeMode::Percent, 1_000)).await;
    let ota_nr = hotel.plan(hotel.derived_plan("OTA-NR", &ota, ChangeMode::Amount, -500)).await;
    let created = hotel.stored(&ota_nr).await;
    let cheaper = RatePlanChanges { derive_value: Some(-1_000), ..RatePlanChanges::default() };
    let ota = hotel.try_update(&ota, cheaper).await.unwrap();
    let after_formula = hotel.stored(&ota_nr).await;
    let moved = RatePlanChanges { parent_id: Some(rack.id), ..RatePlanChanges::default() };
    hotel.try_update(&ota, moved).await.unwrap();
    let after_move = hotel.stored(&ota_nr).await;

    assert_eq!(created.len(), 10);
    assert!(created.iter().all(|p| p.amount == 10_500), "110.00 - 5.00");
    assert!(after_formula.iter().all(|p| p.amount == 8_500), "90.00 - 5.00");
    assert_eq!(after_move.len(), 5, "days RACK has no price for are gone");
    assert!(after_move.iter().all(|p| p.date >= hotel.day(5) && p.amount == 17_500), "180.00 - 5.00");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_bulk_change_touches_exactly_what_it_selects_and_cascades(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let bar = hotel.plan(hotel.standard_plan("BAR", "USD")).await;
    let ota = hotel.plan(hotel.derived_plan("OTA", &bar, ChangeMode::Percent, 1_500)).await;
    for room_type in [hotel.deluxe.id, hotel.standard.id] {
        for occupancy in [1, 2] {
            hotel.try_prices(&bar, &hotel.prices(room_type, 0, 60, occupancy, 10_000)).await.unwrap();
        }
    }
    let change = BulkChange {
        from: hotel.day(10),
        to: hotel.day(40),
        weekdays: vec![Weekday::Saturday, Weekday::Sunday],
        room_type_ids: vec![hotel.deluxe.id],
        occupancies: vec![],
        change: PriceChange { mode: PriceChangeMode::Percent, value: 1_000 },
    };

    let mut tx = hotel.tx().await;
    let changed = rates::bulk_change(&mut tx, hotel.tenant, hotel.user, hotel.property, bar.id, &change).await.unwrap();
    tx.commit().await.unwrap();

    let selected = |p: &Price| {
        p.room_type_id == hotel.deluxe.id
            && p.date >= hotel.day(10)
            && p.date < hotel.day(40)
            && matches!(p.date.weekday(), Weekday::Saturday | Weekday::Sunday)
    };
    let bar_prices = hotel.stored(&bar).await;
    assert_eq!(bar_prices.len(), 240);
    assert_eq!(changed, u64::try_from(bar_prices.iter().filter(|p| selected(p)).count()).unwrap());
    assert!((16..=18).contains(&changed), "8 or 9 weekend days, 2 occupancies: {changed}");
    for price in &bar_prices {
        assert_eq!(price.amount, if selected(price) { 11_000 } else { 10_000 }, "{price:?}");
    }
    for price in hotel.stored(&ota).await {
        assert_eq!(price.amount, if selected(&price) { 12_650 } else { 11_500 }, "OTA {price:?}");
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_bulk_set_fills_cells_and_its_preview_changes_nothing(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let bar = hotel.plan(hotel.standard_plan("BAR", "USD")).await;
    hotel.try_prices(&bar, &hotel.prices(hotel.standard.id, 0, 1, 1, 9_000)).await.unwrap();
    let change = BulkChange {
        from: hotel.day(0),
        to: hotel.day(7),
        weekdays: vec![],
        room_type_ids: vec![hotel.standard.id],
        occupancies: vec![],
        change: PriceChange { mode: PriceChangeMode::Set, value: 12_000 },
    };

    let mut tx = hotel.tx().await;
    let preview = rates::preview_bulk_change(&mut tx, hotel.property, bar.id, &change, 5).await.unwrap();
    tx.commit().await.unwrap();
    let before = hotel.stored(&bar).await;
    let mut tx = hotel.tx().await;
    let changed = rates::bulk_change(&mut tx, hotel.tenant, hotel.user, hotel.property, bar.id, &change).await.unwrap();
    tx.commit().await.unwrap();
    let again = rates::preview_bulk_change(&mut hotel.tx().await, hotel.property, bar.id, &change, 5).await.unwrap();

    assert_eq!(preview.total, 14, "7 days, occupancies 1 and 2 of STD");
    assert_eq!(preview.cells.len(), 5, "at most the requested number of cells");
    assert_eq!((preview.cells[0].before, preview.cells[0].after), (Some(9_000), 12_000));
    assert_eq!((preview.cells[1].before, preview.cells[1].occupancy), (None, 2));
    assert_eq!(before.len(), 1, "a preview writes nothing");
    assert_eq!(changed, 14);
    assert!(hotel.stored(&bar).await.iter().all(|p| p.amount == 12_000));
    assert_eq!(again.total, 0, "nothing left to change");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_bulk_change_stays_in_the_window_and_on_hand_priced_plans(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let bar = hotel.plan(hotel.standard_plan("BAR", "USD")).await;
    let ota = hotel.plan(hotel.derived_plan("OTA", &bar, ChangeMode::Percent, 1_500)).await;
    let change = |from: i64, to: i64| BulkChange {
        from: hotel.day(from),
        to: hotel.day(to),
        weekdays: vec![],
        room_type_ids: vec![hotel.deluxe.id],
        occupancies: vec![],
        change: PriceChange { mode: PriceChangeMode::Amount, value: 500 },
    };
    let bulk = |plan: Uuid, change: BulkChange| {
        let hotel = &hotel;
        async move {
            let mut tx = hotel.tx().await;
            rates::bulk_change(&mut tx, hotel.tenant, hotel.user, hotel.property, plan, &change).await
        }
    };

    assert_eq!(invalid(bulk(ota.id, change(0, 7)).await), "OTA is derived from BAR; change BAR's prices instead");
    let window = "prices and restrictions are set from the business date for 730 days";
    assert_eq!(invalid(bulk(bar.id, change(-1, 7)).await), window);
    assert_eq!(invalid(bulk(bar.id, change(0, rooms::WINDOW_DAYS + 1)).await), window);
    assert_eq!(invalid(bulk(bar.id, change(5, 5)).await), "the range ends after it starts");
}
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `DATABASE_URL=$TEST_DATABASE_URL cargo test -p rates --no-fail-fast --test prices --test derivation`

Expected: fails to compile: ``cannot find function `set_prices` in crate `rates` ``, `` `bulk_change` ``, ``cannot find type `Price` ``, ``unresolved imports `rates::BulkChange` …``.

- [ ] **Step 3: Implement**

Modify `modules/rates/src/lib.rs`:

```diff
diff --git a/modules/rates/src/lib.rs b/modules/rates/src/lib.rs
index d5a5052..dac4c02 100644
--- a/modules/rates/src/lib.rs
+++ b/modules/rates/src/lib.rs
@@ -44,6 +44,7 @@ macro_rules! text_enum {
 mod meals;
 mod plans;
 mod policies;
+mod prices;
 
 pub use meals::{
     MealSupplement, MealSupplementChanges, NewMealSupplement, create_meal_supplement, list_meal_supplements,
@@ -57,11 +58,18 @@ pub use policies::{
     CancellationPolicy, CancellationPolicyChanges, CancellationRule, NewCancellationPolicy, Penalty, PenaltyKind,
     create_cancellation_policy, list_cancellation_policies, update_cancellation_policy,
 };
+pub use prices::{
+    BulkChange, BulkPreview, Price, PriceChange, PriceChangeCell, PriceChangeMode, bulk_change, list_prices,
+    preview_bulk_change, set_prices,
+};
 
 use db::{Event, TenantId, Tx, UserId};
 use time::Date;
 use uuid::Uuid;
 
+/// Largest amount of money, in minor units, anywhere in rates (the tables check it too).
+pub const MAX_AMOUNT: i64 = 100_000_000_000;
+
 #[derive(Debug, thiserror::Error)]
 pub enum RatesError {
     /// The property or the named resource does not exist in this tenant.
```

Modify `modules/rates/src/plans.rs`:

```diff
diff --git a/modules/rates/src/plans.rs b/modules/rates/src/plans.rs
index 53a0621..b920572 100644
--- a/modules/rates/src/plans.rs
+++ b/modules/rates/src/plans.rs
@@ -1,8 +1,10 @@
-use crate::{RatesError, audit, business_date, lock_rates, notify, rate_plans_key, violates};
+use crate::prices::{derive_prices, tree_keys};
+use crate::{RatesError, audit, business_date, lock_rates, notify, rate_plans_key, rates_keys, violates};
 use db::{TenantId, Tx, UserId};
 use serde::Serialize;
 use sqlx::Row;
 use sqlx::postgres::PgRow;
+use time::Duration;
 use uuid::Uuid;
 
 /// Most levels of derived plans below a standard plan.
@@ -205,6 +207,21 @@ impl Tree {
         }
         false
     }
+
+    /// The plans derived from `id`, level by level: children, then grandchildren, and so on.
+    pub(crate) fn descendant_levels(&self, id: Uuid) -> Vec<Vec<Uuid>> {
+        let mut levels: Vec<Vec<Uuid>> = Vec::new();
+        let mut current = vec![id];
+        loop {
+            let next: Vec<Uuid> =
+                current.iter().flat_map(|parent| self.children(*parent).map(|child| child.id)).collect();
+            if next.is_empty() {
+                return levels;
+            }
+            levels.push(next.clone());
+            current = next;
+        }
+    }
 }
 
 /// The residency a plan in `segment` is sold to: `FIT_F` and `FIT_L` fix it.
@@ -380,7 +397,7 @@ pub async fn create_rate_plan(
     property: Uuid,
     input: NewRatePlan,
 ) -> Result<RatePlan, RatesError> {
-    business_date(tx, property).await?;
+    let today = business_date(tx, property).await?;
     lock_rates(tx, property).await?;
     let tree = Tree(list_rate_plans(tx, property).await?);
     let residency = residency_for(input.segment, input.residency)?;
@@ -432,8 +449,14 @@ pub async fn create_rate_plan(
         Err(err) => return Err(err.into()),
     }
     set_room_types(tx, tenant, property, id, &types).await?;
+    let mut keys = vec![rate_plans_key(property)];
+    if input.kind == PlanKind::Derived {
+        let end = today + Duration::days(rooms::WINDOW_DAYS);
+        derive_prices(tx, &[vec![id]], None, today, end, false).await?;
+        keys.extend(rates_keys(property, id, today, end));
+    }
     audit(tx, tenant, actor, "rate_plan.created", "rate_plan", id, serde_json::json!({ "code": input.code })).await?;
-    notify(tx, tenant, property, vec![rate_plans_key(property)]).await?;
+    notify(tx, tenant, property, keys).await?;
     load(tx, property, id).await
 }
 
@@ -464,7 +487,7 @@ pub async fn update_rate_plan(
     expected_version: i32,
     changes: RatePlanChanges,
 ) -> Result<RatePlan, RatesError> {
-    business_date(tx, property).await?;
+    let today = business_date(tx, property).await?;
     lock_rates(tx, property).await?;
     let tree = Tree(list_rate_plans(tx, property).await?);
     let current = tree.get(id).ok_or(RatesError::NotFound("rate plan"))?;
@@ -543,6 +566,18 @@ pub async fn update_rate_plan(
     .execute(&mut **tx)
     .await?;
     set_room_types(tx, tenant, property, id, &types).await?;
+    let mut keys = vec![rate_plans_key(property)];
+    let moved = changes.parent_id.is_some_and(|parent| Some(parent) != current.parent_id);
+    let reformulated = changes.derive_mode.is_some_and(|mode| Some(mode) != current.derive_mode)
+        || changes.derive_value.is_some_and(|value| Some(value) != current.derive_value)
+        || changes.rounding_step.is_some_and(|step| step != current.rounding_step);
+    let added_types = types.iter().any(|t| !current.room_type_ids.contains(t));
+    if current.kind == PlanKind::Derived && (moved || reformulated || added_types) {
+        let end = today + Duration::days(rooms::WINDOW_DAYS);
+        let levels: Vec<Vec<Uuid>> = std::iter::once(vec![id]).chain(tree.descendant_levels(id)).collect();
+        derive_prices(tx, &levels, None, today, end, moved).await?;
+        keys.extend(tree_keys(&tree, property, id, today, end));
+    }
     audit(
         tx,
         tenant,
@@ -553,6 +588,6 @@ pub async fn update_rate_plan(
         serde_json::json!({ "parent_id": changes.parent_id, "active": changes.active }),
     )
     .await?;
-    notify(tx, tenant, property, vec![rate_plans_key(property)]).await?;
+    notify(tx, tenant, property, keys).await?;
     load(tx, property, id).await
 }
```

Create `modules/rates/src/prices.rs`:

```rust
//! Prices: set by hand on standard and custom plans, changed in bulk, and derived down the plan tree in the
//! same transaction, one `insert … select … on conflict do update` per level.

use crate::plans::{PlanKind, RatePlan, Tree, list_rate_plans};
use crate::{MAX_AMOUNT, RatesError, audit, business_date, lock_rates, notify, rates_keys};
use db::{TenantId, Tx, UserId};
use serde::Serialize;
use std::collections::BTreeMap;
use time::{Date, Duration, Weekday};
use uuid::Uuid;

/// `amount` minor units, in the plan's currency, for `occupancy` adults in `room_type_id` on `date`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct Price {
    pub room_type_id: Uuid,
    pub date: Date,
    pub occupancy: i32,
    pub amount: i64,
}

text_enum!(
    /// A bulk change adds basis points (`percent`) or minor units (`amount`) to existing prices, rounded to
    /// the plan's step, or `set`s every selected price to the value.
    PriceChangeMode { Percent = "percent", Amount = "amount", Set = "set" }
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PriceChange {
    pub mode: PriceChangeMode,
    pub value: i64,
}

/// The prices of `[from, to)` on `weekdays` (all if empty), for `room_type_ids` (all the plan sells if empty)
/// and `occupancies` (every one if empty), changed by `change`.
#[derive(Debug, Clone)]
pub struct BulkChange {
    pub from: Date,
    pub to: Date,
    pub weekdays: Vec<Weekday>,
    pub room_type_ids: Vec<Uuid>,
    pub occupancies: Vec<i32>,
    pub change: PriceChange,
}

/// One price a bulk change would change: `before` is `None` where `set` adds a price.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct PriceChangeCell {
    pub room_type_id: Uuid,
    pub date: Date,
    pub occupancy: i32,
    pub before: Option<i64>,
    pub after: i64,
}

/// The first cells a bulk change would change, and how many it would change in all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct BulkPreview {
    pub total: i64,
    pub cells: Vec<PriceChangeCell>,
}

const WINDOW_MESSAGE: &str = "prices and restrictions are set from the business date for 730 days";

/// `[from, to)` must be a non-empty range inside the rate window, which starts at the business date.
pub(crate) fn check_range(today: Date, from: Date, to: Date) -> Result<(), RatesError> {
    if from >= to {
        Err(RatesError::Invalid("the range ends after it starts".into()))
    } else if from < today || to > today + Duration::days(rooms::WINDOW_DAYS) {
        Err(RatesError::Invalid(WINDOW_MESSAGE.into()))
    } else {
        Ok(())
    }
}

/// The plan, which must exist and be priced by hand.
pub(crate) fn hand_priced(tree: &Tree, plan: Uuid) -> Result<&RatePlan, RatesError> {
    let plan = tree.get(plan).ok_or(RatesError::NotFound("rate plan"))?;
    match plan.parent_id.and_then(|parent| tree.get(parent)) {
        Some(parent) if plan.kind == PlanKind::Derived => Err(RatesError::Invalid(format!(
            "{} is derived from {}; change {}'s prices instead",
            plan.code, parent.code, parent.code
        ))),
        _ => Ok(plan),
    }
}

/// The room types `plan` sells: id, code and maximum occupancy.
pub(crate) async fn sold_types(tx: &mut Tx, plan: &RatePlan) -> Result<Vec<(Uuid, String, i32)>, sqlx::Error> {
    sqlx::query_as("select id, code, max_occupancy from room_type where id = any($1)")
        .bind(&plan.room_type_ids)
        .fetch_all(&mut **tx)
        .await
}

/// Cache keys for `plan` and every plan derived from it, for each month of `[from, to)`.
pub(crate) fn tree_keys(tree: &Tree, property: Uuid, plan: Uuid, from: Date, to: Date) -> Vec<String> {
    std::iter::once(vec![plan])
        .chain(tree.descendant_levels(plan))
        .flatten()
        .flat_map(|plan| rates_keys(property, plan, from, to))
        .collect()
}

/// Recomputes the prices of the plans in `levels` from their parents' for `room_types` (all if `None`) on
/// `[from, to)`: each level is derived from the one before, the first from its own parents. `prune` first
/// deletes prices whose parent price is gone, which only happens when a plan moved to another parent.
pub(crate) async fn derive_prices(
    tx: &mut Tx,
    levels: &[Vec<Uuid>],
    room_types: Option<&[Uuid]>,
    from: Date,
    to: Date,
    prune: bool,
) -> Result<(), sqlx::Error> {
    for plans in levels {
        if prune {
            sqlx::query(
                "delete from rate_day d using rate_plan c
                 where c.id = d.rate_plan_id and d.rate_plan_id = any($1) and d.date >= $3 and d.date < $4
                   and ($2::uuid[] is null or d.room_type_id = any($2))
                   and not exists (select 1 from rate_day p
                                   where p.rate_plan_id = c.parent_id and p.date = d.date
                                     and p.room_type_id = d.room_type_id and p.occupancy = d.occupancy)",
            )
            .bind(plans)
            .bind(room_types)
            .bind(from)
            .bind(to)
            .execute(&mut **tx)
            .await?;
        }
        sqlx::query(
            "insert into rate_day (tenant_id, property_id, rate_plan_id, room_type_id, date, occupancy, amount)
             select c.tenant_id, c.property_id, c.id, p.room_type_id, p.date, p.occupancy,
                    app.derive_amount(p.amount, c.derive_mode, c.derive_value, c.rounding_step)
             from rate_plan c
             join rate_plan_room_type s on s.rate_plan_id = c.id
             join rate_day p on p.rate_plan_id = c.parent_id and p.room_type_id = s.room_type_id
             where c.id = any($1) and p.date >= $3 and p.date < $4 and ($2::uuid[] is null or p.room_type_id = any($2))
             on conflict (rate_plan_id, date, room_type_id, occupancy) do update set amount = excluded.amount
             where rate_day.amount is distinct from excluded.amount",
        )
        .bind(plans)
        .bind(room_types)
        .bind(from)
        .bind(to)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// Sets prices on a standard or custom plan and reprices the plans derived from it. A cell listed twice takes
/// its last amount. Prices can be changed but not removed: to stop selling a date, close it.
pub async fn set_prices(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    plan: Uuid,
    prices: &[Price],
) -> Result<(), RatesError> {
    let today = business_date(tx, property).await?;
    lock_rates(tx, property).await?;
    let tree = Tree(list_rate_plans(tx, property).await?);
    let plan = hand_priced(&tree, plan)?;
    let types = sold_types(tx, plan).await?;
    let mut cells: BTreeMap<(Date, Uuid, i32), i64> = BTreeMap::new();
    for price in prices {
        let (_, code, max) = types
            .iter()
            .find(|(id, _, _)| *id == price.room_type_id)
            .ok_or_else(|| RatesError::Invalid(format!("{} does not sell this room type", plan.code)))?;
        if !(1..=*max).contains(&price.occupancy) {
            return Err(RatesError::Invalid(format!("{code} takes 1 to {max} guests")));
        }
        check_range(today, price.date, price.date + Duration::days(1))?;
        if !(0..=MAX_AMOUNT).contains(&price.amount) {
            return Err(RatesError::Invalid(format!("a price is 0 to {MAX_AMOUNT} minor units")));
        }
        cells.insert((price.date, price.room_type_id, price.occupancy), price.amount);
    }
    let (Some(first), Some(last)) = (cells.keys().next(), cells.keys().next_back()) else { return Ok(()) };
    let (from, to) = (first.0, last.0 + Duration::days(1));
    sqlx::query(
        "insert into rate_day (tenant_id, property_id, rate_plan_id, room_type_id, date, occupancy, amount)
         select $1, $2, $3, c.room_type_id, c.date, c.occupancy, c.amount
         from unnest($4::date[], $5::uuid[], $6::integer[], $7::bigint[]) as c (date, room_type_id, occupancy, amount)
         on conflict (rate_plan_id, date, room_type_id, occupancy) do update set amount = excluded.amount",
    )
    .bind(tenant.0)
    .bind(property)
    .bind(plan.id)
    .bind(cells.keys().map(|key| key.0).collect::<Vec<_>>())
    .bind(cells.keys().map(|key| key.1).collect::<Vec<_>>())
    .bind(cells.keys().map(|key| key.2).collect::<Vec<_>>())
    .bind(cells.values().copied().collect::<Vec<_>>())
    .execute(&mut **tx)
    .await?;
    let mut touched: Vec<Uuid> = cells.keys().map(|key| key.1).collect();
    touched.sort_unstable();
    touched.dedup();
    derive_prices(tx, &tree.descendant_levels(plan.id), Some(&touched), from, to, false).await?;
    audit(tx, tenant, actor, "rate_plan.prices_set", "rate_plan", plan.id, serde_json::json!({ "count": cells.len() }))
        .await?;
    notify(tx, tenant, property, tree_keys(&tree, property, plan.id, from, to)).await?;
    Ok(())
}

/// The cells a bulk change on `plan` selects, with their price before and after. Binds: `$1` plan, `$2` from,
/// `$3` to, `$4` room types, `$5` ISO weekdays, `$6` occupancies (empty: all), `$7` mode, `$8` value, `$9` the
/// plan's rounding step.
const CHANGES: &str = "
    select d.room_type_id, d.date, d.occupancy, d.amount as before,
           app.derive_amount(d.amount, $7, $8, $9) as after
    from rate_day d
    where $7 <> 'set' and d.rate_plan_id = $1 and d.date >= $2 and d.date < $3 and d.room_type_id = any($4)
      and extract(isodow from d.date)::integer = any($5)
      and (cardinality($6::integer[]) = 0 or d.occupancy = any($6))
      and d.amount <> app.derive_amount(d.amount, $7, $8, $9)
    union all
    select t.id, s.day::date, o.occupancy, d.amount, $8
    from room_type t
    cross join generate_series($2::date, $3::date - 1, interval '1 day') as s (day)
    cross join generate_series(1, t.max_occupancy) as o (occupancy)
    left join rate_day d on d.rate_plan_id = $1 and d.date = s.day::date and d.room_type_id = t.id
                        and d.occupancy = o.occupancy
    where $7 = 'set' and t.id = any($4) and extract(isodow from s.day)::integer = any($5)
      and (cardinality($6::integer[]) = 0 or o.occupancy = any($6))
      and d.amount is distinct from $8";

/// A bulk change, checked, with its selection resolved to the binds [`CHANGES`] takes.
struct Selection {
    room_types: Vec<Uuid>,
    weekdays: Vec<i32>,
}

fn select(today: Date, plan: &RatePlan, change: &BulkChange) -> Result<Selection, RatesError> {
    check_range(today, change.from, change.to)?;
    let value = change.change.value;
    let in_range = match change.change.mode {
        PriceChangeMode::Percent => (-10_000..=100_000).contains(&value),
        PriceChangeMode::Amount => (-MAX_AMOUNT..=MAX_AMOUNT).contains(&value),
        PriceChangeMode::Set => (0..=MAX_AMOUNT).contains(&value),
    };
    if !in_range {
        return Err(RatesError::Invalid(format!(
            "a change is -10000 to 100000 basis points, or at most {MAX_AMOUNT} minor units"
        )));
    }
    if change.occupancies.iter().any(|occupancy| !(1..=50).contains(occupancy)) {
        return Err(RatesError::Invalid("occupancies are 1 to 50".into()));
    }
    if let Some(unsold) = change.room_type_ids.iter().find(|id| !plan.room_type_ids.contains(id)) {
        return Err(RatesError::Invalid(format!("{} does not sell room type {unsold}", plan.code)));
    }
    let room_types =
        if change.room_type_ids.is_empty() { plan.room_type_ids.clone() } else { change.room_type_ids.clone() };
    let weekdays = if change.weekdays.is_empty() {
        (1..=7).collect()
    } else {
        change.weekdays.iter().map(|day| i32::from(day.number_from_monday())).collect()
    };
    Ok(Selection { room_types, weekdays })
}

/// Applies a bulk change to a standard or custom plan and reprices the plans derived from it. Returns how
/// many of the plan's own prices changed.
pub async fn bulk_change(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    plan: Uuid,
    change: &BulkChange,
) -> Result<u64, RatesError> {
    let today = business_date(tx, property).await?;
    lock_rates(tx, property).await?;
    let tree = Tree(list_rate_plans(tx, property).await?);
    let plan = hand_priced(&tree, plan)?;
    let selection = select(today, plan, change)?;
    let changed = sqlx::query(sqlx::AssertSqlSafe(format!(
        "insert into rate_day (tenant_id, property_id, rate_plan_id, room_type_id, date, occupancy, amount)
         select $10, $11, $1, c.room_type_id, c.date, c.occupancy, c.after from ({CHANGES}) c
         on conflict (rate_plan_id, date, room_type_id, occupancy) do update set amount = excluded.amount"
    )))
    .bind(plan.id)
    .bind(change.from)
    .bind(change.to)
    .bind(&selection.room_types)
    .bind(&selection.weekdays)
    .bind(&change.occupancies)
    .bind(change.change.mode.as_str())
    .bind(change.change.value)
    .bind(plan.rounding_step)
    .bind(tenant.0)
    .bind(property)
    .execute(&mut **tx)
    .await?
    .rows_affected();
    derive_prices(tx, &tree.descendant_levels(plan.id), Some(&selection.room_types), change.from, change.to, false)
        .await?;
    audit(
        tx,
        tenant,
        actor,
        "rate_plan.bulk_changed",
        "rate_plan",
        plan.id,
        serde_json::json!({
            "from": change.from, "to": change.to, "mode": change.change.mode.as_str(),
            "value": change.change.value, "changed": changed,
        }),
    )
    .await?;
    notify(tx, tenant, property, tree_keys(&tree, property, plan.id, change.from, change.to)).await?;
    Ok(changed)
}

/// What [`bulk_change`] would change, without changing anything: the first `limit` cells by date, room type
/// and occupancy, and the total.
pub async fn preview_bulk_change(
    tx: &mut Tx,
    property: Uuid,
    plan: Uuid,
    change: &BulkChange,
    limit: i64,
) -> Result<BulkPreview, RatesError> {
    let today = business_date(tx, property).await?;
    let tree = Tree(list_rate_plans(tx, property).await?);
    let plan = hand_priced(&tree, plan)?;
    let selection = select(today, plan, change)?;
    let rows: Vec<(Uuid, Date, i32, Option<i64>, i64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select c.room_type_id, c.date, c.occupancy, c.before, c.after, count(*) over () as total
         from ({CHANGES}) c join room_type rt on rt.id = c.room_type_id
         order by c.date, rt.sort_order, rt.code, c.occupancy
         limit $10"
    )))
    .bind(plan.id)
    .bind(change.from)
    .bind(change.to)
    .bind(&selection.room_types)
    .bind(&selection.weekdays)
    .bind(&change.occupancies)
    .bind(change.change.mode.as_str())
    .bind(change.change.value)
    .bind(plan.rounding_step)
    .bind(limit)
    .fetch_all(&mut **tx)
    .await?;
    Ok(BulkPreview {
        total: rows.first().map_or(0, |row| row.5),
        cells: rows
            .into_iter()
            .map(|(room_type_id, date, occupancy, before, after, _)| PriceChangeCell {
                room_type_id,
                date,
                occupancy,
                before,
                after,
            })
            .collect(),
    })
}

/// A plan's prices on `[from, to)`, by date, room type and occupancy.
pub async fn list_prices(
    tx: &mut Tx,
    property: Uuid,
    plan: Uuid,
    from: Date,
    to: Date,
) -> Result<Vec<Price>, sqlx::Error> {
    sqlx::query_as(
        "select room_type_id, date, occupancy, amount from rate_day
         where property_id = $1 and rate_plan_id = $2 and date >= $3 and date < $4
         order by date, room_type_id, occupancy",
    )
    .bind(property)
    .bind(plan)
    .bind(from)
    .bind(to)
    .fetch_all(&mut **tx)
    .await
}
```

- [ ] **Step 4: Run the checks**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p rates
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: `prices` 6 passed, `derivation` 1 passed; earlier rates tests still pass.

- [ ] **Step 5: Commit**

```sh
git add -A
git commit -m "feat(rates): prices set by hand or in bulk, with derived plans repriced in the same transaction and a property-based chain test"
```

### Task 8: Restrictions, and plans that inherit them

Restrictions (closed, minimum and maximum stay, closed to arrival or departure) are set per plan, room type
and date over a range; fields left out stay as they are. A derived plan that inherits restrictions holds a
copy of its parent's rows, rewritten level by level with every change, and refuses its own; turning
inheritance on replaces its rows with the parent's, turning it off keeps the copy.

**Files:**
- Modify: `modules/rates/src/lib.rs`
- Modify: `modules/rates/src/plans.rs`
- Create: `modules/rates/src/restrictions.rs`
- Test: `modules/rates/tests/common/mod.rs`
- Test: `modules/rates/tests/restrictions.rs` (new)

**Interfaces:**
- Produces: `Restriction`, `RestrictionChange { from, to, weekdays, room_type_ids, closed, min_stay: Option<Option<i32>>, max_stay, closed_to_arrival, closed_to_departure }`, `set_restrictions(…) -> u64`, `list_restrictions(tx, property, plan, from, to)`; crate-internal `derive_restrictions`, `inheriting_levels`.

- [ ] **Step 1: Write the failing tests**

Modify `modules/rates/tests/common/mod.rs`:

```diff
diff --git a/modules/rates/tests/common/mod.rs b/modules/rates/tests/common/mod.rs
index 9754488..52646db 100644
--- a/modules/rates/tests/common/mod.rs
+++ b/modules/rates/tests/common/mod.rs
@@ -169,3 +169,34 @@ impl Hotel {
         rates::list_prices(&mut self.tx().await, self.property, plan.id, window.0, window.1).await.unwrap()
     }
 }
+
+impl Hotel {
+    /// Restrictions on every day of `[from, to)` for `room_types`, with no field set yet.
+    pub fn restrict(&self, room_types: &[Uuid], from: i64, to: i64) -> rates::RestrictionChange {
+        rates::RestrictionChange {
+            from: self.day(from),
+            to: self.day(to),
+            weekdays: vec![],
+            room_type_ids: room_types.to_vec(),
+            closed: None,
+            min_stay: None,
+            max_stay: None,
+            closed_to_arrival: None,
+            closed_to_departure: None,
+        }
+    }
+
+    /// Sets restrictions in their own transaction, committed if it succeeds.
+    pub async fn try_restrict(&self, plan: &RatePlan, change: rates::RestrictionChange) -> Result<u64, RatesError> {
+        let mut tx = self.tx().await;
+        let changed = rates::set_restrictions(&mut tx, self.tenant, self.user, self.property, plan.id, &change).await?;
+        tx.commit().await.unwrap();
+        Ok(changed)
+    }
+
+    /// `plan`'s restrictions over the whole rate window, by date and room type.
+    pub async fn restrictions(&self, plan: &RatePlan) -> Vec<rates::Restriction> {
+        let window = (self.day(0), self.day(rooms::WINDOW_DAYS));
+        rates::list_restrictions(&mut self.tx().await, self.property, plan.id, window.0, window.1).await.unwrap()
+    }
+}
```

Create `modules/rates/tests/restrictions.rs`:

```rust
mod common;

use common::Hotel;
use rates::{ChangeMode, RatePlanChanges, RatesError, Restriction, RestrictionChange};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

fn invalid(result: Result<impl std::fmt::Debug, RatesError>) -> String {
    match result {
        Err(RatesError::Invalid(message)) => message,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

/// `(day offset, room type code, closed, min stay, closed to arrival)` of each row, for compact assertions.
fn summary(hotel: &Hotel, rows: &[Restriction]) -> Vec<(i64, &'static str, bool, Option<i32>, bool)> {
    rows.iter()
        .map(|r| {
            let code = if r.room_type_id == hotel.deluxe.id { "DLX" } else { "STD" };
            ((r.date - hotel.day(0)).whole_days(), code, r.closed, r.min_stay, r.closed_to_arrival)
        })
        .collect()
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn restrictions_are_set_per_day_and_fields_left_out_stay(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let bar = hotel.plan(hotel.standard_plan("BAR", "USD")).await;
    let (deluxe, standard) = (hotel.deluxe.id, hotel.standard.id);

    let closed = hotel.try_restrict(&bar, RestrictionChange { closed: Some(true), ..hotel.restrict(&[deluxe], 0, 2) });
    closed.await.unwrap();
    let min_stay = RestrictionChange { min_stay: Some(Some(2)), ..hotel.restrict(&[deluxe, standard], 1, 3) };
    hotel.try_restrict(&bar, min_stay).await.unwrap();
    let arrival = RestrictionChange {
        closed_to_arrival: Some(true),
        weekdays: vec![hotel.day(1).weekday()],
        ..hotel.restrict(&[standard], 0, 14)
    };
    let arrivals_closed = hotel.try_restrict(&bar, arrival).await.unwrap();
    let before_clearing = hotel.restrictions(&bar).await;
    let cleared = RestrictionChange { min_stay: Some(None), ..hotel.restrict(&[deluxe, standard], 0, 14) };
    hotel.try_restrict(&bar, cleared).await.unwrap();

    assert_eq!(arrivals_closed, 2, "two of the fourteen days fall on that weekday");
    let week = |day: i64| (day, "STD", false, None, true);
    assert_eq!(
        summary(&hotel, &before_clearing),
        [
            (0, "DLX", true, None, false),
            (1, "DLX", true, Some(2), false),
            (1, "STD", false, Some(2), true),
            (2, "DLX", false, Some(2), false),
            (2, "STD", false, Some(2), false),
            week(8),
        ]
    );
    assert!(hotel.restrictions(&bar).await.iter().all(|r| r.min_stay.is_none()));
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn inheriting_plans_hold_a_copy_of_their_parents_restrictions(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let bar = hotel.plan(hotel.standard_plan("BAR", "USD")).await;
    let inheriting = |code: &str, parent: &rates::RatePlan| rates::NewRatePlan {
        inherit_restrictions: true,
        ..hotel.derived_plan(code, parent, ChangeMode::Percent, 1_000)
    };
    let ota = hotel.plan(inheriting("OTA", &bar)).await;
    let ota_nr = hotel.plan(inheriting("OTA-NR", &ota)).await;
    let own = hotel.plan(hotel.derived_plan("OWN", &bar, ChangeMode::Percent, 1_000)).await;
    let (deluxe, standard) = (hotel.deluxe.id, hotel.standard.id);

    let parent = RestrictionChange { closed: Some(true), min_stay: Some(Some(3)), ..hotel.restrict(&[deluxe], 0, 3) };
    hotel.try_restrict(&bar, parent).await.unwrap();
    let on_ota = hotel.try_restrict(&ota, RestrictionChange { closed: Some(false), ..hotel.restrict(&[deluxe], 0, 1) });
    let on_ota = on_ota.await;
    let on_own = RestrictionChange { closed_to_arrival: Some(true), ..hotel.restrict(&[standard], 5, 6) };
    hotel.try_restrict(&own, on_own).await.unwrap();

    let bar_rows = hotel.restrictions(&bar).await;
    assert_eq!(bar_rows.len(), 3);
    assert_eq!(hotel.restrictions(&ota).await, bar_rows, "a copy, rewritten with the parent's");
    assert_eq!(hotel.restrictions(&ota_nr).await, bar_rows, "inherited down the chain");
    assert_eq!(summary(&hotel, &hotel.restrictions(&own).await), [(5, "STD", false, None, true)]);
    assert_eq!(invalid(on_ota), "OTA inherits BAR's restrictions; change them there, or stop inheriting");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn inheriting_replaces_own_restrictions_and_stopping_keeps_the_copy(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let bar = hotel.plan(hotel.standard_plan("BAR", "USD")).await;
    let own = hotel.plan(hotel.derived_plan("OWN", &bar, ChangeMode::Percent, 1_000)).await;
    let deluxe = hotel.deluxe.id;
    hotel
        .try_restrict(&bar, RestrictionChange { min_stay: Some(Some(3)), ..hotel.restrict(&[deluxe], 1, 2) })
        .await
        .unwrap();
    hotel
        .try_restrict(&own, RestrictionChange { closed: Some(true), ..hotel.restrict(&[deluxe], 5, 6) })
        .await
        .unwrap();

    let inherit = |on: bool| RatePlanChanges { inherit_restrictions: Some(on), ..RatePlanChanges::default() };
    let own = hotel.try_update(&own, inherit(true)).await.unwrap();
    let while_inheriting = hotel.restrictions(&own).await;
    let own = hotel.try_update(&own, inherit(false)).await.unwrap();
    let after_stopping = hotel.restrictions(&own).await;
    let own_again = RestrictionChange { closed: Some(true), ..hotel.restrict(&[deluxe], 1, 2) };
    hotel.try_restrict(&own, own_again).await.unwrap();

    assert_eq!(summary(&hotel, &while_inheriting), [(1, "DLX", false, Some(3), false)]);
    assert_eq!(after_stopping, while_inheriting);
    assert_eq!(summary(&hotel, &hotel.restrictions(&own).await), [(1, "DLX", true, Some(3), false)]);
    assert_eq!(summary(&hotel, &hotel.restrictions(&bar).await), [(1, "DLX", false, Some(3), false)]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn restrictions_are_checked(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let bar =
        hotel.plan(rates::NewRatePlan { room_type_ids: vec![hotel.deluxe.id], ..hotel.standard_plan("BAR", "USD") });
    let bar = bar.await;
    let deluxe = hotel.deluxe.id;
    hotel
        .try_restrict(&bar, RestrictionChange { max_stay: Some(Some(5)), ..hotel.restrict(&[deluxe], 0, 7) })
        .await
        .unwrap();

    let longer_than_max = RestrictionChange { min_stay: Some(Some(7)), ..hotel.restrict(&[deluxe], 0, 7) };
    let zero_nights = RestrictionChange { min_stay: Some(Some(0)), ..hotel.restrict(&[deluxe], 0, 7) };
    let unsold = RestrictionChange { closed: Some(true), ..hotel.restrict(&[hotel.standard.id], 0, 7) };
    let past = RestrictionChange { closed: Some(true), ..hotel.restrict(&[deluxe], -1, 7) };
    let nothing = hotel.restrict(&[deluxe], 0, 7);

    assert_eq!(
        invalid(hotel.try_restrict(&bar, longer_than_max).await),
        "a minimum stay cannot exceed the maximum stay"
    );
    assert_eq!(invalid(hotel.try_restrict(&bar, zero_nights).await), "minimum and maximum stays are 1 to 365 nights");
    assert_eq!(invalid(hotel.try_restrict(&bar, unsold).await), "BAR does not sell this room type");
    assert_eq!(
        invalid(hotel.try_restrict(&bar, past).await),
        "prices and restrictions are set from the business date for 730 days"
    );
    assert_eq!(invalid(hotel.try_restrict(&bar, nothing).await), "set at least one restriction");
}
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `DATABASE_URL=$TEST_DATABASE_URL cargo test -p rates --test restrictions`

Expected: fails to compile: ``unresolved imports `rates::Restriction`, `rates::RestrictionChange` ``, ``cannot find function `set_restrictions` in crate `rates` ``.

- [ ] **Step 3: Implement**

Modify `modules/rates/src/lib.rs`:

```diff
diff --git a/modules/rates/src/lib.rs b/modules/rates/src/lib.rs
index dac4c02..873d743 100644
--- a/modules/rates/src/lib.rs
+++ b/modules/rates/src/lib.rs
@@ -45,6 +45,7 @@ mod meals;
 mod plans;
 mod policies;
 mod prices;
+mod restrictions;
 
 pub use meals::{
     MealSupplement, MealSupplementChanges, NewMealSupplement, create_meal_supplement, list_meal_supplements,
@@ -62,6 +63,7 @@ pub use prices::{
     BulkChange, BulkPreview, Price, PriceChange, PriceChangeCell, PriceChangeMode, bulk_change, list_prices,
     preview_bulk_change, set_prices,
 };
+pub use restrictions::{Restriction, RestrictionChange, list_restrictions, set_restrictions};
 
 use db::{Event, TenantId, Tx, UserId};
 use time::Date;
```

Modify `modules/rates/src/plans.rs`:

```diff
diff --git a/modules/rates/src/plans.rs b/modules/rates/src/plans.rs
index b920572..af94d40 100644
--- a/modules/rates/src/plans.rs
+++ b/modules/rates/src/plans.rs
@@ -1,4 +1,5 @@
 use crate::prices::{derive_prices, tree_keys};
+use crate::restrictions::{derive_restrictions, inheriting_levels};
 use crate::{RatesError, audit, business_date, lock_rates, notify, rate_plans_key, rates_keys, violates};
 use db::{TenantId, Tx, UserId};
 use serde::Serialize;
@@ -453,6 +454,9 @@ pub async fn create_rate_plan(
     if input.kind == PlanKind::Derived {
         let end = today + Duration::days(rooms::WINDOW_DAYS);
         derive_prices(tx, &[vec![id]], None, today, end, false).await?;
+        if input.inherit_restrictions {
+            derive_restrictions(tx, &[vec![id]], None, today, end).await?;
+        }
         keys.extend(rates_keys(property, id, today, end));
     }
     audit(tx, tenant, actor, "rate_plan.created", "rate_plan", id, serde_json::json!({ "code": input.code })).await?;
@@ -572,12 +576,20 @@ pub async fn update_rate_plan(
         || changes.derive_value.is_some_and(|value| Some(value) != current.derive_value)
         || changes.rounding_step.is_some_and(|step| step != current.rounding_step);
     let added_types = types.iter().any(|t| !current.room_type_ids.contains(t));
+    let end = today + Duration::days(rooms::WINDOW_DAYS);
     if current.kind == PlanKind::Derived && (moved || reformulated || added_types) {
-        let end = today + Duration::days(rooms::WINDOW_DAYS);
         let levels: Vec<Vec<Uuid>> = std::iter::once(vec![id]).chain(tree.descendant_levels(id)).collect();
         derive_prices(tx, &levels, None, today, end, moved).await?;
         keys.extend(tree_keys(&tree, property, id, today, end));
     }
+    // A plan that starts inheriting, or inherits from a new parent or for new room types, takes a fresh copy;
+    // one that stops inheriting keeps its copy as its own restrictions.
+    let inherits = changes.inherit_restrictions.unwrap_or(current.inherit_restrictions);
+    if inherits && (!current.inherit_restrictions || moved || added_types) {
+        let levels: Vec<Vec<Uuid>> = std::iter::once(vec![id]).chain(inheriting_levels(&tree, id)).collect();
+        derive_restrictions(tx, &levels, None, today, end).await?;
+        keys.extend(tree_keys(&tree, property, id, today, end));
+    }
     audit(
         tx,
         tenant,
```

Create `modules/rates/src/restrictions.rs`:

```rust
//! Restrictions per plan, room type and date. A derived plan that inherits restrictions holds a copy of its
//! parent's rows, rewritten in the same transaction as every change to them.

use crate::plans::{RatePlan, Tree, list_rate_plans};
use crate::prices::{check_range, sold_types};
use crate::{RatesError, audit, business_date, lock_rates, notify, rates_keys, violates};
use db::{TenantId, Tx, UserId};
use serde::Serialize;
use time::{Date, Weekday};
use uuid::Uuid;

/// A plan's restrictions for a room type on a date. Days without a row have none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, sqlx::FromRow, utoipa::ToSchema)]
pub struct Restriction {
    pub room_type_id: Uuid,
    pub date: Date,
    /// Not sold that night.
    pub closed: bool,
    /// Fewest nights a stay including this night may have.
    pub min_stay: Option<i32>,
    /// Most nights a stay including this night may have.
    pub max_stay: Option<i32>,
    /// A stay may not start on this date.
    pub closed_to_arrival: bool,
    /// A stay may not end (check out) on this date.
    pub closed_to_departure: bool,
}

/// Restrictions to set on every day of `[from, to)` on `weekdays` (all if empty) for `room_type_ids` (all the
/// plan sells if empty). `None` leaves a field as it is; `min_stay` or `max_stay` of `Some(None)` removes it.
#[derive(Debug, Clone)]
pub struct RestrictionChange {
    pub from: Date,
    pub to: Date,
    pub weekdays: Vec<Weekday>,
    pub room_type_ids: Vec<Uuid>,
    pub closed: Option<bool>,
    pub min_stay: Option<Option<i32>>,
    pub max_stay: Option<Option<i32>>,
    pub closed_to_arrival: Option<bool>,
    pub closed_to_departure: Option<bool>,
}

impl RestrictionChange {
    fn is_empty(&self) -> bool {
        self.closed.is_none()
            && self.min_stay.is_none()
            && self.max_stay.is_none()
            && self.closed_to_arrival.is_none()
            && self.closed_to_departure.is_none()
    }
}

/// The plans below `id` that inherit restrictions, level by level. A plan that sets its own restrictions
/// ends its branch: the plans below it inherit from it.
pub(crate) fn inheriting_levels(tree: &Tree, id: Uuid) -> Vec<Vec<Uuid>> {
    let mut levels: Vec<Vec<Uuid>> = Vec::new();
    let mut current = vec![id];
    loop {
        let next: Vec<Uuid> = tree
            .0
            .iter()
            .filter(|plan| plan.inherit_restrictions && plan.parent_id.is_some_and(|p| current.contains(&p)))
            .map(|plan| plan.id)
            .collect();
        if next.is_empty() {
            return levels;
        }
        levels.push(next.clone());
        current = next;
    }
}

/// Copies restrictions down `levels` for `room_types` (all if `None`) on `[from, to)`: each level gets its
/// parents' rows, and loses rows its parents do not have.
pub(crate) async fn derive_restrictions(
    tx: &mut Tx,
    levels: &[Vec<Uuid>],
    room_types: Option<&[Uuid]>,
    from: Date,
    to: Date,
) -> Result<(), sqlx::Error> {
    for plans in levels {
        sqlx::query(
            "delete from rate_restriction r using rate_plan c
             where c.id = r.rate_plan_id and r.rate_plan_id = any($1) and r.date >= $3 and r.date < $4
               and ($2::uuid[] is null or r.room_type_id = any($2))
               and not exists (select 1 from rate_restriction p
                               where p.rate_plan_id = c.parent_id and p.date = r.date
                                 and p.room_type_id = r.room_type_id)",
        )
        .bind(plans)
        .bind(room_types)
        .bind(from)
        .bind(to)
        .execute(&mut **tx)
        .await?;
        sqlx::query(
            "insert into rate_restriction (tenant_id, property_id, rate_plan_id, room_type_id, date, closed, min_stay,
                                           max_stay, closed_to_arrival, closed_to_departure)
             select c.tenant_id, c.property_id, c.id, p.room_type_id, p.date, p.closed, p.min_stay, p.max_stay,
                    p.closed_to_arrival, p.closed_to_departure
             from rate_plan c
             join rate_plan_room_type s on s.rate_plan_id = c.id
             join rate_restriction p on p.rate_plan_id = c.parent_id and p.room_type_id = s.room_type_id
             where c.id = any($1) and p.date >= $3 and p.date < $4 and ($2::uuid[] is null or p.room_type_id = any($2))
             on conflict (rate_plan_id, date, room_type_id) do update
             set closed = excluded.closed, min_stay = excluded.min_stay, max_stay = excluded.max_stay,
                 closed_to_arrival = excluded.closed_to_arrival, closed_to_departure = excluded.closed_to_departure",
        )
        .bind(plans)
        .bind(room_types)
        .bind(from)
        .bind(to)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// A plan whose own restrictions may be set: one that does not inherit them.
fn own_restrictions(tree: &Tree, plan: Uuid) -> Result<&RatePlan, RatesError> {
    let plan = tree.get(plan).ok_or(RatesError::NotFound("rate plan"))?;
    match plan.parent_id.and_then(|parent| tree.get(parent)) {
        Some(parent) if plan.inherit_restrictions => Err(RatesError::Invalid(format!(
            "{} inherits {}'s restrictions; change them there, or stop inheriting",
            plan.code, parent.code
        ))),
        _ => Ok(plan),
    }
}

/// Sets restrictions on the selected days and copies them to the plans that inherit them. Returns how many
/// of the plan's own days were written.
pub async fn set_restrictions(
    tx: &mut Tx,
    tenant: TenantId,
    actor: UserId,
    property: Uuid,
    plan: Uuid,
    change: &RestrictionChange,
) -> Result<u64, RatesError> {
    let today = business_date(tx, property).await?;
    lock_rates(tx, property).await?;
    let tree = Tree(list_rate_plans(tx, property).await?);
    let plan = own_restrictions(&tree, plan)?;
    if change.is_empty() {
        return Err(RatesError::Invalid("set at least one restriction".into()));
    }
    check_range(today, change.from, change.to)?;
    let stays = [change.min_stay, change.max_stay].into_iter().flatten().flatten();
    if stays.into_iter().any(|nights| !(1..=365).contains(&nights)) {
        return Err(RatesError::Invalid("minimum and maximum stays are 1 to 365 nights".into()));
    }
    let sold = sold_types(tx, plan).await?;
    if change.room_type_ids.iter().any(|id| !sold.iter().any(|(sold, _, _)| sold == id)) {
        return Err(RatesError::Invalid(format!("{} does not sell this room type", plan.code)));
    }
    let room_types =
        if change.room_type_ids.is_empty() { plan.room_type_ids.clone() } else { change.room_type_ids.clone() };
    let weekdays: Vec<i32> = if change.weekdays.is_empty() {
        (1..=7).collect()
    } else {
        change.weekdays.iter().map(|day| i32::from(day.number_from_monday())).collect()
    };
    let written = sqlx::query(
        "insert into rate_restriction (tenant_id, property_id, rate_plan_id, room_type_id, date, closed, min_stay,
                                       max_stay, closed_to_arrival, closed_to_departure)
         select $1, $2, $3, t.id, s.day::date, coalesce($8, false), case when $9 then $10 end,
                case when $11 then $12 end, coalesce($13, false), coalesce($14, false)
         from unnest($4::uuid[]) as t (id)
         cross join generate_series($5::date, $6::date - 1, interval '1 day') as s (day)
         where extract(isodow from s.day)::integer = any($7)
         on conflict (rate_plan_id, date, room_type_id) do update
         set closed = coalesce($8, rate_restriction.closed),
             min_stay = case when $9 then $10 else rate_restriction.min_stay end,
             max_stay = case when $11 then $12 else rate_restriction.max_stay end,
             closed_to_arrival = coalesce($13, rate_restriction.closed_to_arrival),
             closed_to_departure = coalesce($14, rate_restriction.closed_to_departure)",
    )
    .bind(tenant.0)
    .bind(property)
    .bind(plan.id)
    .bind(&room_types)
    .bind(change.from)
    .bind(change.to)
    .bind(&weekdays)
    .bind(change.closed)
    .bind(change.min_stay.is_some())
    .bind(change.min_stay.flatten())
    .bind(change.max_stay.is_some())
    .bind(change.max_stay.flatten())
    .bind(change.closed_to_arrival)
    .bind(change.closed_to_departure)
    .execute(&mut **tx)
    .await;
    let written = match written {
        Ok(result) => result.rows_affected(),
        Err(err) if violates(&err, "rate_restriction_check") => {
            return Err(RatesError::Invalid("a minimum stay cannot exceed the maximum stay".into()));
        }
        Err(err) => return Err(err.into()),
    };
    let levels = inheriting_levels(&tree, plan.id);
    derive_restrictions(tx, &levels, Some(&room_types), change.from, change.to).await?;
    audit(
        tx,
        tenant,
        actor,
        "rate_plan.restrictions_set",
        "rate_plan",
        plan.id,
        serde_json::json!({ "from": change.from, "to": change.to, "written": written }),
    )
    .await?;
    let keys = std::iter::once(plan.id)
        .chain(levels.into_iter().flatten())
        .flat_map(|plan| rates_keys(property, plan, change.from, change.to))
        .collect();
    notify(tx, tenant, property, keys).await?;
    Ok(written)
}

/// A plan's restrictions on `[from, to)`, by date and room type.
pub async fn list_restrictions(
    tx: &mut Tx,
    property: Uuid,
    plan: Uuid,
    from: Date,
    to: Date,
) -> Result<Vec<Restriction>, sqlx::Error> {
    sqlx::query_as(
        "select room_type_id, date, closed, min_stay, max_stay, closed_to_arrival, closed_to_departure
         from rate_restriction
         where property_id = $1 and rate_plan_id = $2 and date >= $3 and date < $4
         order by date, room_type_id",
    )
    .bind(property)
    .bind(plan)
    .bind(from)
    .bind(to)
    .fetch_all(&mut **tx)
    .await
}
```

- [ ] **Step 4: Run the checks**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p rates
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: `restrictions` 4 passed; earlier rates tests still pass.

- [ ] **Step 5: Commit**

```sh
git add -A
git commit -m "feat(rates): restrictions per plan, room type and date, copied to plans that inherit them"
```

### Task 9: The quote

`quote(&QuoteRequest, &QuoteData) -> Quote` is a pure function: nights with room and meal amounts, the total
in the plan's currency, and every violation (inactive plan, residency, room type not sold, occupancy, meal
plan not allowed, no price, no supplement, closed, minimum and maximum stay on any night, closed to arrival
on the check-in date only, closed to departure on the check-out date only). A missing occupancy falls back to
the nearest lower one plus the extra-adult amount. Unit tests in `quote.rs` cover the rules on hand-built
rows; `load_quote` reads the rows and is tested against the database.

**Files:**
- Modify: `modules/rates/src/lib.rs`
- Create: `modules/rates/src/quote.rs`
- Test: `modules/rates/tests/quote.rs` (new)

**Interfaces:**
- Produces: `QuoteRequest { room_type_id, rate_plan_id, meal_plan, check_in, check_out, adults, children, residency }`, `QuoteRoomType`, `QuoteData`, `QuoteNight`, `ViolationKind`, `Violation`, `Quote { nights, total, currency, restrictions_ok, violations }`, `quote(&QuoteRequest, &QuoteData) -> Quote`, `load_quote(tx, property, &QuoteRequest) -> Result<Quote, RatesError>`. Phase 3 reservations and the Phase 9 booking engine call these.

- [ ] **Step 1: Write the failing tests**

Create `modules/rates/tests/quote.rs`:

```rust
mod common;

use common::Hotel;
use rates::{
    ChangeMode, MealPlan, NewMealSupplement, QuoteNight, QuoteRequest, RatesError, Residency, RestrictionChange,
    Segment, ViolationKind,
};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use uuid::Uuid;

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_stay_is_quoted_from_the_stored_rows(_: PgPoolOptions, opts: PgConnectOptions) {
    let hotel = Hotel::new(opts).await;
    let foreign = hotel.plan(rates::NewRatePlan { segment: Segment::FitF, ..hotel.standard_plan("FITF", "USD") }).await;
    let ota = hotel.plan(hotel.derived_plan("OTA", &foreign, ChangeMode::Percent, 1_500)).await;
    hotel.try_prices(&foreign, &hotel.prices(hotel.deluxe.id, 0, 3, 2, 10_000)).await.unwrap();
    hotel.try_prices(&foreign, &hotel.prices(hotel.standard.id, 0, 3, 2, 5_000)).await.unwrap();
    let mut tx = hotel.tx().await;
    for (currency, adult_amount) in [("USD", 1_500), ("LKR", 450_000)] {
        let breakfast = NewMealSupplement {
            meal_plan: MealPlan::Bb,
            currency: currency.into(),
            adult_amount,
            child_amount: 750,
            from: hotel.day(-10),
            to: None,
        };
        rates::create_meal_supplement(&mut tx, hotel.tenant, hotel.user, hotel.property, breakfast).await.unwrap();
    }
    tx.commit().await.unwrap();
    let departures = RestrictionChange { closed_to_departure: Some(true), ..hotel.restrict(&[hotel.deluxe.id], 3, 4) };
    hotel.try_restrict(&ota, departures).await.unwrap();
    let stay = |plan: &rates::RatePlan, residency: Residency| QuoteRequest {
        room_type_id: hotel.deluxe.id,
        rate_plan_id: plan.id,
        meal_plan: MealPlan::Bb,
        check_in: hotel.day(0),
        check_out: hotel.day(3),
        adults: 2,
        children: 1,
        residency,
    };

    let mut tx = hotel.tx().await;
    let quoted = rates::load_quote(&mut tx, hotel.property, &stay(&ota, Residency::NonResident)).await.unwrap();
    let resident = rates::load_quote(&mut tx, hotel.property, &stay(&foreign, Residency::Resident)).await.unwrap();
    let backwards = QuoteRequest { check_out: hotel.day(0), ..stay(&ota, Residency::NonResident) };
    let backwards = rates::load_quote(&mut tx, hotel.property, &backwards).await;
    let unknown = QuoteRequest { rate_plan_id: Uuid::now_v7(), ..stay(&ota, Residency::NonResident) };
    let unknown = rates::load_quote(&mut tx, hotel.property, &unknown).await;

    let night = |day: i64| QuoteNight { date: hotel.day(day), room: 11_500, meal: 3_750 };
    assert_eq!(quoted.nights, [night(0), night(1), night(2)], "the derived plan's resolved price, USD breakfast");
    assert_eq!((quoted.total, quoted.currency.as_str(), quoted.restrictions_ok), (45_750, "USD", false));
    let kinds: Vec<_> = quoted.violations.iter().map(|v| (v.kind, v.date)).collect();
    assert_eq!(kinds, [(ViolationKind::ClosedToDeparture, Some(hotel.day(3)))]);
    assert_eq!(resident.violations[0].kind, ViolationKind::Residency);
    assert!(matches!(&backwards, Err(RatesError::Invalid(m)) if m == "check-out is after check-in"), "{backwards:?}");
    assert!(matches!(unknown, Err(RatesError::NotFound("rate plan"))), "{unknown:?}");
}
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `DATABASE_URL=$TEST_DATABASE_URL cargo test -p rates --test quote`

Expected: fails to compile: ``unresolved imports `rates::QuoteNight`, `rates::QuoteRequest`, `rates::ViolationKind` ``, ``cannot find function `load_quote` in crate `rates` ``.

- [ ] **Step 3: Implement**

`quote.rs` carries its own unit tests (`#[cfg(test)] mod tests`), which run in the next step.

Modify `modules/rates/src/lib.rs`:

```diff
diff --git a/modules/rates/src/lib.rs b/modules/rates/src/lib.rs
index 873d743..aa61f0e 100644
--- a/modules/rates/src/lib.rs
+++ b/modules/rates/src/lib.rs
@@ -45,6 +45,7 @@ mod meals;
 mod plans;
 mod policies;
 mod prices;
+mod quote;
 mod restrictions;
 
 pub use meals::{
@@ -63,6 +64,9 @@ pub use prices::{
     BulkChange, BulkPreview, Price, PriceChange, PriceChangeCell, PriceChangeMode, bulk_change, list_prices,
     preview_bulk_change, set_prices,
 };
+pub use quote::{
+    Quote, QuoteData, QuoteNight, QuoteRequest, QuoteRoomType, Violation, ViolationKind, load_quote, quote,
+};
 pub use restrictions::{Restriction, RestrictionChange, list_restrictions, set_restrictions};
 
 use db::{Event, TenantId, Tx, UserId};
```

Create `modules/rates/src/quote.rs`:

```rust
//! Price quotes: what a stay costs on a plan, night by night, and every reason it cannot be sold. [`quote`] is
//! a pure function of rows [`load_quote`] reads, so reservations (Phase 3) and the booking engine (Phase 9)
//! reuse it with the same rules.

use crate::plans::{MealPlan, RatePlan, Residency, list_rate_plans};
use crate::{MealSupplement, Price, RatesError, Restriction};
use db::Tx;
use serde::Serialize;
use time::{Date, Duration};
use uuid::Uuid;

/// A stay to price: `[check_in, check_out)`, for guests of `residency`.
#[derive(Debug, Clone)]
pub struct QuoteRequest {
    pub room_type_id: Uuid,
    pub rate_plan_id: Uuid,
    pub meal_plan: MealPlan,
    pub check_in: Date,
    pub check_out: Date,
    pub adults: i32,
    pub children: i32,
    pub residency: Residency,
}

/// The room type's capacity, as the quote needs it.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct QuoteRoomType {
    pub id: Uuid,
    pub code: String,
    pub max_adults: i32,
    pub max_children: i32,
    pub max_occupancy: i32,
}

/// The rows a quote reads: the plan's prices for the room type on the stay's nights, its restrictions on those
/// nights and the departure date, and the supplements for the chosen meal plan in the plan's currency.
#[derive(Debug, Clone)]
pub struct QuoteData {
    pub plan: RatePlan,
    pub room_type: QuoteRoomType,
    pub prices: Vec<Price>,
    pub restrictions: Vec<Restriction>,
    pub supplements: Vec<MealSupplement>,
}

/// One night: the room and the meal supplement, in the plan's currency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct QuoteNight {
    pub date: Date,
    pub room: i64,
    pub meal: i64,
}

text_enum!(
    /// Why a stay cannot be sold as quoted.
    ViolationKind {
        Inactive = "inactive",
        Residency = "residency",
        RoomTypeNotSold = "room_type_not_sold",
        Occupancy = "occupancy",
        MealPlanNotAllowed = "meal_plan_not_allowed",
        NoPrice = "no_price",
        NoMealSupplement = "no_meal_supplement",
        Closed = "closed",
        MinStay = "min_stay",
        MaxStay = "max_stay",
        ClosedToArrival = "closed_to_arrival",
        ClosedToDeparture = "closed_to_departure",
    }
);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct Violation {
    pub kind: ViolationKind,
    /// The night (or arrival or departure date) it concerns, if it concerns one.
    pub date: Option<Date>,
    pub message: String,
}

/// What a stay costs. `restrictions_ok` is true when `violations` is empty: nothing stops the sale.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct Quote {
    pub nights: Vec<QuoteNight>,
    pub total: i64,
    pub currency: String,
    pub restrictions_ok: bool,
    pub violations: Vec<Violation>,
}

/// The nights of `[check_in, check_out)`.
fn nights(check_in: Date, check_out: Date) -> Vec<Date> {
    std::iter::successors(Some(check_in), |night| Some(*night + Duration::days(1)))
        .take_while(|night| *night < check_out)
        .collect()
}

/// The room price for `adults` on `date`: the price for that occupancy, or else the nearest lower one plus
/// the plan's extra-adult amount for each adult above it.
fn room_price(plan: &RatePlan, prices: &[Price], date: Date, adults: i32) -> Option<i64> {
    prices
        .iter()
        .filter(|price| price.date == date && price.occupancy <= adults)
        .max_by_key(|price| price.occupancy)
        .map(|price| price.amount + i64::from(adults - price.occupancy) * plan.extra_adult_amount)
}

/// Prices a stay and lists every reason it cannot be sold. Pure: everything it needs is in `data`.
pub fn quote(request: &QuoteRequest, data: &QuoteData) -> Quote {
    let (plan, room_type) = (&data.plan, &data.room_type);
    let mut violations = Vec::new();
    let mut violation = |kind: ViolationKind, date: Option<Date>, message: String| {
        violations.push(Violation { kind, date, message });
    };
    if !plan.active {
        violation(ViolationKind::Inactive, None, format!("{} is not sold any more", plan.code));
    }
    if plan.residency.is_some_and(|residency| residency != request.residency) {
        let who = if request.residency == Residency::Resident { "non-residents" } else { "residents" };
        violation(ViolationKind::Residency, None, format!("{} is sold to {who} only", plan.code));
    }
    if !plan.room_type_ids.contains(&room_type.id) {
        violation(ViolationKind::RoomTypeNotSold, None, format!("{} does not sell {}", plan.code, room_type.code));
    }
    let (adults, children) = (request.adults, request.children);
    if adults < 1
        || adults > room_type.max_adults
        || children < 0
        || children > room_type.max_children
        || adults + children > room_type.max_occupancy
    {
        violation(
            ViolationKind::Occupancy,
            None,
            format!(
                "{} takes 1 to {} adults and up to {} children, {} guests in all",
                room_type.code, room_type.max_adults, room_type.max_children, room_type.max_occupancy
            ),
        );
    }
    if !plan.allowed_meal_plans.contains(&request.meal_plan) {
        let meal_plan = request.meal_plan.as_str();
        violation(ViolationKind::MealPlanNotAllowed, None, format!("{} is not sold with {meal_plan}", plan.code));
    }

    let stay = nights(request.check_in, request.check_out);
    let length = i32::try_from(stay.len()).unwrap_or(i32::MAX);
    let restriction = |date: Date| data.restrictions.iter().find(|r| r.date == date);
    let mut quoted = Vec::with_capacity(stay.len());
    for &date in &stay {
        let room = room_price(plan, &data.prices, date, adults).unwrap_or_else(|| {
            violation(ViolationKind::NoPrice, Some(date), format!("no price for {adults} adults on {date}"));
            0
        });
        let meal = if request.meal_plan == MealPlan::Ro {
            0
        } else {
            let covers = |s: &&MealSupplement| s.from <= date && s.to.is_none_or(|to| date < to);
            match data.supplements.iter().find(covers) {
                Some(supplement) => {
                    i64::from(adults) * supplement.adult_amount + i64::from(children) * supplement.child_amount
                }
                None => {
                    let meal_plan = request.meal_plan.as_str();
                    violation(
                        ViolationKind::NoMealSupplement,
                        Some(date),
                        format!("no {meal_plan} supplement in {} on {date}", plan.currency),
                    );
                    0
                }
            }
        };
        if let Some(r) = restriction(date) {
            if r.closed {
                violation(ViolationKind::Closed, Some(date), format!("{} is closed on {date}", plan.code));
            }
            if let Some(min) = r.min_stay.filter(|min| length < *min) {
                violation(ViolationKind::MinStay, Some(date), format!("stays over {date} are at least {min} nights"));
            }
            if let Some(max) = r.max_stay.filter(|max| length > *max) {
                violation(ViolationKind::MaxStay, Some(date), format!("stays over {date} are at most {max} nights"));
            }
        }
        quoted.push(QuoteNight { date, room, meal });
    }
    if restriction(request.check_in).is_some_and(|r| r.closed_to_arrival) {
        let date = request.check_in;
        violation(ViolationKind::ClosedToArrival, Some(date), format!("arrivals are closed on {date}"));
    }
    if restriction(request.check_out).is_some_and(|r| r.closed_to_departure) {
        let date = request.check_out;
        violation(ViolationKind::ClosedToDeparture, Some(date), format!("departures are closed on {date}"));
    }

    Quote {
        total: quoted.iter().map(|night| night.room + night.meal).sum(),
        nights: quoted,
        currency: plan.currency.clone(),
        restrictions_ok: violations.is_empty(),
        violations,
    }
}

/// Reads what a quote needs and prices the stay.
pub async fn load_quote(tx: &mut Tx, property: Uuid, request: &QuoteRequest) -> Result<Quote, RatesError> {
    if request.check_out <= request.check_in {
        return Err(RatesError::Invalid("check-out is after check-in".into()));
    }
    let plan = list_rate_plans(tx, property)
        .await?
        .into_iter()
        .find(|plan| plan.id == request.rate_plan_id)
        .ok_or(RatesError::NotFound("rate plan"))?;
    let room_type: QuoteRoomType = sqlx::query_as(
        "select id, code, max_adults, max_children, max_occupancy from room_type where id = $1 and property_id = $2",
    )
    .bind(request.room_type_id)
    .bind(property)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(RatesError::NotFound("room type"))?;
    let prices: Vec<Price> = sqlx::query_as(
        "select room_type_id, date, occupancy, amount from rate_day
         where rate_plan_id = $1 and room_type_id = $2 and date >= $3 and date < $4",
    )
    .bind(plan.id)
    .bind(room_type.id)
    .bind(request.check_in)
    .bind(request.check_out)
    .fetch_all(&mut **tx)
    .await?;
    let restrictions: Vec<Restriction> = sqlx::query_as(
        "select room_type_id, date, closed, min_stay, max_stay, closed_to_arrival, closed_to_departure
         from rate_restriction where rate_plan_id = $1 and room_type_id = $2 and date >= $3 and date <= $4",
    )
    .bind(plan.id)
    .bind(room_type.id)
    .bind(request.check_in)
    .bind(request.check_out)
    .fetch_all(&mut **tx)
    .await?;
    let supplements: Vec<MealSupplement> = sqlx::query_as(
        "select id, property_id, meal_plan, currency::text as currency, adult_amount, child_amount,
                lower(valid) as \"from\", upper(valid) as \"to\", version
         from meal_supplement
         where property_id = $1 and meal_plan = $2 and currency = $3 and valid && daterange($4, $5)",
    )
    .bind(property)
    .bind(request.meal_plan.as_str())
    .bind(&plan.currency)
    .bind(request.check_in)
    .bind(request.check_out)
    .fetch_all(&mut **tx)
    .await?;
    Ok(quote(request, &QuoteData { plan, room_type, prices, restrictions, supplements }))
}

#[cfg(test)]
mod tests {
    use super::{QuoteData, QuoteNight, QuoteRequest, QuoteRoomType, ViolationKind, quote};
    use crate::plans::{MealPlan, PlanKind, RatePlan, Residency, Segment};
    use crate::{MealSupplement, Price, Restriction};
    use time::Date;
    use time::macros::date;
    use uuid::Uuid;

    const DELUXE: Uuid = Uuid::from_u128(1);
    const ARRIVAL: Date = date!(2026 - 10 - 01);

    fn day(offset: i64) -> Date {
        ARRIVAL + time::Duration::days(offset)
    }

    fn plan() -> RatePlan {
        RatePlan {
            id: Uuid::from_u128(2),
            property_id: Uuid::from_u128(3),
            code: "BAR".into(),
            name: "Best available".into(),
            kind: PlanKind::Standard,
            segment: Segment::Ibe,
            residency: None,
            currency: "USD".into(),
            parent_id: None,
            depth: 0,
            derive_mode: None,
            derive_value: None,
            rounding_step: 1,
            extra_adult_amount: 2_500,
            inherit_restrictions: false,
            allowed_meal_plans: vec![MealPlan::Ro, MealPlan::Bb],
            cancellation_policy_id: None,
            room_type_ids: vec![DELUXE],
            active: true,
            version: 1,
        }
    }

    fn price(offset: i64, occupancy: i32, amount: i64) -> Price {
        Price { room_type_id: DELUXE, date: day(offset), occupancy, amount }
    }

    fn restriction(offset: i64) -> Restriction {
        Restriction {
            room_type_id: DELUXE,
            date: day(offset),
            closed: false,
            min_stay: None,
            max_stay: None,
            closed_to_arrival: false,
            closed_to_departure: false,
        }
    }

    fn breakfast() -> MealSupplement {
        MealSupplement {
            id: Uuid::from_u128(4),
            property_id: Uuid::from_u128(3),
            meal_plan: MealPlan::Bb,
            currency: "USD".into(),
            adult_amount: 1_500,
            child_amount: 750,
            from: day(-30),
            to: None,
            version: 1,
        }
    }

    /// Three nights of two adults and a child in a deluxe room, bed and breakfast.
    fn stay(nights: i64) -> QuoteRequest {
        QuoteRequest {
            room_type_id: DELUXE,
            rate_plan_id: Uuid::from_u128(2),
            meal_plan: MealPlan::Bb,
            check_in: ARRIVAL,
            check_out: day(nights),
            adults: 2,
            children: 1,
            residency: Residency::NonResident,
        }
    }

    fn data() -> QuoteData {
        QuoteData {
            plan: plan(),
            room_type: QuoteRoomType {
                id: DELUXE,
                code: "DLX".into(),
                max_adults: 2,
                max_children: 1,
                max_occupancy: 3,
            },
            prices: vec![price(0, 2, 10_000), price(1, 2, 10_000), price(2, 2, 12_000), price(3, 2, 12_000)],
            restrictions: vec![],
            supplements: vec![breakfast()],
        }
    }

    fn kinds(request: &QuoteRequest, data: &QuoteData) -> Vec<(ViolationKind, Option<Date>)> {
        quote(request, data).violations.into_iter().map(|v| (v.kind, v.date)).collect()
    }

    #[test]
    fn nights_add_the_room_and_the_meal_supplement_per_person() {
        let quoted = quote(&stay(3), &data());

        let meal = 2 * 1_500 + 750;
        assert_eq!(
            quoted.nights,
            [
                QuoteNight { date: day(0), room: 10_000, meal },
                QuoteNight { date: day(1), room: 10_000, meal },
                QuoteNight { date: day(2), room: 12_000, meal },
            ]
        );
        assert_eq!(quoted.total, 32_000 + 3 * meal);
        assert_eq!(quoted.currency, "USD");
        assert!(quoted.restrictions_ok, "{:?}", quoted.violations);
    }

    #[test]
    fn room_only_has_no_supplement_and_a_missing_supplement_is_a_violation() {
        let room_only = quote(&QuoteRequest { meal_plan: MealPlan::Ro, ..stay(1) }, &data());
        let without_supplement = QuoteData { supplements: vec![], ..data() };

        assert_eq!(room_only.total, 10_000);
        assert_eq!(kinds(&stay(1), &without_supplement), [(ViolationKind::NoMealSupplement, Some(day(0)))]);
    }

    #[test]
    fn a_missing_occupancy_falls_back_to_the_nearest_lower_one_plus_extra_adults() {
        let single_priced = QuoteData { prices: vec![price(0, 1, 8_000), price(0, 3, 30_000)], ..data() };
        let only_triple = QuoteData { prices: vec![price(0, 3, 30_000)], ..data() };

        let quoted = quote(&stay(1), &single_priced);

        assert_eq!(quoted.nights[0].room, 8_000 + 2_500, "one adult's price plus one extra adult");
        assert_eq!(kinds(&stay(1), &only_triple), [(ViolationKind::NoPrice, Some(day(0)))]);
    }

    #[test]
    fn a_minimum_stay_on_any_night_of_the_stay_applies() {
        let second_night =
            QuoteData { restrictions: vec![Restriction { min_stay: Some(3), ..restriction(1) }], ..data() };
        let after_the_stay =
            QuoteData { restrictions: vec![Restriction { min_stay: Some(3), ..restriction(2) }], ..data() };

        assert_eq!(kinds(&stay(2), &second_night), [(ViolationKind::MinStay, Some(day(1)))]);
        assert!(quote(&stay(3), &second_night).restrictions_ok);
        assert!(quote(&stay(2), &after_the_stay).restrictions_ok, "the departure date is not a night");
    }

    #[test]
    fn a_maximum_stay_and_a_closed_night_apply_to_the_nights_of_the_stay() {
        let restrictions =
            vec![Restriction { max_stay: Some(2), ..restriction(0) }, Restriction { closed: true, ..restriction(2) }];
        let data = QuoteData { restrictions, ..data() };

        assert_eq!(
            kinds(&stay(3), &data),
            [(ViolationKind::MaxStay, Some(day(0))), (ViolationKind::Closed, Some(day(2)))]
        );
        assert!(quote(&stay(2), &data).restrictions_ok);
    }

    #[test]
    fn closed_to_arrival_counts_on_the_arrival_date_only() {
        let on_arrival =
            QuoteData { restrictions: vec![Restriction { closed_to_arrival: true, ..restriction(0) }], ..data() };
        let mid_stay =
            QuoteData { restrictions: vec![Restriction { closed_to_arrival: true, ..restriction(1) }], ..data() };

        assert_eq!(kinds(&stay(2), &on_arrival), [(ViolationKind::ClosedToArrival, Some(day(0)))]);
        assert!(quote(&stay(2), &mid_stay).restrictions_ok);
    }

    #[test]
    fn closed_to_departure_counts_on_the_departure_date_only() {
        let on_departure =
            QuoteData { restrictions: vec![Restriction { closed_to_departure: true, ..restriction(2) }], ..data() };
        let mid_stay =
            QuoteData { restrictions: vec![Restriction { closed_to_departure: true, ..restriction(1) }], ..data() };

        assert_eq!(kinds(&stay(2), &on_departure), [(ViolationKind::ClosedToDeparture, Some(day(2)))]);
        assert!(quote(&stay(2), &mid_stay).restrictions_ok);
    }

    #[test]
    fn residency_occupancy_meal_plan_room_type_and_activity_are_checked() {
        let residents_only = QuoteData { plan: RatePlan { residency: Some(Residency::Resident), ..plan() }, ..data() };
        let retired = QuoteData { plan: RatePlan { active: false, room_type_ids: vec![], ..plan() }, ..data() };

        assert_eq!(kinds(&stay(1), &residents_only), [(ViolationKind::Residency, None)]);
        assert_eq!(quote(&stay(1), &residents_only).violations[0].message, "BAR is sold to residents only");
        assert!(quote(&QuoteRequest { residency: Residency::Resident, ..stay(1) }, &residents_only).restrictions_ok);
        assert_eq!(kinds(&QuoteRequest { adults: 3, children: 0, ..stay(1) }, &data())[0].0, ViolationKind::Occupancy);
        assert_eq!(kinds(&QuoteRequest { children: 2, ..stay(1) }, &data())[0].0, ViolationKind::Occupancy);
        assert_eq!(
            kinds(&QuoteRequest { meal_plan: MealPlan::Hb, ..stay(1) }, &data())[0].0,
            ViolationKind::MealPlanNotAllowed
        );
        assert_eq!(
            kinds(&stay(1), &retired),
            [(ViolationKind::Inactive, None), (ViolationKind::RoomTypeNotSold, None)]
        );
    }
}
```

- [ ] **Step 4: Run the checks**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test -p rates
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
```

Expected: rates unit tests 10 passed (8 quote rules), `quote` 1 passed.

- [ ] **Step 5: Commit**

```sh
git add -A
git commit -m "feat(rates): price quotes as a pure function of loaded rows, with nights, meal supplements and every restriction"
```

### Task 10: REST commands for rates

`POST /rate-plans` and `PATCH /rate-plans/{plan}`, `PUT …/prices`, `POST …/bulk-change` (idempotent, 200
`{"changed": n}`), `PUT …/restrictions`, `POST`/`PATCH` for `/meal-supplements` and `/cancellation-policies`,
all behind `RatesManage`. Tests cover the flow (including that a retried bulk change replays instead of
compounding), the rules as problems, permissions by role, another tenant's ids through both properties, the
`rates:` event keys of a price change, and that the OpenAPI document lists the enum values the API sends.

**Files:**
- Modify: `crates/core-api/Cargo.toml`
- Modify: `crates/core-api/src/openapi.rs`
- Modify: `crates/core-api/src/routes/mod.rs`
- Create: `crates/core-api/src/routes/rates.rs`
- Modify: `crates/core-api/src/routes/rooms.rs`
- Test: `crates/core-api/tests/openapi.rs`
- Test: `crates/core-api/tests/rates.rs` (new)

**Interfaces:**
- Consumes: the `rates` functions of Tasks 5–8, `validate_changes`, `routes::rooms::present` (now `pub(crate)`).
- Produces: routes and DTOs `CreateRatePlanRequest`, `UpdateRatePlanRequest`, `PriceRequest`, `SetPricesRequest`, `PriceChangeRequest`, `BulkChangeRequest`, `BulkChangeResponse`, `RestrictionsRequest`, `CreateMealSupplementRequest`, `UpdateMealSupplementRequest`, `CreateCancellationPolicyRequest`, `UpdateCancellationPolicyRequest`; operation ids `create_rate_plan`, `update_rate_plan`, `set_prices`, `bulk_change_prices`, `set_restrictions`, `create_meal_supplement`, `update_meal_supplement`, `create_cancellation_policy`, `update_cancellation_policy`, which the SPA's generated types use.

- [ ] **Step 1: Write the failing tests**

Modify `crates/core-api/tests/openapi.rs`:

```diff
diff --git a/crates/core-api/tests/openapi.rs b/crates/core-api/tests/openapi.rs
index 98c6c16..84c5c41 100644
--- a/crates/core-api/tests/openapi.rs
+++ b/crates/core-api/tests/openapi.rs
@@ -19,6 +19,15 @@ fn the_openapi_document_lists_every_rest_route() {
             "/api/v1/properties/{property}/block-reasons",
             "/api/v1/properties/{property}/block-reasons/{reason}",
             "/api/v1/properties/{property}/blocks/{block}",
+            "/api/v1/properties/{property}/cancellation-policies",
+            "/api/v1/properties/{property}/cancellation-policies/{policy}",
+            "/api/v1/properties/{property}/meal-supplements",
+            "/api/v1/properties/{property}/meal-supplements/{supplement}",
+            "/api/v1/properties/{property}/rate-plans",
+            "/api/v1/properties/{property}/rate-plans/{plan}",
+            "/api/v1/properties/{property}/rate-plans/{plan}/bulk-change",
+            "/api/v1/properties/{property}/rate-plans/{plan}/prices",
+            "/api/v1/properties/{property}/rate-plans/{plan}/restrictions",
             "/api/v1/properties/{property}/room-types",
             "/api/v1/properties/{property}/room-types/order",
             "/api/v1/properties/{property}/room-types/{room_type}",
@@ -76,16 +85,34 @@ fn versioned_responses_declare_their_etag() {
         [
             "create_block",
             "create_block_reason",
+            "create_cancellation_policy",
+            "create_meal_supplement",
             "create_property",
+            "create_rate_plan",
             "create_room",
             "create_room_type",
             "create_section",
             "rename_section",
             "shorten_block",
             "update_block_reason",
+            "update_cancellation_policy",
+            "update_meal_supplement",
             "update_property",
+            "update_rate_plan",
             "update_room",
             "update_room_type",
         ]
     );
 }
+
+/// Enums are documented with the values the API sends and accepts, so generated types match the JSON.
+#[test]
+fn rate_enums_are_documented_with_their_json_values() {
+    let doc = serde_json::to_value(ApiDoc::openapi()).unwrap();
+    let values = |name: &str| doc["components"]["schemas"][name]["enum"].clone();
+
+    assert_eq!(values("PlanKind"), serde_json::json!(["standard", "derived", "custom"]));
+    assert_eq!(values("Segment"), serde_json::json!(["FIT_F", "FIT_L", "OTA", "TA", "IBE"]));
+    assert_eq!(values("MealPlan"), serde_json::json!(["RO", "BB", "HB", "FB"]));
+    assert_eq!(values("Residency"), serde_json::json!(["resident", "non_resident"]));
+}
```

Create `crates/core-api/tests/rates.rs`:

```rust
mod common;

use axum::http::{Method, StatusCode, header};
use common::{TestApp, TestResponse, uuid};
use core_api::events::{LiveEvent, spawn_listener};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::time::Duration as StdDuration;
use time::{Date, Duration, format_description::well_known::Iso8601};
use uuid::Uuid;

async fn post(app: &TestApp, cookie: &str, path: &str, body: Value) -> TestResponse {
    let key = Uuid::now_v7().to_string();
    post_with_key(app, cookie, path, &key, body).await
}

async fn post_with_key(app: &TestApp, cookie: &str, path: &str, key: &str, body: Value) -> TestResponse {
    app.send_with(Method::POST, path, Some(cookie), Some(body), &[("x-goodfolk-csrf", "1"), ("idempotency-key", key)])
        .await
}

async fn patch(app: &TestApp, cookie: &str, path: &str, version: i64, body: Value) -> TestResponse {
    let if_match = format!("\"{version}\"");
    app.send_with(Method::PATCH, path, Some(cookie), Some(body), &[("x-goodfolk-csrf", "1"), ("if-match", &if_match)])
        .await
}

async fn put(app: &TestApp, cookie: &str, path: &str, body: Value) -> TestResponse {
    app.send(Method::PUT, path, Some(cookie), Some(body)).await
}

/// A property with a deluxe room type (up to 2 adults and a child), set up by its owner.
struct Hotel {
    owner: String,
    superuser: PgPool,
    id: Uuid,
    path: String,
    business_date: Date,
    deluxe: Uuid,
}

impl Hotel {
    async fn new(app: &TestApp, opts: PgConnectOptions) -> Self {
        let superuser = PgPool::connect_with(opts).await.unwrap();
        let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
        let property = json!({"code": "GAL", "name": "Galle", "timezone": "Asia/Colombo", "base_currency": "LKR"});
        let property = post(app, &owner, "/api/v1/properties", property).await.body;
        let id = uuid(&property["id"]);
        let path = format!("/api/v1/properties/{id}");
        let business_date = Date::parse(property["business_date"].as_str().unwrap(), &Iso8601::DEFAULT).unwrap();
        let deluxe = json!({"code": "DLX", "name": "Deluxe", "base_occupancy": 2, "max_adults": 2,
                            "max_children": 1, "max_occupancy": 3});
        let deluxe = uuid(&post(app, &owner, &format!("{path}/room-types"), deluxe).await.body["id"]);
        Self { owner, superuser, id, path, business_date, deluxe }
    }

    fn day(&self, offset: i64) -> String {
        (self.business_date + Duration::days(offset)).to_string()
    }

    fn bar(&self) -> Value {
        json!({"code": "BAR", "name": "Best available", "kind": "standard", "segment": "IBE", "currency": "USD",
               "rounding_step": 100, "allowed_meal_plans": ["RO", "BB"], "room_type_ids": [self.deluxe]})
    }

    fn ota(&self, parent: &Value) -> Value {
        json!({"code": "OTA", "name": "Online agents", "kind": "derived", "segment": "OTA", "currency": "USD",
               "parent_id": parent["id"], "derive_mode": "percent", "derive_value": 1500, "rounding_step": 100,
               "room_type_ids": [self.deluxe]})
    }

    /// `(date, amount)` of a plan's prices for two adults, by date.
    async fn prices(&self, plan: &Value) -> Vec<(Date, i64)> {
        sqlx::query_as("select date, amount from rate_day where rate_plan_id = $1 and occupancy = 2 order by date")
            .bind(uuid(&plan["id"]))
            .fetch_all(&self.superuser)
            .await
            .unwrap()
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn plans_prices_and_bulk_changes_over_rest(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let plans = format!("{}/rate-plans", hotel.path);

    let bar = post(&app, &hotel.owner, &plans, hotel.bar()).await;
    let ota = post(&app, &hotel.owner, &plans, hotel.ota(&bar.body)).await;
    let prices: Vec<Value> = (0..14)
        .map(|day| json!({"room_type_id": hotel.deluxe, "date": hotel.day(day), "occupancy": 2, "amount": 10_000}))
        .collect();
    let bar_path = format!("{plans}/{}", bar.body["id"].as_str().unwrap());
    let priced = put(&app, &hotel.owner, &format!("{bar_path}/prices"), json!({"prices": prices})).await;
    let weekends = json!({"from": hotel.day(0), "to": hotel.day(14), "weekdays": [6, 7],
                          "change": {"mode": "percent", "value": 1000}});
    let bulk_path = format!("{bar_path}/bulk-change");
    let bulk = post_with_key(&app, &hotel.owner, &bulk_path, "bulk-change-0001", weekends.clone()).await;
    let replay = post_with_key(&app, &hotel.owner, &bulk_path, "bulk-change-0001", weekends).await;
    let renamed = patch(&app, &hotel.owner, &bar_path, 1, json!({"name": "Rack", "extra_adult_amount": 2500})).await;

    assert_eq!(bar.status, StatusCode::CREATED, "{:?}", bar.body);
    assert_eq!(bar.headers[header::ETAG], "\"1\"");
    assert_eq!(bar.body["room_type_ids"], json!([hotel.deluxe]));
    assert_eq!(ota.status, StatusCode::CREATED, "{:?}", ota.body);
    assert_eq!((ota.body["depth"].clone(), ota.body["derive_value"].clone()), (json!(1), json!(1500)));
    assert_eq!(priced.status, StatusCode::NO_CONTENT, "{:?}", priced.body);
    assert_eq!(bulk.status, StatusCode::OK, "{:?}", bulk.body);
    assert_eq!(bulk.body["changed"], 4, "two weekends of two days");
    assert_eq!(replay.body, bulk.body, "a retry replays the answer and changes nothing again");
    let weekend = |date: &Date| matches!(date.weekday(), time::Weekday::Saturday | time::Weekday::Sunday);
    for (date, amount) in hotel.prices(&bar.body).await {
        assert_eq!(amount, if weekend(&date) { 11_000 } else { 10_000 }, "BAR on {date}");
    }
    for (date, amount) in hotel.prices(&ota.body).await {
        assert_eq!(amount, if weekend(&date) { 12_700 } else { 11_500 }, "OTA on {date}");
    }
    assert_eq!(renamed.status, StatusCode::OK, "{:?}", renamed.body);
    assert_eq!(
        (renamed.headers[header::ETAG].to_str().unwrap(), renamed.body["name"].as_str()),
        ("\"2\"", Some("Rack"))
    );
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn restrictions_meal_supplements_and_cancellation_policies_over_rest(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let bar = post(&app, &hotel.owner, &format!("{}/rate-plans", hotel.path), hotel.bar()).await.body;
    let bar_path = format!("{}/rate-plans/{}", hotel.path, bar["id"].as_str().unwrap());

    let restricted = put(
        &app,
        &hotel.owner,
        &format!("{bar_path}/restrictions"),
        json!({"from": hotel.day(0), "to": hotel.day(3), "room_type_ids": [hotel.deluxe], "min_stay": 2,
               "closed_to_arrival": true}),
    )
    .await;
    let cleared = put(
        &app,
        &hotel.owner,
        &format!("{bar_path}/restrictions"),
        json!({"from": hotel.day(0), "to": hotel.day(1), "min_stay": null}),
    )
    .await;
    let supplements = format!("{}/meal-supplements", hotel.path);
    let breakfast = json!({"meal_plan": "BB", "currency": "USD", "adult_amount": 1500, "child_amount": 750,
                           "from": hotel.day(0)});
    let created = post(&app, &hotel.owner, &supplements, breakfast.clone()).await;
    let overlapping = post(&app, &hotel.owner, &supplements, breakfast).await;
    let supplement_path = format!("{supplements}/{}", created.body["id"].as_str().unwrap());
    let raised =
        patch(&app, &hotel.owner, &supplement_path, 1, json!({"adult_amount": 1800, "to": hotel.day(365)})).await;
    let policy = json!({"name": "Flexible", "rules": [{"days_before_arrival": 2, "penalty": {"kind": "nights", "value": 1}}],
                        "no_show": {"kind": "percent", "value": 10000}});
    let policy = post(&app, &hotel.owner, &format!("{}/cancellation-policies", hotel.path), policy).await;
    let policy_path = format!("{}/cancellation-policies/{}", hotel.path, policy.body["id"].as_str().unwrap());
    let renamed = patch(&app, &hotel.owner, &policy_path, 1, json!({"name": "Flexible 48h"})).await;
    let with_policy =
        patch(&app, &hotel.owner, &bar_path, 1, json!({"cancellation_policy_id": policy.body["id"]})).await;

    assert_eq!(restricted.status, StatusCode::NO_CONTENT, "{:?}", restricted.body);
    assert_eq!(cleared.status, StatusCode::NO_CONTENT, "{:?}", cleared.body);
    let rows: Vec<(Date, Option<i32>, bool)> =
        sqlx::query_as("select date, min_stay, closed_to_arrival from rate_restriction order by date")
            .fetch_all(&hotel.superuser)
            .await
            .unwrap();
    let min_stays: Vec<Option<i32>> = rows.iter().map(|row| row.1).collect();
    assert_eq!(min_stays, [None, Some(2), Some(2)]);
    assert!(rows.iter().all(|row| row.2), "closed to arrival stays when only the minimum stay is cleared");
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    assert_eq!(created.body["to"], Value::Null);
    assert_eq!(overlapping.status, StatusCode::CONFLICT, "{:?}", overlapping.body);
    assert_eq!(raised.status, StatusCode::OK, "{:?}", raised.body);
    assert_eq!((raised.body["adult_amount"].clone(), raised.body["to"].clone()), (json!(1800), json!(hotel.day(365))));
    assert_eq!(policy.status, StatusCode::CREATED, "{:?}", policy.body);
    assert_eq!(renamed.body["name"], "Flexible 48h");
    assert_eq!(with_policy.body["cancellation_policy_id"], policy.body["id"]);
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn rate_rules_are_problems(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let plans = format!("{}/rate-plans", hotel.path);
    let bar = post(&app, &hotel.owner, &plans, hotel.bar()).await.body;
    let ota = post(&app, &hotel.owner, &plans, hotel.ota(&bar)).await.body;
    let ota_path = format!("{plans}/{}", ota["id"].as_str().unwrap());
    let price =
        json!({"prices": [{"room_type_id": hotel.deluxe, "date": hotel.day(0), "occupancy": 2, "amount": 100}]});

    let mut in_rupees = hotel.ota(&bar);
    in_rupees["code"] = json!("OTA-LKR");
    in_rupees["currency"] = json!("LKR");
    let responses = [
        (post(&app, &hotel.owner, &plans, in_rupees).await, StatusCode::UNPROCESSABLE_ENTITY),
        (post(&app, &hotel.owner, &plans, hotel.bar()).await, StatusCode::CONFLICT),
        (put(&app, &hotel.owner, &format!("{ota_path}/prices"), price.clone()).await, StatusCode::UNPROCESSABLE_ENTITY),
        (patch(&app, &hotel.owner, &ota_path, 7, json!({"name": "X"})).await, StatusCode::PRECONDITION_FAILED),
        (patch(&app, &hotel.owner, &ota_path, 1, json!({})).await, StatusCode::UNPROCESSABLE_ENTITY),
        (
            app.send(Method::POST, &format!("{ota_path}/bulk-change"), Some(&hotel.owner), Some(json!({}))).await,
            StatusCode::BAD_REQUEST,
        ),
        (put(&app, &hotel.owner, &format!("{plans}/{}/prices", Uuid::now_v7()), price).await, StatusCode::NOT_FOUND),
        (
            put(&app, &hotel.owner, &format!("{ota_path}/prices"), json!({"prices": []})).await,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
    ];

    for (index, (response, expected)) in responses.into_iter().enumerate() {
        assert_eq!(response.status, expected, "case {index}: {:?}", response.body);
        assert_eq!(response.headers[header::CONTENT_TYPE], "application/problem+json");
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn only_owners_and_managers_manage_rates(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let manager = app.staff(&hotel.superuser, &hotel.owner, "manager@example.com", "manager").await;
    let front_desk = app.staff(&hotel.superuser, &hotel.owner, "desk@example.com", "front_desk").await;
    let accountant = app.staff(&hotel.superuser, &hotel.owner, "accounts@example.com", "accountant").await;
    let plans = format!("{}/rate-plans", hotel.path);

    let by_manager = post(&app, &manager, &plans, hotel.bar()).await;
    let bar_path = format!("{plans}/{}", by_manager.body["id"].as_str().unwrap());
    let price =
        json!({"prices": [{"room_type_id": hotel.deluxe, "date": hotel.day(0), "occupancy": 2, "amount": 100}]});
    let priced_by_manager = put(&app, &manager, &format!("{bar_path}/prices"), price.clone()).await;
    let bulk = json!({"from": hotel.day(0), "to": hotel.day(7), "change": {"mode": "amount", "value": 100}});
    let restriction = json!({"from": hotel.day(0), "to": hotel.day(1), "closed": true});
    let supplement =
        json!({"meal_plan": "BB", "currency": "USD", "adult_amount": 1, "child_amount": 1, "from": hotel.day(0)});
    let policy = json!({"name": "Strict", "rules": [], "no_show": {"kind": "nights", "value": 1}});

    assert_eq!(by_manager.status, StatusCode::CREATED, "{:?}", by_manager.body);
    assert_eq!(priced_by_manager.status, StatusCode::NO_CONTENT, "{:?}", priced_by_manager.body);
    for staff in [&front_desk, &accountant] {
        let refused = [
            post(&app, staff, &plans, hotel.bar()).await,
            put(&app, staff, &format!("{bar_path}/prices"), price.clone()).await,
            post(&app, staff, &format!("{bar_path}/bulk-change"), bulk.clone()).await,
            put(&app, staff, &format!("{bar_path}/restrictions"), restriction.clone()).await,
            post(&app, staff, &format!("{}/meal-supplements", hotel.path), supplement.clone()).await,
            post(&app, staff, &format!("{}/cancellation-policies", hotel.path), policy.clone()).await,
        ];
        for response in refused {
            assert_eq!(response.status, StatusCode::FORBIDDEN, "{:?}", response.body);
        }
    }
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn another_tenants_rates_cannot_be_changed(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let bar = post(&app, &hotel.owner, &format!("{}/rate-plans", hotel.path), hotel.bar()).await.body;
    let breakfast = json!({"meal_plan": "BB", "currency": "USD", "adult_amount": 1500, "child_amount": 750,
                           "from": hotel.day(0)});
    let supplement = post(&app, &hotel.owner, &format!("{}/meal-supplements", hotel.path), breakfast).await.body;
    let policy = json!({"name": "Flexible", "rules": [], "no_show": {"kind": "nights", "value": 1}});
    let policy = post(&app, &hotel.owner, &format!("{}/cancellation-policies", hotel.path), policy).await.body;
    let intruder = app.signup_owner("intruder@example.com", "Other Hotels").await;
    let own = json!({"code": "KAN", "name": "Kandy", "timezone": "Asia/Colombo", "base_currency": "LKR"});
    let own = format!(
        "/api/v1/properties/{}",
        post(&app, &intruder, "/api/v1/properties", own).await.body["id"].as_str().unwrap()
    );
    let (plan, supplement, policy) =
        (bar["id"].as_str().unwrap(), supplement["id"].as_str().unwrap(), policy["id"].as_str().unwrap());
    let price = json!({"prices": [{"room_type_id": hotel.deluxe, "date": hotel.day(0), "occupancy": 2, "amount": 1}]});
    let bulk = json!({"from": hotel.day(0), "to": hotel.day(7), "change": {"mode": "set", "value": 1}});

    // Through the other tenant's property, and through the intruder's own property with the other tenant's ids.
    for property in [hotel.path.as_str(), own.as_str()] {
        let responses = [
            patch(&app, &intruder, &format!("{property}/rate-plans/{plan}"), 1, json!({"name": "X"})).await,
            put(&app, &intruder, &format!("{property}/rate-plans/{plan}/prices"), price.clone()).await,
            post(&app, &intruder, &format!("{property}/rate-plans/{plan}/bulk-change"), bulk.clone()).await,
            put(
                &app,
                &intruder,
                &format!("{property}/rate-plans/{plan}/restrictions"),
                json!({"from": hotel.day(0), "to": hotel.day(1), "closed": true}),
            )
            .await,
            patch(&app, &intruder, &format!("{property}/meal-supplements/{supplement}"), 1, json!({"adult_amount": 1}))
                .await,
            patch(&app, &intruder, &format!("{property}/cancellation-policies/{policy}"), 1, json!({"name": "X"}))
                .await,
        ];
        for response in responses {
            assert_eq!(response.status, StatusCode::NOT_FOUND, "{property}: {:?}", response.body);
        }
    }
    let untouched: (i32, i64, i64, i32, i32) = sqlx::query_as(
        "select (select version from rate_plan), (select count(*) from rate_day), (select count(*) from rate_restriction),
                (select version from meal_supplement), (select version from cancellation_policy)",
    )
    .fetch_one(&hotel.superuser)
    .await
    .unwrap();
    assert_eq!(untouched, (1, 0, 0, 1, 1), "nothing of the other tenant changed");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_price_change_tells_screens_to_refetch_each_plans_months(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    spawn_listener(PgPool::connect_with(opts.clone()).await.unwrap(), app.state.events.clone()).await.unwrap();
    let hotel = Hotel::new(&app, opts).await;
    let plans = format!("{}/rate-plans", hotel.path);
    let bar = post(&app, &hotel.owner, &plans, hotel.bar()).await.body;
    let ota = post(&app, &hotel.owner, &plans, hotel.ota(&bar)).await.body;
    let mut events = app.state.events.subscribe();

    let price =
        json!({"prices": [{"room_type_id": hotel.deluxe, "date": hotel.day(0), "occupancy": 2, "amount": 100}]});
    put(&app, &hotel.owner, &format!("{plans}/{}/prices", bar["id"].as_str().unwrap()), price).await;

    let LiveEvent::Invalidate(event) =
        tokio::time::timeout(StdDuration::from_secs(5), events.recv()).await.unwrap().unwrap()
    else {
        panic!("expected an invalidation")
    };
    let month = &hotel.day(0)[..7];
    assert_eq!(event.property_id, Some(hotel.id));
    assert_eq!(
        event.keys,
        [
            format!("rates:{}:{}:{month}", hotel.id, bar["id"].as_str().unwrap()),
            format!("rates:{}:{}:{month}", hotel.id, ota["id"].as_str().unwrap()),
        ]
    );
}
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `DATABASE_URL=$TEST_DATABASE_URL cargo test -p core-api --no-fail-fast --test rates --test openapi`

Expected: the 6 `rates` tests FAIL (the routes answer 404), and `the_openapi_document_lists_every_rest_route`, `versioned_responses_declare_their_etag` and `rate_enums_are_documented_with_their_json_values` FAIL.

- [ ] **Step 3: Implement**

Modify `crates/core-api/Cargo.toml`:

```diff
diff --git a/crates/core-api/Cargo.toml b/crates/core-api/Cargo.toml
index 36b64e9..dd06239 100644
--- a/crates/core-api/Cargo.toml
+++ b/crates/core-api/Cargo.toml
@@ -18,6 +18,7 @@ garde.workspace = true
 identity.workspace = true
 mimalloc.workspace = true
 property.workspace = true
+rates.workspace = true
 rooms.workspace = true
 serde.workspace = true
 serde_json.workspace = true
```

Modify `crates/core-api/src/openapi.rs`:

```diff
diff --git a/crates/core-api/src/openapi.rs b/crates/core-api/src/openapi.rs
index c12a04b..623b3dd 100644
--- a/crates/core-api/src/openapi.rs
+++ b/crates/core-api/src/openapi.rs
@@ -1,8 +1,10 @@
 use crate::routes::{
-    BedRequest, CreateBlockReasonRequest, CreateBlockRequest, CreatePropertyRequest, CreateRoomRangeRequest,
-    CreateRoomRequest, CreateRoomTypeRequest, LoginRequest, ReorderRequest, SectionRequest, ShortenBlockRequest,
-    SignupRequest, SwitchTenantRequest, UpdateBlockReasonRequest, UpdatePropertyRequest, UpdateRoomRequest,
-    UpdateRoomTypeRequest,
+    BedRequest, BulkChangeRequest, BulkChangeResponse, CreateBlockReasonRequest, CreateBlockRequest,
+    CreateCancellationPolicyRequest, CreateMealSupplementRequest, CreatePropertyRequest, CreateRatePlanRequest,
+    CreateRoomRangeRequest, CreateRoomRequest, CreateRoomTypeRequest, LoginRequest, PriceChangeRequest, PriceRequest,
+    ReorderRequest, RestrictionsRequest, SectionRequest, SetPricesRequest, ShortenBlockRequest, SignupRequest,
+    SwitchTenantRequest, UpdateBlockReasonRequest, UpdateCancellationPolicyRequest, UpdateMealSupplementRequest,
+    UpdatePropertyRequest, UpdateRatePlanRequest, UpdateRoomRequest, UpdateRoomTypeRequest,
 };
 use utoipa::OpenApi;
 
@@ -30,6 +32,15 @@ use utoipa::OpenApi;
         crate::routes::blocks::update_reason,
         crate::routes::blocks::create,
         crate::routes::blocks::shorten,
+        crate::routes::rates::create_plan,
+        crate::routes::rates::update_plan,
+        crate::routes::rates::set_prices,
+        crate::routes::rates::bulk_change,
+        crate::routes::rates::set_restrictions,
+        crate::routes::rates::create_supplement,
+        crate::routes::rates::update_supplement,
+        crate::routes::rates::create_policy,
+        crate::routes::rates::update_policy,
     ),
     components(schemas(
         SignupRequest,
@@ -49,6 +60,18 @@ use utoipa::OpenApi;
         UpdateBlockReasonRequest,
         CreateBlockRequest,
         ShortenBlockRequest,
+        CreateRatePlanRequest,
+        UpdateRatePlanRequest,
+        PriceRequest,
+        SetPricesRequest,
+        PriceChangeRequest,
+        BulkChangeRequest,
+        BulkChangeResponse,
+        RestrictionsRequest,
+        CreateMealSupplementRequest,
+        UpdateMealSupplementRequest,
+        CreateCancellationPolicyRequest,
+        UpdateCancellationPolicyRequest,
         identity::Profile,
         identity::TenantSummary,
         identity::Grant,
@@ -61,6 +84,18 @@ use utoipa::OpenApi;
         rooms::BlockKind,
         rooms::BlockReason,
         rooms::Block,
+        rates::PlanKind,
+        rates::Segment,
+        rates::Residency,
+        rates::ChangeMode,
+        rates::MealPlan,
+        rates::PriceChangeMode,
+        rates::RatePlan,
+        rates::MealSupplement,
+        rates::PenaltyKind,
+        rates::Penalty,
+        rates::CancellationRule,
+        rates::CancellationPolicy,
     ))
 )]
 pub struct ApiDoc;
```

Modify `crates/core-api/src/routes/mod.rs`:

```diff
diff --git a/crates/core-api/src/routes/mod.rs b/crates/core-api/src/routes/mod.rs
index ee90994..8d89158 100644
--- a/crates/core-api/src/routes/mod.rs
+++ b/crates/core-api/src/routes/mod.rs
@@ -2,6 +2,7 @@ pub(crate) mod auth;
 pub(crate) mod blocks;
 mod health;
 pub(crate) mod properties;
+pub(crate) mod rates;
 pub(crate) mod room_types;
 pub(crate) mod rooms;
 
@@ -21,6 +22,11 @@ use tower_http::trace::TraceLayer;
 pub use auth::{LoginRequest, SignupRequest, SwitchTenantRequest};
 pub use blocks::{CreateBlockReasonRequest, CreateBlockRequest, ShortenBlockRequest, UpdateBlockReasonRequest};
 pub use properties::{CreatePropertyRequest, UpdatePropertyRequest};
+pub use rates::{
+    BulkChangeRequest, BulkChangeResponse, CreateCancellationPolicyRequest, CreateMealSupplementRequest,
+    CreateRatePlanRequest, PriceChangeRequest, PriceRequest, RestrictionsRequest, SetPricesRequest,
+    UpdateCancellationPolicyRequest, UpdateMealSupplementRequest, UpdateRatePlanRequest,
+};
 pub use room_types::{BedRequest, CreateRoomTypeRequest, UpdateRoomTypeRequest};
 pub use rooms::{CreateRoomRangeRequest, CreateRoomRequest, ReorderRequest, SectionRequest, UpdateRoomRequest};
 
@@ -40,6 +46,10 @@ pub fn router(state: AppState) -> Router {
         .route(&format!("{PROPERTY}/sections"), post(rooms::create_section))
         .route(&format!("{PROPERTY}/block-reasons"), post(blocks::create_reason))
         .route(&format!("{PROPERTY}/rooms/{{room}}/blocks"), post(blocks::create))
+        .route(&format!("{PROPERTY}/rate-plans"), post(rates::create_plan))
+        .route(&format!("{PROPERTY}/rate-plans/{{plan}}/bulk-change"), post(rates::bulk_change))
+        .route(&format!("{PROPERTY}/meal-supplements"), post(rates::create_supplement))
+        .route(&format!("{PROPERTY}/cancellation-policies"), post(rates::create_policy))
         .route_layer(from_fn_with_state(state.clone(), idempotency::idempotent));
 
     let requests = Router::new()
@@ -56,6 +66,11 @@ pub fn router(state: AppState) -> Router {
         .route(&format!("{PROPERTY}/sections/{{section}}"), patch(rooms::rename_section))
         .route(&format!("{PROPERTY}/block-reasons/{{reason}}"), patch(blocks::update_reason))
         .route(&format!("{PROPERTY}/blocks/{{block}}"), patch(blocks::shorten))
+        .route(&format!("{PROPERTY}/rate-plans/{{plan}}"), patch(rates::update_plan))
+        .route(&format!("{PROPERTY}/rate-plans/{{plan}}/prices"), put(rates::set_prices))
+        .route(&format!("{PROPERTY}/rate-plans/{{plan}}/restrictions"), put(rates::set_restrictions))
+        .route(&format!("{PROPERTY}/meal-supplements/{{supplement}}"), patch(rates::update_supplement))
+        .route(&format!("{PROPERTY}/cancellation-policies/{{policy}}"), patch(rates::update_policy))
         .route("/graphql", post(graphql::handler))
         .merge(commands)
         .layer(from_fn(|request, next| deadline(REQUEST_TIMEOUT, request, next)));
```

Create `crates/core-api/src/routes/rates.rs`:

```rust
use crate::auth::TenantContext;
use crate::concurrency::{IfMatch, Versioned};
use crate::error::{ApiError, Changes, validate, validate_changes};
use crate::extract::{ApiJson, ApiPath};
use crate::routes::rooms::present;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use db::Scope;
use garde::Validate;
use identity::Permission;
use rates::{
    BulkChange, CancellationPolicy, CancellationPolicyChanges, CancellationRule, ChangeMode, MAX_AMOUNT, MealPlan,
    MealSupplement, MealSupplementChanges, NewCancellationPolicy, NewMealSupplement, NewRatePlan, Penalty, PlanKind,
    Price, PriceChange, PriceChangeMode, RatePlan, RatePlanChanges, RatesError, Residency, RestrictionChange, Segment,
};
use serde::{Deserialize, Serialize};
use time::{Date, Weekday};
use utoipa::ToSchema;
use uuid::Uuid;

/// Maps the rates module's errors to problem details.
fn rates_error(err: RatesError) -> ApiError {
    match err {
        RatesError::NotFound(_) => ApiError::not_found(err.to_string()),
        RatesError::VersionMismatch(_) => ApiError::precondition_failed(err.to_string()),
        RatesError::Conflict(message) => ApiError::conflict(message),
        RatesError::Invalid(message) => ApiError::unprocessable(message),
        RatesError::Database(db_err) => db_err.into(),
    }
}

fn one() -> i64 {
    1
}

fn room_only() -> Vec<MealPlan> {
    vec![MealPlan::Ro]
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreateRatePlanRequest {
    /// Capital letters, digits, `_` and `-`, unique within the property. Cannot be changed later.
    #[garde(pattern(r"^[A-Z0-9_-]{1,20}$"))]
    pub code: String,
    #[garde(length(chars, min = 1, max = 100))]
    pub name: String,
    #[garde(skip)]
    pub kind: PlanKind,
    #[garde(skip)]
    pub segment: Segment,
    /// Left out on a `FIT_F` or `FIT_L` plan, the segment's residency; otherwise any guest.
    #[garde(skip)]
    pub residency: Option<Residency>,
    /// ISO 4217. A derived plan uses its parent's.
    #[garde(pattern(r"^[A-Z]{3}$"))]
    pub currency: String,
    /// Derived plans only.
    #[garde(skip)]
    pub parent_id: Option<Uuid>,
    /// Derived plans only.
    #[garde(skip)]
    pub derive_mode: Option<ChangeMode>,
    /// Derived plans only: basis points (`percent`, -10000 to 100000) or minor units (`amount`).
    #[garde(skip)]
    pub derive_value: Option<i64>,
    /// Minor units; derived and bulk-changed prices are rounded half-up to a multiple of it. Default 1.
    #[serde(default = "one")]
    #[garde(range(min = 1, max = 100_000_000))]
    pub rounding_step: i64,
    /// Minor units added per adult above the highest occupancy priced below a stay's. Default 0.
    #[serde(default)]
    #[garde(range(min = 0, max = MAX_AMOUNT))]
    pub extra_adult_amount: i64,
    /// Derived plans only: copy the parent's restrictions.
    #[serde(default)]
    #[garde(skip)]
    pub inherit_restrictions: bool,
    /// Default `["RO"]`.
    #[serde(default = "room_only")]
    #[garde(length(min = 1, max = 4))]
    pub allowed_meal_plans: Vec<MealPlan>,
    #[garde(skip)]
    pub cancellation_policy_id: Option<Uuid>,
    /// A derived plan sells a subset of its parent's room types.
    #[garde(length(min = 1, max = 200))]
    pub room_type_ids: Vec<Uuid>,
}

/// Fields left out stay as they are; `residency` and `cancellation_policy_id` sent as `null` are cleared.
/// The code, kind and currency never change.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdateRatePlanRequest {
    #[garde(inner(length(chars, min = 1, max = 100)))]
    pub name: Option<String>,
    #[garde(skip)]
    pub segment: Option<Segment>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<Residency>, nullable)]
    #[garde(skip)]
    pub residency: Option<Option<Residency>>,
    /// Moves a derived plan under another parent; its prices (and inherited restrictions) follow the new one.
    #[garde(skip)]
    pub parent_id: Option<Uuid>,
    #[garde(skip)]
    pub derive_mode: Option<ChangeMode>,
    #[garde(skip)]
    pub derive_value: Option<i64>,
    #[garde(inner(range(min = 1, max = 100_000_000)))]
    pub rounding_step: Option<i64>,
    #[garde(inner(range(min = 0, max = MAX_AMOUNT)))]
    pub extra_adult_amount: Option<i64>,
    #[garde(skip)]
    pub inherit_restrictions: Option<bool>,
    #[garde(inner(length(min = 1, max = 4)))]
    pub allowed_meal_plans: Option<Vec<MealPlan>>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<Uuid>, nullable)]
    #[garde(skip)]
    pub cancellation_policy_id: Option<Option<Uuid>>,
    #[garde(inner(length(min = 1, max = 200)))]
    pub room_type_ids: Option<Vec<Uuid>>,
    /// `false` stops selling the plan; its prices stay.
    #[garde(skip)]
    pub active: Option<bool>,
}

impl Changes for UpdateRatePlanRequest {
    fn is_empty(&self) -> bool {
        self.name.is_none()
            && self.segment.is_none()
            && self.residency.is_none()
            && self.parent_id.is_none()
            && self.derive_mode.is_none()
            && self.derive_value.is_none()
            && self.rounding_step.is_none()
            && self.extra_adult_amount.is_none()
            && self.inherit_restrictions.is_none()
            && self.allowed_meal_plans.is_none()
            && self.cancellation_policy_id.is_none()
            && self.room_type_ids.is_none()
            && self.active.is_none()
    }
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct PriceRequest {
    #[garde(skip)]
    pub room_type_id: Uuid,
    /// `YYYY-MM-DD`, from the business date for 730 days.
    #[garde(skip)]
    pub date: Date,
    /// Adults, 1 to the room type's maximum occupancy.
    #[garde(range(min = 1, max = 50))]
    pub occupancy: i32,
    /// Minor units in the plan's currency.
    #[garde(range(min = 0, max = MAX_AMOUNT))]
    pub amount: i64,
}

/// Prices for a standard or custom plan; derived plans are repriced from them in the same transaction.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct SetPricesRequest {
    #[garde(length(min = 1, max = 5000), dive)]
    pub prices: Vec<PriceRequest>,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct PriceChangeRequest {
    #[garde(skip)]
    pub mode: PriceChangeMode,
    /// Basis points (`percent`), or minor units (`amount`, `set`).
    #[garde(skip)]
    pub value: i64,
}

/// Changes a plan's prices on `[from, to)`. Left out, `weekdays` (ISO: 1 = Monday … 7 = Sunday),
/// `room_type_ids` and `occupancies` mean all.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct BulkChangeRequest {
    #[garde(skip)]
    pub from: Date,
    #[garde(skip)]
    pub to: Date,
    #[serde(default)]
    #[garde(length(max = 7), inner(range(min = 1, max = 7)))]
    pub weekdays: Vec<u8>,
    #[serde(default)]
    #[garde(length(max = 200))]
    pub room_type_ids: Vec<Uuid>,
    #[serde(default)]
    #[garde(length(max = 50), inner(range(min = 1, max = 50)))]
    pub occupancies: Vec<i32>,
    #[garde(dive)]
    pub change: PriceChangeRequest,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct BulkChangeResponse {
    /// How many of the plan's own prices changed.
    pub changed: u64,
}

/// Sets restrictions on every day of `[from, to)`. Left out, `weekdays` (ISO) and `room_type_ids` mean all,
/// and a restriction field means "leave as it is"; `min_stay` or `max_stay` sent as `null` removes it.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct RestrictionsRequest {
    #[garde(skip)]
    pub from: Date,
    #[garde(skip)]
    pub to: Date,
    #[serde(default)]
    #[garde(length(max = 7), inner(range(min = 1, max = 7)))]
    pub weekdays: Vec<u8>,
    #[serde(default)]
    #[garde(length(max = 200))]
    pub room_type_ids: Vec<Uuid>,
    #[garde(skip)]
    pub closed: Option<bool>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<i32>, nullable)]
    #[garde(skip)]
    pub min_stay: Option<Option<i32>>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<i32>, nullable)]
    #[garde(skip)]
    pub max_stay: Option<Option<i32>>,
    #[garde(skip)]
    pub closed_to_arrival: Option<bool>,
    #[garde(skip)]
    pub closed_to_departure: Option<bool>,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreateMealSupplementRequest {
    /// `BB`, `HB` or `FB`: room only has no supplement.
    #[garde(skip)]
    pub meal_plan: MealPlan,
    #[garde(pattern(r"^[A-Z]{3}$"))]
    pub currency: String,
    /// Minor units per adult per night.
    #[garde(range(min = 0, max = MAX_AMOUNT))]
    pub adult_amount: i64,
    /// Minor units per child per night.
    #[garde(range(min = 0, max = MAX_AMOUNT))]
    pub child_amount: i64,
    /// The first night it applies to.
    #[garde(skip)]
    pub from: Date,
    /// The first night it no longer applies to; left out, until further notice.
    #[garde(skip)]
    pub to: Option<Date>,
}

/// Fields left out stay as they are; `to` sent as `null` makes the supplement open-ended.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdateMealSupplementRequest {
    #[garde(inner(range(min = 0, max = MAX_AMOUNT)))]
    pub adult_amount: Option<i64>,
    #[garde(inner(range(min = 0, max = MAX_AMOUNT)))]
    pub child_amount: Option<i64>,
    #[garde(skip)]
    pub from: Option<Date>,
    #[serde(default, deserialize_with = "present")]
    #[schema(value_type = Option<Date>, nullable)]
    #[garde(skip)]
    pub to: Option<Option<Date>>,
}

impl Changes for UpdateMealSupplementRequest {
    fn is_empty(&self) -> bool {
        self.adult_amount.is_none() && self.child_amount.is_none() && self.from.is_none() && self.to.is_none()
    }
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CreateCancellationPolicyRequest {
    #[garde(length(chars, min = 1, max = 100))]
    pub name: String,
    /// Each with its own number of days before arrival.
    #[garde(length(max = 10))]
    pub rules: Vec<CancellationRule>,
    #[garde(skip)]
    pub no_show: Penalty,
}

/// Fields left out stay as they are; `rules` replaces every rule.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct UpdateCancellationPolicyRequest {
    #[garde(inner(length(chars, min = 1, max = 100)))]
    pub name: Option<String>,
    #[garde(inner(length(max = 10)))]
    pub rules: Option<Vec<CancellationRule>>,
    #[garde(skip)]
    pub no_show: Option<Penalty>,
}

impl Changes for UpdateCancellationPolicyRequest {
    fn is_empty(&self) -> bool {
        self.name.is_none() && self.rules.is_none() && self.no_show.is_none()
    }
}

/// ISO weekday numbers (1 = Monday) as weekdays; validation keeps them in 1 to 7.
fn weekdays(numbers: &[u8]) -> Vec<Weekday> {
    numbers.iter().map(|number| Weekday::Monday.nth_next(number - 1)).collect()
}

#[utoipa::path(post, operation_id = "create_rate_plan", path = "/api/v1/properties/{property}/rate-plans", request_body = CreateRatePlanRequest,
    params(("property" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 201, body = RatePlan,
        headers(("ETag" = String, description = "the version, e.g. \"1\"; send it back as If-Match"))), (status = 403), (status = 404), (status = 409), (status = 422)))]
pub async fn create_plan(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
    ApiJson(body): ApiJson<CreateRatePlanRequest>,
) -> Result<Versioned<RatePlan>, ApiError> {
    ctx.require(Permission::RatesManage, Some(property))?;
    validate(&body)?;
    let input = NewRatePlan {
        code: body.code,
        name: body.name,
        kind: body.kind,
        segment: body.segment,
        residency: body.residency,
        currency: body.currency,
        parent_id: body.parent_id,
        derive_mode: body.derive_mode,
        derive_value: body.derive_value,
        rounding_step: body.rounding_step,
        extra_adult_amount: body.extra_adult_amount,
        inherit_restrictions: body.inherit_restrictions,
        allowed_meal_plans: body.allowed_meal_plans,
        cancellation_policy_id: body.cancellation_policy_id,
        room_type_ids: body.room_type_ids,
    };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let created = rates::create_rate_plan(&mut tx, ctx.tenant, ctx.user, property, input).await.map_err(rates_error)?;
    tx.commit().await?;
    Ok(Versioned::created(created.version, created))
}

#[utoipa::path(patch, operation_id = "update_rate_plan", path = "/api/v1/properties/{property}/rate-plans/{plan}", request_body = UpdateRatePlanRequest,
    params(("property" = Uuid, Path), ("plan" = Uuid, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = RatePlan,
        headers(("ETag" = String, description = "the version, e.g. \"1\"; send it back as If-Match"))), (status = 403), (status = 404), (status = 409), (status = 412), (status = 422), (status = 428)))]
pub async fn update_plan(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, plan)): ApiPath<(Uuid, Uuid)>,
    IfMatch(version): IfMatch,
    ApiJson(body): ApiJson<UpdateRatePlanRequest>,
) -> Result<Versioned<RatePlan>, ApiError> {
    ctx.require(Permission::RatesManage, Some(property))?;
    validate_changes(&body)?;
    let changes = RatePlanChanges {
        name: body.name,
        segment: body.segment,
        residency: body.residency,
        parent_id: body.parent_id,
        derive_mode: body.derive_mode,
        derive_value: body.derive_value,
        rounding_step: body.rounding_step,
        extra_adult_amount: body.extra_adult_amount,
        inherit_restrictions: body.inherit_restrictions,
        allowed_meal_plans: body.allowed_meal_plans,
        cancellation_policy_id: body.cancellation_policy_id,
        room_type_ids: body.room_type_ids,
        active: body.active,
    };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let updated = rates::update_rate_plan(&mut tx, ctx.tenant, ctx.user, property, plan, version, changes)
        .await
        .map_err(rates_error)?;
    tx.commit().await?;
    Ok(Versioned::ok(updated.version, updated))
}

/// Sets prices cell by cell (the rate grid saves edited cells in batches). Not versioned: the last write to a
/// cell wins. A cell listed twice takes its last amount.
#[utoipa::path(put, operation_id = "set_prices", path = "/api/v1/properties/{property}/rate-plans/{plan}/prices", request_body = SetPricesRequest,
    params(("property" = Uuid, Path), ("plan" = Uuid, Path)),
    responses((status = 204), (status = 403), (status = 404), (status = 422)))]
pub async fn set_prices(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, plan)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<SetPricesRequest>,
) -> Result<StatusCode, ApiError> {
    ctx.require(Permission::RatesManage, Some(property))?;
    validate(&body)?;
    let prices: Vec<Price> = body
        .prices
        .into_iter()
        .map(|p| Price { room_type_id: p.room_type_id, date: p.date, occupancy: p.occupancy, amount: p.amount })
        .collect();
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    rates::set_prices(&mut tx, ctx.tenant, ctx.user, property, plan, &prices).await.map_err(rates_error)?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// A percent or amount change is not safe to repeat, so this command takes an `Idempotency-Key` like a create.
#[utoipa::path(post, operation_id = "bulk_change_prices", path = "/api/v1/properties/{property}/rate-plans/{plan}/bulk-change", request_body = BulkChangeRequest,
    params(("property" = Uuid, Path), ("plan" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 200, body = BulkChangeResponse), (status = 403), (status = 404), (status = 422)))]
pub async fn bulk_change(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, plan)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<BulkChangeRequest>,
) -> Result<Json<BulkChangeResponse>, ApiError> {
    ctx.require(Permission::RatesManage, Some(property))?;
    validate(&body)?;
    let change = BulkChange {
        from: body.from,
        to: body.to,
        weekdays: weekdays(&body.weekdays),
        room_type_ids: body.room_type_ids,
        occupancies: body.occupancies,
        change: PriceChange { mode: body.change.mode, value: body.change.value },
    };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let changed =
        rates::bulk_change(&mut tx, ctx.tenant, ctx.user, property, plan, &change).await.map_err(rates_error)?;
    tx.commit().await?;
    Ok(Json(BulkChangeResponse { changed }))
}

#[utoipa::path(put, operation_id = "set_restrictions", path = "/api/v1/properties/{property}/rate-plans/{plan}/restrictions", request_body = RestrictionsRequest,
    params(("property" = Uuid, Path), ("plan" = Uuid, Path)),
    responses((status = 204), (status = 403), (status = 404), (status = 422)))]
pub async fn set_restrictions(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, plan)): ApiPath<(Uuid, Uuid)>,
    ApiJson(body): ApiJson<RestrictionsRequest>,
) -> Result<StatusCode, ApiError> {
    ctx.require(Permission::RatesManage, Some(property))?;
    validate(&body)?;
    let change = RestrictionChange {
        from: body.from,
        to: body.to,
        weekdays: weekdays(&body.weekdays),
        room_type_ids: body.room_type_ids,
        closed: body.closed,
        min_stay: body.min_stay,
        max_stay: body.max_stay,
        closed_to_arrival: body.closed_to_arrival,
        closed_to_departure: body.closed_to_departure,
    };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    rates::set_restrictions(&mut tx, ctx.tenant, ctx.user, property, plan, &change).await.map_err(rates_error)?;
    tx.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(post, operation_id = "create_meal_supplement", path = "/api/v1/properties/{property}/meal-supplements", request_body = CreateMealSupplementRequest,
    params(("property" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 201, body = MealSupplement,
        headers(("ETag" = String, description = "the version, e.g. \"1\"; send it back as If-Match"))), (status = 403), (status = 404), (status = 409), (status = 422)))]
pub async fn create_supplement(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
    ApiJson(body): ApiJson<CreateMealSupplementRequest>,
) -> Result<Versioned<MealSupplement>, ApiError> {
    ctx.require(Permission::RatesManage, Some(property))?;
    validate(&body)?;
    let input = NewMealSupplement {
        meal_plan: body.meal_plan,
        currency: body.currency,
        adult_amount: body.adult_amount,
        child_amount: body.child_amount,
        from: body.from,
        to: body.to,
    };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let created =
        rates::create_meal_supplement(&mut tx, ctx.tenant, ctx.user, property, input).await.map_err(rates_error)?;
    tx.commit().await?;
    Ok(Versioned::created(created.version, created))
}

#[utoipa::path(patch, operation_id = "update_meal_supplement", path = "/api/v1/properties/{property}/meal-supplements/{supplement}", request_body = UpdateMealSupplementRequest,
    params(("property" = Uuid, Path), ("supplement" = Uuid, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = MealSupplement,
        headers(("ETag" = String, description = "the version, e.g. \"1\"; send it back as If-Match"))), (status = 403), (status = 404), (status = 409), (status = 412), (status = 422), (status = 428)))]
pub async fn update_supplement(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, supplement)): ApiPath<(Uuid, Uuid)>,
    IfMatch(version): IfMatch,
    ApiJson(body): ApiJson<UpdateMealSupplementRequest>,
) -> Result<Versioned<MealSupplement>, ApiError> {
    ctx.require(Permission::RatesManage, Some(property))?;
    validate_changes(&body)?;
    let changes = MealSupplementChanges {
        adult_amount: body.adult_amount,
        child_amount: body.child_amount,
        from: body.from,
        to: body.to,
    };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let updated = rates::update_meal_supplement(&mut tx, ctx.tenant, ctx.user, property, supplement, version, changes)
        .await
        .map_err(rates_error)?;
    tx.commit().await?;
    Ok(Versioned::ok(updated.version, updated))
}

#[utoipa::path(post, operation_id = "create_cancellation_policy", path = "/api/v1/properties/{property}/cancellation-policies", request_body = CreateCancellationPolicyRequest,
    params(("property" = Uuid, Path), ("Idempotency-Key" = String, Header)),
    responses((status = 201, body = CancellationPolicy,
        headers(("ETag" = String, description = "the version, e.g. \"1\"; send it back as If-Match"))), (status = 403), (status = 404), (status = 409), (status = 422)))]
pub async fn create_policy(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath(property): ApiPath<Uuid>,
    ApiJson(body): ApiJson<CreateCancellationPolicyRequest>,
) -> Result<Versioned<CancellationPolicy>, ApiError> {
    ctx.require(Permission::RatesManage, Some(property))?;
    validate(&body)?;
    let input = NewCancellationPolicy { name: body.name, rules: body.rules, no_show: body.no_show };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let created =
        rates::create_cancellation_policy(&mut tx, ctx.tenant, ctx.user, property, input).await.map_err(rates_error)?;
    tx.commit().await?;
    Ok(Versioned::created(created.version, created))
}

#[utoipa::path(patch, operation_id = "update_cancellation_policy", path = "/api/v1/properties/{property}/cancellation-policies/{policy}", request_body = UpdateCancellationPolicyRequest,
    params(("property" = Uuid, Path), ("policy" = Uuid, Path), ("If-Match" = String, Header)),
    responses((status = 200, body = CancellationPolicy,
        headers(("ETag" = String, description = "the version, e.g. \"1\"; send it back as If-Match"))), (status = 403), (status = 404), (status = 409), (status = 412), (status = 422), (status = 428)))]
pub async fn update_policy(
    State(state): State<AppState>,
    ctx: TenantContext,
    ApiPath((property, policy)): ApiPath<(Uuid, Uuid)>,
    IfMatch(version): IfMatch,
    ApiJson(body): ApiJson<UpdateCancellationPolicyRequest>,
) -> Result<Versioned<CancellationPolicy>, ApiError> {
    ctx.require(Permission::RatesManage, Some(property))?;
    validate_changes(&body)?;
    let changes = CancellationPolicyChanges { name: body.name, rules: body.rules, no_show: body.no_show };
    let mut tx = db::begin(&state.pool, Scope::tenant(ctx.tenant)).await?;
    let updated = rates::update_cancellation_policy(&mut tx, ctx.tenant, ctx.user, property, policy, version, changes)
        .await
        .map_err(rates_error)?;
    tx.commit().await?;
    Ok(Versioned::ok(updated.version, updated))
}
```

Modify `crates/core-api/src/routes/rooms.rs`:

```diff
diff --git a/crates/core-api/src/routes/rooms.rs b/crates/core-api/src/routes/rooms.rs
index fd2a274..42be52f 100644
--- a/crates/core-api/src/routes/rooms.rs
+++ b/crates/core-api/src/routes/rooms.rs
@@ -27,7 +27,9 @@ pub(crate) fn rooms_error(err: RoomsError) -> ApiError {
 }
 
 /// Tells a field sent as `null` (`Some(None)`: clear it) from one left out (`None`: keep it).
-fn present<'de, T: Deserialize<'de>, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Option<T>>, D::Error> {
+pub(crate) fn present<'de, T: Deserialize<'de>, D: Deserializer<'de>>(
+    deserializer: D,
+) -> Result<Option<Option<T>>, D::Error> {
     Option::<T>::deserialize(deserializer).map(Some)
 }
 
```

- [ ] **Step 4: Run the checks**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cd web/pms
bun run api:schemas && bun run codegen && git diff --exit-code -- src/lib/api
bun run lint && bun run check && bun run test && bun run build
```

Expected: every test passes (1 ignored, the Phase 1 gate); regenerate and commit the API types.

- [ ] **Step 5: Commit**

```sh
git add -A
git commit -m "feat(api): rate plan, price, bulk change, restriction, meal supplement and cancellation policy commands"
```

### Task 11: GraphQL reads and the performance gates

`ratePlans` (tree order), `rateGrid` (prices and restrictions, at most 93 days), `bulkChangePreview` (first 50
cells and the total), `mealSupplements`, `cancellationPolicies` and `quote` (at most 90 nights), all behind
`RatesView`, with the rates enums mirrored as GraphQL enums. Two ignored release-mode tests measure the
spec's gates. Profiling the bulk gate (first ~900 ms) led to three changes in earlier code: `app.derive_amount`
loses `STRICT` so it is inlined, `rate_day` gets `fillfactor = 50` so a full rewrite is HOT, and existing
prices are rewritten with `UPDATE` (plus an insert of missing cells where a parent can gain some) instead of
an upsert; see the Verified section for the numbers.

**Files:**
- Modify: `crates/core-api/src/graphql.rs`
- Test: `crates/core-api/tests/perf.rs`
- Test: `crates/core-api/tests/rate_reads.rs` (new)
- Modify: `migrations/0006_rates.sql`
- Modify: `modules/rates/src/plans.rs`
- Modify: `modules/rates/src/prices.rs`

**Interfaces:**
- Consumes: `rates::{list_rate_plans, list_prices, list_restrictions, preview_bulk_change, list_meal_supplements, list_cancellation_policies, load_quote}`.
- Produces: GraphQL `ratePlans`, `rateGrid { prices restrictions }`, `bulkChangePreview`, `mealSupplements`, `cancellationPolicies`, `quote`, and enums `PlanKind`, `Segment`, `Residency`, `ChangeMode`, `MealPlan`, `PriceChangeMode`, `PenaltyKind`, `ViolationKind`; crate-internal `rates::prices::Reprice::{Existing, Added, Moved}`.

- [ ] **Step 1: Write the failing tests**

Modify `crates/core-api/tests/perf.rs`:

````diff
diff --git a/crates/core-api/tests/perf.rs b/crates/core-api/tests/perf.rs
index ccdb89d..4f2d715 100644
--- a/crates/core-api/tests/perf.rs
+++ b/crates/core-api/tests/perf.rs
@@ -1,11 +1,15 @@
-//! Phase 1 performance gate: one month of `inventory` for a 200-room, 12-type property is answered in
-//! under 20 ms at p95. Timed in-process through the router, so it is server time: authentication, the
-//! indexed range scan of at most 372 rows, and JSON, without the network.
+//! Performance gates, timed in-process through the router, so they measure server time (authentication, the
+//! queries and JSON) without the network:
 //!
-//! Ignored by default because debug builds are several times slower. Run it in release mode:
+//! - Phase 1: one month of `inventory` for a 200-room, 12-type property in under 20 ms at p95.
+//! - Phase 2: a 62-day `rateGrid` for 1 plan and 12 room types with 2 occupancies (1488 prices, plus a
+//!   restriction per type and day) in under 30 ms at p95; a bulk change of one year of prices for 12 room
+//!   types, with two levels of derived plans below it, in under 300 ms (median of ten runs).
+//!
+//! Ignored by default because debug builds are several times slower. Run them in release mode, one at a time:
 //!
 //! ```sh
-//! DATABASE_URL=$TEST_DATABASE_URL cargo test --release -p core-api --test perf -- --ignored --nocapture
+//! DATABASE_URL=$TEST_DATABASE_URL cargo test --release -p core-api --test perf -- --ignored --nocapture --test-threads=1
 //! ```
 
 mod common;
@@ -88,3 +92,150 @@ async fn a_month_of_inventory_for_200_rooms_is_served_under_20ms_at_p95(_: PgPoo
     println!("inventory(month), {ROOMS} rooms / {ROOM_TYPES} types: p50 {p50:?}, p95 {p95:?}");
     assert!(p95 < Duration::from_millis(20), "p95 {p95:?} is over the 20 ms gate");
 }
+
+/// An owner's property with 12 room types for up to 2 adults and a child; returns the property's API path,
+/// its business date and the room type ids.
+async fn property_with_room_types(app: &TestApp, owner: &str) -> (String, time::Date, Vec<Value>) {
+    let property = json!({"code": "RATES", "name": "Rates Hotel", "timezone": "Asia/Colombo", "base_currency": "LKR"});
+    let property = post(app, owner, "/api/v1/properties", property).await.body;
+    let path = format!("/api/v1/properties/{}", property["id"].as_str().unwrap());
+    let business_date = time::Date::parse(property["business_date"].as_str().unwrap(), &Iso8601::DEFAULT).unwrap();
+    let mut types = Vec::new();
+    for index in 0..ROOM_TYPES {
+        let room_type = json!({"code": format!("T{index}"), "name": format!("Type {index}"), "base_occupancy": 2,
+                               "max_adults": 2, "max_children": 1, "max_occupancy": 3});
+        types.push(post(app, owner, &format!("{path}/room-types"), room_type).await.body["id"].clone());
+    }
+    (path, business_date, types)
+}
+
+/// Prices for 1 and 2 adults in every room type on each day of `[business date, + days)`, in requests of at
+/// most 5000 prices.
+async fn price_every_cell(app: &TestApp, owner: &str, plan_path: &str, day0: time::Date, types: &[Value], days: i64) {
+    let prices: Vec<Value> = (0..days)
+        .flat_map(|day| types.iter().map(move |room_type| (day, room_type)))
+        .flat_map(|(day, room_type)| {
+            let date = (day0 + time::Duration::days(day)).to_string();
+            (1..=2).map(move |occupancy| {
+                json!({"room_type_id": room_type, "date": date, "occupancy": occupancy, "amount": 10_000 + day * 10})
+            })
+        })
+        .collect();
+    for batch in prices.chunks(5000) {
+        let response =
+            app.send(Method::PUT, &format!("{plan_path}/prices"), Some(owner), Some(json!({"prices": batch}))).await;
+        assert_eq!(response.status, StatusCode::NO_CONTENT, "{:?}", response.body);
+    }
+}
+
+fn rate_plan(code: &str, types: &[Value], parent: Option<&Value>) -> Value {
+    match parent {
+        None => json!({"code": code, "name": code, "kind": "standard", "segment": "IBE", "currency": "USD",
+                       "room_type_ids": types}),
+        Some(parent) => json!({"code": code, "name": code, "kind": "derived", "segment": "OTA", "currency": "USD",
+                               "parent_id": parent["id"], "derive_mode": "percent", "derive_value": 1500,
+                               "rounding_step": 100, "inherit_restrictions": true, "room_type_ids": types}),
+    }
+}
+
+#[sqlx::test(migrator = "db::MIGRATOR")]
+#[ignore = "performance gate; run in release mode (see the module docs)"]
+async fn a_62_day_rate_grid_for_12_room_types_is_served_under_30ms_at_p95(_: PgPoolOptions, opts: PgConnectOptions) {
+    const DAYS: i64 = 62;
+    let app = TestApp::new(opts).await;
+    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
+    let (path, day0, types) = property_with_room_types(&app, &owner).await;
+    let bar = post(&app, &owner, &format!("{path}/rate-plans"), rate_plan("BAR", &types, None)).await.body;
+    let bar_path = format!("{path}/rate-plans/{}", bar["id"].as_str().unwrap());
+    price_every_cell(&app, &owner, &bar_path, day0, &types, DAYS).await;
+    let to = (day0 + time::Duration::days(DAYS)).to_string();
+    let restriction = json!({"from": day0.to_string(), "to": to, "min_stay": 2, "closed_to_arrival": false});
+    let restricted = app.send(Method::PUT, &format!("{bar_path}/restrictions"), Some(&owner), Some(restriction)).await;
+    assert_eq!(restricted.status, StatusCode::NO_CONTENT, "{:?}", restricted.body);
+    let query = json!({
+        "query": "query ($p: UUID!, $plan: UUID!, $from: Date!, $to: Date!) {
+                    rateGrid(propertyId: $p, ratePlanId: $plan, from: $from, to: $to) {
+                      prices { roomTypeId date occupancy amount }
+                      restrictions { roomTypeId date closed minStay maxStay closedToArrival closedToDeparture }
+                    }
+                  }",
+        "variables": {"p": path.trim_start_matches("/api/v1/properties/"), "plan": bar["id"],
+                      "from": day0.to_string(), "to": to},
+    });
+
+    let mut samples: Vec<Duration> = Vec::with_capacity(SAMPLES);
+    for round in 0..SAMPLES + 20 {
+        let started = Instant::now();
+        let response = app.send(Method::POST, "/graphql", Some(&owner), Some(query.clone())).await;
+        let elapsed = started.elapsed();
+        let grid = &response.body["data"]["rateGrid"];
+        assert_eq!(grid["prices"].as_array().map(Vec::len), Some(1488), "{:?}", response.body);
+        assert_eq!(grid["restrictions"].as_array().map(Vec::len), Some(744));
+        if round >= 20 {
+            samples.push(elapsed);
+        }
+    }
+
+    samples.sort();
+    let p50 = samples[SAMPLES / 2];
+    let p95 = samples[SAMPLES * 95 / 100 - 1];
+    println!("rateGrid, 62 days x 12 types x 2 occupancies: p50 {p50:?}, p95 {p95:?}");
+    assert!(p95 < Duration::from_millis(30), "p95 {p95:?} is over the 30 ms gate");
+}
+
+#[sqlx::test(migrator = "db::MIGRATOR")]
+#[ignore = "performance gate; run in release mode (see the module docs)"]
+async fn a_year_of_bulk_change_with_two_derived_levels_takes_under_300ms_at_the_median(
+    _: PgPoolOptions,
+    opts: PgConnectOptions,
+) {
+    const DAYS: i64 = 365;
+    const RUNS: usize = 10;
+    let app = TestApp::new(opts.clone()).await;
+    let superuser = PgPool::connect_with(opts).await.unwrap();
+    let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
+    let (path, day0, types) = property_with_room_types(&app, &owner).await;
+    let bar = post(&app, &owner, &format!("{path}/rate-plans"), rate_plan("BAR", &types, None)).await.body;
+    let bar_path = format!("{path}/rate-plans/{}", bar["id"].as_str().unwrap());
+    price_every_cell(&app, &owner, &bar_path, day0, &types, DAYS).await;
+    let ota = post(&app, &owner, &format!("{path}/rate-plans"), rate_plan("OTA", &types, Some(&bar))).await.body;
+    post(&app, &owner, &format!("{path}/rate-plans"), rate_plan("OTA-NR", &types, Some(&ota))).await;
+    let rows: i64 = sqlx::query_scalar("select count(*) from rate_day").fetch_one(&superuser).await.unwrap();
+    assert_eq!(rows, 3 * 365 * 12 * 2, "three plans, every cell priced");
+    // As autovacuum leaves a table after a bulk load, so it does not start in the middle of the timed runs.
+    sqlx::query("vacuum analyze rate_day").execute(&superuser).await.unwrap();
+
+    let mut runs: Vec<Duration> = Vec::with_capacity(RUNS);
+    for run in 0..RUNS + 2 {
+        // Up 5 %, then down 5 %, so every run changes every price.
+        let value = if run % 2 == 0 { 500 } else { -500 };
+        let change = json!({"from": day0.to_string(), "to": (day0 + time::Duration::days(DAYS)).to_string(),
+                            "change": {"mode": "percent", "value": value}});
+        let started = Instant::now();
+        let response = post_ok(&app, &owner, &format!("{bar_path}/bulk-change"), change).await;
+        let elapsed = started.elapsed();
+        assert_eq!(response.body["changed"], 365 * 12 * 2, "{:?}", response.body);
+        // The first two warm the connection pool and Postgres' caches.
+        if run >= 2 {
+            runs.push(elapsed);
+        }
+    }
+
+    runs.sort();
+    let (median, slowest) = (runs[RUNS / 2], runs[RUNS - 1]);
+    println!(
+        "bulk change, 365 days x 12 types x 2 occupancies, 2 derived levels: median {median:?}, slowest {slowest:?}"
+    );
+    // The median of ten runs, like the p95 of the read gates: one slow run on a busy machine is noise.
+    assert!(median < Duration::from_millis(300), "median {median:?} is over the 300 ms gate");
+}
+
+/// Sends a command with a fresh idempotency key and expects 200.
+async fn post_ok(app: &TestApp, cookie: &str, path: &str, body: Value) -> TestResponse {
+    let key = Uuid::now_v7().to_string();
+    let response = app
+        .send_with(Method::POST, path, Some(cookie), Some(body), &[("x-goodfolk-csrf", "1"), ("idempotency-key", &key)])
+        .await;
+    assert_eq!(response.status, StatusCode::OK, "{:?}", response.body);
+    response
+}
````

Create `crates/core-api/tests/rate_reads.rs`:

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
    let response = app
        .send_with(Method::POST, path, Some(cookie), Some(body), &[("x-goodfolk-csrf", "1"), ("idempotency-key", &key)])
        .await;
    assert!(response.status.is_success(), "{path}: {:?}", response.body);
    response
}

async fn graphql(app: &TestApp, cookie: &str, query: &str, variables: Value) -> Value {
    let response =
        app.send(Method::POST, "/graphql", Some(cookie), Some(json!({"query": query, "variables": variables}))).await;
    assert_eq!(response.status, StatusCode::OK, "{:?}", response.body);
    response.body
}

/// A property with a deluxe room type, BAR (USD, standard) and OTA (BAR + 15 %, inheriting restrictions),
/// BAR priced at 100.00 for two adults on the first week, a minimum stay of 2 on the third day and USD
/// breakfast at 15.00 per adult and 7.50 per child.
struct Hotel {
    owner: String,
    superuser: PgPool,
    id: String,
    business_date: Date,
    deluxe: Value,
    bar: Value,
    ota: Value,
}

impl Hotel {
    async fn new(app: &TestApp, opts: PgConnectOptions) -> Self {
        let superuser = PgPool::connect_with(opts).await.unwrap();
        let owner = app.signup_owner("owner@example.com", "Lagoon Hotels").await;
        let property = json!({"code": "GAL", "name": "Galle", "timezone": "Asia/Colombo", "base_currency": "LKR"});
        let property = post(app, &owner, "/api/v1/properties", property).await.body;
        let id = property["id"].as_str().unwrap().to_owned();
        let path = format!("/api/v1/properties/{id}");
        let business_date = Date::parse(property["business_date"].as_str().unwrap(), &Iso8601::DEFAULT).unwrap();
        let day = |offset: i64| (business_date + Duration::days(offset)).to_string();
        let deluxe = json!({"code": "DLX", "name": "Deluxe", "base_occupancy": 2, "max_adults": 2,
                            "max_children": 1, "max_occupancy": 3});
        let deluxe = post(app, &owner, &format!("{path}/room-types"), deluxe).await.body["id"].clone();
        let bar = json!({"code": "BAR", "name": "Best available", "kind": "standard", "segment": "FIT_F",
                         "currency": "USD", "allowed_meal_plans": ["RO", "BB"], "room_type_ids": [deluxe]});
        let bar = post(app, &owner, &format!("{path}/rate-plans"), bar).await.body;
        let ota = json!({"code": "OTA", "name": "Online agents", "kind": "derived", "segment": "OTA",
                         "currency": "USD", "parent_id": bar["id"], "derive_mode": "percent", "derive_value": 1500,
                         "inherit_restrictions": true, "allowed_meal_plans": ["RO", "BB"], "room_type_ids": [deluxe]});
        let ota = post(app, &owner, &format!("{path}/rate-plans"), ota).await.body;
        let bar_path = format!("{path}/rate-plans/{}", bar["id"].as_str().unwrap());
        let prices: Vec<Value> = (0..7)
            .map(|offset| json!({"room_type_id": deluxe, "date": day(offset), "occupancy": 2, "amount": 10_000}))
            .collect();
        let priced =
            app.send(Method::PUT, &format!("{bar_path}/prices"), Some(&owner), Some(json!({"prices": prices}))).await;
        assert_eq!(priced.status, StatusCode::NO_CONTENT);
        let restriction = json!({"from": day(2), "to": day(3), "min_stay": 2});
        let restricted =
            app.send(Method::PUT, &format!("{bar_path}/restrictions"), Some(&owner), Some(restriction)).await;
        assert_eq!(restricted.status, StatusCode::NO_CONTENT);
        let breakfast = json!({"meal_plan": "BB", "currency": "USD", "adult_amount": 1500, "child_amount": 750,
                               "from": day(0)});
        post(app, &owner, &format!("{path}/meal-supplements"), breakfast).await;
        Self { owner, superuser, id, business_date, deluxe, bar, ota }
    }

    fn day(&self, offset: i64) -> String {
        (self.business_date + Duration::days(offset)).to_string()
    }
}

const PLANS: &str = "query ($p: UUID!) {
    ratePlans(propertyId: $p) { code kind segment residency currency depth deriveMode deriveValue inheritRestrictions
                                allowedMealPlans roomTypeIds parentId version }
    mealSupplements(propertyId: $p) { mealPlan currency adultAmount childAmount from to }
    cancellationPolicies(propertyId: $p) { name }
}";

const GRID: &str = "query ($p: UUID!, $plan: UUID!, $from: Date!, $to: Date!) {
    rateGrid(propertyId: $p, ratePlanId: $plan, from: $from, to: $to) {
        prices { roomTypeId date occupancy amount }
        restrictions { date closed minStay maxStay closedToArrival closedToDeparture }
    }
}";

const QUOTE: &str = "query ($p: UUID!, $type: UUID!, $plan: UUID!, $in: Date!, $out: Date!, $residency: Residency!) {
    quote(propertyId: $p, roomTypeId: $type, ratePlanId: $plan, mealPlan: BB, checkIn: $in, checkOut: $out,
          adults: 2, children: 1, residency: $residency) {
        nights { date room meal } total currency restrictionsOk violations { kind date message }
    }
}";

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn plans_supplements_and_the_rate_grid_are_read_over_graphql(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;

    let plans = graphql(&app, &hotel.owner, PLANS, json!({"p": hotel.id})).await;
    let grid = graphql(
        &app,
        &hotel.owner,
        GRID,
        json!({"p": hotel.id, "plan": hotel.ota["id"], "from": hotel.day(0), "to": hotel.day(31)}),
    )
    .await;
    let too_long = graphql(
        &app,
        &hotel.owner,
        GRID,
        json!({"p": hotel.id, "plan": hotel.ota["id"], "from": hotel.day(0), "to": hotel.day(94)}),
    )
    .await;

    let plans = &plans["data"];
    assert_eq!(plans["ratePlans"][0]["code"], "BAR");
    assert_eq!(plans["ratePlans"][0]["segment"], "FIT_F");
    assert_eq!(plans["ratePlans"][0]["residency"], "NON_RESIDENT");
    assert_eq!(plans["ratePlans"][0]["allowedMealPlans"], json!(["RO", "BB"]));
    assert_eq!(plans["ratePlans"][1]["code"], "OTA");
    assert_eq!(plans["ratePlans"][1]["depth"], 1);
    assert_eq!(plans["ratePlans"][1]["deriveMode"], "PERCENT");
    assert_eq!(plans["ratePlans"][1]["parentId"], hotel.bar["id"]);
    assert_eq!(plans["ratePlans"][1]["roomTypeIds"], json!([hotel.deluxe]));
    assert_eq!(
        plans["mealSupplements"],
        json!([{"mealPlan": "BB", "currency": "USD", "adultAmount": 1500,
                                                  "childAmount": 750, "from": hotel.day(0), "to": null}])
    );
    assert_eq!(plans["cancellationPolicies"], json!([]));
    let prices = grid["data"]["rateGrid"]["prices"].as_array().unwrap();
    assert_eq!(prices.len(), 7);
    assert_eq!(prices[0], json!({"roomTypeId": hotel.deluxe, "date": hotel.day(0), "occupancy": 2, "amount": 11500}));
    assert_eq!(
        grid["data"]["rateGrid"]["restrictions"],
        json!([{"date": hotel.day(2), "closed": false, "minStay": 2, "maxStay": null, "closedToArrival": false,
                "closedToDeparture": false}]),
        "OTA inherits BAR's restrictions"
    );
    assert_eq!(too_long["errors"][0]["message"], "the range must be 1 to 93 days");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn a_bulk_change_is_previewed_and_a_stay_quoted_over_graphql(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let preview = "query ($p: UUID!, $plan: UUID!, $from: Date!, $to: Date!) {
        bulkChangePreview(propertyId: $p, ratePlanId: $plan, from: $from, to: $to, mode: PERCENT, value: 1000) {
            total cells { date occupancy before after }
        }
    }";

    let previewed = graphql(
        &app,
        &hotel.owner,
        preview,
        json!({"p": hotel.id, "plan": hotel.bar["id"], "from": hotel.day(0), "to": hotel.day(3)}),
    )
    .await;
    let stay = |plan: &Value, from: i64, to: i64, residency: &str| {
        json!({"p": hotel.id, "type": hotel.deluxe, "plan": plan["id"], "in": hotel.day(from), "out": hotel.day(to),
               "residency": residency})
    };
    let quoted = graphql(&app, &hotel.owner, QUOTE, stay(&hotel.ota, 0, 3, "NON_RESIDENT")).await;
    let refused = graphql(&app, &hotel.owner, QUOTE, stay(&hotel.bar, 2, 3, "RESIDENT")).await;

    let preview = &previewed["data"]["bulkChangePreview"];
    assert_eq!(preview["total"], 3);
    assert_eq!(preview["cells"][0], json!({"date": hotel.day(0), "occupancy": 2, "before": 10000, "after": 11000}));
    let quote = &quoted["data"]["quote"];
    assert_eq!(quote["nights"][0], json!({"date": hotel.day(0), "room": 11500, "meal": 3750}));
    assert_eq!((quote["total"].clone(), quote["currency"].clone()), (json!(3 * (11500 + 3750)), json!("USD")));
    assert_eq!(quote["restrictionsOk"], true, "three nights over the third day meet its minimum stay of 2");
    let violations: Vec<&str> = refused["data"]["quote"]["violations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["kind"].as_str().unwrap())
        .collect();
    assert_eq!(violations, ["RESIDENCY", "MIN_STAY"], "a one-night stay over the third day is too short");
}

#[sqlx::test(migrator = "db::MIGRATOR")]
async fn every_role_reads_rates_and_another_tenant_reads_nothing(_: PgPoolOptions, opts: PgConnectOptions) {
    let app = TestApp::new(opts.clone()).await;
    let hotel = Hotel::new(&app, opts).await;
    let housekeeping = app.staff(&hotel.superuser, &hotel.owner, "hk@example.com", "housekeeping").await;
    let intruder = app.signup_owner("intruder@example.com", "Other Hotels").await;
    let grid = json!({"p": hotel.id, "plan": hotel.bar["id"], "from": hotel.day(0), "to": hotel.day(7)});
    let stay = json!({"p": hotel.id, "type": hotel.deluxe, "plan": hotel.bar["id"], "in": hotel.day(0),
                      "out": hotel.day(2), "residency": "NON_RESIDENT"});

    let by_housekeeping = graphql(&app, &housekeeping, GRID, grid.clone()).await;
    let plans_by_intruder = graphql(&app, &intruder, PLANS, json!({"p": hotel.id})).await;
    let grid_by_intruder = graphql(&app, &intruder, GRID, grid).await;
    let quote_by_intruder = graphql(&app, &intruder, QUOTE, stay).await;

    assert_eq!(by_housekeeping["data"]["rateGrid"]["prices"].as_array().map(Vec::len), Some(7));
    assert_eq!(plans_by_intruder["data"]["ratePlans"], json!([]));
    assert_eq!(plans_by_intruder["data"]["mealSupplements"], json!([]));
    assert_eq!(grid_by_intruder["data"]["rateGrid"], json!({"prices": [], "restrictions": []}));
    assert_eq!(quote_by_intruder["errors"][0]["message"], "rate plan not found");
}
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `DATABASE_URL=$TEST_DATABASE_URL cargo test -p core-api --test rate_reads`

Expected: the 3 tests FAIL: the fields do not exist yet, so `data` is null.

- [ ] **Step 3: Implement**

Modify `crates/core-api/src/graphql.rs`:

```diff
diff --git a/crates/core-api/src/graphql.rs b/crates/core-api/src/graphql.rs
index e7e48ee..bb920d7 100644
--- a/crates/core-api/src/graphql.rs
+++ b/crates/core-api/src/graphql.rs
@@ -179,6 +179,226 @@ pub struct InventoryDayNode {
     pub available: i32,
 }
 
+/// A GraphQL enum mirroring one of the rates module's enums, with conversions both ways.
+macro_rules! mirror_enum {
+    ($(#[$meta:meta])* $node:ident as $name:literal from $source:path { $($variant:ident),+ $(,)? }) => {
+        $(#[$meta])*
+        #[derive(Enum, Clone, Copy, PartialEq, Eq)]
+        #[graphql(name = $name)]
+        pub enum $node {
+            $($variant),+
+        }
+
+        impl From<$source> for $node {
+            fn from(value: $source) -> Self {
+                match value {
+                    $(<$source>::$variant => $node::$variant),+
+                }
+            }
+        }
+
+        impl From<$node> for $source {
+            fn from(value: $node) -> Self {
+                match value {
+                    $($node::$variant => <$source>::$variant),+
+                }
+            }
+        }
+    };
+}
+
+mirror_enum!(PlanKindNode as "PlanKind" from rates::PlanKind { Standard, Derived, Custom });
+mirror_enum!(SegmentNode as "Segment" from rates::Segment { FitF, FitL, Ota, Ta, Ibe });
+mirror_enum!(ResidencyNode as "Residency" from rates::Residency { Resident, NonResident });
+mirror_enum!(ChangeModeNode as "ChangeMode" from rates::ChangeMode { Percent, Amount });
+mirror_enum!(MealPlanNode as "MealPlan" from rates::MealPlan { Ro, Bb, Hb, Fb });
+mirror_enum!(PriceChangeModeNode as "PriceChangeMode" from rates::PriceChangeMode { Percent, Amount, Set });
+mirror_enum!(PenaltyKindNode as "PenaltyKind" from rates::PenaltyKind { Nights, Percent, Amount });
+mirror_enum!(ViolationKindNode as "ViolationKind" from rates::ViolationKind {
+    Inactive,
+    Residency,
+    RoomTypeNotSold,
+    Occupancy,
+    MealPlanNotAllowed,
+    NoPrice,
+    NoMealSupplement,
+    Closed,
+    MinStay,
+    MaxStay,
+    ClosedToArrival,
+    ClosedToDeparture,
+});
+
+/// A rate plan. Amounts are minor units in its currency.
+#[derive(SimpleObject)]
+pub struct RatePlanNode {
+    pub id: Uuid,
+    pub code: String,
+    pub name: String,
+    pub kind: PlanKindNode,
+    pub segment: SegmentNode,
+    /// `null`: any guest.
+    pub residency: Option<ResidencyNode>,
+    pub currency: String,
+    pub parent_id: Option<Uuid>,
+    /// Levels of plans above this one; the list is in tree order, so this indents it.
+    pub depth: i32,
+    pub derive_mode: Option<ChangeModeNode>,
+    pub derive_value: Option<i64>,
+    pub rounding_step: i64,
+    pub extra_adult_amount: i64,
+    pub inherit_restrictions: bool,
+    pub allowed_meal_plans: Vec<MealPlanNode>,
+    pub cancellation_policy_id: Option<Uuid>,
+    pub room_type_ids: Vec<Uuid>,
+    pub active: bool,
+    pub version: i32,
+}
+
+impl From<rates::RatePlan> for RatePlanNode {
+    fn from(p: rates::RatePlan) -> Self {
+        Self {
+            id: p.id,
+            code: p.code,
+            name: p.name,
+            kind: p.kind.into(),
+            segment: p.segment.into(),
+            residency: p.residency.map(Into::into),
+            currency: p.currency,
+            parent_id: p.parent_id,
+            depth: p.depth,
+            derive_mode: p.derive_mode.map(Into::into),
+            derive_value: p.derive_value,
+            rounding_step: p.rounding_step,
+            extra_adult_amount: p.extra_adult_amount,
+            inherit_restrictions: p.inherit_restrictions,
+            allowed_meal_plans: p.allowed_meal_plans.into_iter().map(Into::into).collect(),
+            cancellation_policy_id: p.cancellation_policy_id,
+            room_type_ids: p.room_type_ids,
+            active: p.active,
+            version: p.version,
+        }
+    }
+}
+
+/// A price for `occupancy` adults, in minor units of the plan's currency.
+#[derive(SimpleObject)]
+pub struct RatePriceNode {
+    pub room_type_id: Uuid,
+    pub date: Date,
+    pub occupancy: i32,
+    pub amount: i64,
+}
+
+#[derive(SimpleObject)]
+pub struct RestrictionNode {
+    pub room_type_id: Uuid,
+    pub date: Date,
+    pub closed: bool,
+    pub min_stay: Option<i32>,
+    pub max_stay: Option<i32>,
+    pub closed_to_arrival: bool,
+    pub closed_to_departure: bool,
+}
+
+/// A plan's resolved prices and restrictions for a date range. Days without a row have none.
+#[derive(SimpleObject)]
+pub struct RateGridNode {
+    pub prices: Vec<RatePriceNode>,
+    pub restrictions: Vec<RestrictionNode>,
+}
+
+/// One price a bulk change would change: `before` is `null` where `SET` adds a price.
+#[derive(SimpleObject)]
+pub struct PriceChangeCellNode {
+    pub room_type_id: Uuid,
+    pub date: Date,
+    pub occupancy: i32,
+    pub before: Option<i64>,
+    pub after: i64,
+}
+
+#[derive(SimpleObject)]
+pub struct BulkPreviewNode {
+    /// How many of the plan's prices would change.
+    pub total: i64,
+    /// The first 50, by date, room type and occupancy.
+    pub cells: Vec<PriceChangeCellNode>,
+}
+
+/// Per person per night on top of the room price, for the nights from `from` until `to` (`null`: open-ended).
+#[derive(SimpleObject)]
+pub struct MealSupplementNode {
+    pub id: Uuid,
+    pub meal_plan: MealPlanNode,
+    pub currency: String,
+    pub adult_amount: i64,
+    pub child_amount: i64,
+    pub from: Date,
+    pub to: Option<Date>,
+    pub version: i32,
+}
+
+#[derive(SimpleObject)]
+pub struct PenaltyNode {
+    pub kind: PenaltyKindNode,
+    pub value: i64,
+}
+
+impl From<rates::Penalty> for PenaltyNode {
+    fn from(penalty: rates::Penalty) -> Self {
+        Self { kind: penalty.kind.into(), value: penalty.value }
+    }
+}
+
+#[derive(SimpleObject)]
+pub struct CancellationRuleNode {
+    pub days_before_arrival: i32,
+    pub penalty: PenaltyNode,
+}
+
+#[derive(SimpleObject)]
+pub struct CancellationPolicyNode {
+    pub id: Uuid,
+    pub name: String,
+    /// Furthest from arrival first.
+    pub rules: Vec<CancellationRuleNode>,
+    pub no_show: PenaltyNode,
+    pub version: i32,
+}
+
+#[derive(SimpleObject)]
+pub struct QuoteNightNode {
+    pub date: Date,
+    pub room: i64,
+    pub meal: i64,
+}
+
+#[derive(SimpleObject)]
+pub struct ViolationNode {
+    pub kind: ViolationKindNode,
+    pub date: Option<Date>,
+    pub message: String,
+}
+
+/// What a stay costs, in minor units of `currency`. `restrictionsOk` is true when nothing stops the sale.
+#[derive(SimpleObject)]
+pub struct QuoteNode {
+    pub nights: Vec<QuoteNightNode>,
+    pub total: i64,
+    pub currency: String,
+    pub restrictions_ok: bool,
+    pub violations: Vec<ViolationNode>,
+}
+
+/// A rates rule the query broke, as a GraphQL error; database errors stay hidden.
+fn rates_error(err: rates::RatesError) -> async_graphql::Error {
+    match err {
+        rates::RatesError::Database(db_err) => internal(db_err),
+        other => async_graphql::Error::new(other.to_string()),
+    }
+}
+
 /// Checks `permission` for `property` and opens a transaction in the caller's tenant.
 async fn scoped(ctx: &Context<'_>, permission: Permission, property: Uuid) -> async_graphql::Result<Tx> {
     let pool = ctx.data::<PgPool>()?;
@@ -325,6 +545,202 @@ impl Query {
             })
             .collect())
     }
+
+    /// The property's rate plans in tree order: each standard or custom plan, by code, followed by the plans
+    /// derived from it, depth first.
+    async fn rate_plans(&self, ctx: &Context<'_>, property_id: Uuid) -> async_graphql::Result<Vec<RatePlanNode>> {
+        let mut tx = scoped(ctx, Permission::RatesView, property_id).await?;
+        let plans = rates::list_rate_plans(&mut tx, property_id).await.map_err(internal)?;
+        tx.commit().await.map_err(internal)?;
+        Ok(plans.into_iter().map(RatePlanNode::from).collect())
+    }
+
+    /// A plan's prices and restrictions for `[from, to)` (at most 93 days), by date, room type and occupancy.
+    async fn rate_grid(
+        &self,
+        ctx: &Context<'_>,
+        property_id: Uuid,
+        rate_plan_id: Uuid,
+        from: Date,
+        to: Date,
+    ) -> async_graphql::Result<RateGridNode> {
+        check_range(from, to, 93)?;
+        let mut tx = scoped(ctx, Permission::RatesView, property_id).await?;
+        let prices = rates::list_prices(&mut tx, property_id, rate_plan_id, from, to).await.map_err(internal)?;
+        let restrictions =
+            rates::list_restrictions(&mut tx, property_id, rate_plan_id, from, to).await.map_err(internal)?;
+        tx.commit().await.map_err(internal)?;
+        Ok(RateGridNode {
+            prices: prices
+                .into_iter()
+                .map(|p| RatePriceNode {
+                    room_type_id: p.room_type_id,
+                    date: p.date,
+                    occupancy: p.occupancy,
+                    amount: p.amount,
+                })
+                .collect(),
+            restrictions: restrictions
+                .into_iter()
+                .map(|r| RestrictionNode {
+                    room_type_id: r.room_type_id,
+                    date: r.date,
+                    closed: r.closed,
+                    min_stay: r.min_stay,
+                    max_stay: r.max_stay,
+                    closed_to_arrival: r.closed_to_arrival,
+                    closed_to_departure: r.closed_to_departure,
+                })
+                .collect(),
+        })
+    }
+
+    /// What a bulk change would do to a standard or custom plan's prices on `[from, to)`, without doing it.
+    /// Left out, `weekdays` (ISO: 1 = Monday), `roomTypeIds` and `occupancies` mean all.
+    #[allow(clippy::too_many_arguments)]
+    async fn bulk_change_preview(
+        &self,
+        ctx: &Context<'_>,
+        property_id: Uuid,
+        rate_plan_id: Uuid,
+        from: Date,
+        to: Date,
+        #[graphql(default)] weekdays: Vec<u8>,
+        #[graphql(default)] room_type_ids: Vec<Uuid>,
+        #[graphql(default)] occupancies: Vec<i32>,
+        mode: PriceChangeModeNode,
+        value: i64,
+    ) -> async_graphql::Result<BulkPreviewNode> {
+        if weekdays.iter().any(|day| !(1..=7).contains(day)) {
+            return Err(async_graphql::Error::new("weekdays are 1 (Monday) to 7 (Sunday)"));
+        }
+        let change = rates::BulkChange {
+            from,
+            to,
+            weekdays: weekdays.iter().map(|day| time::Weekday::Monday.nth_next(day - 1)).collect(),
+            room_type_ids,
+            occupancies,
+            change: rates::PriceChange { mode: mode.into(), value },
+        };
+        let mut tx = scoped(ctx, Permission::RatesView, property_id).await?;
+        let preview =
+            rates::preview_bulk_change(&mut tx, property_id, rate_plan_id, &change, 50).await.map_err(rates_error)?;
+        tx.commit().await.map_err(internal)?;
+        Ok(BulkPreviewNode {
+            total: preview.total,
+            cells: preview
+                .cells
+                .into_iter()
+                .map(|c| PriceChangeCellNode {
+                    room_type_id: c.room_type_id,
+                    date: c.date,
+                    occupancy: c.occupancy,
+                    before: c.before,
+                    after: c.after,
+                })
+                .collect(),
+        })
+    }
+
+    /// Meal supplements by currency, meal plan and start.
+    async fn meal_supplements(
+        &self,
+        ctx: &Context<'_>,
+        property_id: Uuid,
+    ) -> async_graphql::Result<Vec<MealSupplementNode>> {
+        let mut tx = scoped(ctx, Permission::RatesView, property_id).await?;
+        let supplements = rates::list_meal_supplements(&mut tx, property_id).await.map_err(internal)?;
+        tx.commit().await.map_err(internal)?;
+        Ok(supplements
+            .into_iter()
+            .map(|s| MealSupplementNode {
+                id: s.id,
+                meal_plan: s.meal_plan.into(),
+                currency: s.currency,
+                adult_amount: s.adult_amount,
+                child_amount: s.child_amount,
+                from: s.from,
+                to: s.to,
+                version: s.version,
+            })
+            .collect())
+    }
+
+    /// Cancellation policies by name.
+    async fn cancellation_policies(
+        &self,
+        ctx: &Context<'_>,
+        property_id: Uuid,
+    ) -> async_graphql::Result<Vec<CancellationPolicyNode>> {
+        let mut tx = scoped(ctx, Permission::RatesView, property_id).await?;
+        let policies = rates::list_cancellation_policies(&mut tx, property_id).await.map_err(internal)?;
+        tx.commit().await.map_err(internal)?;
+        Ok(policies
+            .into_iter()
+            .map(|p| CancellationPolicyNode {
+                id: p.id,
+                name: p.name,
+                rules: p
+                    .rules
+                    .into_iter()
+                    .map(|rule| CancellationRuleNode {
+                        days_before_arrival: rule.days_before_arrival,
+                        penalty: rule.penalty.into(),
+                    })
+                    .collect(),
+                no_show: p.no_show.into(),
+                version: p.version,
+            })
+            .collect())
+    }
+
+    /// Prices a stay of `[checkIn, checkOut)` (at most 90 nights) and lists every reason it cannot be sold.
+    #[allow(clippy::too_many_arguments)]
+    async fn quote(
+        &self,
+        ctx: &Context<'_>,
+        property_id: Uuid,
+        room_type_id: Uuid,
+        rate_plan_id: Uuid,
+        meal_plan: MealPlanNode,
+        check_in: Date,
+        check_out: Date,
+        adults: i32,
+        children: i32,
+        residency: ResidencyNode,
+    ) -> async_graphql::Result<QuoteNode> {
+        if check_out > check_in && check_out - check_in > Duration::days(90) {
+            return Err(async_graphql::Error::new("a quote is for at most 90 nights"));
+        }
+        let request = rates::QuoteRequest {
+            room_type_id,
+            rate_plan_id,
+            meal_plan: meal_plan.into(),
+            check_in,
+            check_out,
+            adults,
+            children,
+            residency: residency.into(),
+        };
+        let mut tx = scoped(ctx, Permission::RatesView, property_id).await?;
+        let quote = rates::load_quote(&mut tx, property_id, &request).await.map_err(rates_error)?;
+        tx.commit().await.map_err(internal)?;
+        Ok(QuoteNode {
+            nights: quote
+                .nights
+                .into_iter()
+                .map(|n| QuoteNightNode { date: n.date, room: n.room, meal: n.meal })
+                .collect(),
+            total: quote.total,
+            currency: quote.currency,
+            restrictions_ok: quote.restrictions_ok,
+            violations: quote
+                .violations
+                .into_iter()
+                .map(|v| ViolationNode { kind: v.kind.into(), date: v.date, message: v.message })
+                .collect(),
+        })
+    }
 }
 
 #[cfg(test)]
```

Modify `migrations/0006_rates.sql`:

```diff
diff --git a/migrations/0006_rates.sql b/migrations/0006_rates.sql
index 13e780c..3486b80 100644
--- a/migrations/0006_rates.sql
+++ b/migrations/0006_rates.sql
@@ -4,9 +4,10 @@
 -- A price changed by `value` basis points (`percent`) or minor units (`amount`), either possibly negative,
 -- never below 0, rounded half-up to a multiple of `step`. Derived plans apply their formula to the parent's
 -- price with it, and bulk changes apply theirs to a plan's own prices. Integer arithmetic only: every input
--- is bounded by the checks below, so nothing overflows. Mirrored in web/pms/src/lib/rates.ts for previews.
+-- is bounded by the checks below, so nothing overflows. Not STRICT, so the planner inlines it into the
+-- statements that call it once per row.
 create function app.derive_amount(base bigint, mode text, value bigint, step bigint) returns bigint
-language sql immutable strict parallel safe
+language sql immutable parallel safe
 as $$
   select case mode
     when 'percent' then (2 * greatest(base * (10000 + value), 0) + 10000 * step) / (20000 * step) * step
@@ -89,6 +90,8 @@ create table rate_plan_room_type (
 
 -- Resolved prices per plan, room type, date and occupancy (adults), derived plans included: a write to a
 -- plan's prices recomputes its descendants' rows in the same transaction, so reads never derive anything.
+-- Half of each page is left free: a bulk change rewrites every price on a page, and the new versions then fit
+-- beside the old ones (HOT updates, no index writes).
 create table rate_day (
   tenant_id uuid not null,
   property_id uuid not null,
@@ -101,7 +104,7 @@ create table rate_day (
   foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade,
   foreign key (property_id, rate_plan_id, room_type_id)
     references rate_plan_room_type (property_id, rate_plan_id, room_type_id) on delete cascade
-);
+) with (fillfactor = 50);
 create index rate_day_property_date_idx on rate_day (property_id, date);
 
 -- Resolved restrictions per plan, room type and date. A derived plan that inherits restrictions gets a copy
```

Modify `modules/rates/src/plans.rs`:

```diff
diff --git a/modules/rates/src/plans.rs b/modules/rates/src/plans.rs
index af94d40..2f8c363 100644
--- a/modules/rates/src/plans.rs
+++ b/modules/rates/src/plans.rs
@@ -1,4 +1,4 @@
-use crate::prices::{derive_prices, tree_keys};
+use crate::prices::{Reprice, derive_prices, tree_keys};
 use crate::restrictions::{derive_restrictions, inheriting_levels};
 use crate::{RatesError, audit, business_date, lock_rates, notify, rate_plans_key, rates_keys, violates};
 use db::{TenantId, Tx, UserId};
@@ -453,7 +453,7 @@ pub async fn create_rate_plan(
     let mut keys = vec![rate_plans_key(property)];
     if input.kind == PlanKind::Derived {
         let end = today + Duration::days(rooms::WINDOW_DAYS);
-        derive_prices(tx, &[vec![id]], None, today, end, false).await?;
+        derive_prices(tx, &[vec![id]], None, today, end, Reprice::Added).await?;
         if input.inherit_restrictions {
             derive_restrictions(tx, &[vec![id]], None, today, end).await?;
         }
@@ -579,7 +579,8 @@ pub async fn update_rate_plan(
     let end = today + Duration::days(rooms::WINDOW_DAYS);
     if current.kind == PlanKind::Derived && (moved || reformulated || added_types) {
         let levels: Vec<Vec<Uuid>> = std::iter::once(vec![id]).chain(tree.descendant_levels(id)).collect();
-        derive_prices(tx, &levels, None, today, end, moved).await?;
+        let reprice = if moved { Reprice::Moved } else { Reprice::Added };
+        derive_prices(tx, &levels, None, today, end, reprice).await?;
         keys.extend(tree_keys(&tree, property, id, today, end));
     }
     // A plan that starts inheriting, or inherits from a new parent or for new room types, takes a fresh copy;
```

Modify `modules/rates/src/prices.rs`:

```diff
diff --git a/modules/rates/src/prices.rs b/modules/rates/src/prices.rs
index b7cfd41..0d6981a 100644
--- a/modules/rates/src/prices.rs
+++ b/modules/rates/src/prices.rs
@@ -1,5 +1,5 @@
 //! Prices: set by hand on standard and custom plans, changed in bulk, and derived down the plan tree in the
-//! same transaction, one `insert … select … on conflict do update` per level.
+//! same transaction, set-based, level by level.
 
 use crate::plans::{PlanKind, RatePlan, Tree, list_rate_plans};
 use crate::{MAX_AMOUNT, RatesError, audit, business_date, lock_rates, notify, rates_keys};
@@ -101,19 +101,31 @@ pub(crate) fn tree_keys(tree: &Tree, property: Uuid, plan: Uuid, from: Date, to:
         .collect()
 }
 
+/// Which rows repricing a level touches besides updating the prices it already has.
+#[derive(Debug, Clone, Copy, PartialEq, Eq)]
+pub(crate) enum Reprice {
+    /// The parents only changed prices they had: nothing to add.
+    Existing,
+    /// The parents may have new prices: add them below too.
+    Added,
+    /// The plan moved to another parent: add its new parent's cells and drop those the new parent lacks.
+    Moved,
+}
+
 /// Recomputes the prices of the plans in `levels` from their parents' for `room_types` (all if `None`) on
-/// `[from, to)`: each level is derived from the one before, the first from its own parents. `prune` first
-/// deletes prices whose parent price is gone, which only happens when a plan moved to another parent.
+/// `[from, to)`: each level is derived from the one before, the first from its own parents. Per level, one
+/// `update … from` rewrites the prices it has (an upsert of existing rows measured about a third slower for
+/// the bulk-change gate), and for `Added` and `Moved` one `insert … select` adds the missing ones.
 pub(crate) async fn derive_prices(
     tx: &mut Tx,
     levels: &[Vec<Uuid>],
     room_types: Option<&[Uuid]>,
     from: Date,
     to: Date,
-    prune: bool,
+    reprice: Reprice,
 ) -> Result<(), sqlx::Error> {
     for plans in levels {
-        if prune {
+        if reprice == Reprice::Moved {
             sqlx::query(
                 "delete from rate_day d using rate_plan c
                  where c.id = d.rate_plan_id and d.rate_plan_id = any($1) and d.date >= $3 and d.date < $4
@@ -130,15 +142,13 @@ pub(crate) async fn derive_prices(
             .await?;
         }
         sqlx::query(
-            "insert into rate_day (tenant_id, property_id, rate_plan_id, room_type_id, date, occupancy, amount)
-             select c.tenant_id, c.property_id, c.id, p.room_type_id, p.date, p.occupancy,
-                    app.derive_amount(p.amount, c.derive_mode, c.derive_value, c.rounding_step)
-             from rate_plan c
-             join rate_plan_room_type s on s.rate_plan_id = c.id
-             join rate_day p on p.rate_plan_id = c.parent_id and p.room_type_id = s.room_type_id
-             where c.id = any($1) and p.date >= $3 and p.date < $4 and ($2::uuid[] is null or p.room_type_id = any($2))
-             on conflict (rate_plan_id, date, room_type_id, occupancy) do update set amount = excluded.amount
-             where rate_day.amount is distinct from excluded.amount",
+            "update rate_day d set amount = app.derive_amount(p.amount, c.derive_mode, c.derive_value, c.rounding_step)
+             from rate_plan c, rate_day p
+             where c.id = any($1) and d.rate_plan_id = c.id and d.date >= $3 and d.date < $4
+               and ($2::uuid[] is null or d.room_type_id = any($2))
+               and p.rate_plan_id = c.parent_id and p.date = d.date and p.room_type_id = d.room_type_id
+               and p.occupancy = d.occupancy
+               and d.amount <> app.derive_amount(p.amount, c.derive_mode, c.derive_value, c.rounding_step)",
         )
         .bind(plans)
         .bind(room_types)
@@ -146,6 +156,25 @@ pub(crate) async fn derive_prices(
         .bind(to)
         .execute(&mut **tx)
         .await?;
+        if reprice != Reprice::Existing {
+            sqlx::query(
+                "insert into rate_day (tenant_id, property_id, rate_plan_id, room_type_id, date, occupancy, amount)
+                 select c.tenant_id, c.property_id, c.id, p.room_type_id, p.date, p.occupancy,
+                        app.derive_amount(p.amount, c.derive_mode, c.derive_value, c.rounding_step)
+                 from rate_plan c
+                 join rate_plan_room_type s on s.rate_plan_id = c.id
+                 join rate_day p on p.rate_plan_id = c.parent_id and p.room_type_id = s.room_type_id
+                 where c.id = any($1) and p.date >= $3 and p.date < $4
+                   and ($2::uuid[] is null or p.room_type_id = any($2))
+                 on conflict (rate_plan_id, date, room_type_id, occupancy) do nothing",
+            )
+            .bind(plans)
+            .bind(room_types)
+            .bind(from)
+            .bind(to)
+            .execute(&mut **tx)
+            .await?;
+        }
     }
     Ok(())
 }
@@ -200,24 +229,27 @@ pub async fn set_prices(
     let mut touched: Vec<Uuid> = cells.keys().map(|key| key.1).collect();
     touched.sort_unstable();
     touched.dedup();
-    derive_prices(tx, &tree.descendant_levels(plan.id), Some(&touched), from, to, false).await?;
+    derive_prices(tx, &tree.descendant_levels(plan.id), Some(&touched), from, to, Reprice::Added).await?;
     audit(tx, tenant, actor, "rate_plan.prices_set", "rate_plan", plan.id, serde_json::json!({ "count": cells.len() }))
         .await?;
     notify(tx, tenant, property, tree_keys(&tree, property, plan.id, from, to)).await?;
     Ok(())
 }
 
-/// The cells a bulk change on `plan` selects, with their price before and after. Binds: `$1` plan, `$2` from,
-/// `$3` to, `$4` room types, `$5` ISO weekdays, `$6` occupancies (empty: all), `$7` mode, `$8` value, `$9` the
-/// plan's rounding step.
+/// The existing prices `d` a bulk change on plan `$1` selects: `[$2, $3)`, room types `$4`, ISO weekdays `$5`,
+/// occupancies `$6` (empty: all), and that `$7` (mode) by `$8` (value), rounded to `$9`, changes.
+const EXISTING: &str = "d.rate_plan_id = $1 and d.date >= $2 and d.date < $3 and d.room_type_id = any($4)
+      and extract(isodow from d.date)::integer = any($5)
+      and (cardinality($6::integer[]) = 0 or d.occupancy = any($6))
+      and d.amount <> app.derive_amount(d.amount, $7, $8, $9)";
+
+/// The cells a bulk change on `plan` selects, with their price before and after; binds as for [`EXISTING`].
+/// `set` also selects cells without a price, which it adds.
 const CHANGES: &str = "
     select d.room_type_id, d.date, d.occupancy, d.amount as before,
            app.derive_amount(d.amount, $7, $8, $9) as after
     from rate_day d
-    where $7 <> 'set' and d.rate_plan_id = $1 and d.date >= $2 and d.date < $3 and d.room_type_id = any($4)
-      and extract(isodow from d.date)::integer = any($5)
-      and (cardinality($6::integer[]) = 0 or d.occupancy = any($6))
-      and d.amount <> app.derive_amount(d.amount, $7, $8, $9)
+    where $7 <> 'set' and {EXISTING}
     union all
     select t.id, s.day::date, o.occupancy, d.amount, $8
     from room_type t
@@ -229,6 +261,11 @@ const CHANGES: &str = "
       and (cardinality($6::integer[]) = 0 or o.occupancy = any($6))
       and d.amount is distinct from $8";
 
+/// [`CHANGES`] with [`EXISTING`] filled in.
+fn changes() -> String {
+    CHANGES.replace("{EXISTING}", EXISTING)
+}
+
 /// A bulk change, checked, with its selection resolved to the binds [`CHANGES`] takes.
 struct Selection {
     room_types: Vec<Uuid>,
@@ -279,27 +316,38 @@ pub async fn bulk_change(
     let tree = Tree(list_rate_plans(tx, property).await?);
     let plan = hand_priced(&tree, plan)?;
     let selection = select(today, plan, change)?;
-    let changed = sqlx::query(sqlx::AssertSqlSafe(format!(
-        "insert into rate_day (tenant_id, property_id, rate_plan_id, room_type_id, date, occupancy, amount)
-         select $10, $11, $1, c.room_type_id, c.date, c.occupancy, c.after from ({CHANGES}) c
-         on conflict (rate_plan_id, date, room_type_id, occupancy) do update set amount = excluded.amount"
-    )))
-    .bind(plan.id)
-    .bind(change.from)
-    .bind(change.to)
-    .bind(&selection.room_types)
-    .bind(&selection.weekdays)
-    .bind(&change.occupancies)
-    .bind(change.change.mode.as_str())
-    .bind(change.change.value)
-    .bind(plan.rounding_step)
-    .bind(tenant.0)
-    .bind(property)
-    .execute(&mut **tx)
-    .await?
-    .rows_affected();
-    derive_prices(tx, &tree.descendant_levels(plan.id), Some(&selection.room_types), change.from, change.to, false)
-        .await?;
+    // `set` may add prices; `percent` and `amount` only change existing ones, so they update in place.
+    let apply = if change.change.mode == PriceChangeMode::Set {
+        format!(
+            "insert into rate_day (tenant_id, property_id, rate_plan_id, room_type_id, date, occupancy, amount)
+             select $10, $11, $1, c.room_type_id, c.date, c.occupancy, c.after from ({}) c
+             on conflict (rate_plan_id, date, room_type_id, occupancy) do update set amount = excluded.amount",
+            changes()
+        )
+    } else {
+        format!(
+            "update rate_day d set amount = app.derive_amount(d.amount, $7, $8, $9)
+             where d.tenant_id = $10 and d.property_id = $11 and {EXISTING}"
+        )
+    };
+    let changed = sqlx::query(sqlx::AssertSqlSafe(apply))
+        .bind(plan.id)
+        .bind(change.from)
+        .bind(change.to)
+        .bind(&selection.room_types)
+        .bind(&selection.weekdays)
+        .bind(&change.occupancies)
+        .bind(change.change.mode.as_str())
+        .bind(change.change.value)
+        .bind(plan.rounding_step)
+        .bind(tenant.0)
+        .bind(property)
+        .execute(&mut **tx)
+        .await?
+        .rows_affected();
+    let reprice = if change.change.mode == PriceChangeMode::Set { Reprice::Added } else { Reprice::Existing };
+    let levels = tree.descendant_levels(plan.id);
+    derive_prices(tx, &levels, Some(&selection.room_types), change.from, change.to, reprice).await?;
     audit(
         tx,
         tenant,
@@ -332,9 +380,10 @@ pub async fn preview_bulk_change(
     let selection = select(today, plan, change)?;
     let rows: Vec<(Uuid, Date, i32, Option<i64>, i64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
         "select c.room_type_id, c.date, c.occupancy, c.before, c.after, count(*) over () as total
-         from ({CHANGES}) c join room_type rt on rt.id = c.room_type_id
+         from ({}) c join room_type rt on rt.id = c.room_type_id
          order by c.date, rt.sort_order, rt.code, c.occupancy
-         limit $10"
+         limit $10",
+        changes()
     )))
     .bind(plan.id)
     .bind(change.from)
```

- [ ] **Step 4: Run the checks**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
DATABASE_URL=$TEST_DATABASE_URL cargo test --release -p core-api --test perf -- --ignored --nocapture --test-threads=1
cd web/pms
bun run api:schemas && bun run codegen && git diff --exit-code -- src/lib/api
bun run lint && bun run check && bun run test && bun run build
```

Expected: every test passes; the three gates print their timings and pass (see the Verified section for the margins on the bulk gate); regenerate and commit `schema.graphql` and `gql/`.

- [ ] **Step 5: Commit**

```sh
git add -A
git commit -m "feat(graphql): rate plans, the rate grid, bulk change previews, meal supplements, cancellation policies and quotes, with performance gates"
```

### Task 12: Rooms page busy state, DateGrid snap-back, room type form reset

The Phase 1 UI follow-ups. `Pending` tracks commands in flight by key, so the rooms page disables only the
row, the order buttons or the form a command changes. `DateGrid` kept its chosen cell while showing a clamped
one when rows went away, and jumped back when they returned; it now commits the clamped cell. The new e2e test
also exposed a race on the room types page: the form was cleared only after the refetch that shows the new
type, wiping a second type typed quickly; it is now cleared as soon as the create succeeds.

**Files:**
- Modify: `web/pms/src/lib/components/DateGrid.svelte`
- Test: `web/pms/src/lib/pending.spec.ts` (new)
- Create: `web/pms/src/lib/pending.svelte.ts`
- Modify: `web/pms/src/routes/(app)/p/[property]/room-types/+page.svelte`
- Modify: `web/pms/src/routes/(app)/p/[property]/rooms/+page.svelte`
- Test: `web/pms/tests/e2e/inventory.spec.ts`

**Interfaces:**
- Produces: `Pending` (`has(key)`, `run(key, command)`) in `$lib/pending.svelte`, used again by the rates screens.

- [ ] **Step 1: Write the failing tests**

Create `web/pms/src/lib/pending.spec.ts`:

```ts
import { describe, expect, it } from 'vitest';
import { Pending } from './pending.svelte';

describe('Pending', () => {
	it('marks a key as running until its command settles', async () => {
		const pending = new Pending();
		let finish = () => {};
		const running = pending.run('room-1', () => new Promise<void>((resolve) => (finish = resolve)));

		expect(pending.has('room-1')).toBe(true);
		expect(pending.has('room-2')).toBe(false);
		finish();
		await running;
		expect(pending.has('room-1')).toBe(false);
	});

	it('clears the key when the command fails, and passes the failure on', async () => {
		const pending = new Pending();

		await expect(pending.run('order', () => Promise.reject(new Error('refused')))).rejects.toThrow(
			'refused'
		);
		expect(pending.has('order')).toBe(false);
	});

	it('keeps other keys running while one finishes', async () => {
		const pending = new Pending();
		let finishFirst = () => {};
		const first = pending.run('a', () => new Promise<void>((resolve) => (finishFirst = resolve)));
		const second = pending.run('b', () => new Promise<never>(() => {}));

		finishFirst();
		await first;

		expect([pending.has('a'), pending.has('b')]).toEqual([false, true]);
		void second;
	});
});
```

Modify `web/pms/tests/e2e/inventory.spec.ts`:

```diff
diff --git a/web/pms/tests/e2e/inventory.spec.ts b/web/pms/tests/e2e/inventory.spec.ts
index a79af2a..d3a412c 100644
--- a/web/pms/tests/e2e/inventory.spec.ts
+++ b/web/pms/tests/e2e/inventory.spec.ts
@@ -76,3 +76,34 @@ test('blocking a room reduces availability on the calendar until it is released'
 
 	await expect(grid.getByRole('gridcell', { name: `DLX ${today}: 5 available` })).toBeVisible();
 });
+
+test('the active cell stays on its row when room types are retired and restored', async ({
+	page,
+	context
+}) => {
+	await signUp(page);
+	await createProperty(page, 'GAL');
+	await page.getByRole('link', { name: 'Room types' }).click();
+	for (const code of ['AAA', 'BBB', 'CCC']) await addRoomType(page, code, `Type ${code}`);
+	const settings = await context.newPage();
+	await settings.goto(page.url());
+
+	await page.getByRole('link', { name: 'Inventory' }).click();
+	const today = (await page.getByTestId('business-date').textContent())!.trim();
+	const grid = page.getByRole('grid', { name: 'Availability' });
+	const cell = (code: string) => grid.getByRole('gridcell', { name: `${code} ${today}:` });
+	await cell('CCC').click();
+	await expect(grid).toHaveAttribute(
+		'aria-activedescendant',
+		(await cell('CCC').getAttribute('id'))!
+	);
+
+	// Retiring the last type moves the active cell up a row; restoring it must not move it back.
+	await settings.getByRole('button', { name: 'Deactivate CCC' }).click();
+	await expect(cell('CCC')).toHaveCount(0);
+	const bbb = (await cell('BBB').getAttribute('id'))!;
+	await expect(grid).toHaveAttribute('aria-activedescendant', bbb);
+	await settings.getByRole('button', { name: 'Activate CCC' }).click();
+	await expect(cell('CCC')).toBeVisible();
+	await expect(grid).toHaveAttribute('aria-activedescendant', bbb);
+});
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `cd web/pms && bun run test && bun run test:e2e inventory`

Expected: `pending.spec.ts` fails (`Cannot find module './pending.svelte'`); the new e2e test fails at its last step (`Expected: "…-1-…" Received: "…-2-…"`: the active cell went back to row 2).

- [ ] **Step 3: Implement**

Modify `web/pms/src/lib/components/DateGrid.svelte`:

```diff
diff --git a/web/pms/src/lib/components/DateGrid.svelte b/web/pms/src/lib/components/DateGrid.svelte
index c862aa6..9d47ebd 100644
--- a/web/pms/src/lib/components/DateGrid.svelte
+++ b/web/pms/src/lib/components/DateGrid.svelte
@@ -55,6 +55,13 @@
 	// The chosen cell, kept inside the grid when rows or columns go away (a room type is retired), so
 	// `aria-activedescendant` always names a rendered cell.
 	const active = $derived(clampCell(chosen, { rows: rows.length, columns: columns.length }));
+	// Once the grid shrinks, the clamped cell becomes the chosen one, so the cell does not jump back to its
+	// old row when the rows come back.
+	$effect.pre(() => {
+		if (rows.length > 0 && (active.row !== chosen.row || active.column !== chosen.column)) {
+			chosen = active;
+		}
+	});
 
 	// Start on `initialColumn`, scrolled to the left edge, whenever the columns change (a new month).
 	$effect(() => {
```

Create `web/pms/src/lib/pending.svelte.ts`:

```ts
import { SvelteSet } from 'svelte/reactivity';

/**
 * Commands in flight, by key (a row's id, a form's name), so a screen disables only what a running
 * command affects instead of the whole page.
 */
export class Pending {
	#keys = new SvelteSet<string>();

	has(key: string): boolean {
		return this.#keys.has(key);
	}

	/** Runs `command` with `key` marked as running until it settles. */
	async run<T>(key: string, command: () => Promise<T>): Promise<T> {
		this.#keys.add(key);
		try {
			return await command();
		} finally {
			this.#keys.delete(key);
		}
	}
}
```

Modify `web/pms/src/routes/(app)/p/[property]/room-types/+page.svelte`:

```diff
diff --git a/web/pms/src/routes/(app)/p/[property]/room-types/+page.svelte b/web/pms/src/routes/(app)/p/[property]/room-types/+page.svelte
index 4fefb2a..fe0a5af 100644
--- a/web/pms/src/routes/(app)/p/[property]/room-types/+page.svelte
+++ b/web/pms/src/routes/(app)/p/[property]/room-types/+page.svelte
@@ -62,7 +62,6 @@
 	async function create(event: SubmitEvent) {
 		event.preventDefault();
 		const body = { code: draft.code.toUpperCase(), name: draft.name, ...capacity(draft) };
-		let succeeded = false;
 		await run(async () => {
 			unwrap(
 				await rest.POST('/api/v1/properties/{property}/room-types', {
@@ -73,12 +72,10 @@
 					body
 				})
 			);
-			succeeded = true;
-		}, createForm.failed);
-		if (succeeded) {
+			// Cleared before the refetch shows the new type, so the next one can be typed at once.
 			draft = emptyDraft();
 			createForm.reset();
-		}
+		}, createForm.failed);
 	}
 
 	function update(type: RoomType, body: { name?: string; active?: boolean } & object) {
```

Modify `web/pms/src/routes/(app)/p/[property]/rooms/+page.svelte`:

```diff
diff --git a/web/pms/src/routes/(app)/p/[property]/rooms/+page.svelte b/web/pms/src/routes/(app)/p/[property]/rooms/+page.svelte
index 9e7e6ab..4adb259 100644
--- a/web/pms/src/routes/(app)/p/[property]/rooms/+page.svelte
+++ b/web/pms/src/routes/(app)/p/[property]/rooms/+page.svelte
@@ -3,6 +3,7 @@
 	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
 	import { errorMessage } from '$lib/api/problem';
 	import { formKeys, ifMatch, rest, unwrap } from '$lib/api/rest';
+	import { Pending } from '$lib/pending.svelte';
 	import {
 		fetchRooms,
 		fetchRoomTypes,
@@ -35,7 +36,8 @@
 	const groups = $derived(groupRooms(rooms.data?.rooms ?? [], roomTypes.data ?? [], groupBy));
 
 	let error = $state('');
-	let busy = $state(false);
+	// Only the row, list or form a command changes is disabled while it runs.
+	const pending = new Pending();
 	let bulkDialog = $state<HTMLDialogElement>();
 	let bulk = $state({
 		roomTypeId: '',
@@ -52,15 +54,18 @@
 	const singleForm = formKeys();
 	const sectionForm = formKeys();
 
-	/** Runs a command; shows its problem if it fails, and refetches after a version conflict. */
+	/**
+	 * Runs a command under `key` (a room's id, `order`, or a form's name); shows its problem if it fails,
+	 * and refetches either way.
+	 */
 	async function run(
+		key: string,
 		command: () => Promise<unknown>,
 		onError?: (err: unknown) => void
 	): Promise<boolean> {
-		busy = true;
 		error = '';
 		try {
-			await command();
+			await pending.run(key, command);
 			await client.invalidateQueries({ queryKey: roomsKey(propertyId) });
 			return true;
 		} catch (err) {
@@ -68,8 +73,6 @@
 			onError?.(err);
 			await client.invalidateQueries({ queryKey: roomsKey(propertyId) });
 			return false;
-		} finally {
-			busy = false;
 		}
 	}
 
@@ -90,6 +93,7 @@
 			section_id: bulk.sectionId || null
 		};
 		const added = await run(
+			'bulk',
 			async () =>
 				unwrap(
 					await rest.POST('/api/v1/properties/{property}/rooms/bulk', {
@@ -116,6 +120,7 @@
 			floor: single.floor || null
 		};
 		const added = await run(
+			'room',
 			async () =>
 				unwrap(
 					await rest.POST('/api/v1/properties/{property}/rooms', {
@@ -138,6 +143,7 @@
 		event.preventDefault();
 		const body = { name: sectionName };
 		const added = await run(
+			'section',
 			async () =>
 				unwrap(
 					await rest.POST('/api/v1/properties/{property}/sections', {
@@ -165,7 +171,7 @@
 			active?: boolean;
 		}
 	) {
-		return run(async () =>
+		return run(room.id, async () =>
 			unwrap(
 				await rest.PATCH('/api/v1/properties/{property}/rooms/{room}', {
 					params: { path: { property: propertyId, room: room.id }, header: ifMatch(room.version) },
@@ -181,7 +187,7 @@
 		const from = all.findIndex((r) => r.id === room.id);
 		const moved = moveItem(all, from, from + delta);
 		if (moved.every((r, index) => r.id === all[index].id)) return;
-		return run(async () =>
+		return run('order', async () =>
 			unwrap(
 				await rest.PUT('/api/v1/properties/{property}/rooms/order', {
 					params: { path: { property: propertyId } },
@@ -207,7 +213,9 @@
 			</select>
 		</label>
 		{#if manage}
-			<button disabled={busy || activeTypes.length === 0} onclick={openBulk}>Add rooms…</button>
+			<button disabled={pending.has('bulk') || activeTypes.length === 0} onclick={openBulk}
+				>Add rooms…</button
+			>
 		{/if}
 	</div>
 	{#if activeTypes.length === 0}
@@ -236,7 +244,7 @@
 								<select
 									aria-label="Type of room {room.number}"
 									value={room.roomTypeId}
-									disabled={busy}
+									disabled={pending.has(room.id)}
 									onchange={(event) => update(room, { room_type_id: event.currentTarget.value })}
 								>
 									{#each roomTypes.data as type (type.id)}
@@ -248,7 +256,7 @@
 								<input
 									aria-label="Floor of room {room.number}"
 									value={room.floor ?? ''}
-									disabled={busy}
+									disabled={pending.has(room.id)}
 									onchange={(event) => update(room, { floor: event.currentTarget.value || null })}
 								/>
 							</td>
@@ -256,7 +264,7 @@
 								<select
 									aria-label="Section of room {room.number}"
 									value={room.sectionId ?? ''}
-									disabled={busy}
+									disabled={pending.has(room.id)}
 									onchange={(event) =>
 										update(room, { section_id: event.currentTarget.value || null })}
 								>
@@ -277,18 +285,18 @@
 								<button
 									class="secondary"
 									aria-label="Move room {room.number} up"
-									disabled={busy}
+									disabled={pending.has('order')}
 									onclick={() => move(room, -1)}>↑</button
 								>
 								<button
 									class="secondary"
 									aria-label="Move room {room.number} down"
-									disabled={busy}
+									disabled={pending.has('order')}
 									onclick={() => move(room, 1)}>↓</button
 								>
 								<button
 									class="secondary"
-									disabled={busy}
+									disabled={pending.has(room.id)}
 									aria-label="{room.active ? 'Deactivate' : 'Activate'} room {room.number}"
 									onclick={() => update(room, { active: !room.active })}
 									>{room.active ? 'Deactivate' : 'Activate'}</button
@@ -316,14 +324,14 @@
 				</select>
 			</label>
 			<label>Floor <input maxlength="20" bind:value={single.floor} /></label>
-			<button disabled={busy}>Add room</button>
+			<button disabled={pending.has('room')}>Add room</button>
 		</form>
 
 		<h2>Housekeeping sections</h2>
 		<p>{rooms.data.sections.map((section) => section.name).join(', ') || 'None yet.'}</p>
 		<form class="inline-form" aria-label="New section" onsubmit={addSection}>
 			<label>Name <input required maxlength="100" bind:value={sectionName} /></label>
-			<button disabled={busy}>Add section</button>
+			<button disabled={pending.has('section')}>Add section</button>
 		</form>
 	{/if}
 {:else}
@@ -359,7 +367,8 @@
 		{/if}
 		{#if error}<p class="error" role="alert">{error}</p>{/if}
 		<div class="actions">
-			<button disabled={busy || bulkNumbers.length === 0 || bulkNumbers.length > MAX_RANGE}
+			<button
+				disabled={pending.has('bulk') || bulkNumbers.length === 0 || bulkNumbers.length > MAX_RANGE}
 				>Add {bulkNumbers.length} rooms</button
 			>
 			<button type="button" class="secondary" onclick={() => bulkDialog?.close()}>Cancel</button>
```

- [ ] **Step 4: Run the checks**

```sh
cd web/pms
bun run api:schemas && bun run codegen && git diff --exit-code -- src/lib/api
bun run lint && bun run check && bun run test && bun run build
bun run test:e2e
```

Expected: 51 unit tests; 5 Playwright tests pass.

- [ ] **Step 5: Commit**

```sh
git add -A
git commit -m "fix(web): disable only the row or form a rooms command changes, and keep the grid's active cell from jumping back when rows return"
```

### Task 13: Rates data layer

GraphQL documents and fetchers, query keys that match the server's events, money formatting and parsing by
currency (string arithmetic, half-up), grid rows (room type × adults, then restrictions), a price index, the
formula text of a derived plan, restriction summaries, and the batcher that saves grid edits together.
`can(profile, 'manageRates', property)` mirrors `RatesManage`.

**Files:**
- Test: `web/pms/src/lib/rates.spec.ts` (new)
- Create: `web/pms/src/lib/rates.ts`
- Test: `web/pms/src/lib/session.spec.ts`
- Modify: `web/pms/src/lib/session.ts`

**Interfaces:**
- Produces (in `$lib/rates`): `RatePlansDocument`, `RateGridDocument`, `BulkPreviewDocument`, `QuoteDocument`; types `RatePlan`, `MealSupplement`, `RateGrid`, `Restriction`, `RateRow`; `ratePlansKey(p)`, `ratesKey(p, plan, month)`, `fetchRatePlans`, `fetchRateMonth`, `formatMoney(amount, currency)`, `parseMoney(text, currency)`, `rateRows(plan, roomTypes)`, `indexRates(grid)`, `formula(plan, parent)`, `restrictionSummary(r)`, `batcher(key, save, delay, onError)`; `manageRates` in `$lib/session`.

- [ ] **Step 1: Write the failing tests**

Create `web/pms/src/lib/rates.spec.ts`:

```ts
import { afterEach, describe, expect, it, vi } from 'vitest';
import {
	batcher,
	formatMoney,
	formula,
	indexRates,
	parseMoney,
	ratePlansKey,
	rateRows,
	ratesKey,
	restrictionSummary,
	type RatePlan
} from './rates';

const plan = (overrides: Partial<RatePlan> = {}): RatePlan => ({
	id: 'bar',
	code: 'BAR',
	name: 'Best available',
	kind: 'STANDARD',
	segment: 'IBE',
	residency: null,
	currency: 'USD',
	parentId: null,
	depth: 0,
	deriveMode: null,
	deriveValue: null,
	roundingStep: 100,
	extraAdultAmount: 0,
	inheritRestrictions: false,
	allowedMealPlans: ['RO'],
	cancellationPolicyId: null,
	roomTypeIds: ['dlx'],
	active: true,
	version: 1,
	...overrides
});

describe('keys', () => {
	it('match the server events', () => {
		expect(ratePlansKey('p1')).toEqual(['rate-plans:p1']);
		expect(ratesKey('p1', 'bar', '2026-07')).toEqual(['rates:p1:bar:2026-07']);
	});
});

describe('money', () => {
	it('formats minor units with the currency’s decimals', () => {
		expect(formatMoney(15050, 'USD')).toBe('150.50');
		expect(formatMoney(4500000, 'LKR')).toBe('45,000.00');
		expect(formatMoney(1500, 'JPY')).toBe('1,500');
	});

	it('parses what a person types into minor units', () => {
		expect(parseMoney('150.5', 'USD')).toBe(15050);
		expect(parseMoney(' 45,000 ', 'LKR')).toBe(4500000);
		expect(parseMoney('0.005', 'USD')).toBe(1);
		expect(parseMoney('1500', 'JPY')).toBe(1500);
		expect(parseMoney('', 'USD')).toBeNull();
		expect(parseMoney('-5', 'USD')).toBeNull();
		expect(parseMoney('abc', 'USD')).toBeNull();
	});
});

describe('rateRows', () => {
	it('lists each sold room type by occupancy, then its restrictions', () => {
		const types = [
			{ id: 'std', code: 'STD', name: 'Standard', maxAdults: 1, active: true },
			{ id: 'dlx', code: 'DLX', name: 'Deluxe', maxAdults: 2, active: true }
		];

		const rows = rateRows(plan({ roomTypeIds: ['dlx'] }), types);

		expect(rows.map((row) => row.label)).toEqual([
			'DLX · 1 adult',
			'DLX · 2 adults',
			'DLX · restrictions'
		]);
		expect(rows[1]).toMatchObject({ id: 'dlx:2', roomTypeId: 'dlx', occupancy: 2, code: 'DLX' });
		expect(rows[2].occupancy).toBeNull();
	});
});

describe('indexRates', () => {
	it('finds a price and a day’s restrictions', () => {
		const index = indexRates({
			prices: [{ roomTypeId: 'dlx', date: '2026-07-04', occupancy: 2, amount: 12000 }],
			restrictions: [
				{
					roomTypeId: 'dlx',
					date: '2026-07-04',
					closed: false,
					minStay: 2,
					maxStay: null,
					closedToArrival: true,
					closedToDeparture: false
				}
			]
		});

		expect(index.price('dlx', '2026-07-04', 2)).toBe(12000);
		expect(index.price('dlx', '2026-07-04', 1)).toBeUndefined();
		expect(index.restriction('dlx', '2026-07-04')?.minStay).toBe(2);
	});
});

describe('formula', () => {
	it('describes how a derived plan follows its parent', () => {
		const bar = plan();

		expect(formula(plan({ deriveMode: 'PERCENT', deriveValue: 1500 }), bar)).toBe(
			'BAR + 15%, rounded to 1.00'
		);
		expect(formula(plan({ deriveMode: 'AMOUNT', deriveValue: -1050, roundingStep: 1 }), bar)).toBe(
			'BAR − 10.50'
		);
	});
});

describe('restrictionSummary', () => {
	it('names each restriction that is set', () => {
		expect(
			restrictionSummary({
				closed: true,
				minStay: 2,
				maxStay: 7,
				closedToArrival: true,
				closedToDeparture: true
			})
		).toBe('Closed · Min 2 · Max 7 · CTA · CTD');
		expect(
			restrictionSummary({
				closed: false,
				minStay: null,
				maxStay: null,
				closedToArrival: false,
				closedToDeparture: false
			})
		).toBe('');
	});
});

describe('batcher', () => {
	afterEach(() => vi.useRealTimers());

	it('saves the edits made in a short burst together, the last edit of a cell winning', async () => {
		vi.useFakeTimers();
		const saved: string[][] = [];
		const batch = batcher<{ cell: string; value: string }>(
			(edit) => edit.cell,
			async (edits) => void saved.push(edits.map((edit) => `${edit.cell}=${edit.value}`)),
			400
		);

		batch.add({ cell: 'a', value: '1' });
		await vi.advanceTimersByTimeAsync(300);
		batch.add({ cell: 'b', value: '2' });
		batch.add({ cell: 'a', value: '3' });
		expect(batch.has('a')).toBe(true);
		await vi.advanceTimersByTimeAsync(400);

		expect(saved).toEqual([['a=3', 'b=2']]);
		expect(batch.has('a')).toBe(false);
	});

	it('hands a failed save to its caller and keeps nothing queued', async () => {
		vi.useFakeTimers();
		const failures: unknown[] = [];
		const batch = batcher<{ cell: string }>(
			(edit) => edit.cell,
			() => Promise.reject(new Error('offline')),
			400,
			(err) => failures.push(err)
		);

		batch.add({ cell: 'a' });
		await vi.advanceTimersByTimeAsync(400);

		expect(failures).toHaveLength(1);
		expect(batch.has('a')).toBe(false);
	});
});
```

Modify `web/pms/src/lib/session.spec.ts`:

```diff
diff --git a/web/pms/src/lib/session.spec.ts b/web/pms/src/lib/session.spec.ts
index 1ec2b79..0da20b2 100644
--- a/web/pms/src/lib/session.spec.ts
+++ b/web/pms/src/lib/session.spec.ts
@@ -25,4 +25,16 @@ describe('can', () => {
 		expect(can(desk, 'blockRooms', 'p1')).toBe(true);
 		expect(can(housekeeping, 'blockRooms', 'p1')).toBe(false);
 	});
+
+	it('lets owners and managers manage rates, and nobody else', () => {
+		const owner = profile([{ role: 'owner' }]);
+		const manager = profile([{ role: 'manager' }]);
+		const accountant = profile([{ role: 'accountant' }]);
+		const desk = profile([{ role: 'front_desk' }]);
+
+		expect(can(owner, 'manageRates', 'p1')).toBe(true);
+		expect(can(manager, 'manageRates', 'p1')).toBe(true);
+		expect(can(accountant, 'manageRates', 'p1')).toBe(false);
+		expect(can(desk, 'manageRates', 'p1')).toBe(false);
+	});
 });
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `cd web/pms && bun run test`

Expected: `rates.spec.ts` fails: `Cannot find module './rates'`.

- [ ] **Step 3: Implement**

Run `bun run codegen` after adding the documents, so the generated query types exist.

Create `web/pms/src/lib/rates.ts`:

```ts
import { graphql } from './api/gql';
import type { RateGridQuery, RatePlansQuery } from './api/gql/graphql';
import { query } from './api/graphql';
import { addDays, monthDays } from './inventory';

export const RatePlansDocument = graphql(`
	query RatePlans($propertyId: UUID!) {
		ratePlans(propertyId: $propertyId) {
			id
			code
			name
			kind
			segment
			residency
			currency
			parentId
			depth
			deriveMode
			deriveValue
			roundingStep
			extraAdultAmount
			inheritRestrictions
			allowedMealPlans
			cancellationPolicyId
			roomTypeIds
			active
			version
		}
		mealSupplements(propertyId: $propertyId) {
			id
			mealPlan
			currency
			adultAmount
			childAmount
			from
			to
			version
		}
		cancellationPolicies(propertyId: $propertyId) {
			id
			name
			version
		}
	}
`);

export const RateGridDocument = graphql(`
	query RateGrid($propertyId: UUID!, $ratePlanId: UUID!, $from: Date!, $to: Date!) {
		rateGrid(propertyId: $propertyId, ratePlanId: $ratePlanId, from: $from, to: $to) {
			prices {
				roomTypeId
				date
				occupancy
				amount
			}
			restrictions {
				roomTypeId
				date
				closed
				minStay
				maxStay
				closedToArrival
				closedToDeparture
			}
		}
	}
`);

export const BulkPreviewDocument = graphql(`
	query BulkPreview(
		$propertyId: UUID!
		$ratePlanId: UUID!
		$from: Date!
		$to: Date!
		$weekdays: [Int!]!
		$roomTypeIds: [UUID!]!
		$mode: PriceChangeMode!
		$value: Int!
	) {
		bulkChangePreview(
			propertyId: $propertyId
			ratePlanId: $ratePlanId
			from: $from
			to: $to
			weekdays: $weekdays
			roomTypeIds: $roomTypeIds
			mode: $mode
			value: $value
		) {
			total
			cells {
				roomTypeId
				date
				occupancy
				before
				after
			}
		}
	}
`);

export const QuoteDocument = graphql(`
	query Quote(
		$propertyId: UUID!
		$roomTypeId: UUID!
		$ratePlanId: UUID!
		$mealPlan: MealPlan!
		$checkIn: Date!
		$checkOut: Date!
		$adults: Int!
		$children: Int!
		$residency: Residency!
	) {
		quote(
			propertyId: $propertyId
			roomTypeId: $roomTypeId
			ratePlanId: $ratePlanId
			mealPlan: $mealPlan
			checkIn: $checkIn
			checkOut: $checkOut
			adults: $adults
			children: $children
			residency: $residency
		) {
			nights {
				date
				room
				meal
			}
			total
			currency
			restrictionsOk
			violations {
				kind
				date
				message
			}
		}
	}
`);

export type RatePlan = RatePlansQuery['ratePlans'][number];
export type MealSupplement = RatePlansQuery['mealSupplements'][number];
export type RateGrid = RateGridQuery['rateGrid'];
export type Restriction = RateGrid['restrictions'][number];

/** Plans, meal supplements and cancellation policies change together under `rate-plans:<property>`. */
export function ratePlansKey(propertyId: string) {
	return [`rate-plans:${propertyId}`] as const;
}

/** Query key shared with the server's `rates:<property>:<plan>:<yyyy-mm>` event. */
export function ratesKey(propertyId: string, planId: string, month: string) {
	return [`rates:${propertyId}:${planId}:${month}`] as const;
}

export async function fetchRatePlans(propertyId: string, signal?: AbortSignal) {
	return query(RatePlansDocument, { propertyId }, signal);
}

/** A plan's prices and restrictions for a month (`YYYY-MM`). */
export async function fetchRateMonth(
	propertyId: string,
	ratePlanId: string,
	month: string,
	signal?: AbortSignal
) {
	const days = monthDays(month);
	const to = addDays(days[days.length - 1], 1);
	return (await query(RateGridDocument, { propertyId, ratePlanId, from: days[0], to }, signal))
		.rateGrid;
}

/** Digits after the decimal point in `currency` (2 for USD and LKR, 0 for JPY). */
function minorDigits(currency: string): number {
	return new Intl.NumberFormat('en', { style: 'currency', currency }).resolvedOptions()
		.maximumFractionDigits!;
}

/** Minor units as a plain number in `currency`, e.g. `15050` USD as `150.50`. */
export function formatMoney(amount: number, currency: string): string {
	const digits = minorDigits(currency);
	return new Intl.NumberFormat('en', {
		minimumFractionDigits: digits,
		maximumFractionDigits: digits
	}).format(amount / 10 ** digits);
}

/** What a person typed as minor units of `currency`, rounded half-up; `null` if it is not an amount. */
export function parseMoney(text: string, currency: string): number | null {
	const cleaned = text.replaceAll(',', '').trim();
	if (!/^\d+(\.\d*)?$/.test(cleaned)) return null;
	const digits = minorDigits(currency);
	const [whole, fraction = ''] = cleaned.split('.');
	const kept = fraction.padEnd(digits, '0').slice(0, digits);
	const roundUp = fraction.length > digits && Number(fraction[digits]) >= 5;
	return Number(whole) * 10 ** digits + Number(kept || '0') + (roundUp ? 1 : 0);
}

export interface RateRow {
	id: string;
	label: string;
	roomTypeId: string;
	code: string;
	/** Adults; `null` for the room type's restrictions row. */
	occupancy: number | null;
}

/** Grid rows: each active room type the plan sells, by occupancy (1 to its maximum adults), then its restrictions. */
export function rateRows(
	plan: Pick<RatePlan, 'roomTypeIds'>,
	roomTypes: readonly { id: string; code: string; maxAdults: number; active: boolean }[]
): RateRow[] {
	return roomTypes
		.filter((type) => type.active && plan.roomTypeIds.includes(type.id))
		.flatMap((type) => [
			...Array.from({ length: type.maxAdults }, (_, index) => ({
				id: `${type.id}:${index + 1}`,
				label: `${type.code} · ${index + 1} adult${index === 0 ? '' : 's'}`,
				roomTypeId: type.id,
				code: type.code,
				occupancy: index + 1
			})),
			{
				id: `${type.id}:restrictions`,
				label: `${type.code} · restrictions`,
				roomTypeId: type.id,
				code: type.code,
				occupancy: null
			}
		]);
}

/** Looks up a month's prices and restrictions. */
export function indexRates(grid: RateGrid) {
	const prices = new Map(
		grid.prices.map((p) => [`${p.roomTypeId}|${p.date}|${p.occupancy}`, p.amount])
	);
	const restrictions = new Map(grid.restrictions.map((r) => [`${r.roomTypeId}|${r.date}`, r]));
	return {
		price: (roomTypeId: string, date: string, occupancy: number) =>
			prices.get(`${roomTypeId}|${date}|${occupancy}`),
		restriction: (roomTypeId: string, date: string) => restrictions.get(`${roomTypeId}|${date}`)
	};
}

/** How a derived plan follows its parent, e.g. `BAR + 15%, rounded to 1.00`. */
export function formula(
	plan: Pick<RatePlan, 'deriveMode' | 'deriveValue' | 'roundingStep' | 'currency'>,
	parent: Pick<RatePlan, 'code'>
): string {
	const value = plan.deriveValue ?? 0;
	const sign = value < 0 ? '−' : '+';
	const size =
		plan.deriveMode === 'PERCENT'
			? `${Math.abs(value) / 100}%`
			: formatMoney(Math.abs(value), plan.currency);
	const rounding =
		plan.roundingStep > 1 ? `, rounded to ${formatMoney(plan.roundingStep, plan.currency)}` : '';
	return `${parent.code} ${sign} ${size}${rounding}`;
}

/** The restrictions set on a day, e.g. `Closed · Min 2 · CTA`; empty when there are none. */
export function restrictionSummary(
	r: Pick<Restriction, 'closed' | 'minStay' | 'maxStay' | 'closedToArrival' | 'closedToDeparture'>
): string {
	return [
		r.closed && 'Closed',
		r.minStay && `Min ${r.minStay}`,
		r.maxStay && `Max ${r.maxStay}`,
		r.closedToArrival && 'CTA',
		r.closedToDeparture && 'CTD'
	]
		.filter(Boolean)
		.join(' · ');
}

/**
 * Collects edits and saves them together once none has come for `delay` ms, so cells edited in quick
 * succession go out in one request. A cell edited twice is saved with its last value. A failed save goes to
 * `onError`; its edits are not retried (the screen refetches and shows what was saved).
 */
export function batcher<T>(
	key: (item: T) => string,
	save: (items: T[]) => Promise<void>,
	delay = 400,
	onError: (err: unknown) => void = () => {}
) {
	let queued = new Map<string, T>();
	let saving = new Set<string>();
	let timer: ReturnType<typeof setTimeout> | undefined;

	async function flush() {
		clearTimeout(timer);
		const items = queued;
		queued = new Map();
		if (items.size === 0) return;
		saving = new Set([...saving, ...items.keys()]);
		try {
			await save([...items.values()]);
		} catch (err) {
			onError(err);
		} finally {
			for (const itemKey of items.keys()) saving.delete(itemKey);
		}
	}

	return {
		add(item: T) {
			queued.set(key(item), item);
			clearTimeout(timer);
			timer = setTimeout(flush, delay);
		},
		/** Whether an edit of this cell is waiting to be saved or being saved. */
		has(itemKey: string) {
			return queued.has(itemKey) || saving.has(itemKey);
		},
		flush
	};
}
```

Modify `web/pms/src/lib/session.ts`:

```diff
diff --git a/web/pms/src/lib/session.ts b/web/pms/src/lib/session.ts
index 1466b33..f1e4c78 100644
--- a/web/pms/src/lib/session.ts
+++ b/web/pms/src/lib/session.ts
@@ -20,7 +20,8 @@ type Role = Profile['grants'][number]['role'];
 /** Roles allowed each action, mirroring `identity::Permission` on the server. */
 const ACTIONS = {
 	manageRooms: ['owner', 'manager'],
-	blockRooms: ['owner', 'manager', 'front_desk']
+	blockRooms: ['owner', 'manager', 'front_desk'],
+	manageRates: ['owner', 'manager']
 } satisfies Record<string, Role[]>;
 
 /** UI hint only; the API enforces permissions. A grant counts tenant-wide or for `propertyId`. */
```

- [ ] **Step 4: Run the checks**

```sh
cd web/pms
bun run api:schemas && bun run codegen && git diff --exit-code -- src/lib/api
bun run lint && bun run check && bun run test && bun run build
```

Expected: 61 unit tests pass.

- [ ] **Step 5: Commit**

```sh
git add -A
git commit -m "feat(web): rates data layer: queries, keys matching rate events, money formatting, grid rows and a save batcher"
```

### Task 14: Rate plans screen

The plan tree (indented by depth, with segment, currency and residency badges and each derived plan's
formula) and an edit panel for creating and changing plans: kind, parent and formula for derived plans (the
currency is the parent's and cannot be typed), segment, residency, rounding, extra-adult amount, room types,
meal plans, cancellation policy and active.

**Files:**
- Modify: `web/pms/src/routes/(app)/p/[property]/+layout.svelte`
- Create: `web/pms/src/routes/(app)/p/[property]/rate-plans/+page.svelte`
- Test: `web/pms/tests/e2e/rates.spec.ts` (new)

**Interfaces:**
- Consumes: `fetchRatePlans`, `formula`, `formatMoney`, `parseMoney`, `Pending`, `formKeys`.
- Produces: the page and its navigation link; e2e helper `savePlan(page, …)` in `rates.spec.ts`.

- [ ] **Step 1: Write the failing tests**

Create `web/pms/tests/e2e/rates.spec.ts`:

```ts
import { expect, test, type Page } from '@playwright/test';
import { addRoomType, createProperty, signUp } from './helpers';

/** Fills the rate plan editor and saves it. */
async function savePlan(
	page: Page,
	plan: {
		code?: string;
		name?: string;
		kind?: string;
		parent?: string;
		percent?: string;
		currency?: string;
		segment?: string;
	}
) {
	const editor = page.getByRole('form', { name: 'Rate plan' });
	if (plan.code) await editor.getByLabel('Code').fill(plan.code);
	if (plan.name) await editor.getByLabel('Name').fill(plan.name);
	if (plan.kind) await editor.getByLabel('Kind').selectOption(plan.kind);
	if (plan.parent) await editor.getByLabel('Derived from').selectOption({ label: plan.parent });
	if (plan.percent) await editor.getByLabel('Change (%)').fill(plan.percent);
	if (plan.currency) await editor.getByLabel('Currency').fill(plan.currency);
	if (plan.segment) await editor.getByLabel('Segment').selectOption(plan.segment);
	await editor.getByRole('button', { name: 'Save plan' }).click();
	await expect(editor).toBeHidden();
}

test('a revenue manager builds a tree of rate plans', async ({ page }) => {
	await signUp(page);
	await createProperty(page, 'GAL');
	await page.getByRole('link', { name: 'Room types' }).click();
	await addRoomType(page, 'DLX', 'Deluxe');

	await page.getByRole('link', { name: 'Rate plans' }).click();
	await page.getByRole('button', { name: 'New rate plan' }).click();
	await savePlan(page, { code: 'BAR', name: 'Best available', currency: 'USD', segment: 'FIT_F' });
	await page.getByRole('button', { name: 'New rate plan' }).click();
	await savePlan(page, {
		code: 'OTA',
		name: 'Online agents',
		kind: 'derived',
		parent: 'BAR',
		percent: '15',
		segment: 'OTA'
	});
	await page.getByRole('button', { name: 'New rate plan' }).click();
	await savePlan(page, { code: 'FITL', name: 'Residents', currency: 'LKR', segment: 'FIT_L' });

	const rows = page.getByRole('table', { name: 'Rate plans' }).getByRole('row');
	await expect(rows.nth(1)).toContainText('BAR');
	await expect(rows.nth(1)).toContainText('Non-residents');
	await expect(rows.nth(2)).toContainText('OTA');
	await expect(rows.nth(2)).toContainText('BAR + 15%');
	await expect(rows.nth(3)).toContainText('FITL');
	await expect(rows.nth(3)).toContainText('LKR');
	await expect(rows.nth(3)).toContainText('Residents only');

	await page.getByRole('button', { name: 'Edit OTA' }).click();
	await savePlan(page, { percent: '20' });
	await expect(rows.nth(2)).toContainText('BAR + 20%');

	// A derived plan takes its parent's currency, and a refused save says why.
	await page.getByRole('button', { name: 'New rate plan' }).click();
	const editor = page.getByRole('form', { name: 'Rate plan' });
	await editor.getByLabel('Kind').selectOption('derived');
	await editor.getByLabel('Derived from').selectOption({ label: 'FITL' });
	await expect(editor.getByLabel('Currency')).toHaveValue('LKR');
	await expect(editor.getByLabel('Currency')).toBeDisabled();
	await editor.getByLabel('Code').fill('BAR');
	await editor.getByLabel('Name').fill('Duplicate');
	await editor.getByLabel('Change (%)').fill('10');
	await editor.getByRole('button', { name: 'Save plan' }).click();
	await expect(editor.getByRole('alert')).toContainText('a rate plan with code BAR already exists');
});
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `cd web/pms && bun run test:e2e rates`

Expected: the test fails: `locator.click: Test timeout of 30000ms exceeded` (there is no "Rate plans" link).

- [ ] **Step 3: Implement**

Modify `web/pms/src/routes/(app)/p/[property]/+layout.svelte`:

```diff
diff --git a/web/pms/src/routes/(app)/p/[property]/+layout.svelte b/web/pms/src/routes/(app)/p/[property]/+layout.svelte
index 0671161..60db964 100644
--- a/web/pms/src/routes/(app)/p/[property]/+layout.svelte
+++ b/web/pms/src/routes/(app)/p/[property]/+layout.svelte
@@ -9,7 +9,8 @@
 		{ href: resolve('/(app)/p/[property]', { property }), label: 'Overview' },
 		{ href: resolve('/(app)/p/[property]/room-types', { property }), label: 'Room types' },
 		{ href: resolve('/(app)/p/[property]/rooms', { property }), label: 'Rooms' },
-		{ href: resolve('/(app)/p/[property]/inventory', { property }), label: 'Inventory' }
+		{ href: resolve('/(app)/p/[property]/inventory', { property }), label: 'Inventory' },
+		{ href: resolve('/(app)/p/[property]/rate-plans', { property }), label: 'Rate plans' }
 	]);
 </script>
 
```

Create `web/pms/src/routes/(app)/p/[property]/rate-plans/+page.svelte`:

```svelte
<script lang="ts">
	import { page } from '$app/state';
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { errorMessage } from '$lib/api/problem';
	import { formKeys, ifMatch, rest, unwrap } from '$lib/api/rest';
	import type { components } from '$lib/api/openapi';
	import { Pending } from '$lib/pending.svelte';
	import {
		fetchRatePlans,
		formatMoney,
		formula,
		parseMoney,
		ratePlansKey,
		type RatePlan
	} from '$lib/rates';
	import { fetchRoomTypes, roomTypesKey } from '$lib/rooms';
	import { can, fetchMe } from '$lib/session';

	type Schemas = components['schemas'];
	type Kind = Schemas['PlanKind'];
	type Segment = Schemas['Segment'];
	type MealPlan = Schemas['MealPlan'];

	const SEGMENTS: { value: Segment; label: string }[] = [
		{ value: 'FIT_F', label: 'FIT-F (foreign independent travellers)' },
		{ value: 'FIT_L', label: 'FIT-L (local independent travellers)' },
		{ value: 'OTA', label: 'OTA (online travel agents)' },
		{ value: 'TA', label: 'TA (travel agent contracts)' },
		{ value: 'IBE', label: 'IBE (own booking engine)' }
	];
	const MEAL_PLANS: MealPlan[] = ['RO', 'BB', 'HB', 'FB'];

	const propertyId = $derived(page.params.property ?? '');
	const client = useQueryClient();
	const me = createQuery(() => ({ queryKey: ['me'], queryFn: fetchMe }));
	const plans = createQuery(() => ({
		queryKey: ratePlansKey(propertyId),
		queryFn: ({ signal }) => fetchRatePlans(propertyId, signal)
	}));
	const roomTypes = createQuery(() => ({
		queryKey: roomTypesKey(propertyId),
		queryFn: ({ signal }) => fetchRoomTypes(propertyId, signal)
	}));
	const manage = $derived(!!me.data && can(me.data, 'manageRates', propertyId));
	const byId = $derived(new Map((plans.data?.ratePlans ?? []).map((plan) => [plan.id, plan])));

	interface Draft {
		id: string | null;
		version: number;
		code: string;
		name: string;
		kind: Kind;
		segment: Segment;
		residency: '' | 'resident' | 'non_resident';
		currency: string;
		parentId: string;
		deriveMode: 'percent' | 'amount';
		/** Percent (`15`, `-5`) or a signed amount (`-10.50`). */
		deriveValue: string;
		roundingStep: string;
		extraAdult: string;
		inherit: boolean;
		mealPlans: MealPlan[];
		roomTypeIds: string[];
		policyId: string;
		active: boolean;
	}

	let draft = $state<Draft | null>(null);
	let error = $state('');
	const pending = new Pending();
	const createForm = formKeys();
	const parent = $derived(draft?.kind === 'derived' ? byId.get(draft.parentId) : undefined);
	const currency = $derived(parent?.currency ?? draft?.currency ?? '');

	function newDraft(): Draft {
		return {
			id: null,
			version: 0,
			code: '',
			name: '',
			kind: 'standard',
			segment: 'IBE',
			residency: '',
			currency: '',
			parentId: '',
			deriveMode: 'percent',
			deriveValue: '',
			roundingStep: '1',
			extraAdult: '0',
			inherit: false,
			mealPlans: ['RO'],
			roomTypeIds: (roomTypes.data ?? []).filter((t) => t.active).map((t) => t.id),
			policyId: '',
			active: true
		};
	}

	function editDraft(plan: RatePlan): Draft {
		const value = plan.deriveValue ?? 0;
		const sign = value < 0 ? '-' : '';
		return {
			id: plan.id,
			version: plan.version,
			code: plan.code,
			name: plan.name,
			kind: plan.kind.toLowerCase() as Kind,
			segment: plan.segment,
			residency: (plan.residency?.toLowerCase() ?? '') as Draft['residency'],
			currency: plan.currency,
			parentId: plan.parentId ?? '',
			deriveMode: plan.deriveMode === 'AMOUNT' ? 'amount' : 'percent',
			deriveValue:
				plan.deriveMode === 'AMOUNT'
					? sign + formatMoney(Math.abs(value), plan.currency).replaceAll(',', '')
					: String(value / 100),
			roundingStep: formatMoney(plan.roundingStep, plan.currency).replaceAll(',', ''),
			extraAdult: formatMoney(plan.extraAdultAmount, plan.currency).replaceAll(',', ''),
			inherit: plan.inheritRestrictions,
			mealPlans: [...plan.allowedMealPlans],
			roomTypeIds: [...plan.roomTypeIds],
			policyId: plan.cancellationPolicyId ?? '',
			active: plan.active
		};
	}

	/** The derivation fields of a derived plan's request, in the units the API takes. */
	function derivation(d: Draft) {
		const negative = d.deriveValue.trim().startsWith('-');
		const size = d.deriveValue.trim().replace(/^[-+]/, '');
		const value =
			d.deriveMode === 'percent'
				? Math.round(Number(size) * 100)
				: (parseMoney(size, currency) ?? Number.NaN);
		if (!size || !Number.isFinite(value)) throw new Error('Enter the change as a number.');
		return {
			parent_id: d.parentId,
			derive_mode: d.deriveMode,
			derive_value: negative ? -value : value,
			inherit_restrictions: d.inherit
		};
	}

	function money(text: string, label: string): number {
		const amount = parseMoney(text, currency);
		if (amount === null) throw new Error(`Enter ${label} as an amount, e.g. 1.00.`);
		return amount;
	}

	async function save(event: SubmitEvent) {
		event.preventDefault();
		if (!draft) return;
		const d = draft;
		error = '';
		try {
			const common = {
				name: d.name,
				segment: d.segment,
				rounding_step: money(d.roundingStep, 'the rounding step'),
				extra_adult_amount: money(d.extraAdult, 'the extra adult amount'),
				allowed_meal_plans: d.mealPlans,
				room_type_ids: d.roomTypeIds,
				...(d.kind === 'derived' ? derivation(d) : {})
			};
			await pending.run('plan', async () => {
				if (d.id) {
					unwrap(
						await rest.PATCH('/api/v1/properties/{property}/rate-plans/{plan}', {
							params: { path: { property: propertyId, plan: d.id }, header: ifMatch(d.version) },
							body: {
								...common,
								residency: d.residency || null,
								cancellation_policy_id: d.policyId || null,
								active: d.active
							}
						})
					);
				} else {
					const body = {
						...common,
						code: d.code.toUpperCase(),
						kind: d.kind,
						currency: currency.toUpperCase(),
						residency: d.residency || undefined,
						cancellation_policy_id: d.policyId || undefined
					};
					unwrap(
						await rest.POST('/api/v1/properties/{property}/rate-plans', {
							params: {
								path: { property: propertyId },
								header: { 'Idempotency-Key': createForm.keyFor(body) }
							},
							body
						})
					);
					createForm.reset();
				}
			});
			draft = null;
		} catch (err) {
			error = err instanceof Error && !('problem' in err) ? err.message : errorMessage(err);
			if (!draft?.id) createForm.failed(err);
		} finally {
			await client.invalidateQueries({ queryKey: ratePlansKey(propertyId) });
		}
	}

	function residencyLabel(plan: RatePlan): string {
		if (plan.residency === 'RESIDENT') return 'Residents only';
		if (plan.residency === 'NON_RESIDENT') return 'Non-residents only';
		return 'Any guest';
	}

	function toggle<T>(list: T[], item: T, on: boolean): T[] {
		return on ? [...list.filter((x) => x !== item), item] : list.filter((x) => x !== item);
	}
</script>

<h1>Rate plans</h1>
{#if error && !draft}<p class="error" role="alert">{error}</p>{/if}

{#if plans.error || roomTypes.error}
	<p class="error" role="alert">{errorMessage(plans.error ?? roomTypes.error)}</p>
{:else if plans.data && roomTypes.data}
	<table aria-label="Rate plans">
		<thead>
			<tr>
				<th>Code</th>
				<th>Name</th>
				<th>Sold to</th>
				<th>Pricing</th>
				<th>Status</th>
				{#if manage}<th><span class="visually-hidden">Actions</span></th>{/if}
			</tr>
		</thead>
		<tbody>
			{#each plans.data.ratePlans as plan (plan.id)}
				<tr class:inactive={!plan.active}>
					<td style:padding-left="{0.6 + plan.depth * 1.5}rem">
						{#if plan.depth > 0}<span aria-hidden="true">↳ </span>{/if}{plan.code}
					</td>
					<td>{plan.name}</td>
					<td>
						<span class="badge">{plan.segment.replace('_', '-')}</span>
						<span class="badge">{plan.currency}</span>
						<span class="badge">{residencyLabel(plan)}</span>
					</td>
					<td>
						{#if plan.parentId && byId.get(plan.parentId)}
							{formula(plan, byId.get(plan.parentId)!)}
						{:else}
							{plan.kind === 'CUSTOM' ? 'Custom, priced by hand' : 'Priced by hand'}
						{/if}
					</td>
					<td>{plan.active ? 'Active' : 'Inactive'}</td>
					{#if manage}
						<td>
							<button
								class="secondary"
								aria-label="Edit {plan.code}"
								onclick={() => {
									error = '';
									draft = editDraft(plan);
								}}>Edit</button
							>
						</td>
					{/if}
				</tr>
			{:else}
				<tr><td colspan="6">No rate plans yet.</td></tr>
			{/each}
		</tbody>
	</table>
	{#if manage && !draft}
		<button
			onclick={() => {
				error = '';
				draft = newDraft();
			}}>New rate plan</button
		>
	{/if}

	{#if draft}
		<form class="form panel" aria-label="Rate plan" onsubmit={save}>
			<h2>{draft.id ? `Edit ${draft.code}` : 'New rate plan'}</h2>
			<label
				>Code <input
					required
					pattern={'[A-Za-z0-9_\\-]{1,20}'}
					disabled={!!draft.id}
					bind:value={draft.code}
				/></label
			>
			<label>Name <input required maxlength="100" bind:value={draft.name} /></label>
			<label>
				Kind
				<select disabled={!!draft.id} bind:value={draft.kind}>
					<option value="standard">Standard (priced by hand, can have derived plans)</option>
					<option value="derived">Derived (priced from another plan)</option>
					<option value="custom">Custom (priced by hand, stands alone)</option>
				</select>
			</label>
			{#if draft.kind === 'derived'}
				<label>
					Derived from
					<select required bind:value={draft.parentId}>
						<option value="" disabled>Choose a plan</option>
						{#each plans.data.ratePlans as plan (plan.id)}
							{#if plan.kind !== 'CUSTOM' && plan.id !== draft.id}
								<option value={plan.id}>{plan.code}</option>
							{/if}
						{/each}
					</select>
				</label>
				<label>
					Change by
					<select bind:value={draft.deriveMode}>
						<option value="percent">Percentage</option>
						<option value="amount">Amount</option>
					</select>
				</label>
				<label
					>{draft.deriveMode === 'percent' ? 'Change (%)' : `Change (${currency})`}
					<input required inputmode="decimal" bind:value={draft.deriveValue} /></label
				>
				<label class="check"
					><input type="checkbox" bind:checked={draft.inherit} /> Inherit the parent's restrictions</label
				>
			{/if}
			<label>
				Currency
				<input
					required
					pattern={'[A-Za-z]{3}'}
					disabled={!!draft.id || draft.kind === 'derived'}
					value={currency}
					oninput={(event) => draft && (draft.currency = event.currentTarget.value)}
				/>
			</label>
			<label>
				Segment
				<select bind:value={draft.segment}>
					{#each SEGMENTS as segment (segment.value)}
						<option value={segment.value}>{segment.label}</option>
					{/each}
				</select>
			</label>
			<label>
				Sold to
				<select bind:value={draft.residency}>
					<option value=""
						>{draft.segment.startsWith('FIT') ? 'As the segment says' : 'Any guest'}</option
					>
					<option value="resident">Residents only</option>
					<option value="non_resident">Non-residents only</option>
				</select>
			</label>
			<label>Round prices to <input inputmode="decimal" bind:value={draft.roundingStep} /></label>
			<label
				>Each extra adult above a priced occupancy <input
					inputmode="decimal"
					bind:value={draft.extraAdult}
				/></label
			>
			<fieldset>
				<legend>Room types sold</legend>
				{#each roomTypes.data.filter((t) => t.active || draft?.roomTypeIds.includes(t.id)) as type (type.id)}
					<label class="check">
						<input
							type="checkbox"
							checked={draft.roomTypeIds.includes(type.id)}
							onchange={(event) =>
								draft &&
								(draft.roomTypeIds = toggle(
									draft.roomTypeIds,
									type.id,
									event.currentTarget.checked
								))}
						/>
						{type.code} · {type.name}
					</label>
				{/each}
			</fieldset>
			<fieldset>
				<legend>Meal plans sold</legend>
				{#each MEAL_PLANS as mealPlan (mealPlan)}
					<label class="check">
						<input
							type="checkbox"
							checked={draft.mealPlans.includes(mealPlan)}
							onchange={(event) =>
								draft &&
								(draft.mealPlans = toggle(draft.mealPlans, mealPlan, event.currentTarget.checked))}
						/>
						{mealPlan}
					</label>
				{/each}
			</fieldset>
			<label>
				Cancellation policy
				<select bind:value={draft.policyId}>
					<option value="">None</option>
					{#each plans.data.cancellationPolicies as policy (policy.id)}
						<option value={policy.id}>{policy.name}</option>
					{/each}
				</select>
			</label>
			{#if draft.id}
				<label class="check"><input type="checkbox" bind:checked={draft.active} /> Active</label>
			{/if}
			{#if error}<p class="error" role="alert">{error}</p>{/if}
			<div class="actions">
				<button disabled={pending.has('plan')}>Save plan</button>
				<button type="button" class="secondary" onclick={() => (draft = null)}>Cancel</button>
			</div>
		</form>
	{/if}
{:else}
	<p>Loading…</p>
{/if}

<style>
	.badge {
		display: inline-block;
		padding: 0 0.4rem;
		margin-right: 0.25rem;
		border: 1px solid var(--border);
		border-radius: var(--radius);
		font-size: 0.85em;
	}
	.panel {
		margin-top: var(--space);
		padding: var(--space);
		border: 1px solid var(--border);
		border-radius: var(--radius);
		max-width: 32rem;
	}
	.check {
		display: flex;
		gap: 0.4rem;
		align-items: center;
	}
	fieldset {
		border: 1px solid var(--border);
		border-radius: var(--radius);
	}
</style>
```

- [ ] **Step 4: Run the checks**

```sh
cd web/pms
bun run api:schemas && bun run codegen && git diff --exit-code -- src/lib/api
bun run lint && bun run check && bun run test && bun run build
bun run test:e2e rates
```

Expected: 1 Playwright test passes.

- [ ] **Step 5: Commit**

```sh
git add -A
git commit -m "feat(web): rate plans screen: the plan tree with segment, currency and residency badges, and an edit panel"
```

### Task 15: Rate grid, bulk change, restrictions and quote

The Rates screen: a plan selector and month navigation over the Phase 1 `DateGrid`. Price cells open an input
on click or Enter and are saved in batches; derived plans are read-only and show their formula. "Bulk
change…" previews the change on the server before applying it; "Restrictions…" sets restrictions over a
range; a quote panel prices a stay on the selected plan.

**Files:**
- Modify: `web/pms/src/routes/(app)/p/[property]/+layout.svelte`
- Create: `web/pms/src/routes/(app)/p/[property]/rates/+page.svelte`
- Test: `web/pms/tests/e2e/rates.spec.ts`

**Interfaces:**
- Consumes: everything in `$lib/rates`, `DateGrid`, `Pending`, `formKeys`, inventory month helpers.
- Produces: the page and its navigation link.

- [ ] **Step 1: Write the failing tests**

Modify `web/pms/tests/e2e/rates.spec.ts`:

```diff
diff --git a/web/pms/tests/e2e/rates.spec.ts b/web/pms/tests/e2e/rates.spec.ts
index 583079c..4b5bc2a 100644
--- a/web/pms/tests/e2e/rates.spec.ts
+++ b/web/pms/tests/e2e/rates.spec.ts
@@ -73,3 +73,115 @@ test('a revenue manager builds a tree of rate plans', async ({ page }) => {
 	await editor.getByRole('button', { name: 'Save plan' }).click();
 	await expect(editor.getByRole('alert')).toContainText('a rate plan with code BAR already exists');
 });
+
+/** `YYYY-MM-DD` plus `days`. */
+function addDays(date: string, days: number): string {
+	const moved = new Date(`${date}T00:00:00Z`);
+	moved.setUTCDate(moved.getUTCDate() + days);
+	return moved.toISOString().slice(0, 10);
+}
+
+/** The first day of the month after `date`'s, and the first Saturday and Monday in it. */
+function nextMonth(date: string) {
+	const first = new Date(`${date.slice(0, 7)}-01T00:00:00Z`);
+	first.setUTCMonth(first.getUTCMonth() + 1);
+	const start = first.toISOString().slice(0, 10);
+	const offset = (weekday: number) => (weekday - first.getUTCDay() + 7) % 7;
+	const last = new Date(first);
+	last.setUTCMonth(last.getUTCMonth() + 1);
+	last.setUTCDate(0);
+	return {
+		start,
+		end: last.toISOString().slice(0, 10),
+		saturday: addDays(start, offset(6)),
+		monday: addDays(start, offset(1))
+	};
+}
+
+test('prices are edited in the grid, changed in bulk and quoted', async ({ page }) => {
+	await signUp(page);
+	await createProperty(page, 'GAL');
+	await page.getByRole('link', { name: 'Room types' }).click();
+	await addRoomType(page, 'DLX', 'Deluxe');
+	await page.getByRole('link', { name: 'Rate plans' }).click();
+	await page.getByRole('button', { name: 'New rate plan' }).click();
+	await savePlan(page, { code: 'BAR', name: 'Best available', currency: 'USD', segment: 'FIT_F' });
+	await page.getByRole('button', { name: 'New rate plan' }).click();
+	await savePlan(page, {
+		code: 'OTA',
+		name: 'Online agents',
+		kind: 'derived',
+		parent: 'BAR',
+		percent: '15',
+		segment: 'OTA'
+	});
+
+	await page.getByRole('link', { name: 'Rates', exact: true }).click();
+	const today = (await page.getByTestId('business-date').textContent())!.trim();
+	const grid = page.getByRole('grid', { name: 'Prices' });
+	const cell = (row: string, date: string, text: string) =>
+		grid.getByRole('gridcell', { name: `${row} ${date}: ${text}` });
+
+	// Type a price into a cell; it is saved when the cell is left.
+	await cell('DLX · 2 adults', today, 'no price').click();
+	await page.getByLabel(`Price for DLX · 2 adults on ${today}`).fill('150');
+	await page.keyboard.press('Enter');
+	await expect(cell('DLX · 2 adults', today, '150.00')).toBeVisible();
+
+	// The derived plan follows at once: 150.00 + 15% = 172.50, rounded to 173.00.
+	await page.getByLabel('Rate plan').selectOption({ label: 'OTA' });
+	await expect(page.getByText('Derived from BAR: BAR + 15%, rounded to 1.00')).toBeVisible();
+	await expect(cell('DLX · 2 adults', today, '173.00')).toBeVisible();
+	await page.getByLabel('Rate plan').selectOption({ label: 'BAR' });
+
+	// Fill next month at 100.00, then weekends +10%, each previewed before it is applied.
+	const month = nextMonth(today);
+	const bulk = page.getByRole('dialog', { name: 'Bulk change' });
+	await page.getByRole('button', { name: 'Bulk change…' }).click();
+	await bulk.getByLabel('From').fill(month.start);
+	await bulk.getByLabel('Through').fill(month.end);
+	await bulk.getByLabel('Change').selectOption('set');
+	await bulk.getByLabel('Value').fill('100');
+	await bulk.getByRole('button', { name: 'Preview' }).click();
+	await expect(bulk.getByTestId('preview-total')).toContainText('prices change');
+	await bulk.getByRole('button', { name: 'Apply' }).click();
+	await expect(bulk).toBeHidden();
+	await page.getByRole('button', { name: 'Bulk change…' }).click();
+	await bulk.getByLabel('From').fill(month.start);
+	await bulk.getByLabel('Through').fill(month.end);
+	for (const day of ['Mon', 'Tue', 'Wed', 'Thu', 'Fri']) await bulk.getByLabel(day).uncheck();
+	await bulk.getByLabel('Change').selectOption('percent');
+	await bulk.getByLabel('Value').fill('10');
+	await bulk.getByRole('button', { name: 'Preview' }).click();
+	await expect(bulk.getByRole('table', { name: 'Changes' })).toContainText('100.00 → 110.00');
+	await bulk.getByRole('button', { name: 'Apply' }).click();
+	await expect(bulk).toBeHidden();
+
+	await page.getByRole('button', { name: 'Next month' }).click();
+	await expect(cell('DLX · 2 adults', month.saturday, '110.00')).toBeVisible();
+	await expect(cell('DLX · 1 adult', month.monday, '100.00')).toBeVisible();
+	await page.getByLabel('Rate plan').selectOption({ label: 'OTA' });
+	await expect(cell('DLX · 2 adults', month.saturday, '127.00')).toBeVisible();
+	await expect(cell('DLX · 2 adults', month.monday, '115.00')).toBeVisible();
+	await page.getByLabel('Rate plan').selectOption({ label: 'BAR' });
+	await page.getByRole('button', { name: 'Previous month' }).click();
+
+	// A minimum stay shows on the restrictions row and stops a one-night quote.
+	const restrictions = page.getByRole('dialog', { name: 'Restrictions' });
+	await page.getByRole('button', { name: 'Restrictions…' }).click();
+	await restrictions.getByLabel('From').fill(today);
+	await restrictions.getByLabel('Through').fill(today);
+	await restrictions.getByLabel('Minimum stay').fill('2');
+	await restrictions.getByRole('button', { name: 'Save restrictions' }).click();
+	await expect(restrictions).toBeHidden();
+	await expect(cell('DLX · restrictions', today, 'Min 2')).toBeVisible();
+
+	const quote = page.getByRole('form', { name: 'Quote' });
+	await quote.getByLabel('Check-in').fill(today);
+	await quote.getByLabel('Check-out').fill(addDays(today, 1));
+	await quote.getByLabel('Residency').selectOption('NON_RESIDENT');
+	await quote.getByRole('button', { name: 'Quote' }).click();
+	const result = page.getByRole('region', { name: 'Quote result' });
+	await expect(result).toContainText('Total 150.00 USD');
+	await expect(result).toContainText(`stays over ${today} are at least 2 nights`);
+});
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `cd web/pms && bun run test:e2e rates`

Expected: the new test fails: `locator.click: Test timeout of 30000ms exceeded` (there is no "Rates" link); the Task 14 test passes.

- [ ] **Step 3: Implement**

Modify `web/pms/src/routes/(app)/p/[property]/+layout.svelte`:

```diff
diff --git a/web/pms/src/routes/(app)/p/[property]/+layout.svelte b/web/pms/src/routes/(app)/p/[property]/+layout.svelte
index 60db964..c0ec4bb 100644
--- a/web/pms/src/routes/(app)/p/[property]/+layout.svelte
+++ b/web/pms/src/routes/(app)/p/[property]/+layout.svelte
@@ -10,7 +10,8 @@
 		{ href: resolve('/(app)/p/[property]/room-types', { property }), label: 'Room types' },
 		{ href: resolve('/(app)/p/[property]/rooms', { property }), label: 'Rooms' },
 		{ href: resolve('/(app)/p/[property]/inventory', { property }), label: 'Inventory' },
-		{ href: resolve('/(app)/p/[property]/rate-plans', { property }), label: 'Rate plans' }
+		{ href: resolve('/(app)/p/[property]/rate-plans', { property }), label: 'Rate plans' },
+		{ href: resolve('/(app)/p/[property]/rates', { property }), label: 'Rates' }
 	]);
 </script>
 
```

Create `web/pms/src/routes/(app)/p/[property]/rates/+page.svelte`:

```svelte
<script lang="ts">
	import { page } from '$app/state';
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { SvelteMap } from 'svelte/reactivity';
	import type { BulkPreviewQuery, QuoteQuery } from '$lib/api/gql/graphql';
	import { query } from '$lib/api/graphql';
	import { errorMessage } from '$lib/api/problem';
	import { formKeys, rest, unwrap } from '$lib/api/rest';
	import DateGrid from '$lib/components/DateGrid.svelte';
	import { addDays, monthDays, monthOf, shiftMonth } from '$lib/inventory';
	import { Pending } from '$lib/pending.svelte';
	import { fetchProperties, propertiesKey } from '$lib/properties';
	import {
		BulkPreviewDocument,
		QuoteDocument,
		batcher,
		fetchRateMonth,
		fetchRatePlans,
		formatMoney,
		formula,
		indexRates,
		parseMoney,
		ratePlansKey,
		rateRows,
		ratesKey,
		restrictionSummary,
		type RateRow
	} from '$lib/rates';
	import { fetchRoomTypes, roomTypesKey } from '$lib/rooms';
	import { can, fetchMe } from '$lib/session';

	const WEEKDAYS = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'];
	const propertyId = $derived(page.params.property ?? '');
	const client = useQueryClient();
	const me = createQuery(() => ({ queryKey: ['me'], queryFn: fetchMe }));
	const properties = createQuery(() => ({
		queryKey: propertiesKey,
		queryFn: ({ signal }) => fetchProperties(signal)
	}));
	const plans = createQuery(() => ({
		queryKey: ratePlansKey(propertyId),
		queryFn: ({ signal }) => fetchRatePlans(propertyId, signal)
	}));
	const roomTypes = createQuery(() => ({
		queryKey: roomTypesKey(propertyId),
		queryFn: ({ signal }) => fetchRoomTypes(propertyId, signal)
	}));

	const businessDate = $derived(
		properties.data?.find((property) => property.id === propertyId)?.businessDate ?? ''
	);
	let chosenPlan = $state('');
	const plan = $derived(
		plans.data?.ratePlans.find((p) => p.id === chosenPlan) ?? plans.data?.ratePlans[0]
	);
	const parent = $derived(
		plan?.parentId ? plans.data?.ratePlans.find((p) => p.id === plan.parentId) : undefined
	);
	const manage = $derived(!!me.data && can(me.data, 'manageRates', propertyId));
	const editable = $derived(manage && !!plan && plan.kind !== 'DERIVED');
	let chosenMonth = $state<string | null>(null);
	const month = $derived(chosenMonth ?? (businessDate ? monthOf(businessDate) : ''));
	const days = $derived(month ? monthDays(month) : []);
	const grid = createQuery(() => ({
		queryKey: ratesKey(propertyId, plan?.id ?? '', month),
		queryFn: ({ signal }) => fetchRateMonth(propertyId, plan!.id, month, signal),
		enabled: !!plan && !!month
	}));
	const rates = $derived(indexRates(grid.data ?? { prices: [], restrictions: [] }));
	const rows = $derived(plan ? rateRows(plan, roomTypes.data ?? []) : []);
	const currency = $derived(plan?.currency ?? '');
	const monthLabel = $derived(
		month
			? new Date(`${month}-01T00:00:00Z`).toLocaleDateString(undefined, {
					month: 'long',
					year: 'numeric',
					timeZone: 'UTC'
				})
			: ''
	);

	let error = $state('');
	const pending = new Pending();

	/** Prices typed but not saved yet, by `planId|roomTypeId|date|occupancy`, shown until the refetch. */
	const drafts = new SvelteMap<string, number>();
	interface Edit {
		planId: string;
		room_type_id: string;
		date: string;
		occupancy: number;
		amount: number;
	}
	const cellKey = (planId: string, roomTypeId: string, date: string, occupancy: number) =>
		`${planId}|${roomTypeId}|${date}|${occupancy}`;
	const edits = batcher<Edit>(
		(edit) => cellKey(edit.planId, edit.room_type_id, edit.date, edit.occupancy),
		async (items) => {
			const planId = items[0].planId;
			const prices = items.map(({ room_type_id, date, occupancy, amount }) => ({
				room_type_id,
				date,
				occupancy,
				amount
			}));
			try {
				unwrap(
					await rest.PUT('/api/v1/properties/{property}/rate-plans/{plan}/prices', {
						params: { path: { property: propertyId, plan: planId } },
						body: { prices }
					})
				);
			} finally {
				await refetchPlan(planId);
				for (const edit of items) {
					drafts.delete(cellKey(planId, edit.room_type_id, edit.date, edit.occupancy));
				}
			}
		},
		400,
		(err) => (error = errorMessage(err))
	);

	/** Refetches every loaded month of `planId` (and, through events, the plans derived from it). */
	function refetchPlan(planId: string) {
		const prefix = `rates:${propertyId}:${planId}:`;
		return client.invalidateQueries({ predicate: (q) => String(q.queryKey[0]).startsWith(prefix) });
	}

	function price(row: RateRow, date: string): number | undefined {
		if (!plan || row.occupancy === null) return undefined;
		return (
			drafts.get(cellKey(plan.id, row.roomTypeId, date, row.occupancy)) ??
			rates.price(row.roomTypeId, date, row.occupancy)
		);
	}

	function cellLabel(row: RateRow, date: string): string {
		if (row.occupancy === null) {
			const restriction = rates.restriction(row.roomTypeId, date);
			return `${row.label} ${date}: ${(restriction && restrictionSummary(restriction)) || 'none'}`;
		}
		const amount = price(row, date);
		return `${row.label} ${date}: ${amount === undefined ? 'no price' : formatMoney(amount, currency)}`;
	}

	let editing = $state<{ row: RateRow; date: string; text: string } | null>(null);

	function startEditing(row: RateRow, date: string) {
		finishEditing();
		if (!editable || row.occupancy === null || date < businessDate) return;
		const amount = price(row, date);
		editing = {
			row,
			date,
			text: amount === undefined ? '' : formatMoney(amount, currency).replaceAll(',', '')
		};
	}

	/** Queues the edited price for saving; an empty or unchanged cell is left as it is. */
	function finishEditing() {
		const current = editing;
		editing = null;
		if (!current || !plan || current.row.occupancy === null || current.text.trim() === '') return;
		const amount = parseMoney(current.text, currency);
		if (amount === null) {
			error = `“${current.text}” is not a price.`;
			return;
		}
		if (amount === price(current.row, current.date)) return;
		error = '';
		const edit = {
			planId: plan.id,
			room_type_id: current.row.roomTypeId,
			date: current.date,
			occupancy: current.row.occupancy,
			amount
		};
		drafts.set(cellKey(edit.planId, edit.room_type_id, edit.date, edit.occupancy), amount);
		edits.add(edit);
	}

	function focusOnMount(node: HTMLInputElement) {
		node.focus();
		node.select();
	}

	function choosePlan(id: string) {
		finishEditing();
		void edits.flush();
		chosenPlan = id;
	}

	/** Range forms: `through` is the last day changed; the API takes `[from, to)`. */
	interface RangeForm {
		from: string;
		through: string;
		weekdays: number[];
		roomTypeIds: string[];
	}

	function rangeForm(): RangeForm {
		return {
			from: businessDate,
			through: businessDate,
			weekdays: [1, 2, 3, 4, 5, 6, 7],
			roomTypeIds: plan ? [...plan.roomTypeIds] : []
		};
	}

	function toggle<T>(list: T[], item: T, on: boolean): T[] {
		return on ? [...list.filter((x) => x !== item), item] : list.filter((x) => x !== item);
	}

	let bulkDialog = $state<HTMLDialogElement>();
	let bulk = $state({
		...rangeForm(),
		mode: 'percent' as 'percent' | 'amount' | 'set',
		value: ''
	});
	let preview = $state<BulkPreviewQuery['bulkChangePreview'] | null>(null);
	let dialogError = $state('');
	const bulkKeys = formKeys();

	function openBulk() {
		bulk = { ...rangeForm(), mode: 'percent', value: '' };
		preview = null;
		dialogError = '';
		bulkDialog?.showModal();
	}

	/** The bulk change's value in the API's units: basis points, or minor units (signed for `amount`). */
	function bulkValue(): number {
		const text = bulk.value.trim();
		const negative = text.startsWith('-');
		const size = text.replace(/^[-+]/, '');
		const value =
			bulk.mode === 'percent' ? Math.round(Number(size) * 100) : parseMoney(size, currency);
		if (!size || value === null || !Number.isFinite(value) || (bulk.mode === 'set' && negative)) {
			throw new Error('Enter the value as a number.');
		}
		return negative ? -value : value;
	}

	function bulkBody() {
		return {
			from: bulk.from,
			to: addDays(bulk.through, 1),
			weekdays: bulk.weekdays.toSorted(),
			room_type_ids: bulk.roomTypeIds,
			change: { mode: bulk.mode, value: bulkValue() }
		};
	}

	async function previewBulk() {
		if (!plan) return;
		dialogError = '';
		try {
			const body = bulkBody();
			preview = (
				await query(BulkPreviewDocument, {
					propertyId,
					ratePlanId: plan.id,
					from: body.from,
					to: body.to,
					weekdays: body.weekdays,
					roomTypeIds: body.room_type_ids,
					mode: body.change.mode.toUpperCase() as 'PERCENT' | 'AMOUNT' | 'SET',
					value: body.change.value
				})
			).bulkChangePreview;
		} catch (err) {
			preview = null;
			dialogError = err instanceof Error && !('problem' in err) ? err.message : errorMessage(err);
		}
	}

	async function applyBulk(event: SubmitEvent) {
		event.preventDefault();
		if (!plan) return;
		const planId = plan.id;
		dialogError = '';
		try {
			const body = bulkBody();
			await pending.run('bulk', async () =>
				unwrap(
					await rest.POST('/api/v1/properties/{property}/rate-plans/{plan}/bulk-change', {
						params: {
							path: { property: propertyId, plan: planId },
							header: { 'Idempotency-Key': bulkKeys.keyFor(body) }
						},
						body
					})
				)
			);
			bulkKeys.reset();
			bulkDialog?.close();
		} catch (err) {
			bulkKeys.failed(err);
			dialogError = err instanceof Error && !('problem' in err) ? err.message : errorMessage(err);
		} finally {
			await refetchPlan(planId);
		}
	}

	let restrictionsDialog = $state<HTMLDialogElement>();
	let restriction = $state({
		...rangeForm(),
		closed: '',
		minStay: null as number | null,
		maxStay: null as number | null,
		arrival: '',
		departure: ''
	});

	function openRestrictions() {
		restriction = {
			...rangeForm(),
			closed: '',
			minStay: null,
			maxStay: null,
			arrival: '',
			departure: ''
		};
		dialogError = '';
		restrictionsDialog?.showModal();
	}

	/** `''` leaves a yes/no restriction as it is. */
	const flag = (value: string) => (value === '' ? undefined : value === 'true');
	/** A blank number field is `null`: leave that stay limit as it is. */
	const nights = (value: number | null) => value ?? undefined;

	async function saveRestrictions(event: SubmitEvent) {
		event.preventDefault();
		if (!plan) return;
		const planId = plan.id;
		dialogError = '';
		try {
			await pending.run('restrictions', async () =>
				unwrap(
					await rest.PUT('/api/v1/properties/{property}/rate-plans/{plan}/restrictions', {
						params: { path: { property: propertyId, plan: planId } },
						body: {
							from: restriction.from,
							to: addDays(restriction.through, 1),
							weekdays: restriction.weekdays.toSorted(),
							room_type_ids: restriction.roomTypeIds,
							closed: flag(restriction.closed),
							min_stay: nights(restriction.minStay),
							max_stay: nights(restriction.maxStay),
							closed_to_arrival: flag(restriction.arrival),
							closed_to_departure: flag(restriction.departure)
						}
					})
				)
			);
			restrictionsDialog?.close();
		} catch (err) {
			dialogError = errorMessage(err);
		} finally {
			await refetchPlan(planId);
		}
	}

	let stay = $state({
		roomTypeId: '',
		mealPlan: 'RO' as 'RO' | 'BB' | 'HB' | 'FB',
		checkIn: '',
		checkOut: '',
		adults: 2,
		children: 0,
		residency: 'NON_RESIDENT' as 'RESIDENT' | 'NON_RESIDENT'
	});
	let quoted = $state<QuoteQuery['quote'] | null>(null);
	let quoteError = $state('');

	async function runQuote(event: SubmitEvent) {
		event.preventDefault();
		if (!plan) return;
		quoteError = '';
		try {
			quoted = (
				await query(QuoteDocument, {
					propertyId,
					ratePlanId: plan.id,
					roomTypeId: stay.roomTypeId || plan.roomTypeIds[0],
					mealPlan: plan.allowedMealPlans.includes(stay.mealPlan)
						? stay.mealPlan
						: plan.allowedMealPlans[0],
					checkIn: stay.checkIn,
					checkOut: stay.checkOut,
					adults: stay.adults,
					children: stay.children,
					residency: stay.residency
				})
			).quote;
		} catch (err) {
			quoted = null;
			quoteError = errorMessage(err);
		}
	}

	const typeCode = (id: string) => roomTypes.data?.find((type) => type.id === id)?.code ?? '?';
</script>

<h1>Rates</h1>
{#if plans.error || roomTypes.error}
	<p class="error" role="alert">{errorMessage(plans.error ?? roomTypes.error)}</p>
{:else if plans.data && roomTypes.data}
	{#if !plan}
		<p>No rate plans yet. Add one on the Rate plans page.</p>
	{:else}
		<div class="inline-form">
			<label>
				Rate plan
				<select value={plan.id} onchange={(event) => choosePlan(event.currentTarget.value)}>
					{#each plans.data.ratePlans as option (option.id)}
						<option value={option.id}>{option.code}</option>
					{/each}
				</select>
			</label>
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
			<span class="hint"
				>Business date: <span data-testid="business-date">{businessDate}</span></span
			>
			{#if editable}
				<button onclick={openBulk}>Bulk change…</button>
			{/if}
			{#if manage && !(plan.kind === 'DERIVED' && plan.inheritRestrictions)}
				<button class="secondary" onclick={openRestrictions}>Restrictions…</button>
			{/if}
		</div>
		<p class="hint">
			{plan.name} · {plan.currency}
			{#if parent}
				· Derived from {parent.code}: {formula(plan, parent)}. Its prices follow {parent.code} and cannot
				be edited here.
			{:else if editable}
				· Click a price, or use the arrow keys and Enter, to change it; changes save as you go.
			{/if}
		</p>
		{#if error}<p class="error" role="alert">{error}</p>{/if}

		{#if grid.error}
			<p class="error" role="alert">{errorMessage(grid.error)}</p>
		{:else if grid.data}
			{#if rows.length === 0}
				<p>This plan sells no active room types.</p>
			{:else}
				<DateGrid
					label="Prices"
					{rows}
					columns={days}
					{cellLabel}
					railWidth={200}
					columnWidth={76}
					initialColumn={Math.max(0, days.indexOf(businessDate))}
					onactivate={startEditing}
				>
					{#snippet header(date)}
						<span class="day" class:today={date === businessDate}>
							<small>{WEEKDAYS[(new Date(`${date}T00:00:00Z`).getUTCDay() + 6) % 7]}</small>
							{Number(date.slice(8))}
						</span>
					{/snippet}
					{#snippet cell(row, date)}
						{#if editing && editing.row.id === row.id && editing.date === date}
							<input
								class="price"
								aria-label="Price for {row.label} on {date}"
								inputmode="decimal"
								bind:value={editing.text}
								use:focusOnMount
								onkeydown={(event) => {
									event.stopPropagation();
									if (event.key === 'Enter') finishEditing();
									if (event.key === 'Escape') editing = null;
								}}
								onblur={finishEditing}
							/>
						{:else if row.occupancy === null}
							{@const r = rates.restriction(row.roomTypeId, date)}
							<small class="restriction">{r ? restrictionSummary(r) : ''}</small>
						{:else}
							{@const amount = price(row, date)}
							<span
								class:draft={plan &&
									drafts.has(cellKey(plan.id, row.roomTypeId, date, row.occupancy))}
								>{amount === undefined ? '–' : formatMoney(amount, currency)}</span
							>
						{/if}
					{/snippet}
				</DateGrid>
			{/if}
		{:else}
			<p>Loading…</p>
		{/if}

		<form class="inline-form quote" aria-label="Quote" onsubmit={runQuote}>
			<h2>Quote a stay on {plan.code}</h2>
			<label>
				Room type
				<select bind:value={stay.roomTypeId}>
					{#each plan.roomTypeIds as id (id)}<option value={id}>{typeCode(id)}</option>{/each}
				</select>
			</label>
			<label>
				Meal plan
				<select bind:value={stay.mealPlan}>
					{#each plan.allowedMealPlans as mealPlan (mealPlan)}<option value={mealPlan}
							>{mealPlan}</option
						>{/each}
				</select>
			</label>
			<label>Check-in <input type="date" required bind:value={stay.checkIn} /></label>
			<label>Check-out <input type="date" required bind:value={stay.checkOut} /></label>
			<label>Adults <input type="number" min="1" max="50" bind:value={stay.adults} /></label>
			<label>Children <input type="number" min="0" max="50" bind:value={stay.children} /></label>
			<label>
				Residency
				<select bind:value={stay.residency}>
					<option value="NON_RESIDENT">Non-resident</option>
					<option value="RESIDENT">Resident</option>
				</select>
			</label>
			<button>Quote</button>
		</form>
		{#if quoteError}<p class="error" role="alert">{quoteError}</p>{/if}
		{#if quoted}
			<section aria-label="Quote result" aria-live="polite">
				<table>
					<thead><tr><th>Night</th><th>Room</th><th>Meals</th></tr></thead>
					<tbody>
						{#each quoted.nights as night (night.date)}
							<tr>
								<td>{night.date}</td>
								<td>{formatMoney(night.room, quoted.currency)}</td>
								<td>{formatMoney(night.meal, quoted.currency)}</td>
							</tr>
						{/each}
					</tbody>
				</table>
				<p>
					<strong>Total {formatMoney(quoted.total, quoted.currency)} {quoted.currency}</strong>
					{quoted.restrictionsOk ? '· can be sold' : '· cannot be sold as quoted:'}
				</p>
				<ul>
					{#each quoted.violations as violation, index (index)}<li class="error">
							{violation.message}
						</li>{/each}
				</ul>
			</section>
		{/if}
	{/if}
{:else}
	<p>Loading…</p>
{/if}

{#snippet rangeFields(form: RangeForm)}
	<label>From <input type="date" required bind:value={form.from} /></label>
	<label>Through <input type="date" required bind:value={form.through} /></label>
	<fieldset>
		<legend>Days of the week</legend>
		{#each WEEKDAYS as day, index (day)}
			<label class="check">
				<input
					type="checkbox"
					checked={form.weekdays.includes(index + 1)}
					onchange={(event) =>
						(form.weekdays = toggle(form.weekdays, index + 1, event.currentTarget.checked))}
				/>
				{day}
			</label>
		{/each}
	</fieldset>
	<fieldset>
		<legend>Room types</legend>
		{#each plan?.roomTypeIds ?? [] as id (id)}
			<label class="check">
				<input
					type="checkbox"
					checked={form.roomTypeIds.includes(id)}
					onchange={(event) =>
						(form.roomTypeIds = toggle(form.roomTypeIds, id, event.currentTarget.checked))}
				/>
				{typeCode(id)}
			</label>
		{/each}
	</fieldset>
{/snippet}

<dialog bind:this={bulkDialog} aria-labelledby="bulk-title">
	<form class="form" onsubmit={applyBulk}>
		<h2 id="bulk-title">Bulk change</h2>
		{@render rangeFields(bulk)}
		<label>
			Change
			<select bind:value={bulk.mode} onchange={() => (preview = null)}>
				<option value="percent">By a percentage (e.g. 10 or -5)</option>
				<option value="amount">By an amount (e.g. 5.00 or -5.00)</option>
				<option value="set">Set to</option>
			</select>
		</label>
		<label
			>Value <input
				required
				inputmode="decimal"
				bind:value={bulk.value}
				oninput={() => (preview = null)}
			/></label
		>
		{#if preview}
			<p data-testid="preview-total">
				{preview.total}
				{preview.total === 1 ? 'price changes' : 'prices change'}{preview.total >
				preview.cells.length
					? `; the first ${preview.cells.length}:`
					: ':'}
			</p>
			<table aria-label="Changes">
				<tbody>
					{#each preview.cells as change (`${change.date}|${change.roomTypeId}|${change.occupancy}`)}
						<tr>
							<td>{change.date}</td>
							<td>{typeCode(change.roomTypeId)} · {change.occupancy}</td>
							<td
								>{change.before === null || change.before === undefined
									? '–'
									: formatMoney(change.before, currency)} → {formatMoney(
									change.after,
									currency
								)}</td
							>
						</tr>
					{/each}
				</tbody>
			</table>
			{#if parent === undefined && plans.data?.ratePlans.some((p) => p.parentId === plan?.id)}
				<p class="hint">Plans derived from {plan?.code} change with it.</p>
			{/if}
		{/if}
		{#if dialogError}<p class="error" role="alert">{dialogError}</p>{/if}
		<div class="actions">
			<button type="button" class="secondary" onclick={previewBulk}>Preview</button>
			<button disabled={!preview || preview.total === 0 || pending.has('bulk')}>Apply</button>
			<button type="button" class="secondary" onclick={() => bulkDialog?.close()}>Cancel</button>
		</div>
	</form>
</dialog>

<dialog bind:this={restrictionsDialog} aria-labelledby="restrictions-title">
	<form class="form" onsubmit={saveRestrictions}>
		<h2 id="restrictions-title">Restrictions</h2>
		{@render rangeFields(restriction)}
		<p class="hint">Fields left blank stay as they are.</p>
		{#each [['closed', 'Closed'], ['arrival', 'Closed to arrival'], ['departure', 'Closed to departure']] as const as [field, label] (field)}
			<label>
				{label}
				<select bind:value={restriction[field]}>
					<option value="">Leave as it is</option>
					<option value="true">Yes</option>
					<option value="false">No</option>
				</select>
			</label>
		{/each}
		<label
			>Minimum stay <input
				type="number"
				min="1"
				max="365"
				bind:value={restriction.minStay}
			/></label
		>
		<label
			>Maximum stay <input
				type="number"
				min="1"
				max="365"
				bind:value={restriction.maxStay}
			/></label
		>
		{#if dialogError}<p class="error" role="alert">{dialogError}</p>{/if}
		<div class="actions">
			<button disabled={pending.has('restrictions')}>Save restrictions</button>
			<button type="button" class="secondary" onclick={() => restrictionsDialog?.close()}
				>Cancel</button
			>
		</div>
	</form>
</dialog>

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
	.price {
		width: 100%;
		padding: 0.2rem;
		text-align: right;
	}
	.draft {
		font-style: italic;
		color: var(--muted);
	}
	.restriction {
		font-size: 0.75em;
		color: var(--danger);
		text-align: center;
	}
	.quote {
		margin-top: calc(var(--space) * 2);
	}
	.quote h2 {
		flex-basis: 100%;
		margin: 0;
	}
	.check {
		display: flex;
		gap: 0.4rem;
		align-items: center;
	}
	fieldset {
		border: 1px solid var(--border);
		border-radius: var(--radius);
		display: flex;
		flex-wrap: wrap;
		gap: 0.5rem;
	}
</style>
```

- [ ] **Step 4: Run the checks**

```sh
cd web/pms
bun run api:schemas && bun run codegen && git diff --exit-code -- src/lib/api
bun run lint && bun run check && bun run test && bun run build
bun run test:e2e
```

Expected: 7 Playwright tests pass.

- [ ] **Step 5: Commit**

```sh
git add -A
git commit -m "feat(web): rate grid with cells saved in batches, bulk changes previewed before they apply, restrictions and a quote panel"
```

### Task 16: Meal supplements screen

A table per currency with inline edits and an add form; an overlap shows the API's 409. The quote panel now
saves typed prices before quoting, which the new test caught (it quoted while the price was still in the
400 ms batch).

**Files:**
- Modify: `web/pms/src/routes/(app)/p/[property]/+layout.svelte`
- Create: `web/pms/src/routes/(app)/p/[property]/meal-plans/+page.svelte`
- Modify: `web/pms/src/routes/(app)/p/[property]/rates/+page.svelte`
- Test: `web/pms/tests/e2e/rates.spec.ts`

**Interfaces:**
- Consumes: `fetchRatePlans`, `formatMoney`, `parseMoney`, `Pending`, `formKeys`, `ifMatch`.
- Produces: the page and its navigation link.

- [ ] **Step 1: Write the failing tests**

Modify `web/pms/tests/e2e/rates.spec.ts`:

```diff
diff --git a/web/pms/tests/e2e/rates.spec.ts b/web/pms/tests/e2e/rates.spec.ts
index 4b5bc2a..885bf3c 100644
--- a/web/pms/tests/e2e/rates.spec.ts
+++ b/web/pms/tests/e2e/rates.spec.ts
@@ -185,3 +185,66 @@ test('prices are edited in the grid, changed in bulk and quoted', async ({ page
 	await expect(result).toContainText('Total 150.00 USD');
 	await expect(result).toContainText(`stays over ${today} are at least 2 nights`);
 });
+
+test('meal supplements are set per currency and added to a quote per person', async ({ page }) => {
+	await signUp(page);
+	await createProperty(page, 'GAL');
+	await page.getByRole('link', { name: 'Room types' }).click();
+	await addRoomType(page, 'DLX', 'Deluxe');
+	await page.getByRole('link', { name: 'Rate plans' }).click();
+	await page.getByRole('button', { name: 'New rate plan' }).click();
+	const editor = page.getByRole('form', { name: 'Rate plan' });
+	await editor.getByLabel('BB').check();
+	await savePlan(page, { code: 'BAR', name: 'Best available', currency: 'USD', segment: 'FIT_F' });
+
+	await page.getByRole('link', { name: 'Meal plans' }).click();
+	const today = (await page.getByTestId('business-date').textContent())!.trim();
+	const add = page.getByRole('form', { name: 'New meal supplement' });
+	for (const [mealPlan, adult, child] of [
+		['BB', '15', '7.50'],
+		['HB', '30', '15']
+	]) {
+		await add.getByLabel('Meal plan').selectOption(mealPlan);
+		await add.getByLabel('Currency').fill('USD');
+		await add.getByLabel('Per adult').fill(adult);
+		await add.getByLabel('Per child').fill(child);
+		await add.getByLabel('From').fill(today);
+		await add.getByRole('button', { name: 'Add supplement' }).click();
+		await expect(page.getByRole('row', { name: new RegExp(`^${mealPlan} `) })).toBeVisible();
+	}
+	const usd = page.getByRole('table', { name: 'Meal supplements in USD' });
+	await expect(usd.getByRole('row', { name: /^BB / })).toContainText('15.00');
+
+	// A second breakfast price for the same dates is refused.
+	await add.getByLabel('Meal plan').selectOption('BB');
+	await add.getByRole('button', { name: 'Add supplement' }).click();
+	await expect(page.getByRole('alert')).toContainText('already covers some of these dates');
+
+	await usd.getByRole('button', { name: 'Edit BB' }).click();
+	await usd.getByLabel('Per adult for BB').fill('18');
+	await usd.getByRole('button', { name: 'Save BB' }).click();
+	await expect(usd.getByRole('row', { name: /^BB / })).toContainText('18.00');
+
+	// Price a night and quote it bed and breakfast for two adults.
+	await page.getByRole('link', { name: 'Rates', exact: true }).click();
+	await page
+		.getByRole('grid', { name: 'Prices' })
+		.getByRole('gridcell', { name: `DLX · 2 adults ${today}: no price` })
+		.click();
+	await page.getByLabel(`Price for DLX · 2 adults on ${today}`).fill('100');
+	await page.keyboard.press('Enter');
+	const quote = page.getByRole('form', { name: 'Quote' });
+	await quote.getByLabel('Meal plan').selectOption('BB');
+	await quote.getByLabel('Check-in').fill(today);
+	await quote.getByLabel('Check-out').fill(addDays(today, 1));
+	await expect(
+		page
+			.getByRole('grid', { name: 'Prices' })
+			.getByRole('gridcell', { name: `DLX · 2 adults ${today}: 100.00` })
+	).toBeVisible();
+	await quote.getByRole('button', { name: 'Quote' }).click();
+	// 100.00 room + 2 × 18.00 breakfast.
+	await expect(page.getByRole('region', { name: 'Quote result' })).toContainText(
+		'Total 136.00 USD'
+	);
+});
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `cd web/pms && bun run test:e2e rates`

Expected: the new test fails: `locator.click: Test timeout of 30000ms exceeded` (there is no "Meal plans" link); the earlier two pass.

- [ ] **Step 3: Implement**

Modify `web/pms/src/routes/(app)/p/[property]/+layout.svelte`:

```diff
diff --git a/web/pms/src/routes/(app)/p/[property]/+layout.svelte b/web/pms/src/routes/(app)/p/[property]/+layout.svelte
index c0ec4bb..e4cce50 100644
--- a/web/pms/src/routes/(app)/p/[property]/+layout.svelte
+++ b/web/pms/src/routes/(app)/p/[property]/+layout.svelte
@@ -11,7 +11,8 @@
 		{ href: resolve('/(app)/p/[property]/rooms', { property }), label: 'Rooms' },
 		{ href: resolve('/(app)/p/[property]/inventory', { property }), label: 'Inventory' },
 		{ href: resolve('/(app)/p/[property]/rate-plans', { property }), label: 'Rate plans' },
-		{ href: resolve('/(app)/p/[property]/rates', { property }), label: 'Rates' }
+		{ href: resolve('/(app)/p/[property]/rates', { property }), label: 'Rates' },
+		{ href: resolve('/(app)/p/[property]/meal-plans', { property }), label: 'Meal plans' }
 	]);
 </script>
 
```

Create `web/pms/src/routes/(app)/p/[property]/meal-plans/+page.svelte`:

```svelte
<script lang="ts">
	import { page } from '$app/state';
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { errorMessage } from '$lib/api/problem';
	import { formKeys, ifMatch, rest, unwrap } from '$lib/api/rest';
	import { Pending } from '$lib/pending.svelte';
	import { fetchProperties, propertiesKey } from '$lib/properties';
	import {
		fetchRatePlans,
		formatMoney,
		parseMoney,
		ratePlansKey,
		type MealSupplement
	} from '$lib/rates';
	import { can, fetchMe } from '$lib/session';

	const MEAL_PLANS = [
		{ value: 'BB', label: 'BB · bed and breakfast' },
		{ value: 'HB', label: 'HB · half board' },
		{ value: 'FB', label: 'FB · full board' }
	] as const;

	const propertyId = $derived(page.params.property ?? '');
	const client = useQueryClient();
	const me = createQuery(() => ({ queryKey: ['me'], queryFn: fetchMe }));
	const properties = createQuery(() => ({
		queryKey: propertiesKey,
		queryFn: ({ signal }) => fetchProperties(signal)
	}));
	const plans = createQuery(() => ({
		queryKey: ratePlansKey(propertyId),
		queryFn: ({ signal }) => fetchRatePlans(propertyId, signal)
	}));
	const manage = $derived(!!me.data && can(me.data, 'manageRates', propertyId));
	const businessDate = $derived(
		properties.data?.find((property) => property.id === propertyId)?.businessDate ?? ''
	);
	/** Supplements by currency; the list is already ordered by currency, meal plan and start. */
	const byCurrency = $derived(
		Map.groupBy(plans.data?.mealSupplements ?? [], (supplement) => supplement.currency)
	);

	let error = $state('');
	const pending = new Pending();
	const addForm = formKeys();
	let draft = $state({
		mealPlan: 'BB' as 'BB' | 'HB' | 'FB',
		currency: '',
		adult: '',
		child: '0',
		from: '',
		to: ''
	});
	let editing = $state<{ id: string; adult: string; child: string; to: string } | null>(null);

	function amount(text: string, currency: string): number {
		const value = parseMoney(text, currency);
		if (value === null) throw new Error(`“${text}” is not an amount.`);
		return value;
	}

	async function run(key: string, command: () => Promise<unknown>) {
		error = '';
		try {
			await pending.run(key, command);
			return true;
		} catch (err) {
			error = err instanceof Error && !('problem' in err) ? err.message : errorMessage(err);
			return false;
		} finally {
			await client.invalidateQueries({ queryKey: ratePlansKey(propertyId) });
		}
	}

	async function add(event: SubmitEvent) {
		event.preventDefault();
		const currency = draft.currency.toUpperCase();
		let body;
		try {
			body = {
				meal_plan: draft.mealPlan,
				currency,
				adult_amount: amount(draft.adult, currency),
				child_amount: amount(draft.child, currency),
				from: draft.from || businessDate,
				to: draft.to || null
			};
		} catch (err) {
			error = (err as Error).message;
			return;
		}
		const added = await run('add', async () => {
			try {
				unwrap(
					await rest.POST('/api/v1/properties/{property}/meal-supplements', {
						params: {
							path: { property: propertyId },
							header: { 'Idempotency-Key': addForm.keyFor(body) }
						},
						body
					})
				);
			} catch (err) {
				addForm.failed(err);
				throw err;
			}
		});
		if (added) addForm.reset();
	}

	function edit(supplement: MealSupplement) {
		editing = {
			id: supplement.id,
			adult: formatMoney(supplement.adultAmount, supplement.currency).replaceAll(',', ''),
			child: formatMoney(supplement.childAmount, supplement.currency).replaceAll(',', ''),
			to: supplement.to ?? ''
		};
	}

	async function save(supplement: MealSupplement) {
		if (!editing) return;
		const changes = editing;
		const saved = await run(supplement.id, async () =>
			unwrap(
				await rest.PATCH('/api/v1/properties/{property}/meal-supplements/{supplement}', {
					params: {
						path: { property: propertyId, supplement: supplement.id },
						header: ifMatch(supplement.version)
					},
					body: {
						adult_amount: amount(changes.adult, supplement.currency),
						child_amount: amount(changes.child, supplement.currency),
						to: changes.to || null
					}
				})
			)
		);
		if (saved) editing = null;
	}
</script>

<h1>Meal plans</h1>
<p class="hint">
	Room only (RO) is free. Bed and breakfast, half board and full board are charged per person per
	night on top of the room price, in the rate plan's currency. Business date:
	<span data-testid="business-date">{businessDate}</span>
</p>
{#if error}<p class="error" role="alert">{error}</p>{/if}

{#if plans.error}
	<p class="error" role="alert">{errorMessage(plans.error)}</p>
{:else if plans.data}
	{#each byCurrency as [currency, supplements] (currency)}
		<h2>{currency}</h2>
		<table aria-label="Meal supplements in {currency}">
			<thead>
				<tr>
					<th>Meal plan</th>
					<th>Per adult</th>
					<th>Per child</th>
					<th>From</th>
					<th>Until (first night not charged)</th>
					{#if manage}<th><span class="visually-hidden">Actions</span></th>{/if}
				</tr>
			</thead>
			<tbody>
				{#each supplements as supplement (supplement.id)}
					{@const code = supplement.mealPlan}
					<tr>
						<td>{code}</td>
						{#if editing?.id === supplement.id}
							<td><input aria-label="Per adult for {code}" bind:value={editing.adult} /></td>
							<td><input aria-label="Per child for {code}" bind:value={editing.child} /></td>
							<td>{supplement.from}</td>
							<td><input type="date" aria-label="Until for {code}" bind:value={editing.to} /></td>
							<td class="actions">
								<button
									aria-label="Save {code}"
									disabled={pending.has(supplement.id)}
									onclick={() => save(supplement)}>Save</button
								>
								<button class="secondary" onclick={() => (editing = null)}>Cancel</button>
							</td>
						{:else}
							<td>{formatMoney(supplement.adultAmount, currency)}</td>
							<td>{formatMoney(supplement.childAmount, currency)}</td>
							<td>{supplement.from}</td>
							<td>{supplement.to ?? 'Until further notice'}</td>
							{#if manage}
								<td>
									<button
										class="secondary"
										aria-label="Edit {code}"
										onclick={() => edit(supplement)}>Edit</button
									>
								</td>
							{/if}
						{/if}
					</tr>
				{/each}
			</tbody>
		</table>
	{:else}
		<p>No meal supplements yet.</p>
	{/each}

	{#if manage}
		<h2>Add a supplement</h2>
		<form class="inline-form" aria-label="New meal supplement" onsubmit={add}>
			<label>
				Meal plan
				<select bind:value={draft.mealPlan}>
					{#each MEAL_PLANS as mealPlan (mealPlan.value)}
						<option value={mealPlan.value}>{mealPlan.label}</option>
					{/each}
				</select>
			</label>
			<label>Currency <input required pattern={'[A-Za-z]{3}'} bind:value={draft.currency} /></label>
			<label>Per adult <input required inputmode="decimal" bind:value={draft.adult} /></label>
			<label>Per child <input required inputmode="decimal" bind:value={draft.child} /></label>
			<label>From <input type="date" bind:value={draft.from} /></label>
			<label>Until (optional) <input type="date" bind:value={draft.to} /></label>
			<button disabled={pending.has('add')}>Add supplement</button>
		</form>
	{/if}
{:else}
	<p>Loading…</p>
{/if}
```

Modify `web/pms/src/routes/(app)/p/[property]/rates/+page.svelte`:

```diff
diff --git a/web/pms/src/routes/(app)/p/[property]/rates/+page.svelte b/web/pms/src/routes/(app)/p/[property]/rates/+page.svelte
index f12f8be..58860a8 100644
--- a/web/pms/src/routes/(app)/p/[property]/rates/+page.svelte
+++ b/web/pms/src/routes/(app)/p/[property]/rates/+page.svelte
@@ -379,6 +379,8 @@
 		event.preventDefault();
 		if (!plan) return;
 		quoteError = '';
+		// Prices typed a moment ago are saved first, so the quote sees them.
+		await edits.flush();
 		try {
 			quoted = (
 				await query(QuoteDocument, {
```

- [ ] **Step 4: Run the checks**

```sh
cd web/pms
bun run api:schemas && bun run codegen && git diff --exit-code -- src/lib/api
bun run lint && bun run check && bun run test && bun run build
bun run test:e2e
```

Expected: 8 Playwright tests pass.

- [ ] **Step 5: Commit**

```sh
git add -A
git commit -m "feat(web): meal supplements per currency, and quotes that save typed prices first"
```

### Task 17: Documentation and the Phase 2 gate

api-conventions.md gains the rate events, the rates lock, the bulk-change idempotency rule, the GraphQL bounds
and enum spelling; ROADMAP.md links this plan and marks the Phase 1 follow-ups done; README.md lists the
`rates` module, the Phase 2 gates and a step-by-step script to try the rates screens by hand.

**Files:**
- Modify: `README.md`
- Modify: `docs/ROADMAP.md`
- Modify: `docs/design/api-conventions.md`

**Interfaces:**
- None.

- [ ] **Step 1: Make the change**

Modify `README.md`:

````diff
diff --git a/README.md b/README.md
index cb6effc..a708a63 100644
--- a/README.md
+++ b/README.md
@@ -11,7 +11,7 @@ Multi-tenant, cloud-hosted hotel property management system.
 |---|---|
 | `crates/core-api` | axum HTTP API (REST commands, GraphQL reads, server-sent events) |
 | `crates/db` | Postgres pool, migrations, tenant-scoped transactions, change events |
-| `modules/*` | Domain modules (`identity`, `property`, `rooms`, …) |
+| `modules/*` | Domain modules (`identity`, `property`, `rooms`, `rates`, …) |
 | `migrations/` | SQL migrations, applied by `core-api migrate` |
 | `web/pms` | SvelteKit staff app (single-page) |
 
@@ -45,11 +45,13 @@ bun run lint && bun run check && bun run test && bun run build
 
 ### Performance gates
 
-Phase 1 sets two, both run by hand because shared CI machines make timings noisy:
+Run by hand, because shared CI machines make timings noisy, and one at a time (`--test-threads=1`), since they share one Postgres:
 
 ```sh
 # inventory(month) for a 200-room, 12-type property: p95 under 20 ms server time
-DATABASE_URL=$DATABASE_OWNER_URL cargo test --release -p core-api --test perf -- --ignored --nocapture
+# rateGrid for 62 days, 12 room types, 2 occupancies: p95 under 30 ms
+# a bulk change of one year of 12 room types with 2 derived levels: median under 300 ms
+DATABASE_URL=$DATABASE_OWNER_URL cargo test --release -p core-api --test perf -- --ignored --nocapture --test-threads=1
 
 # the month grid for the same property renders in under 50 ms and scrolls at 60 fps (see End-to-end tests)
 cd web/pms && E2E_PERF=1 E2E_DATABASE_URL=... bun run test:e2e --grep @perf
@@ -71,6 +73,23 @@ E2E_DATABASE_URL=postgres://goodfolk_api:goodfolk_api_dev@localhost:5432/goodfol
 
 Chromium runs with its sandbox on, as in CI. If your machine cannot start it ("No usable sandbox!", for example on distributions that restrict unprivileged user namespaces), add `PLAYWRIGHT_NO_SANDBOX=1` for local runs; the config refuses it when `CI` is set.
 
+### Trying rates by hand
+
+With the API and `bun run dev` running (see Development; run `cargo run -p core-api -- migrate` first, as the schema owner, to add the Phase 2 tables):
+
+1. Sign up, add a property, and on **Room types** add `DLX` (2 adults, 1 child, max 3) and `STD` (2 adults); add a few rooms of each on **Rooms**.
+2. On **Rate plans**, add:
+   - `BAR`: standard, USD, segment IBE, meal plans RO, BB and HB.
+   - `OTA`: derived from BAR, change 15 %, segment OTA, meal plans RO and BB, tick "Inherit the parent's restrictions".
+   - `CORP`: custom, USD, segment TA.
+   - `FITF`: standard, USD, segment FIT-F (sold to non-residents only).
+   - `FITL`: standard, LKR, segment FIT-L (sold to residents only). Its prices are set by hand: a derived plan must use its parent's currency.
+3. On **Rates**, pick `BAR`, click a price cell, type `150` and press Enter (Tab or clicking elsewhere also saves). Switch to `OTA`: the same cell shows 173.00 (150.00 + 15 %, rounded to 1.00), and it cannot be edited.
+4. Still on `BAR`, open **Bulk change…**: from 1 July to 31 July next year, "Set to" `100`, **Preview**, **Apply**. Open it again for the same dates with only Sat and Sun ticked, "By a percentage" `10`, **Preview** (100.00 → 110.00), **Apply**. Go to July with the month arrows: weekends show 110.00; `OTA` shows 127.00 on weekends and 115.00 on weekdays.
+5. Open **Restrictions…** on `BAR` for a July Saturday: minimum stay `2`, closed to arrival "Yes". The restrictions row shows "Min 2 · CTA", on `OTA` too (it inherits them).
+6. On **Meal plans**, add BB in USD at 15.00 per adult and 7.50 per child, and HB in USD at 30.00 and 15.00.
+7. Back on **Rates**, quote `OTA`, DLX, BB, 2 adults, 1 child, non-resident: arriving on that Saturday for one night lists the minimum stay and the closed arrival; arriving on the Friday for two nights prices both nights with 37.50 of breakfast each. Quote `FITF` as a resident: it is refused ("sold to non-residents only").
+
 ### Configuration
 
 The API (`core-api serve`) reads:
````

Modify `docs/ROADMAP.md`:

```diff
diff --git a/docs/ROADMAP.md b/docs/ROADMAP.md
index 740ab4e..a9d03e3 100644
--- a/docs/ROADMAP.md
+++ b/docs/ROADMAP.md
@@ -10,6 +10,7 @@ Companion to [ARCHITECTURE.md](ARCHITECTURE.md). Each phase ends in something th
 | [design/api-conventions.md](design/api-conventions.md) | Auth, CSRF, errors, idempotency, concurrency, events, GraphQL and REST shapes |
 | [superpowers/plans/2026-09-23-phase-0-foundations.md](superpowers/plans/2026-09-23-phase-0-foundations.md) | **Phase 0 implementation plan**: 15 test-first tasks with complete code, run in order on a clean repository before the plan was written |
 | [superpowers/plans/2026-09-24-phase-1-rooms-inventory.md](superpowers/plans/2026-09-24-phase-1-rooms-inventory.md) | **Phase 1 implementation plan**: 17 tasks with complete code, executed in order on the Phase 0 code before the plan was written |
+| [superpowers/plans/2026-09-25-phase-2-rates-meal-plans.md](superpowers/plans/2026-09-25-phase-2-rates-meal-plans.md) | **Phase 2 implementation plan**: 17 tasks with complete code, executed in order on the Phase 1 code before the plan was written |
 | [specs/](specs/) | Phases 1–9: scope, data, API, UI, rules, required tests and performance gates |
 
 Each later phase gets its step-by-step implementation plan at the start of that phase, written and verified against the code as it stands then (the same method as Phase 0). Writing code-level plans for Phase 7 now would mean guessing at code that Phases 1–6 have not written yet.
@@ -47,7 +48,7 @@ Moved out of Phase 0 during planning (nothing used them yet): outbox → Pub/Sub
   - Log a warning when `load_grants` skips a role it doesn't recognise.
   - Still open, not in the Phase 1 plan: check the SSE `?property=` filter against the user's grants (property-scoped grants exist now, but the stream only carries cache keys); a purge job for expired sessions, old idempotency keys and old `login_failure` rows (runs in `jobs-svc`, Phase 7).
 
-## Phase 2 — Rates and meal plans ([spec](specs/phase-2-rates-meal-plans.md))
+## Phase 2 — Rates and meal plans ([spec](specs/phase-2-rates-meal-plans.md), [plan](superpowers/plans/2026-09-25-phase-2-rates-meal-plans.md))
 
 - Rate plans: standard, derived and custom, with segment tags (FIT-F, FIT-L, OTA, TA, IBE) (FIT-F = non-resident, FIT-L = resident; residency enforcement; resident prices set by hand; derivation only within the same currency).
 - `rate_day` with occupancy pricing and restrictions. Transactional recomputation of derived plans, depth limit, cycle prevention.
@@ -57,8 +58,8 @@ Moved out of Phase 0 during planning (nothing used them yet): outbox → Pub/Sub
 - Carried over from the Phase 1 reviews (do these before Phase 3 adds reservations to `inventory_day`):
   - **Retype deadlock (resolved).** `contribute` is now one UPDATE per type, and every counter writer locks its rows up front with `rooms::inventory::lock_days`; a test runs 30 concurrent opposite-retype pairs of blocked rooms. The lock-order bullet in [api-conventions.md](design/api-conventions.md) now states that rule.
   - **Scan-order locking (resolved).** `lock_days` locks every row a command will change in ascending `(room_type_id, date)` order before its first UPDATE, so the UPDATEs' plan order no longer matters; reservations must use it too.
-  - Tests still to add: concurrent sign-in attempts against the throttle; a REST cross-tenant POST of a room or section.
-  - Smaller follow-ups: `ETag` on replayed idempotent 201s and in the OpenAPI annotations; an empty PATCH still bumps the version; the rooms page's single page-wide busy flag; `DateGrid` can jump back to an old row when rows shrink and grow again.
+  - Tests added in the Phase 2 plan: concurrent sign-in attempts against the throttle; REST cross-tenant POSTs of a room, a room range and a section.
+  - Follow-ups done in the Phase 2 plan: replayed idempotent creates carry their `ETag`, and the OpenAPI document declares it; an update that names no field is a 422 and keeps the version; the rooms page disables only the row or form a command changes; `DateGrid` keeps its active cell when rows shrink and grow again.
 
 ## Phase 3 — Reservations ([spec](specs/phase-3-reservations.md))
 
```

Modify `docs/design/api-conventions.md`:

```diff
diff --git a/docs/design/api-conventions.md b/docs/design/api-conventions.md
index f0426ea..61297fd 100644
--- a/docs/design/api-conventions.md
+++ b/docs/design/api-conventions.md
@@ -67,7 +67,7 @@ Request DTOs derive `garde::Validate` and are checked with `error::validate(&bod
 ## Idempotency (every create command)
 
 - The client sends `Idempotency-Key: <8–200 chars>`, one fresh UUID per user action, reused only when retrying that same action.
-- Mount create routes in the `commands` router, which has `.route_layer(from_fn_with_state(state, idempotency::idempotent))`.
+- Mount create routes in the `commands` router, which has `.route_layer(from_fn_with_state(state, idempotency::idempotent))`. So is any other command that is not safe to repeat: `POST …/rate-plans/{plan}/bulk-change` (a second "+10 %" would compound) answers 200 and replays like a create.
 - Same key, same request: the stored first response is replayed, with its status, body, `Content-Type` and `ETag`. Same key, different request: 422. The request hash covers the user, method, path, query string and body, so a different user or query with the same key is a different request.
 - First request still running: 409, and the client retries. A claim is abandoned once it has been unfinished for 60 s (`routes::ABANDONED_CLAIM_AFTER`, the 15 s timeout plus margin), for example after a timeout or a dropped connection, and the next request with that key takes it over and runs.
 - A 5xx response, or one larger than 1 MiB, is not stored; its claim is released so the client may retry.
@@ -80,6 +80,7 @@ Request DTOs derive `garde::Validate` and are checked with `error::validate(&bod
 - **Idempotency claim lifetime:** an unfinished claim is abandoned after 60 s (`ABANDONED_CLAIM_AFTER`) and taken over by the next request with its key; finalizing and releasing touch only the request's own claim (matched on `created_at`).
 - **Live events:** the broadcast channel carries `events::LiveEvent` (`Invalidate(db::Event)` | `Resync`). The listener (`events::spawn_listener`, given a pool on the direct listen URL) sends `Resync` whenever its database connection drops, and again once it has reconnected if the first reconnect attempt failed, since changes committed in between were missed. Streams send it as a `resync` event, as they do when a subscriber lags.
 - **`inventory_day` lock order:** every counter UPDATE is preceded, in its transaction, by one ordered lock of every row it will change: `rooms::inventory::lock_days` (`select … order by room_type_id, date for update`), called after the command's room and block row locks and before its first counter update. Counter UPDATEs are bounded to the counter window (`[business date, business date + 730 days)`) so they never write a row outside that lock. Rows are therefore locked in ascending `(room_type_id, date)` order, all at once, and two counter updaters cannot deadlock on `inventory_day` whatever order or plan their UPDATEs use afterwards. A new counter writer (reservations included) must call it the same way; locking a few extra days is fine. Keep each UPDATE valid on its own for the counter constraints: `rooms::inventory::contribute` changes `physical` and `out_of_order` in one statement so `out_of_order <= physical` holds on every row. Window extension (`rooms::extend_window`, INSERT … ON CONFLICT DO NOTHING) is not covered: its SELECT is ordered by `(room_type_id, date)` so rows are inserted in the same order, but Postgres does not formally guarantee INSERT … SELECT insertion order, so it will move to the Phase 7 nightly job under a property-level lock (see the Phase 7 carry-over in [ROADMAP.md](../ROADMAP.md)).
+- **Rates lock:** every write to a property's rate plans, prices and restrictions first takes `rates::lock_rates` (a transaction advisory lock on the property), then reads the plan tree. A change to one plan rewrites the plans derived from it level by level, so writers in one property run one at a time instead of following a lock order over `rate_day` rows. Reads (grid, quote) take no lock.
 - **Startup RLS guard:** `serve` calls `db::assert_rls_applies(&pool)` and refuses to start as a superuser, a `BYPASSRLS` role or a role that owns (directly or through membership) a table in `public`.
 
 ## Optimistic concurrency
@@ -91,15 +92,16 @@ Request DTOs derive `garde::Validate` and are checked with `error::validate(&bod
 
 ## Change events
 
-After a successful write, in the same transaction, call `db::notify(&mut tx, &Event { tenant_id, property_id, keys })`. Keys name **TanStack Query keys** the client should invalidate (`"properties"`, `"room-types:<property>"`, `"rooms:<property>"` (rooms, sections and block reasons), `"inventory:<property>:<yyyy-mm>"` (one per month a change touches), later `"tape:<property>:<tileStart>"` …). Events never carry data. Choose keys so that a change refetches only screens that show it; the SPA's query keys start with the same string (`web/pms/src/lib/rooms.ts`, `inventory.ts`).
+After a successful write, in the same transaction, call `db::notify(&mut tx, &Event { tenant_id, property_id, keys })`. Keys name **TanStack Query keys** the client should invalidate (`"properties"`, `"room-types:<property>"`, `"rooms:<property>"` (rooms, sections and block reasons), `"inventory:<property>:<yyyy-mm>"` (one per month a change touches), `"rate-plans:<property>"` (rate plans, meal supplements and cancellation policies), `"rates:<property>:<plan>:<yyyy-mm>"` (one per plan and month a price or restriction change touches, derived plans included), later `"tape:<property>:<tileStart>"` …). Events never carry data. Choose keys so that a change refetches only screens that show it; the SPA's query keys start with the same string (`web/pms/src/lib/rooms.ts`, `inventory.ts`).
 
-Keep events small: `pg_notify` payloads must stay under 8000 bytes. Inventory month keys are clamped to the counter window (`[business date, business date + 730 days)`, `rooms::inventory::clamped_month_keys`, crate-internal), which caps a change at 25 month keys, and blocks may not end past the window. A new key family that grows with a date range needs the same kind of bound.
+Keep events small: `pg_notify` payloads must stay under 8000 bytes. Inventory month keys are clamped to the counter window (`[business date, business date + 730 days)`, `rooms::inventory::clamped_month_keys`, crate-internal), which caps a change at 25 month keys, and blocks may not end past the window. A new key family that grows with a date range needs the same kind of bound. Rate keys grow with the number of plans too (a change to a plan with many derived plans), so `rates` writes only inside the same 730-day window and sends its keys in as many events as it takes to keep each payload under 6000 bytes of keys.
 
 ## GraphQL
 
 - Schema in `crates/core-api/src/graphql.rs`: depth ≤ 8, complexity ≤ 500. Introspection returns `null` in production. Persisted-query allowlist from Phase 4.
 - Resolvers read `PgPool` and `TenantContext` from the request context, check the permission for the `propertyId` argument (`graphql::scoped`), and batch relations with DataLoaders once the first nested relation exists (Phase 1 lists are flat: rooms carry `roomTypeId`, blocks carry `roomId`).
-- Date-range queries are bounded: `inventory` spans at most 93 days, `blocks` at most 400.
+- Date-range queries are bounded: `inventory` and `rateGrid` span at most 93 days, `blocks` at most 400, a `quote` at most 90 nights; `bulkChangePreview` returns the first 50 changed cells and the total.
+- Enums shared with REST are mirrored as GraphQL enums (`graphql::mirror_enum!`); GraphQL spells values in capitals (`FIT_F`, `NON_RESIDENT`, `PERCENT`), REST as the database does (`FIT_F`, `non_resident`, `percent`).
 - Fields are camelCase; IDs are `UUID`; money is `{ amount: Int (minor units, as string if > 2^53), currency }`; dates are ISO `YYYY-MM-DD`.
 - Lists use cursor pagination (`first`, `after` → `{ nodes, pageInfo { endCursor, hasNextPage } }`) once they can exceed a few hundred rows (reservations, guests).
 
```

- [ ] **Step 2: Run the checks**

```sh
DATABASE_URL=$TEST_DATABASE_URL cargo test --workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
DATABASE_URL=$TEST_DATABASE_URL cargo test --release -p core-api --test perf -- --ignored --nocapture --test-threads=1
cd web/pms
bun run api:schemas && bun run codegen && git diff --exit-code -- src/lib/api
bun run lint && bun run check && bun run test && bun run build
bun run test:e2e
E2E_PERF=1 bun run test:e2e --grep @perf
```

Expected: 205 Rust tests pass (3 ignored: the gates, which pass when run as above; see the Verified section); no generated-type drift; 61 web tests; 8 Playwright tests; the `@perf` grid test passes.

- [ ] **Step 3: Commit**

```sh
git add -A
git commit -m "docs: Phase 2 events, rates lock, GraphQL bounds, performance gates, a manual test script and roadmap status"
```

## Running it locally

After the last task, to try Phase 2 by hand on your development database (as README.md "Trying rates by hand" describes):

```sh
export DATABASE_OWNER_URL=postgres://goodfolk_owner:goodfolk_owner_dev@localhost:5432/goodfolk
export DATABASE_URL=postgres://goodfolk_api:goodfolk_api_dev@localhost:5432/goodfolk
cargo run -p core-api -- migrate      # adds migrations 0005 and 0006
cargo run -p core-api                 # API on :8080
cd web/pms && bun run dev             # app on :5173 (restart a running dev server so it picks up the new routes)
```

Then follow the script: room types and rooms; plans `BAR` (standard, USD), `OTA` (BAR + 15 %, inheriting restrictions), `CORP` (custom), `FITF` (USD, non-residents) and `FITL` (LKR, residents, priced by hand); prices typed in the grid; next July set to 100 and its weekends +10 % with previews, and `OTA` following; restrictions on a July Saturday; BB and HB supplements; quotes that show the minimum stay, closed arrival, breakfast per person and the residency rule.

## Phase 2 done when

- A revenue manager creates standard, derived and custom plans tagged FIT-F, FIT-L, OTA, TA or IBE in their own currencies, with resident prices set by hand (Tasks 4, 5, 10, 14; e2e "a revenue manager builds a tree of rate plans").
- They edit prices in the grid and apply previewed bulk changes; derived plans update in the same save (Tasks 7, 10, 11, 15; e2e "prices are edited in the grid, changed in bulk and quoted"; `derivation.rs`).
- BB/HB/FB are sold as per-person supplements on plans that allow them (Tasks 6, 9, 16; e2e "meal supplements are set per currency and added to a quote per person").
- Restrictions are set per plan, room type and date and inherited by derived plans that ask for it (Tasks 8, 15).
- The gates hold (Task 11): grid p95 < 30 ms; bulk change < 300 ms (with the margin noted above).
- CI is green: fmt, clippy, Rust tests, cargo-deny, generated types, lint, svelte-check, web tests, build, e2e.

## Self-review

**Spec coverage.**

| Spec item | Where |
|---|---|
| Tables `rate_plan`, `rate_plan_room_type`, `rate_day`, `meal_supplement`, `cancellation_policy` (+ `rate_restriction`); isolation cases | Task 4 |
| Derived price = parent ± percent or amount, half-up to `rounding_step`; same currency; depth ≤ 3; no cycles | Tasks 4 (function), 5 (tree rules), 7 (repricing) |
| Recalculation in the same transaction, set-based per level; own restrictions unless `inherit_restrictions` | Tasks 7, 8 (Decision 9) |
| Occupancy pricing with fallback to the nearest lower occupancy plus the extra-adult amount | Tasks 5 (setting), 9 (quote) |
| Meal supplement = adults × adult + children × child; RO = 0; `allowed_meal_plans` | Tasks 4, 6, 9 |
| `quote(…)` pure, unit-tested, reused later; residency rule | Task 9 |
| REST routes (`rates.manage`) incl. prices, bulk change, restrictions, meal supplements, cancellation policies | Tasks 3, 10 |
| GraphQL `ratePlans` (tree), `rateGrid`, `quote` | Task 11 |
| Events `rate-plans:<p>`, `rates:<p>:<plan>:<yyyy-mm>` | Tasks 5–8 (emitted), 10 (tested), 13 (client keys) |
| UI: plan tree with badges and edit panel; rate grid (same component, plan selector, save on blur in batches, derived read-only with formula, bulk dialog with preview); meal supplements per currency | Tasks 14, 15, 16 |
| Tests: property-based derivation (percent, amount, negative, rounding, depth 3, cycles, currency); bulk change exactness and cascade; quote nights, supplements, min stay across the stay, CTA/CTD; isolation | Tasks 4, 5, 7, 9, 4 |
| Performance gates | Task 11 |
| Phase 1 carry-overs: concurrent throttle test, cross-tenant POST, ETag on replays and in OpenAPI, empty PATCH, rooms page busy flag, DateGrid snap-back | Tasks 1, 2, 12 |
| Not in this plan (spec: out of scope or later): channel mapping (Phase 8), taxes and the tax-inclusive flag (Phase 7), a cancellation policy screen (Phase 8, Decision 18) | — |

**Placeholder scan.** No "TBD", "similar to", or undefined names: every file is shown in full or as its exact diff, and each task's Interfaces list the names later tasks use.

**Type consistency.** Names were checked by compiling and running each task in order: for example `rates::bulk_change(tx, tenant, actor, property, plan, &BulkChange) -> u64` (Task 7) is what `routes::rates::bulk_change` (Task 10) calls, and the SPA's `ratesKey(p, plan, month)` produces the `rates:<p>:<plan>:<yyyy-mm>` strings `rates::rates_keys` emits.

## Execution handoff

Plan complete and saved to `docs/superpowers/plans/2026-09-25-phase-2-rates-meal-plans.md`. Two execution options:

1. **Subagent-driven (recommended):** a fresh subagent per task, with a review between tasks (superpowers:subagent-driven-development).
2. **Inline execution:** execute the tasks in one session with checkpoints (superpowers:executing-plans).
