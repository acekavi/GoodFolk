// See https://svelte.dev/docs/kit/types#app.d.ts
// for information about these interfaces
declare global {
	namespace App {
		// interface Error {}
		// interface Locals {}
		// interface PageData {}
		// interface PageState {}
		// interface Platform {}
	}

	/** Cloudflare Turnstile's browser API, once its script has loaded (book page only). */
	interface Window {
		turnstile?: {
			render(
				element: HTMLElement,
				options: {
					sitekey: string;
					action?: string;
					callback: (token: string) => void;
					'expired-callback': () => void;
					'error-callback': () => void;
				}
			): string;
			reset(widget: string): void;
			remove(widget: string): void;
		};
	}
}

export {};
