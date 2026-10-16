-- The relay wakes when an outbox row commits instead of polling: enqueueing also notifies `gf_outbox`, which Postgres
-- delivers only if the enqueueing transaction commits. Each API instance's LISTEN connection hears it and wakes its
-- relay; an idle relay otherwise only polls every 30 s, so idle compute can suspend. Several enqueues in one
-- transaction send one notification, since Postgres folds identical ones.
create or replace function app.outbox_enqueue(p_id uuid, p_topic text, p_payload jsonb) returns void
language sql security definer set search_path = pg_catalog, public, pg_temp
as $$
  insert into outbox (id, topic, payload) values (p_id, p_topic, p_payload);
  select pg_notify('gf_outbox', '');
$$;
revoke all on function app.outbox_enqueue(uuid, text, jsonb) from public;
grant execute on function app.outbox_enqueue(uuid, text, jsonb) to goodfolk_app;
