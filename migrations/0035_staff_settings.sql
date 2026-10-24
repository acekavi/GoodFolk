-- Phase 8a: staff invitations, a POS role, membership versions, per-rate-plan deposit rules and the SLTDA licence.

alter table role_grant drop constraint role_grant_role_check;
alter table role_grant add constraint role_grant_role_check
  check (role in ('owner', 'manager', 'front_desk', 'housekeeping', 'accountant', 'hk_supervisor', 'pos'));

-- Bumped on every change to a member's grants, so a stale edit is refused.
alter table membership add column version integer not null default 1;

create table invitation (
  id uuid primary key,
  tenant_id uuid not null references tenant (id) on delete cascade,
  email citext not null,
  display_name text not null check (length(display_name) between 1 and 200),
  grants jsonb not null check (jsonb_typeof(grants) = 'array' and jsonb_array_length(grants) between 1 and 50),
  token_hash bytea not null unique,
  status text not null check (status in ('pending', 'accepted', 'revoked', 'expired')),
  invited_by uuid references app_user (id) on delete set null,
  created_at timestamptz not null default now(),
  expires_at timestamptz not null,
  accepted_at timestamptz, accepted_user_id uuid references app_user (id) on delete set null,
  check ((status = 'accepted') = (accepted_at is not null))
);
create unique index invitation_one_pending_idx on invitation (tenant_id, email) where status = 'pending';

alter table property add column sltda_licence text check (length(sltda_licence) between 1 and 50);
alter table rate_plan add column deposit_rule jsonb check (jsonb_typeof(deposit_rule) = 'object');
alter table reservation_room add column deposit_terms jsonb check (jsonb_typeof(deposit_terms) = 'object');

alter table invitation enable row level security;
alter table invitation force row level security;
create policy tenant_isolation on invitation for all
  using (tenant_id = (select app.current_tenant())) with check (tenant_id = (select app.current_tenant()));

-- The default privileges in 0001 grant every new table its full set to goodfolk_app, so the revoke comes after the
-- create. An invitation is never deleted: it is revoked, accepted or expired.
revoke all on invitation from goodfolk_app;
grant select, insert, update on invitation to goodfolk_app;

-- Preview and accept find an invitation by its token before any tenant is known. Forced row-level security applies to
-- the table owner too, and on Cloud SQL the owner is neither a superuser nor BYPASSRLS, so the security definer
-- functions below see no rows without these policies for the owner (the role running this migration), as with
-- fx_import_read in 0022 and jobs_read in 0031. They cover the invitation (read and the accepting update) and the
-- tenant's name. goodfolk_app is never a member of the owner (db::assert_rls_applies), so the API gains nothing from
-- them.
create policy invitation_owner_read on invitation for select to current_user using (true);
create policy invitation_owner_accept on invitation for update to current_user using (true) with check (true);
create policy invitation_tenant_read on tenant for select to current_user using (true);

create function app.invitation_by_token(hash bytea)
returns table (id uuid, tenant_id uuid, email citext, display_name text, grants jsonb, status text,
               expires_at timestamptz, tenant_name text, inviter_name text)
language sql stable security definer set search_path = pg_catalog, public, pg_temp
as $$
  select i.id, i.tenant_id, i.email, i.display_name, i.grants, i.status, i.expires_at, t.name, u.display_name
  from invitation i
  join tenant t on t.id = i.tenant_id
  left join app_user u on u.id = i.invited_by
  where i.token_hash = hash
$$;

-- jobs-svc's mail endpoint knows the invitation, not its token.
create function app.invitation_by_id(id uuid)
returns table (tenant_id uuid, email citext, display_name text, grants jsonb, status text, expires_at timestamptz,
               tenant_name text, inviter_name text)
language sql stable security definer set search_path = pg_catalog, public, pg_temp
as $$
  select i.tenant_id, i.email, i.display_name, i.grants, i.status, i.expires_at, t.name, u.display_name
  from invitation i
  join tenant t on t.id = i.tenant_id
  left join app_user u on u.id = i.invited_by
  where i.id = invitation_by_id.id
$$;

create function app.accept_invitation_row(id uuid, user_id uuid)
returns boolean
language plpgsql volatile security definer set search_path = pg_catalog, public, pg_temp
as $$
declare n bigint;
begin
  update invitation i set status = 'accepted', accepted_at = now(), accepted_user_id = user_id
    where i.id = accept_invitation_row.id and i.status = 'pending' and i.expires_at > now();
  get diagnostics n = row_count;
  return n = 1;
end $$;

revoke all on function app.invitation_by_token(bytea), app.invitation_by_id(uuid),
  app.accept_invitation_row(uuid, uuid) from public;
grant execute on function app.invitation_by_token(bytea), app.invitation_by_id(uuid),
  app.accept_invitation_row(uuid, uuid) to goodfolk_app;
