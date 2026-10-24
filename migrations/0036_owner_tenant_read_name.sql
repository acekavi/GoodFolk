-- 0035's owner read policy on tenant, named as the other owner-only policies are.
alter policy invitation_tenant_read on tenant rename to owner_tenant_read;
