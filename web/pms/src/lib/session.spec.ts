import { describe, expect, it } from 'vitest';
import { can, isTenantOwner, type Profile } from './session';

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

describe('can', () => {
	it('lets owners and managers manage rooms, and front desk block them', () => {
		const manager = profile([{ role: 'manager', property_id: 'p1' }]);
		const desk = profile([{ role: 'front_desk' }]);
		const housekeeping = profile([{ role: 'housekeeping' }]);

		expect(can(manager, 'manageRooms', 'p1')).toBe(true);
		expect(can(manager, 'manageRooms', 'p2')).toBe(false);
		expect(can(desk, 'manageRooms', 'p1')).toBe(false);
		expect(can(desk, 'blockRooms', 'p1')).toBe(true);
		expect(can(housekeeping, 'blockRooms', 'p1')).toBe(false);
	});
});
