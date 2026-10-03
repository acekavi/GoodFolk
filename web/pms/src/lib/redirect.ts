/**
 * The page to return to after signing in, from the login route's `redirect` parameter: only a path on this
 * origin, so a crafted link cannot send a signed-in user to another site. The path is checked again after
 * the URL parser has normalised it, since `/.//evil.com` becomes `//evil.com`.
 */
export function safeRedirect(raw: string | null, origin: string): string | null {
	if (!raw || !raw.startsWith('/') || raw.includes('\\')) return null;
	try {
		const url = new URL(raw, origin);
		const path = url.pathname + url.search + url.hash;
		if (url.origin !== origin || path.startsWith('//') || path.startsWith('/\\')) return null;
		return new URL(path, origin).origin === origin ? path : null;
	} catch {
		return null;
	}
}
