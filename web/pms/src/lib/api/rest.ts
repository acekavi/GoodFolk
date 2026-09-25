import createClient from 'openapi-fetch';
import type { paths } from './openapi';
import { ApiError, toApiError } from './problem';

/** REST client for commands. Same origin, so the session cookie is sent automatically. */
export const rest = createClient<paths>({
	baseUrl: '',
	headers: { 'x-goodfolk-csrf': '1' }
});

/** A fresh key per user action; reuse it only when retrying that same action. */
export function idempotencyKey(): string {
	return crypto.randomUUID();
}

/** The data of an openapi-fetch result, or its problem thrown as an ApiError. */
export function unwrap<T>(result: { data?: T; error?: unknown; response: Response }): T {
	if (!result.response.ok) throw toApiError(result.error, result.response.status);
	return result.data as T;
}

/** The `If-Match` header for an update of the resource at `version`. */
export function ifMatch(version: number) {
	return { 'If-Match': `"${version}"` };
}

/**
 * Idempotency keys for one create form. Resending the same body (a double-click or a retry) reuses
 * the key, so it cannot create twice; an edited body is a different request and gets a new key.
 * Call `reset()` after a success so the next create starts fresh, and `failed(err)` when a submit
 * fails: a definitive client error (4xx) rotates the key, so an unchanged resubmit is a new request
 * rather than a replay of the stored error; a network error, a 5xx or a 409 "still in progress"
 * keeps it, since the first request may yet succeed.
 */
export function formKeys() {
	let key = idempotencyKey();
	let last = '';
	function reset() {
		key = idempotencyKey();
		last = '';
	}
	return {
		keyFor(body: unknown): string {
			const serialized = JSON.stringify(body);
			if (last && serialized !== last) key = idempotencyKey();
			last = serialized;
			return key;
		},
		reset,
		failed(err: unknown) {
			if (!(err instanceof ApiError) || err.status < 400 || err.status > 499) return;
			if (err.status === 409 && err.problem.detail?.includes('in progress')) return;
			reset();
		}
	};
}
