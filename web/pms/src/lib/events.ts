import type { QueryClient } from '@tanstack/svelte-query';
import { rangeTouchesMonth, tapeEventTiles } from './tape';

/**
 * Applies one server-sent event to the query cache. `invalidate` carries the cache keys that
 * changed; `resync` means events were missed, so everything is refetched.
 */
export function applyEvent(client: QueryClient, type: string, data: string): void {
	if (type === 'resync') {
		void client.invalidateQueries();
		return;
	}
	if (type === 'invalidate') {
		const keys: string[] = JSON.parse(data);
		for (const key of keys) {
			if (key.startsWith('tape:')) invalidateTape(client, key);
			else void client.invalidateQueries({ queryKey: [key] });
		}
	}
}

/** Tape tiles are cached under `['tape', property, tileStart, pageKey]`; only those touching the event's month refetch. */
function invalidateTape(client: QueryClient, eventKey: string): void {
	const propertyId = eventKey.split(':')[1];
	const tape = { queryKey: ['tape', propertyId] };
	const cached = client
		.getQueryCache()
		.findAll(tape)
		.map((query) => query.queryKey[2] as string);
	const stale = new Set(tapeEventTiles(eventKey, propertyId, cached));
	if (stale.size > 0) {
		void client.invalidateQueries({
			...tape,
			predicate: (query) => stale.has(query.queryKey[2] as string)
		});
	}
	// The Needs a room lists, cached under `['tape-unassigned', property, from, to]`, with a day in the month.
	const month = eventKey.split(':')[2];
	void client.invalidateQueries({
		queryKey: ['tape-unassigned', propertyId],
		predicate: (query) =>
			rangeTouchesMonth(query.queryKey[2] as string, query.queryKey[3] as string, month)
	});
}

/** Keeps the cache fresh while the app is open. Returns a function that disconnects. */
export function connectEvents(client: QueryClient, propertyId?: string): () => void {
	const url = propertyId
		? `/api/v1/events?property=${encodeURIComponent(propertyId)}`
		: '/api/v1/events';
	const source = new EventSource(url);
	for (const type of ['invalidate', 'resync']) {
		source.addEventListener(type, (event) =>
			applyEvent(client, type, (event as MessageEvent).data)
		);
	}
	// EventSource reconnects by itself; anything missed while disconnected is refetched on reconnect.
	source.addEventListener('open', () => applyEvent(client, 'resync', '{}'));
	return () => source.close();
}
