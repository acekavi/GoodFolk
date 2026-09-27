-- Phase 3: guests, reservations, the rooms they book with a nightly price snapshot, and gapless per-property
-- counters. A reservation has no stored status: it is derived from its rooms' statuses when read. Amounts are
-- bigint minor units in the room's currency (the rate plan's at booking).
create extension if not exists pg_trgm;

-- Guests belong to the tenant, not to a property, so a chain shares guest history. The ID document number is
-- sealed with AES-256-GCM (db::crypto): id_doc_number_enc is nonce || ciphertext || tag, id_doc_key_id names the
-- key that sealed it (for rotation) and id_doc_last4 is the plaintext tail that responses show masked.
create table guest (
  id uuid primary key,
  tenant_id uuid not null references tenant (id) on delete cascade,
  -- Empty for guests with a single name.
  first_name text not null default '' check (length(first_name) <= 100),
  last_name text not null check (length(last_name) between 1 and 100),
  email text check (length(email) between 3 and 254 and email = lower(email)),
  phone text check (length(phone) between 3 and 30),
  country char(2) check (country ~ '^[A-Z]{2}$'),
  residency text not null check (residency in ('resident', 'non_resident')),
  id_doc_type text check (id_doc_type in ('passport', 'nic', 'driving_licence', 'other')),
  id_doc_number_enc bytea check (octet_length(id_doc_number_enc) >= 28),
  id_doc_key_id text check (id_doc_key_id ~ '^[A-Za-z0-9_-]{1,16}$'),
  id_doc_last4 text check (length(id_doc_last4) <= 4),
  notes text not null default '' check (length(notes) <= 2000),
  version integer not null default 1,
  created_at timestamptz not null default now(),
  -- Lets reservations reference (tenant_id, guest id), so a booking can never name another tenant's guest.
  unique (tenant_id, id),
  constraint guest_id_doc_check
    check (num_nulls(id_doc_type, id_doc_number_enc, id_doc_key_id, id_doc_last4) in (0, 4))
);
-- Guest search: similarity on the full name, exact match on email or phone. Queries must use this expression.
create index guest_name_trgm_idx on guest using gin (lower(first_name || ' ' || last_name) gin_trgm_ops);
create index guest_email_idx on guest (tenant_id, email) where email is not null;
create index guest_phone_idx on guest (tenant_id, phone) where phone is not null;

-- Gapless numbers per property (`confirmation` now, `invoice` in Phase 7), taken with an upsert that increments
-- `value` inside the transaction that uses the number, so a rolled-back booking does not burn one.
create table property_counter (
  tenant_id uuid not null,
  property_id uuid not null,
  name text not null check (name in ('confirmation')),
  value bigint not null check (value >= 1),
  primary key (property_id, name),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade
);

-- One booking: one or more rooms (reservation_room). The confirmation number is the property code and a
-- zero-padded sequence of at least 6 digits (GFK-000123, GFK-1000000). Channel columns and account_id arrive
-- with channels (Phase 8) and accounts.
create table reservation (
  id uuid primary key,
  tenant_id uuid not null,
  property_id uuid not null,
  confirmation_no text not null,
  source text not null check (source in ('front_desk', 'ibe', 'channel', 'phone', 'email')),
  booker_guest_id uuid not null,
  guarantee text not null default 'none' check (guarantee in ('none', 'card', 'deposit', 'account')),
  hold_expires_at timestamptz,
  notes text not null default '' check (length(notes) <= 2000),
  created_by uuid references app_user (id) on delete set null,
  created_at timestamptz not null default now(),
  version integer not null default 1,
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade,
  foreign key (tenant_id, booker_guest_id) references guest (tenant_id, id),
  constraint reservation_confirmation_no_check
    check (confirmation_no ~ '^[A-Z0-9]{2,10}-([0-9]{6}|[1-9][0-9]{6,})$'),
  constraint reservation_property_id_confirmation_no_key unique (property_id, confirmation_no),
  unique (property_id, id)
);
create index reservation_booker_guest_idx on reservation (tenant_id, booker_guest_id);
-- Confirmation-number prefix search (starts_with(confirmation_no, …)), which the collation-aware unique index cannot serve.
create index reservation_confirmation_prefix_idx on reservation (property_id, confirmation_no text_pattern_ops);

