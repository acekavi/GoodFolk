import type { Handle, HandleFetch, HandleServerError, ServerInit } from '@sveltejs/kit';
import { env } from '$env/dynamic/private';
import { apiOrigin } from '$lib/config';
import { clientAddress, isApiCall, startupProblem, startupWarning, toApi } from '$lib/server/api';
import { paintFirst } from '$lib/server/hydrate';

/** Where this server reaches the API (`IBE_API_ORIGIN`): a private address in production, the public one by default. */
function serverApiOrigin(): string {
	return (env.IBE_API_ORIGIN || apiOrigin()).replace(/\/$/, '');
}

/**
 * A load's API answers are inlined for the browser with `content-length`, which openapi-fetch reads. The app's
 * scripts are not preloaded (CSS is inlined, so nothing else is), and its boot waits for the first frame (`paintFirst`):
 * the page's largest paint does not share a phone's connection with them.
 */
export const handle: Handle = ({ event, resolve }) =>
	resolve(event, {
		filterSerializedResponseHeaders: (name) => name === 'content-length',
		preload: ({ type }) => type === 'css',
		transformPageChunk: ({ html }) => paintFirst(html)
	});

/**
 * Fails fast: a production server without the SSR secret would count every guest's render against itself. Warns of a
 * production server that cannot tell its guests apart (`startupWarning`).
 */
export const init: ServerInit = () => {
	const problem = startupProblem(process.env.NODE_ENV, env.IBE_SSR_SECRET || undefined);
	if (problem) throw new Error(problem);
	const warning = startupWarning(process.env.NODE_ENV, env.ADDRESS_HEADER || undefined);
	if (warning) console.warn(warning);
};

/**
 * A server render's API calls go to `IBE_API_ORIGIN`, on behalf of the guest (see `toApi`). The guest's address is
 * looked up only for those, and only with the secret to prove it.
 */
export const handleFetch: HandleFetch = async ({ event, request, fetch }) => {
	const secret = env.IBE_SSR_SECRET || undefined;
	const guest =
		secret && isApiCall(request, apiOrigin())
			? clientAddress(() => event.getClientAddress())
			: null;
	return fetch(toApi(request, apiOrigin(), serverApiOrigin(), guest, secret));
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
