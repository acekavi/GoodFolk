/**
 * A `401` from the API means the session has ended: the member was removed, the session expired, or it was signed
 * out elsewhere. The API clients report every response's status here, and the signed-in shell registers what to do
 * about it (sign the tab out), so a `401` from any call is handled in one place.
 */
let handler: (() => void) | null = null;

/** Calls `signOut` on the next `401`, until the returned function unregisters it. */
export function onUnauthorized(signOut: () => void): () => void {
	handler = signOut;
	return () => {
		if (handler === signOut) handler = null;
	};
}

/** Called by the API clients with each response's status. */
export function reportStatus(status: number): void {
	if (status === 401) handler?.();
}
