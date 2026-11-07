import { searchFromUrl } from '$lib/search';
import type { PageLoad } from './$types';

/**
 * The results, as a universal load: the first page is rendered on the server (calls to `IBE_API_ORIGIN`, inlined for
 * the browser), and a changed search is asked by the browser itself of `PUBLIC_API_ORIGIN`, so core-api's per-IP
 * search limit counts the guest. Nothing is searched before the residency question is answered.
 */
export const load: PageLoad = async ({ fetch, url, parent }) => {
	const { site } = await parent();
	return {
		searched: await searchFromUrl(fetch, site.code, url, site.business_date),
		soldOut: url.searchParams.has('sold_out')
	};
};
