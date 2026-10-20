# Phase 7b: Night Audit, jobs-svc and Reports (Spec)

**Depends on:** Phase 7a ([phase-7-folio-audit.md](phase-7-folio-audit.md)). **Implementation plan:** written at phase
start in `docs/superpowers/plans/`. Design: [ARCHITECTURE.md §7.5](../ARCHITECTURE.md).

## Done when

- The night audit walks through its review screens, then closes the day. The close covers postings, the rate lock, statistics, reports and advancing the business date. It is crash-safe and idempotent.
- Daily statistics and core reports exist, along with the liability reports (TDL quarterly, SSCL monthly and quarterly, VAT per period, service charge).

## Night audit

The review screens are GraphQL queries, each with actions:
1. **Arrivals not checked in:** mark as no-show (the default; posts the no-show fee per policy), extend, or cancel. All must be resolved before the close.
2. **Departures not checked out:** check out or extend.
3. **POS bills:** grouped by outlet, showing what was posted to rooms and what was settled directly. Open bills are highlighted.
4. **Booking bills:** a preview of tonight's room, meal-plan, laundry and manual charges.
5. **Payments:** by method and currency, with base totals. Non-resident LKR payments without evidence are flagged.

The close is an `audit_run` with checkpointed steps:
1. `post_charges`: 7a's `post_business_date_charges`.
2. `lock_fx`.
3. `snapshot_stats`.
4. `render_reports`.
5. `advance_business_date`.

Each step is idempotent. A crash resumes from the last completed step, and a second run for the same date is a no-op.

## jobs-svc

Runs on Cloud Run, triggered by Cloud Scheduler every 15 minutes. It runs:
- the night audit close;
- the daily exchange-rate import, moved from core-api;
- inventory window extension;
- idempotency-key, session and login-failure cleanup;
- the media sweep, moved from core-api;
- a counter drift check.

core-api calls it over gRPC for manual runs.

## Carried over from Phase 1

- Run `rooms::extend_window` under a property-level lock.
- The roll updates `property.business_date` with the row locked `for update`. Counter writers read the business date `for share`.

## Tests that must exist

- Night-audit resilience: kill the process after each step in turn; rerunning reaches the same final state with no duplicate postings.
- Statistics and report golden tests.
- Isolation cases for every new table.

## Performance gates

- The night-audit close for a 200-room property takes < 10 s.
