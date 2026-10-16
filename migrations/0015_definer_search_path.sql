-- Security definer functions must not search the session's temporary schema. With `search_path = pg_catalog, public`
-- Postgres still searches pg_temp first for relations, so a goodfolk_app session could create a temporary table named
-- `outbox` (or `guest_search`) with a trigger and have the function, running as the migration owner, write to it and
-- fire the trigger with the owner's privileges. Listing pg_temp last makes the real tables win. 0009 and 0014 are
-- already applied, so this fixes their functions here instead of editing them.
alter function app.sync_guest_search() set search_path = pg_catalog, public, pg_temp;
alter function app.search_guest_ids(text, int) set search_path = pg_catalog, public, pg_temp;
alter function app.outbox_enqueue(uuid, text, jsonb) set search_path = pg_catalog, public, pg_temp;
alter function app.outbox_claim(int, int) set search_path = pg_catalog, public, pg_temp;
alter function app.outbox_sent(uuid) set search_path = pg_catalog, public, pg_temp;
alter function app.outbox_retry(uuid, text, int) set search_path = pg_catalog, public, pg_temp;
alter function app.event_first_seen(text) set search_path = pg_catalog, public, pg_temp;

-- A message the receiver will never accept: finished, with the reason kept. One statement, so a crash cannot leave
-- the row sent without its error.
create function app.outbox_refused(p_id uuid, p_error text) returns void
language sql security definer set search_path = pg_catalog, public, pg_temp
as $$ update outbox set sent_at = now(), last_error = left(p_error, 500) where id = p_id $$;
revoke all on function app.outbox_refused(uuid, text) from public;
grant execute on function app.outbox_refused(uuid, text) to goodfolk_app;
