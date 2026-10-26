# Phase 7d: Reports and Liability Reports Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Everyday reports (occupancy, ADR, RevPAR and revenue breakdowns over a range, plus the daily lists) on screen,
as PDF and as weekly and monthly emails. Liability reports (TDL Form A, SSCL, VAT with IRD's CSV schedules, and service
charge) for the person at the hotel who files them.

**Architecture:**
- **A new module `modules/reports`:**
  - computes the everyday reports from 7b's `daily_stats` snapshots;
  - computes the liability reports live from closed-date folio lines, tax lines, payments and invoices.
- **Persistence:** a `report` table records every rendered report (key, data, PDF).
- **Rendering:** media-svc gains a generic Typst report job beside the invoice one. Its result reaches core-api through
  `media-results`, as invoices do.
- **The close:** a new step, `render_reports`, runs after `advance_business_date`. It queues every period that has just
  fully closed.
- **Email:** once a weekly or monthly report's PDF is rendered, core-api enqueues one `mail-send` job per recipient.
  jobs-svc sends it with the PDF attached.

**Tech Stack:**
- **Server:** Rust (core-api, jobs-svc, media-svc), with the modules `audit`, `folio`, `identity` and the new
  `reports`, and the crates `mail`, `media-types` and `storage`.
- **Data:** Postgres migration 0041, GraphQL and REST.
- **PDFs:** Typst templates in media-svc.
- **Web:** SvelteKit pages under `web/pms/src/routes/(app)/p/[property]/reports`, with Playwright tests.

**Spec:** [docs/specs/phase-7d-reports.md](../../specs/phase-7d-reports.md). Read its "Settled for the builder" section;
those rulings bind every task.

## Global Constraints

**Data and rounding:**
- **Closed days only.** Every figure comes from business dates before the property's current business date. A range
  past it shows those days as "not closed yet" and never estimates them.
- **Refunds, reversals and credit notes** reduce the period their own business date falls in. No closed period ever
  changes.
- **Money is whole LKR** on every report: sums of base amounts, rounded once per printed figure, as invoices do.
- **Absorbed taxes are included** wherever the hotel owes them.

**TDL** (SLTDA Form A, quarterly, Q1 = April–June, Q2 = July–September, Q3 = October–December, Q4 = January–March):
- **The form:** Room Sales, Food Sales, Bar Sales, Other Sales, then Total Turnover; Deduction: Service Charge, VAT; then
  Total Turnover less deductions; then TDL.
- **The rate:** 1%, or 0.5% when Total Turnover less deductions is not over Rs 3,000,000.
- **The fixture:** Room 55,800, Food 237,973, Bar 3,183,537, Other 0 and VAT 0, with SC 23,797 deducted, gives a base
  of 3,453,513 and TDL 34,535 (whole rupees).

**SSCL:**
- 2.5% of liable turnover, quarterly calendar periods shown month by month.
- **Payment due:** the 20th of the following month for the first two months of the quarter, and the 20th of the month
  after the quarter for the third. The return is due the 20th of the month after the quarter.
- **The exemption** needs `property.sltda_licence` to be set, and covers foreign-currency `card_terminal` and
  `bank_transfer` payments only.

**VAT:**
- **Schedule 01 columns:** `Serial No, Invoice Date, Tax Invoice No, Purchaser's TIN, Name of the Purchaser,
  Description, Value of supply, VAT Amount`.
- **Dates:** MM/DD/YYYY.
- **The dummy TIN** for invoices without a purchaser TIN is `100902885`, combined into one row per rate. That row is
  dated the period's last day, described `Inclusive Invoices at 18%` and numbered `INCL01_<period code>`.
- **File name:** `<TIN>_VAT_SCHEDULE01_<period code>_<YYYYMMDD>_ORIGINAL_V1.csv`.
- **Period codes:** `YYQ0` for a quarter and `YYQM` for a month.

**Emails:**
- **Weekly:** weeks run Monday to Sunday, and the email is sent after Sunday closes.
- **Monthly:** sent after the month's last day closes.
- **Exactly once** per (property, kind, period).
- **Recipients:** owners and managers, unless that person turned the email off for that property.

**Permissions:**
- `ReportsView`: owner, manager and accountant.
- `ReportsLiability`: owner and accountant.

**Rules carried over from earlier phases:**
- `CARGO_BUILD_JOBS=3` and `-j 3`.
- Per task, only the changed crates' tests (for core-api, only the touched test binaries), plus `cargo fmt --check` and
  `cargo clippy --workspace --all-targets -- -D warnings`. Never `cargo test --workspace`.
