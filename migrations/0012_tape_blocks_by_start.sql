-- Under forced row-level security `period && daterange(...)` is not leakproof, so no index can use it as an index
-- condition (the tape chart read all 60 blocks of 10 rooms to keep 2). Date and integer comparisons are leakproof:
-- the tape window finds short blocks by start (no block of up to 31 days that overlaps the window can start more
-- than 31 days before it) and the rare long ones through their own small index. Same shape as 0011's stays.
alter table room_block
  add column starts date generated always as (lower(period)) stored,
  add column days integer generated always as (upper(period) - lower(period)) stored;
create index room_block_room_starts_idx on room_block (room_id, starts) where released_at is null;
create index room_block_long_idx on room_block (room_id) where released_at is null and days > 31;
