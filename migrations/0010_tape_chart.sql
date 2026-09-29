-- Stays with no room yet. Check-in needs a room, so only confirmed stays can be unassigned. The tape chart's
-- "Needs a room" list scans exactly these, and the index stays tiny because nearly every stay is auto-assigned.
create index reservation_room_unassigned_idx on reservation_room using gist (property_id, stay)
  where room_id is null and status = 'confirmed';
