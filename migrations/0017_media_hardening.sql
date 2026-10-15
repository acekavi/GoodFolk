-- Phase 6a hardening of 0016: dedup within a property, tenant-checked foreign keys on the media links, and a capped
-- sweep candidate list.

-- The same file is stored once per property and purpose, not per tenant: a duplicate linked across properties would let
-- deleting one property cascade into another's links, and `media:<property>` events would miss the other property.
drop index media_asset_dedup_idx;
create unique index media_asset_dedup_idx on media_asset (tenant_id, property_id, purpose, blake3)
  where blake3 is not null;

-- Links reference their asset and their parent through (tenant_id, id), so a link can never join two tenants' rows.
alter table media_asset add constraint media_asset_tenant_id_id_key unique (tenant_id, id);
alter table room_type add constraint room_type_tenant_id_id_key unique (tenant_id, id);
alter table room_issue add constraint room_issue_tenant_id_id_key unique (tenant_id, id);

alter table room_type_media
  drop constraint room_type_media_room_type_id_fkey,
  drop constraint room_type_media_asset_id_fkey,
  add foreign key (tenant_id, room_type_id) references room_type (tenant_id, id) on delete cascade,
  add foreign key (tenant_id, asset_id) references media_asset (tenant_id, id) on delete cascade;
alter table room_issue_media
  drop constraint room_issue_media_issue_id_fkey,
  drop constraint room_issue_media_asset_id_fkey,
  add foreign key (tenant_id, issue_id) references room_issue (tenant_id, id) on delete cascade,
  add foreign key (tenant_id, asset_id) references media_asset (tenant_id, id) on delete cascade;
alter table guest_document
  drop constraint guest_document_asset_id_fkey,
  add foreign key (tenant_id, asset_id) references media_asset (tenant_id, id) on delete cascade;

-- At most 1000 candidates of each kind per run, so a large backlog is spread over several days.
create or replace function app.media_sweep_due()
returns table (tenant_id uuid, kind text, id uuid)
language sql stable security definer set search_path = pg_catalog, public, pg_temp
as $$
  (select d.tenant_id, 'expired_document', d.id from guest_document d
   where d.uploaded_at < now() - interval '1 year'
   order by d.uploaded_at limit 1000)
  union all
  (select a.tenant_id, 'orphan', a.id from media_asset a
   where a.status in ('ready', 'failed') and a.created_at < now() - interval '24 hours'
     and not exists (select 1 from room_type_media l where l.asset_id = a.id)
     and not exists (select 1 from room_issue_media l where l.asset_id = a.id)
     and not exists (select 1 from guest_document l where l.asset_id = a.id)
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
