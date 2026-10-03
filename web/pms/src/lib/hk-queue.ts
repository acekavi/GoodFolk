import { ApiError } from './api/problem';
import { idempotencyKey } from './api/rest';
import { withStore } from './hk-db';
import type { Condition, Severity } from './housekeeping';

/**
 * The housekeeper app's offline queue. Condition changes and issue reports made without a connection wait
 * here, in IndexedDB, and `replay` sends them strictly in the order they were made.
 *
 * - A `412`, `409`, `403` (or any other client error) is that item's own conflict: it stays, marked, for the
 *   housekeeper to retry or discard, and replay carries on with the rest.
 * - A network error or a server error stops replay with the queue intact; a `401` stops it with a request to
 *   sign in. Nothing is lost either way.
 * - An issue carries an `Idempotency-Key` from the moment it is queued, so a replay that reached the server
 *   twice creates it once.
 */

interface Base {
	/** Assigned by the store; ascending, so it is also the queue order. */
	id: number;
	propertyId: string;
	roomId: string;
	roomNumber: string;
	createdAt: number;
	status: 'waiting' | 'conflict';
	/** The HTTP status that made this a conflict. */
	conflictStatus?: number;
}

export interface ConditionItem extends Base {
	kind: 'condition';
	condition: Condition;
	/**
	 * The condition version this change was based on. `null` for a change queued behind another on the same
	 * room: it is based on the version that earlier change produces, filled in when that one is sent.
	 */
	version: number | null;
}

export interface IssueItem extends Base {
	kind: 'issue';
	severity: Severity;
	description: string;
	/** The `Idempotency-Key`; stays the same for every attempt. */
	key: string;
}

export type QueueItem = ConditionItem | IssueItem;

export type NewItem =
	| Omit<ConditionItem, 'id' | 'createdAt' | 'status' | 'conflictStatus'>
	| (Omit<IssueItem, 'id' | 'createdAt' | 'status' | 'conflictStatus' | 'key'> & { key?: string });

/** Sends one item, and answers with the room's new condition version for a condition change. */
export type Send = (item: QueueItem) => Promise<{ version?: number } | void>;

type WithoutId<T> = T extends unknown ? Omit<T, 'id'> : never;

export interface Store {
	/** Every item, in queue order. */
	all(): Promise<QueueItem[]>;
	/** Stores a new item and gives it the next id. */
	add(item: WithoutId<QueueItem>): Promise<QueueItem>;
	put(item: QueueItem): Promise<void>;
	remove(id: number): Promise<void>;
}

export interface ReplayResult {
	/** Why replay stopped early, or `null` when it went through every waiting item. */
	stopped: 'network' | 'sign-in' | null;
}

/** The queue in IndexedDB. */
export function idbStore(): Store {
	return {
		all: async () =>
			(await withStore<QueueItem[]>('queue', 'readonly', (store) => store.getAll())).sort(
				(a, b) => a.id - b.id
			),
		add: async (item) => {
			const id = await withStore<IDBValidKey>('queue', 'readwrite', (store) => store.add(item));
			return { ...item, id: id as number } as QueueItem;
		},
		put: async (item) => {
			await withStore('queue', 'readwrite', (store) => store.put(item));
		},
		remove: async (id) => {
			await withStore('queue', 'readwrite', (store) => store.delete(id));
		}
	};
}

/** An in-memory store, for tests. */
export function memoryStore(): Store {
	const items = new Map<number, QueueItem>();
	let next = 1;
	return {
		all: async () => [...items.values()].sort((a, b) => a.id - b.id).map((item) => ({ ...item })),
		add: async (item) => {
			const stored = { ...item, id: next++ } as QueueItem;
			items.set(stored.id, stored);
			return { ...stored };
		},
		put: async (item) => {
			items.set(item.id, { ...item });
		},
		remove: async (id) => {
			items.delete(id);
		}
	};
}

type Outcome = 'conflict' | 'network' | 'sign-in';

