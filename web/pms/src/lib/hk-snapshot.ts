import { clearStore, sameOwner, withStore, type Owner } from './hk-db';
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
	clear(): Promise<void>;
}

export function idbKv(): Kv {
	return {
		get: (key) => withStore('kv', 'readonly', (store) => store.get(key)),
		set: async (key, value) => {
			await withStore('kv', 'readwrite', (store) => store.put(value, key));
		},
		clear: () => clearStore('kv')
	};
}

export function memoryKv(): Kv {
	const map = new Map<string, unknown>();
	return {
		get: async (key) => map.get(key),
		set: async (key, value) => {
			map.set(key, value);
		},
		clear: async () => {
			map.clear();
		}
	};
}

/**
 * The saved lists, each kept with its owner and handed back only to that owner. Saving and loading never
 * throw: storage can be unavailable or full, and the live list does not depend on it.
 */
export function createSnapshots(kv: Kv) {
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
				await kv.set(`snapshot:${snapshot.propertyId}`, snapshot);
				await kv.set('snapshot:last', snapshot.propertyId);
			}, undefined),
		/** `owner`'s saved list of `propertyId`, or of the last property when none is given. */
		load: (propertyId: string | null, owner: Owner | null) =>
			safe(async () => {
				const id = propertyId ?? ((await kv.get('snapshot:last')) as string | undefined);
				const found = id ? ((await kv.get(`snapshot:${id}`)) as Snapshot | undefined) : undefined;
				return found && sameOwner(found.owner, owner) ? found : undefined;
			}, undefined),
		/** The rooms an issue can be reported for, saved while the Report issue page has them. */
		saveChoices: (propertyId: string, owner: Owner, choices: RoomChoice[]) =>
			safe(() => kv.set(`choices:${propertyId}`, { owner, choices }), undefined),
		loadChoices: (propertyId: string, owner: Owner | null) =>
			safe(async () => {
				const found = (await kv.get(`choices:${propertyId}`)) as
					{ owner: Owner; choices: RoomChoice[] } | undefined;
				return found && sameOwner(found.owner, owner) ? found.choices : undefined;
			}, undefined),
		/** The user who last signed in on this device, who may be shown their saved list without a connection. */
		saveOwner: (owner: Owner) => safe(() => kv.set('owner', owner), undefined),
		loadOwner: () => safe(async () => (await kv.get('owner')) as Owner | undefined, undefined),
		/** Forgets everything, for sign-out. */
		wipe: () => kv.clear()
	};
}

/** The device's saved lists. */
export const snapshots = createSnapshots(idbKv());
