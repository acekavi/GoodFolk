-- Phase 7d: the close's `render_reports` step runs after the business date has advanced, so a run can be completed
-- without it when queuing the reports failed. A tick queues them again for each property's latest completed run that
-- lacks the step: the reports of every period closed through it, so an older run never needs it.

-- As for `audit_run` in 0031: forced row-level security applies to the owner too, so the definer below needs it.
create policy jobs_read on audit_run_step for select to current_user using (true);

create function app.jobs_unrendered_runs()
returns table (tenant_id uuid, run_id uuid, property_id uuid)
language sql stable security definer set search_path = pg_catalog, public, pg_temp
as $$ select r.tenant_id, r.id, r.property_id
      from (select distinct on (property_id) tenant_id, id, property_id from audit_run
            where status = 'completed' order by property_id, business_date desc) r
      where not exists (select 1 from audit_run_step s where s.run_id = r.id and s.step = 'render_reports')
      order by r.id $$;

revoke all on function app.jobs_unrendered_runs() from public;
grant execute on function app.jobs_unrendered_runs() to goodfolk_app;
