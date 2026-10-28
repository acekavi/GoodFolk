-- Phase 7d: the VAT report reads a return period's invoices and credit notes by the business date each was issued on,
-- which lives in its data snapshot. Under forced row-level security `->>` is not leakproof, so no index on the
-- expression could serve the period as an index condition (see 0011); a stored column compared as a date can. A
-- text-to-date cast is not immutable (it reads DateStyle), so the column is built from the ISO date's parts.
alter table invoice add column issued_on date generated always as (make_date(
  substr(data->>'issued_on', 1, 4)::integer,
  substr(data->>'issued_on', 6, 2)::integer,
  substr(data->>'issued_on', 9, 2)::integer)) stored;
create index invoice_issued_on_idx on invoice (property_id, issued_on);
