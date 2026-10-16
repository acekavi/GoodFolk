-- Phase 7a hardening of 0020: money rows stay in their property, POS bills are append-only and currencies are checked.

-- POS bills: the target of a payment, kept as they were posted.
alter table pos_bill add column lines jsonb not null default '[]';
alter table pos_bill add constraint pos_bill_tenant_id_id_key unique (tenant_id, id);
alter table payment add foreign key (tenant_id, pos_bill_id) references pos_bill (tenant_id, id);
alter table payment add constraint payment_target_check check (folio_id is not null or pos_bill_id is not null);
revoke update, delete on pos_bill from goodfolk_app;

-- Property-consistent references, as 0007 and 0008 do. Accounts are tenant-wide, so their references stay on the tenant.
alter table folio add constraint folio_property_id_id_key unique (property_id, id);
alter table outlet add constraint outlet_property_id_id_key unique (property_id, id);

alter table folio
  drop constraint folio_tenant_id_reservation_room_id_fkey,
  drop constraint folio_tenant_id_reservation_id_fkey,
  add foreign key (property_id, reservation_room_id) references reservation_room (property_id, id),
  add foreign key (property_id, reservation_id) references reservation (property_id, id);
alter table invoice
  drop constraint invoice_tenant_id_folio_id_fkey,
  drop constraint invoice_tenant_id_property_id_fkey,
  add foreign key (property_id, folio_id) references folio (property_id, id),
  add foreign key (tenant_id, property_id) references property (tenant_id, id);
alter table pos_bill
  drop constraint pos_bill_tenant_id_outlet_id_fkey,
  drop constraint pos_bill_tenant_id_reservation_room_id_fkey,
  drop constraint pos_bill_tenant_id_property_id_fkey,
  add foreign key (property_id, outlet_id) references outlet (property_id, id),
  add foreign key (property_id, reservation_room_id) references reservation_room (property_id, id),
  add foreign key (tenant_id, property_id) references property (tenant_id, id);

-- Currencies are upper-case ISO 4217 codes.
alter table folio_line add constraint folio_line_currency_check check (currency ~ '^[A-Z]{3}$');
alter table payment add constraint payment_currency_check check (currency ~ '^[A-Z]{3}$');
alter table fx_rate add constraint fx_rate_currency_check check (currency ~ '^[A-Z]{3}$');
alter table property add constraint property_accepted_currencies_check
  check (cardinality(accepted_currencies) >= 1 and array_to_string(accepted_currencies, ',') ~ '^[A-Z]{3}(,[A-Z]{3})*$');

-- A folio changes only by closing it.
revoke update on folio from goodfolk_app;
grant update (status, version) on folio to goodfolk_app;

alter table invoice add constraint invoice_credit_note_check check ((kind = 'credit_note') = (credits is not null));
alter table invoice add constraint invoice_credit_note_reason_check
  check (kind <> 'credit_note' or length(reason) between 1 and 500);
alter table invoice add constraint invoice_serial_check check (serial ~ '^(CN-)?[A-Z0-9]{2,10}-[0-9]{4}-[0-9]{6}$');

alter table routing_rule add constraint routing_rule_category_check
  check (category ~ '^(room|meal_plan|laundry|misc|fee|pos:[A-Z0-9]{1,10})$');
alter table folio_line_tax add constraint folio_line_tax_kind_check check (kind in ('tax', 'levy', 'service_charge'));
alter table folio_line_tax add constraint folio_line_tax_presentation_check
  check (presentation in ('on_bill', 'absorbed'));
alter table tax_rule add constraint tax_rule_condition_check check (jsonb_typeof(condition) = 'object');
alter table folio_line add foreign key (tenant_id, reverses) references folio_line (tenant_id, id);

create index fx_rate_latest_idx on fx_rate (property_id, currency, date desc);
create index invoice_folio_idx on invoice (folio_id);
create index payment_refund_of_idx on payment (refund_of) where refund_of is not null;
