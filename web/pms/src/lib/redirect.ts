/**
 * The page to return to after signing in, from the login route's `redirect` parameter: only a path on this
 * origin, so a crafted link cannot send a signed-in user to another site.
 */
export function safeRedirect(raw: string | null, origin: string): string | null {
	if (!raw || !raw.startsWith('/') || raw.startsWith('//') || raw.includes('\\')) return null;
	try {
		const url = new URL(raw, origin);
		return url.origin === origin ? url.pathname + url.search + url.hash : null;
	} catch {
		return null;
	}
}
