import { describe, expect, it } from 'vitest';
import {
	ALL,
	grantFromNode,
	grantLabel,
	grantRows,
	grantsBody,
	grantText,
	inviteSignInHref,
	isInvitee,
	JUNIOR_ROLES,
	landingProperty,
	matrixRows,
	mayActOn,
	mayEditMember,
	mayGive,
	newGrantRow,
	roleLabel,
	roleOptions,
	scopeOptions,
	type Grant
} from './staff';

describe('roleLabel and grantLabel', () => {
	it('names the role and where it applies', () => {
		expect(roleLabel('hk_supervisor')).toBe('Housekeeping supervisor');
		expect(grantLabel({ role: 'front_desk', property_name: 'Lagoon House' })).toBe(
			'Front desk · Lagoon House'
		);
		expect(grantLabel({ role: 'manager', property_name: null })).toBe('Manager · All properties');
	});
});

describe('inviteSignInHref', () => {
	it('returns to the invitation page after sign-in', () => {
		const href = inviteSignInHref('ab-_9');
		expect(href).toBe('/login?redirect=%2Finvite%2Fab-_9');
		expect(new URL(href, 'http://x').searchParams.get('redirect')).toBe('/invite/ab-_9');
	});
});

describe('isInvitee', () => {
	it('compares emails without case', () => {
		expect(isInvitee({ email: 'Ann@Example.lk' }, 'ann@example.lk')).toBe(true);
		expect(isInvitee({ email: 'bob@example.lk' }, 'ann@example.lk')).toBe(false);
	});
});

describe('landingProperty', () => {
	it('opens the first granted property', () => {
		expect(
			landingProperty({
				grants: [
					{ role: 'front_desk', property_id: 'p1' },
					{ role: 'pos', property_id: 'p2' }
				]
			})
		).toBe('p1');
	});
	it('falls back to the property list for a tenant-wide grant or none', () => {
		expect(
			landingProperty({
				grants: [
					{ role: 'front_desk', property_id: 'p1' },
					{ role: 'manager', property_id: null }
				]
			})
		).toBeNull();
		expect(landingProperty({ grants: [] })).toBeNull();
	});
});

