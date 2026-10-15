-- Phase 6: the transactional outbox and receiver deduplication. Not tenant data: only the relay and receivers use
-- them, through the security definer functions below; goodfolk_app has no grants on the tables.
create table outbox (
  id uuid primary key,
  topic text not null check (topic ~ '^[a-z][a-z0-9-]{1,62}$'),
  payload jsonb not null,
  created_at timestamptz not null default now(),
  attempts integer not null default 0,
  next_attempt_at timestamptz not null default now(),
  sent_at timestamptz,
  last_error text
);
create index outbox_due_idx on outbox (next_attempt_at) where sent_at is null;
create table processed_event (message_id text primary key, processed_at timestamptz not null default now());
revoke all on outbox, processed_event from public;
revoke all on outbox, processed_event from goodfolk_app;

create function app.outbox_enqueue(p_id uuid, p_topic text, p_payload jsonb) returns void
language sql security definer set search_path = pg_catalog, public
as $$ insert into outbox (id, topic, payload) values (p_id, p_topic, p_payload) $$;

-- Claims due rows for `lease` seconds: a relay that dies mid-send leaves them due again after the lease.
create function app.outbox_claim(max_rows int, lease_seconds int)
returns table (id uuid, topic text, payload jsonb, attempts int)
language sql security definer set search_path = pg_catalog, public
as $$
  update outbox o set attempts = o.attempts + 1, next_attempt_at = now() + make_interval(secs => lease_seconds)
  where o.id in (select id from outbox where sent_at is null and next_attempt_at <= now()
                 order by next_attempt_at limit least(greatest(max_rows, 1), 100) for update skip locked)
  returning o.id, o.topic, o.payload, o.attempts
$$;
create function app.outbox_sent(p_id uuid) returns void
language sql security definer set search_path = pg_catalog, public
as $$ update outbox set sent_at = now(), last_error = null where id = p_id $$;
create function app.outbox_retry(p_id uuid, p_error text, p_delay_seconds int) returns void
language sql security definer set search_path = pg_catalog, public
as $$ update outbox set last_error = left(p_error, 500), next_attempt_at = now() + make_interval(secs => p_delay_seconds) where id = p_id $$;
-- True the first time a message id is seen. Call inside the receiver's transaction so a rollback forgets it.
create function app.event_first_seen(p_message_id text) returns boolean
language sql security definer set search_path = pg_catalog, public
as $$ with ins as (insert into processed_event (message_id) values (p_message_id) on conflict do nothing returning 1)
      select exists (select 1 from ins) $$;

revoke all on function app.outbox_enqueue(uuid, text, jsonb), app.outbox_claim(int, int), app.outbox_sent(uuid),
  app.outbox_retry(uuid, text, int), app.event_first_seen(text) from public;
grant execute on function app.outbox_enqueue(uuid, text, jsonb), app.outbox_claim(int, int), app.outbox_sent(uuid),
  app.outbox_retry(uuid, text, int), app.event_first_seen(text) to goodfolk_app;
