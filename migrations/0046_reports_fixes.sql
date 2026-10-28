-- Phase 7d, before merge: the outlets 0041 could not give a report group, a report key that tells a VAT month from
-- the quarter beginning the same day, and the cleanup of reports rendered on demand.

-- 0041 gave each outlet its `food` group reading `outlet`, which has FORCE ROW LEVEL SECURITY: an owner that is
-- neither a superuser nor BYPASSRLS read no outlets there. Give every outlet without a group its `food` one now.
-- Both tables are forced, so lift it for the backfill, as 0027 and 0031 do.
-- backfill: start
alter table outlet no force row level security;
alter table outlet_report_group no force row level security;
insert into outlet_report_group (tenant_id, outlet_id, report_group, effective_from)
select o.tenant_id, o.id, 'food', date '2000-01-01' from outlet o
where not exists (select 1 from outlet_report_group g where g.outlet_id = o.id)
on conflict do nothing;
alter table outlet force row level security;
alter table outlet_report_group force row level security;
-- backfill: end

-- A monthly VAT return for July and the Q3 return both start on 1 July, so a property that changes its return period
-- keeps both: a report is the property's, kind's and period's, start and end.
drop index report_period_idx;
create unique index report_period_idx on report (property_id, kind, period_start, period_end)
  where kind <> 'on_demand';

-- jobs-svc's daily cleanup deletes each property's reports rendered on demand once they are 30 days old.
create index report_on_demand_idx on report (property_id, created_at) where kind = 'on_demand';
