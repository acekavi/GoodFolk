import { describe, expect, it } from 'vitest';
import { isTenantOwner, type Profile } from './session';

function profile(grants: Profile['grants']): Profile {
	return { user_id: 'u', email: 'a@b.lk', display_name: 'A', tenants: [], grants };
}

describe('isTenantOwner', () => {
	it('is true only for a tenant-wide owner grant', () => {
		expect(isTenantOwner(profile([{ role: 'owner' }]))).toBe(true);
		expect(isTenantOwner(profile([{ role: 'owner', property_id: 'p1' }]))).toBe(false);
		expect(isTenantOwner(profile([{ role: 'manager' }]))).toBe(false);
	});
});
