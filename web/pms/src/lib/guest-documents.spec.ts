import { QueryClient } from '@tanstack/svelte-query';
import { describe, expect, it } from 'vitest';
import { applyEvent } from './events';
import {
	documentKindLabel,
	DOCUMENT_KINDS,
	guestDocumentsKey,
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
	it('says You for the viewer’s own uploads', () => {
		expect(uploaderLabel('u1', 'u1')).toBe('You');
		expect(uploaderLabel('u2', 'u1')).toBe('Another user');
		expect(uploaderLabel(null, 'u1')).toBe('A deleted user');
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
