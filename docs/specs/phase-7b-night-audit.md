# Phase 7b: Night Audit, jobs-svc and Daily Statistics (Spec)

**Depends on:** Phase 7a ([phase-7-folio-audit.md](phase-7-folio-audit.md)). **Implementation plan:** written at phase
start in `docs/superpowers/plans/`. Design: [ARCHITECTURE.md §7.5](../ARCHITECTURE.md).

The owner split the original 7b in two (2026-10-20):
- **7b** (this spec): the night-audit review and close, `jobs-svc`, the business-date roll with its Phase 1 locking fixes, and the daily statistics snapshot.
- **7d:** the report PDFs and the liability reports, read from 7b's snapshots ([phase-7d-reports.md](phase-7d-reports.md)).

Laundry stays its own phase ([laundry.md](laundry.md)).

## Done when

- **The night audit runs.** Staff work through the five review lists, resolve every arrival and departure, and close the day.
- **The close is safe.** It is checkpointed, crash-safe and idempotent. A run killed at any step and resumed reaches the same final state, with no duplicate postings.
- **Unattended days close.** A property nobody closes by its audit time is closed automatically.
- **jobs-svc runs scheduled work.** It runs the close, the exchange-rate import, the media sweep, window extension, cleanup and the drift check. core-api no longer runs any of them.
- **Daily statistics are snapshotted at each close.** They can be read by date range.
- Every gate in "Performance gates" passes.

## The review

One GraphQL query, `nightAudit(propertyId)`, returns five lists for the property's business date D. Each also carries a blocking count: unresolved arrivals plus unresolved departures.

1. **Arrivals not checked in.** Stays with `lower(stay) = D` that are still `confirmed` or `tentative`, with guest, room, total and booked terms. Actions (each with `If-Match` on the reservation room):
   - **No-show** (the default; confirmed rooms only): `reservations::mark_no_show`.
     - It sets the status to `no_show` and releases `sold` from D on.
     - It posts the booked terms' `no_show` penalty to the reservation's deposit folio:
       - computed by the cancellation `penalty()` formula against the stay's total;
       - a `no_show_fee` line in category `fee`, under the key `no_show:<reservation_room>`;
       - taxed by the `*` rules, and tax-inclusive when the rate plan's prices include tax, exactly like the cancellation fee.

     The deposit stays on the deposit folio for a refund or a write-off. `folio::post_cancellation_fee` is generalised to take the source and key.
   - **Delay arrival**: `reservations::delay_arrival`. The stay becomes `[D+1, departure)`; D's `reservation_night` row is deleted and its `sold` released. A one-night stay is refused (`409`, "Change the dates with Modify").
   - **Cancel**: the existing `cancel_room`, with its penalty.

   A tentative room offers only Delay arrival and Cancel.
