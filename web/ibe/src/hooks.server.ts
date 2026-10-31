import type { Handle, HandleFetch, HandleServerError } from '@sveltejs/kit';
import { env } from '$env/dynamic/private';
import { apiOrigin } from '$lib/config';

/** Where this server reaches the API (`IBE_API_ORIGIN`): a private address in production, the public one by default. */
function serverApiOrigin(): string {
	return (env.IBE_API_ORIGIN || apiOrigin()).replace(/\/$/, '');
}

/** A load's API answers are inlined for the browser with `content-length`, which openapi-fetch reads. */
export const handle: Handle = ({ event, resolve }) =>
	resolve(event, { filterSerializedResponseHeaders: (name) => name === 'content-length' });

/**
 * A server render's API calls go to `IBE_API_ORIGIN` instead of the public origin, carrying the guest's
 * `CF-Connecting-IP` when Cloudflare set one, so core-api's per-IP limits (with `TRUST_CLOUDFLARE_IP=true`) count
 * the guest rather than this server.
 */
export const handleFetch: HandleFetch = async ({ event, request, fetch }) => {
	const origin = apiOrigin();
	if (!request.url.startsWith(`${origin}/`)) return fetch(request);
	const url = serverApiOrigin() + request.url.slice(origin.length);
	const forwarded = new Request(url, request);
	const guest = event.request.headers.get('cf-connecting-ip');
	if (guest) forwarded.headers.set('cf-connecting-ip', guest);
	return fetch(forwarded);
};

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
