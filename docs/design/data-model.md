# Data Model

The reference schema for every phase. Phase 0 tables exist in `migrations/0001_foundation.sql`, Phase 1 tables in `migrations/0003_login_throttle.sql` and `migrations/0004_rooms_inventory.sql`, Phase 2 tables in `migrations/0006_rates.sql`, Phase 3 tables in `migrations/0007_reservations.sql` (3a) and `migrations/0008_reservations_3b.sql` (3b). Tables for later phases are the target design: the migrations that create them are written in their phase and may refine columns, but must keep the rules below.

Related: [ARCHITECTURE.md](../ARCHITECTURE.md) (why), [api-conventions.md](api-conventions.md) (how data leaves the API).

## Rules for every table

1. **Tenant-owned rows carry `tenant_id uuid not null`, with forced row-level security** and a case in `crates/db/tests/isolation.rs`. The schema guard test fails otherwise. Tables that are not tenant-owned (for example `session`, `fx_rate`) must not name any column `tenant_id`.
2. **Property-scoped rows also carry `property_id`**, and their composite indexes lead with `property_id` (or `tenant_id` for tenant-wide tables).
3. **IDs are UUIDv7** generated in Rust. Human-facing numbers (confirmation, invoice) are separate columns, unique per property.
4. **Money is `bigint` minor units plus a `char(3)` ISO-4217 currency** on the same row. Never floats. Exchange rates are `numeric(18,8)`.
5. **Stay and business dates are `date` in the property's time zone. Ranges are `daterange` with `[)` bounds** (check-in inclusive, check-out exclusive). Event times are `timestamptz`.
6. **Rows that users edit concurrently carry `version integer not null default 1`.** Updates use `where version = $n` and increment it; the API maps a miss to `412 Precondition Failed` (`If-Match`).
7. **Financial records are append-only.** Folio lines, payments, invoices, audit runs and `audit_log` are never updated or deleted by the app role; corrections are reversal rows.
8. **Enumerations are `text` with a `check` constraint** (easy to extend in a migration), mirrored by a Rust enum with `as_str`/`parse`.

## Phase 0: Foundation (implemented)

| Table | Key columns | Notes |
|---|---|---|
| `tenant` | `id`, `name` | Read policy also exposes tenants the current user belongs to (via `membership`) |
| `app_user` | `id`, `email citext unique`, `password_hash`, `display_name` | Global identity; no RLS. Bind emails as `$1::citext` |
| `membership` | `(tenant_id, user_id)` | Users see their own memberships in any tenant; only the current tenant can write |
| `role_grant` | `id`, `tenant_id`, `user_id`, `property_id null`, `role` | `null` property = tenant-wide. Roles: `owner`, `manager`, `front_desk`, `housekeeping`, `accountant` |
| `session` | `id`, `token_hash bytea unique`, `user_id`, `active_tenant_id`, `expires_at` | SHA-256 of the cookie token; no RLS (looked up before a tenant is known) |
| `property` | `id`, `tenant_id`, `code` (unique per tenant), `name`, `timezone`, `base_currency`, `version` | Later phases add settings columns (below) |
| `audit_log` | `id`, `tenant_id`, `actor_user_id`, `action`, `entity`, `entity_id`, `data jsonb`, `at` | Append-only (no update/delete grant) |
| `idempotency_key` | `(tenant_id, key)`, `request_hash`, `status_code`, `response_body` | Replay store for create commands. A cleanup job (Phase 7 `jobs-svc`) deletes keys older than 7 days |

## Phase 1: Rooms and inventory (implemented)

Created by `migrations/0004_rooms_inventory.sql`. Every table below carries `tenant_id` and references its property through `(tenant_id, property_id)`, and a room's type, section, blocks and reasons through `(property_id, id)`, so a row can never point into another tenant or property.

