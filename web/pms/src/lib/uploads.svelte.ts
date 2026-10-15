import {
	refusal,
	uploadErrorMessage,
	uploadFile,
	uploadQueue,
	type UploadOptions,
	type UploadProgress
} from './upload';

/** What an upload is for: the property, the purpose, its target and, for a guest document, its kind. */
export type UploadTarget = Pick<UploadOptions, 'propertyId' | 'purpose' | 'targetId' | 'kind'>;

/** One file on its way up, as its tile shows it. */
export interface Upload {
	id: string;
	name: string;
	file: File;
	target: UploadTarget;
	controller: AbortController;
	state: 'waiting' | 'sending' | 'processing' | 'failed';
	progress: UploadProgress | null;
	error: string;
	/** Set once completed; the tile goes when the list it belongs to shows this asset. */
	assetId: string | null;
}

/** Files uploaded at once; each sends up to three parts in parallel. */
export const PARALLEL_FILES = 3;
export const PROCESSING_FAILED = 'Processing failed';

/** How far an upload's parts have gone, in whole percent. */
export function percent(progress: UploadProgress | null): number {
	return progress && progress.total > 0 ? Math.round((progress.loaded / progress.total) * 100) : 0;
}

/**
 * A screen's uploads, for showing as tiles: each waits, sends with its progress, then shows as
 * processing until the screen's list holds its asset ({@link settle}), or fails with the reason.
 * At most {@link PARALLEL_FILES} send at once. `onsent` runs after each completed upload, also once
 * the screen has gone, so the list can be refetched; a list that cannot tell which asset a row is can
 * dismiss the upload once it has refetched. {@link destroy} stops them all when the screen goes.
 */
export class Uploads {
	items = $state<Upload[]>([]);
	#queue = uploadQueue(PARALLEL_FILES, (upload: Upload) => this.#send(upload));
	/** Set when the screen goes away; from then on nothing may touch the state. */
	#destroyed = false;

	#onsent: (upload: Upload) => void;
	#upload: typeof uploadFile;

	constructor(onsent: (upload: Upload) => void, upload: typeof uploadFile = uploadFile) {
		this.#onsent = onsent;
		this.#upload = upload;
	}

	/**
	 * Adds files for `target`. A file `refuse` (or the purpose's own check) gives a reason for is shown
	 * as failed at once and never sent.
	 */
	add(files: File[], target: UploadTarget, refuse?: (file: File, index: number) => string | null) {
		files.forEach((file, index) => {
			const reason = refuse?.(file, index) ?? refusal(file, target.purpose);
			this.items.push({
				id: crypto.randomUUID(),
				name: file.name,
				file,
				target,
				controller: new AbortController(),
				state: reason ? 'failed' : 'waiting',
				progress: null,
				error: reason ?? '',
				assetId: null
			});
			// The pushed object is the reactive proxy; updates must go through it.
			if (!reason) this.#queue.add(this.items[this.items.length - 1]);
		});
	}

	/** Uploads not failed and not yet in the list: those that will take a place in it. */
	get active(): number {
		return this.items.filter((upload) => upload.state !== 'failed').length;
	}

	async #send(upload: Upload) {
		if (upload.controller.signal.aborted) return;
		upload.state = 'sending';
		let asset;
		try {
			asset = await this.#upload(upload.file, {
				...upload.target,
				signal: upload.controller.signal,
				onProgress: (progress) => {
					if (!this.#destroyed) upload.progress = progress;
				}
			});
		} catch (err) {
			if (this.#destroyed) return;
			if (upload.controller.signal.aborted) return this.dismiss(upload);
			upload.state = 'failed';
			upload.error = uploadErrorMessage(err);
			return;
		}
		// The refetch shows the file even if the screen has gone, for the next visit.
		this.#onsent(upload);
		if (this.#destroyed) return;
		if (asset.status === 'failed') {
			upload.state = 'failed';
			upload.error = asset.error ?? PROCESSING_FAILED;
		} else {
			upload.state = 'processing';
			upload.assetId = asset.asset_id;
		}
	}

	/** Whether the upload can still be cancelled: it is waiting, or sending and not yet completing. */
	cancellable(upload: Upload): boolean {
		return (
			upload.state === 'waiting' ||
			(upload.state === 'sending' && upload.progress?.phase !== 'completing')
		);
	}

	cancel(upload: Upload) {
		upload.controller.abort(new DOMException('Upload cancelled', 'AbortError'));
		// A waiting upload has no request to end, so its tile goes now.
		if (upload.state === 'waiting') this.dismiss(upload);
	}

	dismiss(upload: Upload) {
		this.items = this.items.filter((u) => u.id !== upload.id);
	}

	/**
	 * Drops the finished uploads whose asset the list now shows, for good: a later removal of that file
	 * must not bring its tile back.
	 */
	settle(shown: Iterable<string>) {
		const ids = [...shown];
		const arrived = (u: Upload) => u.assetId !== null && ids.includes(u.assetId);
		if (this.items.some(arrived)) this.items = this.items.filter((u) => !arrived(u));
	}

	/**
	 * Stops everything as the screen goes: those waiting never start, and those sending are aborted,
	 * which deletes them on the server. One already completing is left to finish.
	 */
	destroy() {
		this.#destroyed = true;
		this.#queue.stop();
		for (const upload of this.items) {
			upload.controller.abort(new DOMException('The page was left', 'AbortError'));
		}
	}
}
