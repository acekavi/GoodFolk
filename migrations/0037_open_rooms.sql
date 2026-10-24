-- Phase 8a: the night audit's deposits list starts from each property's rooms still to arrive.
create index reservation_room_open_idx on reservation_room (property_id, reservation_id)
  where status in ('tentative', 'confirmed');
