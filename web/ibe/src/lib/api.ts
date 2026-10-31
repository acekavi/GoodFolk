import createClient from 'openapi-fetch';
import type { components, paths } from './api/openapi';
import { apiOrigin } from './config';

export type SiteView = components['schemas']['SiteView'];
export type SiteRoomType = components['schemas']['SiteRoomType'];
export type SiteMedia = components['schemas']['SiteMedia'];
export type Image = components['schemas']['Image'];
export type SearchResult = components['schemas']['SearchResult'];
export type RoomTypeResult = components['schemas']['RoomTypeResult'];
export type PlanOffer = components['schemas']['PlanOffer'];
export type MealOffer = components['schemas']['MealOffer'];
export type Residency = components['schemas']['Residency'];
export type BookingBody = components['schemas']['BookingBody'];
export type BookingConfirmed = components['schemas']['BookingConfirmed'];
export type ManageView = components['schemas']['ManageView'];

type Fetch = typeof fetch;

/**
 * The public API, called with `fetch`: in a `load`, the one SvelteKit passes it, so a server render's calls are
 * inlined for the browser and go through `handleFetch`, which sends them to the private `IBE_API_ORIGIN`; in the
 * browser, the global one, to `PUBLIC_API_ORIGIN`. No cookies either way.
 */
export function publicApi(fetch: Fetch = globalThis.fetch, origin: string = apiOrigin()) {
	return createClient<paths>({ baseUrl: origin, fetch, credentials: 'omit' });
}

/** RFC 9457 problem details, as every API error is answered with. */
export interface Problem {
	title: string;
	status: number;
	detail?: string;
	[extension: string]: unknown;
}

export function isProblem(value: unknown): value is Problem {
	return (
		typeof value === 'object' &&
		value !== null &&
		typeof (value as Problem).title === 'string' &&
		typeof (value as Problem).status === 'number'
	);
}

/** What the guest is told when a limit of the API is reached (`429`). */
export const TOO_MANY_TRIES = 'Too many tries; wait a minute';
/** What the guest is told when the API cannot be reached. */
export const NETWORK_TROUBLE =
	'We could not reach the hotel’s booking system. Check your connection and try again.';

/** The site of the property coded `code`, or `null` when it has none or it is turned off (`404`). */
export async function fetchSite(fetch: Fetch, code: string): Promise<SiteView | null> {
	const { data, response } = await publicApi(fetch).GET('/api/public/v1/ibe/{code}/site', {
		params: { path: { code } }
	});
	if (response.status === 404) return null;
	if (!data) throw new Error(`the booking site answered ${response.status}`);
	return data;
}
