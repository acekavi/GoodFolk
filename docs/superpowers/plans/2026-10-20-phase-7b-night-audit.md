# Phase 7b Night Audit, jobs-svc and Daily Statistics — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close a property's business day through a guided night-audit review and a checkpointed, crash-safe close
that posts the night, locks the day's rates, snapshots daily statistics and advances the business date. Do it by hand
or automatically at the audit time, from a new `jobs-svc` that also takes over every scheduled job from core-api.

**Architecture:**
- **A new crate `modules/audit`** holds:
  - the review queries;
  - the close engine (`audit_run` with insert-only step rows, each step committed with its own work);
  - the daily statistics;
  - the ops alerts.
- **`modules/reservations`** gains three commands: `mark_no_show`, `delay_arrival` and `extend_stay`.
- **`modules/folio`** gains a no-show fee and a dry-run preview of tonight's postings.
- **A new binary `crates/jobs-svc`**, a tonic and axum service like media-svc. It runs the close and every scheduled
  job behind svc-auth. Cloud Scheduler calls `POST /tick` every 15 minutes, and core-api calls `StartClose` and
  `RetryClose` over gRPC.
- **core-api** loses `tasks.rs` and wires the REST, GraphQL and SSE side.
- **The SPA** gains a Night audit page, an Audit history page, the audit time and an ops-alert banner.

**Tech Stack:**
- Rust 1.99, axum 0.8, sqlx 0.9, async-graphql 7.2, tonic 0.14 with protox.
- PostgreSQL 17 with forced RLS.
- SvelteKit 2 / Svelte 5, TanStack Query, Playwright, Bun.

**Spec:** [docs/specs/phase-7b-night-audit.md](../../specs/phase-7b-night-audit.md). It is the authority: read the
sections a task names. Design background: ARCHITECTURE.md §7.5. 7a's folio design: [phase-7-folio-audit.md](../../specs/phase-7-folio-audit.md).

**Execution method (the owner's standing preference):**
- Each task is implemented once on branch `phase-7b-night-audit`, then reviewed, with fix rounds.
- A final whole-branch review and one fix wave follow, then a manual testing guide.
- Code blocks are exact where they carry weight: SQL, signatures, constants, names and named tests.

## Global Constraints

- **Environment** (no Docker):
  - Postgres on 5432 (`DATABASE_URL=postgres://goodfolk_owner:goodfolk_owner_dev@localhost:5432/goodfolk`; start it
    with `~/.cache/goodfolk-pg/inst/bin/pg_ctl -D ~/.cache/goodfolk-pg/data -o "-p 5432 -k ''" -l ~/.cache/goodfolk-pg/log start`
    if it is down). There is no psql; use Bun SQL or a Rust test.
  - RustFS on :9000 (`deploy/dev/rustfs.sh start`).
  - The e2e DB is `goodfolk_e2e_p7b`, which the controller creates and migrates.
  - Never drop a database. Never `pkill -f`; stop only exact PIDs you started. Leave the owner's vite dev server
    alone. Use `CARGO_BUILD_JOBS=8`.
- **Green before every commit:**
  - **Rust tasks:** `cargo test --workspace` (in the background), `cargo fmt --all --check` and
    `cargo clippy --workspace --all-targets -- -D warnings`.
  - **Web tasks:** `bun run lint && bun run check && bun run test && bun run build`, plus the default e2e suite.
  - After an API change, run `bun run api:schemas && bun run codegen` in `web/pms` and commit the generated files.
  - Use Bun only, never npm.
- **Commits:** the exact messages given. No body, no trailer, no AI attribution. Never push.
- **Migrations** continue from `0031`. They are applied to `goodfolk` and `goodfolk_e2e_p7b` only. Every `security definer` function sets
  `search_path = pg_catalog, public, pg_temp` (the schema test enforces it).
- **Names (exact):**
  - Close steps, in order: `resolve`, `post_charges`, `lock_fx`, `snapshot_stats`, `advance_business_date`.
  - Run statuses: `running`, `failed`, `completed`. Run modes: `manual`, `auto`.
  - Ops alert kinds: `audit`, `drift`.
  - Statistics dimensions: `segment`, `source`, `residency`, `room_type`, `outlet`. The key for revenue no stay
    accounts for is `unattributed`.
  - Posting key of a no-show fee: `no_show:<reservation_room>`.
  - Event key: `audit:<property>`.
  - Audit log actions: `reservation_room.no_show`, `reservation_room.arrival_delayed`, `reservation_room.extended`,
    `night_audit.started`, `night_audit.retried`, `night_audit.completed`, `night_audit.failed`.
  - The advisory lock of a run is `pg_try_advisory_lock(hashtext('audit:' || <property>::text))`. A job's lock is
    `hashtext('job:' || <job>)`.
- **Permissions:** `NightAudit` (new) for owner, manager and front_desk. Everything else is unchanged.
- **Locking (spec "Locking"):**
  - Every read of `property.business_date` by a writer is `for share`.
  - `rooms::extend_window` takes that share lock before inserting.
  - The roll (`snapshot_stats` plus `advance_business_date`) takes `property` `for update` first in its transaction.
  - The rest of the order is unchanged: `reservation_room` → `room` → `room_condition` → `room_block` →
    `inventory_day` → `folio` → `property_counter`, with `reservation` last.
- **Money:**
  - Amounts are `bigint` minor units. Base amounts use `fx::to_base` (exact decimals, half-up), never floats.
  - Statistics revenue is **net of tax**, in base, on the **posting** business date. Lines with source `adjustment`
    are ledger moves, not revenue.
- **jobs-svc:**
  - Dev port 8091. Environment:
    - `DATABASE_URL`;
    - `DATABASE_DIRECT_URL` (a direct connection for session advisory locks; dev default `DATABASE_URL`);
    - `APP_ENV`;
    - `SVC_AUTH` and `INTERNAL_SHARED_SECRET` (svc-auth);
    - `JOBS_SVC_AUDIENCE`;
    - `JOBS_TICK=internal|off` (default `internal` outside production; production is `off`, driven by Cloud Scheduler);
    - `FX_IMPORT=on|off`;
    - the storage variables, for the media sweep.
  - core-api gains `JOBS_SVC_URL` (dev default `http://localhost:8091`) and `JOBS_SVC_AUDIENCE`.
- **Performance gates** (release builds; never raise a threshold):
  - the close for 200 rooms (160 in house, with POS bills and payments) < 10 s;
  - `nightAudit` for 200 rooms p95 < 100 ms;
  - every existing gate stays green.

## Review Focus

- **The desk keeps working during a close.** A reservation, posting or POS bill racing the roll either lands
  entirely in D's statistics or entirely on D + 1, never half of each, and nothing deadlocks. Test: Task 2 (racing a
  held `for update`) and Task 6 (racing the real roll).
- **A partly failed close is repaired, not redone.** A stay that fails to post fails the run with the stay named. Once
  the cause is fixed (a rate entered), **Retry** posts only what is missing and completes. Test: Task 6.
- **Double clicks and double ticks.** Two Close clicks, a Close plus a tick, or two ticks at once give one run and
  one execution. A second call returns the same run id. Test: Tasks 6 and 7.
- **Local time edges.** The audit moment is computed in the property's own time zone: a midnight `audit_time`, a 23:30
  one, a property created today, and a time zone with daylight saving (`Europe/London` across the October change).
  Test: Task 7.
- **A stale review.** An arrival resolved in another tab meanwhile makes the row action `412` and refetch, and the
  close re-checks the blocking count on the server whatever the screen showed. Test: Tasks 9 and 11.

---

## File structure

