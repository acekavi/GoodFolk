import { QueryClient } from '@tanstack/svelte-query';
import { describe, expect, it, vi } from 'vitest';
import { applyEvent } from './events';

describe('applyEvent', () => {
	it('invalidates exactly the keys named by an invalidate event', () => {
		const client = new QueryClient();
		const spy = vi.spyOn(client, 'invalidateQueries');

		applyEvent(client, 'invalidate', '["properties"]');

		expect(spy).toHaveBeenCalledTimes(1);
		expect(spy).toHaveBeenCalledWith({ queryKey: ['properties'] });
	});

	it('invalidates everything on resync', () => {
		const client = new QueryClient();
		const spy = vi.spyOn(client, 'invalidateQueries');

		applyEvent(client, 'resync', '{}');

		expect(spy).toHaveBeenCalledWith();
	});

	it('ignores unknown event types', () => {
		const client = new QueryClient();
		const spy = vi.spyOn(client, 'invalidateQueries');

		applyEvent(client, 'something-else', '[]');

		expect(spy).not.toHaveBeenCalled();
	});
});
