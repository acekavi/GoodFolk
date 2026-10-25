-- Phase 8a: the invitation definers honour the caller's tenant, and accepting goes through row-level security.
--
-- `app.invitation_by_id` (jobs-svc's mail endpoint) now finds only an invitation of the transaction's tenant, and
-- also returns the token's hash, so a job carrying a token that a resend has since replaced sends nothing. Its return
-- type changes, so it is dropped and created again with its grants.
drop function app.invitation_by_id(uuid);
create function app.invitation_by_id(id uuid)
returns table (tenant_id uuid, email citext, display_name text, grants jsonb, status text, expires_at timestamptz,
               tenant_name text, inviter_name text, token_hash bytea)
language sql stable security definer set search_path = pg_catalog, public, pg_temp
as $$
  select i.tenant_id, i.email, i.display_name, i.grants, i.status, i.expires_at, t.name, u.display_name, i.token_hash
  from invitation i
  join tenant t on t.id = i.tenant_id
  left join app_user u on u.id = i.invited_by
  where i.id = invitation_by_id.id and i.tenant_id = (select app.current_tenant())
$$;
revoke all on function app.invitation_by_id(uuid) from public;
grant execute on function app.invitation_by_id(uuid) to goodfolk_app;

-- Accept marks the invitation accepted with a plain update in its own tenant-scoped transaction, under
-- row-level security, so the definer and the owner's update policy it needed are gone.
drop function app.accept_invitation_row(uuid, uuid);
drop policy invitation_owner_accept on invitation;
