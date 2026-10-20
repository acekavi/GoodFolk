-- Phase 7a: a `transfer` payment moves a credit between two folios (a reservation's deposit to its first stay, or a
-- charge to account), as a pair sharing a `transfer_id`. It is never refunded: the original payment is.
alter table payment drop constraint payment_method_check;
alter table payment add constraint payment_method_check
  check (method in ('cash', 'card_terminal', 'bank_transfer', 'account', 'transfer'));
