import type { QueryClient } from '@tanstack/svelte-query';
import { sameOwner, type Owner } from './hk-db';
import { toApiError } from './api/problem';
import { rest } from './api/rest';
import { hkQueue, type NewItem, type QueueItem, type Send, type WhoAmI } from './hk-queue';
import { snapshots } from './hk-snapshot';
import { ownerOf } from './session';
import { hkKey, issuesKey, reportIssue, setCondition } from './housekeeping';

type WithoutOwner<T> = T extends unknown ? Omit<T, 'owner'> : never;

/** Who the session belongs to right now, asked of the server and never answered from a cache. */
const whoAmI: WhoAmI = async () => {
	const { data, error, response } = await rest.GET('/api/v1/me', { cache: 'no-store' });
	if (!response.ok || !data) throw toApiError(error, response.status);
	return ownerOf(data);
};

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
		const { stopped, current } = await hkQueue.replay(send, owner, whoAmI);
		this.signIn = stopped === 'sign-in';
		if (current) {
			// The session is someone else's now (another window signed in): nothing was sent, and what is
			// shown and sent from here on is theirs.
			this.owner = current;
			void this.#client?.invalidateQueries({ queryKey: ['me'] });
		}
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

	/** Removes what `owner` kept on the device: their queue, saved lists and the note that they signed in. */
	async wipe(owner: Owner) {
		await Promise.allSettled([hkQueue.discardOwned(owner), snapshots.wipeFor(owner)]);
		await this.refresh();
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

/** Forgets `owner`'s offline work and choices on this device, for their sign-out; other users' stay. */
export async function wipeDevice(owner: Owner) {
	await sync.wipe(owner);
	try {
		localStorage.removeItem(REMEMBERED_PROPERTY);
	} catch {
		// Nothing was remembered.
	}
}
