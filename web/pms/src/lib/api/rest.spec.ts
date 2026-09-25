import { describe, expect, it } from 'vitest';
import { ApiError } from './problem';
import { ifMatch, unwrap } from './rest';

describe('unwrap', () => {
	it('returns the data of a successful response', () => {
		expect(unwrap({ data: { id: 'x' }, response: new Response(null, { status: 201 }) })).toEqual({
			id: 'x'
		});
	});

	it('throws the problem of a failed response as an ApiError', () => {
		const problem = { type: 'about:blank', title: 'Conflict', status: 409, detail: 'taken' };

		const failed = () => unwrap({ error: problem, response: new Response(null, { status: 409 }) });

		expect(failed).toThrow(ApiError);
		expect(failed).toThrow('taken');
	});
});

describe('ifMatch', () => {
	it('quotes the version as a strong ETag', () => {
		expect(ifMatch(3)).toEqual({ 'If-Match': '"3"' });
	});
});
