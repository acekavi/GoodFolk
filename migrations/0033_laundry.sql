-- Phase 8: linen and guest laundry. Linen items, where they are kept, the par each room type needs, an append-only
-- movement ledger with the stock it adds up to, laundry batches sent out, the price list for guests' laundry, and
-- guests' laundry orders with their lines. Ops alerts gain the `linen` kind.

alter table ops_alert drop constraint ops_alert_kind_check;
alter table ops_alert add constraint ops_alert_kind_check check (kind in ('audit', 'drift', 'linen'));

create table linen_item (
  id uuid primary key, tenant_id uuid not null, property_id uuid not null,
  code text not null check (code ~ '^[A-Z0-9_]{1,20}$'),
  name text not null check (length(name) between 1 and 100),
  active boolean not null default true, version integer not null default 1,
  unique (property_id, code), unique (property_id, id),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade
);

create table linen_location (
  id uuid primary key, tenant_id uuid not null, property_id uuid not null,
  name text not null check (length(name) between 1 and 100),
  kind text not null check (kind in ('store', 'pantry', 'laundry', 'vendor')),
  section_id uuid,
  active boolean not null default true, version integer not null default 1,
  unique (property_id, name), unique (property_id, id),
  check ((kind = 'pantry') = (section_id is not null)),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade,
  foreign key (property_id, section_id) references housekeeping_section (property_id, id)
);
create unique index linen_location_one_store_idx on linen_location (property_id) where kind = 'store' and active;
create unique index linen_location_one_pantry_per_section_idx on linen_location (section_id) where active;

create table linen_par (
  tenant_id uuid not null, property_id uuid not null, room_type_id uuid not null, item_id uuid not null,
  quantity integer not null check (quantity between 1 and 100),
  primary key (room_type_id, item_id),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade,
  foreign key (property_id, room_type_id) references room_type (property_id, id),
  foreign key (property_id, item_id) references linen_item (property_id, id)
);

create table laundry_batch (
  id uuid primary key, tenant_id uuid not null, property_id uuid not null,
  to_location_id uuid not null, from_location_id uuid not null,
  status text not null check (status in ('sent', 'received')),
  note text check (length(note) <= 500),
  sent_by uuid, sent_at timestamptz not null default now(),
  received_by uuid, received_at timestamptz,
  version integer not null default 1,
  check ((status = 'received') = (received_at is not null)),
  constraint laundry_batch_locations_check check (from_location_id <> to_location_id),
  unique (property_id, id),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade,
  foreign key (property_id, to_location_id) references linen_location (property_id, id),
  foreign key (property_id, from_location_id) references linen_location (property_id, id)
);

create table linen_movement (
  id uuid primary key, tenant_id uuid not null, property_id uuid not null,
  item_id uuid not null,
  kind text not null check (kind in ('receive', 'transfer', 'send', 'return', 'write_off')),
  quantity integer not null check (quantity between 1 and 100000),
  from_location_id uuid, to_location_id uuid,
  batch_id uuid,
  reason text check (length(reason) between 1 and 500),
  business_date date not null,
  moved_by uuid, moved_at timestamptz not null default now(),
  check ((kind = 'receive') = (from_location_id is null)),
  check ((kind = 'write_off') = (to_location_id is null)),
  check (kind <> 'write_off' or reason is not null),
  check (from_location_id is distinct from to_location_id),
  check ((kind in ('send', 'return')) <= (batch_id is not null)),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade,
  foreign key (property_id, item_id) references linen_item (property_id, id),
  foreign key (property_id, from_location_id) references linen_location (property_id, id),
  foreign key (property_id, to_location_id) references linen_location (property_id, id),
  foreign key (property_id, batch_id) references laundry_batch (property_id, id)
);
create index linen_movement_property_idx on linen_movement (property_id, moved_at desc);
create index linen_movement_batch_idx on linen_movement (batch_id) where batch_id is not null;

create table linen_stock (
  tenant_id uuid not null, property_id uuid not null, location_id uuid not null, item_id uuid not null,
  quantity integer not null check (quantity >= 0),
  primary key (location_id, item_id),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade,
  foreign key (property_id, location_id) references linen_location (property_id, id),
  foreign key (property_id, item_id) references linen_item (property_id, id)
);

create table laundry_price (
  id uuid primary key, tenant_id uuid not null, property_id uuid not null,
  item_code text not null check (item_code ~ '^[A-Z0-9_]{1,20}$'),
  service text not null check (service in ('wash', 'press', 'dry_clean', 'express')),
  name text not null check (length(name) between 1 and 100),
  price bigint not null check (price > 0),
  active boolean not null default true, version integer not null default 1,
  unique (property_id, item_code, service), unique (property_id, id),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade
);

create table laundry_order (
  id uuid primary key, tenant_id uuid not null, property_id uuid not null,
  reservation_room_id uuid not null,
  room_number text not null,
  status text not null check (status in ('collected', 'processing', 'ready', 'delivered', 'cancelled')),
  note text check (length(note) <= 500),
  created_by uuid, created_at timestamptz not null default now(),
  status_by uuid, status_at timestamptz not null default now(),
  delivered_at timestamptz,
  version integer not null default 1,
  check ((status = 'delivered') = (delivered_at is not null)),
  unique (property_id, id),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade,
  -- 0030 dropped reservation_room (tenant_id, id); the property key still keeps the stay in this property.
  foreign key (property_id, reservation_room_id) references reservation_room (property_id, id)
);
create index laundry_order_open_idx on laundry_order (property_id, status) where status not in ('delivered', 'cancelled');
create index laundry_order_stay_idx on laundry_order (reservation_room_id);

create table laundry_order_line (
  tenant_id uuid not null, property_id uuid not null, order_id uuid not null, position integer not null check (position between 1 and 50),
  price_id uuid not null, item_code text not null, service text not null, name text not null,
  unit_price bigint not null check (unit_price > 0),
  quantity integer not null check (quantity between 1 and 100),
  primary key (order_id, position),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade,
  foreign key (property_id, order_id) references laundry_order (property_id, id) on delete cascade,
  foreign key (property_id, price_id) references laundry_price (property_id, id)
);

do $$
declare t text;
begin
  foreach t in array array['linen_item', 'linen_location', 'linen_par', 'laundry_batch', 'linen_movement', 'linen_stock',
                           'laundry_price', 'laundry_order', 'laundry_order_line'] loop
    execute format('alter table %I enable row level security', t);
    execute format('alter table %I force row level security', t);
    execute format(
      'create policy tenant_isolation on %I for all using (tenant_id = (select app.current_tenant())) with check (tenant_id = (select app.current_tenant()))',
      t);
  end loop;
end $$;

-- The default privileges in 0001 grant every new table its full set to goodfolk_app, so the revokes come after the
-- creates. The movement ledger is append-only; order lines are replaced while an order is `collected`; nothing else
-- is deleted.
revoke all on linen_item, linen_location, linen_par, laundry_batch, linen_movement, linen_stock, laundry_price,
  laundry_order, laundry_order_line from goodfolk_app;
grant select, insert, update on linen_item, linen_location, linen_par, laundry_batch, linen_stock, laundry_price,
  laundry_order to goodfolk_app;
grant select, insert on linen_movement to goodfolk_app;
grant select, insert, delete on laundry_order_line to goodfolk_app;
