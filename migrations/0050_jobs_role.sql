-- Security fix L2: the definers that read or purge across every tenant are jobs-svc's alone.
--
-- `goodfolk_jobs` is jobs-svc's group role: a member of `goodfolk_app`, so it has everything the API role has, under the
-- same row-level security, plus execute on these functions, which `goodfolk_app` loses. core-api connects as a role in
-- `goodfolk_app` only, so an injection there could no longer list every tenant's properties and runs, purge their
-- sessions, or sweep their media. The outbox's definers stay `goodfolk_app`'s: core-api runs the relay.
--
-- Roles are an operations step, as `goodfolk_app` and `goodfolk_api` are: deploy/dev/postgres-init.sql creates them
-- for development and CI, and production creates them by hand before this migration runs (README "Deploying", which
-- also gives the rollout order that keeps a running jobs-svc working). This migration only moves the grants, and stops
-- with a clear message when the role is missing.
do $$
begin
  if not exists (select 1 from pg_roles where rolname = 'goodfolk_jobs') then
    raise exception 'role goodfolk_jobs does not exist: create it first (create role goodfolk_jobs nologin in role goodfolk_app; see README "Deploying")';
  end if;
  if not pg_has_role('goodfolk_jobs', 'goodfolk_app', 'member') then
    raise exception 'role goodfolk_jobs must be a member of goodfolk_app (grant goodfolk_app to goodfolk_jobs; see README "Deploying")';
  end if;
end $$;

revoke execute on function app.jobs_properties(), app.jobs_open_runs(), app.jobs_unrendered_runs(),
  app.purge_expired(integer, integer), app.fx_import_targets(), app.media_sweep_due(), app.purge_events()
  from goodfolk_app;
grant execute on function app.jobs_properties(), app.jobs_open_runs(), app.jobs_unrendered_runs(),
  app.purge_expired(integer, integer), app.fx_import_targets(), app.media_sweep_due(), app.purge_events()
  to goodfolk_jobs;
