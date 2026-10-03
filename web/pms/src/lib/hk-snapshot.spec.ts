import { describe, expect, it } from 'vitest';
import type { Owner } from './hk-db';
import { createSnapshots, memoryKv, type Snapshot } from './hk-snapshot';
import { createQueue, memoryStore } from './hk-queue';

const A: Owner = { userId: 'a', tenantId: 't' };
const B: Owner = { userId: 'b', tenantId: 't' };
const elsewhere: Owner = { userId: 'a', tenantId: 'other' };

const snapshot: Snapshot = {
	owner: A,
	propertyId: 'p',
	propertyName: 'Hotel',
	businessDate: '2026-10-03',
	savedAt: 1,
	supervise: true,
	rooms: [],
	typeCodes: []
};

describe('saved lists', () => {
	it('are shown to their owner', async () => {
		const saved = createSnapshots(memoryKv());
		await saved.save(snapshot);
		expect(await saved.load('p', A)).toEqual(snapshot);
		expect(await saved.load(null, A)).toEqual(snapshot);
	});

	it('are never shown to another user, in another tenant, or to no one', async () => {
		const saved = createSnapshots(memoryKv());
		await saved.save(snapshot);
		expect(await saved.load('p', B)).toBeUndefined();
		expect(await saved.load(null, B)).toBeUndefined();
		expect(await saved.load('p', elsewhere)).toBeUndefined();
		expect(await saved.load('p', null)).toBeUndefined();
	});

	it('keep the rooms to report for to their owner as well', async () => {
		const saved = createSnapshots(memoryKv());
		await saved.saveChoices('p', A, [{ roomId: 'r', number: '101' }]);
		expect(await saved.loadChoices('p', A)).toEqual([{ roomId: 'r', number: '101' }]);
		expect(await saved.loadChoices('p', B)).toBeUndefined();
	});

	it('sign-out clears both stores', async () => {
		const kv = memoryKv();
		const saved = createSnapshots(kv);
		const queue = createQueue(memoryStore());
		await saved.save(snapshot);
		await saved.saveOwner(A);
		await queue.enqueue({
			kind: 'issue',
			owner: A,
			propertyId: 'p',
			roomId: 'r',
			roomNumber: '101',
			severity: 'LOW',
			description: 'x'
		});
		await saved.wipe();
		await queue.clear();
		expect(await saved.load('p', A)).toBeUndefined();
		expect(await saved.loadOwner()).toBeUndefined();
		expect(await queue.items()).toEqual([]);
	});
});