- Bun only in web/pms. Commits carry no AI attribution. Never push.
- The test DB: `DATABASE_URL=postgres://goodfolk_owner:goodfolk_owner_dev@localhost:5432/goodfolk`. RustFS runs on :9000.

## Review Focus

1. **A folio settled partly in USD by card and partly in LKR cash:** only the USD card share is exempt, and the report
   lists the foreign cash separately. Owned by Task 4.
2. **A folio that settles, gets a late charge and settles again:** each line's exemption comes from the settlement that
   covers it, with no double count. Owned by Task 4.
3. **A night audit catching up over a week boundary and a month end in one tick:** both reports are queued, each exactly
   once. Owned by Task 7.
4. **A property that is not VAT-registered, or has no TIN:** the VAT page and the CSV download refuse with clear copy
   and never produce an empty file. Owned by Task 5.
5. **A report range whose last days are not closed yet:** those days show as "not closed yet", the headline figures
   cover only the closed days, and the page says so. Owned by Task 2.

---

### Task 1: Schema, permissions and settings

**Files:**
- Create: `migrations/0041_reports.sql`
- Modify:
  - `modules/identity/src/rbac.rs` (`ReportsView`, `ReportsLiability`, described and mapped to roles)
  - the property settings code where `tin` and `sltda_licence` are saved (search `sltda_licence` in modules/ and
    crates/core-api/src/routes/), to add `tdl_registration_no` and `vat_return_period`
  - the outlet code (search `service_charge` in modules/folio/src) for `report_group`, with history
  - `modules/audit/src/close.rs` (`Step::RenderReports` exists but does nothing yet; Task 7 fills it)
  - `modules/audit/src/alerts.rs` (`AlertKind::Reports`)
  - the web settings pages `settings/taxes` and `settings/outlets`
  - the API schemas, regenerated
- Test: `crates/db/tests/` (an RLS isolation case for the new tables, as for the others), the identity rbac tests,
  the property settings and outlet tests in their existing files, and `crates/core-api/tests` for the settings routes
  touched

**Migration 0041:**
- `property`:
  - add `tdl_registration_no text check (length between 1 and 30)`, nullable;
  - add `vat_return_period text not null default 'quarterly' check (vat_return_period in ('monthly', 'quarterly'))`.
- `outlet_report_group (tenant_id, outlet_id, report_group text check in ('food','bar','other'), effective_from date,
  primary key (outlet_id, effective_from))`:
  - Backfill one row per existing outlet: `food`, effective from `'2000-01-01'`.
  - A change inserts a row effective from the property's current business date; an existing row for that date is
    replaced.
- `report`:
  - Columns: `(id uuid pk, tenant_id, property_id, kind text check in ('weekly','monthly','tdl','sscl','vat',
    'service_charge','on_demand'), period_start date, period_end date, data jsonb, status text check in
    ('rendering','ready','failed'), pdf_key text, error text, created_at, rendered_at)`.
  - Constraint: `unique (property_id, kind, period_start) where kind <> 'on_demand'`.
- `report_mail_pref (tenant_id, user_id, property_id, kind text check in ('weekly','monthly'), enabled boolean not
  null, primary key (user_id, property_id, kind))`. No row means on.
- `report_mail (tenant_id, report_id, user_id, primary key (report_id, user_id))`: the exactly-once record of each
  email enqueued.
- `audit_run_step.step` check: add `'render_reports'`. `ops_alert.kind` check: add `'reports'`.
- **RLS:** forced on every new table, with the `tenant_isolation` policy, as in 0031.
- **Index:** `invoice (property_id, issued_at)` if the VAT query needs it (decide in Task 5; it is added here only if
  Task 5 says so).

**Interfaces:**
- Produces:
  - `Permission::ReportsView` and `Permission::ReportsLiability`;
  - `folio::outlet_report_group(tx, outlet, date) -> ReportGroup` and `ReportGroup { Food, Bar, Other }`;
  - property settings gain `tdl_registration_no: Option<String>` and `vat_return_period: VatReturnPeriod
    { Monthly, Quarterly }`;
  - `audit::Step::RenderReports` (after `AdvanceBusinessDate` in `Step::ALL`).

