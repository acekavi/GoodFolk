-- Phase 7d: the VAT report reads a return period's invoices and credit notes by the business date each was issued on,
-- which lives in its data snapshot. ISO dates sort as text, so the period is a range scan of this index rather than a
-- read of every snapshot the property ever issued.
create index invoice_issued_on_idx on invoice (property_id, (data->>'issued_on'));
