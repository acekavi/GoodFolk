-- Phase 7a: the daily exchange-rate import has no tenant, so it needs the list of properties and the currencies each
-- one accepts across tenants. Each property's rates are then written in its own tenant-scoped transaction.

-- Forced row-level security applies to the table owner too, and on Cloud SQL the owner is neither a superuser nor
-- BYPASSRLS, so the security definer function below would see no properties without this read-only policy for the
-- owner (the role running this migration), as with sweep_read in 0016. goodfolk_app is never a member of the owner
-- (db::assert_rls_applies), so the API gains nothing from it.
create policy fx_import_read on property for select to current_user using (true);

create function app.fx_import_targets()
returns table (tenant_id uuid, property_id uuid, base_currency text, accepted_currencies text[])
language sql stable security definer set search_path = pg_catalog, public, pg_temp
as $$
  select p.tenant_id, p.id, p.base_currency::text, p.accepted_currencies::text[] from property p
$$;

revoke all on function app.fx_import_targets() from public;
grant execute on function app.fx_import_targets() to goodfolk_app;
