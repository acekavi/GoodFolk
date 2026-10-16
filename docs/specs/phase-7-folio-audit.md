# Phase 7a: Folio, Taxes, Desk Payments and Invoices (Spec)

**Depends on:** Phases 3, 5, 6a. **Implementation plan:** `docs/superpowers/plans/2026-10-16-phase-7a-folio.md`.
Design: [ARCHITECTURE.md §4.4, §7.5, §7.6](../ARCHITECTURE.md).

Phase 7 is split three ways (the owner's decision, 2026-10-16):
- **7a** (this spec): folios, the tax engine, exchange rates, desk payments, the POS bill contract and invoices.
- **7b:** the night audit and `jobs-svc` ([phase-7b-night-audit.md](phase-7b-night-audit.md)).
- **7c:** the online payment gateways and the bank guides ([phase-7c-payment-gateways.md](phase-7c-payment-gateways.md)).

Linen and guest laundry ([laundry.md](laundry.md)) follow 7a.

## Done when

- **Folios exist.** Every stay, every account (per property) and every reservation (for deposits) has a folio.
- **Charges are taxed.** Charges post with taxes and service charge calculated by the rules engine (Sri Lanka preset, order A) in the line's currency, with a base-currency (LKR) equivalent. Corrections are reversals and transfers, never edits.
- **Desk payments and refunds work.** This covers cash, card terminal, bank transfer and charge to account, in any accepted currency. Deposits taken before arrival move to the stay at check-in.
- **Room and meal-plan charges post.** They post for a business date idempotently, from a manager action now and from 7b's night audit later.
- **POS bills post.** A POS posts bills over REST, idempotently.
- **Invoices are issued.** IRD-format tax invoices are issued in LKR without cents, with gapless serials, rendered to a stored PDF. Credit notes correct them.
- **Exchange rates import daily.** The Central Bank TT selling rate is imported every day, with manual override and a loud failure.
- Every gate in "Performance gates" passes.

## Folios

- **Kinds:**
  - `guest`: one per reservation room, created with it.
  - `account`: one per account and property, created on first use. This is the city ledger.
  - `deposit`: one per reservation, created with it.
- **Status.** A folio is `open` or `invoiced`. An invoiced folio takes no lines until a credit note reopens it.
- **Balances.** A folio holds lines in any accepted currency. Its balance is per currency, with the base equivalent alongside.

## Lines

- **Fields:**
  - `source`: `room`, `meal_plan`, `laundry`, `pos`, `manual`, `cancellation_fee`, `no_show_fee` (7b) or `adjustment`;
  - `category`: `room`, `meal_plan`, `laundry`, `pos:<outlet code>`, `misc`, `fee`;
  - description, quantity, unit and net amount;
  - currency, base amount, rate and the rate's date;
  - business date and posting time, and who posted it.
- **Append-only.** The app role has no `update` or `delete` grant on lines, their taxes or payments.
- **Reversal.** An equal negative line with `reverses = <line>` and a reason (1–500 characters). It carries its own negative tax lines, mirroring the original's. A line can be reversed once, and a reversal can't itself be reversed.
- **Transfer.** A reversal on the source folio plus an identical line on the target folio, linked by `transfer_id`, in one transaction. Both folios must be open, and in the same property for guest and account folios.
- **Posting key.** System postings carry a `posting_key` that is unique per folio: `room:<reservation_room>:<date>`, `meal_plan:<reservation_room>:<date>`, `pos:<pos_bill>`, `cancellation:<reservation_room>`. A repeat is a no-op that returns the existing line.

## Routing

- **Rules.** A reservation with an account has `routing_rule` rows (`category` → account folio). On creation they default to `room` and `meal_plan` when the reservation has an account. Staff edit them per reservation (`FolioAdjust`).
- **Effect.** A charge posted for a stay goes to the account folio when a rule matches its category, and to the guest folio otherwise.

## Room and meal-plan posting

- **What it does.** `post_business_date_charges(property, business_date)` posts, for every reservation room checked in and in house on that date (`arrival ≤ date < departure`, not checked out):
  - the room charge from `reservation_night`'s price for that date;
  - the meal-plan supplements, per person, from the same snapshot.

  It is idempotent through posting keys, and returns the counts.
- **The trigger.** `POST …/folio/post-charges` (`FolioAdjust`) runs it for the property's current business date. 7b's close calls the same function.
- **At check-out.** Check-out first posts any unposted nights before the departure date, so no night is missed.

## Taxes

- **When.** Every line is taxed at posting with the property's rules effective on the line's business date. The result is stored as `folio_line_tax` rows (rule code, name, base amount, rate, amount, `on_bill` | `absorbed`). Past lines never change.
- **Engine.** `modules/tax` is pure: `calculate(net, category, rules, context) -> Breakdown` and `back_calculate(gross, …)`. The engine and rule model follow ARCHITECTURE §7.6.3:
  - rule fields: `kind`, `rate_bp`, `applies_to`, `base` components, `sequence`, `presentation`, `condition` (`always` | `vat_registered` | `residency` | `payment_currency`), and `effective_from` / `effective_to`.
  - Rounding is half-up per component in minor units, and the remainder goes to net, so components always sum to the total.
- **Inclusive prices.** A rate plan gains `prices_include_tax` (default false). When true, its nightly price is back-calculated, so net plus taxes equal the price exactly.
- **Sri Lanka preset.** Applied when a property is created, and to existing properties by the migration. Editable afterwards (`TaxSettingsManage`):
  - service charge 10% on `pos:*`; on rooms, off;
  - TDL 1%, or 0.5% when `tdl_small_turnover`; its base is net, excluding service charge;
  - SSCL 2.5%; its base is net + SC + TDL;
  - VAT 18% only when `vat_registered`; its base is net + SC + TDL + SSCL;
  - order A, everything on the bill.
- **Property settings.** `vat_registered`, `tin`, `tdl_small_turnover`, `legal_name` and `address` (for invoices).

## Payments

- **Methods.** `cash`; `card_terminal` (approval code, last 4, brand); `bank_transfer` (reference); `account`, which moves the amount to the reservation's account folio as a charge there and a credit here (undone by moving the account's line back to the stay, never by a refund); and `transfer`, internal only, never chosen by staff, for a credit moving between two folios.
- **Currency.** Any accepted currency, stored with its base equivalent.
- **Evidence.** An optional `evidence` text (for example a bank encashment receipt). 7b flags non-resident LKR payments without it.
- **Refunds.** A refund is a negative payment with `refund_of`, in the same currency and method. In total it can't exceed what remains of the original. Its base amount is the original's in proportion, at the original's rate, so a fully refunded payment nets to zero in LKR. Refunds need `FolioAdjust`.
- **Deposits.**
  - Payments taken before arrival go on the reservation's deposit folio.
  - At the first check-in of the reservation, the deposit folio's balance in each currency moves to that stay's guest folio as a pair of `transfer` payments sharing a `transfer_id`: the base amounts are carried exactly, and the pair's rate is the rate on the date moved, for reference. Undoing that check-in moves it back the same way. A deposit refunded after it moved is refunded from the stay's folio, up to the credit it holds.
  - When a reservation room is cancelled, its Phase 3 cancellation penalty posts to the reservation's deposit folio as `cancellation_fee`, in the room's currency, taxed (tax-inclusive when the room's rate plan prices include tax), once per room. The remaining credit stays there, to be refunded (`FolioAdjust`) or written off by an `adjustment` line.

## POS bills

- **Endpoint.** `POST /api/v1/properties/{p}/pos-bills` takes `{outlet, bill_no, business_date?, currency, items: [{description, quantity, unit_net, category?}], target}`. The target is `{reservation_room}` or `{settled: {method, details}}`. It is idempotent on `(property, outlet, bill_no)`: a repeat returns the original, and a repeat with a different body is `409`.
- **Taxes.** GoodFolk taxes each item with the outlet's rules (category `pos:<outlet>`).
- **Posting.** A bill posted to a stay creates one line per item, routed as usual, with posting key `pos:<bill>:<n>`. A bill settled directly is recorded with its payment, attached to no folio, so the audit still counts the revenue.
- **Outlets** are a property setting: `code` (`^[A-Z0-9]{1,10}$`), name, `service_charge` (on or off), active.
- **Permission.** A POS calls with a user that has the new `PosPost` permission (owner, manager, or a POS service user created by `add-staff`). The gRPC `PostPosBill` waits until a POS service exists.

## Invoices and credit notes

- **Issuing.** `POST …/folios/{id}/invoice` (`InvoiceIssue`) works only when the folio's balance is zero in every currency and the folio has at least one line. In one transaction it:
  1. takes `property_counter('invoice')`;
  2. snapshots the invoice data;
  3. sets the folio `invoiced`;
  4. writes an `invoice.issued` audit entry;
  5. enqueues the `invoice-render` job in the outbox.
- **Rendering.** media-svc renders the PDF with Typst and stores it privately. The invoice's `pdf_key` arrives through `/internal/events`, as media results do.
- **Format.** ARCHITECTURE §7.6.4:
  - "TAX INVOICE" when VAT-registered, otherwise "INVOICE";
  - the supplier's legal name, address and TIN;
  - the purchaser's name and TIN (from the account, or optionally from the guest);
  - serial `<property code>-<YYYY>-<6-digit number>`;
  - invoice date and supply period;
  - each line, and each on-bill tax with its rate;
  - **every value in LKR without cents**, half-up from each line's base amount, with the original currency shown for reference on foreign lines;
  - the total in words (English);
  - the payment methods.

  The layout is a Typst template, so the IRD specification can be followed when published without code changes.
- **Credit notes.** `POST …/invoices/{id}/credit-note` (`FolioAdjust`, with a reason) takes `property_counter('credit_note')`, renders likewise and reopens the folio. A new invoice is then issued as usual.
- **Links.** `GET …/invoices/{id}/link` gives a 5-minute link and is audited.

## Exchange rates

- **Table.** `fx_rate (property, date, currency, rate_to_base (numeric 18,6), source: cbsl | manual, source_date)`. Base equivalents use the rate for the line's business date: the latest row with `date ≤ business_date`. A missing rate for a foreign-currency posting is a `409` "No exchange rate for USD on <date>".
- **Import.** A daily core-api task, around 10:30 in `Asia/Colombo` and once at start-up, with an advisory lock. It fetches the Central Bank TT selling rate for each accepted non-base currency (the `exrates_resultstt.php` form). It stores today's rate, or the last published one, with its `source_date`. A parse failure or a missing currency logs an error and sets an `fx_alert` for the property, shown as a banner to owners and managers until a rate arrives. A stale rate is never written as today's without its true `source_date`.
- **Override.** `PUT …/fx-rates/{date}/{currency}` (`FxOverride`: owner, manager), audited. 7b moves the task to `jobs-svc` and locks the rates.

## Permissions

| Permission | Allows | Roles |
|---|---|---|
| `FolioView` | Folios, lines, payments, invoices | owner, manager, front_desk, accountant |
| `FolioPost` | Charges, payments | owner, manager, front_desk |
| `FolioAdjust` | Reversals, transfers, refunds, routing, credit notes, posting tonight's charges | owner, manager |
| `InvoiceIssue` | Issue invoices, invoice links | owner, manager, front_desk, accountant |
| `TaxSettingsManage` | Tax rules, outlets, property tax settings | owner |
| `FxOverride` | Manual exchange rates | owner, manager |
| `PosPost` | POS bills | owner, manager |

## API

- **REST.** Idempotent creates, `If-Match` on settings:
  - charges, reversals, transfers, payments, refunds;
  - routing (`PUT …/reservations/{id}/routing`);
  - `POST …/folio/post-charges` and `POST …/pos-bills`;
  - invoice and credit note issuing, and their links;
  - tax rules (CRUD and "apply preset"), outlets, property tax settings, `fx-rates`.
- **GraphQL.**
  - `folios(propertyId, reservationId | reservationRoomId | accountId)`: lines with taxes, payments, balances per currency, status, invoices.
  - `taxRules`, `outlets`, `fxRates(propertyId, from, to)`, `fxAlert`.
- **Events.** `folio:<folio>`, `fx:<property>`.

## UI

- **Reservation modal: Folios tab.** One tab per folio (deposit, each room's guest folio, the account folio). Each shows lines with expandable taxes, payments, per-currency balances with LKR, and the actions Post charge, Take payment, Refund, Reverse, Transfer, Routing, Issue invoice and View invoice.
- **Accounts page.** Each account's folio per property.
- **Settings.**
  - **Taxes:** property tax settings, rules with effective dates, apply preset.
  - **Outlets.**
  - **Exchange rates:** a table, overrides and the alert banner.
- **Reservations page.** A **Post tonight's charges** button for `FolioAdjust`.

## Tests that must exist

- **Tax golden tests:**
  - order A on the Rs. 10,000 restaurant bill gives 13,425.45, and order B gives 13,355.00;
  - inclusive back-calculation sums exactly;
  - the rounding remainder goes to net;
  - effective-dated rules;
  - not VAT-registered;
  - the small TDL band.
- **Lines:** they can't be updated or deleted by the app role; a reversal nets to zero, including its taxes; a line can't be reversed twice; a transfer moves the exact amount.
- **Idempotency:** posting the business date's charges twice adds nothing; check-out posts unposted nights; a replayed POS bill posts once, and a conflicting replay gets `409`.
- **Invoice serials** are gapless under concurrent issues, and a failed issue consumes no serial. A credit note and re-issue works. A foreign-currency line converts to whole LKR as specified.
- **Deposits** move at first check-in; a cancellation posts the penalty to the deposit folio.
- **Payments:** routing by category; charge-to-account; refund limits.
- **Exchange rates:** the Central Bank parser reads a saved response; a changed layout raises the alert; a weekend rate keeps its true `source_date`; a missing rate gets `409`.
- **Isolation cases** for every new table.
- **Playwright:**
  - post a charge and take a payment;
  - issue an invoice and view the PDF;
  - a POS bill posted to a room;
  - a deposit moves at check-in;
  - a housekeeper has no folio access.

## Performance gates

- Posting a charge with taxes: p95 < 30 ms.
- `post_business_date_charges` for a 200-room property at about 80% occupancy: < 3 s.
- `folios` for a folio of 200 lines: p95 < 30 ms.
- Invoice PDF ready (issue to `pdf_key`): < 2 s.
