import type { QueryClient } from '@tanstack/svelte-query';
import { hkQueue, type NewItem, type QueueItem, type Send } from './hk-queue';
import { hkKey, issuesKey, reportIssue, setCondition } from './housekeeping';

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
	#client: QueryClient | undefined;

	readonly waiting = $derived(this.items.filter((item) => item.status === 'waiting'));
	readonly conflicts = $derived(this.items.filter((item) => item.status === 'conflict'));

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

	async enqueue(item: NewItem) {
		await hkQueue.enqueue(item);
		await this.refresh();
		void this.replay();
	}

	/** Sends what waits, in order, when there is a connection; then refreshes what the changes touched. */
	async replay() {
		if (!this.online) return;
		await this.refresh();
		if (this.waiting.length === 0) return;
		const properties = new Set(this.waiting.map((item) => item.propertyId));
		const { stopped } = await hkQueue.replay(send);
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

	async discard(id: number) {
		await hkQueue.discard(id);
		await this.refresh();
	}
}

export const sync = new HkSync();
