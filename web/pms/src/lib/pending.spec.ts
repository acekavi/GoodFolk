import { describe, expect, it } from 'vitest';
import { Pending } from './pending.svelte';

describe('Pending', () => {
	it('marks a key as running until its command settles', async () => {
		const pending = new Pending();
		let finish = () => {};
		const running = pending.run('room-1', () => new Promise<void>((resolve) => (finish = resolve)));

		expect(pending.has('room-1')).toBe(true);
		expect(pending.has('room-2')).toBe(false);
		finish();
		await running;
		expect(pending.has('room-1')).toBe(false);
	});

	it('clears the key when the command fails, and passes the failure on', async () => {
		const pending = new Pending();

		await expect(pending.run('order', () => Promise.reject(new Error('refused')))).rejects.toThrow(
			'refused'
		);
		expect(pending.has('order')).toBe(false);
	});

	it('keeps other keys running while one finishes', async () => {
		const pending = new Pending();
		let finishFirst = () => {};
		const first = pending.run('a', () => new Promise<void>((resolve) => (finishFirst = resolve)));
		const second = pending.run('b', () => new Promise<never>(() => {}));

		finishFirst();
		await first;

		expect([pending.has('a'), pending.has('b')]).toEqual([false, true]);
		void second;
	});

	it('stays busy for overlapping commands on the same key until all of them settle', async () => {
		const pending = new Pending();
		let finishFirst = () => {};
		let finishSecond = () => {};
		const first = pending.run(
			'order',
			() => new Promise<void>((resolve) => (finishFirst = resolve))
		);
		const second = pending.run(
			'order',
			() => new Promise<void>((resolve) => (finishSecond = resolve))
		);

		finishFirst();
		await first;
		expect(pending.has('order')).toBe(true);

		finishSecond();
		await second;
		expect(pending.has('order')).toBe(false);
	});
});
