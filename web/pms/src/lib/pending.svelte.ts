import { SvelteSet } from 'svelte/reactivity';

/**
 * Commands in flight, by key (a row's id, a form's name), so a screen disables only what a running
 * command affects instead of the whole page.
 */
export class Pending {
	#keys = new SvelteSet<string>();

	has(key: string): boolean {
		return this.#keys.has(key);
	}

	/** Runs `command` with `key` marked as running until it settles. */
	async run<T>(key: string, command: () => Promise<T>): Promise<T> {
		this.#keys.add(key);
		try {
			return await command();
		} finally {
			this.#keys.delete(key);
		}
	}
}
