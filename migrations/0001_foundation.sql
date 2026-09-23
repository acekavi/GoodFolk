create extension if not exists citext;
create schema if not exists app;

-- The app role is normally created by the environment (deploy/dev/postgres-init.sql,
-- or by hand on Neon). Creating it here keeps a bare database usable.
do $$
begin
  create role goodfolk_app nologin;
exception when duplicate_object then null;
end $$;

grant usage on schema public, app to goodfolk_app;

-- Tenant context is set per transaction with set_config(..., true).
-- STABLE functions wrapped in (select ...) inside policies are evaluated once per query.
create function app.current_tenant() returns uuid language sql stable
as $$ select nullif(current_setting('app.tenant_id', true), '')::uuid $$;

create function app.current_user_id() returns uuid language sql stable
as $$ select nullif(current_setting('app.user_id', true), '')::uuid $$;

create table tenant (
  id uuid primary key,
  name text not null check (length(name) between 1 and 200),
  created_at timestamptz not null default now()
);

create table app_user (
  id uuid primary key,
  email citext not null unique,
  password_hash text not null,
  display_name text not null check (length(display_name) between 1 and 200),
  created_at timestamptz not null default now()
);

create table membership (
  tenant_id uuid not null references tenant(id) on delete cascade,
  user_id uuid not null references app_user(id) on delete cascade,
  created_at timestamptz not null default now(),
  primary key (tenant_id, user_id)
);
create index membership_user_idx on membership(user_id);

create table property (
  id uuid primary key,
  tenant_id uuid not null references tenant(id) on delete cascade,
  code text not null check (code ~ '^[A-Z0-9]{2,10}$'),
  name text not null check (length(name) between 1 and 200),
  timezone text not null,
  base_currency char(3) not null check (base_currency ~ '^[A-Z]{3}$'),
  version integer not null default 1,
  created_at timestamptz not null default now(),
  unique (tenant_id, code)
);

create table role_grant (
  id uuid primary key,
  tenant_id uuid not null,
  user_id uuid not null,
  property_id uuid references property(id) on delete cascade,
  role text not null check (role in ('owner', 'manager', 'front_desk', 'housekeeping', 'accountant')),
  foreign key (tenant_id, user_id) references membership(tenant_id, user_id) on delete cascade,
  unique nulls not distinct (tenant_id, user_id, property_id, role)
);
create index role_grant_user_idx on role_grant(tenant_id, user_id);

-- Looked up by token before any tenant is known, so no RLS. active_tenant_id is the tenant the
-- user is working in, not an owner: only tables whose rows belong to a tenant use `tenant_id`.
create table session (
  id uuid primary key,
  token_hash bytea not null unique,
  user_id uuid not null references app_user(id) on delete cascade,
  active_tenant_id uuid references tenant(id) on delete set null,
  created_at timestamptz not null default now(),
  expires_at timestamptz not null
);
create index session_user_idx on session(user_id);

create table audit_log (
  id uuid primary key,
  tenant_id uuid not null references tenant(id) on delete cascade,
  actor_user_id uuid references app_user(id) on delete set null,
  action text not null,
  entity text not null,
  entity_id uuid,
  data jsonb not null default '{}',
  at timestamptz not null default now()
);
create index audit_log_tenant_at_idx on audit_log(tenant_id, at desc);

create table idempotency_key (
  tenant_id uuid not null references tenant(id) on delete cascade,
  key text not null check (length(key) between 8 and 200),
  request_hash bytea not null,
  status_code smallint,
  response_body bytea,
  created_at timestamptz not null default now(),
  primary key (tenant_id, key)
);

-- Row-level security. app_user and session are global identity tables (no tenant column).
alter table tenant enable row level security;
alter table tenant force row level security;
create policy tenant_read on tenant for select using (
  id = (select app.current_tenant())
  or id in (select tenant_id from membership where user_id = (select app.current_user_id()))
);
create policy tenant_write on tenant for all
  using (id = (select app.current_tenant()))
  with check (id = (select app.current_tenant()));

alter table membership enable row level security;
alter table membership force row level security;
create policy membership_read on membership for select using (
  tenant_id = (select app.current_tenant()) or user_id = (select app.current_user_id())
);
create policy membership_write on membership for all
  using (tenant_id = (select app.current_tenant()))
  with check (tenant_id = (select app.current_tenant()));

do $$
declare t text;
begin
  foreach t in array array['property', 'role_grant', 'audit_log', 'idempotency_key'] loop
    execute format('alter table %I enable row level security', t);
    execute format('alter table %I force row level security', t);
    execute format(
      'create policy tenant_isolation on %I for all using (tenant_id = (select app.current_tenant())) with check (tenant_id = (select app.current_tenant()))',
      t);
  end loop;
end $$;

grant select, insert, update, delete on all tables in schema public to goodfolk_app;
revoke all on _sqlx_migrations from goodfolk_app;
revoke update, delete on audit_log from goodfolk_app;
grant execute on all functions in schema app to goodfolk_app;
alter default privileges in schema public grant select, insert, update, delete on tables to goodfolk_app;
