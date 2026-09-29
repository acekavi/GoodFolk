-- Guest name search that keeps tenant isolation, without bypassing row-level security.
--
-- The target platform (Cloud SQL) has no superuser and no BYPASSRLS role, and user-defined functions can
-- never be marked LEAKPROOF there. pg_trgm's `<%` (word similarity) is not leakproof, so under forced
-- row-level security Postgres refuses to use it as an index condition: it can only be applied as a filter
-- after the tenant_id qual, which means a plain `select ... from guest where lower($1) <% ...` scans every
-- one of the tenant's guests (see `search_guests`'s old comment, and api-conventions.md before this migration).
--
-- The fix here is NOT to bypass RLS: it is to give the trigram index a table that RLS was never protecting in
-- the first place, and to make the ONLY door into that table a function that enforces the tenant filter
-- itself, in the same way row-level security would have.
--
--   1. `guest_search` is a narrow copy of `guest`: just the id, the tenant and the lowercased full name. It
--      carries NO row-level security at all (there is nothing on it worth protecting on its own -- an id and
--      a name, no email, phone, sealed ID number, or anything else); it belongs to the migration owner, and
--      every privilege on it is revoked from `goodfolk_app` and from `public`, so the application role has no
--      way to read it directly, indexed or not.
--   2. A trigger on `guest` (`SECURITY DEFINER`, owned by the migration owner) keeps it in step: inserting or
--      renaming a guest upserts its row here; deleting a guest removes it.
--   3. `app.search_guest_ids`, also `SECURITY DEFINER` and owned by the migration owner, is the only way to
--      read `guest_search`. It filters by `app.current_tenant()` -- the exact same session setting every
--      row-level security policy in this database trusts, set once per transaction by `db::begin` and never
--      forgeable by the application role -- and returns nothing at all when that setting is unset. It returns
--      guest ids only, never any guest data. The caller (`search_guests`, in `modules/reservations`) then
--      reads the matching guests back from `guest`, where forced row-level security applies exactly as it
--      does everywhere else, so a bug in this function's filter could at worst return an id, never a row of
--      someone else's data.
--
-- Net result: the trigram index is a normal, fully usable GIN index (no leakproof requirement applies to it,
-- because nothing sits between it and the SECURITY DEFINER function that owns the table it indexes), and the
-- application role still never has a byte of guest data it isn't entitled to.
create extension if not exists btree_gin;

-- No RLS. tenant_id + the lowercased full name only: enough to search by, nothing worth protecting on its
-- own. `guest_id` is the primary key (one row per guest, kept in step by the trigger below).
create table guest_search (
  guest_id uuid primary key,
  tenant_id uuid not null,
  name text not null
);

-- `alter default privileges` in 0001 grants every new table in this schema select/insert/update/delete as
-- soon as it is created (it runs as the migration owner, the same role that creates this table), so those
-- grants land on `guest_search` too unless revoked here. Revoke from `public` as well, defensively: nothing
-- should ever be able to read this table except the SECURITY DEFINER functions below, whatever privilege
-- this database's roles pick up in the future.
revoke all on guest_search from public;
revoke all on guest_search from goodfolk_app;

-- btree_gin gives the uuid column a GIN operator class, so `tenant_id` and the trigram-indexed `name` can
-- share one GIN index; a scan of it alone answers the query below, the tenant a plain equality condition, no
-- leakproof requirement standing in the way -- this table's only reader is a SECURITY DEFINER function, not a
-- row-level-security-restricted role.
create index guest_search_tenant_name_idx on guest_search using gin (tenant_id, name gin_trgm_ops);

-- Backfill. `guest` has FORCE ROW LEVEL SECURITY, which applies its policies to the table owner too (the role
-- running this migration), and no tenant is set here, so a plain select would see zero rows; lift FORCE for
-- the moment it takes to copy every tenant's guests, exactly as 0004 does to backfill `property`.
alter table guest no force row level security;
insert into guest_search (guest_id, tenant_id, name)
select id, tenant_id, lower(first_name || ' ' || last_name) from guest;
alter table guest force row level security;

-- Keeps `guest_search` in step with `guest`. SECURITY DEFINER so it runs as the migration owner (the only
-- role with real privileges on `guest_search`) regardless of who inserts, renames or deletes a guest; a fixed
-- search_path keeps it from resolving an unqualified name to an object some other role slipped into a schema
-- earlier in the caller's search_path.
create function app.sync_guest_search() returns trigger
language plpgsql security definer
set search_path = pg_catalog, public
as $$
begin
  if tg_op = 'DELETE' then
    delete from guest_search where guest_id = old.id;
    return old;
  end if;
  insert into guest_search (guest_id, tenant_id, name)
  values (new.id, new.tenant_id, lower(new.first_name || ' ' || new.last_name))
  on conflict (guest_id) do update set tenant_id = excluded.tenant_id, name = excluded.name;
  return new;
end;
$$;

-- One trigger, all three events: a rename is `update of first_name, last_name` (an unrelated update, such as
-- to `notes` or `email`, does not fire this trigger at all), a new guest is `insert`, a removed guest is
-- `delete`. `tenant_id` is included too: `guest`'s row-level security policy already has a `with check` that
-- blocks the application role from changing a row's tenant, so this can't fire under normal operation today --
-- but keeping it here means a future move between tenants (done by a role that can bypass that check) stays
-- correctly reflected in `guest_search` without this trigger needing a second look.
create trigger guest_search_sync
  after insert or update of first_name, last_name, tenant_id or delete on guest
  for each row execute function app.sync_guest_search();

-- The only way `goodfolk_app` can search `guest_search`. SECURITY DEFINER (runs as the migration owner, the
-- table's only reader), STABLE (same inputs, same result within a statement -- it reads nothing else), and a
-- fixed search_path for the same reason as the trigger function above.
--
-- Safety, in one place:
--  - it trusts exactly the setting row-level security itself trusts (`app.current_tenant()`), set once per
--    transaction by `db::begin` and never writable by the application role;
--  - with that setting unset, `app.current_tenant()` is null and this returns no rows at all, the same as
--    every row-level-security policy in this database;
--  - it returns ids only, never a name, email, phone or anything else -- a name search that somehow matched
--    the wrong tenant would leak nothing beyond a guest id, and the caller then reads that id back from
--    `guest`, where forced row-level security applies as usual and would refuse a foreign id outright;
--  - `goodfolk_app` has no privilege on `guest_search` itself (revoked above), so this function is the only
--    door into the table, and the `revoke`/`grant` below are the only way through it.
create function app.search_guest_ids(query text, max_rows int)
returns table (guest_id uuid, score real)
language sql stable security definer
set search_path = pg_catalog, public
as $$
  select guest_id, similarity(lower(query), name) as score
  from guest_search
  where app.current_tenant() is not null
    and tenant_id = app.current_tenant()
    and lower(query) <% name
  order by word_similarity(lower(query), name) desc, similarity(lower(query), name) desc, guest_id
  limit least(greatest(max_rows, 1), 50)
$$;

revoke all on function app.search_guest_ids(text, int) from public;
grant execute on function app.search_guest_ids(text, int) to goodfolk_app;