- [ ] **Step 1: Write the tests**
  - **rbac matrix:**
    - owner has both permissions;
    - manager has `ReportsView` only;
    - accountant has both;
    - front_desk, housekeeping, hk_supervisor and pos have neither.
  - **Settings round-trip:** `tdl_registration_no` and `vat_return_period` are audited with before and after values,
    honour `If-Match`, and are refused over 30 characters or as an unknown period.
  - **Outlet report group:**
    - a change applies from the business date;
    - reading a date before it gives the old group;
    - the change is audited.
  - **RLS isolation:** `report`, `report_mail_pref`, `report_mail` and `outlet_report_group` are invisible across
    tenants.
- [ ] **Step 2: Run them and see them fail. Implement. Run them and see them pass.** Apply the migration with
  `DATABASE_OWNER_URL=… cargo run -q -p core-api -- migrate`. On the web side, add the fields to the Taxes page
  ("TDL registration number", and "VAT return period": Monthly or Quarterly) and the Outlets page ("Report group": Food,
  Bar or Other, with the hint "used for SLTDA's TDL form; a change applies from today's business date"). Then run
  `bun run api:schemas && bun run codegen`, followed by `bun run check && bun run test`.
- [ ] **Step 3:** Commit: `feat(reports): schema, permissions and report settings`.

### Task 2: Everyday reports and daily lists

**Files:**
- Create: `modules/reports/Cargo.toml`, `modules/reports/src/lib.rs`, `modules/reports/src/everyday.rs`,
  `modules/reports/tests/everyday.rs`
- Modify:
  - `Cargo.toml` (workspace member);
  - `crates/core-api/src/graphql.rs`: `reportSummary(propertyId, from, to)` and `dailyList(propertyId, date, kind:
    ARRIVALS|DEPARTURES|IN_HOUSE)`, needing `ReportsView`. `dailyList` also accepts `ReservationsView`.
- Test: `modules/reports/tests/everyday.rs`, `crates/core-api/tests/reports.rs` (new)

**Interfaces:**
- Consumes: `audit::daily_stats(tx, property, from, to)`, `BreakdownRow`. The daily lists come from the same SQL the
  night audit review uses (`audit::ARRIVALS_SQL` and `DEPARTURES_SQL`), or the reservation module's list queries
  for in-house guests; reuse, don't copy.
- Produces:
  - `reports::summary(tx, property, from: Date, to: Date) -> Result<Summary, ReportsError>`, where `Summary`
    contains:
    - `closed_from` and `closed_to` (`Option<Date>`) and `not_closed: Vec<Date>`;
    - `headline: Headline { occupancy_bp, adr, revpar, room_nights, total_revenue }`;
    - `last_year: Option<Headline>`;
    - `days: Vec<DayRow>`;
    - `breakdowns: BTreeMap<Dimension, Vec<BreakdownTotal { key, room_nights, room_revenue, revenue }>>`.
  - `reports::daily_list(tx, property, date, kind: ListKind) -> Result<Vec<ListRow>, ReportsError>`.
  - `reports::last_year_range(kind: RangeKind, from, to) -> Option<(Date, Date)>`.
  - `ReportsError { NotFound, Invalid(String), Database }`, in the style of the other modules.
- **Headline maths:**
  - occupancy = Σ rooms_sold / Σ rooms_available;
  - ADR = Σ room_revenue / Σ rooms_sold;
  - RevPAR = Σ room_revenue / Σ rooms_available;
  - each rounded once, never averaged from daily ratios.
- **Last year:**
  - a whole calendar month compares with the same month;
  - a Monday-to-Sunday week compares with the week 364 days earlier;
  - any other range compares with the same dates, and 29 February has no counterpart.
  - It is `None` unless every day of that range is closed.
- **Range limit:** at most 366 days (`Invalid`).

- [ ] **Step 1: Write the tests** (seed `daily_stats` rows directly through `audit::store`)
  - `headline_is_ratio_of_sums`: two days, 10 of 20 rooms sold at 1,000, then 20 of 20 at 3,000. Occupancy 7,500 bp,
    ADR 2,333, RevPAR 1,750.
  - `open_days_are_listed_not_estimated`: the range ends past the business date, so `not_closed` lists those days
    and the headline covers only the closed ones.
  - `last_year_only_when_fully_closed`: half a year back gives `None`; all closed gives `Some`.
  - `week_compares_364_days_back`, `month_compares_same_month`, `feb29_custom_range_has_no_counterpart`.
  - `breakdowns_sum_over_range`.
  - `range_over_366_days_is_invalid`.
  - core-api: a front_desk user gets `dailyList` but is refused `reportSummary`; a manager gets both.
