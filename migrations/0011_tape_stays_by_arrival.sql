-- Under forced row-level security `stay && daterange(...)` is not leakproof, so no index can use it as an index
-- condition (the tape chart read 1,156 stays of 10 rooms to keep 29). Date and integer comparisons are leakproof:
-- the tape window finds short stays by arrival (no stay of up to 31 nights that overlaps the window can arrive
-- more than 31 days before it) and the rare long ones through their own small index.
alter table reservation_room add column nights integer generated always as (upper(stay) - lower(stay)) stored;
create index reservation_room_room_arrival_idx on reservation_room (room_id, arrival) where room_id is not null;
create index reservation_room_long_stay_idx on reservation_room (room_id) where nights > 31;
