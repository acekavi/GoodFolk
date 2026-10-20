-- 0020 added this key for folio and POS bill references through (tenant_id, id); 0021 moved those to (property_id, id),
-- so nothing references it. Its index matches the RLS predicate `tenant_id = current_setting('app.tenant_id')`, and
-- the planner, guessing that it narrows a property's rooms, chose a generic plan for the reservations list that scans
-- every room. (`reservation_tenant_id_id_key` stays: `routing_rule` references it.)
alter table reservation_room drop constraint reservation_room_tenant_id_id_key;