describe('the manager rules', () => {
	const owner: Grant[] = [{ role: 'owner', property_id: null }];
	const managerOfA: Grant[] = [{ role: 'manager', property_id: 'a' }];
	const wideManager: Grant[] = [{ role: 'manager', property_id: null }];
	const desk: Grant[] = [{ role: 'front_desk', property_id: 'a' }];

	it('lets nobody give owner, and owners give anything else anywhere', () => {
		expect(mayGive(owner, { role: 'owner', property_id: null })).toBe(false);
		expect(mayGive(owner, { role: 'manager', property_id: 'b' })).toBe(true);
		expect(mayGive(owner, { role: 'pos', property_id: null })).toBe(true);
	});

	it('lets a manager give junior roles only at the properties they manage', () => {
		for (const role of JUNIOR_ROLES) {
			expect(mayGive(managerOfA, { role, property_id: 'a' }), role).toBe(true);
			expect(mayGive(managerOfA, { role, property_id: 'b' }), role).toBe(false);
			expect(mayGive(managerOfA, { role, property_id: null }), role).toBe(false);
		}
		expect(mayGive(managerOfA, { role: 'manager', property_id: 'a' })).toBe(false);
	});

	it('lets a tenant-wide manager give junior roles anywhere, tenant-wide included', () => {
		expect(mayGive(wideManager, { role: 'housekeeping', property_id: null })).toBe(true);
		expect(mayGive(wideManager, { role: 'housekeeping', property_id: 'b' })).toBe(true);
		expect(mayGive(wideManager, { role: 'manager', property_id: 'b' })).toBe(false);
	});

	it('lets others give nothing', () => {
		expect(mayGive(desk, { role: 'front_desk', property_id: 'a' })).toBe(false);
		expect(roleOptions(desk)).toEqual([]);
	});

	it('lets a manager act only on members whose every grant is junior at their properties', () => {
		expect(mayActOn(managerOfA, [{ role: 'front_desk', property_id: 'a' }])).toBe(true);
		expect(
			mayActOn(managerOfA, [
				{ role: 'front_desk', property_id: 'a' },
				{ role: 'housekeeping', property_id: 'b' }
			])
		).toBe(false);
		expect(mayActOn(managerOfA, [{ role: 'front_desk', property_id: null }])).toBe(false);
		expect(mayActOn(managerOfA, [{ role: 'manager', property_id: 'a' }])).toBe(false);
		expect(mayActOn(wideManager, [{ role: 'accountant', property_id: null }])).toBe(true);
		expect(mayActOn(wideManager, owner)).toBe(false);
		expect(mayActOn(owner, [{ role: 'manager', property_id: null }])).toBe(true);
		expect(mayActOn(desk, [{ role: 'front_desk', property_id: 'a' }])).toBe(false);
	});

	it('never offers to change or remove oneself', () => {
		const me = { user_id: 'u1', grants: owner };
		expect(mayEditMember(me, { userId: 'u1', grants: owner })).toBe(false);
		expect(mayEditMember(me, { userId: 'u2', grants: owner })).toBe(true);
	});

	it('offers the roles each may give, keeping those a member already holds', () => {
		expect(roleOptions(owner)).toEqual([
			'manager',
			'front_desk',
			'housekeeping',
			'hk_supervisor',
			'accountant',
			'pos'
		]);
		expect(roleOptions(owner, owner)[0]).toBe('owner');
		expect(roleOptions(managerOfA)).toEqual(JUNIOR_ROLES);
	});

	it('offers the properties each may give a role at', () => {
		const ids = ['a', 'b'];
		expect(scopeOptions(owner, 'manager', ids)).toEqual([ALL, 'a', 'b']);
		expect(scopeOptions(owner, 'owner', ids, owner)).toEqual([ALL]);
		expect(scopeOptions(managerOfA, 'front_desk', ids)).toEqual(['a']);
		expect(scopeOptions(managerOfA, 'manager', ids)).toEqual([]);
		expect(scopeOptions(wideManager, 'pos', ids)).toEqual([ALL, 'a', 'b']);
		// A held grant at a property the list doesn't show is kept as it is.
		expect(
			scopeOptions(owner, 'front_desk', ['a'], [{ role: 'front_desk', property_id: 'z' }])
		).toEqual([ALL, 'a', 'z']);
	});

	it('starts a new row at front desk where the user may give it', () => {
		expect(newGrantRow(owner, ['a', 'b'])).toEqual({ role: 'front_desk', scope: ALL });
		expect(newGrantRow(managerOfA, ['a', 'b'])).toEqual({ role: 'front_desk', scope: 'a' });
		expect(newGrantRow(desk, ['a'])).toBeNull();
	});
});

describe('grant rows', () => {
	it('send each grant once, with every property as no property', () => {
		expect(
			grantsBody([
				{ role: 'front_desk', scope: 'a' },
				{ role: 'front_desk', scope: 'a' },
				{ role: 'pos', scope: ALL }
			])
		).toEqual([
			{ role: 'front_desk', property_id: 'a' },
			{ role: 'pos', property_id: null }
		]);
		expect(grantRows([{ role: 'owner', property_id: null }])).toEqual([
			{ role: 'owner', scope: ALL }
		]);
	});

	it('read GraphQL grants and name their property', () => {
		const grant = grantFromNode({ role: 'HK_SUPERVISOR', propertyId: 'a' });
		expect(grant).toEqual({ role: 'hk_supervisor', property_id: 'a' });
		const names = new Map([['a', 'Lagoon House']]);
		expect(grantText(grant, names)).toBe('Housekeeping supervisor · Lagoon House');
		expect(grantText({ role: 'pos', property_id: null }, names)).toBe('POS · All properties');
		expect(grantText({ role: 'pos', property_id: 'b' }, names)).toBe('POS · Another property');
	});
});

describe('matrixRows', () => {
	it('marks the roles holding each permission, a column per role', () => {
		expect(
			matrixRows([
				{ permission: 'StaffManage', allows: 'Invite staff', roles: ['OWNER', 'MANAGER'] },
				{ permission: 'PosPost', allows: 'POS bills', roles: ['OWNER', 'MANAGER', 'POS'] }
			])
		).toEqual([
			{
				permission: 'StaffManage',
				allows: 'Invite staff',
				holds: [true, true, false, false, false, false, false]
			},
			{
				permission: 'PosPost',
				allows: 'POS bills',
				holds: [true, true, false, false, false, false, true]
			}
		]);
	});
});
