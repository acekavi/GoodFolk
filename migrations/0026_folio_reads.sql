-- Phase 7a: a folio is read newest first, up to a cap per folio, so its lines and payments are indexed in that order.
create index folio_line_folio_recent_idx on folio_line (folio_id, posted_at desc, id desc);
create index payment_folio_recent_idx on payment (folio_id, id desc);
drop index payment_folio_idx;
