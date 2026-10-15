import { afterEach, describe, expect, it, vi } from 'vitest';
import { ApiError, NETWORK_ERROR } from './api/problem';
import {
	PartFailed,
	UploadRefused,
	fileType,
	partRanges,
	putPart,
	refusal,
	retryablePart,
	sha256Hex,
	uploadErrorMessage,
	uploadFile,
	withRetry,
	type UploadApi
} from './upload';

const MIB = 1024 * 1024;
const file = (name: string, type: string, size: number) => ({ name, type, size });

describe('partRanges', () => {
	it('cuts a file into parts numbered from 1, the last one shorter', () => {
		expect(partRanges(20, 8)).toEqual([
			{ number: 1, start: 0, end: 8 },
			{ number: 2, start: 8, end: 16 },
			{ number: 3, start: 16, end: 20 }
		]);
	});

	it('makes one part of a file smaller than a part', () => {
		expect(partRanges(5, 8 * MIB)).toEqual([{ number: 1, start: 0, end: 5 }]);
	});

	it('does not add an empty part when the size is an exact multiple', () => {
		expect(partRanges(16 * MIB, 8 * MIB)).toHaveLength(2);
	});
});

describe('refusal', () => {
	it('accepts JPEG, PNG and WebP photos up to 25 MB', () => {
		expect(refusal(file('a.jpg', 'image/jpeg', 25 * MIB), 'gallery')).toBeNull();
		expect(refusal(file('a.png', 'image/png', 1), 'issue_photo')).toBeNull();
		expect(refusal(file('a.webp', 'image/webp', 1), 'gallery')).toBeNull();
	});

	it('refuses a photo over 25 MB with the limit', () => {
		expect(refusal(file('a.jpg', 'image/jpeg', 25 * MIB + 1), 'gallery')).toBe(
			'Too large: the limit is 25 MB'
		);
	});

	it('refuses a guest document over 15 MB with its limit', () => {
		expect(refusal(file('a.pdf', 'application/pdf', 15 * MIB), 'guest_document')).toBeNull();
		expect(refusal(file('a.pdf', 'application/pdf', 15 * MIB + 1), 'guest_document')).toBe(
			'Too large: the limit is 15 MB'
		);
	});

	it('refuses anything but a supported image as a photo', () => {
		expect(refusal(file('a.gif', 'image/gif', 10), 'gallery')).toBe('Not a supported image');
		expect(refusal(file('a.pdf', 'application/pdf', 10), 'issue_photo')).toBe(
			'Not a supported image'
		);
	});

	it('refuses an unaccepted guest document type', () => {
		expect(refusal(file('a.txt', 'text/plain', 10), 'guest_document')).toBe(
			'This file type is not accepted here'
		);
	});

	it('refuses HEIC, by type or by extension', () => {
		expect(refusal(file('a.heic', 'image/heic', 10), 'gallery')).toBe(
			'This file type is not accepted here'
		);
		expect(refusal(file('IMG_1.HEIC', '', 10), 'guest_document')).toBe(
			'This file type is not accepted here'
		);
	});

	it('refuses an empty file', () => {
		expect(refusal(file('a.jpg', 'image/jpeg', 0), 'gallery')).toBe('The file is empty');
	});
});

describe('fileType', () => {
	it('prefers the browser’s type and falls back to the extension', () => {
		expect(fileType({ name: 'x.png', type: 'image/jpeg' })).toBe('image/jpeg');
		expect(fileType({ name: 'x.JPEG', type: '' })).toBe('image/jpeg');
		expect(fileType({ name: 'noext', type: '' })).toBe('');
	});
});

