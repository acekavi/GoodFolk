/** The housekeeper app's IndexedDB: the offline `queue` of changes and `kv`, its saved snapshots. */
const NAME = 'goodfolk-hk';

let opened: Promise<IDBDatabase> | undefined;

export function openDb(): Promise<IDBDatabase> {
	opened ??= new Promise((resolve, reject) => {
		const request = indexedDB.open(NAME, 1);
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

/** Runs `work` on one object store in a transaction and resolves with its request's result. */
export async function withStore<T>(
	store: 'queue' | 'kv',
	mode: IDBTransactionMode,
	work: (store: IDBObjectStore) => IDBRequest<T>
): Promise<T> {
	const db = await openDb();
	return new Promise((resolve, reject) => {
		const request = work(db.transaction(store, mode).objectStore(store));
		request.onsuccess = () => resolve(request.result);
		request.onerror = () => reject(request.error);
	});
}
