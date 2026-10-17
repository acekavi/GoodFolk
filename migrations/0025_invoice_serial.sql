-- Phase 7a: an invoice or credit note number is at least 6 digits, so the series continues past 999,999.
alter table invoice drop constraint invoice_serial_check;
alter table invoice add constraint invoice_serial_check check (serial ~ '^(CN-)?[A-Z0-9]{2,10}-[0-9]{4}-[0-9]{6,}$');
