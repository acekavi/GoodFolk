/** RFC 9457 problem details, as returned by every API error. */
export interface Problem {
	type: string;
	title: string;
	status: number;
	detail?: string;
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