```
migrations/0031_night_audit.sql                  every new table, column, grant, policy and definer (Task 1)
modules/identity/src/rbac.rs                     NightAudit (Task 1)
crates/db/tests/{isolation.rs,night_audit_schema.rs}   isolation and grants (Task 1)
modules/{rooms,reservations,folio,housekeeping,fx}/src  business-date share locks, closed-day rates (Task 2)
modules/reservations/src/{no_show.rs,cancellation.rs,reservations.rs}   no-show, delay, extend, booked/cancelled dates (Task 3)
modules/folio/src/{payments.rs,posting.rs}       no-show fee; preview of tonight's postings (Tasks 3, 4)
modules/audit/src/{lib.rs,review.rs}             the review lists and blocking count (Task 4)
modules/audit/src/stats.rs                       the daily statistics (Task 5)
modules/audit/src/{close.rs,alerts.rs}           the close engine and ops alerts (Task 6)
proto/goodfolk/jobs/v1/jobs.proto, crates/proto  the jobs contract (Task 7)
crates/jobs-svc/                                 the service: close RPCs, tick, auto close, resume (Task 7); the
                                                 scheduled jobs moved from core-api plus cleanup and drift (Task 8)
Dockerfile.jobs-svc                              (Task 7)
crates/core-api/src/{tasks.rs (deleted),main.rs,config.rs}   (Task 8)
crates/core-api/src/{state.rs,routes/night_audit.rs,routes/reservations.rs,routes/properties.rs,graphql.rs,events.rs}  (Task 9)
crates/core-api/tests/{night_audit.rs,perf.rs}   (Tasks 9, 10)
web/pms src/lib/night-audit.ts, components/{AuditStepTracker,NoShowDialog}.svelte,
        routes (app)/p/[property]/{night-audit,night-audit/history}, property page, layout banner   (Task 11)
web/pms playwright.config.ts, tests/e2e/night-audit.spec.ts   (Task 12)
README.md, docs/{ROADMAP,ARCHITECTURE}.md, docs/design/{api-conventions,data-model}.md   (Task 13)
```

---

### Task 1: Schema, the NightAudit permission and isolation

**Files:**
- Create `migrations/0031_night_audit.sql`.
- Modify `modules/identity/src/rbac.rs`, plus every exhaustive permission list the compiler or tests name, such as the
  web `can()` map and `profile` permission names.
- Test: `crates/db/tests/isolation.rs` (new cases) and `crates/db/tests/night_audit_schema.rs` (new).

**Migration (exact where it matters).** Follow 0020's style: header comment, RLS, policies, grants. Lift FORCE RLS
around the backfill with `-- backfill: start` / `-- backfill: end`, as 0020 and 0027 do.

```sql
alter table property add column audit_time time not null default '03:00';

-- "On D" for statistics: the business date a reservation was booked, and the one a room was cancelled or marked a
-- no-show on. Timestamps can't say it: a business date is not a calendar date.
alter table reservation add column booked_business_date date;
alter table reservation_room add column cancelled_business_date date;
-- backfill: start
update reservation r set booked_business_date = (r.created_at at time zone p.timezone)::date
  from property p where p.id = r.property_id;
update reservation_room rr set cancelled_business_date = (rr.cancelled_at at time zone p.timezone)::date
  from property p where p.id = rr.property_id and rr.status = 'cancelled';
update reservation_room set cancelled_business_date = lower(stay) where status = 'no_show';
-- backfill: end
alter table reservation alter column booked_business_date set not null;
alter table reservation_room add constraint reservation_room_cancelled_business_date_check
  check ((status in ('cancelled', 'no_show')) = (cancelled_business_date is not null));

create table audit_run (
  id uuid primary key,
  tenant_id uuid not null,
  property_id uuid not null,
  business_date date not null,
  mode text not null check (mode in ('manual', 'auto')),
  status text not null check (status in ('running', 'failed', 'completed')),
  started_by uuid references app_user (id) on delete set null,
  attempts integer not null default 1 check (attempts between 1 and 1000),
  error text check (length(error) <= 2000),
  -- The failed step's report: {"step": ..., "stays": [{"reservation_room": ..., "room": ..., "error": ...}]}.
  failure jsonb,
  started_at timestamptz not null default now(),
  finished_at timestamptz,
  unique (property_id, business_date),
  unique (tenant_id, id),
  check ((status = 'running') = (finished_at is null)),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade
);

create table audit_run_step (
  tenant_id uuid not null,
  run_id uuid not null,
  step text not null
    check (step in ('resolve', 'post_charges', 'lock_fx', 'snapshot_stats', 'advance_business_date')),
  completed_at timestamptz not null default now(),
  detail jsonb not null default '{}',
  primary key (run_id, step),
  foreign key (tenant_id, run_id) references audit_run (tenant_id, id) on delete cascade
);

create table daily_stats (
  tenant_id uuid not null,
  property_id uuid not null,
  business_date date not null,
  rooms_total integer not null, rooms_out_of_order integer not null, rooms_available integer not null,
  rooms_sold integer not null, complimentary_rooms integer not null, occupancy_bp integer not null,
  adults integer not null, children integer not null, arrivals integer not null, departures integer not null,
  no_shows integer not null, cancellations integer not null, walk_ins integer not null,
  room_revenue bigint not null, meal_plan_revenue bigint not null, pos_revenue bigint not null,
  fee_revenue bigint not null, other_revenue bigint not null, adr bigint not null, revpar bigint not null,
  taxes jsonb not null, payments jsonb not null,
  taken_at timestamptz not null default now(),
  primary key (property_id, business_date),
  unique (tenant_id, property_id, business_date),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade
);

create table daily_stats_breakdown (
  tenant_id uuid not null,
  property_id uuid not null,
  business_date date not null,
  dimension text not null check (dimension in ('segment', 'source', 'residency', 'room_type', 'outlet')),
  key text not null check (length(key) between 1 and 100),
  room_nights integer not null,
  room_revenue bigint not null,
  revenue bigint not null,
  primary key (property_id, business_date, dimension, key),
  foreign key (tenant_id, property_id, business_date)
    references daily_stats (tenant_id, property_id, business_date) on delete cascade
);

create table ops_alert (
  tenant_id uuid not null,
  property_id uuid not null,
  kind text not null check (kind in ('audit', 'drift')),
  message text not null check (length(message) between 1 and 2000),
  raised_at timestamptz not null default now(),
  primary key (property_id, kind),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade
);

-- jobs-svc's memory between ticks: when a job (per key: a property, or '' for a global job) last succeeded.
-- No tenant data, so no RLS.
create table job_state (
  job text not null check (job in ('fx_import', 'media_sweep', 'window', 'cleanup', 'drift')),
  key text not null,
  done_for date,
  last_ok_at timestamptz,
  last_error text check (length(last_error) <= 2000),
  primary key (job, key)
);
```

- **RLS:** enable and force it, with the tenant policy exactly as 0020 writes it, on `audit_run`, `audit_run_step`,
  `daily_stats`, `daily_stats_breakdown` and `ops_alert`.
- **Indexes:** `audit_run (property_id, business_date desc)` and `audit_run (status) where status <> 'completed'`.
- **Grants to `goodfolk_app`:**
  - `audit_run`: select and insert, plus `update (status, attempts, error, failure, finished_at)`;
  - `audit_run_step`, `daily_stats` and `daily_stats_breakdown`: select and insert only;
  - `ops_alert`: select, insert, update and delete;
  - `job_state`: select, insert and update.
- **Definers.** Owner read policies follow 0022's `fx_import_read`; it already covers `property`. Add
  `create policy jobs_read on audit_run for select to current_user using (true);` and
  `create policy purge_idempotency on idempotency_key for delete to current_user using (true);`. Then:

```sql
create function app.jobs_properties()
returns table (tenant_id uuid, property_id uuid, business_date date, timezone text, audit_time time)
language sql stable security definer set search_path = pg_catalog, public, pg_temp
as $$ select p.tenant_id, p.id, p.business_date, p.timezone, p.audit_time from property p order by p.id $$;

-- Runs a tick must look at: running (to resume) or failed (to retry).
create function app.jobs_open_runs()
returns table (tenant_id uuid, run_id uuid, property_id uuid, status text, mode text, attempts integer)
language sql stable security definer set search_path = pg_catalog, public, pg_temp
as $$ select r.tenant_id, r.id, r.property_id, r.status, r.mode, r.attempts from audit_run r
      where r.status <> 'completed' order by r.id $$;

create function app.purge_expired(login_window_secs integer, idempotency_days integer)
returns table (sessions bigint, login_failures bigint, idempotency_keys bigint)
language plpgsql volatile security definer set search_path = pg_catalog, public, pg_temp
as $$
declare s bigint; l bigint; i bigint;
begin
  delete from session where expires_at < now(); get diagnostics s = row_count;
  delete from login_failure where at < now() - make_interval(secs => login_window_secs); get diagnostics l = row_count;
  delete from idempotency_key where created_at < now() - make_interval(days => idempotency_days);
  get diagnostics i = row_count;
  return query select s, l, i;
end $$;
```

