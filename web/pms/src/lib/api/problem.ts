/** RFC 9457 problem details, as returned by every API error. */
export interface Problem {
	type: string;
	title: string;
	status: number;
	detail?: string;
	/** Extension members, such as `conflicts` on a 409 for an overlapping room block. */
	[extension: string]: unknown;
}

export class ApiError extends Error {
	constructor(readonly problem: Problem) {
		super(problem.detail ?? problem.title);
	}

	get status(): number {
		return this.problem.status;
	}
}

export function isProblem(value: unknown): value is Problem {
	return (
		typeof value === 'object' &&
		value !== null &&
		typeof (value as Problem).title === 'string' &&
		typeof (value as Problem).status === 'number'
	);
}

/** Turns any failed response body into an ApiError. */
export function toApiError(body: unknown, status: number): ApiError {
	return new ApiError(
		isProblem(body) ? body : { type: 'about:blank', title: 'Request failed', status }
	);
}

export const NETWORK_ERROR = 'Network error — check your connection and try again.';

/** A message for any error a request can throw: the API's problem, or a network failure. */
export function errorMessage(error: unknown): string {
	return error instanceof ApiError ? error.message : NETWORK_ERROR;
}
