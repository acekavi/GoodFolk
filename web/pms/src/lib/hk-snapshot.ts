import { withStore } from './hk-db';
import type { MyRoom } from './housekeeping';

/** What My rooms needs to show without a connection, as last read online. */
export interface Snapshot {
	propertyId: string;
	propertyName: string;
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

const get = <T>(key: string) =>
	withStore<T | undefined>('kv', 'readonly', (store) => store.get(key));
const set = (key: string, value: unknown) =>
	withStore('kv', 'readwrite', (store) => store.put(value, key));

/** Saves `snapshot` as the property's and as the last one taken. Never throws: saving is a convenience. */
export async function saveSnapshot(snapshot: Snapshot): Promise<void> {
	try {
		await set(`snapshot:${snapshot.propertyId}`, snapshot);
		await set('snapshot:last', snapshot.propertyId);
	} catch {
		// Storage can be unavailable or full; the live list does not depend on it.
	}
}

/** The saved snapshot of `propertyId`, or of the last property when none is given. */
export async function loadSnapshot(propertyId: string | null): Promise<Snapshot | undefined> {
	try {
		const id = propertyId ?? (await get<string>('snapshot:last'));
		return id ? await get<Snapshot>(`snapshot:${id}`) : undefined;
	} catch {
		return undefined;
	}
}

/** The rooms an issue can be reported for, saved while the Report issue page has them. */
export async function saveChoices(propertyId: string, choices: RoomChoice[]): Promise<void> {
	try {
		await set(`choices:${propertyId}`, choices);
	} catch {
		// As for `saveSnapshot`.
	}
}

export async function loadChoices(propertyId: string): Promise<RoomChoice[] | undefined> {
	try {
		return await get<RoomChoice[]>(`choices:${propertyId}`);
	} catch {
		return undefined;
	}
}
