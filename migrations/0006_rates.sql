-- Phase 2: rate plans, resolved daily prices and restrictions, meal supplements and cancellation policies.
-- Amounts are bigint minor units in the plan's (or supplement's) currency, at most 100 000 000 000.

-- A price changed by `value` basis points (`percent`) or minor units (`amount`), either possibly negative,
-- never below 0, rounded half-up to a multiple of `step`. Derived plans apply their formula to the parent's
-- price with it, and bulk changes apply theirs to a plan's own prices. Integer arithmetic only: every input
-- is bounded by the checks below, so nothing overflows. Not STRICT, so the planner inlines it into the
-- statements that call it once per row.
create function app.derive_amount(base bigint, mode text, value bigint, step bigint) returns bigint
language sql immutable parallel safe
as $$
  select case mode
    when 'percent' then (2 * greatest(base * (10000 + value), 0) + 10000 * step) / (20000 * step) * step
    when 'amount' then (2 * greatest(base + value, 0) + step) / (2 * step) * step
  end
$$;

-- Penalties for cancelling: `rules` is a list of {"days_before_arrival": n, "penalty": {"kind", "value"}},
-- `no_show` one penalty. Kinds: `nights` (value = nights), `percent` (basis points of the stay), `amount`
-- (minor units). The rates module checks the shape; reservations (Phase 3) apply them.
create table cancellation_policy (
  id uuid primary key,
  tenant_id uuid not null,
  property_id uuid not null,
  name text not null check (length(name) between 1 and 100),
  rules jsonb not null check (jsonb_typeof(rules) = 'array'),
  no_show jsonb not null check (jsonb_typeof(no_show) = 'object'),
  version integer not null default 1,
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade,
  unique (property_id, name),
  unique (property_id, id)
);

-- Standard plans are priced by hand and may have derived plans; derived plans are priced from their parent
-- (same currency, at most 3 levels below a standard plan, no cycles: checked by the rates module); custom plans
-- are priced by hand and stand alone.
create table rate_plan (
  id uuid primary key,
  tenant_id uuid not null,
  property_id uuid not null,
  code text not null check (code ~ '^[A-Z0-9_-]{1,20}$'),
  name text not null check (length(name) between 1 and 100),
  kind text not null check (kind in ('standard', 'derived', 'custom')),
  segment text not null check (segment in ('FIT_F', 'FIT_L', 'OTA', 'TA', 'IBE')),
  residency text check (residency in ('resident', 'non_resident')),
  currency char(3) not null check (currency ~ '^[A-Z]{3}$'),
  parent_id uuid,
  derive_mode text check (derive_mode in ('percent', 'amount')),
  derive_value bigint check (derive_value between -100000000000 and 100000000000),
  rounding_step bigint not null default 1 check (rounding_step between 1 and 100000000),
  extra_adult_amount bigint not null default 0 check (extra_adult_amount between 0 and 100000000000),
  inherit_restrictions boolean not null default false,
  allowed_meal_plans text[] not null default '{RO}'
    check (cardinality(allowed_meal_plans) > 0 and allowed_meal_plans <@ array['RO', 'BB', 'HB', 'FB']),
  cancellation_policy_id uuid,
  active boolean not null default true,
  version integer not null default 1,
  created_at timestamptz not null default now(),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade,
  foreign key (property_id, parent_id) references rate_plan (property_id, id),
  foreign key (property_id, cancellation_policy_id) references cancellation_policy (property_id, id),
  unique (property_id, code),
  unique (property_id, id),
  constraint rate_plan_derivation_check check (
    (kind = 'derived') = (parent_id is not null)
    and (kind = 'derived') = (derive_mode is not null and derive_value is not null)
    and parent_id is distinct from id
    and (derive_mode is distinct from 'percent' or derive_value between -10000 and 100000)
  ),
  -- FIT-F is sold only to non-residents and FIT-L only to residents.
  constraint rate_plan_segment_residency_check check (
    (segment <> 'FIT_F' or residency is not distinct from 'non_resident')
    and (segment <> 'FIT_L' or residency is not distinct from 'resident')
  )
);
create index rate_plan_parent_idx on rate_plan (parent_id);

