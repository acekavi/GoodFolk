-- Phase 7d: reports and liability reports. The property's TDL registration number and VAT return period, each
-- outlet's report group with its history, the reports themselves, the weekly and monthly email preferences, and the
-- exactly-once record of each email enqueued. Audit runs gain the `render_reports` step and ops alerts the `reports`
-- kind.

alter table property add column tdl_registration_no text check (length(tdl_registration_no) between 1 and 30);
alter table property add column vat_return_period text not null default 'quarterly'
  check (vat_return_period in ('monthly', 'quarterly'));

-- Which TDL line an outlet's sales fall under, from a date on. A change inserts a row effective from the property's
-- business date, so a closed day keeps the group it was reported under.
create table outlet_report_group (
  tenant_id uuid not null,
  outlet_id uuid not null,
  report_group text not null check (report_group in ('food', 'bar', 'other')),
  effective_from date not null,
  primary key (outlet_id, effective_from),
  foreign key (tenant_id, outlet_id) references outlet (tenant_id, id) on delete cascade
);

insert into outlet_report_group (tenant_id, outlet_id, report_group, effective_from)
select tenant_id, id, 'food', date '2000-01-01' from outlet;

create table report (
  id uuid primary key,
  tenant_id uuid not null,
  property_id uuid not null,
  kind text not null check (kind in ('weekly', 'monthly', 'tdl', 'sscl', 'vat', 'service_charge', 'on_demand')),
  period_start date not null,
  period_end date not null,
  data jsonb not null,
  status text not null check (status in ('rendering', 'ready', 'failed')),
  pdf_key text,
  error text check (length(error) <= 2000),
  created_at timestamptz not null default now(),
  rendered_at timestamptz,
  check (period_end >= period_start),
  unique (tenant_id, id),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade
);
create unique index report_period_idx on report (property_id, kind, period_start) where kind <> 'on_demand';

-- No row means the email is on.
create table report_mail_pref (
  tenant_id uuid not null,
  user_id uuid not null references app_user (id) on delete cascade,
  property_id uuid not null,
  kind text not null check (kind in ('weekly', 'monthly')),
  enabled boolean not null,
  primary key (user_id, property_id, kind),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade
);

create table report_mail (
  tenant_id uuid not null,
  report_id uuid not null,
  user_id uuid not null references app_user (id) on delete cascade,
  primary key (report_id, user_id),
  foreign key (tenant_id, report_id) references report (tenant_id, id) on delete cascade
);

alter table audit_run_step drop constraint audit_run_step_step_check;
alter table audit_run_step add constraint audit_run_step_step_check
  check (step in ('resolve', 'post_charges', 'lock_fx', 'snapshot_stats', 'advance_business_date', 'render_reports'));

alter table ops_alert drop constraint ops_alert_kind_check;
alter table ops_alert add constraint ops_alert_kind_check check (kind in ('audit', 'drift', 'linen', 'reports'));

do $$
declare t text;
begin
  foreach t in array array['outlet_report_group', 'report', 'report_mail_pref', 'report_mail'] loop
    execute format('alter table %I enable row level security', t);
    execute format('alter table %I force row level security', t);
    execute format(
      'create policy tenant_isolation on %I for all using (tenant_id = (select app.current_tenant())) with check (tenant_id = (select app.current_tenant()))',
      t);
  end loop;
end $$;

-- The default privileges in 0001 grant every new table its full set to goodfolk_app, so the revokes come after the
-- creates. A mail record is append-only; a report is rendered in place and may be replaced.
revoke all on outlet_report_group, report, report_mail_pref, report_mail from goodfolk_app;
grant select, insert, update on outlet_report_group to goodfolk_app;
grant select, insert, update, delete on report, report_mail_pref to goodfolk_app;
grant select, insert on report_mail to goodfolk_app;
