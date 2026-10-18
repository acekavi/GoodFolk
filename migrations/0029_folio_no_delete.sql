-- A folio is never deleted: an empty one stays as the record that it was opened, and one with lines or payments was
-- already undeletable through their foreign keys.
revoke delete on folio from goodfolk_app;
