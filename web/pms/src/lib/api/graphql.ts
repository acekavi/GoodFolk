import type { TypedDocumentString } from './gql/graphql';
import { ApiError, toApiError } from './problem';

interface GraphQLResult<T> {
	data?: T;
	errors?: { message: string }[];
}

/**
 * Runs a read-only GraphQL query by its generated document id. Production accepts only those ids; the
 * development server also gets the text, so a document edited since the last codegen still runs.
 */
export async function query<TResult, TVariables>(
	document: TypedDocumentString<TResult, TVariables>,
	variables?: TVariables,
	signal?: AbortSignal
): Promise<TResult> {
	const response = await fetch('/graphql', {
		method: 'POST',
		headers: { 'content-type': 'application/json', 'x-goodfolk-csrf': '1' },
		body: JSON.stringify({
			documentId: document.__meta__?.hash,
			query: import.meta.env.DEV ? document.toString() : undefined,
			variables
		}),
		signal
	});
	// A proxy in front of the API (502, 504) answers with an HTML page, not JSON.
	if (!response.headers.get('content-type')?.includes('json')) {
		throw toApiError(undefined, response.status);
	}
	const body: GraphQLResult<TResult> = await response.json();
	if (!response.ok) throw toApiError(body, response.status);
	if (body.errors?.length || !body.data) {
		throw new ApiError({
			type: 'about:blank',
			title: body.errors?.[0]?.message ?? 'Query failed',
			status: 200
		});
	}
	return body.data;
}
