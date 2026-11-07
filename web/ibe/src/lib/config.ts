import { env } from '$env/dynamic/public';

/** Read at runtime, so one build serves every environment. Each has a development default. */

/** Where browsers reach the public API (`PUBLIC_API_ORIGIN`): bookings and cancels go there from the guest. */
export function apiOrigin(): string {
	return (env.PUBLIC_API_ORIGIN || 'http://localhost:8080').replace(/\/$/, '');
}

/** The booking domain's own origin (`PUBLIC_IBE_ORIGIN`), core-api's `IBE_PUBLIC_ORIGIN`; any other host is custom. */
export function bookingOrigin(): string {
	return (env.PUBLIC_IBE_ORIGIN || 'http://localhost:5174').replace(/\/$/, '');
}

/** Cloudflare Turnstile's site key (`PUBLIC_TURNSTILE_SITEKEY`); the default is its test key, which always passes. */
export function turnstileSiteKey(): string {
	return env.PUBLIC_TURNSTILE_SITEKEY || '1x00000000000000000000AA';
}
