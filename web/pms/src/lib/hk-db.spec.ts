import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

type Callback = (() => void) | null;

/** A transaction whose request succeeds and whose completion or abort the test fires by hand. */
function fakeIndexedDb() {
	const transaction = {
		oncomplete: null as Callback,
		onerror: null as Callback,
		onabort: null as Callback,
		error: null as DOMException | null,
		objectStore: () => ({})
	};
	const request = {
		onsuccess: null as Callback,
		onerror: null as Callback,
		result: 'stored',
		error: null
	};
	const db = { transaction: () => transaction };
	vi.stubGlobal('indexedDB', {
		open: () => {
			const opening: { onsuccess: Callback; result: unknown } = { onsuccess: null, result: db };
			queueMicrotask(() => opening.onsuccess?.());
			return opening;
		}
	});
	return { transaction, request };
}

describe('withStore', () => {
	beforeEach(() => vi.resetModules());
	afterEach(() => vi.unstubAllGlobals());

	it('resolves when the transaction completes, not when the request succeeds', async () => {
		const { transaction, request } = fakeIndexedDb();
		const { withStore } = await import('./hk-db');
		let settled = false;
		const result = withStore(
			'kv',
			'readwrite',
			() => request as unknown as IDBRequest<string>
		).then((value) => {
			settled = true;
			return value;
		});
		await vi.waitFor(() => expect(transaction.oncomplete).toBeTypeOf('function'));

		request.onsuccess?.();
		await Promise.resolve();
		expect(settled).toBe(false);

		transaction.oncomplete?.();
		expect(await result).toBe('stored');
	});

	it('rejects when the transaction aborts after the request succeeded', async () => {
		const { transaction, request } = fakeIndexedDb();
		const { withStore } = await import('./hk-db');
		const result = withStore('kv', 'readwrite', () => request as unknown as IDBRequest<string>);
		await vi.waitFor(() => expect(transaction.onabort).toBeTypeOf('function'));

		request.onsuccess?.();
		transaction.error = new DOMException('quota', 'QuotaExceededError');
		transaction.onabort?.();

		await expect(result).rejects.toThrow('quota');
	});
});
