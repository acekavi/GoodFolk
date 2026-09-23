import { afterEach, describe, expect, it, vi } from 'vitest';
import { query } from './graphql';
import { ApiError } from './problem';
import type { TypedDocumentString } from './gql/graphql';

const document = '{ properties { id } }' as unknown as TypedDocumentString<unknown, never>;

function respondWith(response: Response) {
	vi.stubGlobal('fetch', vi.fn().mockResolvedValue(response));
}

describe('query', () => {
	afterEach(() => {
		vi.unstubAllGlobals();
	});

	it('turns a non-JSON error page from a proxy into an ApiError with its status', async () => {
		respondWith(
			new Response('<html>502 Bad Gateway</html>', {
				status: 502,
				headers: { 'content-type': 'text/html' }
			})
		);

		const error = await query(document).catch((err: unknown) => err);

		expect(error).toBeInstanceOf(ApiError);
		expect((error as ApiError).status).toBe(502);
	});

	it('uses the problem details of a JSON error', async () => {
		const problem = { type: 'about:blank', title: 'Forbidden', status: 403, detail: 'no tenant' };
		respondWith(
			new Response(JSON.stringify(problem), {
				status: 403,
				headers: { 'content-type': 'application/problem+json' }
			})
		);

		const error = await query(document).catch((err: unknown) => err);

		expect((error as ApiError).message).toBe('no tenant');
	});
});
