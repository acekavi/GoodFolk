// See https://svelte.dev/docs/kit/types#app.d.ts
// for information about these interfaces
declare global {
	namespace App {
		// interface Error {}
		// interface Locals {}
		// interface PageData {}
		/** The reservation the tape chart shows in a modal over itself (shallow routing). */
		interface PageState {
			reservation?: string;
		}
		// interface Platform {}
	}
}

export {};
