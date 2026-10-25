import { graphql } from './api/gql';
import type { RoomTypeGalleriesQuery } from './api/gql/graphql';
import { query } from './api/graphql';

export const RoomTypeGalleriesDocument = graphql(`
	query RoomTypeGalleries($propertyId: UUID!) {
		roomTypes(propertyId: $propertyId) {
			id
			media {
				assetId
				status
				error
				caption
				width
				height
				kind
				durationMs
				sources {
					format
					size
					width
					height
					url
					mime
				}
				renditions {
					size
					format
					width
					height
					url
				}
			}
		}
	}
`);

export type MediaItem = RoomTypeGalleriesQuery['roomTypes'][number]['media'][number];
/** A photo as the issue photos query reads it: a gallery item without the video fields. */
export type Photo = Omit<MediaItem, 'kind' | 'durationMs' | 'sources'>;
export type Rendition = MediaItem['renditions'][number];
export type RenditionSize = Rendition['size'];
export type ImageFormat = Rendition['format'];
export type VideoSource = MediaItem['sources'][number];

/**
 * The property's galleries, photos and documents all change under the server's `media:<property>`
 * event, which `applyEvent` turns into an invalidation of exactly this key.
 */
export function mediaKey(propertyId: string) {
	return [`media:${propertyId}`] as const;
}

/** Each room type's gallery, in order, by room type id. */
export async function fetchGalleries(
	propertyId: string,
	signal?: AbortSignal
): Promise<Record<string, MediaItem[]>> {
	const { roomTypes } = await query(RoomTypeGalleriesDocument, { propertyId }, signal);
	return Object.fromEntries(roomTypes.map((type) => [type.id, type.media]));
}

/** The long-edge sizes a responsive image picks from; `THUMB` is a square crop shown on its own. */
export const DISPLAY_SIZES: readonly RenditionSize[] = ['SM', 'MD', 'LG', 'XL'];

/** The renditions of one format among `sizes`, narrowest first. */
export function renditionsOf(
	renditions: readonly Rendition[],
	format: ImageFormat,
	sizes: readonly RenditionSize[]
): Rendition[] {
	return renditions
		.filter((r) => r.format === format && sizes.includes(r.size))
		.sort((a, b) => a.width - b.width);
}

/**
 * The sizes the large view picks from: `LG`, else the largest below it, since a photo is never
 * upscaled and a small one has no `LG`; the square thumbnail only when it has nothing else.
 */
export const LARGE_SIZES: readonly RenditionSize[] = ['THUMB', 'SM', 'MD', 'LG'];

/** The widest rendition of one format among `sizes`, as a one-item list, or none. */
export function largestOf(
	renditions: readonly Rendition[],
	format: ImageFormat,
	sizes: readonly RenditionSize[]
): Rendition[] {
	return renditionsOf(renditions, format, sizes).slice(-1);
}

/** A `srcset` of the given renditions: each URL with its width descriptor. */
export function srcset(renditions: readonly Rendition[]): string {
	return renditions.map((r) => `${r.url} ${r.width}w`).join(', ');
}

/** A tile's box, as `getBoundingClientRect` gives it. */
export interface Box {
	left: number;
	top: number;
	right: number;
	bottom: number;
}

/** The tile a dragged photo would land on: the one under the point, else the one nearest it. */
export function dropIndex(boxes: readonly Box[], x: number, y: number): number {
	let best = -1;
	let bestDistance = Infinity;
	boxes.forEach((box, index) => {
		const dx = Math.max(box.left - x, 0, x - box.right);
		const dy = Math.max(box.top - y, 0, y - box.bottom);
		const distance = Math.hypot(dx, dy);
		if (distance < bestDistance) {
			best = index;
			bestDistance = distance;
		}
	});
	return best;
}

/** A room type holds at most this many videos. */
export const MAX_VIDEOS = 3;
export const TOO_MANY_VIDEOS = `A room type can have at most ${MAX_VIDEOS} videos`;

/** The videos that count toward {@link MAX_VIDEOS}: every one but a failed one, as the server counts. */
export function countedVideos(media: readonly Pick<MediaItem, 'kind' | 'status'>[]): number {
	return media.filter((item) => item.kind === 'VIDEO' && item.status !== 'FAILED').length;
}

/** The sizes a video's poster picks from: `MD`, else the largest below it, since a small clip has no `MD`. */
const POSTER_SIZES: readonly RenditionSize[] = ['THUMB', 'SM', 'MD'];

/** The URL of a video's WebP poster frame, if it has one. */
export function posterOf(renditions: readonly Rendition[]): string | undefined {
	return largestOf(renditions, 'WEBP', POSTER_SIZES)[0]?.url;
}

/** A video's length as `m:ss`, to the nearest second. */
export function formatDuration(ms: number): string {
	const total = Math.round(ms / 1000);
	return `${Math.floor(total / 60)}:${String(total % 60).padStart(2, '0')}`;
}

/**
 * A video's `<source>` list: AV1 first, where the browser decodes it, then H.264, the largest first
 * within a codec. The browser plays the first source it can.
 */
export function videoSources(node: {
	sources: readonly VideoSource[];
}): { src: string; type: string }[] {
	const codec = (source: VideoSource) => (source.format === 'AV1' ? 0 : 1);
	return [...node.sources]
		.sort((a, b) => codec(a) - codec(b) || b.width - a.width)
		.map((source) => ({ src: source.url, type: source.mime }));
}
