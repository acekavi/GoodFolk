import { describe, expect, it, vi } from 'vitest';
import {
	cachedHosts,
	codeForHost,
	customRoute,
	HOST_CACHE_MS,
	HOST_CACHE_SIZE,
	MISS_CACHE_MS,
	onBookingDomain
} from './host';

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

	it('remember a host with no site only briefly', async () => {
		const fetch = vi.fn<typeof globalThis.fetch>(async () => new Response('', { status: 404 }));

		await codeForHost(fetch, 'soon.example.com', 0);
		await codeForHost(fetch, 'soon.example.com', MISS_CACHE_MS - 1);
		expect(fetch).toHaveBeenCalledOnce();
		await codeForHost(fetch, 'soon.example.com', MISS_CACHE_MS);
		expect(fetch).toHaveBeenCalledTimes(2);
	});

	it('that cannot be a site’s custom host are never looked up', async () => {
		const fetch = vi.fn<typeof globalThis.fetch>();

		for (const host of [
			'localhost',
			'GAL.example/x',
			'a..b.example',
			'-bad.example',
			`${'a'.repeat(64)}.example`
		]) {
			expect(await codeForHost(fetch, host, 0)).toBeNull();
		}
		expect(fetch).not.toHaveBeenCalled();
	});

	it('are kept up to a bound, the least recently used dropped first', async () => {
		const fetch = vi.fn<typeof globalThis.fetch>(
			async () => new Response(JSON.stringify({ code: 'GAL' }), { status: 200 })
		);
		const now = 10 * HOST_CACHE_MS;
		await codeForHost(fetch, 'first.example.com', now);

		for (let n = 0; n < HOST_CACHE_SIZE + 50; n++) {
			await codeForHost(fetch, `host-${n}.example.com`, now);
			// The first host stays in use, so it is never the least recently used.
			await codeForHost(fetch, 'first.example.com', now);
			expect(cachedHosts()).toBeLessThanOrEqual(HOST_CACHE_SIZE);
		}

		expect(cachedHosts()).toBe(HOST_CACHE_SIZE);
		const asked = fetch.mock.calls.length;
		await codeForHost(fetch, 'first.example.com', now);
		await codeForHost(fetch, 'host-0.example.com', now);
		expect(fetch.mock.calls.length).toBe(asked + 1);
	});
});
