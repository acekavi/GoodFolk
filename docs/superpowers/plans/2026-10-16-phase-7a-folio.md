# Phase 7a Folio, Taxes, Desk Payments and Invoices — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or
> superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give every stay, account and reservation a folio. Charges post with Sri Lanka taxes calculated by a rules
engine, desk payments and refunds work, POS bills post, nightly room charges post idempotently, and IRD-format invoices
are issued in LKR with gapless serials and rendered to PDF.

**Architecture:**
- **New crates:**
  - `modules/tax`: a pure rules engine (no database);
  - `modules/fx`: rates, overrides, and the Central Bank `RateSource`;
  - `modules/folio`: folios, lines, payments, routing, posting, POS bills, invoices.
- **`reservations`** depends on `folio`, for the check-in, check-out and cancellation hooks. `folio` never depends on
  `reservations`; it reads the reservation tables directly.
- **core-api** wires REST, GraphQL, the daily rate task and `/internal/events`.
- **media-svc** renders invoice PDFs with Typst on a new push endpoint.
- **The SPA** gains a Folios tab and settings screens.

**Tech Stack:**
- Rust 1.97, axum 0.8, sqlx 0.9, async-graphql 7.2, tonic 0.14.
- typst 0.15, typst-pdf 0.15 and typst-assets 0.15, all Apache-2.0.
- PostgreSQL 17 with forced RLS.
- SvelteKit 2 / Svelte 5, Playwright, Bun.

**Spec:** [docs/specs/phase-7-folio-audit.md](../../specs/phase-7-folio-audit.md). It is the authority: read the
sections a task names. Design background: ARCHITECTURE.md §4.4 and §7.5–7.6.

