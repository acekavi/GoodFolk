-- Phase 7a: folios, taxes, payments, POS bills, exchange rates and invoices.

alter table property
  add column accepted_currencies char(3)[] not null default '{LKR,USD,EUR,GBP}',
  add column vat_registered boolean not null default false,
  add column tdl_small_turnover boolean not null default false,
  add column tin text check (tin ~ '^[0-9A-Z-]{5,20}$'),
  add column legal_name text check (length(legal_name) between 1 and 200),
  add column address text check (length(address) <= 500);
alter table account add column tin text check (tin ~ '^[0-9A-Z-]{5,20}$');
alter table guest add column tin text check (tin ~ '^[0-9A-Z-]{5,20}$');
alter table rate_plan add column prices_include_tax boolean not null default false;

alter table property_counter drop constraint property_counter_name_check;
alter table property_counter add constraint property_counter_name_check
  check (name in ('confirmation', 'invoice', 'credit_note'));

-- Folios reference a booking and a stay through (tenant_id, id), so a folio can never join two tenants' rows.
alter table reservation add constraint reservation_tenant_id_id_key unique (tenant_id, id);
alter table reservation_room add constraint reservation_room_tenant_id_id_key unique (tenant_id, id);

create table tax_rule (
  id uuid primary key,
  tenant_id uuid not null,
  property_id uuid not null,
  code text not null check (code ~ '^[A-Z][A-Z0-9_]{0,15}$'),
  name text not null check (length(name) between 1 and 100),
  kind text not null check (kind in ('tax', 'levy', 'service_charge')),
  rate_bp integer not null check (rate_bp between 0 and 10000),
  applies_to text[] not null,
  base text[] not null,
  sequence integer not null,
  presentation text not null check (presentation in ('on_bill', 'absorbed')),
  condition jsonb not null default '{"type":"always"}',
  effective_from date not null,
  effective_to date,
  version integer not null default 1,
  check (effective_to is null or effective_to > effective_from),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade
);

create table outlet (
  id uuid primary key,
  tenant_id uuid not null,
  property_id uuid not null,
  code text not null check (code ~ '^[A-Z0-9]{1,10}$'),
  name text not null check (length(name) between 1 and 100),
  service_charge boolean not null default true,
  active boolean not null default true,
  version integer not null default 1,
  unique (property_id, code),
  unique (tenant_id, id),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade
);

create table fx_rate (
  tenant_id uuid not null,
  property_id uuid not null,
  date date not null,
  currency char(3) not null,
  rate_to_base numeric(18,6) not null check (rate_to_base > 0),
  source text not null check (source in ('cbsl', 'manual')),
  source_date date not null,
  set_by uuid,
  set_at timestamptz not null default now(),
  primary key (property_id, date, currency),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade
);

create table fx_alert (
  property_id uuid primary key,
  tenant_id uuid not null,
  message text not null,
  raised_at timestamptz not null default now(),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade
);

create table folio (
  id uuid primary key,
  tenant_id uuid not null,
  property_id uuid not null,
  kind text not null check (kind in ('guest', 'account', 'deposit')),
  reservation_room_id uuid,
  account_id uuid,
  reservation_id uuid,
  status text not null default 'open' check (status in ('open', 'invoiced')),
  created_at timestamptz not null default now(),
  version integer not null default 1,
  check ((kind = 'guest') = (reservation_room_id is not null)),
  check ((kind = 'account') = (account_id is not null)),
  check ((kind = 'deposit') = (reservation_id is not null)),
  unique (tenant_id, id),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade,
  foreign key (tenant_id, reservation_room_id) references reservation_room (tenant_id, id),
  foreign key (tenant_id, account_id) references account (tenant_id, id),
  foreign key (tenant_id, reservation_id) references reservation (tenant_id, id)
);
create unique index folio_guest_idx on folio (reservation_room_id) where kind = 'guest';
create unique index folio_account_idx on folio (property_id, account_id) where kind = 'account';
create unique index folio_deposit_idx on folio (reservation_id) where kind = 'deposit';

create table folio_line (
  id uuid primary key,
  tenant_id uuid not null,
  folio_id uuid not null,
  source text not null check (source in
    ('room', 'meal_plan', 'laundry', 'pos', 'manual', 'cancellation_fee', 'no_show_fee', 'adjustment')),
  category text not null check (category ~ '^(room|meal_plan|laundry|misc|fee|pos:[A-Z0-9]{1,10})$'),
  description text not null check (length(description) between 1 and 200),
  quantity integer not null default 1 check (quantity <> 0),
  net bigint not null,
  total bigint not null,
  currency char(3) not null,
  base_total bigint not null,
  rate numeric(18,6) not null,
  rate_date date not null,
  business_date date not null,
  posting_key text,
  reverses uuid,
  reason text check (length(reason) <= 500),
  transfer_id uuid,
  posted_by uuid,
  posted_at timestamptz not null default now(),
  unique (tenant_id, id),
  unique (folio_id, posting_key),
  foreign key (tenant_id, folio_id) references folio (tenant_id, id)
);
create unique index folio_line_reversed_once_idx on folio_line (reverses) where reverses is not null;