`property` gains: `check_in_time time not null default '14:00'`, `check_out_time time not null default '12:00'`, `business_date date not null` (set to the property's local today when created; moved only by night audit).

| Table | Key columns | Constraints and indexes |
|---|---|---|
| `room_type` | `id`, `tenant_id`, `property_id`, `code`, `name`, `base_occupancy`, `max_adults`, `max_children`, `max_occupancy`, `bed_config jsonb` (`[{kind, count}]`), `amenities text[]`, `sort_order`, `active`, `version` | `unique (property_id, code)`; `check (base_occupancy <= max_occupancy)`, `max_adults <= max_occupancy <= max_adults + max_children` |
| `housekeeping_section` | `id`, `tenant_id`, `property_id`, `name`, `version` | `unique (property_id, name)` |
| `room` | `id`, `tenant_id`, `property_id`, `room_type_id`, `number text`, `floor text null`, `section_id null`, `active`, `sort_order`, `version` | `unique (property_id, number)` |
| `block_reason` | `id`, `tenant_id`, `property_id`, `code`, `label`, `default_kind`, `active`, `version` | `unique (property_id, code)`. Seeded per property: `RENOVATION`, `CONSTRUCTION`, `MAINTENANCE`, `OTHER` (out of order), `DEEP_CLEAN` (out of service) |
| `room_block` | `id`, `tenant_id`, `property_id`, `room_id`, `period daterange`, `kind` (`out_of_order` \| `out_of_service`), `reason_id`, `note`, `created_by`, `released_at null`, `version` | `room_block_no_overlap: exclude using gist (room_id with =, period with &&) where (released_at is null)`; GiST `(property_id, period)`. `released_at` marks a block cancelled before it started; shortening moves `upper(period)` |
| `inventory_day` | `(property_id, room_type_id, date)`, `tenant_id`, `physical`, `sold`, `out_of_order` | `check (out_of_order between 0 and physical)`; index `(property_id, date)`. Counters updated in the same transaction as rooms, blocks and (from Phase 3) reservations. `available = physical − sold − out_of_order`. Rows exist from the business date for 730 days. A nightly job (Phase 7) recomputes them and alerts on drift (`rooms::find_drift`) |

Extensions: `btree_gist` (needed by the exclusion constraints) and, from Phase 3, `pg_trgm` (guest name search).

| Global table | Key columns | Notes |
|---|---|---|
| `login_failure` | `id`, `email citext`, `at` | Sign-in throttling (`migrations/0003_login_throttle.sql`). No RLS and no `tenant_id`: looked up before a tenant is known. One row per attempt, written before the password is checked; a successful sign-in deletes the email's rows. The Phase 7 purge job deletes rows older than the window |

## Phase 2: Rates and meal plans (implemented)

Created by `migrations/0006_rates.sql` (`0005_idempotency_etag.sql` adds `idempotency_key.etag`, so a replayed create carries its `ETag`). Every table carries `tenant_id` and `property_id` and references its property through `(tenant_id, property_id)`; prices and restrictions reference the plan's room type through `(property_id, rate_plan_id, room_type_id)`. Amounts are minor units in the plan's (or supplement's) currency, at most 100 000 000 000.

| Table | Key columns | Constraints and indexes |
|---|---|---|
| `rate_plan` | `id`, `tenant_id`, `property_id`, `code`, `name`, `kind` (`standard` \| `derived` \| `custom`), `segment` (`FIT_F` \| `FIT_L` \| `OTA` \| `TA` \| `IBE`), `residency` (`resident` \| `non_resident` \| null = any), `currency`, `parent_id null`, `derive_mode` (`percent` \| `amount`), `derive_value bigint` (basis points or minor units), `rounding_step bigint`, `extra_adult_amount bigint`, `inherit_restrictions bool`, `allowed_meal_plans text[]`, `cancellation_policy_id null`, `active`, `version` | `unique (property_id, code)`; `rate_plan_derivation_check`: a plan has a parent and a formula exactly when it is derived, percent within −100 % … +1000 %; `rate_plan_segment_residency_check`: `FIT_F` is `non_resident`, `FIT_L` is `resident`. Same currency as the parent, derivation depth ≤ 3, no cycles, and only standard or derived plans as parents: enforced in the `rates` module, tested |
| `rate_plan_room_type` | `(rate_plan_id, room_type_id)`, `tenant_id`, `property_id` | Which room types a plan sells. A derived plan sells a subset of its parent's. Removing a type deletes the plan's prices and restrictions for it (`on delete cascade`) |
| `rate_day` | `(rate_plan_id, date, room_type_id, occupancy)`, `tenant_id`, `property_id`, `amount bigint` | **Resolved** prices, derived plans included; `occupancy` is the number of adults (1 … 50). Recomputed in the same transaction as the parent change (one `insert … select … on conflict do update` per level, through `app.derive_amount`). Index `(property_id, date)` for grids and search |
| `rate_restriction` | `(rate_plan_id, date, room_type_id)`, `tenant_id`, `property_id`, `closed bool`, `min_stay null`, `max_stay null`, `closed_to_arrival`, `closed_to_departure` | Restrictions per plan, room type and date (the target design kept them on `rate_day`; they apply to every occupancy, so they have their own row). **Resolved**: a derived plan with `inherit_restrictions` holds a copy of its parent's rows, rewritten with them. Index `(property_id, date)` |
| `meal_supplement` | `id`, `tenant_id`, `property_id`, `meal_plan` (`BB` \| `HB` \| `FB`), `currency`, `adult_amount`, `child_amount`, `valid daterange`, `version` | Per person per night on top of the room price. `RO` is always 0 and has no rows. `valid` has a lower bound; no upper bound means until further notice. Exclusion `meal_supplement_no_overlap`: no overlapping `valid` for the same `(property_id, meal_plan, currency)` |
| `cancellation_policy` | `id`, `tenant_id`, `property_id`, `name`, `rules jsonb`, `no_show jsonb`, `version` | `unique (property_id, name)`. `rules`: list of `{days_before_arrival, penalty: {kind: nights\|percent\|amount, value}}` (percent in basis points); `no_show`: one penalty |

`app.derive_amount(base, mode, value, step)` changes a price by `value` basis points (`percent`) or minor units (`amount`), never below 0, rounded half-up to a multiple of `step`, in integer arithmetic. Derived plans apply their formula to the parent's price with it, and bulk changes apply theirs to a plan's own prices.

## Phase 3: Reservations and guests

Phase 3 ships in two slices. **3a** (`migrations/0007_reservations.sql`) creates `guest`, `property_counter`, `reservation`, `reservation_room` and `reservation_night` as below. **3b** (`migrations/0008_reservations_3b.sql`) adds `account` and `reservation_guest`, `reservation.account_id`, `room_type.overbooking` and `reservation_room`'s check-in/out columns. Channel columns (`channel_code`, `channel_ref`) still arrive later, with channels (Phase 8). Property-scoped tables reference their property through `(tenant_id, property_id)`, and rooms, room types, rate plans and reservations through `(property_id, id)`; guests and accounts are referenced through `(tenant_id, id)`. Amounts are minor units in the row's `currency`.

`room_type` gains (3b): `overbooking integer not null default 0` (`room_type_overbooking_check`: 0–20). How many more rooms of the type may be sold than are physically available: a night is sellable when `physical - sold - out_of_order + overbooking > 0`.

| Table | Key columns | Constraints and indexes |
|---|---|---|
| `guest` | `id`, `tenant_id`, `first_name` (empty for a single-name guest), `last_name`, `email text null` (stored lowercased), `phone null`, `country char(2) null`, `residency` (`resident` \| `non_resident`, required), `id_doc_type null` (`passport` \| `nic` \| `driving_licence` \| `other`), `id_doc_number_enc bytea null` (AES-256-GCM: nonce ‖ ciphertext ‖ tag, AAD = tenant id ‖ guest id), `id_doc_key_id null` (the key that sealed it, for rotation), `id_doc_last4 null` (plaintext tail of at most 4 characters, never more than half the number, shown only masked), `notes`, `version`, `created_at` | Tenant-wide (no `property_id`), so a chain shares guest history; RLS on the tenant alone. `guest_id_doc_check`: the four `id_doc_*` columns are all set or all null. Trigram GIN index `guest_name_trgm_idx` on `lower(first_name \|\| ' ' \|\| last_name)` (`pg_trgm`; searches must use that expression); `(tenant_id, email)` and `(tenant_id, phone)` for exact matches |
| `account` | `id`, `tenant_id`, `kind` (`company` \| `travel_agent`), `name`, `contact jsonb` (default `{}`, `{email?, phone?, address?, contact_name?}`), `credit_limit bigint null` (≥ 0), `currency`, `active`, `version`, `created_at` | 3b. Companies and travel agents a reservation can be billed to (invoicing and the city ledger are Phase 7). Tenant-wide like `guest`, not scoped to a property; `unique (tenant_id, id)` lets `reservation.account_id` reference it by composite key. `account_kind_check`, `account_name_check` (1–200), `account_contact_check` (a JSON object), `account_credit_limit_check`, `account_currency_check`. Index `(tenant_id, lower(name))` for listing: `tenant_id` is leakproof and becomes an index condition under forced row-level security, but `lower(name)` is not, so a list sorted or filtered by it is a plain scan of the tenant's own accounts — fine, since accounts are few per tenant |
| `reservation` | `id`, `tenant_id`, `property_id`, `confirmation_no`, `source` (`front_desk` \| `ibe` \| `channel` \| `phone` \| `email`), `booker_guest_id`, `guarantee` (`none` \| `card` \| `deposit` \| `account`, default `none`), `hold_expires_at null`, `notes`, `account_id null` (3b; composite FK `(tenant_id, account_id)` to `account`), `created_by`, `created_at`, `version`; later `channel_code null`, `channel_ref null` | **No stored status**: it is derived from the rooms' statuses when read (`domain::reservation_status`). No `segment` either: it comes from each room's rate plan. `reservation_property_id_confirmation_no_key`: unique per property; `reservation_confirmation_prefix_idx` `(property_id, confirmation_no text_pattern_ops)` serves prefix search (`starts_with(confirmation_no, …)`). `reservation_confirmation_no_check`: `<PROPERTY CODE>-<sequence>`, zero-padded to 6 digits and growing past them (`GFK-000123`, `GFK-1000000`). Later `unique (property_id, channel_code, channel_ref)` where not null (idempotent channel ingestion) |
| `reservation_room` | `id`, `tenant_id`, `property_id`, `reservation_id`, `room_type_id`, `room_id null`, `stay daterange`, `arrival date` (generated: `lower(stay)`, stored), `adults` (≥ 1), `children` (≥ 0), `rate_plan_id`, `meal_plan` (`RO` \| `BB` \| `HB` \| `FB`), `status` (`tentative` \| `confirmed` \| `checked_in` \| `checked_out` \| `cancelled` \| `no_show`), `primary_guest_id` (its residency prices the room), `currency` (the plan's), `cancellation_terms jsonb null` (the plan's policy at booking: `{rules, no_show}`), `cancelled_at null`, `cancelled_by null`, `cancellation_penalty bigint null`, `checked_in_at timestamptz null` (3b), `checked_in_business_date date null` (3b), `checked_out_at timestamptz null` (3b), `eta time null`, `version` | **`reservation_room_no_double_booking`: `exclude using gist (room_id with =, stay with &&) where (room_id is not null and status not in ('cancelled','no_show'))`**, so double booking is impossible. `reservation_room_stay_check`: non-empty, bounded, `[)`. `reservation_room_cancellation_check`: `cancelled_at` is set exactly when `status` is `cancelled`, `cancellation_penalty` exactly when `cancelled_at` is, and `cancelled_by` only then. `reservation_room_checked_in_at_check` (3b): `checked_in_at` is set exactly when `status` is `checked_in` or `checked_out`. `reservation_room_checked_in_business_date_check` (3b): `checked_in_business_date` is set exactly when `checked_in_at` is. `reservation_room_checked_out_at_check` (3b): `checked_out_at` is set exactly when `status` is `checked_out`. GiST `(property_id, stay)` serves tape-chart tiles and date-range lists; `reservation_room_arrival_idx (property_id, arrival, id)` serves the reservations list, sorted and paged by arrival (plain date comparisons are leakproof, so under row-level security they can be index conditions, which `lower(stay)` can't). Check-out early sets `upper(stay)` to the actual date |
| `reservation_night` | `(reservation_room_id, date)`, `tenant_id`, `property_id`, `room_amount`, `meal_amount`, `currency` | Price snapshot at booking (amounts ≥ 0); later rate changes do not reprice existing bookings |
| `reservation_guest` | `(reservation_room_id, guest_id)`, `tenant_id`, `property_id` | 3b. Additional occupants of a room, beyond its `primary_guest_id`. Composite FKs `(property_id, reservation_room_id)` to `reservation_room` (`on delete cascade`) and `(tenant_id, guest_id)` to `guest` keep both ends in the room's own property and tenant. Index `(tenant_id, guest_id)` |
| `property_counter` | `(property_id, name)`, `tenant_id`, `value bigint` | Gapless numbers (`confirmation`; `invoice` is added to the `name` check in Phase 7), taken with `insert … on conflict (property_id, name) do update set value = property_counter.value + 1 returning value` inside the transaction that uses the number |

## Phase 5: Housekeeping and laundry

| Table | Key columns | Notes |
|---|---|---|
| `room_status` | `room_id` (pk), `tenant_id`, `property_id`, `condition` (`dirty` \| `cleaning` \| `clean` \| `inspected`), `updated_by`, `updated_at`, `version` | Occupancy (vacant / occupied / due out / due in) is derived from reservations, not stored |
| `hk_assignment` | `id`, `tenant_id`, `property_id`, `business_date`, `room_id`, `user_id` | `unique (room_id, business_date)` |
| `room_issue` | `id`, `tenant_id`, `property_id`, `room_id`, `category`, `severity` (`low` \| `medium` \| `high`), `description`, `status` (`open` \| `in_progress` \| `resolved`), `reported_by`, `assigned_to null`, `created_at`, `resolved_at` | Photos via `media_link` |
| `linen_item` | `id`, `tenant_id`, `property_id`, `code`, `name` | |
| `linen_par` | `(room_type_id, linen_item_id)`, `tenant_id`, `quantity` | Par level per room of that type |
| `linen_location` | `id`, `tenant_id`, `property_id`, `name`, `kind` (`store` \| `pantry` \| `laundry` \| `vendor`) | |
| `linen_stock` | `(location_id, linen_item_id)`, `tenant_id`, `quantity` | Maintained from movements |
| `linen_movement` | `id`, `tenant_id`, `property_id`, `linen_item_id`, `from_location_id null`, `to_location_id null`, `quantity`, `kind` (`transfer` \| `send` \| `receive` \| `write_off`), `batch_id null`, `note`, `by`, `at` | Append-only |
| `laundry_batch` | `id`, `tenant_id`, `property_id`, `vendor_location_id`, `sent_at`, `received_at null`, `status` | Sent vs received counts come from its movements; differences are shown and logged |
| `laundry_service_item` | `id`, `tenant_id`, `property_id`, `code`, `name`, `service` (`wash` \| `press` \| `dry_clean` \| `express`), `price`, `currency`, `active` | Guest laundry price list |
| `laundry_order` | `id`, `tenant_id`, `property_id`, `reservation_room_id`, `room_id`, `status` (`collected` \| `processing` \| `ready` \| `delivered`), `folio_line_id null`, `created_at`, `delivered_at` | Charge posted to the folio on `delivered` |
| `laundry_order_line` | `(order_id, service_item_id)`, `tenant_id`, `quantity`, `unit_price` | |

## Phase 6: Media and events

| Table | Key columns | Notes |
|---|---|---|
| `media_asset` | `id`, `tenant_id`, `content_hash bytea` (BLAKE3), `kind` (`image` \| `video` \| `audio` \| `document`), `mime`, `bytes`, `width`, `height`, `duration_ms`, `status` (`uploading` \| `processing` \| `ready` \| `failed`), `visibility` (`public` \| `private`), `original_key`, `archive_key`, `created_by`, `created_at` | `unique (tenant_id, content_hash)` (deduplication) |
| `media_rendition` | `(asset_id, name)`, `tenant_id`, `key`, `mime`, `bytes`, `width`, `height` | e.g. `avif-640`, `webp-1024`, `hls-master` |
| `media_link` | `(asset_id, entity, entity_id)`, `tenant_id`, `sort_order` | Room-type galleries, issue photos, guest documents |
| `outbox` | `id bigserial`, `tenant_id`, `topic`, `payload jsonb`, `created_at`, `published_at null` | Written in the business transaction; a relay publishes to Pub/Sub for internal services. UI invalidation stays on `LISTEN/NOTIFY` |

## Phase 7: Folio, tax, payments, night audit

`property` gains: `vat_registered bool`, `tdl_band` (`standard` \| `small`), `tin text`, `sltda_licence_no text`, `audit_time time`.

| Table | Key columns | Notes |
|---|---|---|
| `folio` | `id`, `tenant_id`, `property_id`, `number` (per property), `reservation_id null`, `guest_id null`, `account_id null`, `currency`, `status` (`open` \| `closed`) | A reservation can have several folios (split billing) |
| `folio_line` | `id`, `tenant_id`, `property_id`, `folio_id`, `business_date`, `kind` (`charge` \| `payment` \| `adjustment`), `source` (`room` \| `meal_plan` \| `laundry` \| `pos` \| `manual` \| `no_show_fee`), `description`, `quantity`, `unit_amount`, `amount`, `currency`, `fx_rate numeric(18,8)`, `base_amount`, `reverses_id null`, `pos_bill_id null`, `created_by`, `created_at` | **Append-only.** A reversal points at the original with `reverses_id` |
| `folio_tax_line` | `id`, `tenant_id`, `folio_line_id`, `tax_rule_id`, `code`, `base_amount`, `amount`, `currency`, `presentation` (`on_bill` \| `absorbed`) | Computed and frozen at posting |
| `tax_rule` | `id`, `tenant_id`, `property_id`, `code`, `name`, `kind` (`tax` \| `levy` \| `service_charge`), `rate_bp`, `applies_to text[]`, `base_components text[]`, `sequence`, `presentation`, `condition jsonb`, `valid daterange` | Sri Lanka preset, order A (ARCHITECTURE §7.6) |
| `outlet` | `id`, `tenant_id`, `property_id`, `code`, `name` | POS outlets (restaurant, bar …) |
| `pos_bill` | `id`, `tenant_id`, `property_id`, `outlet_id`, `bill_no`, `business_date`, `currency`, `total`, `settlement` (`folio` \| `direct`), `folio_id null`, `status` (`open` \| `settled` \| `void`), `items jsonb` | `unique (property_id, outlet_id, bill_no)`, so `PostPosBill` is idempotent |
| `payment` | `id`, `tenant_id`, `property_id`, `folio_id`, `provider` (`manual` \| `cybersource` \| `mpgs` \| `payments_lk` \| `channel`), `method` (`cash` \| `card_terminal` \| `online_card` \| `bank_transfer` \| `account` \| `ota`), `status` (`pending` \| `authorized` \| `captured` \| `voided` \| `refunded` \| `failed`), `amount`, `currency`, `provider_ref`, `approval_code`, `card_brand`, `card_last4`, `residency_evidence_ref null`, `created_by`, `created_at` | State changes are new `payment_event` rows; `status` is the latest |
| `payment_event` | `id`, `tenant_id`, `payment_id`, `status`, `provider_payload jsonb`, `at` | Append-only; webhook and API responses |
| `card_token` | `id`, `tenant_id`, `guest_id`, `provider`, `token_ref`, `card_brand`, `last4`, `expiry` | Gateway tokens only; never card numbers |
| `merchant_account` | `id`, `tenant_id`, `property_id`, `provider`, `gateway_host`, `merchant_id`, `secret_name` (Secret Manager), `currencies char(3)[]`, `active` | Per-property credentials |
| `fx_rate` | `(date, currency, source)`, `buying numeric`, `selling numeric`, `published_on date` | **Global** (Central Bank data), no RLS, no `tenant_id` |
| `fx_override` | `(property_id, date, currency)`, `tenant_id`, `rate`, `set_by`, `set_at` | Manual override; audit-logged |
| `invoice` | `id`, `tenant_id`, `property_id`, `folio_id`, `serial_no` (gapless per property), `issued_at`, `lkr_total bigint` (whole rupees), `document jsonb`, `pdf_asset_id` | IRD tax-invoice format |
| `audit_run` | `id`, `tenant_id`, `property_id`, `business_date`, `status`, `steps jsonb`, `started_by`, `started_at`, `finished_at` | `unique (property_id, business_date)`; checkpointed steps |
| `daily_stat` | `(property_id, business_date)`, `tenant_id`, `rooms_available`, `rooms_sold`, `room_revenue`, `adr`, `revpar`, `by_segment jsonb`, `by_source jsonb`, `residency_mix jsonb` | Base currency; written by the audit |

## Phase 8: Settings and channels

| Table | Key columns | Notes |
|---|---|---|
| `invitation` | `id`, `tenant_id`, `email citext`, `grants jsonb`, `token_hash`, `expires_at`, `accepted_at null`, `invited_by` | Staff onboarding |
| `channel_connection` | `id`, `tenant_id`, `property_id`, `provider` (`channex`), `external_property_id`, `status`, `secret_name` | |
| `channel_room_map` | `(connection_id, room_type_id)`, `tenant_id`, `external_room_id` | |
| `channel_rate_map` | `(connection_id, rate_plan_id)`, `tenant_id`, `external_rate_id`, `currency` | Currency must equal the plan's |
| `channel_booking_event` | `id`, `tenant_id`, `connection_id`, `external_id`, `revision`, `payload jsonb`, `received_at`, `processed_at null`, `reservation_id null`, `error null` | `unique (connection_id, external_id, revision)` |

## Phase 9: Booking engine

| Table | Key columns | Notes |
|---|---|---|
| `ibe_site` | `id`, `tenant_id`, `property_id`, `domain` (unique), `branding jsonb`, `published bool` | |
| Holds | `reservation_room.status = 'tentative'` with `reservation.hold_expires_at` | A job releases expired holds (and their inventory) |
