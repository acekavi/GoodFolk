-- Reservations made before folios existed (0020) have none: give every reservation its deposit folio and every
-- reservation room its guest folio, as `reservations::create_reservation` opens them, and every reservation billed
-- to an account with no routing the default routing (`folio::DEFAULT_ROUTING`: room and meal_plan) to that account.
-- These tables have FORCE ROW LEVEL SECURITY, which applies to the migration owner too and no tenant is set here, so
-- lift it for the backfill, as 0020 does.
alter table folio no force row level security;
alter table reservation no force row level security;
alter table reservation_room no force row level security;
alter table routing_rule no force row level security;
-- backfill: start
insert into folio (id, tenant_id, property_id, kind, reservation_id)
select gen_random_uuid(), r.tenant_id, r.property_id, 'deposit', r.id
from reservation r
where not exists (select 1 from folio f where f.kind = 'deposit' and f.reservation_id = r.id)
on conflict do nothing;

insert into folio (id, tenant_id, property_id, kind, reservation_room_id)
select gen_random_uuid(), rr.tenant_id, rr.property_id, 'guest', rr.id
from reservation_room rr
where not exists (select 1 from folio f where f.kind = 'guest' and f.reservation_room_id = rr.id)
on conflict do nothing;

insert into routing_rule (tenant_id, reservation_id, category, account_id)
select r.tenant_id, r.id, c.category, r.account_id
from reservation r
cross join (values ('room'), ('meal_plan')) as c(category)
where r.account_id is not null
  and not exists (select 1 from routing_rule rule where rule.reservation_id = r.id)
on conflict do nothing;
-- backfill: end
alter table folio force row level security;
alter table reservation force row level security;
alter table reservation_room force row level security;
alter table routing_rule force row level security;
