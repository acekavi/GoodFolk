import { SvelteMap } from 'svelte/reactivity';

/**
 * Commands in flight, by key (a row's id, a form's name), so a screen disables only what a running
 * command affects instead of the whole page. A key stays busy for as long as any of its overlapping
 * commands are running, counted by reference so one settling early doesn't clear a key another command
 * still holds.
 */
export class Pending {
	#counts = new SvelteMap<string, number>();

	has(key: string): boolean {
		return (this.#counts.get(key) ?? 0) > 0;
	}

	/** Runs `command` with `key` marked as running until it settles. */
	async run<T>(key: string, command: () => Promise<T>): Promise<T> {
		this.#counts.set(key, (this.#counts.get(key) ?? 0) + 1);
		try {
			return await command();
		} finally {
			const count = (this.#counts.get(key) ?? 1) - 1;
			if (count > 0) this.#counts.set(key, count);
			else this.#counts.delete(key);
		}
	}
}
