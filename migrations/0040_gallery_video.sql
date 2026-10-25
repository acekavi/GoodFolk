-- Phase 6b: room tour videos are a fourth media purpose. Like gallery photos they are public, so the check that ties
-- visibility to purpose now names both. The constraint names are the ones Postgres generated in 0016.
alter table media_asset drop constraint media_asset_purpose_check;
alter table media_asset add constraint media_asset_purpose_check
  check (purpose in ('gallery', 'issue_photo', 'guest_document', 'gallery_video'));

alter table media_asset drop constraint media_asset_check;
alter table media_asset add constraint media_asset_check
  check ((visibility = 'public') = (purpose in ('gallery', 'gallery_video')));
