-- Phase 7a: a posting key names one charge for the whole tenant, whichever folio it landed on. Every key embeds a
-- globally unique id (a stay, a POS bill), so a replay after the stay's routing or account changed still finds the
-- line instead of posting it again. Reversal and transfer lines carry no key.
create unique index folio_line_posting_key_idx on folio_line (tenant_id, posting_key) where posting_key is not null;