- [ ] **Step 2: Fail, implement, pass. Then fmt and clippy.**
- [ ] **Step 3:** Commit: `feat(reports): everyday reports and daily lists`.

### Task 3: TDL (Form A)

**Files:**
- Create: `modules/reports/src/liability/mod.rs`, `modules/reports/src/liability/lines.rs`,
  `modules/reports/src/liability/tdl.rs`, `modules/reports/tests/tdl.rs`

**Interfaces:**
- Consumes: `folio::outlet_report_group` (Task 1).
- Produces:
  - `liability::lines::revenue_lines(tx, property, from, to) -> Vec<RevenueLine>`, the one shared reader of
    closed-date revenue. Each `RevenueLine` holds `{ business_date, folio_id: Option<Uuid>, category, outlet:
    Option<Uuid>, net, taxes: Vec<(code, kind, presentation, amount)> }` in base LKR. Its rules for what counts as
    revenue (reversals, transfers, adjustments, POS bills settled at the outlet) are exactly those of
    `audit::stats::compute` and `DAY_LINES_SQL`; generalise that query over a date range rather than writing a second
    rule set.
  - `reports::tdl(tx, property, quarter: FiscalQuarter) -> Result<TdlForm, ReportsError>`, where `TdlForm` holds
    `{ name, registration_no, period_label, room, food, bar, other, total_turnover, service_charge, vat, base,
    rate_bp: 100|50, tdl, not_closed: bool }`.
  - `FiscalQuarter { fy_start_year: i32, q: 1..=4 }` with `range()` and `label()`. The label is like
    `"April – June (2026)"`, matching SLTDA's sample.
- **The sales lines:**
  - **Room:** category `room`.
  - **Food:** `meal_plan`, plus `pos:<outlet>` lines whose outlet group on the line's date is food.
  - **Bar:** `pos:<outlet>` lines of bar outlets.
  - **Other:** `fee`, `laundry`, `misc`, plus `pos:<outlet>` lines of other outlets.
  - Each line is its net + its `SC` + its `VAT` amounts. Passed-on TDL and SSCL are excluded; absorbed SC or VAT
    still count.
  - `service_charge` and `vat` are the SC and VAT within those lines.

- [ ] **Step 1: Write the tests**
  - `sltda_worked_example`: seed lines producing Room 55,800, Food 237,973 (of which SC 23,797) and Bar 3,183,537,
    with no VAT and no other. Assert total 3,477,310, base 3,453,513, rate 1%, TDL 34,535.
  - `half_percent_under_three_million`: SLTDA's 0.5% example (Room 2,900, Food 22,900 with SC 2,290, Bar 2,323,520)
    gives base 2,347,030 and TDL 11,735.
  - `outlet_group_change_does_not_restate_a_closed_quarter`.
  - `credit_reversal_counts_in_its_own_quarter`.
  - `passed_on_levies_are_not_sales`.
  - `quarter_with_open_days_is_flagged`.
- [ ] **Step 2: Fail, implement, pass. Then fmt and clippy.**
- [ ] **Step 3:** Commit: `feat(reports): TDL in SLTDA's Form A`.

### Task 4: SSCL with the foreign-currency exemption

**Files:**
- Create: `modules/reports/src/liability/sscl.rs`, `modules/reports/src/liability/settlement.rs`,
  `modules/reports/tests/sscl.rs`

**Interfaces:**
- Consumes: `revenue_lines` (Task 3).
- Produces:
  - `reports::sscl(tx, property, quarter: CalendarQuarter) -> Result<SsclReport, ReportsError>`, where `SsclReport`
    holds:
    - `months: [SsclMonth; 3]`, each `{ month, turnover, exempt_same_month, exempt_earlier_months, bad_debts: 0,
      liable, levy, foreign_cash, due }`;
    - `quarter_totals`, `return_due`, `sltda_registered: bool` and `method_note: &'static str`.
  - `CalendarQuarter { year, q }`.
  - `settlement::segments(tx, property, upto: Date) -> Vec<Segment { folio_id, settled_on: Date, exempt_share_bp:
    u32, covers_from: Date }>`.
- **Turnover:** revenue lines' net plus every tax except VAT (passed-on TDL and SSCL stay in), by month.
- **The settlement algorithm** (it decides the figures, so it is spelled out):

