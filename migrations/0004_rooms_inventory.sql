-- Phase 1: room types, housekeeping sections, rooms, block reasons, room blocks and inventory counters.
create extension if not exists btree_gist;

-- Property settings used from Phase 3 on. business_date is the property's local "today" when it is created;
-- only night audit (Phase 7) moves it.
alter table property
  add column check_in_time time not null default '14:00',
  add column check_out_time time not null default '12:00',
  add column business_date date;
-- Lets child rows reference (tenant_id, property_id), so a row can never point at another tenant's property.
alter table property add constraint property_tenant_id_id_key unique (tenant_id, id);

create table room_type (
  id uuid primary key,
  tenant_id uuid not null,
  property_id uuid not null,
  code text not null check (code ~ '^[A-Z0-9]{1,10}$'),
  name text not null check (length(name) between 1 and 100),
  base_occupancy integer not null check (base_occupancy between 1 and 50),
  max_adults integer not null check (max_adults between 1 and 50),
  max_children integer not null check (max_children between 0 and 50),
  max_occupancy integer not null check (max_occupancy between 1 and 50),
  bed_config jsonb not null default '[]',
  amenities text[] not null default '{}',
  sort_order integer not null default 0,
  active boolean not null default true,
  version integer not null default 1,
  created_at timestamptz not null default now(),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade,
  unique (property_id, code),
  unique (property_id, id),
  check (base_occupancy <= max_occupancy),
  check (max_adults <= max_occupancy),
  check (max_occupancy <= max_adults + max_children)
);

create table housekeeping_section (
  id uuid primary key,
  tenant_id uuid not null,
  property_id uuid not null,
  name text not null check (length(name) between 1 and 100),
  version integer not null default 1,
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade,
  unique (property_id, name),
  unique (property_id, id)
);

-- Composite foreign keys keep a room's type and section in the room's own property.
create table room (
  id uuid primary key,
  tenant_id uuid not null,
  property_id uuid not null,
  room_type_id uuid not null,
  number text not null check (number ~ '^[A-Za-z0-9-]{1,10}$'),
  floor text check (length(floor) between 1 and 20),
  section_id uuid,
  active boolean not null default true,
  sort_order integer not null default 0,
  version integer not null default 1,
  created_at timestamptz not null default now(),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade,
  foreign key (property_id, room_type_id) references room_type (property_id, id),
  foreign key (property_id, section_id) references housekeeping_section (property_id, id),
  unique (property_id, number),
  unique (property_id, id)
);
create index room_room_type_idx on room (room_type_id);

create table block_reason (
  id uuid primary key,
  tenant_id uuid not null,
  property_id uuid not null,
  code text not null check (code ~ '^[A-Z0-9_]{1,20}$'),
  label text not null check (length(label) between 1 and 100),
  default_kind text not null check (default_kind in ('out_of_order', 'out_of_service')),
  active boolean not null default true,
  version integer not null default 1,
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade,
  unique (property_id, code),
  unique (property_id, id)
);

-- A room out of order (removed from inventory) or out of service (still sellable, flagged) for [from, to).
-- released_at is set when a block is cancelled before it starts; shortening a block moves upper(period).
create table room_block (
  id uuid primary key,
  tenant_id uuid not null,
  property_id uuid not null,
  room_id uuid not null,
  period daterange not null check (not isempty(period) and not lower_inf(period) and not upper_inf(period)),
  kind text not null check (kind in ('out_of_order', 'out_of_service')),
  reason_id uuid not null,
  note text not null default '' check (length(note) <= 500),
  created_by uuid references app_user (id) on delete set null,
  created_at timestamptz not null default now(),
  released_at timestamptz,
  version integer not null default 1,
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade,
  foreign key (property_id, room_id) references room (property_id, id) on delete cascade,
  foreign key (property_id, reason_id) references block_reason (property_id, id),
  constraint room_block_no_overlap exclude using gist (room_id with =, period with &&) where (released_at is null)
);
create index room_block_property_period_idx on room_block using gist (property_id, period);

-- One counter row per room type per day, from the business date to 730 days ahead, kept in step with rooms
-- and blocks in the same transaction. available = physical - sold - out_of_order.
create table inventory_day (
  tenant_id uuid not null,
  property_id uuid not null,
  room_type_id uuid not null,
  date date not null,
  physical integer not null default 0 check (physical >= 0),
  sold integer not null default 0 check (sold >= 0),
  out_of_order integer not null default 0 check (out_of_order between 0 and physical),
  primary key (property_id, room_type_id, date),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade,
  foreign key (property_id, room_type_id) references room_type (property_id, id) on delete cascade
);
create index inventory_day_property_date_idx on inventory_day (property_id, date);

-- Backfill existing properties. FORCE ROW LEVEL SECURITY applies policies to the table owner too, so it is
-- lifted while the owner running this migration updates every tenant's rows. The seeded ids are random
-- (Postgres 17 has no UUIDv7 function); new rows get UUIDv7 ids from Rust.
alter table property no force row level security;
update property set business_date = (now() at time zone timezone)::date;
insert into block_reason (id, tenant_id, property_id, code, label, default_kind)
select gen_random_uuid(), p.tenant_id, p.id, r.code, r.label, r.kind
from property p
cross join (values
  ('RENOVATION', 'Renovation', 'out_of_order'),
  ('CONSTRUCTION', 'Construction', 'out_of_order'),
  ('MAINTENANCE', 'Maintenance', 'out_of_order'),
  ('DEEP_CLEAN', 'Deep clean', 'out_of_service'),
  ('OTHER', 'Other', 'out_of_order')
) as r (code, label, kind);
alter table property force row level security;
alter table property alter column business_date set not null;

do $$
declare t text;
begin
  foreach t in array array['room_type', 'housekeeping_section', 'room', 'block_reason', 'room_block', 'inventory_day'] loop
    execute format('alter table %I enable row level security', t);
    execute format('alter table %I force row level security', t);
    execute format(
      'create policy tenant_isolation on %I for all using (tenant_id = (select app.current_tenant())) with check (tenant_id = (select app.current_tenant()))',
      t);
  end loop;
end $$;
