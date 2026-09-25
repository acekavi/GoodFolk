import { describe, expect, it } from 'vitest';
import { ApiError } from './problem';
import { formKeys, ifMatch, unwrap } from './rest';

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

describe('formKeys', () => {
	it('returns the same key for the same body twice', () => {
		const keys = formKeys();
		const body = { code: 'STD', name: 'Standard' };
		const key1 = keys.keyFor(body);
		const key2 = keys.keyFor(body);
		expect(key1).toBe(key2);
	});

	it('returns a new key for a changed body', () => {
		const keys = formKeys();
		const key1 = keys.keyFor({ code: 'STD', name: 'Standard' });
		const key2 = keys.keyFor({ code: 'STD', name: 'Standard Room' });
		expect(key1).not.toBe(key2);
	});

	it('returns a new key after reset() even for the same body', () => {
		const keys = formKeys();
		const body = { code: 'STD', name: 'Standard' };
		const key1 = keys.keyFor(body);
		keys.reset();
		const key2 = keys.keyFor(body);
		expect(key1).not.toBe(key2);
	});
});

describe('formKeys failed()', () => {
	const problem = (status: number, detail: string) =>
		new ApiError({ type: 'about:blank', title: 'Error', status, detail });
	const body = { code: 'STD', name: 'Standard' };

	/** The key an unchanged resubmit gets after the first attempt failed with `err`. */
	function keysAround(err: unknown): [string, string] {
		const keys = formKeys();
		const first = keys.keyFor(body);
		keys.failed(err);
		return [first, keys.keyFor(body)];
	}

	it('rotates the key after a definitive client error, so a resubmit is a new request', () => {
		const [first, second] = keysAround(problem(422, 'a room type with code STD already exists'));
		expect(second).not.toBe(first);
	});

	it('keeps the key after a network error, so a retry cannot create twice', () => {
		const [first, second] = keysAround(new TypeError('Failed to fetch'));
		expect(second).toBe(first);
	});

	it('keeps the key after a server error', () => {
		const [first, second] = keysAround(problem(500, 'Internal error'));
		expect(second).toBe(first);
	});

	it('keeps the key while the first request with it is still in progress', () => {
		const inProgress = problem(
			409,
			'a request with this Idempotency-Key is still in progress; retry shortly'
		);
		const [first, second] = keysAround(inProgress);
		expect(second).toBe(first);
	});

	it('rotates the key after any other conflict', () => {
		const [first, second] = keysAround(problem(409, 'the room is already blocked'));
		expect(second).not.toBe(first);
	});
});
