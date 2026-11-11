-- Security fix L2: the definers that read or purge across every tenant are jobs-svc's alone.
--
-- `goodfolk_jobs` is jobs-svc's group role: a member of `goodfolk_app`, so it has everything the API role has, under the
-- same row-level security, plus execute on these functions, which `goodfolk_app` loses. core-api connects as a role in
-- `goodfolk_app` only, so an injection there could no longer list every tenant's properties and runs, purge their
-- sessions, or sweep their media. The environment normally creates the role with its login (deploy/dev/postgres-init.sql,
-- or by hand in production, as `goodfolk_app`); creating it here keeps a bare database usable.
--
-- The outbox's definers stay `goodfolk_app`'s: core-api runs the relay.
do $$
begin
  create role goodfolk_jobs nologin;
exception when duplicate_object or unique_violation then null;
end $$;
grant goodfolk_app to goodfolk_jobs;

revoke execute on function app.jobs_properties(), app.jobs_open_runs(), app.jobs_unrendered_runs(),
  app.purge_expired(integer, integer), app.fx_import_targets(), app.media_sweep_due(), app.purge_events()
  from goodfolk_app;
grant execute on function app.jobs_properties(), app.jobs_open_runs(), app.jobs_unrendered_runs(),
  app.purge_expired(integer, integer), app.fx_import_targets(), app.media_sweep_due(), app.purge_events()
  to goodfolk_jobs;
