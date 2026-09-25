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
