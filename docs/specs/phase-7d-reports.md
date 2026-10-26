# Phase 7d: Reports and Liability Reports (Spec)

**Depends on:** Phase 7b ([phase-7b-night-audit.md](phase-7b-night-audit.md)) for the night audit, `daily_stats`,
`daily_stats_breakdown`, the close's steps and `jobs-svc`. It also uses Phase 7a's folios, payments, tax lines and invoices,
Phase 8a's mail (SES through jobs-svc `/mail`), and media-svc's Typst PDF rendering. **Implementation plan:** written at
phase start in `docs/superpowers/plans/`.

Agreed with the owner, 2026-10-26:
- **Who files:** someone at the hotel, not an outside accountant or tax specialist. Each liability report is a finished
  sheet they copy into the official form or upload, never raw data they must work through.
- **How often the business reports are read:** weekly or monthly. They are both on screen and emailed as PDFs.
- **The tax reports and the everyday reports were played back and approved as described below.**

The owner had no accountant answers, so the rules were researched from the official sources (listed at the end). "Builder's
call" marks a decision that was not discussed with the owner.

## Done when

- The **Reports** pages exist:
  - occupancy, ADR and RevPAR over a range;
  - revenue by room type, segment, source, residency and outlet;
  - arrivals, departures and in-house lists.
- Owners and managers get a **weekly PDF** (Monday, for the week before) and a **monthly PDF** (the 1st, for the month
  before) by email, each switchable off per person.
- The **liability reports** exist, on screen and as PDF:
  - **TDL** quarterly, in SLTDA's Form A layout;
  - **SSCL**, monthly figures for the quarterly return;
  - **VAT** per return period, with IRD's Schedule 01 CSV;
  - **service charge** collected.
- Every figure comes from closed business days, so it never changes after the fact.

## Where the numbers come from

- **Closed days only.** Every report reads 7b's `daily_stats` and `daily_stats_breakdown` snapshots, plus folio lines,
  tax lines and payments whose business date is closed.
  - A range that reaches into days not yet closed shows them as "not closed yet", never estimates them.
- **The business date is what counts**, never the wall clock.
  - A refund, a reversal or a credit note reduces the period its own business date falls in. The original period is
    not touched.
  - **Builder's call:** this matches how SSCL and VAT are accounted (on an accrual basis, adjusting in the period of the
    credit note).
- **Money is whole LKR** on every report, with the same rounding rules as invoices: sums of base amounts, rounded once
  per printed figure.
- **Taxes absorbed rather than passed on are included** wherever the hotel owes them (ARCHITECTURE §7.6).

## The everyday reports

### On screen: Reports

**Range.** A range picker with this week, last week, this month, last month and custom dates. Weeks run Monday to Sunday.

**Headline figures:**
- occupancy;
- ADR (room revenue per room sold);
- RevPAR;
- room nights sold;
- total revenue.
- **Builder's call:** each shows the same period last year when a full year of closed days exists for it, else nothing.

**Day table.** Each day's figures, as `daily_stats` holds them.

**Breakdowns.** Room nights, room revenue and total revenue by room type, segment, source, residency (resident or
foreign) and outlet.

**Daily lists.** Arrivals, departures and in-house guests for one chosen day, printable.

**Download PDF.** Every report and list can be downloaded as a PDF of what is on screen.

### By email

| Report | Sent | Covers |
|---|---|---|
| Weekly | Monday | The previous Monday to Sunday |
| Monthly | The 1st | The previous month |

