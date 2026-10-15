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
export type Rendition = MediaItem['renditions'][number];
export type RenditionSize = Rendition['size'];
export type ImageFormat = Rendition['format'];

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
