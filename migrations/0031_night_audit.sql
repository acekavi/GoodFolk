-- Phase 7b: night audit. A property's audit time, the business dates statistics count a booking and a cancellation
-- on, the audit runs and their steps, the daily statistics snapshot, ops alerts, and jobs-svc's own state.

alter table property add column audit_time time not null default '03:00';

-- "On D" for statistics: the business date a reservation was booked, and the one a room was cancelled or marked a
-- no-show on. Timestamps can't say it: a business date is not a calendar date.
alter table reservation add column booked_business_date date;
alter table reservation_room add column cancelled_business_date date;

-- `reservation` and `reservation_room` have FORCE ROW LEVEL SECURITY, which applies to the migration owner too and no
-- tenant is set here, so lift it for the backfill, as 0020 and 0027 do.
alter table reservation no force row level security;
alter table reservation_room no force row level security;
-- backfill: start
update reservation r set booked_business_date = (r.created_at at time zone p.timezone)::date
  from property p where p.id = r.property_id;
update reservation_room rr set cancelled_business_date = (rr.cancelled_at at time zone p.timezone)::date
  from property p where p.id = rr.property_id and rr.status = 'cancelled';
update reservation_room set cancelled_business_date = lower(stay) where status = 'no_show';
-- backfill: end
alter table reservation force row level security;
alter table reservation_room force row level security;
alter table reservation alter column booked_business_date set not null;
alter table reservation_room add constraint reservation_room_cancelled_business_date_check
  check ((status in ('cancelled', 'no_show')) = (cancelled_business_date is not null));

create table audit_run (
  id uuid primary key,
  tenant_id uuid not null,
  property_id uuid not null,
  business_date date not null,
  mode text not null check (mode in ('manual', 'auto')),
  status text not null check (status in ('running', 'failed', 'completed')),
  started_by uuid references app_user (id) on delete set null,
  attempts integer not null default 1 check (attempts between 1 and 1000),
  error text check (length(error) <= 2000),
  -- The failed step's report: {"step": ..., "stays": [{"reservation_room": ..., "room": ..., "error": ...}]}.
  failure jsonb,
  started_at timestamptz not null default now(),
  finished_at timestamptz,
  unique (property_id, business_date),
  unique (tenant_id, id),
  check ((status = 'running') = (finished_at is null)),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade
);

create table audit_run_step (
  tenant_id uuid not null,
  run_id uuid not null,
  step text not null
    check (step in ('resolve', 'post_charges', 'lock_fx', 'snapshot_stats', 'advance_business_date')),
  completed_at timestamptz not null default now(),
  detail jsonb not null default '{}',
  primary key (run_id, step),
  foreign key (tenant_id, run_id) references audit_run (tenant_id, id) on delete cascade
);

create table daily_stats (
  tenant_id uuid not null,
  property_id uuid not null,
  business_date date not null,
  rooms_total integer not null, rooms_out_of_order integer not null, rooms_available integer not null,
  rooms_sold integer not null, complimentary_rooms integer not null, occupancy_bp integer not null,
  adults integer not null, children integer not null, arrivals integer not null, departures integer not null,
  no_shows integer not null, cancellations integer not null, walk_ins integer not null,
  room_revenue bigint not null, meal_plan_revenue bigint not null, pos_revenue bigint not null,
  fee_revenue bigint not null, other_revenue bigint not null, adr bigint not null, revpar bigint not null,
  taxes jsonb not null, payments jsonb not null,
  taken_at timestamptz not null default now(),
  primary key (property_id, business_date),
  unique (tenant_id, property_id, business_date),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade
);

create table daily_stats_breakdown (
  tenant_id uuid not null,
  property_id uuid not null,
  business_date date not null,
  dimension text not null check (dimension in ('segment', 'source', 'residency', 'room_type', 'outlet')),
  key text not null check (length(key) between 1 and 100),
  room_nights integer not null,
  room_revenue bigint not null,
  revenue bigint not null,
  primary key (property_id, business_date, dimension, key),
  foreign key (tenant_id, property_id, business_date)
    references daily_stats (tenant_id, property_id, business_date) on delete cascade
);

