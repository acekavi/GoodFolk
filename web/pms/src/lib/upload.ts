import type { components } from './api/openapi';
import { ApiError, NETWORK_ERROR } from './api/problem';
import { idempotencyKey, rest, unwrap } from './api/rest';

export type Purpose = components['schemas']['Purpose'];
export type DocumentKind = components['schemas']['DocumentKind'];
export type MediaAsset = components['schemas']['MediaAsset'];
type StartUploadRequest = components['schemas']['StartUploadRequest'];
type StartedUpload = components['schemas']['StartedUpload'];
type UploadedPart = components['schemas']['UploadedPart'];

const MIB = 1024 * 1024;
const IMAGES = ['image/jpeg', 'image/png', 'image/webp'];

/** What each purpose accepts, mirroring the server's table. The server decides from the bytes. */
export const LIMITS: Record<Purpose, { types: readonly string[]; maxBytes: number }> = {
	gallery: { types: IMAGES, maxBytes: 25 * MIB },
	issue_photo: { types: IMAGES, maxBytes: 25 * MIB },
	guest_document: { types: [...IMAGES, 'application/pdf'], maxBytes: 15 * MIB }
};

const BY_EXTENSION: Record<string, string> = {
	jpg: 'image/jpeg',
	jpeg: 'image/jpeg',
	png: 'image/png',
	webp: 'image/webp',
	pdf: 'application/pdf',
	heic: 'image/heic',
	heif: 'image/heif'
};
const HEIC = new Set(['image/heic', 'image/heif', 'image/heic-sequence', 'image/heif-sequence']);

/** Refusal messages, the same words the server uses. */
export const NOT_ACCEPTED = 'This file type is not accepted here';
export const NOT_AN_IMAGE = 'Not a supported image';
export const EMPTY = 'The file is empty';

/** The file's type as the browser reports it, or from its extension when the browser does not know. */
export function fileType(file: { name: string; type: string }): string {
	if (file.type) return file.type.toLowerCase();
	const extension = file.name.split('.').pop()?.toLowerCase() ?? '';
	return BY_EXTENSION[extension] ?? '';
}

/**
 * Why `purpose` would refuse the file, or `null` if it looks acceptable. A convenience that spares a
 * pointless upload; the server sniffs the bytes and has the final word.
 */
export function refusal(
	file: { name: string; type: string; size: number },
	purpose: Purpose
): string | null {
	const { types, maxBytes } = LIMITS[purpose];
	const type = fileType(file);
	if (HEIC.has(type)) return NOT_ACCEPTED;
	if (!types.includes(type)) return purpose === 'guest_document' ? NOT_ACCEPTED : NOT_AN_IMAGE;
	if (file.size === 0) return EMPTY;
	if (file.size > maxBytes) return `Too large: the limit is ${maxBytes / MIB} MB`;
	return null;
}

/** A file the client refused before uploading anything. */
export class UploadRefused extends Error {}

/** A part PUT that storage did not accept; `status` is `null` when no answer came. */
export class PartFailed extends Error {
	constructor(readonly status: number | null) {
		super(status === null ? 'The part could not be sent' : `Storage answered ${status}`);
	}
}

/** The words to show for a failed upload. */
export function uploadErrorMessage(error: unknown): string {
	return error instanceof ApiError || error instanceof UploadRefused
		? error.message
		: NETWORK_ERROR;
}

export interface PartRange {
	number: number;
	start: number;
	end: number;
}

/** The byte ranges of a file of `size` bytes cut into parts of `partSize`, numbered from 1. */
export function partRanges(size: number, partSize: number): PartRange[] {
	const ranges: PartRange[] = [];
	for (let start = 0, number = 1; start < size; start += partSize, number++) {
		ranges.push({ number, start, end: Math.min(start + partSize, size) });
	}
	return ranges;
}

/** Waits before each retry of a failed part. */
export const RETRY_DELAYS: readonly number[] = [1000, 2000, 4000];
/** Parts sent at once. */
export const PARALLEL_PARTS = 3;