2. **Departures not checked out.** Stays that are `checked_in` with `upper(stay) = D`, with their folio balances. Actions:
   - **Check out**: the existing command.
   - **Extend a night**: `reservations::extend_stay`.
     - It adds night D+1 at the stay's last night's room and meal amounts, with no quote or rate restrictions.
     - The new night must be sellable and the room free (`409` otherwise, with the server's reason).
     - `modify_room` is not used, because it quotes added nights afresh.
3. **POS bills of D**, grouped by outlet. Each bill shows its number, its total, and either the stay it posted to (with the room number) or "settled at the outlet", with a total per outlet. A bill posted to a stay that has since checked out with a non-zero balance is highlighted.
4. **Booking bills.**
   - **Preview of tonight's room and meal-plan lines** per in-house stay. It is computed by a dry run of the code `post_business_date_charges` uses, so it skips keys already posted, and it flags a stay that would fail (for example, for a missing rate).
   - **The day's manual and fee lines** already posted, by category.
5. **Payments of D.** Grouped by method, then currency, with original and base totals and refunds. A charge to account shows as city ledger. An LKR payment on a stay whose rate plan segment is `FIT_F` (non-resident) and whose evidence is empty is **flagged**.

## The close

An `audit_run` per `(property, business_date)`: `mode` `manual` or `auto`; `started_by` (null for the system); `status` `running`, `failed` or `completed`; `error`, `started_at` and `finished_at`.

Each step runs in one transaction, which also inserts the step's `audit_run_step` row `(run_id, step, completed_at, detail jsonb)`. A step is done exactly when its row exists, so a crash loses all of a step or none of it, and every step is idempotent when redone.

0. **`resolve`.**
   - **Manual mode:** this step only checks that no arrival or departure is unresolved. Starting a manual close is refused (`409`, naming the count) until both lists are empty.
   - **Auto mode:**
     - every unresolved confirmed arrival is marked a no-show;
     - every tentative one is cancelled under its own terms;
     - every due departure still in house is extended one night at its last night's price.

     If an extension is refused, the step fails.
1. **`post_charges`**: `folio::post_business_date_charges(D)`. Any stay that fails **fails the run**, and the step's report lists the stays. A rerun posts only what is missing, thanks to the posting keys.
2. **`lock_fx`.** For every accepted non-base currency with a rate in effect on D, this writes that rate as an `fx_rate` row dated D, if there is none. From then on, `fx::set_manual` refuses any date before the property's business date (`409`, "That day is closed"), and the import skips such a property for that date without raising an alert.
3. **`snapshot_stats`** and 4. **`advance_business_date`** commit together, in one transaction holding `property` `FOR UPDATE`. That transaction:
   - checks `business_date = D`;
   - takes the statistics for D;
   - sets the business date to D + 1;
   - runs `rooms::extend_window`;
   - inserts both step rows;
   - marks the run `completed`.

   Nothing dated D can therefore land after its statistics are taken.

**Running a close.**
- **One run at a time per property.** A run executes under the session advisory lock `hashtext('audit:' || property)`.
- **Starting again is safe.** Starting a close for a date that already has a run returns that run, and a run for a completed date is a no-op.
- **A crashed run resumes.** A tick resumes any `running` run whose lock is free.
- **A failed run waits for a person.** It waits for **Retry** (`NightAudit`). An auto run is retried on each tick, at most 3 times, after which an `ops_alert` of kind `audit` is raised.
- **The alert clears** when the date completes.

## Locking: carried over from Phase 1

Every writer that reads the business date reads `property` `FOR SHARE`. That covers the inventory counters, reservation commands, folio postings, POS bills, housekeeping and exchange-rate overrides. `rooms::extend_window` takes that share lock **before** inserting counter rows. The roll holds `FOR UPDATE` and waits on no row a writer could hold, since a writer inserts counter rows only after its share lock. So a writer that already holds other rows can safely take the share lock mid-transaction. [api-conventions.md](../design/api-conventions.md) records the rule.

The roll takes `FOR UPDATE`, so it waits for writers in flight and holds new ones back only for the snapshot transaction. `rooms::extend_window` runs under the same `FOR UPDATE` lock in the roll and in the daily job.

## jobs-svc

A new binary, `crates/jobs-svc`, built like media-svc:
- axum and tonic on one port;
- svc-auth on every route: Google ID tokens in production, the shared secret locally;
- JSON logs in production;
- `Dockerfile.jobs-svc`.

It connects as the application role and enters each tenant through `db::begin`. Reads across tenants (the property list, session cleanup, the drift scan) go through `SECURITY DEFINER` functions with a pinned `search_path`, such as `app.jobs_properties()` and `app.purge_expired()`.

**`goodfolk/jobs/v1/jobs.proto`:**
- `Tick()` runs every due job once and returns a report.
  - In production Cloud Scheduler calls it as HTTP `POST /tick` with an OIDC token every 15 minutes.
  - Locally, `JOBS_TICK=internal` makes the service tick itself every 15 minutes; `off` disables it (the e2e suite).
- `StartClose(tenant, property, user)` creates or returns the run for the property's business date, starts it in a background task and returns the run id at once. `RetryClose(tenant, run, user)` does the same for a failed run.
  - core-api calls both with its `TokenSource`.
  - On Cloud Run, jobs-svc is deployed with CPU always allocated (`--no-cpu-throttling`), so a background close keeps running after the call returns.

**Jobs.** Each runs under its own advisory lock, so overlapping ticks or two instances never run one twice.

| Job | Due | Notes |
|---|---|---|
| Auto close | the property's local time is past the audit moment of D: D at `audit_time` when it is 12:00 or later, D + 1 at `audit_time` when it is earlier (default 03:00) | `mode = auto`; D has no completed run |
| Resume | a `running` run whose lock is free | crash recovery |
| Exchange-rate import | 10:30 `Asia/Colombo` daily, and at start-up | moved from core-api, with `FX_IMPORT=on\|off` |
| Media sweep | daily | moved from core-api |
| Window extension | daily per property | `extend_window` under the property `FOR UPDATE` lock |
| Cleanup | daily | expired `session` rows; `login_failure` rows older than the throttle window; `idempotency_key` rows older than 7 days |
| Drift check | daily per property | `rooms::find_drift`; a drift is logged as an error and raised as an `ops_alert` of kind `drift`, cleared by a clean check; nothing is repaired automatically |

**Alerts.** `ops_alert (property, kind, message, raised_at)`, with kinds `audit` and `drift`, one per property and kind. Owners and managers see it as a banner, like `fx_alert`.

## Daily statistics

These are taken in the snapshot transaction for D, as base-currency (LKR) figures from stored base amounts. They are insert-only: a closed day is never rewritten, and a later correction counts on the day it is posted.

**`daily_stats`**, one row per `(property, business_date)`:
- **Rooms:**
  - `rooms_total`: active rooms;
  - `rooms_out_of_order`: out of order on D;
  - `rooms_available`: total less out of order. Out of service still counts as available.
- **Occupancy:**
  - `rooms_sold`: stays with D as a night, in house at the close;
  - `complimentary_rooms`: sold nights with a zero room amount, included in sold;
  - `occupancy_bp = rooms_sold × 10000 / rooms_available`.
- **Counts:** `adults`, `children`, `arrivals`, `departures`, `no_shows`, `cancellations` (cancelled on D) and `walk_ins` (booked and checked in on D).
  - Two new columns, both backfilled from their timestamps in the property's time zone, make "on D" exact: `reservation.booked_business_date` and `reservation_room.cancelled_business_date`.
- **Revenue, net of tax:** `room_revenue`, `meal_plan_revenue`, `pos_revenue`, `fee_revenue` (cancellation and no-show) and `other_revenue`.
  - These are summed from `folio_line.base_amount` with `business_date = D`, reversals included. Revenue is recognised on the posting business date.
  - `adr = room_revenue / rooms_sold` and `revpar = room_revenue / rooms_available`, in minor units, rounded half-up, 0 when the divisor is 0.
- **`taxes`:** jsonb `{code: base amount}` from `folio_line_tax` of D's lines.
- **`payments`:** jsonb `{method: base amount}` of D's payments, refunds included.

**`daily_stats_breakdown`** `(property, business_date, dimension, key, room_nights, room_revenue, revenue)`. The dimensions:
- `segment`: the rate plan's;
- `source`: the reservation's;
- `residency`: the primary guest's;
- `room_type`;
- `outlet`: POS revenue per `pos:<outlet>` category, with `room_nights` 0.

Each dimension's rows sum to the day's totals.

Read through GraphQL `dailyStats(propertyId, from, to)`, at most 366 days, with the breakdowns.

## Permissions

- **`NightAudit`** (new: owner, manager, front desk) covers the review query, starting a close, Retry, and the audit history.
- **`dailyStats`** needs `NightAudit` or `FolioView`, so an accountant can read it.
- **Existing permissions still apply:**
  - The review's row actions keep their own permissions. Check-out, no-show and delay arrival use `FrontDeskCheckIn`. Cancel and Modify use theirs.
  - The `audit_time` setting needs `PropertiesManage`.

## API

- **REST:**
  - `POST …/reservation-rooms/{room}/no-show`
  - `POST …/reservation-rooms/{room}/delay-arrival`
  - `POST /api/v1/properties/{p}/night-audit/close`: calls `StartClose` and returns the run, `202`. It is refused with `409` while anything is unresolved.
  - `POST …/night-audit/runs/{run}/retry`
  - `PATCH` property `audit_time`
- **GraphQL:** `nightAudit`, `auditRuns(propertyId, first, after)` with steps and reports, `dailyStats`, and the `ops_alert` banners.
- **Events:** `audit:<property>` for run and step changes, delivered to users holding `NightAudit` or `FolioView`. The existing `reservations`, `tape:` and `folio:` keys are sent for every change the actions and the close make.
- **Audit log:** `reservation_room.no_show`, `reservation_room.arrival_delayed`, `night_audit.started`, `night_audit.retried`, `night_audit.completed` and `night_audit.failed`. Actions taken by an auto close are recorded with no actor and `mode: auto` in their data.

## UI

- **Night audit page** (nav, `NightAudit`):
  - a header with the business date, the blocking count and **Close the day** (disabled while the count is above zero);
  - the five review lists, with row actions in confirm dialogs that show the fee or price effect;
  - a step tracker during a close, with each failed step's report and **Retry**.
- **Audit history:** past runs (mode, who, duration, step reports) and a day table from `dailyStats` (occupancy, ADR, RevPAR, revenue, arrivals, departures).
- **Settings → Property:** **Audit time**.
- **Banners:** `ops_alert` for owners and managers.

## Tests that must exist

- **Resilience:**
  - A test-only failure hook stops the run after each step's commit and also inside each step before its commit. A rerun reaches the same final state: no duplicate lines, one `daily_stats` row, the business date advanced exactly once.
  - A second close for the same date is a no-op.
  - jobs-svc started as a child process is killed with SIGKILL during a close and restarted, and the next tick completes the run.
- **Concurrency:**
  - two concurrent `StartClose` calls create one run;
  - a reservation write racing the roll lands wholly before the snapshot or wholly on D + 1;
  - `extend_window` racing the roll;
  - two concurrent ticks run each job once.
- **Actions:**
  - No-show releases inventory, posts its fee once (a replay posts nothing) with taxes, and leaves the deposit on the deposit folio.
  - Delay arrival works, and a one-night stay is refused.
  - The auto `resolve` path works, including an extension refused because the room is taken, which fails the run and raises the alert.
  - A closed date's rate cannot be overridden or imported.
- **Statistics:** a golden test over a seeded day with every kind of row checks exact figures, and every breakdown sums to the totals.
- **jobs-svc:**
  - auth refusals;
  - when a job is due, including `audit_time` after midnight and a property created today;
  - cleanup boundaries.
- **Isolation:** `audit_run`, `audit_run_step`, `daily_stats`, `daily_stats_breakdown` and `ops_alert` join the tenant isolation suite, with schema tests for their grants (steps and statistics insert-only).
- **e2e:** resolve an arrival as a no-show, extend a departure, close the day, and see the business date move and the run in the history.

## Performance gates

- The close for a 200-room property (160 stays in house, with POS bills and payments) takes **< 10 s**, release build.
- `nightAudit` for a 200-room property: **p95 < 100 ms**.