-- One room of a booking for [check-in, check-out). room_id is null until a room is assigned; an assigned room
-- can hold only one active stay per night. cancellation_terms is the plan's cancellation policy at booking
-- ({"rules": [...], "no_show": {...}}, null when the plan had none); cancelling records when, by whom and the
-- penalty those terms give.
create table reservation_room (
  id uuid primary key,
  tenant_id uuid not null,
  property_id uuid not null,
  reservation_id uuid not null,
  room_type_id uuid not null,
  room_id uuid,
  stay daterange not null,
  -- lower(stay), stored so the list's arrival filters and keyset use plain date comparisons: those are
  -- leakproof, so under row-level security they can be index conditions, where lower(stay) can't. Never null:
  -- reservation_room_stay_check refuses empty stays.
  arrival date generated always as (lower(stay)) stored,
  adults integer not null check (adults between 1 and 50),
  children integer not null check (children between 0 and 50),
  rate_plan_id uuid not null,
  meal_plan text not null check (meal_plan in ('RO', 'BB', 'HB', 'FB')),
  status text not null
    check (status in ('tentative', 'confirmed', 'checked_in', 'checked_out', 'cancelled', 'no_show')),
  primary_guest_id uuid not null,
  currency char(3) not null check (currency ~ '^[A-Z]{3}$'),
  cancellation_terms jsonb check (jsonb_typeof(cancellation_terms) = 'object'),
  cancelled_at timestamptz,
  cancelled_by uuid references app_user (id) on delete set null,
  cancellation_penalty bigint check (cancellation_penalty >= 0),
  eta time,
  version integer not null default 1,
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade,
  foreign key (property_id, reservation_id) references reservation (property_id, id) on delete cascade,
  foreign key (property_id, room_type_id) references room_type (property_id, id),
  foreign key (property_id, room_id) references room (property_id, id),
  foreign key (property_id, rate_plan_id) references rate_plan (property_id, id),
  foreign key (tenant_id, primary_guest_id) references guest (tenant_id, id),
  unique (property_id, id),
  -- Date ranges are always stored as [lower, upper); this also refuses empty and unbounded stays.
  constraint reservation_room_stay_check
    check (not isempty(stay) and lower_inc(stay) and not upper_inc(stay) and not upper_inf(stay)),
  -- A cancelled stay, and only a cancelled one, records when and its penalty; cancelled_by may go null later
  -- (the user is deleted) but is never set on a stay that is not cancelled.
  constraint reservation_room_cancellation_check check (
    (status = 'cancelled') = (cancelled_at is not null)
    and (cancelled_at is null) = (cancellation_penalty is null)
    and (cancelled_by is null or cancelled_at is not null)
  ),
  constraint reservation_room_no_double_booking exclude using gist (room_id with =, stay with &&)
    where (room_id is not null and status not in ('cancelled', 'no_show'))
);
-- Stays overlapping a date range (tape chart, lists by arrival).
create index reservation_room_property_stay_idx on reservation_room using gist (property_id, stay);
create index reservation_room_reservation_idx on reservation_room (reservation_id);
-- The reservations list by arrival (its default sort), paged by (arrival, id).
create index reservation_room_arrival_idx on reservation_room (property_id, arrival, id);

-- The price of each night of a stay, fixed at booking: later rate changes do not reprice existing bookings.
create table reservation_night (
  tenant_id uuid not null,
  property_id uuid not null,
  reservation_room_id uuid not null,
  date date not null,
  room_amount bigint not null check (room_amount >= 0),
  meal_amount bigint not null check (meal_amount >= 0),
  currency char(3) not null check (currency ~ '^[A-Z]{3}$'),
  primary key (reservation_room_id, date),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade,
  foreign key (property_id, reservation_room_id) references reservation_room (property_id, id) on delete cascade
);

-- A reservation's history: the audit entries of the reservation and of its rooms.
create index audit_log_entity_idx on audit_log (entity_id, at desc) where entity_id is not null;

do $$
declare t text;
begin
  foreach t in array array['guest', 'property_counter', 'reservation', 'reservation_room', 'reservation_night'] loop
    execute format('alter table %I enable row level security', t);
    execute format('alter table %I force row level security', t);
    execute format(
      'create policy tenant_isolation on %I for all using (tenant_id = (select app.current_tenant())) with check (tenant_id = (select app.current_tenant()))',
      t);
  end loop;
end $$;
