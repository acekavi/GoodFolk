import type { QueryClient } from '@tanstack/svelte-query';

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
		for (const key of keys) void client.invalidateQueries({ queryKey: [key] });
	}
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
