-- The original response's Content-Type, so replays are byte-for-byte identical (e.g. application/problem+json).
alter table idempotency_key add column content_type text;