function sleep(ms: number, signal?: AbortSignal): Promise<void> {
	return new Promise((resolve, reject) => {
		if (signal?.aborted) return reject(signal.reason);
		const timer = setTimeout(() => {
			signal?.removeEventListener('abort', onAbort);
			resolve();
		}, ms);
		function onAbort() {
			clearTimeout(timer);
			reject(signal!.reason);
		}
		signal?.addEventListener('abort', onAbort, { once: true });
	});
}

/** Runs `attempt`, and again after each of `delays` while it fails with a `retryable` error. */
export async function withRetry<T>(
	attempt: () => Promise<T>,
	retryable: (error: unknown) => boolean,
	signal?: AbortSignal,
	delays: readonly number[] = RETRY_DELAYS
): Promise<T> {
	for (let tries = 0; ; tries++) {
		try {
			return await attempt();
		} catch (error) {
			if (signal?.aborted || tries >= delays.length || !retryable(error)) throw error;
			await sleep(delays[tries], signal);
		}
	}
}

/** No answer, a server error, a timeout or throttling may pass on a retry; other refusals will not. */
export function retryablePart(error: unknown): boolean {
	if (!(error instanceof PartFailed)) return false;
	const { status } = error;
	return status === null || status >= 500 || status === 408 || status === 429;
}

/** PUTs one part to its presigned URL and returns the `ETag` storage answered, quotes and all. */
export async function putPart(
	url: string,
	body: Blob,
	signal: AbortSignal,
	put: typeof fetch = fetch
): Promise<string> {
	let response: Response;
	try {
		response = await put(url, { method: 'PUT', body, signal });
	} catch (error) {
		if (signal.aborted) throw error;
		throw new PartFailed(null);
	}
	if (!response.ok) throw new PartFailed(response.status);
	const etag = response.headers.get('etag');
	// Missing only when storage does not expose the header to this origin (its CORS rules).
	if (!etag) throw new Error('Storage did not expose the part’s ETag');
	return etag;
}

/** Runs `task` on each item, at most `limit` at a time; stops taking items after a failure. */
async function eachLimit<T>(items: readonly T[], limit: number, task: (item: T) => Promise<void>) {
	let next = 0;
	let failed = false;
	async function worker() {
		while (!failed && next < items.length) {
			const item = items[next++];
			try {
				await task(item);
			} catch (error) {
				failed = true;
				throw error;
			}
		}
	}
	await Promise.all(Array.from({ length: Math.min(limit, items.length) }, worker));
}

export async function sha256Hex(data: ArrayBuffer): Promise<string> {
	// SubtleCrypto cannot hash incrementally; a whole file (at most 25 MiB) is fine.
	const digest = await crypto.subtle.digest('SHA-256', data);
	return Array.from(new Uint8Array(digest), (byte) => byte.toString(16).padStart(2, '0')).join('');
}

/** The server calls an upload makes; replaced in tests. */
export interface UploadApi {
	start(propertyId: string, body: StartUploadRequest, key: string): Promise<StartedUpload>;
	complete(
		propertyId: string,
		uploadId: string,
		parts: UploadedPart[],
		key: string
	): Promise<MediaAsset>;
	cancel(propertyId: string, uploadId: string): Promise<void>;
	put: typeof fetch;
}

export const restUploadApi: UploadApi = {
	async start(propertyId, body, key) {
		return unwrap(
			await rest.POST('/api/v1/properties/{property}/media/uploads', {
				params: { path: { property: propertyId }, header: { 'Idempotency-Key': key } },
				body
			})
		);
	},
	async complete(propertyId, uploadId, parts, key) {
		return unwrap(
			await rest.POST('/api/v1/properties/{property}/media/uploads/{upload}/complete', {
				params: {
					path: { property: propertyId, upload: uploadId },
					header: { 'Idempotency-Key': key }
				},
				body: { parts }
			})
		);
	},
	async cancel(propertyId, uploadId) {
		unwrap(
			await rest.DELETE('/api/v1/properties/{property}/media/uploads/{upload}', {
				params: { path: { property: propertyId, upload: uploadId } }
			})
		);
	},
	put: (input, init) => fetch(input, init)
};

