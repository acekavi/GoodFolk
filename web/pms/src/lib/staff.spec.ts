import { describe, expect, it } from 'vitest';
import { grantLabel, inviteSignInHref, isInvitee, landingProperty, roleLabel } from './staff';

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
