-- Phase 9: the public booking site (IBE). The per-property site settings, the room type description shown on it, the
-- reservation manage-link tokens, the per-IP rate-limit counters, the `ibe_logo` media purpose and the definer that
-- resolves a property code or custom host to an enabled site.

create table ibe_site (
  tenant_id uuid not null,
  property_id uuid primary key,
  enabled boolean not null default false,
  -- The property's code when last saved. Codes are unique only within a tenant, but the public site is addressed by
  -- the code alone, so among enabled sites it must be unique across all of them.
  public_code text,
  custom_host text unique check (custom_host = lower(custom_host) and length(custom_host) <= 253
    and custom_host ~ '^([a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$'),
  logo_asset_id uuid,
  hero_asset_id uuid,
  accent char(7) check (accent ~ '^#[0-9a-f]{6}$'),
  description text not null default '' check (length(description) <= 500),
  contact_phone text check (length(contact_phone) between 1 and 50),
  contact_email text check (length(contact_email) between 3 and 254),
  terms text not null default '' check (length(terms) <= 10000),
  privacy text not null default '' check (length(privacy) <= 10000),
  version integer not null default 1,
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade,
  foreign key (tenant_id, logo_asset_id) references media_asset (tenant_id, id),
  foreign key (tenant_id, hero_asset_id) references media_asset (tenant_id, id)
);

create unique index ibe_site_public_code_idx on ibe_site (upper(public_code)) where enabled;

alter table room_type add column description text not null default '' check (length(description) <= 1000);

-- The manage link of a booking: only the SHA-256 of the 32-byte token is kept.
create table reservation_access_token (
  tenant_id uuid not null,
  reservation_id uuid primary key,
  sha256 bytea not null unique check (octet_length(sha256) = 32),
  expires_at timestamptz not null,
  created_at timestamptz not null default now(),
  foreign key (tenant_id, reservation_id) references reservation (tenant_id, id) on delete cascade
);

do $$
declare t text;
begin
  foreach t in array array['ibe_site', 'reservation_access_token'] loop
    execute format('alter table %I enable row level security', t);
    execute format('alter table %I force row level security', t);
    execute format(
      'create policy tenant_isolation on %I for all using (tenant_id = (select app.current_tenant())) with check (tenant_id = (select app.current_tenant()))',
      t);
  end loop;
end $$;

-- Fixed-window counters for the public API's limits, one row per (scope, IP, window). They are global per IP and hold
-- no tenant data, so there is no tenant column and no row-level security.
create table ibe_rate_hit (
  scope text not null,
  ip inet not null,
  window_start timestamptz not null,
  hits integer not null default 0,
  primary key (scope, ip, window_start)
);

-- The logo is public like gallery photos; the checks 0040 made name every purpose.
alter table media_asset drop constraint media_asset_purpose_check;
alter table media_asset add constraint media_asset_purpose_check
  check (purpose in ('gallery', 'issue_photo', 'guest_document', 'gallery_video', 'ibe_logo'));

alter table media_asset drop constraint media_asset_check;
alter table media_asset add constraint media_asset_check
  check ((visibility = 'public') = (purpose in ('gallery', 'gallery_video', 'ibe_logo')));

-- The public API has no tenant when a request arrives. Forced row-level security applies to the table owner too, and
-- on Cloud SQL the owner is neither a superuser nor BYPASSRLS, so the security definer function below needs a
-- read-only policy for the owner (the role running this migration), as jobs_read in 0031. `property` already has one
-- (fx_import_read in 0022). goodfolk_app is never a member of the owner (db::assert_rls_applies), so the API gains
-- nothing from it.
create policy ibe_read on ibe_site for select to current_user using (true);

-- By host when a host is given, else by code: never either. The unique indexes on `custom_host` and on the enabled
-- sites' `public_code` make the answer at most one row. $1 is the code, $2 the host: a parameter name would lose to
-- the columns of the same name.
create function app.ibe_resolve(code text, host text)
returns table (tenant_id uuid, property_id uuid)
language sql stable security definer set search_path = pg_catalog, public, pg_temp
as $$ select s.tenant_id, s.property_id
      from ibe_site s
      where s.enabled
        and case when $2 is not null then s.custom_host = lower($2) else upper(s.public_code) = upper($1) end $$;

revoke all on function app.ibe_resolve(text, text) from public;
grant execute on function app.ibe_resolve(text, text) to goodfolk_app;

-- A logo or hero image the site uses is not an orphan, whatever its age or whether a gallery still links it. The
-- function is 0018's with the site as one more link.
create or replace function app.media_sweep_due()
returns table (tenant_id uuid, kind text, id uuid)
language sql stable security definer set search_path = pg_catalog, public, pg_temp
as $$
  (select e.tenant_id, 'expired_document', e.id
   from ((select d.tenant_id, d.id, d.recheck_after as due from guest_document d
          where d.recheck_after is not null and d.recheck_after < now()
          order by d.recheck_after limit 1000)
         union all
         (select d.tenant_id, d.id, d.uploaded_at + interval '1 year' from guest_document d
          where d.recheck_after is null and d.uploaded_at < now() - interval '1 year'
          order by d.uploaded_at limit 1000)) e
   order by e.due limit 1000)
  union all
  (select a.tenant_id, 'orphan', a.id from media_asset a
   where a.status in ('ready', 'failed') and a.created_at < now() - interval '24 hours'
     and not exists (select 1 from room_type_media l where l.asset_id = a.id)
     and not exists (select 1 from room_issue_media l where l.asset_id = a.id)
     and not exists (select 1 from guest_document l where l.asset_id = a.id)
     and not exists (select 1 from ibe_site l where a.id in (l.logo_asset_id, l.hero_asset_id))
   order by a.created_at limit 1000)
  union all
  (select a.tenant_id, 'stale_upload', a.id from media_asset a
   where a.status = 'uploading' and a.created_at < now() - interval '24 hours'
   order by a.created_at limit 1000)
  union all
  (select a.tenant_id, 'stuck', a.id from media_asset a
   where a.status = 'processing' and a.processing_at < now() - interval '1 hour'
   order by a.processing_at limit 1000)
$$;
