-- Retention without starvation: a guest document the sweep kept (its guest still has a recent or future stay) waits
-- until the earliest moment it could expire before it is a candidate again, so kept documents can never fill the
-- sweep's 1000-candidate limit ahead of expired ones. Null means a year after upload.
alter table guest_document add column recheck_after timestamptz;

-- `timestamptz + interval` is not immutable, so `coalesce(recheck_after, uploaded_at + interval '1 year')` cannot be
-- indexed; the two cases get a partial index each and the function below merges them in that order.
create index guest_document_recheck_idx on guest_document (recheck_after) where recheck_after is not null;
create index guest_document_unchecked_idx on guest_document (uploaded_at) where recheck_after is null;

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
