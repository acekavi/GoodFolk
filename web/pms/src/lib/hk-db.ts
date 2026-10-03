/** The housekeeper app's IndexedDB: the offline `queue` of changes and `kv`, its saved snapshots. */
const NAME = 'goodfolk-hk';

let opened: Promise<IDBDatabase> | undefined;

export function openDb(): Promise<IDBDatabase> {
	opened ??= new Promise<IDBDatabase>((resolve, reject) => {
		const request = indexedDB.open(NAME, 1);
		// Another tab holding an older version open must not hang sign-out: fail, and let callers carry on.
		request.onblocked = () => {
			opened = undefined;
			reject(new Error('the housekeeper database is in use by another tab'));
		};
		request.onupgradeneeded = () => {
			request.result.createObjectStore('queue', { keyPath: 'id', autoIncrement: true });
			request.result.createObjectStore('kv');
		};
		request.onsuccess = () => resolve(request.result);
		request.onerror = () => {
			opened = undefined;
			reject(request.error);
		};
	});
	return opened;
}

/**
 * Runs `work` on one object store in a transaction and resolves with its request's result once the transaction
 * has completed, so a resolved write is durable; rejects if the request fails or the transaction aborts.
 */
export async function withStore<T>(
	store: 'queue' | 'kv',
	mode: IDBTransactionMode,
	work: (store: IDBObjectStore) => IDBRequest<T>
): Promise<T> {
	const db = await openDb();
	return new Promise((resolve, reject) => {
		const transaction = db.transaction(store, mode);
		const request = work(transaction.objectStore(store));
		transaction.oncomplete = () => resolve(request.result);
		transaction.onerror = () => reject(transaction.error ?? request.error);
		transaction.onabort = () =>
			reject(transaction.error ?? new Error('the transaction was aborted'));
	});
}

/** Whose data this is: everything the app keeps on the device is tagged with one, and shown or sent only to them. */
export interface Owner {
	userId: string;
	tenantId: string;
}

export function sameOwner(a: Owner | null | undefined, b: Owner | null | undefined): boolean {
	return !!a && !!b && a.userId === b.userId && a.tenantId === b.tenantId;
}
