import { redirect } from '@sveltejs/kit';
import { picksFromUrl, pricePicks } from '$lib/booking';
import { searchFromUrl, stayQuery } from '$lib/search';
import type { PageLoad } from './$types';

/**
 * The stay and the rooms chosen, priced again by a fresh search (never by the address). A choice no longer offered
 * sends the guest back to the results with "That room was just booked".
 */
export const load: PageLoad = async ({ fetch, url, parent }) => {
	const { site, base } = await parent();
	const searched = await searchFromUrl(fetch, site.code, url, site.business_date);
	if (searched.kind !== 'found') redirect(303, `${base}/rooms${url.search}`);
	const picks = picksFromUrl(url, searched.stay.rooms.length);
	if (!picks) redirect(303, `${base}/rooms?${stayQuery(searched.stay)}`);
	const rooms = pricePicks(searched.result, picks);
	if (!rooms) redirect(303, `${base}/rooms?${stayQuery(searched.stay)}&sold_out=1`);
	return { stay: searched.stay, rooms };
};
