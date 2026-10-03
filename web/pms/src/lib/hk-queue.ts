import { ApiError } from './api/problem';
import { idempotencyKey } from './api/rest';
import { sameOwner, withStore, type Owner } from './hk-db';
import type { Condition, Severity } from './housekeeping';

/**
 * The housekeeper app's offline queue. Condition changes and issue reports made without a connection wait
 * here, in IndexedDB, and `replay` sends them strictly in the order they were made.
 *
 * - A `412`, `409`, `403` (or any other client error) is that item's own conflict: it stays, marked, for the
 *   housekeeper to retry or discard, and replay carries on with the rest.
 * - A network error (a `TypeError` from `fetch`) or a server error stops replay with the queue intact; a `401` stops it with a request to
 *   sign in. Nothing is lost either way.
 * - An issue carries an `Idempotency-Key` from the moment it is queued, so a replay that reached the server
 *   twice creates it once.
 */

interface Base {
	/** Assigned by the store; ascending, so it is also the queue order. */
	id: number;
	/** Who made the change; replay sends it only under this user. */
	owner: Owner;
	propertyId: string;
	roomId: string;
	roomNumber: string;
	createdAt: number;
	/** The property's business date of the list it was made against, when that was known. */
	businessDate?: string;
	status: 'waiting' | 'conflict';
	/** The HTTP status that made this a conflict. */
	conflictStatus?: number;
	/** The refusal was a 403 or 412 for an item made on an earlier business date: the day rolled. */
	dateRolled?: boolean;
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
	| Omit<ConditionItem, 'id' | 'createdAt' | 'status' | 'conflictStatus' | 'dateRolled'>
	| (Omit<IssueItem, 'id' | 'createdAt' | 'status' | 'conflictStatus' | 'dateRolled' | 'key'> & {
			key?: string;
	  });

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
	/**
	 * Why replay stopped early, or `null` when it went through every waiting item. `other-user` means the
	 * session now belongs to someone else (`current`); nothing was sent.
	 */
	stopped: 'network' | 'sign-in' | 'other-user' | null;
	current?: Owner;
}

/** Asks the server, not a cache, who the session belongs to; rejects like a request does. */
export type WhoAmI = () => Promise<Owner>;

/** The property's business date now, or `undefined` when it cannot be read. */
export type BusinessDateOf = (propertyId: string) => Promise<string | undefined>;

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
	// `fetch` rejects with a TypeError when there is no connection.
	if (error instanceof TypeError) return { outcome: 'network' };
	// Anything else is not a connection problem: retrying would only fail again, so it is a conflict.
	if (!(error instanceof ApiError)) return { outcome: 'conflict' };
	const { status } = error;
	if (status === 401) return { outcome: 'sign-in' };
	// The first attempt of this very key is still running: wait, do not call it a conflict.
	if (status === 409 && error.problem.detail?.includes('in progress'))
		return { outcome: 'network' };
	if (status === 408 || status === 429 || status >= 500) return { outcome: 'network' };
	return { outcome: 'conflict', status };
}

/**
 * Whether a refusal is the business date rolling: a 403 (no longer assigned) or 412 (version moved on) for
 * an item made against an earlier date. Unknown dates are never taken for a roll.
 */
async function rolled(item: QueueItem, status: number | undefined, today?: BusinessDateOf) {
	if ((status !== 403 && status !== 412) || !item.businessDate || !today) return false;
	try {
		const now = await today(item.propertyId);
		return !!now && now > item.businessDate;
	} catch {
		return false;
	}
}

const sameRoom = (a: QueueItem, b: QueueItem) =>
	sameOwner(a.owner, b.owner) && a.propertyId === b.propertyId && a.roomId === b.roomId;

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

	async function run(
		send: Send,
		owner: Owner,
		whoAmI: WhoAmI,
		today?: BusinessDateOf
	): Promise<ReplayResult> {
		for (;;) {
			const item = (await store.all()).find(
				(candidate) => candidate.status === 'waiting' && sameOwner(candidate.owner, owner)
			);
			if (!item) return { stopped: null };
			// The session may have changed hands since the owner was last read (another window signed in as
			// someone else), so ask just before every send: an item is only ever sent as the user who made it.
			let current: Owner;
			try {
				current = await whoAmI();
			} catch (error) {
				return { stopped: classify(error).outcome === 'sign-in' ? 'sign-in' : 'network' };
			}
			if (!sameOwner(current, owner)) return { stopped: 'other-user', current };
			// Nothing to base it on (what it was queued behind is gone): it cannot be sent.
			if (item.kind === 'condition' && item.version === null) {
				await store.put({ ...item, status: 'conflict' });
				await cascade(item, undefined);
				continue;
			}
			try {
				const sent = await send(item);
				// The next change learns its version before this one leaves the queue, so a crash between
				// the two writes cannot strand it without one.
				await passVersion(item, sent?.version);
				await store.remove(item.id);
			} catch (error) {
				const { outcome, status } = classify(error);
				if (outcome !== 'conflict') return { stopped: outcome };
				await store.put({
					...item,
					status: 'conflict',
					conflictStatus: status,
					dateRolled: (await rolled(item, status, today)) || undefined
				});
				await cascade(item, status);
			}
		}
	}

	return {
		enqueue,
		items: () => store.all(),
		/**
		 * Sends `owner`'s waiting items in order, never anyone else's. A second call while one runs shares it.
		 * `today` lets a 403 or 412 be told apart as the business date rolling.
		 */
		replay(
			send: Send,
			owner: Owner,
			whoAmI: WhoAmI,
			today?: BusinessDateOf
		): Promise<ReplayResult> {
			running ??= run(send, owner, whoAmI, today).finally(() => {
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
				await store.put({
					...item,
					status: 'waiting',
					conflictStatus: undefined,
					dateRolled: undefined
				});
				return;
			}
			const earlier = all.some(
				(other) => other.id < id && other.kind === 'condition' && sameRoom(item, other)
			);
			await store.put({
				...item,
				version: earlier ? null : version,
				status: 'waiting',
				conflictStatus: undefined,
				dateRolled: undefined
			});
			for (const other of all) {
				if (
					other.id > id &&
					other.kind === 'condition' &&
					other.status === 'conflict' &&
					other.version === null &&
					sameRoom(item, other)
				) {
					await store.put({
						...other,
						status: 'waiting',
						conflictStatus: undefined,
						dateRolled: undefined
					});
				}
			}
		},
		discard: (id: number) => store.remove(id),
		/** Drops every item that is not `owner`'s. */
		async discardOthers(owner: Owner) {
			for (const item of await store.all()) {
				if (!sameOwner(item.owner, owner)) await store.remove(item.id);
			}
		},
		/** Drops every item that is `owner`'s, for their sign-out. */
		async discardOwned(owner: Owner) {
			for (const item of await store.all()) {
				if (sameOwner(item.owner, owner)) await store.remove(item.id);
			}
		}
	};
}

/** The device's queue. */
export const hkQueue = createQueue(idbStore());
