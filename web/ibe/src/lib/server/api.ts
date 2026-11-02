/**
 * A server render's call to the public API, as this server sends it: to `serverOrigin` (`IBE_API_ORIGIN`) instead of
 * the public origin, and, when `ssrSecret` (`IBE_SSR_SECRET`, shared with core-api) is set, naming the guest the page
 * is rendered for in `X-IBE-Client-IP`, proved by `X-IBE-SSR-Secret`. core-api then counts the call against the guest's
 * per-IP limits rather than this server's. A call to anywhere else is sent as it is.
 */
export function toApi(
	request: Request,
	publicOrigin: string,
	serverOrigin: string,
	clientIp: string,
	ssrSecret: string | undefined
): Request {
	if (!request.url.startsWith(`${publicOrigin}/`)) return request;
	const forwarded = new Request(serverOrigin + request.url.slice(publicOrigin.length), request);
	if (ssrSecret) {
		forwarded.headers.set('x-ibe-client-ip', clientIp);
		forwarded.headers.set('x-ibe-ssr-secret', ssrSecret);
	}
	return forwarded;
}