```text
For each folio, walk its lines and payments in business-date order (payments: refunds negative, transfers out
negative, as stats::compute treats them). After each business date D, if the base balance (lines' totals − payments)
is 0 and something happened since the previous settlement point, D is a settlement point.
A segment is (previous point, D]: its lines are covered by D. Its share = Σ base of payments in the segment with
currency <> 'LKR' and method in (card_terminal, bank_transfer) ÷ Σ base of all positive payments in the segment,
clamped to [0, 1] (in basis points).
In month M, exempt_same_month = Σ over segments settled in M of share × turnover of their lines dated in M;
exempt_earlier_months = Σ over segments settled in M of share × turnover of their lines dated before M.
Lines not yet covered by a settlement point at M's end are not exempt in M. Only settlement points up to the
property's last closed date count, so a closed month never changes.
foreign_cash = Σ base of payments in M with currency <> 'LKR' and method = cash (shown, never exempt).
Without property.sltda_licence, every exempt figure is 0 and the report says "Not SLTDA registered: no exemption".
```

- **The method note** (printed on the report, so the hotel's accountant can check it): "Exempt: services paid in foreign
  currency by card or bank transfer (SSCL Act No. 25 of 2022, First Schedule Part II item 10), measured when each bill
  is settled."

- [ ] **Step 1: Write the tests**
  - `usd_card_share_exempt_lkr_cash_not`: 60% USD card and 40% LKR cash exempts 60% of the folio's turnover.
  - `foreign_cash_listed_not_exempt`.
  - `unsettled_folio_adjusts_month_it_settles`: turnover in July, settled in August, shows as `exempt_earlier_months`
    in August; July's figures are identical before and after.
  - `resettled_folio_not_double_counted`.
  - `not_sltda_registered_exempts_nothing`.
  - `due_dates`: Q3 2026 gives 20 Aug, 20 Sep, 20 Oct, and the return 20 Oct.
  - `levy_is_two_and_half_percent_of_liable`.
- [ ] **Step 2: Fail, implement, pass. Then fmt and clippy.**
- [ ] **Step 3:** Commit: `feat(reports): SSCL with the foreign-currency exemption`.

### Task 5: VAT, the IRD schedules and the service charge report

**Files:**
- Create: `modules/reports/src/liability/vat.rs`, `modules/reports/src/liability/service_charge.rs`,
  `modules/reports/tests/vat.rs`
- Modify: `crates/core-api/src/routes/` (a new `reports.rs`: `GET …/reports/vat/{period}/schedule01.csv` and
  `…/schedule04.csv`, needing `ReportsLiability`), `crates/core-api/src/routes/mod.rs`, `openapi.rs`, and the GraphQL
  liability queries `tdlReport`, `ssclReport`, `vatReport` and `serviceChargeReport`
- Test: `modules/reports/tests/vat.rs`, `crates/core-api/tests/reports.rs`

**Interfaces:**
- Produces:
  - `reports::vat(tx, property, period: VatPeriod) -> Result<VatReport, ReportsError>`. `VatReport` holds `{ period_code,
    value_of_supply, output_vat, credit_notes_value, credit_notes_vat, net_value, net_vat, schedule01:
    Vec<Schedule01Row>, schedule04: Vec<Schedule04Row> }`.
  - `VatPeriod::{Month(year, month), Quarter(year, q)}` with `code()`: `2630` for Q3 2026, and `2632` for August 2026
    (Q3, month 2 of the quarter).
  - `reports::schedule01_csv(&VatReport) -> String` and `schedule04_csv`.
  - `reports::schedule_file_name(tin, schedule: 1|4, period_code, today) -> String`.
  - `reports::service_charge(tx, property, month) -> ServiceChargeReport { by_outlet: Vec<(outlet code, collected,
    absorbed)>, rooms: (collected, absorbed), total }`.
- **The rows:**
  - **From invoices,** `kind = 'invoice'`, dated by `data.issued_on` within the period. VAT and value come from the
    invoice snapshot's taxes and lines, so the CSV matches the printed invoice.
  - **The value of supply** is the invoice's taxable value excluding VAT. Under the cascade that includes SC and
    passed-on TDL and SSCL.
  - **A purchaser with a TIN** gets one row per invoice per VAT rate.
  - **No TIN:** combined per rate under `100902885`, as in the Global Constraints.
- **Credit notes** (`kind = 'credit_note'`) go to Schedule 04. Its columns: `Serial No, TIN No, Invoice Date, Invoice
  No, Tax Credit / Tax Debit Note, Date of Tax Credit / Tax Debit Note, Tax Credit No. / Tax Debit Note No., Value of
  Tax Credit Note / Tax Debit Note, VAT Amount, Issued by Me`. The note is `Credit`, issued by `Y`, and the TIN is the
  purchaser's or `100902885`.
- **Refusals:**
  - not VAT-registered: `Invalid("This property is not VAT registered")`;
  - no TIN: `Invalid("Set the property's TIN on Settings → Taxes and invoices first")`.
- **CSV:**
  - it has a header row;
  - fields are quoted only when they contain a comma or a quote;
  - amounts are plain integers;
  - lines end with CRLF.

- [ ] **Step 1: Write the tests**
  - `schedule01_exact_columns_and_dates`: two TIN invoices and three walk-in invoices in Q3 2026 give two rows plus
    one combined row: `INCL01_2630`, dated `09/30/2026`, TIN `100902885`, `Inclusive Invoices at 18%`.
  - `period_codes`: Q3 2026 is `2630` and August 2026 is `2632`.
  - `file_name`: `300767028_VAT_SCHEDULE01_2630_20261020_ORIGINAL_V1.csv`.
  - `credit_note_reduces_its_own_period_and_is_schedule04`.
  - `not_vat_registered_refused`, `no_tin_refused` (also at the REST route: 422, and no file).
  - `service_charge_by_outlet_and_rooms`.
  - core-api: a manager gets 403 on the liability queries and the CSV route; an accountant gets 200 with
    `content-type: text/csv` and a `content-disposition` carrying the file name.
- [ ] **Step 2: Fail, implement, pass.** Add an index only if the VAT test's query plan needs one, and say so in the
  report. Regenerate the schemas. Then fmt and clippy.
- [ ] **Step 3:** Commit: `feat(reports): VAT schedules and service charge`.

### Task 6: PDFs: the generic report render

**Files:**
- Modify:
  - `crates/media-types/src/lib.rs`:
    - `ReportRenderJob { tenant_id, report_id, template: ReportTemplate, data: serde_json::Value }`;
    - `ReportTemplate { Summary, DailyList, TdlFormA, Sscl, Vat, ServiceCharge }`;
    - `MediaResult::ReportRendered { tenant_id, report_id, key }`;
    - `storage::report_pdf_key(tenant, report) -> "t/{tenant}/reports/{report}.pdf"`.
  - `crates/media-svc/src/render.rs`: route `report-render` pushes to the template. Reuse the invoice path's body
    limits, fonts and storage; add `MAX_REPORT_BYTES` of 4 MiB of JSON.
  - `crates/core-api/src/routes/internal.rs` and `modules/reports/src/store.rs`: apply `ReportRendered` (sets
    `report.status = ready`, `pdf_key`, `rendered_at`) or `Failed` (`status = failed`, raises the `reports` ops
    alert).
  - `crates/core-api`:
    - `POST …/reports/render` with `{ kind: on_demand, report: summary|daily_list|tdl|sscl|vat|service_charge,
      params }` creates an `on_demand` report and enqueues its render. Needs `ReportsView`, or `ReportsLiability` for
      the liability ones.
    - `GET …/reports/{id}` gives the status and, when ready, a link signed for 5 minutes. The link is audited as
      `report.viewed`.
  - The `PUSH_ENDPOINT_REPORT_RENDER` config and the README env table.
- Create: `crates/media-svc/templates/reports/{summary,daily_list,tdl_form_a,sscl,vat,service_charge}.typ`
- Test: `crates/media-svc/tests/render.rs` (extended), `crates/core-api/tests/reports.rs`

**Interfaces:**
- Consumes: the report data structs of Tasks 2–5, which must be `Serialize`.
- Produces:
  - `reports::queue_render(tx, tenant, property, kind, period_start, period_end, template, data) -> Result<Uuid,
    ReportsError>`. It inserts a `report` row as `rendering` and writes the `report-render` outbox row in the same
    transaction. It is idempotent on the unique key: it returns the existing id and never enqueues twice.
- **Templates:**
  - each shows the property's legal name and the period, and is A4 portrait;
  - `tdl_form_a` reproduces SLTDA's Form A, with the GovPay line, "Due before the end of the following quarter", and
    signature and date lines;
  - `sscl` prints `method_note`.

- [ ] **Step 1: Write the tests**
  - media-svc: each template renders a fixture JSON to a PDF that starts with `%PDF-`, containing the strings
    `"Form A"`, `"34,535"` (TDL fixture), `"Exempt: services paid in foreign currency"` (SSCL) and `"2630"` (VAT).
    Extract the text with the existing PDF test helper if there is one; otherwise assert on the Typst intermediate.
  - core-api:
    - `queue_render_is_idempotent`: the same key twice gives one row and one outbox job;
    - `on_demand_render_round_trip`: through media-svc in-process, as the invoice tests do; poll to `ready` and get a
      signed link;
    - `render_failure_raises_reports_alert`;
    - `liability_render_needs_reports_liability`.
- [ ] **Step 2: Fail, implement, pass.** Run `cargo test -p media-svc --test render`, `cargo test -p core-api --test
  reports` and `cargo test -p media-types -p reports`. Then fmt and clippy.
- [ ] **Step 3:** Commit: `feat(reports): PDF rendering for reports`.

### Task 7: The close's `render_reports` step and the emails

**Files:**
- Modify:
  - `modules/audit/src/close.rs`: `RenderReports` runs after `AdvanceBusinessDate` in its own committed step. It
    calls a hook, so `audit` does not depend on `reports`; follow the existing `CloseHooks` pattern, or a function
    pointer passed by jobs-svc.
  - `modules/reports/src/periods.rs`: `due_periods(tx, property, closed_through: Date) -> Vec<(Kind, Date, Date)>`
    gives every weekly, monthly, TDL quarter, SSCL quarter, VAT period and SC month whose last day is ≤
    `closed_through`, with no `report` row.
    - **Builder's call:** the scan reaches back 13 months at most.
    - For a property created recently, it starts from its first closed date.
  - `crates/jobs-svc/src/tick.rs` (wire the hook).
  - `crates/mail/src/lib.rs`:
    - `Message.attachments: Vec<Attachment { filename, content_type, bytes }>`;
    - `SesMailer` sends raw MIME (multipart/mixed) when there are attachments;
    - `DevMailer` writes the attachment beside the message;
    - `MailJob::Report { tenant_id, report_id, user_id }`.
  - `crates/jobs-svc/src/mail.rs`: build the report email:
    - **subject:** `"<Property> weekly report, 19–25 Oct 2026"` or `"monthly report, October 2026"`;
    - **body:** occupancy, ADR, RevPAR and total revenue;
    - **attachment:** the PDF read from storage.
    - A missing PDF is retryable.
  - `modules/reports/src/store.rs`: on `ReportRendered` for `weekly` or `monthly`, insert `report_mail` rows (`on
    conflict do nothing`) and write one `mail-send` outbox row per new row. Recipients are the property's owners and
    managers, minus anyone whose `report_mail_pref` is off.
  - The GraphQL and REST endpoints for a user's own `report_mail_pref` (`reportEmails(propertyId)`,
    `setReportEmail(propertyId, kind, enabled)`).
