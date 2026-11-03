import type { Image, SiteView } from './api';

/**
 * A `srcset` of one format's renditions, narrowest first; the square thumbnail only when there is nothing else. The
 * `phone` rendition (800 px at a lower quality) is left out unless `phone` is asked for: see [`sources`].
 */
export function srcset(
	images: readonly Image[],
	format: 'avif' | 'webp',
	{ phone = false }: { phone?: boolean } = {}
): string {
	const ofFormat = images.filter(
		(image) => image.format === format && (phone || image.size !== 'phone')
	);
	const scaled = ofFormat.filter((image) => image.size !== 'thumb');
	return (scaled.length ? scaled : ofFormat)
		.sort((a, b) => a.width - b.width)
		.map((image) => `${image.url} ${image.width}w`)
		.join(', ');
}

/** The screens the `phone` rendition is offered to: a phone's, the hero's 4:3 breakpoint. */
export const NARROW = '(width < 40rem)';
/** Every other screen. */
export const WIDE = '(width >= 40rem)';

/** One `<source>` of a `<picture>`. */
export interface Source {
	type: 'image/avif' | 'image/webp';
	srcset: string;
	/** The screens it is for; every screen without. */
	media?: string;
}

/**
 * A `<picture>`'s sources, AVIF before WebP. For a page's largest paint (`largestPaint`) with a `phone` rendition, a
 * narrow screen is offered it first (it arrives quickly on a phone's connection, slightly softer); a wider screen, and
 * every other picture, gets the full-quality sizes only.
 */
export function sources(images: readonly Image[], { largestPaint = false } = {}): Source[] {
	const full: Source[] = [
		{ type: 'image/avif', srcset: srcset(images, 'avif') },
		{ type: 'image/webp', srcset: srcset(images, 'webp') }
	];
	if (!largestPaint || !images.some((image) => image.size === 'phone')) return full;
	return [
		{ type: 'image/avif', srcset: srcset(images, 'avif', { phone: true }), media: NARROW },
		{ type: 'image/webp', srcset: srcset(images, 'webp', { phone: true }), media: NARROW },
		...full.map((source) => ({ ...source, media: WIDE }))
	];
}

/** The image an `<img>` falls back to: the WebP nearest `width` wide without going under it, else the widest. */
export function fallback(images: readonly Image[], width: number): Image | null {
	const webp = images
		.filter((image) => image.format === 'webp' && image.size !== 'thumb' && image.size !== 'phone')
		.sort((a, b) => a.width - b.width);
	return webp.find((image) => image.width >= width) ?? webp.at(-1) ?? images[0] ?? null;
}

/** The origin the site's images are served from (the media CDN), from its first image; `null` with none. */
export function mediaOrigin(site: SiteView): string | null {
	const first =
		site.hero?.images[0] ??
		site.logo[0] ??
		site.room_types.flatMap((type) => type.media.flatMap((item) => item.images))[0];
	return first ? new URL(first.url).origin : null;
}