describe('withRetry', () => {
	afterEach(() => {
		vi.useRealTimers();
	});

	it('retries after 1 s, 2 s and 4 s, then gives up', async () => {
		vi.useFakeTimers();
		const attempt = vi.fn().mockRejectedValue(new PartFailed(503));
		const result = withRetry(attempt, retryablePart);
		const settled = result.catch((error: unknown) => error);

		await vi.advanceTimersByTimeAsync(0);
		expect(attempt).toHaveBeenCalledTimes(1);
		await vi.advanceTimersByTimeAsync(999);
		expect(attempt).toHaveBeenCalledTimes(1);
		await vi.advanceTimersByTimeAsync(1);
		expect(attempt).toHaveBeenCalledTimes(2);
		await vi.advanceTimersByTimeAsync(2000);
		expect(attempt).toHaveBeenCalledTimes(3);
		await vi.advanceTimersByTimeAsync(3999);
		expect(attempt).toHaveBeenCalledTimes(3);
		await vi.advanceTimersByTimeAsync(1);
		expect(attempt).toHaveBeenCalledTimes(4);

		expect(await settled).toBeInstanceOf(PartFailed);
		await vi.advanceTimersByTimeAsync(10_000);
		expect(attempt).toHaveBeenCalledTimes(4);
	});

	it('returns the first success', async () => {
		vi.useFakeTimers();
		const attempt = vi
			.fn()
			.mockRejectedValueOnce(new PartFailed(null))
			.mockResolvedValueOnce('"etag"');
		const result = withRetry(attempt, retryablePart);

		await vi.advanceTimersByTimeAsync(1000);

		expect(await result).toBe('"etag"');
		expect(attempt).toHaveBeenCalledTimes(2);
	});

	it('does not retry an error that will not pass', async () => {
		const attempt = vi.fn().mockRejectedValue(new PartFailed(403));

		await expect(withRetry(attempt, retryablePart)).rejects.toBeInstanceOf(PartFailed);
		expect(attempt).toHaveBeenCalledTimes(1);
	});

	it('stops waiting when aborted', async () => {
		vi.useFakeTimers();
		const controller = new AbortController();
		const attempt = vi.fn().mockRejectedValue(new PartFailed(500));
		const result = withRetry(attempt, retryablePart, controller.signal);
		const settled = result.catch((error: unknown) => error);
		await vi.advanceTimersByTimeAsync(0);

		controller.abort(new Error('cancelled'));

		expect(await settled).toEqual(new Error('cancelled'));
		await vi.advanceTimersByTimeAsync(10_000);
		expect(attempt).toHaveBeenCalledTimes(1);
	});
});

describe('retryablePart', () => {
	it('retries no answer, server errors, timeouts and throttling only', () => {
		expect(retryablePart(new PartFailed(null))).toBe(true);
		expect(retryablePart(new PartFailed(500))).toBe(true);
		expect(retryablePart(new PartFailed(408))).toBe(true);
		expect(retryablePart(new PartFailed(429))).toBe(true);
		expect(retryablePart(new PartFailed(403))).toBe(false);
		expect(retryablePart(new Error('other'))).toBe(false);
	});
});

describe('putPart', () => {
	const signal = new AbortController().signal;

	it('returns the ETag exactly as storage sent it', async () => {
		const put = vi.fn().mockResolvedValue(new Response(null, { headers: { etag: '"abc"' } }));

		expect(await putPart('https://s3/part', new Blob(['x']), signal, put)).toBe('"abc"');
		expect(put).toHaveBeenCalledWith('https://s3/part', expect.objectContaining({ method: 'PUT' }));
	});

	it('turns a network failure and a refusal into PartFailed', async () => {
		const offline = vi.fn().mockRejectedValue(new TypeError('Failed to fetch'));
		const refused = vi.fn().mockResolvedValue(new Response(null, { status: 503 }));

		await expect(putPart('u', new Blob(['x']), signal, offline)).rejects.toEqual(
			new PartFailed(null)
		);
		await expect(putPart('u', new Blob(['x']), signal, refused)).rejects.toEqual(
			new PartFailed(503)
		);
	});
});

describe('sha256Hex', () => {
	it('hashes to 64 hex digits', async () => {
		expect(await sha256Hex(new TextEncoder().encode('abc').buffer as ArrayBuffer)).toBe(
			'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad'
		);
	});
});

describe('uploadErrorMessage', () => {
	it('shows refusals and API problems, and a network message otherwise', () => {
		const problem = { type: 'about:blank', title: 'Unprocessable', status: 422, detail: 'Nope' };
		expect(uploadErrorMessage(new UploadRefused('Not a supported image'))).toBe(
			'Not a supported image'
		);
		expect(uploadErrorMessage(new ApiError(problem))).toBe('Nope');
		expect(uploadErrorMessage(new PartFailed(500))).toBe(NETWORK_ERROR);
	});
});

