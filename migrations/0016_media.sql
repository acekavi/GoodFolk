-- Phase 6a: media assets (gallery photos, issue photos, guest ID documents), the links that use them, and the
-- daily sweep's candidates. The files live in object storage under deterministic keys (t/{tenant}/a/{asset}/...);
-- these tables hold what is known about them.

-- One stored file. Within a tenant and purpose the same file (same BLAKE3) is stored once: a second upload links to
-- the existing asset. `pending_target` holds the link to write ({kind, id, document_kind?}) while the file is
-- uploading; it is cleared once the link is written.
create table media_asset (
  id uuid primary key,
  tenant_id uuid not null references tenant (id) on delete cascade,
  property_id uuid not null,
  purpose text not null check (purpose in ('gallery', 'issue_photo', 'guest_document')),
  visibility text not null check (visibility in ('public', 'private')),
  status text not null default 'uploading' check (status in ('uploading', 'processing', 'ready', 'failed')),
  error text check (length(error) <= 200),
  filename text not null check (length(filename) between 1 and 255),
  declared_bytes bigint not null check (declared_bytes > 0),
  sha256 bytea not null check (octet_length(sha256) = 32),
  blake3 bytea check (octet_length(blake3) = 32),
  mime text,
  bytes bigint,
  width integer,
  height integer,
  s3_upload_id text,
  archive_key text,
  renditions jsonb not null default '[]',
  pending_target jsonb check (jsonb_typeof(pending_target) = 'object'),
  created_by uuid references app_user (id) on delete set null,
  created_at timestamptz not null default now(),
  processing_at timestamptz,
  ready_at timestamptz,
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade,
  check ((visibility = 'public') = (purpose = 'gallery'))
);
create unique index media_asset_dedup_idx on media_asset (tenant_id, purpose, blake3) where blake3 is not null;
create index media_asset_sweep_idx on media_asset (status, created_at);

-- A room type's gallery, in `position` order.
create table room_type_media (
  tenant_id uuid not null,
  room_type_id uuid not null references room_type (id) on delete cascade,
  asset_id uuid not null references media_asset (id) on delete cascade,
  position integer not null,
  caption text not null default '' check (char_length(caption) <= 200),
  primary key (room_type_id, asset_id)
);
create index room_type_media_asset_idx on room_type_media (asset_id);

-- An issue's photos, at most 5 (checked when an upload starts).
create table room_issue_media (
  tenant_id uuid not null,
  issue_id uuid not null references room_issue (id) on delete cascade,
  asset_id uuid not null references media_asset (id) on delete cascade,
  primary key (issue_id, asset_id)
);
create index room_issue_media_asset_idx on room_issue_media (asset_id);

-- A guest's ID documents. Guests are tenant-wide, so a document is too.
create table guest_document (
  id uuid primary key,
  tenant_id uuid not null,
  guest_id uuid not null,
  asset_id uuid not null references media_asset (id) on delete cascade,
  kind text not null check (kind in ('passport', 'nic', 'driving_licence', 'other')),
  uploaded_by uuid references app_user (id) on delete set null,
  uploaded_at timestamptz not null default now(),
  foreign key (tenant_id, guest_id) references guest (tenant_id, id) on delete cascade
);
create index guest_document_guest_idx on guest_document (guest_id);
create index guest_document_asset_idx on guest_document (asset_id);

do $$
declare t text;
begin
  foreach t in array array['media_asset', 'room_type_media', 'room_issue_media', 'guest_document'] loop
    execute format('alter table %I enable row level security', t);
    execute format('alter table %I force row level security', t);
    execute format(
      'create policy tenant_isolation on %I for all using (tenant_id = (select app.current_tenant())) with check (tenant_id = (select app.current_tenant()))',
      t);
    -- The sweep's candidate list (app.media_sweep_due below) spans tenants. Forced row-level security applies to
    -- the table owner too, and on Cloud SQL the owner is neither a superuser nor BYPASSRLS, so the security definer
    -- function would see no rows without this read-only policy for the owner (the role running this migration).
    -- goodfolk_app is never a member of the owner (db::assert_rls_applies), so the API gains nothing from it.
    execute format('create policy sweep_read on %I for select to %I using (true)', t, current_user);
  end loop;
end $$;

-- Sweep candidates across tenants (the sweep has no tenant): ids only. Each is then handled in its own tenant-scoped
-- transaction, where row-level security applies as usual and the rule is checked again before anything changes. A
-- guest document over a year old is only a candidate here: whether its guest still has a stay within the last 365
-- days of a property's business date (or later) is decided in that transaction (modules/media/src/sweep.rs).
create function app.media_sweep_due()
returns table (tenant_id uuid, kind text, id uuid)
language sql stable security definer set search_path = pg_catalog, public, pg_temp
as $$
  select d.tenant_id, 'expired_document', d.id from guest_document d
  where d.uploaded_at < now() - interval '1 year'
  union all
  select a.tenant_id, 'orphan', a.id from media_asset a
  where a.status in ('ready', 'failed') and a.created_at < now() - interval '24 hours'
    and not exists (select 1 from room_type_media l where l.asset_id = a.id)
    and not exists (select 1 from room_issue_media l where l.asset_id = a.id)
    and not exists (select 1 from guest_document l where l.asset_id = a.id)
  union all
  select a.tenant_id, 'stale_upload', a.id from media_asset a
  where a.status = 'uploading' and a.created_at < now() - interval '24 hours'
  union all
  select a.tenant_id, 'stuck', a.id from media_asset a
  where a.status = 'processing' and a.processing_at < now() - interval '1 hour'
$$;

-- Purges finished outbox rows (sent more than 7 days ago) and receiver dedupe ids older than 30 days.
create function app.purge_events(out outbox_rows bigint, out processed_events bigint)
language sql security definer set search_path = pg_catalog, public, pg_temp
as $$
  with o as (delete from outbox where sent_at < now() - interval '7 days' returning 1),
       p as (delete from processed_event where processed_at < now() - interval '30 days' returning 1)
  select (select count(*) from o), (select count(*) from p)
$$;

revoke all on function app.media_sweep_due(), app.purge_events() from public;
grant execute on function app.media_sweep_due(), app.purge_events() to goodfolk_app;