- **Who gets them:** every owner and manager of the property.
  - Each person can turn each email off for themselves (**Builder's call:** under their own account settings).
- **When they go out:**
  - A report is sent once the night audit has closed the period's last day. It goes from the close's new
    `render_reports` step, in the property's time zone, not at a fixed clock time.
  - **Builder's call:** if the close is late, the report goes when the close happens. Each report is sent once:
    keyed by property, kind and period, so a re-run close does not send it again.
- **The email:** a short summary (occupancy, ADR, RevPAR, total revenue) and the PDF attached.
  - **Builder's call:** attached, not a link, since the hotel's links are signed for minutes.
  - The PDF is rendered by media-svc and stored privately, as invoices are.

### Daily PDFs dropped

- The roadmap outline had the close render every report every day. It is **dropped**, since the owner reads weekly
  and monthly.
- The `render_reports` step renders only:
  - the weekly and monthly reports;
  - each liability report whose period the close has just finished (a quarter for TDL, a month for SSCL, the VAT
    period).
- Those liability PDFs are stored for the hotel to download. They are **not emailed**.

## The liability reports

**Who sees them:** owners and accountants, through a new `ReportsLiability` permission. They are listed by period, and
each is on screen and as a PDF.

### TDL (SLTDA), quarterly

**Periods:** quarters of the April–March financial year:

| Quarter | Months |
|---|---|
| Q1 | April–June |
| Q2 | July–September |
| Q3 | October–December |
| Q4 | January–March |

**Layout:** exactly SLTDA's **Form A** (tourist hotels):
- Name, TDL registration number and period;
- Room Sales, Food Sales, Bar Sales, Other Sales, then Total Turnover;
- Deduction: Service Charge, VAT;
- Total Turnover less deductions;
- TDL at the rate.

**Rate:** 1%, or 0.5% when the quarter's turnover is not over Rs 3,000,000. The rate is chosen from that quarter's
figure, as SLTDA states it, **not** the property's `tdl_small_turnover` billing setting. The report says which rate applied.

**The four sales lines:**
- **Room Sales:** room revenue.
- **Food Sales:** meal-plan revenue, plus the outlets marked *food*.
- **Bar Sales:** the outlets marked *bar*.
- **Other Sales:** fees, other revenue and the outlets marked *other*.
- **Builder's call:**
  - Outlets gain a **report group** (food, bar or other), default food, set on Settings → Outlets.
  - Each sales line is its net revenue plus the service charge and VAT on it. These are the two amounts Form A then
    deducts.
  - TDL and SSCL passed on to guests are levies collected, not sales, and are left out.

**The footer:**
- "Due before the end of the following quarter".
- "Pay through GovPay": SLTDA has required GovPay for TDL since 22 December 2025.
- A place to sign (Director / Proprietor) and date.

**Property settings:** the property gains a **TDL registration number** (optional text, up to 30 characters), printed on
the form.

### SSCL (IRD), monthly figures for the quarterly return

**Periods:** calendar quarters (January, April, July and October starts), each shown by month.

**Each month shows:**
- **Turnover:** every revenue line, excluding VAT.
  - Hotels are "services", so 100% of turnover is liable.
  - **Builder's call:** passed-on TDL and SSCL are part of the sum receivable and stay in.
- **Less, the exempt foreign-currency turnover** (see below).
- **Less, bad debts:** **Builder's call:** none until the system writes off debts, shown as Rs 0 with that note.
- **The liable turnover, and the levy at 2.5%.**
- **The payment due date:**
  - the 20th of the next month for the quarter's first two months;
  - the 20th of the month after the quarter for the third.

The quarter's totals and the return's due date (the 20th of the month after the quarter) close the report.

**The foreign-currency exemption** (SSCL Act, First Schedule, Part II, item 10):
- Hotel, guest house and restaurant services are exempt when payment is received in foreign currency through a bank and
  the business is registered with SLTDA.
- The property gains an **SLTDA registered** setting. Without it, nothing is exempt and the report says why.
- **Builder's call:** what "received in foreign currency through a bank" covers:
  - **Payments counted:** in a currency other than LKR, made by card terminal or bank transfer. Foreign-currency cash
    is not "through a bank", and it is listed separately so the hotel can see it.
  - **How turnover is matched to them:** each folio's turnover is split in the proportion its settled payments were
    made that way, measured when the folio is settled (zero balance).
  - **Unsettled folios:** a folio not yet settled at the month's end counts as not exempt that month. It is adjusted in
    the month it settles.
  - The report states this method in a note, so the hotel's accountant can check it.

### VAT (IRD), per return period

- **Only for VAT-registered properties.** Others see "This property is not VAT registered".
- **Period:** the property gains a **VAT return period** setting, monthly or quarterly (default quarterly). Periods use
  IRD's period code: `YYQ0` for a quarter, `YYQM` for a month.
- **On screen and PDF:** for the period:
  - value of supply;
  - output VAT at 18%;
  - credit notes issued;
  - net figures, matching the return's cages A and 0.
- **Schedule 01 CSV**, as IRD's e-filing takes it:
  - **Columns, in order:** Serial No, Invoice Date (MM/DD/YYYY), Tax Invoice No, Purchaser's TIN, Name of the
    Purchaser, Description, Value of supply, VAT Amount.
  - **Rows:**
    - Each issued tax invoice whose purchaser has a TIN is one row.
    - Invoices without a purchaser TIN are one combined row per VAT rate under the dummy TIN `100902885`. That row
      has the period's last day as its date, a description "Inclusive Invoices at 18%", and an invoice number
      `INCL01_<period code>`.
  - **File name:** `<TIN>_VAT_SCHEDULE01_<period code>_<YYYYMMDD>_ORIGINAL_V1.csv`, dated the download day.
  - **Builder's call:** a header row, matching IRD's downloadable template.
- **Credit notes:** they go in IRD's Schedule 04 file. **Builder's call:** produced the same way, with "Issued by me" `Y`.
- **Not in scope:** input VAT on purchases (the hotel's own bills are not in the PMS), and RAMIS's web API, which is
  CSV upload only for now.

### Service charge collected

**Periods:** months.

**Shows:**
- the service charge collected per outlet and on rooms, for each month;
- any service charge that was absorbed.

**Builder's call:** this is a statement of what was collected, for the hotel's own staff distribution. It does not
compute who gets what.

## Settled for the builder

All of these are **Builder's calls**, made after the builder check.

- **Where the liability figures come from.**
  - **Computed live** from folio lines, their tax lines, payments, invoices and credit notes whose business date is
    closed. Phase 7a dates each line with its business date (migration 0032), so every day since then is reportable,
    with no backfill.
  - The everyday reports read the 7b snapshots. The liability reports don't, since the snapshots lack per-outlet tax
    splits and payment currencies.
  - Add indexes as the performance gates need them.
- **"Never changes after the fact" and the SSCL exemption.**
  - A month's SSCL figures use only what was settled by the end of that month, so they never change later.
  - When a folio with turnover in earlier months settles, its exempt share appears in the month it settles, as a
    separate line: "Exemption for folios settled this month (turnover of earlier months)".
  - Stored PDFs and on-demand renders therefore always agree. The stored PDF is the record.
- **Outlet report groups.**
  - A change takes effect from the property's next unclosed business date. It is kept as a small history
    (outlet, group, effective from) and audited.
  - A closed quarter's TDL is never restated.
- **The TDL test fixture.** SLTDA's worked example, with Other and VAT both 0:
  - Total Turnover 3,477,310.00;
  - less Service Charge 23,797.00;
  - Total Turnover less deductions 3,453,513.00;
  - TDL 34,535.13, shown as 34,535.

  The Rs 3,000,000 threshold is tested against Total Turnover less deductions (the levy's base). SLTDA's 0.5% example
  agrees: 2,347,030.00 gives 11,735.15.
- **Credit notes** exist since Phase 7a, with their own serial and business date.
  - Schedule 04 references the original invoice's number and date, and the purchaser's TIN, or the dummy TIN when it
    had none.
  - VAT rows are grouped by the rate on each tax line. An invoice with several rates contributes one row per rate.
    Today every row is 18%.
- **Exactly-once emails and the close.**
  - `render_reports` runs **after** `advance_business_date` (a migration widens `audit_run_step.step`), so a rendering
    or mail failure never blocks the business date.
  - A failure raises an ops alert (`kind` `reports`), and jobs-svc retries on its next tick.
  - A new table records each sent report under a unique (property, kind, period) key.
  - A close that finishes several periods at once, after a late catch-up, sends every report whose period is now fully
    closed and not yet sent.
- **Per-user email settings** are per user and per property: a small table keyed (user, property, kind), with no row
  meaning on.
- **Roles:**
  - `ReportsView`: owner, manager and accountant.
  - `ReportsLiability`: owner and accountant.
  - These are the existing role names (`owner`, `manager`, `accountant`).
- **Same period last year.**
  - **A month** compares with the same calendar month.
  - **A week** compares with the week 52 weeks earlier, so the same weekdays line up.
  - **A custom range** compares with the same dates, 29 February having no counterpart.
- **PDFs.**
  - media-svc's `/render` is invoice-specific today. It gains a generic report job: a template name plus JSON data,
    rendered by the bundled Typst templates and stored privately.
  - On-screen downloads render on demand from the same closed data.
- **VAT value of supply** is the taxable value excluding VAT. Under the platform's cascade order (ARCHITECTURE §7.6) that
  includes service charge and passed-on TDL and SSCL.
  - The file name and the return use the property's existing `tin`.

## Permissions and settings

- **`ReportsView`:** the everyday reports. Owners, managers and accountants.
  - Front desk sees only the daily lists, through their existing reservation permissions. **Builder's call:** no new
    permission is needed for them.
- **`ReportsLiability`:** the liability reports. Owners and accountants.
- **New property settings**, on Settings → Taxes and invoices, saved with `If-Match` and audited like the existing ones:
  - TDL registration number;
  - SLTDA registered;
  - VAT return period.
- **New outlet field:** report group, on Settings → Outlets.
- **New per-user settings:** the weekly and monthly report emails, each on or off (default on).

## Tests that must exist

- **TDL:**
  - A fixture quarter reproduces SLTDA's own worked example: Room 55,800, Food 237,973, Bar 3,183,537, SC 23,797, giving
    TDL 34,535.13. The PMS rounds the TDL to whole rupees, so it shows 34,535.
  - The 0.5% rate applies to a quarter under Rs 3,000,000.
- **SSCL:**
  - Foreign-currency card and bank-transfer payments exempt their share of a settled folio's turnover.
  - Foreign cash does not.
  - An unsettled folio adjusts the month it settles.
  - A property not SLTDA registered exempts nothing.
- **VAT:**
  - The Schedule 01 CSV has the exact columns, date format, the dummy-TIN combined row per rate and the file name.
  - A credit note reduces its own period.
- **Closed days only:** a range reaching past the business date shows the open days as not closed, and never changes once
  closed.
- **Email:**
  - The weekly and monthly emails are sent exactly once per period, even when a close is re-run.
  - They are not sent to a person who turned them off.
- **e2e:**
  - Open Reports for last month and see the headline figures.
  - Download the TDL Form A PDF for a closed quarter.
  - Download the Schedule 01 CSV for a VAT-registered property.

## Performance gates

- A month's everyday report (31 days of a 200-room property) answers in under 100 ms server time.
- A quarter's liability report answers in under 300 ms.
- The `render_reports` step adds under 5 s to a close that ends a month.

## Sources

- SLTDA, *Tourism Development Levy: TDL clearance* (Form A, rates, quarters, payment):
  https://sltda.gov.lk/storage/common_media/TDL%20clearnce.pdf
- SLTDA, GovPay notice (TDL through GovPay from 22 Dec 2025):
  https://www.sltda.gov.lk/storage/common_media/customer%20information%20gov%20pay%20mandate.pdf
- *Social Security Contribution Levy Act, No. 25 of 2022* (sections 3, 8, 16 and 17; First Schedule Part II item 10;
  Second Schedule): https://www.ird.gov.lk/en/publications/Acts_SSCL/SSCL_Act_No.%2025_2022_E.pdf
- IRD, SSCL Act consolidated to 2026 (item 10 unchanged):
  https://www.ird.gov.lk/si/publications/Acts_SSCL/SSCL_Cons_Act_-_2026_Changes.pdf
- IRD, *How to file VAT* quick guide, 2025 (Schedule 01 fields, dummy TINs, file names, period codes):
  https://www.ird.gov.lk/en/eServices/Lists/FilingReturns/Attachments/5/Quick_Guide_VAT_2025_V3.pdf
