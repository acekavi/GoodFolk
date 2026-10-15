import { describe, expect, it, vi } from 'vitest';
import { ApiError } from './api/problem';
import type { MediaAsset, UploadOptions } from './upload';
import { percent, Uploads, type UploadTarget } from './uploads.svelte';

const target: UploadTarget = { propertyId: 'p1', purpose: 'issue_photo', targetId: 'i1' };
const photo = (name = 'room.jpg') => new File(['0123456789'], name, { type: 'image/jpeg' });
const asset = (status: MediaAsset['status'] = 'processing'): MediaAsset => ({
	asset_id: 'a1',
	purpose: 'issue_photo',
	status,
	filename: 'room.jpg'
});

/** An upload the test finishes by hand. */
function controlled() {
	let finish!: (asset: MediaAsset) => void;
	let fail!: (error: unknown) => void;
	let options!: UploadOptions;
	const upload = vi.fn(
		(_file: File, opts: UploadOptions) =>
			new Promise<MediaAsset>((resolve, reject) => {
				options = opts;
				finish = resolve;
				fail = reject;
			})
	);
	return {
		upload,
		finish: (a: MediaAsset) => finish(a),
		fail: (e: unknown) => fail(e),
		options: () => options
	};
}

const settled = () => new Promise((resolve) => setTimeout(resolve));

describe('Uploads', () => {
	it('sends a file for its target, then shows it processing and tells the screen', async () => {
		const run = controlled();
		const onsent = vi.fn();
		const uploads = new Uploads(onsent, run.upload);

		uploads.add([photo()], target);
		expect(uploads.items[0].state).toBe('sending');
		expect(run.options()).toMatchObject(target);
		expect(uploads.cancellable(uploads.items[0])).toBe(true);

		run.finish(asset());
		await settled();
		expect(uploads.items[0]).toMatchObject({ state: 'processing', assetId: 'a1' });
		expect(onsent).toHaveBeenCalledWith(uploads.items[0]);
		expect(uploads.active).toBe(1);
	});

	it('refuses a file the purpose does not take, or the caller refuses, without sending it', () => {
		const run = controlled();
		const uploads = new Uploads(() => {}, run.upload);

		uploads.add(
			[new File(['x'], 'scan.heic', { type: 'image/heic' }), photo('a.jpg'), photo('b.jpg')],
			target,
			(_, index) => (index === 2 ? 'An issue can have at most 5 photos' : null)
		);

		expect(uploads.items.map((u) => [u.state, u.error])).toEqual([
			['failed', 'This file type is not accepted here'],
			['sending', ''],
			['failed', 'An issue can have at most 5 photos']
		]);
		expect(run.upload).toHaveBeenCalledTimes(1);
		expect(uploads.active).toBe(1);
	});

	it('shows why an upload failed', async () => {
		const run = controlled();
		const uploads = new Uploads(() => {}, run.upload);

		uploads.add([photo()], target);
		run.fail(
			new ApiError({
				type: 'about:blank',
				title: 'Conflict',
				status: 409,
				detail: 'An issue can have at most 5 photos'
			})
		);
		await settled();

		expect(uploads.items[0]).toMatchObject({
			state: 'failed',
			error: 'An issue can have at most 5 photos'
		});
	});

	it('shows processing that failed straight away with its reason', async () => {
		const run = controlled();
		const uploads = new Uploads(() => {}, run.upload);

		uploads.add([photo()], target);
		run.finish({ ...asset('failed'), error: 'Not a supported image' });
		await settled();

		expect(uploads.items[0]).toMatchObject({ state: 'failed', error: 'Not a supported image' });
	});

	it('drops an upload once the list shows its asset', async () => {
		const run = controlled();
		const uploads = new Uploads(() => {}, run.upload);
		uploads.add([photo()], target);
		run.finish(asset());
		await settled();

		uploads.settle(['other']);
		expect(uploads.items).toHaveLength(1);
		uploads.settle(['a1']);
		expect(uploads.items).toHaveLength(0);
	});

	it('cancels a sending upload by aborting it, and its tile goes', async () => {
		const run = controlled();
		const uploads = new Uploads(() => {}, run.upload);
		uploads.add([photo()], target);

		uploads.cancel(uploads.items[0]);
		expect(run.options().signal?.aborted).toBe(true);
		run.fail(run.options().signal?.reason);
		await settled();

		expect(uploads.items).toHaveLength(0);
	});

	it('aborts every upload and starts no more once the screen goes', async () => {
		const run = controlled();
		const onsent = vi.fn();
		const uploads = new Uploads(onsent, run.upload);
		uploads.add([photo('1.jpg'), photo('2.jpg'), photo('3.jpg'), photo('4.jpg')], target);
		expect(run.upload).toHaveBeenCalledTimes(3);

		uploads.destroy();
		expect(run.options().signal?.aborted).toBe(true);
		run.fail(run.options().signal?.reason);
		await settled();

		expect(run.upload).toHaveBeenCalledTimes(3);
	});
});

describe('percent', () => {
	it('is the share of bytes sent, in whole percent', () => {
		expect(percent(null)).toBe(0);
		expect(percent({ phase: 'uploading', loaded: 1, total: 3 })).toBe(33);
		expect(percent({ phase: 'completing', loaded: 3, total: 3 })).toBe(100);
	});
});