-- The room types a plan sells. Prices and restrictions exist only for these, and go when a type is removed.
create table rate_plan_room_type (
  tenant_id uuid not null,
  property_id uuid not null,
  rate_plan_id uuid not null,
  room_type_id uuid not null,
  primary key (rate_plan_id, room_type_id),
  unique (property_id, rate_plan_id, room_type_id),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade,
  foreign key (property_id, rate_plan_id) references rate_plan (property_id, id) on delete cascade,
  foreign key (property_id, room_type_id) references room_type (property_id, id) on delete cascade
);

-- Resolved prices per plan, room type, date and occupancy (adults), derived plans included: a write to a
-- plan's prices recomputes its descendants' rows in the same transaction, so reads never derive anything.
-- Half of each page is left free: a bulk change rewrites every price on a page, and the new versions then fit
-- beside the old ones (HOT updates, no index writes).
create table rate_day (
  tenant_id uuid not null,
  property_id uuid not null,
  rate_plan_id uuid not null,
  room_type_id uuid not null,
  date date not null,
  occupancy integer not null check (occupancy between 1 and 50),
  amount bigint not null check (amount between 0 and 100000000000),
  primary key (rate_plan_id, date, room_type_id, occupancy),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade,
  foreign key (property_id, rate_plan_id, room_type_id)
    references rate_plan_room_type (property_id, rate_plan_id, room_type_id) on delete cascade
) with (fillfactor = 50);
create index rate_day_property_date_idx on rate_day (property_id, date);

-- Resolved restrictions per plan, room type and date. A derived plan that inherits restrictions gets a copy
-- of its parent's rows in the same transaction as every change to them.
create table rate_restriction (
  tenant_id uuid not null,
  property_id uuid not null,
  rate_plan_id uuid not null,
  room_type_id uuid not null,
  date date not null,
  closed boolean not null default false,
  min_stay integer check (min_stay between 1 and 365),
  max_stay integer check (max_stay between 1 and 365),
  closed_to_arrival boolean not null default false,
  closed_to_departure boolean not null default false,
  primary key (rate_plan_id, date, room_type_id),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade,
  foreign key (property_id, rate_plan_id, room_type_id)
    references rate_plan_room_type (property_id, rate_plan_id, room_type_id) on delete cascade,
  check (min_stay is null or max_stay is null or min_stay <= max_stay)
);
create index rate_restriction_property_date_idx on rate_restriction (property_id, date);

-- Per person per night on top of the room price, for [lower(valid), upper(valid)); no upper bound means
-- until further notice. Room only (RO) is always 0, so it has no rows.
create table meal_supplement (
  id uuid primary key,
  tenant_id uuid not null,
  property_id uuid not null,
  meal_plan text not null check (meal_plan in ('BB', 'HB', 'FB')),
  currency char(3) not null check (currency ~ '^[A-Z]{3}$'),
  adult_amount bigint not null check (adult_amount between 0 and 100000000000),
  child_amount bigint not null check (child_amount between 0 and 100000000000),
  valid daterange not null check (not isempty(valid) and not lower_inf(valid)),
  version integer not null default 1,
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade,
  constraint meal_supplement_no_overlap
    exclude using gist (property_id with =, meal_plan with =, currency with =, valid with &&)
);

do $$
declare t text;
begin
  foreach t in array array['cancellation_policy', 'rate_plan', 'rate_plan_room_type', 'rate_day', 'rate_restriction',
                           'meal_supplement'] loop
    execute format('alter table %I enable row level security', t);
    execute format('alter table %I force row level security', t);
    execute format(
      'create policy tenant_isolation on %I for all using (tenant_id = (select app.current_tenant())) with check (tenant_id = (select app.current_tenant()))',
      t);
  end loop;
end $$;
