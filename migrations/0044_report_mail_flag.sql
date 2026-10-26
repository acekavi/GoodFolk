-- Phase 7d: whether a weekly or monthly report is emailed once its PDF is ready. The close sets it for the period that
-- just ended; a catch-up's older periods are rendered and stored, never mailed.
alter table report add column mail boolean not null default false;
