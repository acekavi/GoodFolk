-- Phase 3b: accounts, additional occupants, a per-room-type overbooking allowance and check-in/out columns.

-- Companies and travel agents a reservation can be billed to (invoicing and the city ledger are Phase 7).
-- Tenant-wide like guest, not scoped to a property, so a chain shares its accounts.
create table account (
  id uuid primary key,
  tenant_id uuid not null,
  kind text not null check (kind in ('company', 'travel_agent')),
  name text not null check (length(name) between 1 and 200),
  contact jsonb not null default '{}' check (jsonb_typeof(contact) = 'object'),
  credit_limit bigint check (credit_limit >= 0),
  currency char(3) not null check (currency ~ '^[A-Z]{3}$'),
  active boolean not null default true,
  version integer not null default 1,
  created_at timestamptz not null default now(),
  -- Lets reservations reference (tenant_id, account id), so a booking can never name another tenant's account.
  unique (tenant_id, id)
);
-- A list sorted or filtered by lower(name): tenant_id is leakproof and becomes an index condition under forced
-- row-level security, but lower(name) is not, so the rest of the scan is a plain filter over the tenant's own
-- accounts. Accounts are few per tenant, so that scan is fine.
create index account_tenant_name_idx on account (tenant_id, lower(name));

alter table reservation add column account_id uuid;
alter table reservation add foreign key (tenant_id, account_id) references account (tenant_id, id);

-- Additional occupants of a room, beyond its primary_guest_id. Composite foreign keys keep both ends in the
-- room's own property and tenant, and dropping the room takes its occupants with it.
create table reservation_guest (
  tenant_id uuid not null,
  property_id uuid not null,
  reservation_room_id uuid not null,
  guest_id uuid not null,
  primary key (reservation_room_id, guest_id),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade,
  foreign key (property_id, reservation_room_id) references reservation_room (property_id, id) on delete cascade,
  foreign key (tenant_id, guest_id) references guest (tenant_id, id)
);
create index reservation_guest_tenant_guest_idx on reservation_guest (tenant_id, guest_id);

-- How many more rooms of this type may be sold than are physically available: a night is sellable when
-- physical - sold - out_of_order + overbooking > 0.
alter table room_type add column overbooking integer not null default 0 check (overbooking between 0 and 20);

-- Check-in/out timestamps. No backfill: 3a has no check-in path, so no existing row can have status checked_in
-- or checked_out, and the new columns default to null, which satisfies every check below as-is.
alter table reservation_room
  add column checked_in_at timestamptz,
  add column checked_in_business_date date,
  add column checked_out_at timestamptz;

alter table reservation_room add constraint reservation_room_checked_in_at_check
  check ((status in ('checked_in', 'checked_out')) = (checked_in_at is not null));
alter table reservation_room add constraint reservation_room_checked_in_business_date_check
  check ((checked_in_at is not null) = (checked_in_business_date is not null));
alter table reservation_room add constraint reservation_room_checked_out_at_check
  check ((status = 'checked_out') = (checked_out_at is not null));

do $$
declare t text;
begin
  foreach t in array array['account', 'reservation_guest'] loop
    execute format('alter table %I enable row level security', t);
    execute format('alter table %I force row level security', t);
    execute format(
      'create policy tenant_isolation on %I for all using (tenant_id = (select app.current_tenant())) with check (tenant_id = (select app.current_tenant()))',
      t);
  end loop;
end $$;
