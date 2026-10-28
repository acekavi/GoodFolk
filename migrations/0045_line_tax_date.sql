-- Phase 7d: the liability reports read a quarter's tax lines by the business date and property of the folio line each
-- belongs to. Reading them through folio_line and folio took three joins over 60,000 rows for a quarter of a 200-room
-- hotel; stored on the tax line (set by a trigger on insert, from its line and folio) they are one index range.

alter table folio_line_tax add column property_id uuid;
alter table folio_line_tax add column business_date date;

-- `folio_line_tax`, `folio_line` and `folio` have FORCE ROW LEVEL SECURITY, which applies to the migration owner too
-- and no tenant is set here, so lift it for the backfill, as 0027 and 0031 do.
-- backfill: start
alter table folio_line_tax no force row level security;
alter table folio_line no force row level security;
alter table folio no force row level security;
update folio_line_tax t set property_id = f.property_id, business_date = l.business_date
  from folio_line l join folio f on f.id = l.folio_id
  where l.id = t.line_id;
alter table folio_line_tax force row level security;
alter table folio_line force row level security;
alter table folio force row level security;
-- backfill: end
alter table folio_line_tax alter column property_id set not null;
alter table folio_line_tax alter column business_date set not null;

create function folio_line_tax_dates() returns trigger language plpgsql set search_path = pg_catalog, public as $$
begin
  select f.property_id, l.business_date into new.property_id, new.business_date
  from folio_line l join folio f on f.id = l.folio_id
  where l.id = new.line_id;
  return new;
end $$;
create trigger folio_line_tax_dates before insert on folio_line_tax
  for each row execute function folio_line_tax_dates();

create index folio_line_tax_property_date_idx on folio_line_tax (property_id, business_date);
