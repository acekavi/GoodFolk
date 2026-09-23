# Phase 8: Settings, Users and Channels (Spec)

**Depends on:** Phases 2, 3, 7. **Implementation plan:** written at phase start in `docs/superpowers/plans/`. Design: [ARCHITECTURE.md §7.7, §9](../ARCHITECTURE.md).

## Done when

- Owners invite staff by email, grant roles tenant-wide or per property, and revoke access (sessions end immediately).
- Every property setting is editable in the UI: times, currencies, taxes (rules editor), policies, reason codes, sections, outlets, payment providers and routing, invoice details (TIN, SLTDA licence).
- A property connects to Channex: room types are imported or matched, rate plans mapped, and availability, rates and restrictions pushed automatically. OTA bookings, modifications and cancellations arrive by webhook and appear on the tape chart.

## Users and roles

- `invitation`: an email with a single-use token (hashed), expiring in 7 days. Accepting creates or links the `app_user`, membership and grants.
- Revoking a membership deletes that user's sessions for the tenant.
- A permissions matrix UI (roles × permissions) shows what each role can do. Custom roles are out of scope for v1.

## Channels (`channel-svc`, gRPC + Pub/Sub consumer)

- **Connect:** store the Channex API key in Secret Manager. Fetch the Channex property, room and rate structure.
- **Import or match room types:** propose matches by name and occupancy; the user confirms, creates or ignores each; `channel_room_map` / `channel_rate_map` are saved. Mapping a rate plan checks that the currencies are equal.
- **ARI push:** outbox events (`inventory.changed`, `rates.changed`, `restrictions.changed`) → `channel-svc` debounces per property (2 s window), merges date ranges, and pushes in batches within Channex rate limits. On failure it retries with exponential backoff; a dead-letter entry raises an alert.
- **Bookings in:** webhook → verify → store `channel_booking_event` (unique on external id + revision) → apply idempotently: create, modify or cancel the reservation with `source = channel`. Unmapped room types go to an "unassigned" queue for the front desk. Acknowledge to Channex only after commit.
- **Full sync** command for recovery (push 365 days).

## Tests that must exist

- Invitation token single use and expiry; a revoked user's next request gets 401.
- ARI debouncing merges ranges and never sends stale values (last write wins by version).
- Booking ingestion is idempotent under replay and out-of-order revisions.
- Contract tests against recorded Channex sandbox payloads.
- Isolation-suite cases.

## Performance gates

- An ARI change reaches Channex in < 10 s p95. A booking webhook becomes a visible reservation in < 3 s p95.
