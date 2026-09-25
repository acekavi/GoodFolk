-- The original response's ETag, so a replayed create carries the created resource's version like the first response.
alter table idempotency_key add column etag text;