create table folio_line_tax (
  tenant_id uuid not null,
  line_id uuid not null,
  code text not null,
  name text not null,
  kind text not null,
  rate_bp integer not null,
  base bigint not null,
  amount bigint not null,
  presentation text not null,
  primary key (line_id, code),
  foreign key (tenant_id, line_id) references folio_line (tenant_id, id)
);

create table payment (
  id uuid primary key,
  tenant_id uuid not null,
  folio_id uuid,
  method text not null check (method in ('cash', 'card_terminal', 'bank_transfer', 'account')),
  amount bigint not null check (amount <> 0),
  currency char(3) not null,
  base_amount bigint not null,
  rate numeric(18,6) not null,
  rate_date date not null,
  details jsonb not null default '{}',
  evidence text,
  refund_of uuid,
  business_date date not null,
  transfer_id uuid,
  taken_by uuid,
  taken_at timestamptz not null default now(),
  pos_bill_id uuid,
  unique (tenant_id, id),
  foreign key (tenant_id, folio_id) references folio (tenant_id, id),
  foreign key (tenant_id, refund_of) references payment (tenant_id, id)
);
create index payment_folio_idx on payment (folio_id);

create table routing_rule (
  tenant_id uuid not null,
  reservation_id uuid not null,
  category text not null,
  account_id uuid not null,
  primary key (reservation_id, category),
  foreign key (tenant_id, reservation_id) references reservation (tenant_id, id) on delete cascade,
  foreign key (tenant_id, account_id) references account (tenant_id, id)
);

create table pos_bill (
  id uuid primary key,
  tenant_id uuid not null,
  property_id uuid not null,
  outlet_id uuid not null,
  bill_no text not null check (length(bill_no) between 1 and 50),
  business_date date not null,
  body_hash bytea not null,
  reservation_room_id uuid,
  settled boolean not null,
  posted_at timestamptz not null default now(),
  unique (property_id, outlet_id, bill_no),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade,
  foreign key (tenant_id, outlet_id) references outlet (tenant_id, id),
  foreign key (tenant_id, reservation_room_id) references reservation_room (tenant_id, id)
);

create table invoice (
  id uuid primary key,
  tenant_id uuid not null,
  property_id uuid not null,
  folio_id uuid not null,
  kind text not null check (kind in ('invoice', 'credit_note')),
  serial text not null,
  credits uuid,
  reason text,
  data jsonb not null,
  total_lkr bigint not null,
  pdf_key text,
  issued_by uuid,
  issued_at timestamptz not null default now(),
  unique (property_id, serial),
  unique (tenant_id, id),
  foreign key (tenant_id, property_id) references property (tenant_id, id) on delete cascade,
  foreign key (tenant_id, folio_id) references folio (tenant_id, id),
  foreign key (tenant_id, credits) references invoice (tenant_id, id)
);
create unique index invoice_credited_once_idx on invoice (credits) where credits is not null;

-- Money rows are append-only: corrections are new rows. The default privileges in 0001 grant every new table its
-- full set to goodfolk_app, so the revokes come after the creates. An invoice is never deleted, and the only column
-- that changes after issue is the key of its rendered PDF.
revoke update, delete on folio_line, folio_line_tax, payment from goodfolk_app;
revoke update, delete on invoice from goodfolk_app;
grant update (pdf_key) on invoice to goodfolk_app;

-- The Sri Lanka preset (`tax::sri_lanka_preset`) for every existing property. `property` has FORCE ROW LEVEL SECURITY,
-- which applies to the migration owner too and no tenant is set here, so lift it for the copy, as 0009 does.
-- `tax_rule` is not yet row-level secured, so the insert needs no such lift.
alter table property no force row level security;
-- backfill: start
insert into tax_rule (id, tenant_id, property_id, code, name, kind, rate_bp, applies_to, base, sequence, presentation,
                      condition, effective_from)
select gen_random_uuid(), p.tenant_id, p.id, r.code, r.name, r.kind,
       case when r.code = 'TDL' and p.tdl_small_turnover then 50 else r.rate_bp end,
       r.applies_to, r.base, r.sequence, 'on_bill', r.condition::jsonb, date '2026-01-01'
from property p
cross join (values
  ('SC', 'Service charge', 'service_charge', 1000, array['pos:*'], array['net'], 10, '{"type":"always"}'),
  ('TDL', 'Tourism development levy', 'levy', 100, array['*'], array['net'], 20, '{"type":"always"}'),
  ('SSCL', 'Social security contribution levy', 'levy', 250, array['*'], array['net', 'SC', 'TDL'], 30,
   '{"type":"always"}'),
  ('VAT', 'Value added tax', 'tax', 1800, array['*'], array['net', 'SC', 'TDL', 'SSCL'], 40,
   '{"type":"vat_registered"}')
) as r(code, name, kind, rate_bp, applies_to, base, sequence, condition);
-- backfill: end
alter table property force row level security;

do $$
declare t text;
begin
  foreach t in array array['tax_rule', 'outlet', 'fx_rate', 'fx_alert', 'folio', 'folio_line', 'folio_line_tax',
                           'payment', 'routing_rule', 'pos_bill', 'invoice'] loop
    execute format('alter table %I enable row level security', t);
    execute format('alter table %I force row level security', t);
    execute format(
      'create policy tenant_isolation on %I for all using (tenant_id = (select app.current_tenant())) with check (tenant_id = (select app.current_tenant()))',
      t);
  end loop;
end $$;
