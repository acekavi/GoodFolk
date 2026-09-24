-- Sign-in attempts per email, for throttling. Looked up before any tenant is known, so, like `session`,
-- it has no RLS and no tenant_id. Attempts for unknown emails are recorded too, so throttling never reveals
-- whether an account exists. A successful sign-in deletes its email's rows.
create table login_failure (
  id uuid primary key,
  email citext not null,
  at timestamptz not null default now()
);
create index login_failure_email_at_idx on login_failure (email, at);
