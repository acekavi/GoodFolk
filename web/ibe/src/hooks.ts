import type { Reroute } from '@sveltejs/kit';
import { codeForHost, customRoute, onBookingDomain } from '$lib/host';

/** A custom host serves its site's pages at its root: `/rooms` there is `/<CODE>/rooms` on the booking domain. */
export const reroute: Reroute = async ({ url, fetch }) => {
	if (onBookingDomain(url)) return;
	return customRoute(url.pathname, await codeForHost(fetch, url.hostname));
};
