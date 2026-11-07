import { publicApi, type ManageView } from '$lib/api';
import type { PageServerLoad } from './$types';

/** What the manage page can show: the booking, or why it cannot. */
export type Managed =
	| { kind: 'booking'; view: ManageView }
	| { kind: 'no_token' }
	| { kind: 'gone' }
	| { kind: 'too_many' };

/**
 * The booking of the manage link's token, read on the server only, and never kept: no cache stores the page, and
 * no link on it sends its address (with the token) on as a referrer. The API is sent the token in a `POST`'s body,
 * so it is in no URL of that call for a request log to record.
 */
export const load: PageServerLoad = async ({ fetch, url, parent, setHeaders }) => {
	setHeaders({ 'cache-control': 'no-store', 'referrer-policy': 'no-referrer' });
	const { site } = await parent();
	return { managed: await managed(fetch, site.code, url.searchParams.get('token')) };
};

async function managed(
	fetch: typeof globalThis.fetch,
	code: string,
	token: string | null
): Promise<Managed> {
	if (!token) return { kind: 'no_token' };
	const { data, response } = await publicApi(fetch).POST(
		'/api/public/v1/ibe/{code}/bookings/manage',
		{ params: { path: { code } }, body: { token } }
	);
	if (data) return { kind: 'booking', view: data };
	if (response.status === 410) return { kind: 'gone' };
	if (response.status === 429) return { kind: 'too_many' };
	throw new Error(`the manage page answered ${response.status}`);
}
