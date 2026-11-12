-- Runs once when the dev/CI Postgres container is first created.
-- goodfolk_app: NOLOGIN group role that owns no tables; RLS applies to it.
-- goodfolk_api: the login role the API connects as.
-- goodfolk_jobs: NOLOGIN group role for jobs-svc: goodfolk_app's rights plus the cross-tenant job functions
--   (migration 0050).
-- goodfolk_jobs_svc: the login role jobs-svc connects as.
create role goodfolk_app nologin;
create role goodfolk_api login password 'goodfolk_api_dev' in role goodfolk_app;
create role goodfolk_jobs nologin in role goodfolk_app;
create role goodfolk_jobs_svc login password 'goodfolk_jobs_svc_dev' in role goodfolk_jobs;
