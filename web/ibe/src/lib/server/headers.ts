/**
 * The security headers every page and endpoint of the booking site carries:
 * - no other site may frame it (`frame-ancestors 'none'`, and `X-Frame-Options` for browsers without CSP 3), so a
 *   booking or a cancellation cannot be clickjacked;
 * - responses are never sniffed as another type;
 * - a link out sends only the site's origin, never a page's address. A page that sets its own `Referrer-Policy` (the
 *   manage page, whose address carries a token, sends none) keeps it.
 *
 * A full Content-Security-Policy (`script-src` and the rest) is a follow-up (ROADMAP "Security follow-ups"): the boot
 * script is rewritten after SvelteKit renders it (`paintFirst`), so SvelteKit's own CSP hashes would not match it, and
 * the policy must also allow Turnstile and the media origin.
 */
export const SECURITY_HEADERS: Readonly<Record<string, string>> = {
	'content-security-policy': "frame-ancestors 'none'",
	'x-frame-options': 'DENY',
	'x-content-type-options': 'nosniff',
	'referrer-policy': 'strict-origin-when-cross-origin'
};

/** Adds the `SECURITY_HEADERS` that `headers` does not already set. */
export function withSecurityHeaders(headers: Headers): void {
	for (const [name, value] of Object.entries(SECURITY_HEADERS)) {
		if (!headers.has(name)) headers.set(name, value);
	}
}