describe('uploadFile', () => {
	const asset = {
		asset_id: 'a1',
		purpose: 'gallery' as const,
		status: 'processing' as const,
		filename: 'room.jpg'
	};

	function fakeApi(put: UploadApi['put']): UploadApi & {
		[K in 'start' | 'complete' | 'cancel']: ReturnType<typeof vi.fn>;
	} {
		return {
			start: vi.fn().mockResolvedValue({
				upload_id: 'u1',
				part_size: 4,
				parts: [1, 2, 3].map((number) => ({ number, url: `https://s3/${number}` })),
				expires_at: '2026-10-16T12:00:00Z'
			}),
			complete: vi.fn().mockResolvedValue(asset),
			cancel: vi.fn().mockResolvedValue(undefined),
			put
		};
	}
	const photo = () => new File(['0123456789'], 'room.jpg', { type: 'image/jpeg' });
	const options = { propertyId: 'p1', purpose: 'gallery' as const, targetId: 'rt1' };

	it('starts, sends every part and completes with their ETags in order', async () => {
		const put = vi.fn(async (url: string | URL | Request) => {
			return new Response(null, { headers: { etag: `"e${String(url).slice(-1)}"` } });
		});
		const api = fakeApi(put as UploadApi['put']);
		const progress = vi.fn();

		expect(await uploadFile(photo(), { ...options, onProgress: progress }, api)).toEqual(asset);

		const [, body] = api.start.mock.calls[0];
		expect(body).toEqual({
			purpose: 'gallery',
			target_id: 'rt1',
			filename: 'room.jpg',
			bytes: 10,
			sha256: await sha256Hex(new TextEncoder().encode('0123456789').buffer as ArrayBuffer)
		});
		expect(put).toHaveBeenCalledTimes(3);
		expect(api.complete).toHaveBeenCalledWith(
			'p1',
			'u1',
			[
				{ number: 1, etag: '"e1"' },
				{ number: 2, etag: '"e2"' },
				{ number: 3, etag: '"e3"' }
			],
			expect.any(String)
		);
		expect(progress).toHaveBeenCalledWith({ phase: 'uploading', loaded: 10, total: 10 });
		expect(progress).toHaveBeenLastCalledWith({ phase: 'completing', loaded: 10, total: 10 });
		expect(api.cancel).not.toHaveBeenCalled();
	});

	it('refuses a file before starting anything', async () => {
		const api = fakeApi(vi.fn());
		const heic = new File(['x'], 'IMG.heic', { type: 'image/heic' });

		await expect(uploadFile(heic, options, api)).rejects.toEqual(
			new UploadRefused('This file type is not accepted here')
		);
		expect(api.start).not.toHaveBeenCalled();
	});

	it('cancels the upload on the server when aborted', async () => {
		const controller = new AbortController();
		const put = vi.fn((_url: string | URL | Request, init?: RequestInit) => {
			return new Promise<Response>((_resolve, reject) => {
				init?.signal?.addEventListener('abort', () => reject(init.signal?.reason));
			});
		});
		const api = fakeApi(put as UploadApi['put']);
		const reason = new DOMException('Cancelled', 'AbortError');
		const result = uploadFile(photo(), { ...options, signal: controller.signal }, api);
		await vi.waitFor(() => expect(put).toHaveBeenCalled());

		controller.abort(reason);

		await expect(result).rejects.toBe(reason);
		expect(api.cancel).toHaveBeenCalledWith('p1', 'u1');
		expect(api.complete).not.toHaveBeenCalled();
	});

	it('cancels the upload when a part fails for good', async () => {
		const put = vi.fn().mockResolvedValue(new Response(null, { status: 403 }));
		const api = fakeApi(put);

		await expect(uploadFile(photo(), options, api)).rejects.toEqual(new PartFailed(403));
		expect(api.cancel).toHaveBeenCalledWith('p1', 'u1');
	});
});