Revoke all of the definer functions from public, and grant execute on them to `goodfolk_app`.

**Rust:**
- Add `Permission::NightAudit` with doc comment "Allows: run the night audit — the review, its actions' close, retry and history."
- Grant it to Manager and FrontDesk (Owner already has everything).
- Keep the web permission list in step with it (grep `folioAdjust` for every list).

- [ ] **Tests:**
  - **Isolation:** one case per new tenant table in `isolation.rs`, using `assert_table_isolated` and a seed variant.
  - **Schema**, in `night_audit_schema.rs`:
    - the app role cannot update `audit_run.business_date` or `mode`, and cannot update or delete `audit_run_step`,
      `daily_stats` or `daily_stats_breakdown`;
    - a second `audit_run` for the same property and date is refused;
    - `cancelled_business_date` is required exactly for cancelled and no-show rooms;
    - each definer returns rows across two tenants;
    - `purge_expired` deletes only rows past their window: an expired session, a fresh session, a login failure older
      than the window and a recent one, a 7-day-old idempotency key and a 6-day-old one.
  - **Backfill test** (pattern: `modules/folio/tests/backfill.rs`): seed a reservation and a cancelled room before the
    migration, re-run the extracted backfill SQL, and assert both dates.
  - **RBAC unit test:** the role matrix for `NightAudit`.
- [ ] Commit with the message `feat(db): night-audit runs, daily statistics, ops alerts and job state`.

### Task 2: Business-date share locks and closed-day exchange rates

The Phase 1 carry-over, plus spec "Locking" and step `lock_fx`'s refusals.

**Files:**
- Modify the `business_date` helpers:
  - `modules/rooms/src/inventory.rs:51`;
  - `modules/reservations/src/lib.rs:133`;
  - `modules/folio/src/lines.rs:574`;
  - `modules/housekeeping/src/assignments.rs:150`.
- Modify every other writer query that reads `p.business_date` inside a write transaction. Find them with
  `grep -rn "business_date" modules/*/src`. For each, either add `for share of p`, or call the helper first in the
  same transaction.
- Modify `modules/rooms/src/inventory.rs` `extend_window`: run
  `select 1 from property where id = $1 for share` before the insert.
- Modify `modules/fx/src/lib.rs`, in `set_manual` and `import_one`.

**Rules:**
- **Every helper becomes:** `select business_date from property where id = $1 for share`. Reads that are not writes
  (GraphQL queries) stay unlocked.
- **`fx::set_manual`** reads the property's business date `for share` and refuses `date < business_date` with a new
  `FxError::Conflict("That day is closed")`, which maps to 409 in core-api (add the arm where `FxError` maps).
- **`fx::import`'s `import_one`** skips, without an alert and with an `info` log, a property whose business date is
  after `today`. Its rates for `today` would land on a closed day.
- **Document the rule** in the doc comment of each helper: one sentence pointing to "Locking" in the 7b spec.

- [ ] **Tests:**
  - **`modules/rooms/tests`:** a writer blocks behind a roll. Session A begins and runs
    `select … from property where id = $1 for update`. Session B starts a real `create_block` (or any counter writer)
    and does not finish within 300 ms. A updates `business_date` and commits, and B then completes using the **new**
    business date: its counter rows begin at D + 1, and no row for D is changed.
  - **Same pattern for `extend_window`:** once A commits, no counter row has been inserted from the old window end.
  - **`modules/fx/tests`:**
    - `set_manual` for yesterday, when the business date is today, is a `Conflict`;
    - for today it succeeds;
    - `import` with `today` before a property's business date stores nothing for it and raises no alert.
- [ ] Commit with the message `fix: writers read the business date for share, and closed days keep their rates`.

### Task 3: No-show, delay arrival and extend stay

Spec "The review", lists 1 and 2.

**Files:**
- Create `modules/reservations/src/no_show.rs`, holding all three commands.
- Modify:
  - `cancellation.rs`: extract the penalty evaluation; set `cancelled_business_date`; take `actor: Option<UserId>`.
  - `reservations.rs`: `create_reservation` sets `booked_business_date`, the business date read in its transaction.
  - `lib.rs`: export the new commands; `audit` takes `Option<UserId>`, and existing callers pass `Some(actor)`.
- Modify `modules/folio/src/payments.rs`: generalise the cancellation fee.
- Tests: `modules/reservations/tests/no_show.rs`, and `modules/folio/tests/payments.rs` (one no-show fee case).

**Interfaces (exact):**
```rust
// modules/reservations/src/cancellation.rs
/// What `penalty` costs against `nights` (room, meal): `nights` = the first N nights' room amounts, `percent` = basis
/// points of the stay's total, half up, `amount` = the amount; never above the stay's total. Shared by cancelling
/// and no-show.
pub(crate) fn penalty_amount(penalty: Penalty, nights: &[(Date, i64, i64)]) -> i64;
pub async fn cancel_room(tx, tenant, actor: Option<UserId>, property, id, expected_version) -> Result<CancelledRoom, ReservationsError>;

// modules/reservations/src/no_show.rs
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct NoShowRoom { pub id: Uuid, pub reservation_id: Uuid, pub version: i32, pub penalty: i64, pub currency: String }
/// Confirmed only (domain transition), arrival ≤ business date. Status no_show, cancelled_business_date = business
/// date, sold released from max(arrival, business date) to departure, penalty = penalty_amount(terms.no_show, nights)
/// (0 without terms) posted by folio::post_no_show_fee. Audits reservation_room.no_show {reservation_id, penalty,
/// currency, mode}. Notifies as cancel_room does.
pub async fn mark_no_show(tx: &mut Tx, tenant: TenantId, actor: Option<UserId>, property: Uuid, id: Uuid,
    expected_version: i32) -> Result<NoShowRoom, ReservationsError>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct ChangedStay { pub id: Uuid, pub reservation_id: Uuid, pub version: i32, pub check_in: Date, pub check_out: Date, pub total: i64, pub currency: String }
/// Tentative or confirmed, arrival ≤ business date D, departure > D + 1 (else Conflict "Change the dates with
/// Modify"). The stay becomes [D + 1, departure): reservation_night rows before D + 1 are deleted, sold released for
/// D only (earlier days are history). The room stays assigned. Audits reservation_room.arrival_delayed {from, to}.
pub async fn delay_arrival(tx, tenant, actor: Option<UserId>, property, id, expected_version) -> Result<ChangedStay, ReservationsError>;

/// Checked in with departure == business date D (else Conflict). Adds night D + 1 at the last night's room and meal
/// amounts (no quote, no restrictions): lock order reservation_room → room → rooms::lock_days(type, D+1, D+2); the
/// night must be sellable (SELLABLE, else Conflict "No room of this type is free on <D+1>"); the room must be free
/// and unblocked on D + 1 (the no-double-booking constraint under a savepoint, and the block check assign_room
/// uses: Conflict naming the booking or block). sold += 1 for D + 1; stay upper + 1. Audits reservation_room.extended.
pub async fn extend_stay(tx, tenant, actor: Option<UserId>, property, id, expected_version) -> Result<ChangedStay, ReservationsError>;

// modules/folio/src/payments.rs — post_cancellation_fee becomes a wrapper of:
pub enum FeeKind { Cancellation, NoShow }   // source cancellation_fee / no_show_fee; key cancellation:<rr> / no_show:<rr>; description "Cancellation fee" / "No-show fee"
pub async fn post_stay_fee(tx, tenant, user: Option<UserId>, property, reservation_room, kind: FeeKind, amount: i64, currency: &str, inclusive: bool) -> Result<(), FolioError>;
pub async fn post_no_show_fee(/* same as post_cancellation_fee */) -> Result<(), FolioError>; // = post_stay_fee(.., FeeKind::NoShow, ..)
```