create table ops_alert (
  tenant_id uuid not null,
  property_id uuid not null,
  kind text not null check (kind in ('audit', 'drift')),
  message text not null check (length(message) between 1 and 2000),
  raised_at timestamptz not null default now(),
  primary key (property_id, kind),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade
);

-- jobs-svc's memory between ticks: when a job (per key: a property, or '' for a global job) last succeeded.
-- No tenant data, so no RLS.
create table job_state (
  job text not null check (job in ('fx_import', 'media_sweep', 'window', 'cleanup', 'drift')),
  key text not null,
  done_for date,
  last_ok_at timestamptz,
  last_error text check (length(last_error) <= 2000),
  primary key (job, key)
);

create index audit_run_property_date_idx on audit_run (property_id, business_date desc);
create index audit_run_open_idx on audit_run (status) where status <> 'completed';

do $$
declare t text;
begin
  foreach t in array array['audit_run', 'audit_run_step', 'daily_stats', 'daily_stats_breakdown', 'ops_alert'] loop
    execute format('alter table %I enable row level security', t);
    execute format('alter table %I force row level security', t);
    execute format(
      'create policy tenant_isolation on %I for all using (tenant_id = (select app.current_tenant())) with check (tenant_id = (select app.current_tenant()))',
      t);
  end loop;
end $$;

-- The default privileges in 0001 grant every new table its full set to goodfolk_app, so the revokes come after the
-- creates. A run only moves forward (its date and mode never change); steps and statistics are append-only.
revoke all on audit_run, audit_run_step, daily_stats, daily_stats_breakdown, ops_alert, job_state from goodfolk_app;
grant select, insert on audit_run, audit_run_step, daily_stats, daily_stats_breakdown to goodfolk_app;
grant update (status, attempts, error, failure, finished_at) on audit_run to goodfolk_app;
grant select, insert, update, delete on ops_alert to goodfolk_app;
grant select, insert, update on job_state to goodfolk_app;

-- jobs-svc has no tenant. Forced row-level security applies to the table owner too, and on Cloud SQL the owner is
-- neither a superuser nor BYPASSRLS, so the security definer functions below see no rows without these read-only
-- policies for the owner (the role running this migration), as with fx_import_read in 0022, which covers `property`.
-- goodfolk_app is never a member of the owner (db::assert_rls_applies), so the API gains nothing from them.
create policy jobs_read on audit_run for select to current_user using (true);
create policy purge_idempotency on idempotency_key for delete to current_user using (true);

create function app.jobs_properties()
returns table (tenant_id uuid, property_id uuid, business_date date, timezone text, audit_time time)
language sql stable security definer set search_path = pg_catalog, public, pg_temp
as $$ select p.tenant_id, p.id, p.business_date, p.timezone, p.audit_time from property p order by p.id $$;

-- Runs a tick must look at: running (to resume) or failed (to retry).
create function app.jobs_open_runs()
returns table (tenant_id uuid, run_id uuid, property_id uuid, status text, mode text, attempts integer)
language sql stable security definer set search_path = pg_catalog, public, pg_temp
as $$ select r.tenant_id, r.id, r.property_id, r.status, r.mode, r.attempts from audit_run r
      where r.status <> 'completed' order by r.id $$;

create function app.purge_expired(login_window_secs integer, idempotency_days integer)
returns table (sessions bigint, login_failures bigint, idempotency_keys bigint)
language plpgsql volatile security definer set search_path = pg_catalog, public, pg_temp
as $$
declare s bigint; l bigint; i bigint;
begin
  delete from session where expires_at < now(); get diagnostics s = row_count;
  delete from login_failure where at < now() - make_interval(secs => login_window_secs); get diagnostics l = row_count;
  delete from idempotency_key where created_at < now() - make_interval(days => idempotency_days);
  get diagnostics i = row_count;
  return query select s, l, i;
end $$;

revoke all on function app.jobs_properties(), app.jobs_open_runs(), app.purge_expired(integer, integer) from public;
grant execute on function app.jobs_properties(), app.jobs_open_runs(), app.purge_expired(integer, integer)
  to goodfolk_app;
