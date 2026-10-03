import type { QueryClient } from '@tanstack/svelte-query';
import { sameOwner, type Owner } from './hk-db';
import { hkQueue, type NewItem, type QueueItem, type Send } from './hk-queue';
import { snapshots } from './hk-snapshot';
import { hkKey, issuesKey, reportIssue, setCondition } from './housekeeping';

type WithoutOwner<T> = T extends unknown ? Omit<T, 'owner'> : never;

/** Sends one queued item to the API. A condition change answers with the room's new version. */
const send: Send = async (item) => {
	if (item.kind === 'issue') {
		await reportIssue(item.propertyId, item.roomId, item.severity, item.description, item.key);
		return;
	}
	if (item.version === null) throw new Error('a queued change has no version to be based on');
	const row = await setCondition(item.propertyId, item.roomId, item.condition, item.version);
	return { version: row.version };
};

/**
 * The housekeeper app's view of the offline queue: what waits, whether there is a connection, and whether
 * the session needs signing in again. The layout `attach`es the query client and drives `replay` on
 * reconnect and focus; screens `enqueue`.
 */
class HkSync {
	items = $state<QueueItem[]>([]);
	online = $state(true);
	/** Replay stopped at a 401: the queue is kept, and the user has to sign in. */
	signIn = $state(false);
	/** The signed-in user, from the session itself: the only one whose items are ever sent. */
	owner = $state.raw<Owner | null>(null);
	/** The user who last signed in on this device, known without a connection. Not proof of a session. */
	remembered = $state.raw<Owner | null | undefined>(undefined);
	#client: QueryClient | undefined;

	/** Whose work is shown: the signed-in user's, or offline the last user's. */
	readonly viewer = $derived(this.owner ?? this.remembered ?? null);
	readonly mine = $derived(this.items.filter((item) => sameOwner(item.owner, this.viewer)));
	readonly waiting = $derived(this.mine.filter((item) => item.status === 'waiting'));
	readonly conflicts = $derived(this.mine.filter((item) => item.status === 'conflict'));
	/** Items another user left on this device; never sent, shown only as a count. */
	readonly others = $derived(
		this.owner ? this.items.filter((item) => !sameOwner(item.owner, this.owner)) : []
	);

	attach(client: QueryClient) {
		this.#client = client;
	}

	async refresh() {
		try {
			this.items = await hkQueue.items();
		} catch {
			this.items = [];
		}
	}

	async enqueue(item: WithoutOwner<NewItem>) {
		const owner = this.viewer;
		if (!owner) throw new Error('nobody is signed in');
		await hkQueue.enqueue({ ...item, owner } as NewItem);
		await this.refresh();
		void this.replay();
	}

	/** Sends what waits, in order, when there is a connection; then refreshes what the changes touched. */
	async replay() {
		const owner = this.owner;
		if (!this.online || !owner) return;
		await this.refresh();
		if (this.waiting.length === 0) return;
		const properties = new Set(this.waiting.map((item) => item.propertyId));
		const { stopped } = await hkQueue.replay(send, owner);
		this.signIn = stopped === 'sign-in';
		await this.refresh();
		for (const property of properties) {
			void this.#client?.invalidateQueries({ queryKey: hkKey(property) });
			void this.#client?.invalidateQueries({ queryKey: issuesKey(property) });
		}
	}

	/** Puts a conflict back in the queue against the room's `version` now, and sends it. */
	async retry(id: number, version: number) {
		await hkQueue.retry(id, version);
		await this.refresh();
		await this.replay();
	}

	async discardOthers() {
		if (!this.owner) return;
		await hkQueue.discardOthers(this.owner);
		await this.refresh();
	}

	/** Empties the device of everything the app kept: the queue, the saved lists and who signed in. */
	async wipe() {
		await Promise.allSettled([hkQueue.clear(), snapshots.wipe()]);
		this.items = [];
		this.owner = null;
		this.remembered = null;
		this.signIn = false;
	}

	async discard(id: number) {
		await hkQueue.discard(id);
		await this.refresh();
	}
}

export const sync = new HkSync();

/** The device's `localStorage` entry for the property the housekeeper app opened last. */
export const REMEMBERED_PROPERTY = 'goodfolk.hk.property';

/** Forgets the user's offline work and choices on this device, for sign-out. */
export async function wipeDevice() {
	await sync.wipe();
	try {
		localStorage.removeItem(REMEMBERED_PROPERTY);
	} catch {
		// Nothing was remembered.
	}
}