/** What a failed send means for the queue. */
function classify(error: unknown): { outcome: Outcome; status?: number } {
	if (!(error instanceof ApiError)) return { outcome: 'network' };
	const { status } = error;
	if (status === 401) return { outcome: 'sign-in' };
	// The first attempt of this very key is still running: wait, do not call it a conflict.
	if (status === 409 && error.problem.detail?.includes('in progress'))
		return { outcome: 'network' };
	if (status === 429 || status >= 500) return { outcome: 'network' };
	return { outcome: 'conflict', status };
}

const sameRoom = (a: QueueItem, b: QueueItem) =>
	a.propertyId === b.propertyId && a.roomId === b.roomId;

export function createQueue(store: Store) {
	let running: Promise<ReplayResult> | undefined;

	async function enqueue(item: NewItem): Promise<QueueItem> {
		const base = { createdAt: Date.now(), status: 'waiting' as const };
		if (item.kind === 'issue') {
			return store.add({ ...item, ...base, key: item.key ?? idempotencyKey() });
		}
		return store.add({ ...item, ...base });
	}

	/** Gives the first change queued behind `item` on its room the version `item` produced. */
	async function passVersion(item: QueueItem, version: number | undefined) {
		if (item.kind !== 'condition' || version === undefined) return;
		const behind = (await store.all()).find(
			(other) =>
				other.id > item.id &&
				other.kind === 'condition' &&
				other.version === null &&
				sameRoom(item, other)
		) as ConditionItem | undefined;
		if (behind) await store.put({ ...behind, version });
	}

	/** A change behind a conflict has no version to be based on, so it is a conflict too. */
	async function cascade(item: QueueItem, status: number | undefined) {
		if (item.kind !== 'condition') return;
		for (const other of await store.all()) {
			if (
				other.id > item.id &&
				other.kind === 'condition' &&
				other.version === null &&
				sameRoom(item, other)
			) {
				await store.put({ ...other, status: 'conflict', conflictStatus: status });
			}
		}
	}

	async function run(send: Send): Promise<ReplayResult> {
		for (;;) {
			const item = (await store.all()).find((candidate) => candidate.status === 'waiting');
			if (!item) return { stopped: null };
			try {
				const sent = await send(item);
				await store.remove(item.id);
				await passVersion(item, sent?.version);
			} catch (error) {
				const { outcome, status } = classify(error);
				if (outcome !== 'conflict') return { stopped: outcome };
				await store.put({ ...item, status: 'conflict', conflictStatus: status });
				await cascade(item, status);
			}
		}
	}

	return {
		enqueue,
		items: () => store.all(),
		/** Sends every waiting item in order. A second call while one runs shares it. */
		replay(send: Send): Promise<ReplayResult> {
			running ??= run(send).finally(() => {
				running = undefined;
			});
			return running;
		},
		/**
		 * Puts a conflict back in the queue, based on `version` (the room's current one). Changes queued
		 * behind it that were conflicts only because of it go back too.
		 */
		async retry(id: number, version: number) {
			const all = await store.all();
			const item = all.find((candidate) => candidate.id === id);
			if (!item || item.status !== 'conflict') return;
			if (item.kind === 'issue') {
				await store.put({ ...item, status: 'waiting', conflictStatus: undefined });
				return;
			}
			const earlier = all.some(
				(other) => other.id < id && other.kind === 'condition' && sameRoom(item, other)
			);
			await store.put({
				...item,
				version: earlier ? null : version,
				status: 'waiting',
				conflictStatus: undefined
			});
			for (const other of all) {
				if (
					other.id > id &&
					other.kind === 'condition' &&
					other.status === 'conflict' &&
					other.version === null &&
					sameRoom(item, other)
				) {
					await store.put({ ...other, status: 'waiting', conflictStatus: undefined });
				}
			}
		},
		discard: (id: number) => store.remove(id)
	};
}

/** The device's queue. */
export const hkQueue = createQueue(idbStore());
