-- Phase 5: room condition, housekeeping assignments, room issues and the housekeeping supervisor role.

-- One row per room, created with the room. No row means no condition, so every room must have one.
create table room_condition (
  room_id uuid primary key references room(id) on delete cascade,
  tenant_id uuid not null,
  property_id uuid not null references property(id) on delete cascade,
  condition text not null default 'clean' check (condition in ('dirty', 'cleaning', 'clean', 'inspected')),
  version integer not null default 1,
  changed_at timestamptz not null default now(),
  changed_by uuid
);
insert into room_condition (room_id, tenant_id, property_id) select id, tenant_id, property_id from room;

create table hk_assignment (
  tenant_id uuid not null,
  property_id uuid not null references property(id) on delete cascade,
  business_date date not null,
  room_id uuid not null references room(id) on delete cascade,
  user_id uuid not null,
  primary key (property_id, business_date, room_id)
);

create table room_issue (
  id uuid primary key,
  tenant_id uuid not null,
  property_id uuid not null references property(id) on delete cascade,
  room_id uuid not null references room(id) on delete cascade,
  severity text not null check (severity in ('low', 'medium', 'high')),
  description text not null check (char_length(description) between 1 and 2000),
  status text not null default 'open' check (status in ('open', 'resolved')),
  reported_by uuid not null,
  reported_at timestamptz not null default now(),
  resolved_by uuid,
  resolved_at timestamptz,
  version integer not null default 1,
  check ((status = 'resolved') = (resolved_at is not null))
);
create index room_issue_open_idx on room_issue (property_id, room_id) where status = 'open';

alter table reservation_room add column condition_before_check_in text
  check (condition_before_check_in in ('dirty', 'cleaning', 'clean', 'inspected'));

alter table role_grant drop constraint role_grant_role_check;
alter table role_grant add constraint role_grant_role_check
  check (role in ('owner', 'manager', 'front_desk', 'housekeeping', 'accountant', 'hk_supervisor'));

do $$
declare t text;
begin
  foreach t in array array['room_condition', 'hk_assignment', 'room_issue'] loop
    execute format('alter table %I enable row level security', t);
    execute format('alter table %I force row level security', t);
    execute format(
      'create policy tenant_isolation on %I for all using (tenant_id = (select app.current_tenant())) with check (tenant_id = (select app.current_tenant()))',
      t);
  end loop;
end $$;
