import { QueryClient } from '@tanstack/svelte-query';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { connectEvents } from '../events';
import { query } from './graphql';
import type { TypedDocumentString } from './gql/graphql';
import { rest } from './rest';
import { onUnauthorized, reportStatus } from './unauthorized';

const respond = (status: number) => async () =>
	new Response(JSON.stringify({ type: 'about:blank', title: 'x', status }), {
		status,
		headers: { 'content-type': 'application/problem+json' }
	});

describe('a 401 signs the tab out', () => {
	afterEach(() => vi.unstubAllGlobals());

	it('calls the registered handler on a 401 only, until it is unregistered', () => {
		const signOut = vi.fn();
		const unregister = onUnauthorized(signOut);

		reportStatus(200);
		reportStatus(403);
		expect(signOut).not.toHaveBeenCalled();
		reportStatus(401);
		expect(signOut).toHaveBeenCalledTimes(1);

		unregister();
		reportStatus(401);
		expect(signOut).toHaveBeenCalledTimes(1);
	});

	it('hears a 401 from a REST call and from a GraphQL query', async () => {
		const signOut = vi.fn();
		const unregister = onUnauthorized(signOut);

		await rest.GET('/api/v1/me', { baseUrl: 'http://localhost', fetch: respond(200) });
		expect(signOut).not.toHaveBeenCalled();
		await rest.GET('/api/v1/me', { baseUrl: 'http://localhost', fetch: respond(401) });
		expect(signOut).toHaveBeenCalledTimes(1);

		vi.stubGlobal('fetch', vi.fn(respond(401)));
		const document = {
			__meta__: { hash: 'h' },
			toString: () => '{ x }'
		} as unknown as TypedDocumentString<unknown, unknown>;
		await expect(query(document)).rejects.toThrow();
		expect(signOut).toHaveBeenCalledTimes(2);
		unregister();
	});

	it('reports a stream the browser gave up on, and not one that is reconnecting', () => {
		class FakeEventSource extends EventTarget {
			static readonly CLOSED = 2;
			static last: FakeEventSource;
			readyState = 0;
			constructor() {
				super();
				FakeEventSource.last = this;
			}
			close() {}
		}
		vi.stubGlobal('EventSource', FakeEventSource);
		const onClosed = vi.fn();

		const disconnect = connectEvents(new QueryClient(), undefined, onClosed);
		FakeEventSource.last.dispatchEvent(new Event('error'));
		expect(onClosed).not.toHaveBeenCalled();
		FakeEventSource.last.readyState = FakeEventSource.CLOSED;
		FakeEventSource.last.dispatchEvent(new Event('error'));
		expect(onClosed).toHaveBeenCalledTimes(1);
		disconnect();
	});
});
