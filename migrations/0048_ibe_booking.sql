-- Phase 9: a booking site's confirmation `mail-send` row carries the booking's manage-link token
-- (`{"IbeConfirmation": {..., "token": ...}}`) to jobs-svc, as an invitation's does (0038). Both finishing functions
-- of the relay now drop either token in the same statement, so it lives only in an undelivered row. Other topics are
-- untouched. `create or replace` keeps each function's grants.
create or replace function app.outbox_sent(p_id uuid) returns void
language sql security definer set search_path = pg_catalog, public, pg_temp
as $$
  update outbox set sent_at = now(), last_error = null,
    payload = case when topic = 'mail-send' then payload #- '{Invitation,token}' #- '{IbeConfirmation,token}'
                   else payload end
  where id = p_id
$$;

create or replace function app.outbox_refused(p_id uuid, p_error text) returns void
language sql security definer set search_path = pg_catalog, public, pg_temp
as $$
  update outbox set sent_at = now(), last_error = left(p_error, 500),
    payload = case when topic = 'mail-send' then payload #- '{Invitation,token}' #- '{IbeConfirmation,token}'
                   else payload end
  where id = p_id
$$;
