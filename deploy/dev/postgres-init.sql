-- Runs once when the dev/CI Postgres container is first created.
-- goodfolk_app: NOLOGIN group role that owns no tables; RLS applies to it.
-- goodfolk_api: the login role the API connects as.
create role goodfolk_app nologin;
create role goodfolk_api login password 'goodfolk_api_dev' in role goodfolk_app;
