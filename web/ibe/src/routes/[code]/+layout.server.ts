import { error } from '@sveltejs/kit';
import { fetchSite } from '$lib/api';
import { onBookingDomain } from '$lib/host';
import type { LayoutServerLoad } from './$types';

/**
 * The property's site, or a plain `404` when it has none or it is turned off. `base` is what the site's own links
 * start with: `/<CODE>` on the booking domain, nothing on its custom host.
 */
export const load: LayoutServerLoad = async ({ params, fetch, url }) => {
	const site = await fetchSite(fetch, params.code);
	if (!site) error(404, 'No booking site here');
	const base = onBookingDomain(url) ? `/${site.code}` : '';
	return { site, base, origin: url.origin };
};
