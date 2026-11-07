import { apiOrigin, bookingOrigin } from './config';

/** How long a custom host's code is kept before it is asked again: a host moved to another site follows soon. */
export const HOST_CACHE_MS = 60_000;
/** How long a host with no enabled site is remembered as such: briefly, so a site just turned on shows soon. */
export const MISS_CACHE_MS = 10_000;
/** The most hosts kept: any client can send any `Host`, so the cache is bounded, least recently used first out. */
export const HOST_CACHE_SIZE = 1_000;

/** What core-api takes as a custom host (`custom_host`): two or more labels of letters, digits and inner hyphens. */
const HOST_NAME = /^([a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$/;

/** Whether `host` could be a site's custom host at all; nothing else is looked up. */
export function plausibleHost(host: string): boolean {
	return host.length <= 253 && HOST_NAME.test(host);
}

/** Whether `url` is on the booking domain, where a site is `/<CODE>`; any other host is a site's custom host. */
export function onBookingDomain(url: URL, origin: string = bookingOrigin()): boolean {
	return url.host === new URL(origin).host;
}

/** Hosts by code, in least-recently-used order (a `Map` keeps insertion order; a hit moves its host to the end). */
const codes = new Map<string, { code: string | null; until: number }>();

/** How many hosts are kept now. */
export function cachedHosts(): number {
	return codes.size;
}

/** Keeps `host`'s answer, making room by dropping expired hosts, then the least recently used. */
function remember(host: string, code: string | null, now: number) {
	codes.delete(host);
	if (codes.size >= HOST_CACHE_SIZE) {
		for (const [kept, { until }] of codes) if (until <= now) codes.delete(kept);
	}
	while (codes.size >= HOST_CACHE_SIZE) codes.delete(codes.keys().next().value!);
	codes.set(host, { code, until: now + (code === null ? MISS_CACHE_MS : HOST_CACHE_MS) });
}

/**
 * The code of the enabled site at the custom host `host`, or `null` when there is none, asked of the API's `by-host`
 * and kept for [`HOST_CACHE_MS`] ([`MISS_CACHE_MS`] for none), so a page costs no lookup most of the time. A host that
 * cannot be a custom host has none, and nothing is asked.
 */
export async function codeForHost(
	fetch: typeof globalThis.fetch,
	host: string,
	now: number = Date.now()
): Promise<string | null> {
	if (!plausibleHost(host)) return null;
	const kept = codes.get(host);
	if (kept && kept.until > now) {
		codes.delete(host);
		codes.set(host, kept);
		return kept.code;
	}
	const response = await fetch(
		`${apiOrigin()}/api/public/v1/ibe/by-host/${encodeURIComponent(host)}`
	);
	if (response.status !== 200 && response.status !== 404)
		throw new Error(`by-host answered ${response.status}`);
	const code = response.status === 200 ? ((await response.json()) as { code: string }).code : null;
	remember(host, code, now);
	return code;
}

/**
 * The route a custom host's address is served by: `/rooms` on the site's host is `/<CODE>/rooms`. A host with no
 * enabled site is served by `/`, which is no page: it finds nothing, not another property's site.
 */
export function customRoute(pathname: string, code: string | null): string {
	if (code === null) return '/';
	return pathname === '/' ? `/${code}` : `/${code}${pathname}`;
}