- Test: `modules/audit/tests/close.rs`, `modules/reports/tests/periods.rs`, `crates/mail/tests/*`,
  `crates/jobs-svc/tests/mail.rs` (or the existing mail test file), `crates/core-api/tests/reports.rs`

**Interfaces:**
- Consumes: `queue_render` (Task 6), the report computations (Tasks 2–5).
- Produces: `reports::render_due(tx, tenant, property, closed_through) -> Result<usize, ReportsError>`, the hook body.
  It queues every due period's render, with the data computed at queue time.

- [ ] **Step 1: Write the tests**
  - **periods:**
    - `sunday_close_queues_weekly`;
    - `month_end_queues_monthly_sscl_sc_and_vat_when_monthly`;
    - `quarter_end_march_queues_tdl_q4`;
    - `catch_up_over_week_and_month_queues_both_once`.
  - **close:**
    - `render_reports_runs_after_advance`: the business date has advanced even when the hook errs; the hook's error
      raises the `reports` alert, and the next tick retries.
    - `rerun_close_does_not_requeue`.
  - **mail:**
    - `ses_raw_mime_with_attachment` (the MIME structure, base64 body);
    - `dev_mailer_writes_attachment`;
    - `report_mail_job_debug_has_no_secrets`.
  - **core-api / reports:**
    - `ready_weekly_mails_owners_and_managers_once`;
    - `opted_out_user_not_mailed`;
    - `redelivered_render_result_does_not_double_mail`.
