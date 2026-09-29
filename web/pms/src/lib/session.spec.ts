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

	it('lets owners and managers manage rates, and nobody else', () => {
		const owner = profile([{ role: 'owner' }]);
		const manager = profile([{ role: 'manager' }]);
		const accountant = profile([{ role: 'accountant' }]);
		const desk = profile([{ role: 'front_desk' }]);

		expect(can(owner, 'manageRates', 'p1')).toBe(true);
		expect(can(manager, 'manageRates', 'p1')).toBe(true);
		expect(can(accountant, 'manageRates', 'p1')).toBe(false);
		expect(can(desk, 'manageRates', 'p1')).toBe(false);
	});

	it('lets every role view reservations, mirroring the server ReservationsView grant', () => {
		for (const role of ['owner', 'manager', 'front_desk', 'housekeeping', 'accountant'] as const) {
			expect(can(profile([{ role, property_id: 'p1' }]), 'viewReservations', 'p1')).toBe(true);
		}
	});

	it('lets owner, manager and front desk manage reservations, mirroring ReservationsManage', () => {
		const owner = profile([{ role: 'owner' }]);
		const manager = profile([{ role: 'manager', property_id: 'p1' }]);
		const desk = profile([{ role: 'front_desk' }]);
		const housekeeping = profile([{ role: 'housekeeping' }]);
		const accountant = profile([{ role: 'accountant' }]);

		expect(can(owner, 'manageReservations', 'p1')).toBe(true);
		expect(can(manager, 'manageReservations', 'p1')).toBe(true);
		expect(can(manager, 'manageReservations', 'p2')).toBe(false);
		expect(can(desk, 'manageReservations', 'p1')).toBe(true);
		expect(can(housekeeping, 'manageReservations', 'p1')).toBe(false);
		expect(can(accountant, 'manageReservations', 'p1')).toBe(false);
	});

	it('lets owner, manager and front desk check in, undo and check out, mirroring FrontDeskCheckIn', () => {
		const owner = profile([{ role: 'owner' }]);
		const manager = profile([{ role: 'manager', property_id: 'p1' }]);
		const desk = profile([{ role: 'front_desk' }]);
		const housekeeping = profile([{ role: 'housekeeping' }]);
		const accountant = profile([{ role: 'accountant' }]);

		expect(can(owner, 'frontDeskCheckIn', 'p1')).toBe(true);
		expect(can(manager, 'frontDeskCheckIn', 'p1')).toBe(true);
		expect(can(manager, 'frontDeskCheckIn', 'p2')).toBe(false);
		expect(can(desk, 'frontDeskCheckIn', 'p1')).toBe(true);
		expect(can(housekeeping, 'frontDeskCheckIn', 'p1')).toBe(false);
		expect(can(accountant, 'frontDeskCheckIn', 'p1')).toBe(false);
	});
});
