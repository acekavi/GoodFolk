import { describe, expect, it } from 'vitest';
import { ApiError } from './api/problem';
import { createQueue, memoryStore, type QueueItem, type Send } from './hk-queue';

const problem = (status: number) => new ApiError({ type: 'about:blank', title: 'x', status });

function condition(roomId: string, version: number | null = 1) {
	return {
		kind: 'condition' as const,
		propertyId: 'p',
		roomId,
		roomNumber: roomId,
		condition: 'CLEANING' as const,
		version
	};
}

const issue = {
	kind: 'issue' as const,
	propertyId: 'p',
	roomId: 'r1',
	roomNumber: '101',
	severity: 'LOW' as const,
	description: 'Leaking tap'
};

/** A sender that records what it was given and answers per room (`ok` unless `fail` names an error). */
function sender(fail: Record<string, unknown> = {}) {
	const sent: QueueItem[] = [];
	const send: Send = async (item) => {
		sent.push(item);
		if (item.roomId in fail) throw fail[item.roomId];
		return { version: 10 };
	};
	return { sent, send };
}

describe('hk queue', () => {
	it('keeps items in the order they were queued', async () => {
		const queue = createQueue(memoryStore());
		await queue.enqueue(condition('a'));
		await queue.enqueue(issue);
		await queue.enqueue(condition('b'));
		expect((await queue.items()).map((item) => item.roomId)).toEqual(['a', 'r1', 'b']);
	});

	it('replays strictly in order and empties the queue', async () => {
		const queue = createQueue(memoryStore());
		await queue.enqueue(condition('a'));
		await queue.enqueue(condition('b'));
		await queue.enqueue(condition('c'));
		const { sent, send } = sender();
		expect(await queue.replay(send)).toEqual({ stopped: null });
		expect(sent.map((item) => item.roomId)).toEqual(['a', 'b', 'c']);
		expect(await queue.items()).toEqual([]);
	});

	it.each([412, 409, 403])(
		'a %i marks only that item as a conflict and the rest still replay',
		async (status) => {
			const queue = createQueue(memoryStore());
			await queue.enqueue(condition('a'));
			await queue.enqueue(condition('b'));
			const { sent, send } = sender({ a: problem(status) });
			await queue.replay(send);
			expect(sent.map((item) => item.roomId)).toEqual(['a', 'b']);
			const left = await queue.items();
			expect(left).toHaveLength(1);
			expect(left[0]).toMatchObject({ roomId: 'a', status: 'conflict', conflictStatus: status });
		}
	);

	it('does not resend a conflict on the next replay', async () => {
		const queue = createQueue(memoryStore());
		await queue.enqueue(condition('a'));
		await queue.replay(sender({ a: problem(412) }).send);
		const again = sender();
		await queue.replay(again.send);
		expect(again.sent).toEqual([]);
		expect(await queue.items()).toHaveLength(1);
	});

	it('stops at a network error, keeps the queue, and resumes from that item', async () => {
		const queue = createQueue(memoryStore());
		await queue.enqueue(condition('a'));
		await queue.enqueue(condition('b'));
		await queue.enqueue(condition('c'));
		const down = sender({ b: new TypeError('Failed to fetch') });
		expect(await queue.replay(down.send)).toEqual({ stopped: 'network' });
		expect(down.sent.map((item) => item.roomId)).toEqual(['a', 'b']);
		expect((await queue.items()).map((item) => [item.roomId, item.status])).toEqual([
			['b', 'waiting'],
			['c', 'waiting']
		]);

		const up = sender();
		expect(await queue.replay(up.send)).toEqual({ stopped: null });
		expect(up.sent.map((item) => item.roomId)).toEqual(['b', 'c']);
		expect(await queue.items()).toEqual([]);
	});

	it('stops at a server error as it does at a network error', async () => {
		const queue = createQueue(memoryStore());
		await queue.enqueue(condition('a'));
		await queue.enqueue(condition('b'));
		expect(await queue.replay(sender({ a: problem(503) }).send)).toEqual({ stopped: 'network' });
		expect(await queue.items()).toHaveLength(2);
	});

	it('pauses at a 401, keeps everything, and resumes after sign-in', async () => {
		const queue = createQueue(memoryStore());
		await queue.enqueue(condition('a'));
		await queue.enqueue(condition('b'));
		const expired = sender({ a: problem(401) });
		expect(await queue.replay(expired.send)).toEqual({ stopped: 'sign-in' });
		expect(expired.sent.map((item) => item.roomId)).toEqual(['a']);
		expect((await queue.items()).map((item) => item.status)).toEqual(['waiting', 'waiting']);

		const back = sender();
		await queue.replay(back.send);
		expect(back.sent.map((item) => item.roomId)).toEqual(['a', 'b']);
	});

	it('gives an issue its Idempotency-Key at enqueue and resends the same key on every replay', async () => {
		const queue = createQueue(memoryStore());
		await queue.enqueue(issue);
		const [queued] = await queue.items();
		expect(queued.kind === 'issue' && queued.key).toMatch(/^[0-9a-f-]{36}$/);

		const keys: string[] = [];
		const flaky: Send = async (item) => {
			if (item.kind === 'issue') keys.push(item.key);
			if (keys.length < 2) throw new TypeError('Failed to fetch');
			return {};
		};
		await queue.replay(flaky);
		await queue.replay(flaky);
		expect(keys).toHaveLength(2);
		expect(keys[0]).toBe(keys[1]);
		expect(keys[0]).toBe(queued.kind === 'issue' ? queued.key : '');
	});

	it('keeps a key it is given, for a request that may already have reached the server', async () => {
		const queue = createQueue(memoryStore());
		await queue.enqueue({ ...issue, key: 'a-key-from-the-form' });
		const [queued] = await queue.items();
		expect(queued.kind === 'issue' && queued.key).toBe('a-key-from-the-form');
	});

	it('a change queued behind another on the same room is based on the version that one produced', async () => {
		const queue = createQueue(memoryStore());
		await queue.enqueue(condition('a', 3));
		await queue.enqueue({ ...condition('a', null), condition: 'CLEAN' });
		const { sent, send } = sender();
		await queue.replay(send);
		expect(sent.map((item) => (item.kind === 'condition' ? item.version : 'issue'))).toEqual([
			3, 10
		]);
	});

	it('a change behind a conflict on the same room is a conflict too', async () => {
		const queue = createQueue(memoryStore());
		await queue.enqueue(condition('a', 3));
		await queue.enqueue({ ...condition('a', null), condition: 'CLEAN' });
		await queue.enqueue(condition('b'));
		const { sent, send } = sender({ a: problem(412) });
		await queue.replay(send);
		expect(sent.map((item) => item.roomId)).toEqual(['a', 'b']);
		expect((await queue.items()).map((item) => item.status)).toEqual(['conflict', 'conflict']);
	});

	it('retry resends a conflict against the current version, in its place in the queue', async () => {
		const queue = createQueue(memoryStore());
		await queue.enqueue(condition('a', 3));
		await queue.enqueue(condition('b'));
		await queue.replay(sender({ a: problem(412) }).send);
		const [conflict] = await queue.items();
		await queue.retry(conflict.id, 7);
		const { sent, send } = sender();
		await queue.replay(send);
		expect(sent).toHaveLength(1);
		expect(sent[0]).toMatchObject({ roomId: 'a', version: 7 });
		expect(await queue.items()).toEqual([]);
	});

	it('retrying a conflict brings back the changes behind it that were conflicts only because of it', async () => {
		const queue = createQueue(memoryStore());
		await queue.enqueue(condition('a', 3));
		await queue.enqueue({ ...condition('a', null), condition: 'CLEAN' });
		await queue.replay(sender({ a: problem(412) }).send);
		const [first] = await queue.items();
		await queue.retry(first.id, 7);
		const { sent, send } = sender();
		await queue.replay(send);
		expect(sent.map((item) => (item.kind === 'condition' ? item.version : 'issue'))).toEqual([
			7, 10
		]);
		expect(await queue.items()).toEqual([]);
	});

	it('a 409 that says the key is in progress waits instead of becoming a conflict', async () => {
		const queue = createQueue(memoryStore());
		await queue.enqueue(issue);
		const busy = new ApiError({
			type: 'about:blank',
			title: 'x',
			status: 409,
			detail: 'a request with this Idempotency-Key is still in progress; retry shortly'
		});
		expect(await queue.replay(sender({ r1: busy }).send)).toEqual({ stopped: 'network' });
		expect((await queue.items())[0].status).toBe('waiting');
	});

	it('discard drops one item and leaves the rest', async () => {
		const queue = createQueue(memoryStore());
		await queue.enqueue(condition('a'));
		await queue.enqueue(condition('b'));
		const [first] = await queue.items();
		await queue.discard(first.id);
		expect((await queue.items()).map((item) => item.roomId)).toEqual(['b']);
	});

	it('two replays at once send each item once', async () => {
		const queue = createQueue(memoryStore());
		await queue.enqueue(condition('a'));
		await queue.enqueue(condition('b'));
		const { sent, send } = sender();
		await Promise.all([queue.replay(send), queue.replay(send)]);
		expect(sent.map((item) => item.roomId)).toEqual(['a', 'b']);
	});
});
