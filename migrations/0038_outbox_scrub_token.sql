-- Phase 8a: a `mail-send` row carries an invitation's token (`{"Invitation": {..., "token": ...}}`) to jobs-svc, the
-- one place it is kept. Once the row is delivered or refused the token is no longer needed, so both of the relay's
-- finishing functions drop it in the same statement: it then lives only in an undelivered row, for seconds. Other
-- topics are untouched. `create or replace` keeps each function's grants.
create or replace function app.outbox_sent(p_id uuid) returns void
language sql security definer set search_path = pg_catalog, public, pg_temp
as $$
  update outbox set sent_at = now(), last_error = null,
    payload = case when topic = 'mail-send' then payload #- '{Invitation,token}' else payload end
  where id = p_id
$$;

create or replace function app.outbox_refused(p_id uuid, p_error text) returns void
language sql security definer set search_path = pg_catalog, public, pg_temp
as $$
  update outbox set sent_at = now(), last_error = left(p_error, 500),
    payload = case when topic = 'mail-send' then payload #- '{Invitation,token}' else payload end
  where id = p_id
$$;