- **Lock order** is the same as `cancel_room`: the row `for update`, the business date (now `for share`), the
  `lock_days`, then the folio, then `reservation`, whose version bumps.
- **The audit `data`** carries `"mode": "manual"` for a person, and `"auto"` when `actor` is `None`.

- [ ] **Tests (`no_show.rs`):**
  - **A no-show:**
    - releases every night's `sold`;
    - posts a `no_show_fee` line on the deposit folio with its taxes, inclusive when the plan's prices are;
    - a replay of the posting adds nothing;
    - the deposit stays on the deposit folio.
  - **Penalty kinds and refusals:**
    - a `nights: 1` no-show costs the first night's room amount; `percent: 5000` costs half the total;
    - no terms costs 0, and no line is posted;
    - a no-show of a checked-in room, or of a room arriving tomorrow, is a `Conflict`;
    - a wrong version is a `VersionMismatch`.
  - **Delay arrival:**
    - moves a 3-night stay to `[D + 1, D + 3)`: the night D row is gone, D's `sold` is down by one, and the later
      days are unchanged;
    - a 1-night stay is refused;
    - a tentative room can be delayed.
  - **Extend stay:**
    - adds a night at the last night's amounts and takes `sold` for D + 1;
    - a full room type on D + 1 is refused with the message;
    - the room booked by another stay from D + 1 is refused naming it;
    - a room blocked on D + 1 is refused;
    - a stay departing tomorrow is refused.
  - **Booking dates:**
    - `create_reservation` stores `booked_business_date`;
    - `cancel_room` stores `cancelled_business_date`.
- [ ] Commit with the message `feat(reservations): no-show with its fee, delayed arrival and a one-night extension`.

### Task 4: The review lists

Spec "The review".

**Files:**
- Create `modules/audit/{Cargo.toml,src/lib.rs,src/review.rs}` and add `audit = { path = "modules/audit" }` to the
  workspace.
- Modify `modules/folio/src/posting.rs`: add a preview that shares `in_house` and `nights` (give `in_house` a `&mut Tx`
  variant rather than copying its SQL).
- Test: `modules/audit/tests/review.rs`, with a `tests/common` seeded the way `modules/folio/tests/common` seeds stays.

**Interfaces (exact):**
```rust
// modules/folio/src/posting.rs
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NightPreview { pub reservation_room: Uuid, pub room_number: Option<String>, pub date: Date,
    pub room_amount: i64, pub meal_amount: i64, pub currency: String,
    /// Why posting would fail: "No room assigned", "No exchange rate for USD on <date>", "No price for <date>".
    pub problem: Option<String>,
    /// Charges of this night already posted (their keys used): 0, 1 or 2.
    pub already_posted: u8 }
/// What post_business_date_charges(date) would post, without posting: the same stays and nights, the same checks.
pub async fn preview_business_date_charges(tx: &mut Tx, property: Uuid, date: Date) -> Result<Vec<NightPreview>, FolioError>;

// modules/audit/src/lib.rs
#[derive(Debug, thiserror::Error)]
pub enum AuditError { #[error("{0} not found")] NotFound(&'static str), #[error("{0}")] Conflict(String),
    #[error("{0}")] Invalid(String), #[error(transparent)] Database(#[from] sqlx::Error),
    #[error(transparent)] Folio(#[from] folio::FolioError), #[error(transparent)] Reservations(#[from] reservations::ReservationsError),
    /// Only from a test's CloseHooks: the run stops as if the process died (left `running`).
    #[error("interrupted")] Interrupted }
pub fn audit_key(property: Uuid) -> String { format!("audit:{property}") }

// modules/audit/src/review.rs
pub struct NightAudit { pub business_date: Date, pub blocking: u32, pub arrivals: Vec<ArrivalRow>,
    pub departures: Vec<DepartureRow>, pub pos_bills: Vec<OutletBills>, pub booking_bills: BookingBills,
    pub payments: Vec<PaymentGroup> }
pub struct ArrivalRow { pub reservation_room: Uuid, pub reservation_id: Uuid, pub confirmation: String,
    pub guest: String, pub room_number: Option<String>, pub arrival: Date, pub departure: Date, pub status: RoomStatus,
    pub total: i64, pub currency: String, pub no_show_penalty: i64, pub version: i32 }
pub struct DepartureRow { pub reservation_room: Uuid, pub reservation_id: Uuid, pub confirmation: String,
    pub guest: String, pub room_number: Option<String>, pub departure: Date,
    /// The guest folio's balance per currency.
    pub balances: Vec<(String, i64)>, pub version: i32 }
pub struct OutletBills { pub outlet: String, pub bills: Vec<PosBillRow>, pub total_base: i64 }
pub struct PosBillRow { pub id: Uuid, pub bill_no: String, pub currency: String, pub total: i64, pub total_base: i64,
    /// Room number of the stay it posted to; None when settled at the outlet.
    pub room_number: Option<String>, pub settled: bool,
    /// Posted to a stay now checked out whose guest folio is not settled.
    pub highlight: bool }
pub struct BookingBills { pub nights: Vec<folio::NightPreview>, pub posted: Vec<CategoryTotal> }
pub struct CategoryTotal { pub category: String, pub lines: u32, pub total_base: i64 }   // manual and fee lines of D
pub struct PaymentGroup { pub method: String, pub currency: String, pub count: u32, pub amount: i64,
    pub base_amount: i64, pub refunds: i64, pub flagged: Vec<Uuid> }
/// Arrivals: tentative/confirmed with arrival <= D. Departures: checked_in with departure <= D.
pub async fn blocking(tx: &mut Tx, property: Uuid, date: Date) -> Result<u32, AuditError>;
pub async fn night_audit(tx: &mut Tx, property: Uuid) -> Result<NightAudit, AuditError>;
```

**Rules:**
- **Ordering and the penalty.** Arrivals and departures are ordered by room number, then confirmation.
  `no_show_penalty` uses `reservations::no_show_penalty(terms, nights)`; export the pure function from Task 3's code.
- **POS bill totals.** A bill's `total` is the sum of the totals of its lines: its folio lines (posting keys
  `pos:<bill>:%`) for a posted bill, or its `lines` jsonb for a settled one (read `modules/folio/src/pos.rs` for the
  shape).
- **Payments.**
  - D's payments are those whose folio is in the property, plus POS payments whose `pos_bill.property_id` is it.
    `transfer` is left out.
  - `account` is shown under the method `account`, which the UI labels "City ledger".
  - `flagged` lists LKR payments, refunds excluded, with empty `evidence` on a folio whose stay's rate plan segment
    is `FIT_F`.
- **Bounded queries.** Each list is one statement, bounded by the stored leakproof columns (`arrival`, `nights`), as
  `posting::in_house` explains. Use `EXPLAIN` in a test to show that arrivals and departures use
  `reservation_room_arrival_idx`.

- [ ] **Tests:**
  - **Seeded day:** one each of
    - a confirmed arrival, a tentative arrival, an overdue confirmed arrival (arrival D − 1);
    - a due departure, an in-house stay;
    - a posted POS bill, a settled POS bill;
    - a cash payment, a USD card payment, a refund;
    - an LKR payment without evidence on a FIT_F stay.
  - **Every list is exact.** `blocking` is 4: three arrivals and one departure.
  - **The preview:**
    - it equals what `post_business_date_charges` then posts;
    - after posting, it shows `already_posted: 2` and posting adds nothing;
    - a USD stay with no rate shows the problem.
  - **The highlight:** a POS bill on a stay since checked out with a balance is highlighted.
  - **Isolation:** another property's rows never appear.
- [ ] Commit with the message `feat(audit): the night-audit review lists and their blocking count`.

### Task 5: Daily statistics

Spec "Daily statistics".

**Files:**
- Create `modules/audit/src/stats.rs`.
- Test: `modules/audit/tests/stats.rs`.

