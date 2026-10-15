import { QueryClient } from '@tanstack/svelte-query';
import { describe, expect, it } from 'vitest';
import { applyEvent } from './events';
import {
	documentKindLabel,
	DOCUMENT_KINDS,
	guestDocumentsKey,
	reservationGuests,
	uploaderLabel,
	viewSize
} from './guest-documents';

describe('viewSize', () => {
	it('opens a PDF as uploaded and an image at its md size', () => {
		expect(viewSize('application/pdf')).toBe('original');
		expect(viewSize('image/jpeg')).toBe('md');
		expect(viewSize('image/webp')).toBe('md');
		expect(viewSize(null)).toBe('md');
	});
});

describe('documentKindLabel', () => {
	it('names every kind, from the API or from GraphQL', () => {
		expect(DOCUMENT_KINDS.map(documentKindLabel)).toEqual([
			'Passport',
			'NIC',
			'Driving licence',
			'Other'
		]);
		expect(documentKindLabel('DRIVING_LICENCE')).toBe('Driving licence');
	});
});

describe('uploaderLabel', () => {
	it('names the uploader, or says their account is gone', () => {
		expect(uploaderLabel('Nimal Perera')).toBe('Nimal Perera');
		expect(uploaderLabel(null)).toBe('A deleted user');
	});
});

describe('reservationGuests', () => {
	const guest = (id: string) => ({ id, firstName: id.toUpperCase(), lastName: 'Silva' });

	it('lists the booker, then each room’s primary guest and occupants, each once', () => {
		const guests = reservationGuests({
			booker: guest('a'),
			rooms: [
				{ primaryGuest: guest('a'), occupants: [guest('a'), guest('b')] },
				{ primaryGuest: guest('c'), occupants: [guest('b')] },
				{ primaryGuest: null, occupants: [] }
			]
		});

		expect(guests.map((g) => g.id)).toEqual(['a', 'b', 'c']);
	});
});

describe('guestDocumentsKey', () => {
	it('is refetched by the server’s media:<property> event', () => {
		const client = new QueryClient();
		client.setQueryData(guestDocumentsKey('p1', 'g1'), []);
		client.setQueryData(guestDocumentsKey('p2', 'g1'), []);

		applyEvent(client, 'invalidate', '["media:p1"]');

		expect(client.getQueryState(guestDocumentsKey('p1', 'g1'))?.isInvalidated).toBe(true);
		expect(client.getQueryState(guestDocumentsKey('p2', 'g1'))?.isInvalidated).toBe(false);
	});
});
