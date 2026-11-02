import type { Image, SiteView } from './api';

/** A `srcset` of one format's renditions, narrowest first; the square thumbnail only when there is nothing else. */
export function srcset(images: readonly Image[], format: 'avif' | 'webp'): string {
	const ofFormat = images.filter((image) => image.format === format);
	const scaled = ofFormat.filter((image) => image.size !== 'thumb');
	return (scaled.length ? scaled : ofFormat)
		.sort((a, b) => a.width - b.width)
		.map((image) => `${image.url} ${image.width}w`)
		.join(', ');
}

/** The image an `<img>` falls back to: the WebP nearest `width` wide without going under it, else the widest. */
export function fallback(images: readonly Image[], width: number): Image | null {
	const webp = images
		.filter((image) => image.format === 'webp' && image.size !== 'thumb')
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
