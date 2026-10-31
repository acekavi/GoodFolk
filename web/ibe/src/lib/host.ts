import { apiOrigin, bookingOrigin } from './config';

/** How long a custom host's code is kept before it is asked again: a host moved to another site follows soon. */
export const HOST_CACHE_MS = 60_000;

/** Whether `url` is on the booking domain, where a site is `/<CODE>`; any other host is a site's custom host. */
export function onBookingDomain(url: URL, origin: string = bookingOrigin()): boolean {
	return url.host === new URL(origin).host;
}

const codes = new Map<string, { code: string | null; until: number }>();

/**
 * The code of the enabled site at the custom host `host`, or `null` when there is none, asked of the API's
 * `by-host` and kept for [`HOST_CACHE_MS`], so a page costs no lookup most of the time.
 */
export async function codeForHost(
	fetch: typeof globalThis.fetch,
	host: string,
	now: number = Date.now()
): Promise<string | null> {
	const kept = codes.get(host);
	if (kept && kept.until > now) return kept.code;
	const response = await fetch(
		`${apiOrigin()}/api/public/v1/ibe/by-host/${encodeURIComponent(host)}`
	);
	if (response.status !== 200 && response.status !== 404)
		throw new Error(`by-host answered ${response.status}`);
	const code = response.status === 200 ? ((await response.json()) as { code: string }).code : null;
	codes.set(host, { code, until: now + HOST_CACHE_MS });
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