export interface UploadProgress {
	phase: 'hashing' | 'uploading' | 'completing';
	/** Bytes of the parts already in storage. */
	loaded: number;
	total: number;
}

export interface UploadOptions {
	propertyId: string;
	purpose: Purpose;
	targetId: string;
	/** Required for a guest document, and only for it. */
	kind?: DocumentKind;
	/** Cancels the upload; one already started is deleted on the server. */
	signal?: AbortSignal;
	onProgress?: (progress: UploadProgress) => void;
}

/**
 * Uploads a file straight to storage: checks it, hashes it, starts the upload, PUTs its parts to their
 * presigned URLs ({@link PARALLEL_PARTS} at a time, each retried after {@link RETRY_DELAYS}), then
 * completes it. Returns the asset, usually still `processing`. Throws {@link UploadRefused}, an
 * `ApiError` (such as the server's 422 refusal), a part failure, or the signal's reason once aborted.
 */
export async function uploadFile(
	file: File,
	options: UploadOptions,
	api: UploadApi = restUploadApi
): Promise<MediaAsset> {
	const { propertyId, purpose, targetId, kind, signal, onProgress } = options;
	const reason = refusal(file, purpose);
	if (reason) throw new UploadRefused(reason);
	signal?.throwIfAborted();

	const total = file.size;
	onProgress?.({ phase: 'hashing', loaded: 0, total });
	const sha256 = await sha256Hex(await file.arrayBuffer());
	signal?.throwIfAborted();

	// Not aborted mid-request: a start the server answered is cancelled below rather than left behind.
	const started = await api.start(
		propertyId,
		{
			purpose,
			target_id: targetId,
			filename: file.name,
			bytes: total,
			sha256,
			...(kind ? { kind } : {})
		},
		idempotencyKey()
	);

	// Aborts the other parts when one fails for good, as well as when the caller cancels.
	const parts = new AbortController();
	const forward = () => parts.abort(signal?.reason);
	signal?.addEventListener('abort', forward, { once: true });
	if (signal?.aborted) forward();
	let completing = false;
	try {
		const ranges = partRanges(total, started.part_size);
		const urls = new Map(started.parts.map((part) => [part.number, part.url]));
		if (ranges.length !== urls.size || ranges.some((range) => !urls.has(range.number))) {
			throw new Error('The server’s parts do not match the file');
		}
		const uploaded: UploadedPart[] = [];
		let loaded = 0;
		onProgress?.({ phase: 'uploading', loaded, total });
		try {
			await eachLimit(ranges, PARALLEL_PARTS, async (range) => {
				const url = urls.get(range.number)!;
				const body = file.slice(range.start, range.end);
				const etag = await withRetry(
					() => putPart(url, body, parts.signal, api.put),
					retryablePart,
					parts.signal
				);
				uploaded.push({ number: range.number, etag });
				loaded += range.end - range.start;
				onProgress?.({ phase: 'uploading', loaded, total });
			});
		} catch (error) {
			parts.abort();
			throw signal?.aborted ? signal.reason : error;
		}
		signal?.throwIfAborted();

		completing = true;
		onProgress?.({ phase: 'completing', loaded, total });
		uploaded.sort((a, b) => a.number - b.number);
		return await api.complete(propertyId, started.upload_id, uploaded, idempotencyKey());
	} catch (error) {
		// Once complete is sent the server owns the upload: it keeps it or has already removed it.
		if (!completing) void api.cancel(propertyId, started.upload_id).catch(() => {});
		throw error;
	} finally {
		signal?.removeEventListener('abort', forward);
	}
}
