import { describe, expect, it, vi } from 'vitest';
import { codeForHost, customRoute, HOST_CACHE_MS, onBookingDomain } from './host';

describe('custom hosts', () => {
	it('are any host but the booking domain’s', () => {
		const origin = 'https://book.example.lk';
		expect(onBookingDomain(new URL('https://book.example.lk/GAL'), origin)).toBe(true);
		expect(onBookingDomain(new URL('https://book.galleforthotel.lk/'), origin)).toBe(false);
	});

	it('serve their site’s pages at their root', () => {
		expect(customRoute('/', 'GAL')).toBe('/GAL');
		expect(customRoute('/rooms', 'GAL')).toBe('/GAL/rooms');
		expect(customRoute('/sitemap.xml', 'GAL')).toBe('/GAL/sitemap.xml');
		expect(customRoute('/rooms', null)).toBe('/');
	});

	it('are looked up by-host once a minute', async () => {
		const fetch = vi.fn<typeof globalThis.fetch>(
			async () => new Response(JSON.stringify({ code: 'GAL' }), { status: 200 })
		);

		expect(await codeForHost(fetch, 'book.galleforthotel.lk', 0)).toBe('GAL');
		expect(await codeForHost(fetch, 'book.galleforthotel.lk', HOST_CACHE_MS - 1)).toBe('GAL');
		expect(fetch).toHaveBeenCalledOnce();
		expect(fetch.mock.calls[0][0]).toBe(
			'http://localhost:8080/api/public/v1/ibe/by-host/book.galleforthotel.lk'
		);
		await codeForHost(fetch, 'book.galleforthotel.lk', HOST_CACHE_MS);
		expect(fetch).toHaveBeenCalledTimes(2);
	});

	it('with no enabled site have no code', async () => {
		const fetch = vi.fn<typeof globalThis.fetch>(async () => new Response('', { status: 404 }));

		expect(await codeForHost(fetch, 'unknown.example.com', 0)).toBeNull();
	});
});