- [ ] **Step 2: Fail, implement, pass.** Run the touched test binaries. Then fmt and clippy.
- [ ] **Step 3:** Commit: `feat(reports): close renders due reports and emails weekly and monthly`.

### Task 8: Web: Reports pages, daily lists, liability pages and email settings

**Files:**
- Create:
  - `web/pms/src/routes/(app)/p/[property]/reports/+page.svelte`: the everyday report, with a range picker (This
    week, Last week, This month, Last month, Custom), the headline cards with last year, the day table, the breakdown
    tables, and **Download PDF**.
  - `reports/lists/+page.svelte`: a date picker, the Arrivals, Departures and In-house tabs, printable, plus
    **Download PDF**.
  - `reports/tax/+page.svelte`: the period lists for TDL (fiscal quarters), SSCL (quarters), VAT (by the property's
    return period) and Service charge (months). Opening one shows the report, **Download PDF**, and for VAT
    **Download Schedule 01 (CSV)** and **Download Schedule 04 (CSV)**.
  - `web/pms/src/lib/reports.ts`: formatters, range presets and the `downloadReport(kind, params)` flow (POST render,
    poll `GET …/reports/{id}` every second up to 30 s, then open the signed link).
  - a section in the user's account settings: "Report emails for <property>: Weekly, Monthly".
