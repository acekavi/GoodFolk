import createClient from 'openapi-fetch';
import type { paths } from './openapi';
import { toApiError } from './problem';

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
