import createClient from 'openapi-fetch';
import type { paths } from './openapi';

/** REST client for commands. Same origin, so the session cookie is sent automatically. */
export const rest = createClient<paths>({
	baseUrl: '',
	headers: { 'x-goodfolk-csrf': '1' }
});

/** A fresh key per user action; reuse it only when retrying that same action. */
export function idempotencyKey(): string {
	return crypto.randomUUID();
}
