import { sameOwner, withStore, type Owner } from './hk-db';
import type { MyRoom } from './housekeeping';

/** What My rooms needs to show without a connection, as last read online. */
export interface Snapshot {
	/** Whose list this is; it is shown to no one else. */
	owner: Owner;
	propertyId: string;
	propertyName: string;
	/** The property's business date the list is for, `YYYY-MM-DD`. */
	businessDate: string;
	/** When it was taken, in milliseconds since the epoch. */
	savedAt: number;
	/** Whether the user may set any condition, not just the housekeeper's two steps. */
	supervise: boolean;
	rooms: MyRoom[];
	/** Room type id to code, for the type shown on each room. */
	typeCodes: [string, string][];
}

export interface RoomChoice {
	roomId: string;
	number: string;
}

export interface Kv {
	get(key: string): Promise<unknown>;
	set(key: string, value: unknown): Promise<void>;
	remove(key: string): Promise<void>;
	keys(): Promise<string[]>;
}

export function idbKv(): Kv {
	return {
		get: (key) => withStore('kv', 'readonly', (store) => store.get(key)),
		set: async (key, value) => {
			await withStore('kv', 'readwrite', (store) => store.put(value, key));
		},
		remove: async (key) => {
			await withStore('kv', 'readwrite', (store) => store.delete(key));
		},
		keys: async () => (await withStore('kv', 'readonly', (store) => store.getAllKeys())).map(String)
	};
}

export function memoryKv(): Kv {
	const map = new Map<string, unknown>();
	return {
		get: async (key) => map.get(key),
		set: async (key, value) => {
			map.set(key, value);
		},
		remove: async (key) => {
			map.delete(key);
		},
		keys: async () => [...map.keys()]
	};
}

/**
 * The saved lists, each kept with its owner and handed back only to that owner. Saving and loading never
 * throw: storage can be unavailable or full, and the live list does not depend on it.
 */
export function createSnapshots(kv: Kv) {
	// Every key holds its owner's user and tenant, so one user's save never replaces another's, and a
	// user's data can be found (and removed) by prefix.
	const mark = (owner: Owner) => `${owner.userId}:${owner.tenantId}`;
	const snapshotKey = (owner: Owner, propertyId: string) => `snapshot:${mark(owner)}:${propertyId}`;
	const lastKey = (owner: Owner) => `last:${mark(owner)}`;
	const choicesKey = (owner: Owner, propertyId: string) => `choices:${mark(owner)}:${propertyId}`;

	async function safe<T>(work: () => Promise<T>, otherwise: T): Promise<T> {
		try {
			return await work();
		} catch {
			return otherwise;
		}
	}
	return {
		/** Saves `snapshot` as its owner's for the property, and as the last one taken. */
		save: (snapshot: Snapshot) =>
			safe(async () => {
				await kv.set(snapshotKey(snapshot.owner, snapshot.propertyId), snapshot);
				await kv.set(lastKey(snapshot.owner), snapshot.propertyId);
			}, undefined),
		/** `owner`'s saved list of `propertyId`, or of the last property when none is given. */
		load: (propertyId: string | null, owner: Owner | null) =>
			safe(async () => {
				if (!owner) return undefined;
				const id = propertyId ?? ((await kv.get(lastKey(owner))) as string | undefined);
				const found = id
					? ((await kv.get(snapshotKey(owner, id))) as Snapshot | undefined)
					: undefined;
				return found && sameOwner(found.owner, owner) ? found : undefined;
			}, undefined),
		/** The rooms an issue can be reported for, saved while the Report issue page has them. */
		saveChoices: (propertyId: string, owner: Owner, choices: RoomChoice[]) =>
			safe(() => kv.set(choicesKey(owner, propertyId), { owner, choices }), undefined),
		loadChoices: (propertyId: string, owner: Owner | null) =>
			safe(async () => {
				if (!owner) return undefined;
				const found = (await kv.get(choicesKey(owner, propertyId))) as
					{ owner: Owner; choices: RoomChoice[] } | undefined;
				return found && sameOwner(found.owner, owner) ? found.choices : undefined;
			}, undefined),
		/** The user who last signed in on this device, who may be shown their saved list without a connection. */
		saveOwner: (owner: Owner) => safe(() => kv.set('owner', owner), undefined),
		loadOwner: () => safe(async () => (await kv.get('owner')) as Owner | undefined, undefined),
		/** Forgets what is saved for `owner`, for their sign-out; other users' saved lists stay. */
		wipeFor: (owner: Owner) =>
			safe(async () => {
				const tag = `:${mark(owner)}`;
				for (const key of await kv.keys()) {
					const mine = ['snapshot', 'last', 'choices'].some((kind) =>
						key.startsWith(`${kind}${tag}`)
					);
					if (mine) await kv.remove(key);
				}
				if (sameOwner((await kv.get('owner')) as Owner | undefined, owner))
					await kv.remove('owner');
			}, undefined)
	};
}

/** The device's saved lists. */
export const snapshots = createSnapshots(idbKv());