- Modify: the property nav (a **Reports** link for `ReportsView`, a **Tax reports** link for `ReportsLiability`, and
  **Daily lists** for `ReservationsView`).
- Test: `web/pms/src/lib/reports.spec.ts`, `web/pms/tests/e2e/reports.spec.ts`

**Interfaces:**
- Consumes: the GraphQL queries `reportSummary`, `dailyList`, `tdlReport`, `ssclReport`, `vatReport`,
  `serviceChargeReport`, `reportEmails` and `setReportEmail`; the REST CSV and render routes (Tasks 2–7).

**Copy:**
- **Not closed yet:** "Not closed yet: figures cover <from>–<to>", and open days show "Not closed yet" in the day table.
- **VAT refusals:** the server's own messages.
- **SSCL:** the method note, shown under the table.

- [ ] **Step 1: Write the unit tests**
  - `presets`: on Wednesday 2026-10-21, This week is 19–25 Oct, Last week is 12–18 Oct, This month is 1–31 Oct and
    Last month is 1–30 Sep.
  - `formatLkr(3453513) === 'Rs 3,453,513'`, `formatOccupancy(7500) === '75.0%'`.
  - `fiscalQuarterLabel` gives `April – June (2026)`.
- [ ] **Step 2: Write `tests/e2e/reports.spec.ts`**
  - Seed a property and close several days, using the night audit e2e helpers.
  - A manager opens Reports → Last week and sees the headline figures. Download PDF gives a `%PDF` response.
  - An accountant opens Tax reports → TDL for the closed quarter, sees Form A's rows, and Download PDF works.
  - On a VAT-registered property with a TIN, Download Schedule 01 gives a CSV with the header row.
  - A front desk user sees Daily lists but not Reports.
  - Turning Weekly off in account settings persists after a reload.
- [ ] **Step 3: Implement.** Run `bun run check && bun run lint && bun run test`, then the e2e specs in the E2E env.
  Commit: `feat(web): reports, tax reports and daily lists`.

### Task 9: Performance gates and docs

**Files:**
- Modify:
  - `crates/core-api/tests/perf.rs`: `reportSummary` for 31 days of a 200-room property, p95 under 100 ms; `tdlReport`
    and `ssclReport` for a quarter of the same property with its folios, p95 under 300 ms.
  - the night audit perf test: a month-end close with `render_reports` adds under 5 s.
  - `README.md`:
    - the Performance gates lines;
    - the env table (`PUSH_ENDPOINT_REPORT_RENDER`);
    - a walkthrough, "Trying reports by hand": close a week, see the weekly email in the dev outbox with its PDF,
      open the Reports pages, read the TDL Form A, SSCL and VAT reports, download the Schedule 01 CSV, and turn an
      email off.
  - `docs/ARCHITECTURE.md` §7.6 and §14: the researched answers replace the open questions, with the spec's sources.
  - `docs/ROADMAP.md`: Phase 7d done, with the deviations.

- [ ] **Step 1:** Write the perf tests. Run them with `cargo test --release -p core-api --test perf -- --ignored
  --nocapture --test-threads=1` (only the new ones, by name). Expected: under the gates. If a gate misses, add an index
  or tighten the query; never raise a gate.
- [ ] **Step 2:** Write the docs. Commit: `docs: Phase 7d reports, walkthrough and gates`.
