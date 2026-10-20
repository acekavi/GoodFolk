-- Phase 7b: the night audit reads one business date's lines and payments, and a settled POS bill's payment, on every
-- close and every review. Without these, each read scans the tenant's whole history.
create index folio_line_business_date_idx on folio_line (business_date);
create index payment_business_date_idx on payment (business_date);
create index payment_pos_bill_idx on payment (pos_bill_id) where pos_bill_id is not null;
