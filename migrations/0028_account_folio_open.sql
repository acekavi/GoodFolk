-- An account's folio is its city ledger until it is invoiced; the next charge then opens a fresh one, so an account
-- holds one open folio per property and any number of invoiced ones.
drop index folio_account_idx;
create unique index folio_account_idx on folio (property_id, account_id) where kind = 'account' and status = 'open';
create index folio_account_all_idx on folio (property_id, account_id, created_at desc) where kind = 'account';
