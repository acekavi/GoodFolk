# Data Model

The reference schema for every phase. Phase 0 tables exist in `migrations/0001_foundation.sql`. Tables for later phases are the target design: the migrations that create them are written in their phase and may refine columns, but must keep the rules below.

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

## Phase 1: Rooms and inventory

`property` gains: `check_in_time time`, `check_out_time time`, `business_date date not null` (set to the property's local today when created; moved only by night audit).

| Table | Key columns | Constraints and indexes |
|---|---|---|
| `room_type` | `id`, `tenant_id`, `property_id`, `code`, `name`, `base_occupancy`, `max_adults`, `max_children`, `max_occupancy`, `bed_config jsonb`, `amenities text[]`, `sort_order`, `active`, `version` | `unique (property_id, code)`; `check (base_occupancy <= max_occupancy)` |
| `housekeeping_section` | `id`, `tenant_id`, `property_id`, `name` | `unique (property_id, name)` |
| `room` | `id`, `tenant_id`, `property_id`, `room_type_id`, `number text`, `floor text`, `section_id null`, `active`, `sort_order`, `version` | `unique (property_id, number)` |
| `block_reason` | `id`, `tenant_id`, `property_id`, `code`, `label`, `default_kind` | Seeded per property: `RENOVATION`, `CONSTRUCTION`, `MAINTENANCE`, `DEEP_CLEAN`, `OTHER` |
| `room_block` | `id`, `tenant_id`, `property_id`, `room_id`, `period daterange`, `kind` (`out_of_order` \| `out_of_service`), `reason_id`, `note`, `created_by`, `released_at null` | `exclude using gist (room_id with =, period with &&) where (released_at is null)`; GiST `(property_id, period)` |
| `inventory_day` | `(property_id, room_type_id, date)`, `tenant_id`, `physical`, `sold`, `out_of_order` | Counters updated in the same transaction as reservations and blocks. `available = physical − sold − out_of_order`. A nightly job recomputes them and alerts on drift |

Extensions: `btree_gist` (needed by the exclusion constraints).

| Global table | Key columns | Notes |
|---|---|---|
| `login_failure` | `id`, `email citext`, `at` | Sign-in throttling (`migrations/0003_login_throttle.sql`). No RLS and no `tenant_id`: looked up before a tenant is known. One row per attempt, written before the password is checked; a successful sign-in deletes the email's rows. The Phase 7 purge job deletes rows older than the window |

## Phase 2: Rates and meal plans

| Table | Key columns | Constraints and indexes |
|---|---|---|
| `rate_plan` | `id`, `tenant_id`, `property_id`, `code`, `name`, `kind` (`standard` \| `derived` \| `custom`), `segment` (`FIT_F` \| `FIT_L` \| `OTA` \| `TA` \| `IBE`), `residency` (`resident` \| `non_resident` \| null = any), `currency`, `parent_id null`, `derive_mode` (`percent` \| `amount`), `derive_value bigint` (basis points or minor units), `rounding_step bigint`, `inherit_restrictions bool`, `allowed_meal_plans text[]`, `cancellation_policy_id`, `active`, `version` | `unique (property_id, code)`; `check ((kind = 'derived') = (parent_id is not null))`. Same currency as the parent (enforced in the module, tested). Derivation depth ≤ 3; no cycles |
| `rate_plan_room_type` | `(rate_plan_id, room_type_id)`, `tenant_id` | Which room types a plan sells |
| `rate_day` | `(rate_plan_id, room_type_id, date, occupancy)`, `tenant_id`, `property_id`, `amount bigint`, `closed bool`, `min_stay`, `max_stay`, `closed_to_arrival`, `closed_to_departure` | **Resolved** prices, derived plans included. Recomputed in the same transaction as the parent change. Index `(property_id, date)` for grids and search |
| `meal_supplement` | `id`, `tenant_id`, `property_id`, `meal_plan` (`RO` \| `BB` \| `HB` \| `FB`), `currency`, `adult_amount`, `child_amount`, `valid daterange` | Per person per night on top of the room price. `RO` is always 0. Exclusion: no overlapping `valid` for the same `(property_id, meal_plan, currency)` |
| `cancellation_policy` | `id`, `tenant_id`, `property_id`, `name`, `rules jsonb` | Rules: list of `{days_before_arrival, penalty: {kind: nights\|percent\|amount, value}}`; `no_show` penalty |

## Phase 3: Reservations and guests

| Table | Key columns | Constraints and indexes |
|---|---|---|
| `guest` | `id`, `tenant_id`, `first_name`, `last_name`, `email citext null`, `phone`, `country char(2)`, `residency` (`resident` \| `non_resident`), `id_doc_type`, `id_doc_number_enc bytea` (field-level encrypted), `notes`, `version` | Tenant-wide, so a chain shares guest history. Trigram index on names for search (`pg_trgm`) |
| `account` | `id`, `tenant_id`, `kind` (`company` \| `travel_agent`), `name`, `contact jsonb`, `credit_limit bigint null`, `currency` | Companies and TAs (city ledger in Phase 7) |
| `reservation` | `id`, `tenant_id`, `property_id`, `confirmation_no` (unique per property), `status`, `source` (`front_desk` \| `ibe` \| `channel` \| `phone` \| `email`), `channel_code null`, `channel_ref null`, `booker_guest_id`, `account_id null`, `segment`, `guarantee` (`none` \| `card` \| `deposit` \| `account`), `hold_expires_at null`, `notes`, `created_by`, `created_at`, `version` | `unique (property_id, channel_code, channel_ref)` where not null (idempotent channel ingestion) |
| `reservation_room` | `id`, `tenant_id`, `property_id`, `reservation_id`, `room_type_id`, `room_id null`, `stay daterange`, `adults`, `children`, `rate_plan_id`, `meal_plan`, `status` (`tentative` \| `confirmed` \| `checked_in` \| `checked_out` \| `cancelled` \| `no_show`), `primary_guest_id`, `eta time null`, `version` | **`exclude using gist (room_id with =, stay with &&) where (room_id is not null and status not in ('cancelled','no_show'))`**, so double booking is impossible. GiST `(property_id, stay)` serves tape-chart tiles. Check-out early sets `upper(stay)` to the actual date |
| `reservation_night` | `(reservation_room_id, date)`, `tenant_id`, `room_amount`, `meal_amount`, `currency` | Price snapshot at booking; later rate changes do not reprice existing bookings |
| `reservation_guest` | `(reservation_room_id, guest_id)`, `tenant_id` | Additional occupants |
| `property_counter` | `(property_id, name)`, `tenant_id`, `value bigint` | Gapless numbers (`confirmation`, later `invoice`), incremented with `update … returning` inside the posting transaction |

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
| Holds | `reservation.status = 'tentative'` with `hold_expires_at` | A job releases expired holds (and their inventory) |
