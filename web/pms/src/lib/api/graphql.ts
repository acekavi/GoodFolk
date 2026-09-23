import type { TypedDocumentString } from './gql/graphql';
import { ApiError, toApiError } from './problem';

interface GraphQLResult<T> {
	data?: T;
	errors?: { message: string }[];
}

/** Runs a read-only GraphQL query. */
export async function query<TResult, TVariables>(
	document: TypedDocumentString<TResult, TVariables>,
	variables?: TVariables,
	signal?: AbortSignal
): Promise<TResult> {
	const response = await fetch('/graphql', {
		method: 'POST',
		headers: { 'content-type': 'application/json', 'x-goodfolk-csrf': '1' },
		body: JSON.stringify({ query: document.toString(), variables }),
		signal
	});
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
