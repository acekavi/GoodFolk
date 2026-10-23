import { QueryClient } from '@tanstack/svelte-query';
import { describe, expect, it, vi } from 'vitest';
import { applyEvent } from './events';
import { foliosKey } from './folio';
import { addDays } from './inventory';
import { auditRunsKey, dailyStatsKey, nightAuditKey, opsAlertsKey } from './night-audit';
import { tapeKey, tileStartFor, unassignedKey } from './tape';

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

	describe('tape events', () => {
		const invalidated = (client: QueryClient, key: readonly unknown[]) =>
			client.getQueryState(key)?.isInvalidated ?? false;

		it('invalidates only the cached tiles of that property that touch the event month', () => {
			const client = new QueryClient();
			const inMonth = tileStartFor('2026-09-10');
			const straddling = tileStartFor('2026-09-30');
			const otherMonth = tileStartFor('2026-12-01');
			const keys = [
				tapeKey('p1', inMonth, 'a,b'),
				tapeKey('p1', inMonth, 'c,d'),
				tapeKey('p1', straddling, 'a,b'),
				tapeKey('p1', otherMonth, 'a,b'),
				tapeKey('p2', inMonth, 'a,b')
			];
			for (const key of keys) client.setQueryData(key, { stays: [], blocks: [] });

			applyEvent(client, 'invalidate', '["tape:p1:2026-09"]');

			expect(keys.map((key) => invalidated(client, key))).toEqual([true, true, true, false, false]);
		});

		it('also refetches a tile that straddles into the event month from the month before', () => {
			const client = new QueryClient();
			const straddling = tileStartFor('2026-09-30');
			expect(addDays(straddling, 13).slice(0, 7)).toBe('2026-10');
			const key = tapeKey('p1', straddling, 'a');
			client.setQueryData(key, { stays: [], blocks: [] });

			applyEvent(client, 'invalidate', '["tape:p1:2026-10"]');

			expect(invalidated(client, key)).toBe(true);
		});

		it('handles tape keys beside other keys in one event, and leaves other queries alone', () => {
			const client = new QueryClient();
			const tile = tapeKey('p1', tileStartFor('2026-09-10'), 'a');
			client.setQueryData(tile, { stays: [], blocks: [] });
			client.setQueryData(['properties'], []);
			client.setQueryData(['inventory:p1:2026-09'], []);

			applyEvent(client, 'invalidate', '["tape:p1:2026-09","properties"]');

			expect(invalidated(client, tile)).toBe(true);
			expect(invalidated(client, ['properties'])).toBe(true);
			expect(invalidated(client, ['inventory:p1:2026-09'])).toBe(false);
		});

		it('refetches the Needs a room lists of that property with a day in the event month', () => {
			const client = new QueryClient();
			const keys = [
				unassignedKey('p1', '2026-09-28', '2026-10-26'),
				unassignedKey('p1', '2026-11-02', '2026-11-30'),
				unassignedKey('p2', '2026-09-28', '2026-10-26')
			];
			for (const key of keys) client.setQueryData(key, []);

			applyEvent(client, 'invalidate', '["tape:p1:2026-10"]');

			expect(keys.map((key) => invalidated(client, key))).toEqual([true, false, false]);
		});

		it('invalidates cached tiles on resync', () => {
			const client = new QueryClient();
			const key = tapeKey('p1', tileStartFor('2026-09-10'), 'a');
			client.setQueryData(key, { stays: [], blocks: [] });

			applyEvent(client, 'resync', '{}');

			expect(invalidated(client, key)).toBe(true);
		});
	});

	it('refetches the folios of every reservation holding the folio of a folio event', () => {
		const client = new QueryClient();
		const holding = foliosKey('p1', 'r1', null);
		const other = foliosKey('p1', 'r2', 'a1');
		client.setQueryData(holding, [{ id: 'f1' }, { id: 'f2' }]);
		client.setQueryData(other, [{ id: 'f3' }]);

		applyEvent(client, 'invalidate', '["folio:f2"]');

		expect(client.getQueryState(holding)?.isInvalidated).toBe(true);
		expect(client.getQueryState(other)?.isInvalidated).toBe(false);
	});

	it("refetches the property's review, runs, statistics and alerts on an audit event", () => {
		const client = new QueryClient();
		const keys = [
			nightAuditKey('p1'),
			auditRunsKey('p1', 1),
			dailyStatsKey('p1', '2026-09-01', '2026-09-30'),
			opsAlertsKey('p1'),
			nightAuditKey('p2'),
			auditRunsKey('p2', 1)
		];
		for (const key of keys) client.setQueryData(key, []);

		applyEvent(client, 'invalidate', '["audit:p1"]');

		expect(keys.map((key) => client.getQueryState(key)?.isInvalidated)).toEqual([
			true,
			true,
			true,
			true,
			false,
			false
		]);
	});

	it("refetches the property's review on a reservations event", () => {
		const client = new QueryClient();
		client.setQueryData(nightAuditKey('p1'), {});
		client.setQueryData(nightAuditKey('p2'), {});

		applyEvent(client, 'invalidate', '["reservations:p1"]');

		expect(client.getQueryState(nightAuditKey('p1'))?.isInvalidated).toBe(true);
		expect(client.getQueryState(nightAuditKey('p2'))?.isInvalidated).toBe(false);
	});
});
