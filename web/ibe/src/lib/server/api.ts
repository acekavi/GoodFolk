/** Whether `request` is a call to the public API at `publicOrigin`. */
export function isApiCall(request: Request, publicOrigin: string): boolean {
	return request.url.startsWith(`${publicOrigin}/`);
}

/**
 * A server render's call to the public API, as this server sends it: to `serverOrigin` (`IBE_API_ORIGIN`) instead of
 * the public origin, and, when `ssrSecret` (`IBE_SSR_SECRET`, shared with core-api) is set and the guest's IP is known,
 * naming the guest the page is rendered for in `X-IBE-Client-IP`, proved by `X-IBE-SSR-Secret`. core-api then counts
 * the call against the guest's per-IP limits rather than this server's. A call to anywhere else is sent as it is.
 */
export function toApi(
	request: Request,
	publicOrigin: string,
	serverOrigin: string,
	clientIp: string | null,
	ssrSecret: string | undefined
): Request {
	if (!isApiCall(request, publicOrigin)) return request;
	const forwarded = new Request(serverOrigin + request.url.slice(publicOrigin.length), request);
	if (ssrSecret && clientIp) {
		forwarded.headers.set('x-ibe-client-ip', clientIp);
		forwarded.headers.set('x-ibe-ssr-secret', ssrSecret);
	}
	return forwarded;
}

/**
 * The guest's IP as adapter-node finds it (`getClientAddress`), or `null` when it cannot: with
 * `ADDRESS_HEADER=CF-Connecting-IP`, a request that did not come through Cloudflare has none. The call then names no
 * guest, rather than the page failing.
 */
export function clientAddress(get: () => string): string | null {
	try {
		return get();
	} catch (error) {
		console.warn(
			`the guest's address is unknown, so the API counts this render against the site: ${error}`
		);
		return null;
	}
}

/** Why this server must not start: in production, without `IBE_SSR_SECRET`, every guest's render would share its limits. */
export function startupProblem(
	nodeEnv: string | undefined,
	ssrSecret: string | undefined
): string | null {
	return nodeEnv === 'production' && !ssrSecret
		? 'IBE_SSR_SECRET is required in production: without it every server render counts against the site’s own per-IP limits'
		: null;
}