**Execution method (the owner's standing preference):**
- Each task is implemented once on branch `phase-7-folio`, then reviewed, with fix rounds.
- A final whole-branch review and one fix wave follow.
- Code blocks are exact where they carry weight: SQL, signatures, constants, the permission table and named tests.

## Global Constraints

- **Environment** (no Docker):
  - Postgres on 5432 (`DATABASE_URL=postgres://goodfolk_owner:goodfolk_owner_dev@localhost:5432/goodfolk`).
  - RustFS on :9000 (`deploy/dev/rustfs.sh start`).
  - libjxl in `~/.cache/goodfolk-libjxl/bin`.
  - The e2e DB is `goodfolk_e2e_p7`, which the controller creates.
  - Never drop a database. Never `pkill -f`; stop only exact PIDs you started. Leave the owner's vite dev server alone.
- **Green before every commit:**
  - **Rust tasks:** `cargo test --workspace` (in the background), `cargo fmt --all --check` and
    `cargo clippy --workspace --all-targets -- -D warnings`.
  - **Web tasks:** `bun run lint && bun run check && bun run test && bun run build`, plus the default e2e suite.
  - After an API change, run `bun run api:schemas && bun run codegen` and commit the generated files.
  - Use Bun only.
- **Commits:** the exact messages given. No body, no trailer, no AI attribution. Never push.
- **Money:**
  - Every amount is `bigint` in minor units with an ISO-4217 `char(3)` currency. Never floats.
  - Exchange rates are `numeric(18,6)` (LKR per one unit of the currency).
  - A base amount is `round_half_up(amount × rate)` in base minor units, rounded with exact decimal arithmetic. Use
    the `rust_decimal` crate (MIT), never `f64`.
- **Line sources** are exactly `room`, `meal_plan`, `laundry`, `pos`, `manual`, `cancellation_fee`, `no_show_fee` and
  `adjustment`. **Categories** are exactly `room`, `meal_plan`, `laundry`, `pos:<OUTLET>`, `misc` and `fee`.
- **Folio kinds** are `guest`, `account` and `deposit`. **Folio statuses** are `open` and `invoiced`.
- **Payment methods** are `cash`, `card_terminal`, `bank_transfer` and `account`, plus `transfer` (internal only, never chosen by staff: a credit moving between two folios).
- **Posting keys:**
  - `room:<reservation_room>:<YYYY-MM-DD>`
  - `meal_plan:<reservation_room>:<YYYY-MM-DD>`
  - `pos:<pos_bill>:<n>`
  - `cancellation:<reservation_room>`
- **Tax:**
  - Rates are in basis points (1000 = 10%).
  - Rounding is half-up per component in minor units, and the remainder goes to net.
  - **Golden figures (order A):** net 1,000,000 cents gives SC 100,000, TDL 10,000, SSCL 27,750, VAT 204,795 and a total
    of 1,342,545. **Order B:** total 1,335,500.
- **Sri Lanka preset** (rule code, kind, rate_bp, applies_to, base, sequence):
  - `SC`, service_charge, 1000, `["pos:*"]`, `["net"]`, 10
  - `TDL`, levy, 100 (50 when `tdl_small_turnover`), `["*"]`, `["net"]`, 20
  - `SSCL`, levy, 250, `["*"]`, `["net","SC","TDL"]`, 30
  - `VAT`, tax, 1800, `["*"]`, `["net","SC","TDL","SSCL"]`, 40, condition `vat_registered`
  - Everything `on_bill`, effective from `2026-01-01`.
- **Invoice serial** is `<PROPERTY CODE>-<YYYY>-<NNNNNN>`, and credit notes use `CN-<PROPERTY CODE>-<YYYY>-<NNNNNN>`.
  Both come from `property_counter` (names `invoice` and `credit_note`) in the issuing transaction. Invoice values are
  whole LKR, half-up per line from base minor units. The total is in English words.
- **Accepted currencies:** `property.accepted_currencies` defaults to `{LKR,USD,EUR,GBP}`. Charges and payments outside
  it are `422`.
- **Permissions** (exact):

  | Permission | Roles |
  |---|---|
  | `FolioView` | owner, manager, front_desk, accountant |
  | `FolioPost` | owner, manager, front_desk |
  | `FolioAdjust` | owner, manager |
  | `InvoiceIssue` | owner, manager, front_desk, accountant |
  | `TaxSettingsManage` | owner |
  | `FxOverride` | owner, manager |
  | `PosPost` | owner, manager |

- **Events:** `folio:<folio>` and `fx:<property>`.
- **Topics:**
  - `invoice-render`: core-api → media-svc `/render`;
  - results come back on `media-results` as the new `MediaResult::InvoiceRendered`.
- **Lock order** (extends api-conventions): `reservation_room` → `room` → `room_condition` → `room_block` →
  `inventory_day` → `folio` → `property_counter`, with `reservation` last.
  - Folio commands lock their folio rows (`for update`) in ascending id order.
- **Every `security definer` function** sets `search_path = pg_catalog, public, pg_temp` (the schema test enforces it).
- **Performance gates:**
  - posting a charge with taxes: p95 < 30 ms;
  - `post_business_date_charges` for 200 rooms at about 80%: < 3 s;
  - `folios` with 200 lines: p95 < 30 ms;
  - invoice PDF ready: < 2 s.

  Never raise a threshold.

## Review Focus

- **Append-only money.** No code path updates or deletes `folio_line`, `folio_line_tax` or `payment`. The app role lacks
  the grants. Test: Task 2.
- **Gapless serials.** A failed or rolled-back issue consumes no number, and concurrent issues never collide or skip.
  Test: Task 8.
- **Idempotent posting.** Every system posting goes through a posting key. Replays add nothing. Test: Tasks 4, 6 and 7.
- **Exact tax arithmetic.** Components always sum to the total, inclusive back-calculation is exact, and the golden
  figures hold. Test: Task 1.
- **Cross-tenant and cross-property money.** A transfer or charge-to-account can never reach another tenant's or
  property's folio. Test: Task 5.

---

## File structure

```
modules/tax/                      rules, calculate, back_calculate, preset (Task 1)
migrations/0020_folio.sql         every table, grants, preset backfill (Task 2)
modules/identity/src/rbac.rs      7 permissions (Task 2)
modules/fx/                       rates, override, CBSL source, parser fixture (Task 3)
modules/folio/                    folio, lines, tax posting, reversal, transfer, routing (Task 4); payments,
                                  deposits (Task 5); business-date posting (Task 6); outlets, POS (Task 7);
                                  invoices, credit notes, words (Task 8)
modules/reservations/src/{stay,cancellation,reservations}.rs   folio hooks (Tasks 4-6)
crates/media-types, crates/media-svc (render.rs, templates/invoice.typ, fonts)   (Task 9)
crates/core-api routes/{folio,settings_tax,fx,pos}.rs, graphql.rs, tasks.rs, internal.rs   (Task 10)
crates/core-api/tests/{folio,perf}.rs                       (Tasks 10, 11)
web/pms src/lib/folio.ts, components/{FolioTab,FolioView,PostCharge,TakePayment}.svelte, ReservationModal  (Task 12)
web/pms routes (app)/p/[property]/settings/{taxes,outlets,fx}, accounts, reservations button  (Task 13)
web/pms tests/e2e/folio.spec.ts, folio-perf (Task 14); README, ROADMAP, ARCHITECTURE, api-conventions (Task 15)
```

---

### Task 1: The tax engine

**Files:**
- Create `modules/tax/{Cargo.toml,src/lib.rs,src/preset.rs}` and add it to the workspace.
- Test: `modules/tax/tests/golden.rs`.

**Interfaces (exact):**
```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)] #[serde(rename_all = "snake_case")]
pub enum Kind { Tax, Levy, ServiceCharge }
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)] #[serde(rename_all = "snake_case")]
pub enum Presentation { OnBill, Absorbed }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)] #[serde(tag = "type", rename_all = "snake_case")]
pub enum Condition { Always, VatRegistered, Residency { resident: bool }, PaymentCurrency { currencies: Vec<String> } }
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule { pub code: String, pub name: String, pub kind: Kind, pub rate_bp: u32,
    pub applies_to: Vec<String>,   // "*", "room", "meal_plan", "laundry", "misc", "fee", "pos:*", "pos:BAR"
    pub base: Vec<String>,         // "net" and/or earlier rule codes
    pub sequence: i32, pub presentation: Presentation, pub condition: Condition,
    pub effective_from: time::Date, pub effective_to: Option<time::Date> }
pub struct Context { pub date: time::Date, pub category: String, pub vat_registered: bool,
    pub resident: Option<bool>, pub currency: String }
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Component { pub code: String, pub name: String, pub kind: Kind, pub rate_bp: u32, pub base: i64,
    pub amount: i64, pub presentation: Presentation }
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Breakdown { pub net: i64, pub components: Vec<Component>, pub total: i64 }  // total = net + Σ on_bill
/// Applicable rules (category match, condition, effective on ctx.date) in `sequence` order; each component's
/// base is the sum of the named parts; amount = round_half_up(base × rate_bp / 10_000).
pub fn calculate(net: i64, rules: &[Rule], ctx: &Context) -> Result<Breakdown, TaxError>;
/// The net whose `calculate(...).total` equals `gross` exactly (binary search over net, then the remainder is
/// moved into net so the total matches; components keep their calculated values).
pub fn back_calculate(gross: i64, rules: &[Rule], ctx: &Context) -> Result<Breakdown, TaxError>;
pub fn sri_lanka_preset(tdl_small_turnover: bool) -> Vec<Rule>;
#[derive(Debug, thiserror::Error)] pub enum TaxError { UnknownBase(String), Cycle(String), Overflow }
```

**Rules:**
- A base may name only `net` and rules with a lower `sequence`. Anything else is `UnknownBase`.
- An absorbed component is computed and returned, but it isn't added to `total`.
- Amounts may be negative (reversals). Rounding is symmetric half-up away from zero.

- [ ] **Golden tests:**
  - **Order A:** 1,000,000 with category `pos:REST` and VAT registered → SC 100,000, TDL 10,000, SSCL 27,750,
    VAT 204,795, total 1,342,545.
  - **Order B** (SSCL base `[net,SC]`, VAT base `[net,SC]`): total 1,335,500.
  - **Room category without SC:** 1,000,000 → TDL 10,000, SSCL 25,250, VAT 186,345.
  - **Not VAT-registered:** no VAT.
  - **Small TDL:** 50 bp.
  - **Effective dates:** a rule ending yesterday is ignored, and a new rate from today applies.
  - **Absorbed TDL:** excluded from the total.
  - **Inclusive:** `back_calculate(1,342,545)` gives net 1,000,000.
  - **Property test:** for many random gross values `back_calculate(g).total == g`, and for every breakdown
    `net + Σ on_bill == total`.
  - **Negative net:** mirrors positive net exactly.
- [ ] Commit with the message `feat(tax): a pure tax and service charge engine with the Sri Lanka preset`.

### Task 2: Schema, permissions and the preset backfill

**Files:**
- Create `migrations/0020_folio.sql`.
- Modify:
  - `modules/identity/src/rbac.rs`;
  - `crates/db/tests/{isolation,schema}.rs`;
  - `modules/property/src/lib.rs` (a new property gets the preset and accepted currencies).

```sql
alter table property
  add column accepted_currencies char(3)[] not null default '{LKR,USD,EUR,GBP}',
  add column vat_registered boolean not null default false,
  add column tdl_small_turnover boolean not null default false,
  add column tin text check (tin ~ '^[0-9A-Z-]{5,20}$'),
  add column legal_name text check (length(legal_name) between 1 and 200),
  add column address text check (length(address) <= 500);
alter table account add column tin text check (tin ~ '^[0-9A-Z-]{5,20}$');
alter table guest add column tin text check (tin ~ '^[0-9A-Z-]{5,20}$');
alter table rate_plan add column prices_include_tax boolean not null default false;
alter table property_counter drop constraint property_counter_name_check;
alter table property_counter add constraint property_counter_name_check check (name in ('confirmation', 'invoice', 'credit_note'));

create table tax_rule (
  id uuid primary key, tenant_id uuid not null, property_id uuid not null,
  code text not null check (code ~ '^[A-Z][A-Z0-9_]{0,15}$'), name text not null check (length(name) between 1 and 100),
  kind text not null check (kind in ('tax', 'levy', 'service_charge')),
  rate_bp integer not null check (rate_bp between 0 and 10000),
  applies_to text[] not null, base text[] not null, sequence integer not null,
  presentation text not null check (presentation in ('on_bill', 'absorbed')),
  condition jsonb not null default '{"type":"always"}',
  effective_from date not null, effective_to date, version integer not null default 1,
  check (effective_to is null or effective_to > effective_from),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade
);
create table outlet (
  id uuid primary key, tenant_id uuid not null, property_id uuid not null,
  code text not null check (code ~ '^[A-Z0-9]{1,10}$'), name text not null check (length(name) between 1 and 100),
  service_charge boolean not null default true, active boolean not null default true, version integer not null default 1,
  unique (property_id, code), foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade
);
create table fx_rate (
  tenant_id uuid not null, property_id uuid not null, date date not null, currency char(3) not null,
  rate_to_base numeric(18,6) not null check (rate_to_base > 0), source text not null check (source in ('cbsl', 'manual')),
  source_date date not null, set_by uuid, set_at timestamptz not null default now(),
  primary key (property_id, date, currency),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade
);
create table fx_alert (property_id uuid primary key, tenant_id uuid not null, message text not null, raised_at timestamptz not null default now());
create table folio (
  id uuid primary key, tenant_id uuid not null, property_id uuid not null,
  kind text not null check (kind in ('guest', 'account', 'deposit')),
  reservation_room_id uuid, account_id uuid, reservation_id uuid,
  status text not null default 'open' check (status in ('open', 'invoiced')),
  created_at timestamptz not null default now(), version integer not null default 1,
  check ((kind = 'guest') = (reservation_room_id is not null)),
  check ((kind = 'account') = (account_id is not null)),
  check ((kind = 'deposit') = (reservation_id is not null)),
  unique (tenant_id, id),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade
);
create unique index folio_guest_idx on folio (reservation_room_id) where kind = 'guest';
create unique index folio_account_idx on folio (property_id, account_id) where kind = 'account';
create unique index folio_deposit_idx on folio (reservation_id) where kind = 'deposit';
create table folio_line (
  id uuid primary key, tenant_id uuid not null, folio_id uuid not null,
  source text not null check (source in ('room','meal_plan','laundry','pos','manual','cancellation_fee','no_show_fee','adjustment')),
  category text not null check (category ~ '^(room|meal_plan|laundry|misc|fee|pos:[A-Z0-9]{1,10})$'),
  description text not null check (length(description) between 1 and 200),
  quantity integer not null default 1 check (quantity <> 0), net bigint not null, total bigint not null,
  currency char(3) not null, base_total bigint not null, rate numeric(18,6) not null, rate_date date not null,
  business_date date not null, posting_key text, reverses uuid, reason text check (length(reason) <= 500),
  transfer_id uuid, posted_by uuid, posted_at timestamptz not null default now(),
  foreign key (tenant_id, folio_id) references folio (tenant_id, id),
  unique (folio_id, posting_key)
);
create unique index folio_line_reversed_once_idx on folio_line (reverses) where reverses is not null;
create table folio_line_tax (
  tenant_id uuid not null, line_id uuid not null references folio_line(id), code text not null, name text not null,
  kind text not null, rate_bp integer not null, base bigint not null, amount bigint not null,
  presentation text not null, primary key (line_id, code)
);
create table payment (
  id uuid primary key, tenant_id uuid not null, folio_id uuid,
  method text not null check (method in ('cash','card_terminal','bank_transfer','account')),
  amount bigint not null check (amount <> 0), currency char(3) not null, base_amount bigint not null,
  rate numeric(18,6) not null, rate_date date not null, details jsonb not null default '{}', evidence text,
  refund_of uuid references payment(id), business_date date not null, transfer_id uuid,
  taken_by uuid, taken_at timestamptz not null default now(), pos_bill_id uuid,
  foreign key (tenant_id, folio_id) references folio (tenant_id, id)
);
create table routing_rule (
  tenant_id uuid not null, reservation_id uuid not null, category text not null, account_id uuid not null,
  primary key (reservation_id, category)
);
create table pos_bill (
  id uuid primary key, tenant_id uuid not null, property_id uuid not null, outlet_id uuid not null references outlet(id),
  bill_no text not null check (length(bill_no) between 1 and 50), business_date date not null,
  body_hash bytea not null, reservation_room_id uuid, settled boolean not null, posted_at timestamptz not null default now(),
  unique (property_id, outlet_id, bill_no)
);
create table invoice (
  id uuid primary key, tenant_id uuid not null, property_id uuid not null, folio_id uuid not null,
  kind text not null check (kind in ('invoice', 'credit_note')), serial text not null, credits uuid references invoice(id),
  reason text, data jsonb not null, total_lkr bigint not null, pdf_key text, issued_by uuid,
  issued_at timestamptz not null default now(), unique (property_id, serial)
);
create unique index invoice_credited_once_idx on invoice (credits) where credits is not null;
```

Apply forced RLS and `tenant_isolation` to all twelve new tables, as 0013 does.
- **Revoke `update, delete`** on `folio_line`, `folio_line_tax` and `payment` from `goodfolk_app`, after the
  default-privileges grant (see 0009 for the revoke pattern). `invoice` allows an `update` of `pdf_key` only: revoke
  `update`, then `grant update (pdf_key) on invoice to goodfolk_app`.
- **Backfill** the Sri Lanka preset `tax_rule` rows for every existing property, lifting FORCE briefly as 0009 does, and
  add `property_counter` rows lazily (the counter code already handles a missing row; check it).
- Verify every column and constraint name against the earlier migrations: `rate_plan`, the `property_counter` check
  name and `guest`.

- [ ] **Tests:**
  - **rbac:** one row per permission.
  - **isolation:** all twelve tables.
  - **schema:** `goodfolk_app` cannot update or delete a `folio_line`, `folio_line_tax` or `payment`, and can only
    update `invoice.pdf_key`.
  - **Preset:** existing properties got the preset, and `create_property` adds it.
  - **Migration:** migrate the dev DB.
- [ ] Commit with the message `feat(folio): schema for folios, taxes, payments, POS bills, rates and invoices`.

### Task 3: Exchange rates

**Files:**
- Create `modules/fx/{Cargo.toml,src/{lib,cbsl}.rs,tests/fixtures/cbsl_usd.html}`.
- Test: `modules/fx/tests/{rates,cbsl}.rs`.

**Interfaces (exact):**
```rust
pub struct Rate { pub rate: rust_decimal::Decimal, pub date: time::Date, pub source_date: time::Date }
/// LKR (base) to base: 1. Otherwise the latest fx_rate with date ≤ on. None → caller returns 409.
pub async fn rate_for(tx: &mut Tx, property: Uuid, currency: &str, on: Date) -> Result<Option<Rate>, FxError>;
pub fn to_base(amount: i64, currency_exponent: u32, base_exponent: u32, rate: Decimal) -> i64; // half-up
pub fn exponent(currency: &str) -> u32; // ISO-4217 minor units (JPY 0, KWD/BHD/OMR 3, else 2)
pub async fn set_manual(tx: &mut Tx, tenant: TenantId, user: UserId, property: Uuid, date: Date, currency: &str, rate: Decimal) -> Result<(), FxError>; // audited fx.overridden
#[async_trait] pub trait RateSource: Send + Sync { async fn tt_selling(&self, currencies: &[String], on: Date) -> Result<Vec<(String, Date, Decimal)>, FxError>; }
pub struct Cbsl { /* reqwest client, base url overridable for tests */ }
pub fn parse_cbsl(html: &str) -> Result<Vec<(String, Date, Decimal)>, FxError>; // FxError::Layout on any surprise
pub async fn import(pool: &PgPool, source: &dyn RateSource, today: Date) -> ImportReport; // per property; sets/clears fx_alert; emits fx:<property>
```

**Rules:**
- **The request.** The Central Bank request posts the form fields probed on 2026-10-16:
  - `lookupPage=lookup_daily_exchange_rates.php`, `startRange=2006-11-11`, `rangeType=dates`;
  - `txtStart` and `txtEnd`, set to the date and the date − 7 days;
  - `chk_cur[]=USD~United States Dollar` (one per currency; map codes to the bank's labels with a table);
  - `submit_button=Submit`.

  The URL is `https://www.cbsl.gov.lk/cbsl_custom/exratestt/exrates_resultstt.php`.
- **Parsing.** Take the newest row ≤ the date for each currency, using the **Sell Rate**.
- **Storage.** The rate is stored under `date = today`, with `source_date` set to the row's own date.
- **Errors.** A missing currency or a parse error writes an `fx_alert` row and logs an error. A success clears the alert.
- **Fixture.** Save a real response to the fixture: fetch it once with curl, and keep a trimmed copy under 30 KB.

- [ ] **Tests:**
  - the parser reads the fixture;
  - a layout change (a column renamed) gives `Layout`;
  - `import` with a fake source stores rates and clears the alert;
  - a fake source that errors sets the alert and stores nothing;
  - a weekend keeps the true `source_date`;
  - `to_base` covers JPY (exponent 0) and rounding;
  - a manual override is audited;
  - `rate_for` falls back to an earlier date.
- [ ] Commit with the message `feat(fx): exchange rates from the Central Bank, with overrides and a loud failure`.

### Task 4: Folios, lines, taxes, reversals, transfers and routing

**Files:**
- Create `modules/folio/{Cargo.toml,src/{lib,folios,lines,routing}.rs}`.
- Modify `modules/reservations` so that creating a reservation creates its deposit folio and each room's guest folio,
  and adding a room creates its guest folio.
- Test: `modules/folio/tests/{lines,routing}.rs`.

**Interfaces (exact):**
```rust
db::text_enum!(Source { Room = "room", MealPlan = "meal_plan", Laundry = "laundry", Pos = "pos", Manual = "manual",
    CancellationFee = "cancellation_fee", NoShowFee = "no_show_fee", Adjustment = "adjustment" });
pub fn folio_key(folio: Uuid) -> String; // "folio:<id>"
pub async fn ensure_guest_folio(tx, tenant, property, reservation_room) -> Result<Uuid, FolioError>;
pub async fn ensure_deposit_folio(tx, tenant, property, reservation) -> Result<Uuid, FolioError>;
pub async fn ensure_account_folio(tx, tenant, property, account) -> Result<Uuid, FolioError>;
pub struct NewCharge { pub source: Source, pub category: String, pub description: String, pub quantity: i32,
    pub unit_net: i64, pub currency: String, pub business_date: Date, pub posting_key: Option<String>, pub inclusive: bool }
/// Taxes via modules/tax with the property's rules and the stay's residency, base via fx; locks the folio;
/// refuses invoiced folios (Conflict) and unaccepted currencies (Invalid); a known posting_key returns the
/// existing line (no-op). Emits folio:<id>, audits folio.charge_posted (manual only).
pub async fn post_charge(tx, tenant, user: Option<UserId>, folio: Uuid, charge: NewCharge) -> Result<LineRow, FolioError>;
/// Routes by category for a stay: the account folio when a routing rule matches, else the guest folio.
pub async fn post_for_stay(tx, tenant, user, reservation_room: Uuid, charge: NewCharge) -> Result<LineRow, FolioError>;
pub async fn reverse(tx, tenant, user, folio: Uuid, line: Uuid, reason: &str) -> Result<LineRow, FolioError>;
pub async fn transfer(tx, tenant, user, from_line: Uuid, to_folio: Uuid, reason: &str) -> Result<(LineRow, LineRow), FolioError>;
pub async fn set_routing(tx, tenant, user, reservation: Uuid, categories: &[String]) -> Result<(), FolioError>;
pub struct FolioView { pub folio: FolioRow, pub lines: Vec<LineWithTaxes>, pub payments: Vec<PaymentRow>, pub balances: Vec<Balance> }
pub struct Balance { pub currency: String, pub amount: i64, pub base_amount: i64 }  // charges − payments
pub async fn folios_for(tx, query: FolioQuery) -> Result<Vec<FolioView>, FolioError>;  // batched, no N+1
```
`FolioError` follows `HousekeepingError`'s shape.

**Rules:**
- **Tax inputs.** Residency comes from the stay's rate plan residency (Phase 2). `vat_registered` and the rules come
  from the property.
- **Line amounts.** `quantity × unit_net` is the net. For an inclusive charge, use `back_calculate(quantity × unit)`.
  `total` is the tax breakdown's total.
- **Reversals and transfers** copy the original's taxes, negated. A transfer is one transaction: lock both folios in
  ascending id order, both must be open, and both must belong to the same tenant and property. A guest folio may transfer
  to the same reservation's account or deposit folios, and to another stay's guest folio in the same property.

- [ ] **Tests:**
  - a charge on `pos:REST` stores the golden components;
  - a currency outside the accepted list gets `Invalid`;
  - a missing rate gets `Conflict`, with the 409 text from the spec;
  - a reversal nets to zero, including taxes;
  - reversing twice gets `Conflict`;
  - reversing a reversal gets `Invalid`;
  - a transfer moves the exact amount and links both lines;
  - a transfer to another property's folio gets `NotFound`;
  - routing sends `room` to the account folio and `pos:BAR` to the guest folio;
  - a posting key replay is a no-op;
  - an invoiced folio refuses charges;
  - creating a reservation creates its folios;
  - `folios_for` makes one round trip per kind (assert the query count, or time it).
- [ ] Commit with the message `feat(folio): folios with taxed, append-only lines, reversals, transfers and routing`.

### Task 5: Payments, refunds and deposits

**Files:**
- Create `modules/folio/src/payments.rs`.
- Modify `modules/reservations/src/{stay,cancellation}.rs`: check-in moves the deposit, and cancelling the whole
  reservation posts the penalty.
- Test: `modules/folio/tests/payments.rs` and the reservations tests.

**Interfaces (exact):**
```rust
pub enum Method { Cash, CardTerminal { approval: String, last4: String, brand: String }, BankTransfer { reference: String }, Account }
pub async fn take_payment(tx, tenant, user, folio: Uuid, method: Method, amount: i64, currency: &str, evidence: Option<&str>) -> Result<PaymentRow, FolioError>;
pub async fn refund(tx, tenant, user, payment: Uuid, amount: i64, reason: &str) -> Result<PaymentRow, FolioError>;
pub async fn move_deposit(tx, tenant, user, reservation: Uuid, to_reservation_room: Uuid) -> Result<(), FolioError>; // no-op on zero balance
pub async fn post_cancellation_fee(tx, tenant, user, property: Uuid, reservation_room: Uuid, amount: i64, currency: &str, inclusive: bool) -> Result<(), FolioError>; // posting key cancellation:<reservation_room>
```

**Rules:**
- **Charge to account.** `Method::Account` needs the reservation to have an account. It posts a `payment` on this
  folio, plus an `adjustment` charge on the account folio with the same amount and currency, linked by `transfer_id`.
- **Refunds.** The refunded total must stay ≤ the original amount. Refunds are refused on an `account` payment; reverse
  the account line instead.
- **Card details.** `last4` is exactly 4 digits. The details never hold a full card number: refuse any run of 12 or
  more digits in any details or evidence field.

- [ ] **Tests:**
  - a payment in USD stores its base amount;
  - an over-refund gets `Invalid`;
  - two partial refunds both succeed;
  - a charge to account moves the amount, and a reservation without an account gets `Invalid`;
  - a deposit moves at the first check-in only;
  - a cancellation posts the penalty to the deposit folio once, even when replayed;
  - a 16-digit number in the details is refused.
- [ ] Commit with the message `feat(folio): desk payments, refunds, charge to account and deposits`.

### Task 6: Posting the business date's charges

**Files:**
- Create `modules/folio/src/posting.rs`.
- Modify `modules/reservations/src/stay.rs`, so check-out posts the unposted nights first.
- Test: `modules/folio/tests/posting.rs`.

**Interfaces (exact):**
```rust
pub struct PostReport { pub stays: u32, pub lines: u32, pub skipped_existing: u32 }
/// Every reservation_room checked in, not checked out, arrival ≤ date < departure: room charge (room_amount)
/// and meal plan (meal_amount, if > 0) from reservation_night for `date`, routed, with posting keys. Processes
/// stays in id order, one transaction per chunk of 50 (so a 200-room run stays well under the gate and a failure
/// leaves completed chunks posted; reruns skip them).
pub async fn post_business_date_charges(pool: &PgPool, tenant: TenantId, user: Option<UserId>, property: Uuid, date: Date) -> Result<PostReport, FolioError>;
pub async fn post_unposted_nights(tx, tenant, user, reservation_room: Uuid, before: Date) -> Result<u32, FolioError>;
```
`inclusive` comes from the rate plan's `prices_include_tax`. The description is "Room <number> — <date>" or "Meal
plan BB — <date>".

- [ ] **Tests:**
  - running twice adds nothing;
  - only in-house stays are posted;
  - a stay checked out today isn't posted for today;
  - routing applies;
  - check-out posts the missing nights exactly once;
  - an inclusive plan's line totals equal the night price;
  - a run killed between chunks resumes cleanly.
- [ ] Commit with the message `feat(folio): post a business date's room and meal-plan charges, once`.

### Task 7: Outlets and POS bills

**Files:**
- Create `modules/folio/src/pos.rs`.
- Test: `modules/folio/tests/pos.rs`.

**Interfaces (exact):**
```rust
pub struct PosItem { pub description: String, pub quantity: i32, pub unit_net: i64, pub category: Option<String> }
pub enum PosTarget { Stay(Uuid), Settled { method: Method } }
pub struct PosBillIn { pub outlet: String, pub bill_no: String, pub business_date: Option<Date>, pub currency: String, pub items: Vec<PosItem>, pub target: PosTarget }
pub enum PosOutcome { Posted(PosBillRow), Replayed(PosBillRow) }
pub async fn post_pos_bill(tx, tenant, user, property: Uuid, bill: PosBillIn) -> Result<PosOutcome, FolioError>;
pub async fn create_outlet / update_outlet / list_outlets (CRUD, If-Match on update)
```

**Rules:**
- **Body hash.** `body_hash` is the SHA-256 of the canonical JSON of the bill. A replay with the same hash returns
  `Replayed`; a different hash gets `Conflict`.
- **Taxes.** The category is `pos:<OUTLET>` unless an item names `misc`. Service charge applies only when
  `outlet.service_charge` is on, which requires an `SC` rule applying to `pos:*` or that outlet.
- **Settled bills.** A settled bill creates a `payment` with `folio_id` null and `pos_bill_id` set, plus its taxed line
  values in the bill row. `pos_bill.lines jsonb` already exists (migration 0021, from Task 2's review).
- **Validation.** 1–200 items. A stay must be in-house at the property.

- [ ] **Tests:**
  - a bill to a room posts its items taxed with SC;
  - an outlet with service charge off gets no SC;
  - a replay returns `Replayed`, and a changed replay gets `Conflict`;
  - a settled bill records a payment with no folio;
  - a stay not in house gets `Invalid`.
- [ ] Commit with the message `feat(folio): outlets and idempotent POS bills`.

### Task 8: Invoices and credit notes

**Files:**
- Create `modules/folio/src/{invoices,words}.rs`.
- Test: `modules/folio/tests/invoices.rs`.

**Interfaces (exact):**
```rust
pub fn lkr_in_words(rupees: i64) -> String; // "Thirteen Thousand Four Hundred Twenty-Five Rupees Only"
#[derive(Serialize, Deserialize)] pub struct InvoiceData { /* supplier{legal_name,address,tin}, purchaser{name,tin}, serial, title ("TAX INVOICE"|"INVOICE"), issued_on, supply_from, supply_to, lines[{description, quantity, currency, total, total_lkr}], taxes[{name, rate_bp, amount_lkr}], net_lkr, total_lkr, total_words, payments[{method, amount, currency}], vat_registered */ }
pub async fn issue_invoice(tx, tenant, user, folio: Uuid) -> Result<InvoiceRow, FolioError>;   // balance zero per currency, ≥1 line; serial; data; folio invoiced; audit; outbox `invoice-render`
pub async fn credit_note(tx, tenant, user, invoice: Uuid, reason: &str) -> Result<InvoiceRow, FolioError>; // CN serial; reopens folio
pub async fn invoice_link_key(tx, tenant, user, property, invoice: Uuid) -> Result<String, FolioError>; // audited invoice.viewed; Conflict when pdf_key is null
pub async fn set_invoice_pdf(tx, invoice: Uuid, key: &str) -> Result<(), FolioError>; // idempotent; only when null
```

**Rules:**
- **Whole LKR.** Each line's `total_lkr` is `round_half_up(base_total / 100)` in rupees. Taxes are summed per code
  from `folio_line_tax`, converted per line the same way.
- **Rounding remainder.** `total_lkr` is the sum of the line totals. Put any rounding difference against the tax sum
  into `net_lkr`, so the printed figures add up.
- **Serials** come from `property_counter` inside the transaction, so a rollback gives the number back.
- **Outbox payload:** `{"tenant_id", "invoice_id", "data": InvoiceData}`.

- [ ] **Tests:**
  - the words for 0, 1, 13,425, 100,000 and 10,000,000;
  - an unbalanced folio gets `Conflict`;
  - issuing freezes the folio, a charge is then refused, a credit note reopens it, and a re-issue takes the next serial;
  - 20 concurrent issues on different folios give 20 consecutive serials with no gaps;
  - a rolled-back issue gives its number back;
  - a USD line converts to whole LKR;
  - a property that isn't VAT-registered is titled "INVOICE";
  - the link writes an audit row.
- [ ] Commit with the message `feat(folio): IRD-format invoices and credit notes with gapless serials`.

### Task 9: Invoice PDFs in media-svc

**Files:**
- Modify:
  - `crates/media-types` (`InvoiceRenderJob`, `MediaResult::InvoiceRendered { tenant_id, invoice_id, key }`);
  - `crates/media-svc/src/{lib,render}.rs`.
- Create `crates/media-svc/templates/invoice.typ` and the fonts in `crates/media-svc/fonts/`.
- Test: `crates/media-svc/tests/render.rs`.

**Rules:**
- **The endpoint.** `POST /render` takes `invoice-render` pushes. It sits behind `svc_auth`, beside `/work`.
- **Rendering.**
  - Render with `typst` and `typst-pdf` in a `spawn_blocking` call: no subprocess, because Typst is pure Rust and
    memory-safe on our own template.
  - The template reads the JSON from a virtual file through `sys.inputs`, or through `json(bytes)` from a
    `World` that provides it.
  - Fonts are bundled. Use `typst-assets` fonts, or Noto Sans (OFL) committed under `fonts/`. Record the font licence
    in `fonts/LICENSE`.
- **Storage.** Write the PDF to `t/{tenant}/invoices/{invoice}.pdf` (private, `application/pdf`), then publish
  `InvoiceRendered`. It is idempotent: same key, same result.
- **Licences.** Check that the `typst` crates' dependency tree passes `deny.toml`, and list any new licences.

- [ ] **Tests:**
  - rendering the golden `InvoiceData` gives a PDF that starts with `%PDF` and contains the serial and the title (use
    `typst-pdf`'s output, plus a text extraction or a raw byte search for an uncompressed tag; if text is compressed,
    assert through Typst's document model before export);
  - render time is under 1 s in release, printed;
  - `/render` without auth gets 401;
  - a repeated push publishes the same key.
- [ ] Commit with the message `feat(media-svc): render invoices to PDF with Typst`.

### Task 10: core-api wiring

**Files:**
- Modify:
  - `crates/core-api` config, state, routes (new `folio.rs`, `taxes.rs`, `fx.rs`, `pos.rs`), `graphql.rs`,
    `tasks.rs` (the daily FX import at 10:30 Asia/Colombo and at start-up, with an advisory lock), `internal.rs`
    (`InvoiceRendered` → `set_invoice_pdf`) and `openapi.rs`;
  - the exported schemas;
  - `events` topics: core-api publishes `invoice-render`, and its `bus_from_env` gains it;
  - `media-svc`'s push endpoint env (`PUSH_ENDPOINT_INVOICE_RENDER`);
  - the Playwright config env.
- Test: `crates/core-api/tests/folio.rs`.

**Rules:**
- **REST:** the spec's API section, with idempotent creates on the `commands` router, the permission table above, and
  a 409 for a missing rate.
- **GraphQL:** `folios`, `taxRules`, `outlets`, `fxRates` and `fxAlert`, batched.
- **SSE:** `folio:<id>` per folio, and `fx:<property>`.

- [ ] **Tests:**
  - a full flow over HTTP: reservation, check-in, post tonight's charges, a manual charge, a USD payment, then an
    invoice, with the PDF key arriving through the in-process media-svc;
  - permissions per role (a housekeeper gets 403 on every folio route);
  - cross-tenant ids get 404;
  - a POS bill replay over HTTP;
  - a rate override is audited.
- [ ] Commit with the message `feat(api): folios, taxes, POS bills, exchange rates and invoices`.

### Task 11: Server performance gates

Add to `crates/core-api/tests/perf.rs` the spec's four gates:
- posting a charge, p95 over 200 samples;
- `post_business_date_charges` for 200 rooms at 80%, median of 3;
- `folios` with 200 lines, p95;
- invoice issue to `pdf_key`, median of 5.

Profile and fix any that fail; never raise one.
- [ ] Commit with the message `test(folio): posting, business date, folio read and invoice speed gates`.

### Task 12: Folios in the reservation modal

**Files:**
- Create `web/pms/src/lib/folio.ts` (+ spec) and `components/{FolioTab,FolioView,PostCharge,TakePayment,RefundDialog,
  TransferDialog,RoutingDialog}.svelte`.
- Modify `ReservationModal.svelte`.

**Rules:**
- **Tabs:** one tab per folio. Lines show expandable taxes. Balances are shown per currency, with LKR.
- **Actions** are gated by permission. Money is formatted per currency's exponent.
- **Invoice:** **Issue invoice** is enabled only at a zero balance. **View invoice** fetches a fresh link each time and
  opens the PDF in a new tab.
- **Live updates:** `folio:<id>`.
- **Unit tests:** money formatting, balance logic, and action enablement.

- [ ] Commit with the message `feat(web): folios, charges, payments and invoices in the reservation`.

### Task 13: Settings screens, account folios, and posting tonight's charges

**Files:**
- Create routes `(app)/p/[property]/settings/{taxes,outlets,exchange-rates}`.
- Modify the account folio view (accounts page) and the **Post tonight's charges** button (reservations page, gated by
  `FolioAdjust`, confirming with counts).
- Add the FX alert banner in the app layout for owners and managers.

- [ ] Commit with the message `feat(web): tax, outlet and exchange-rate settings, account folios and posting tonight's charges`.

### Task 14: End-to-end tests

Create `tests/e2e/folio.spec.ts`, covering the spec's Playwright list:
- a charge plus a payment;
- an invoice issued and viewed;
- a POS bill to a room through the API;
- a deposit that moves at check-in;
- a housekeeper with no folio access.

Add a `@perf`-free run, then the full e2e suite.
- [ ] Commit with the message `test(web): folio end-to-end tests`.

### Task 15: Documentation

Update:
- the README: configuration, "Trying folios by hand" (in the style of the media script) and the FX task;
- the ROADMAP: 7a done, with 7b and 7c listed;
- ARCHITECTURE: §7.5–7.6 as built and the decisions;
- api-conventions: permissions, events and the lock order.

- [ ] Commit with the message `docs: folios, taxes, invoices and the manual folio walkthrough`.