**Interfaces (exact):**
```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DailyStats { pub business_date: Date, pub rooms_total: i32, pub rooms_out_of_order: i32,
    pub rooms_available: i32, pub rooms_sold: i32, pub complimentary_rooms: i32, pub occupancy_bp: i32,
    pub adults: i32, pub children: i32, pub arrivals: i32, pub departures: i32, pub no_shows: i32,
    pub cancellations: i32, pub walk_ins: i32, pub room_revenue: i64, pub meal_plan_revenue: i64,
    pub pos_revenue: i64, pub fee_revenue: i64, pub other_revenue: i64, pub adr: i64, pub revpar: i64,
    pub taxes: BTreeMap<String, i64>, pub payments: BTreeMap<String, i64>, pub breakdown: Vec<BreakdownRow> }
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BreakdownRow { pub dimension: String, pub key: String, pub room_nights: i32, pub room_revenue: i64, pub revenue: i64 }
/// Computes D's statistics (no writes). Called inside the roll's transaction.
pub async fn compute(tx: &mut Tx, property: Uuid, date: Date) -> Result<DailyStats, AuditError>;
/// Inserts `stats` (row + breakdown). A second insert for the date is a unique violation — the caller's
/// step row makes that unreachable.
pub async fn store(tx: &mut Tx, tenant: TenantId, property: Uuid, stats: &DailyStats) -> Result<(), AuditError>;
pub async fn daily_stats(tx: &mut Tx, property: Uuid, from: Date, to: Date) -> Result<Vec<DailyStats>, AuditError>; // ≤ 366 days, else Invalid
```

**Definitions (exact):**
- **Rooms:**
  - `rooms_total`: active rooms of the property;
  - `rooms_out_of_order`: active rooms with an unreleased `out_of_order` block covering D;
  - `rooms_available = total − out_of_order`.
- **Sold rooms:**
  - `rooms_sold`: `checked_in` stays in house on D (`arrival ≤ D < arrival + nights`), the same set `in_house` returns;
  - `complimentary_rooms`: of those, the ones whose `reservation_night` for D has `room_amount = 0`;
  - `adults` and `children`: sums over the sold stays.
- **Occupancy, ADR and RevPAR:**
  - `occupancy_bp = rooms_sold × 10000 / rooms_available`, half-up, 0 when nothing is available;
  - `adr = room_revenue / rooms_sold` and `revpar = room_revenue / rooms_available`, both half-up, 0 on a 0 divisor.
- **Counts:**
  - `arrivals`: `checked_in_business_date = D`;
  - `departures`: `checked_out` stays with `upper(stay) = D`;
  - `no_shows`: `status = 'no_show'` and `cancelled_business_date = D`;
  - `cancellations`: `status = 'cancelled'` and `cancelled_business_date = D`;
  - `walk_ins`: arrivals whose reservation's `booked_business_date = D`.
- **Revenue.** Take every `folio_line` on a folio of the property with `business_date = D` and source not
  `adjustment`.
  - **Net base:** a line's net in base is `base_total − Σ to_base(on_bill tax amount)`, converted at the line's
    `rate`, with the exponents from `fx::exponent`. Use `fx::to_base`.
  - **Categories:** `room` goes to room revenue, `meal_plan` to meal-plan revenue, `pos:*` to POS revenue, `fee` to fee
    revenue, and everything else to other revenue.
  - **Settled POS bills:** add their item lines (from `pos_bill.lines`) to POS revenue, net, converted the same way.
- **`taxes`:** code → Σ `to_base(amount)` over D's lines, on-bill and absorbed alike, with settled POS bills included.
- **`payments`:** method → Σ `base_amount` over D's payments, refunds included and `transfer` excluded.
- **Breakdown:**
  - **Attribution.** A room or meal-plan line belongs to the stay its posting key names. For a reversal, use the key of
    the line it `reverses`. For a transfer's target line, use the key of the line the sibling reversal (same
    `transfer_id`) reverses.
  - **Per stay dimension** (`segment`, `source`, `residency`, `room_type`): `room_nights` counts the sold stays,
    `room_revenue` is the attributed room revenue, and `revenue` is the attributed room plus meal-plan revenue.
    Anything not attributed goes under key `unattributed`.
  - **`outlet`:** key is the outlet code, `revenue` is POS revenue, and `room_nights` and `room_revenue` are 0.
- **Invariants:**
  - per stay dimension, Σ `room_nights` = `rooms_sold`, Σ `room_revenue` = `room_revenue`, and Σ `revenue` =
    `room_revenue + meal_plan_revenue`;
  - Σ outlet `revenue` = `pos_revenue`.

**Shape of the queries:** a handful of grouped statements, then the arithmetic in Rust. Lines of D are fetched once,
with their taxes, by `business_date` through an index. Add `create index folio_line_business_date_idx on folio_line
(business_date)` in a new migration `0032_folio_line_business_date.sql` if `EXPLAIN` shows a sequential scan at the
perf seed.

- [ ] **Tests:**
  - **Golden day.** Seed one property with:
    - 10 rooms, one of them out of order;
    - 4 sold stays: LKR FIT_L, USD FIT_F at a rate of 300.000000, an OTA stay, and one complimentary;
    - a walk-in;
    - a no-show with a fee;
    - a cancellation;
    - a check-out;
    - a POS bill to a room and a settled bill at another outlet;
    - a reversal of a room line;
    - a transfer of a meal line to an account folio;
    - a charge to account;
    - payments by cash, card and refund.

    Assert every field of `compute` exactly. Write the expected figures as constants, with a comment deriving each
    revenue figure from its inputs.
  - **Invariants:** every invariant above holds on the golden day and on a randomised day (seeded `StdRng`, 20 stays).
  - **Store and read back:** `store` then `daily_stats` round-trips. A range over 366 days is `Invalid`.
  - **Zero days:** a day with nothing gives zeros, not division errors.
- [ ] Commit with the message `feat(audit): daily statistics with breakdowns that add up`.

### Task 6: The close engine and ops alerts

Spec "The close". The heart of the phase.

**Files:**
- Create `modules/audit/src/{close.rs,alerts.rs}`.
- Tests: `modules/audit/tests/{close.rs,resilience.rs}`.

**Interfaces (exact):**
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)] #[serde(rename_all = "snake_case")]
pub enum Step { Resolve, PostCharges, LockFx, SnapshotStats, AdvanceBusinessDate }   // as_str/parse, ALL in order
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)] #[serde(rename_all = "snake_case")]
pub enum Mode { Manual, Auto }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)] #[serde(rename_all = "snake_case")]
pub enum RunStatus { Running, Failed, Completed }
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AuditRun { pub id: Uuid, pub property_id: Uuid, pub business_date: Date, pub mode: Mode,
    pub status: RunStatus, pub started_by: Option<Uuid>, pub attempts: i32, pub error: Option<String>,
    pub failure: Option<serde_json::Value>, pub started_at: OffsetDateTime, pub finished_at: Option<OffsetDateTime>,
    pub steps: Vec<RunStep> }
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RunStep { pub step: Step, pub completed_at: OffsetDateTime, pub detail: serde_json::Value }

/// Test seam: called around each step's commit. Production passes &NoHooks.
pub trait CloseHooks: Send + Sync {
    fn before_commit(&self, _step: Step) -> Result<(), AuditError> { Ok(()) }
    fn after_commit(&self, _step: Step) -> Result<(), AuditError> { Ok(()) }
}
pub struct NoHooks;
impl CloseHooks for NoHooks {}

/// The run for the property's current business date: returns the existing one (any status), or inserts a new
/// `running` one. Manual mode refuses (Conflict "Resolve every arrival and departure first (<n> left)") while
/// review::blocking > 0 and no run exists. Audits night_audit.started, notifies audit_key.
pub async fn start_run(pool: &PgPool, tenant: TenantId, user: Option<UserId>, property: Uuid, mode: Mode) -> Result<AuditRun, AuditError>;
/// failed → running (attempts + 1, error/failure cleared, finished_at null). Conflict unless failed.
/// Audits night_audit.retried.
pub async fn retry_run(pool: &PgPool, tenant: TenantId, user: Option<UserId>, run: Uuid) -> Result<AuditRun, AuditError>;
/// Runs the remaining steps of a `running` run under its advisory lock, taken on a fresh connection to `lock`.
/// Ok(None) when another session holds the lock. A step error marks the run failed (error, failure, finished_at,
/// audits night_audit.failed, raises ops_alert audit when mode is auto and attempts >= 3) and returns the run;
/// AuditError::Interrupted is returned as is, leaving the run running.
pub async fn execute(pool: &PgPool, lock: &PgConnectOptions, tenant: TenantId, run: Uuid, hooks: &dyn CloseHooks) -> Result<Option<AuditRun>, AuditError>;
pub async fn get_run(tx: &mut Tx, run: Uuid) -> Result<AuditRun, AuditError>;
pub async fn list_runs(tx: &mut Tx, property: Uuid, first: u32, after: Option<Date>) -> Result<Vec<AuditRun>, AuditError>; // newest first, first ≤ 100

