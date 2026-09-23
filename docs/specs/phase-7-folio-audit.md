# Phase 7: Folio, Taxes, Payments and Night Audit (Spec)

**Depends on:** Phases 3, 5, 6. **Implementation plan:** written at phase start in `docs/superpowers/plans/`. Design: [ARCHITECTURE.md §4.4, §7.5, §7.6, §10.1](../ARCHITECTURE.md).

## Done when

- Every stay has a folio. Charges (room, meal plan, laundry, POS, manual) post with taxes and service charge calculated by the rules engine (Sri Lanka preset, order A), in the line's currency with a base-currency equivalent.
- Payments are taken at the desk (card terminal, recorded manually), online through CyberSource or Mastercard Gateway (whichever the hotel's bank provides), or through payments.lk for LKR. Refunds and voids work.
- The night audit walks through its review screens (arrivals / no-shows, departures, POS bills, booking bills, payments), then closes the day: postings, rate lock, statistics, reports, business date advanced. It is crash-safe and idempotent.
- IRD-format tax invoices are issued in LKR with gapless serials.
- Setup guides exist for CyberSource, MPGS and each major bank.

## Components

- **`folio` module:** folios, lines, reversals, routing (split to account or company), balances by currency.
- **`tax` module:** the rules engine as a pure function `calculate(line, rules, context) -> Vec<TaxLine>`, plus inclusive-price back-calculation. Golden tests use the worked example (Rs. 10,000 restaurant bill → 13,425.45 under order A).
- **`fx` module:** the Central Bank daily TT selling rate fetcher (`RateSource` trait; parses the published page; alerts on failure, never silently reuses stale rates), manual overrides, and the locked rate per business date.
- **`payments` module:** the `PaymentProvider` trait with `manual`, `cybersource` (Unified Checkout, TMS, JWT auth, encrypted webhooks), `mpgs` (Hosted Checkout, tokens, Basic auth, per-property gateway host, `X-Notification-Secret` + order re-fetch) and `payments_lk`. One webhook ingestion endpoint per provider under `/webhooks/`. Routing rules per property (at the desk or online, currency, residency). Card data never touches our servers.
- **POS contract:** gRPC `PostPosBill` and `POST /api/v1/properties/{p}/pos-bills`, idempotent on `(outlet, bill_no)`.
- **`jobs-svc`** (Cloud Run + Cloud Scheduler every 15 min): night audit close, daily rate import, inventory window extension, idempotency-key cleanup, counter drift check. gRPC from core-api for manual runs.

## Night audit

Review screens (each a GraphQL query) and actions:
1. **Arrivals not checked in:** mark no-show (default; posts the no-show fee per policy), extend, or cancel. All must be resolved.
2. **Departures not checked out:** check out or extend.
3. **POS bills:** by outlet; posted to rooms vs settled directly; open bills highlighted.
4. **Booking bills:** a preview of tonight's room, meal-plan, laundry and manual charges.
5. **Payments:** by method and currency with base totals. Flags non-resident LKR payments without evidence.

The close is an `audit_run` with checkpointed steps: `post_charges`, `lock_fx`, `snapshot_stats`, `render_reports`, `advance_business_date`. Each step is idempotent (for example postings are keyed by `(reservation_room, date, source)`). A crash resumes from the last completed step; a second run for the same date is a no-op.

## Tax invoice (IRD format)

- Required fields: heading "TAX INVOICE", supplier and purchaser TIN, serial number, dates, VAT rate, values **in LKR without cents**, total in words, payment method.
- USD folios are converted at the locked rate, with USD shown for reference. Serials come from `property_counter('invoice')` inside the issuing transaction, so there are no gaps.
- Rendered to PDF and stored through media-svc (private).

## Bank setup guides (`docs/guides/payments/`)

`cybersource.md`, `mpgs.md`, and one page per bank (ComBank, HNB, Sampath, BOC, People's, Seylan, NTB, plus NDB / DFCC / Pan Asia once their platform is confirmed). They are written from each bank's current merchant documentation and checked with the bank's merchant team before hotels receive them.

## Tests that must exist

- Tax golden tests (order A and B, inclusive back-calculation, rounding remainder, effective-dated rules, conditions: not VAT-registered, small TDL band).
- Folio lines are append-only (the app role cannot update or delete); reversal nets to zero.
- Night-audit resilience: kill the process after each step in turn; rerunning reaches the same final state with no duplicate postings.
- Provider contract tests against recorded sandbox responses (CyberSource, MPGS, payments.lk); webhook signature and secret verification; replayed webhooks are idempotent.
- Rate fetcher: parses a saved page; a layout change raises an alert.
- Isolation-suite cases for every new table.

## Performance gates

- Posting a charge with taxes: p95 < 30 ms. Night-audit close for a 200-room property < 10 s.
