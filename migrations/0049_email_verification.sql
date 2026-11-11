-- Security fix M2: an owner who signed up must verify their email before they can invite staff.
--
-- `email_verified_at` is null until the person proves they read the address's mail: by following the verification
-- link sent at sign-up, or by accepting an invitation sent there. Everyone with an account before this migration is
-- treated as verified.
alter table app_user add column email_verified_at timestamptz;
update app_user set email_verified_at = created_at;

-- One row per verification email sent: the link's token is stored only as its SHA-256, as an invitation's is. Only
-- the newest unexpired row of a user verifies; the rows of the last day also count the emails sent, which are
-- limited. Users are global, so this table has no tenant and no row-level security, as `app_user` and `session`.
create table email_verification (
  id uuid primary key,
  user_id uuid not null references app_user(id) on delete cascade,
  token_hash bytea not null unique,
  created_at timestamptz not null default now(),
  expires_at timestamptz not null
);
create index email_verification_user_idx on email_verification (user_id, created_at desc);

-- A verification email's `mail-send` row carries its token too (`{"EmailVerification": {..., "token": ...}}`), so
-- the relay drops it once the row is delivered or refused, as it does an invitation's and a site booking's.
create or replace function app.outbox_sent(p_id uuid) returns void
language sql security definer set search_path = pg_catalog, public, pg_temp
as $$
  update outbox set sent_at = now(), last_error = null,
    payload = case when topic = 'mail-send'
                   then payload #- '{Invitation,token}' #- '{IbeConfirmation,token}' #- '{EmailVerification,token}'
                   else payload end
  where id = p_id
$$;

create or replace function app.outbox_refused(p_id uuid, p_error text) returns void
language sql security definer set search_path = pg_catalog, public, pg_temp
as $$
  update outbox set sent_at = now(), last_error = left(p_error, 500),
    payload = case when topic = 'mail-send'
                   then payload #- '{Invitation,token}' #- '{IbeConfirmation,token}' #- '{EmailVerification,token}'
                   else payload end
  where id = p_id
$$;