// alerts.rs
pub enum AlertKind { Audit, Drift }
pub struct OpsAlert { pub kind: AlertKind, pub message: String, pub raised_at: OffsetDateTime }
pub async fn raise(tx, tenant, property, kind, message: &str) -> Result<(), sqlx::Error>;   // upsert, notifies audit_key
pub async fn clear(tx, property, kind) -> Result<(), sqlx::Error>;                          // notifies when a row went
pub async fn list(tx, property) -> Result<Vec<OpsAlert>, sqlx::Error>;
```

**Steps.** Each runs in its own transaction(s). Each first checks whether its step row exists, and skips if so. Its
final transaction inserts the step row (with `detail`), and calls `hooks.before_commit` before the commit and
`hooks.after_commit` after it. It notifies `audit_key`.

1. **`resolve`.**
   - **Manual:** one transaction checks that `blocking == 0` (else a step error, "Resolve every arrival and departure
     first (<n> left)") and inserts the row.
   - **Auto:** for each blocking row, in its own transaction:
     - a confirmed arrival gets `mark_no_show`;
     - a tentative arrival gets `cancel_room`;
     - a departure with `departure == D` gets `extend_stay`;
     - an overdue departure (`departure < D`) is a step error naming the room.

     Every action is at its current version with `actor: None`. Then the step row, with detail
     `{no_shows, cancelled, extended}`. A refused action is a step error naming the room and the reason.
2. **`post_charges`.** `folio::post_business_date_charges(pool, tenant, None, property, D)`.
   - **Failures:** a non-empty `failures` is a step error whose `failure` is
     `{"step":"post_charges","stays":[{reservation_room, room, error}]}` and whose `error` is
     "<n> stays could not be posted".
   - **Otherwise** the step row, with detail `{stays, lines, skipped_existing}`.
3. **`lock_fx`.** One transaction. For each accepted currency other than the base, use `fx::rate_for(D)`. When the rate
   is from an earlier date, insert it as `(D, currency, rate, source, source_date)` with `on conflict do nothing`. The
   detail lists what it locked.
4. **The roll** (`snapshot_stats` plus `advance_business_date`), one transaction:
   - `select business_date from property where id = $1 for update`. If it is not D, that is a step error ("The business
     date moved to <x>").
   - `stats::compute` then `stats::store`.
   - `update property set business_date = D + 1`.
   - `rooms::extend_window`.
   - Insert both step rows.
   - Mark the run `completed` (`finished_at = now()`), audit `night_audit.completed` with the stats headline,
     `alerts::clear(Audit)`, and notify `audit_key`, `PROPERTIES_KEY` and the reservations list key.

**Rules:**
- A completed run is a no-op for `execute`.
- `start_run` for a date whose run is completed returns it unchanged.
- An `execute` that finds the lock taken returns `None` and touches nothing.

- [ ] **Tests (`close.rs`):**
  - **Manual happy path:** every step row exists, the stats row exists, the business date is D + 1, the run is
    `completed`, and the audit and notify keys are right.
  - **Refusals:**
    - a manual start with an unresolved arrival is refused, and no run is inserted;
    - the auto path resolves arrivals and departures as specified;
    - an auto extension refused (the room is taken on D + 1) fails the run naming the room.
  - **Missing rate, then retry:**
    - a USD stay with no rate fails `post_charges` and lists the stay;
    - after `fx::set_manual`, `retry_run` plus `execute` posts only the missing night and completes;
    - `attempts` is 2.
  - **Auto alert:** an auto run failing for the third attempt raises the `audit` ops alert, and a later completion
    clears it.
  - **Concurrency:**
    - two concurrent `start_run` calls give one run id;
    - two concurrent `execute` calls: one returns `None`, and every line is posted once.
  - **The roll versus a racing writer:** a `cancel_room` racing the roll's transaction (start `execute` with a hook
    that pauses in `before_commit(SnapshotStats)` on a `tokio::sync::Barrier`, run the cancel, release) is counted in
    exactly one of D's statistics or D + 1's, never lost.
  - **Closed rates:** after the close, `fx::set_manual` for D is a `Conflict`.
- [ ] **Tests (`resilience.rs`):** for every step, and for both `before_commit` (a crash mid-step) and `after_commit`
  (a crash between steps), a hook returns `Interrupted` once.
  - Assert the run is still `running`. Then `execute` again with `NoHooks`, which completes.
  - Then assert:
    - every posting key is used once;
    - there is one `daily_stats` row;
    - the business date is D + 1, exactly once;
    - no line is duplicated.
  - A third `execute`, and a `start_run` for the new date's predecessor, changes nothing.
- [ ] Commit with the message `feat(audit): the checkpointed, resumable night-audit close`.

### Task 7: jobs-svc: the service, the close RPCs and the tick

Spec "jobs-svc".

**Files:**
- Create `proto/goodfolk/jobs/v1/jobs.proto`.
- Modify `crates/proto/{build.rs,src/lib.rs}`: compile both protos and add `pub mod jobs`.
- Create `crates/jobs-svc/{Cargo.toml,src/{lib.rs,main.rs,config.rs,grpc.rs,tick.rs,due.rs}}`.
- Create `Dockerfile.jobs-svc`, modelled on `Dockerfile.media-svc` without libjxl.
- Tests: `crates/jobs-svc/tests/{close.rs,kill.rs,auth.rs}`, and unit tests in `due.rs`.

**Proto (exact):**
```proto
syntax = "proto3";
package goodfolk.jobs.v1;

// Scheduled work and the night-audit close. Every call needs service auth.
service Jobs {
  // Runs every due job once: auto closes, resumes, then the daily jobs.
  rpc Tick(TickRequest) returns (TickResponse);
  // Creates or returns the close of the property's business date and runs it in the background.
  rpc StartClose(StartCloseRequest) returns (CloseResponse);
  // Retries a failed close and runs it in the background.
  rpc RetryClose(RetryCloseRequest) returns (CloseResponse);
}

