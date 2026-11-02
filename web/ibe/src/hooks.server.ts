import type { Handle, HandleFetch, HandleServerError } from '@sveltejs/kit';
import { env } from '$env/dynamic/private';
import { apiOrigin } from '$lib/config';
import { toApi } from '$lib/server/api';

/** Where this server reaches the API (`IBE_API_ORIGIN`): a private address in production, the public one by default. */
function serverApiOrigin(): string {
	return (env.IBE_API_ORIGIN || apiOrigin()).replace(/\/$/, '');
}

/** A load's API answers are inlined for the browser with `content-length`, which openapi-fetch reads. */
export const handle: Handle = ({ event, resolve }) =>
	resolve(event, { filterSerializedResponseHeaders: (name) => name === 'content-length' });

/** A server render's API calls go to `IBE_API_ORIGIN`, on behalf of the guest (see `toApi`). */
export const handleFetch: HandleFetch = async ({ event, request, fetch }) =>
	fetch(
		toApi(
			request,
			apiOrigin(),
			serverApiOrigin(),
			event.getClientAddress(),
			env.IBE_SSR_SECRET || undefined
		)
	);

/** Logged by route, never by address: a manage link's address carries its token. */
export const handleError: HandleServerError = ({ error, event, status }) => {
	if (status !== 404) {
		const message = error instanceof Error ? error.message : String(error);
		console.error(
			`${event.request.method} ${event.route.id ?? '(no route)'}: ${status} ${message}`
		);
	}
	return { message: status === 404 ? 'Not found' : 'Something went wrong' };
};