message TickRequest {}
message TickResponse { repeated JobOutcome jobs = 1; }
message JobOutcome {
  string job = 1;      // "auto_close", "resume", "fx_import", "media_sweep", "window", "cleanup", "drift"
  string key = 2;      // the property id, or "" for a global job
  string outcome = 3;  // "ran", "skipped" (lock held / not due), or "failed: <message>"
}
message StartCloseRequest { string tenant_id = 1; string property_id = 2; string user_id = 3; }
message RetryCloseRequest { string tenant_id = 1; string run_id = 2; string user_id = 3; }
message CloseResponse { string run_id = 1; }
```

**Service:**
- **`JobsSvc`** holds `{ pool, lock: PgConnectOptions, storage: Option<Arc<Storage>>, fx_source: Option<Arc<dyn RateSource>>, tracker: tokio_util::task::TaskTracker }`.
- **`router(svc, verifier)`:** the tonic `JobsServer` behind `svc_auth::layer`, with `grpc_unauthenticated` mapping as
  in media-svc; `POST /tick` (JSON `TickResponse` fields) behind the same layer; `GET /healthz` open.
- **`StartClose`:** parse the ids, `audit::start_run(.., Mode::Manual)`, then
  `tracker.spawn(execute(.., &NoHooks))`, and return the run id. A completed run is returned without spawning.
- **`RetryClose`:** `retry_run`, then spawn the same way.
- **Error mapping:** `Conflict` → `FAILED_PRECONDITION` (with the message), `NotFound` → `NOT_FOUND`, `Invalid` →
  `INVALID_ARGUMENT`, others → `INTERNAL` (logged).
- **`main.rs`:** like media-svc's. On shutdown, `tracker.close(); tracker.wait()` for up to 30 s.
- **`JOBS_TICK=internal`** runs `tick` every 15 minutes on a `tokio::time::interval`.

**`tick(svc) -> TickResponse`**, in order:
1. **Open runs.** From `app.jobs_open_runs()`:
   - `running` → `execute`, which returns `None` if another session is executing it;
   - `failed` with mode `auto` and `attempts < 3` → `retry_run(None)` then `execute`.
2. **Auto closes.** For each row of `app.jobs_properties()` with `due::audit_due(local_now, business_date, audit_time)`
   and no run for that date → `start_run(Auto)` then `execute`. `local_now` is
   `select now() at time zone $tz`, from the database's clock.
3. **The daily jobs** of Task 8.

Each job's `JobOutcome` is recorded, and one job's failure never stops the others. Auto closes run one property at a
time; the 10 s gate keeps 15 minutes ample.

**`due.rs` (pure, exact):**
```rust
/// The audit moment of business date `d`: d at `audit_time` when it is 12:00 or later, else d + 1 at `audit_time`.
pub fn audit_moment(d: Date, audit_time: Time) -> PrimitiveDateTime;
pub fn audit_due(local_now: PrimitiveDateTime, d: Date, audit_time: Time) -> bool { local_now >= audit_moment(d, audit_time) }
```

- [ ] **Tests:**
  - **`due.rs` units:** 03:00 is due on D + 1 at 03:00 and not at 02:59; 23:30 is due on D at 23:30; 00:00 is due on
    D + 1 at 00:00; 12:00 is due on D at 12:00.
  - **`tick.rs` (database):**
    - a property in `Europe/London` with `audit_time` 01:30 on the October daylight-saving change day is due exactly
      once. Fake `local_now` through a `Clock` parameter of `tick_with(svc, now_for: impl Fn(&str) -> PrimitiveDateTime)`;
      production uses the database clock;
    - a property created today is not due until tomorrow's moment.
  - **`close.rs`** (in-process router on a random port with a shared secret, as core-api's `tests/common/media.rs`
    does):
    - `StartClose` returns a run id and the run completes;
    - a second `StartClose` returns the same id;
    - `RetryClose` on a completed run is `FAILED_PRECONDITION`;
    - concurrent `Tick` plus `StartClose` give one run, executed once;
    - an auto close at the due moment completes with mode `auto`;
    - a `running` run left by an interrupted `execute` (a hook) is resumed by `Tick`.
  - **`auth.rs`:** no token gives `UNAUTHENTICATED` on gRPC and 401 on `/tick`; `/healthz` is open.
  - **`kill.rs`:**
    - spawn the built `jobs-svc` binary (located as `media_svc_exe` locates its binary) with `JOBS_TICK=off` on a
      seeded 120-stay property;
    - call `StartClose`;
    - poll until the `resolve` step row exists, then send SIGKILL (`rustix::process::kill_process`);
    - start a new `jobs-svc` and call `Tick`;
    - the run completes, with every key posted once and the business date advanced once.
- [ ] Commit with the message `feat(jobs-svc): a jobs service that runs the close on request and on schedule`.

### Task 8: jobs-svc takes over the scheduled jobs

Spec "jobs-svc", the jobs table, and "Alerts".

**Files:**
- Create `crates/jobs-svc/src/{daily.rs,cleanup.rs,drift.rs}`.
- Move the media sweep and FX import code out of `crates/core-api/src/tasks.rs` into `daily.rs`, then delete
  `tasks.rs`.
- Modify `crates/core-api/src/{main.rs,config.rs,lib.rs}`: drop `FX_IMPORT`, `run_fx_imports` and `run_sweeps`. Keep
  `DATABASE_LISTEN_URL` for LISTEN.
- Move or adapt the core-api tests that call `sweep_once` or `fx_import_due` to jobs-svc.
- Modify `modules/identity/src/throttle.rs`: make the window a `pub const LOGIN_WINDOW_SECS: i32`.

**Daily jobs.** Each takes `pg_try_advisory_lock(hashtext('job:' || name))` on a direct connection, and records the
result in `job_state` (`done_for` and `last_ok_at` on success; `last_error` otherwise).
- **`fx_import` (key `''`).**
  - Due when `FX_IMPORT=on` and either `done_for` is null, or `done_for < today` with Colombo local time ≥ 10:30.
  - Runs `fx::import(pool, source, today)`.
  - `done_for = today` only when `report.alerted == 0`, as `tasks.rs` does today.
- **`media_sweep` (key `''`).** Due when `done_for < today` (UTC date). Runs the moved `sweep_once`.
- **`window` (key = property).** Due when `done_for < business_date`. In one tenant transaction it runs
  `select 1 from property where id = $1 for update` and then `rooms::extend_window`.
- **`cleanup` (key `''`).** Due daily. Runs
  `select * from app.purge_expired($1, 7)` with `LOGIN_WINDOW_SECS`, and logs the counts.
- **`drift` (key = property).** Due when `done_for < business_date`. Runs `rooms::find_drift`.
  - Non-empty: log an `error` with the rows, and `alerts::raise(Drift, "Inventory counters differ from the bookings
    on <n> days, from <first date>; see the logs")`.
  - Empty: `alerts::clear(Drift)`.

- [ ] **Tests:**
  - Each job runs once when due and is skipped when not, with the `job_state` rows checked.
  - Two concurrent ticks run each job once.
  - **The FX import with a fake `RateSource`:** an alerted import is retried at the next tick; a complete one is not.
  - **Cleanup** deletes exactly the expired rows. The boundaries are already in Task 1's test; here, test the counts.
  - **Drift:** corrupt one `sold` counter, tick, and see the `drift` alert. Fix it, tick the next day, and the alert
    is cleared.
  - **core-api** starts with no FX or sweep task: the config test no longer knows `FX_IMPORT`.
- [ ] Commit with the message `feat(jobs-svc): the exchange-rate import, media sweep, window, cleanup and drift jobs move to jobs-svc`.

### Task 9: core-api: night-audit API, GraphQL and events

Spec "API" and "Permissions".

**Files:**
- Modify `crates/core-api/src/state.rs`: add a `jobs: JobsClient<Channel>` and `jobs_auth: Arc<TokenSource>`, in the
  same pattern as `MediaServices` (a lazy channel, TLS for https, `JobsServices::local()` for tests).
- Modify `config.rs`: `JOBS_SVC_URL` and `JOBS_SVC_AUDIENCE`, with the same production checks as `MEDIA_SVC_URL`.
- Create `routes/night_audit.rs`. Modify `routes/{reservations.rs,properties.rs,mod.rs}`, `graphql.rs`, `events.rs`,
  `openapi.rs` and `error.rs` (the `AuditError` mapping).
- Tests: `crates/core-api/tests/night_audit.rs`. Add `tests/common/jobs.rs` (an in-process jobs-svc, as
  `common/media.rs` builds media-svc).

**REST (exact paths). Every one is `If-Match` where it changes a room, idempotent where the existing room actions are,
and audited by the domain call.**
- **No-show:**
  - `POST /api/v1/properties/{property}/reservation-rooms/{room}/no-show` (`FrontDeskCheckIn`);
  - returns `NoShowRoom`; `412` on a version mismatch, `409` on a refusal.
- **Delay arrival:** `POST …/reservation-rooms/{room}/delay-arrival` (`FrontDeskCheckIn`), returning `ChangedStay`.
- **Extend stay:** `POST …/reservation-rooms/{room}/extend` (`FrontDeskCheckIn`), returning `ChangedStay`.
- **Close:** `POST /api/v1/properties/{property}/night-audit/close` (`NightAudit`).
  - Calls `StartClose` with the user, returning `202` and `{ "run_id": … }`.
  - gRPC `FAILED_PRECONDITION` → `409` with its message, `UNAVAILABLE` → `503` "The jobs service is unavailable".
- **Retry:** `POST /api/v1/properties/{property}/night-audit/runs/{run}/retry` (`NightAudit`). The run must belong to
  the property (404 otherwise).
- **Audit time:** `PATCH /api/v1/properties/{id}` takes `audit_time` (`HH:MM`), validated like `check_in_time`.
  `property::PropertyChanges` gains `audit_time`, the update SQL sets it, `Property` returns it, and the audit data
  includes it.

**GraphQL:**
- `nightAudit(propertyId)` requires `NightAudit`.
- `auditRuns(propertyId, first, after)` requires `NightAudit` or `FolioView`.
- `dailyStats(propertyId, from, to)` requires `NightAudit` or `FolioView`.
- `opsAlerts(propertyId)` requires `PropertiesManage` (owners and managers, as the spec says). `fxAlert` is gated
  by `FolioView`, so do not copy its gate.
- Money fields use the existing money scalar or type that 7a used (grep `Money` in `graphql.rs`).

**Events:** `audit:<property>` keys go only to a user holding `NightAudit` or `FolioView` for the property. Extend the
filter at `events.rs:107-133`.

- [ ] **Tests:**
  - **Every route's permission:** housekeeping gets `403` and front desk is allowed on the close. An accountant can
    read `auditRuns` but cannot close.
  - **Stale and blocking states:** a stale `If-Match` on a no-show gives `412`. A close with an unresolved arrival
    gives `409`, with the count in the message.
  - **Close end to end through the in-process jobs-svc:** `202`, then polling `auditRuns` shows `completed` and the
    property's business date is D + 1.
  - **Retry:** a failed run (no USD rate) can be retried after `PUT fx-rates`, then completes. Retrying another
    property's run gives `404`.
  - **Events:** a front-desk stream gets `audit:` keys and a housekeeping stream does not.
  - **`audit_time`:** a `PATCH` round-trips, and `25:00` is `422`.
  - **jobs-svc down:** with jobs-svc pointed at a closed port, the close is `503`.
- [ ] Regenerate the schemas and client types: `bun run api:schemas && bun run codegen` in `web/pms`.
- [ ] Commit with the message `feat(core-api): the night-audit API, run history, daily statistics and ops alerts`.

### Task 10: Performance gates

**Files:** modify `crates/core-api/tests/perf.rs`, reusing 7a's 200-room posting seed (`business_date_posting_for_200_rooms_under_3s`).

- [ ] **`night_audit_close_for_200_rooms_under_10s`:**
  - 200 rooms, 160 in house;
  - 40 POS bills over 2 outlets, half to rooms and half settled;
  - 120 payments in LKR and USD;
  - no arrivals or departures left.

  Time `audit::start_run(Manual)` plus `execute(NoHooks)` for three consecutive business dates (seed each day's
  nights). Print each, and assert that the median is under 10 s.
- [ ] **`night_audit_review_p95_under_100ms`:** the same seed plus 20 arrivals and 20 departures. `nightAudit` through
  the router, 200 samples; print p50 and p95, and assert p95 < 100 ms.
- [ ] Run every gate in release, `--test-threads=1`, on a quiet machine. If a gate fails, profile it and fix the query
  or index (as 7a did with 0030). Never raise a threshold.
- [ ] Commit with the message `test(perf): the night-audit close and review gates`.

### Task 11: Night audit in the SPA

Spec "UI".

**Files:**
- Create `web/pms/src/lib/night-audit.ts` and `night-audit.spec.ts`: the documents, keys, and pure helpers for
  grouping and labels.
- Create `components/{AuditStepTracker.svelte,AuditActionDialog.svelte}`.
- Create the routes `(app)/p/[property]/night-audit/+page.svelte` and `night-audit/history/+page.svelte`.
- Modify `(app)/p/[property]/+layout.svelte`: a nav entry, shown when `can(profile, 'nightAudit', property)`, and an
  ops-alert banner beside the FX banner.
- Modify `(app)/p/[property]/+page.svelte`: an **Audit time** field next to the check-in and check-out times.
- Modify `lib/events.ts`: `audit:` keys invalidate the `nightAudit`, `auditRuns`, `dailyStats` and `opsAlerts`
  queries.

**Behaviour:**
- **Header:** the business date, "<n> to resolve", and **Close the day** (disabled while n > 0, or while a run is
  `running`).
- **The five sections,** in the spec's order:
  - Arrivals: **No-show** (the default, primary button; its dialog shows the penalty), **Delay arrival** and
    **Cancel**, the last opening the existing cancel flow.
  - Departures: **Check out** (with balances) and **Extend a night**.
  - POS bills: per outlet, with highlights.
  - Booking bills: the preview, with problems shown in the row.
  - Payments: by method and currency, with flags.
- **Row actions** send `If-Match`. A `412` refetches and says "This booking changed; review it again". Any other
  refusal shows the server's message.
- **Step tracker:** after **Close the day**, it shows the five steps as pending, running, done or failed, from
  `auditRuns` (refetched on `audit:` events). A failure shows the error, the failed stays (room and reason) and
  **Retry**. On completion it says "Business date is now <D+1>", and the review reloads for the new date.
- **History:** runs (date, mode, who, duration, status, expandable step details) and a day table from `dailyStats`
  over the last 30 days (occupancy %, ADR, RevPAR, revenue, arrivals, departures), with money through `money.ts`.
- **Accessibility:** the sections are headings with counts, the dialogs follow the existing dialog components, and
  the step tracker uses `aria-live="polite"`.

- [ ] **Unit tests** (`night-audit.spec.ts`): grouping payments by method; the "City ledger" label; the `blocking`
  text; step states derived from a run.
- [ ] Run `bun run lint && bun run check && bun run test && bun run build`.
- [ ] Commit with the message `feat(web): the night audit, its close and the audit history`.

### Task 12: End-to-end tests

**Files:**
- Modify `web/pms/playwright.config.ts`:
  - add a third `webServer`: `cargo run -q -p jobs-svc` on port 8091 with `JOBS_TICK: 'off'`, `FX_IMPORT: 'off'` and
    the shared service auth;
  - give core-api `JOBS_SVC_URL: 'http://localhost:8091'`;
  - drop `FX_IMPORT` from core-api's env.
- Create `web/pms/tests/e2e/night-audit.spec.ts`, using `helpers.ts`. Seed rates with `seedRates`.

- [ ] **Tests:**
  - **The whole flow:** a property with
    - an arrival due today (marked a no-show, after which its fee shows on the deposit folio);
    - a departure due today (extended a night);
    - an in-house stay.

    Close the day, watch the tracker reach done, see the business date move on the overview, and see the run in
    History.
  - **The blocked close:** the button is disabled with "1 to resolve" until the arrival is resolved.
  - **Delay arrival:** the arrival leaves the list, and the reservation's dates change.
  - **Retry:** a USD stay without a rate fails `post_charges` and names the room. Add the rate on Exchange rates, press
    **Retry**, and the close completes.
- [ ] Run the default suite twice. The known flake `new-reservation.spec.ts:175` may fail alone; anything else must pass.
- [ ] Commit with the message `test(e2e): the night audit from review to close`.

### Task 13: Documentation

- [ ] **README:**
  - a **jobs-svc** section: what it runs, its environment (Global Constraints), how to run it next to core-api, and
    that `FX_IMPORT` moved to it;
  - "Trying the night audit by hand", in the 7a walkthrough's style, with exact steps;
  - the environment table updated (`JOBS_SVC_URL`, `JOBS_SVC_AUDIENCE`, `JOBS_TICK`, `DATABASE_DIRECT_URL`, and
    `FX_IMPORT` now on jobs-svc);
  - the end-to-end section mentions jobs-svc on 8091;
  - deploy notes: Cloud Scheduler `POST /tick` every 15 minutes with an OIDC token for the jobs-svc audience; Cloud Run
    with `--no-cpu-throttling`.
- [ ] **ROADMAP:** Phase 7b as done, with its deviations (the rulings in the plan ledger), the measured gates, and the
  carry-overs (overdue departures need a person; reports in 7d). Close the Phase 1 carry-overs.
- [ ] **ARCHITECTURE §7.5:** the night-audit paragraph matches the spec (no "PDF reports" in 7b; jobs-svc's role).
- [ ] **`docs/design/api-conventions.md`:** the business-date share-lock rule in the lock-order section; `NightAudit`
  in the permission matrix; `audit:<property>` in the events table.
- [ ] **`docs/design/data-model.md`:** the 7b tables and columns.
- [ ] Commit with the message `docs: the night audit, jobs-svc and daily statistics`.
