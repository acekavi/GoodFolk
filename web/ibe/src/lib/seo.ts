import type { SiteView } from './api';
import { fallback } from './media';

/** The description search engines and link previews show: the site's own, cut at a word near 160 characters. */
export function metaDescription(site: SiteView): string {
	const text = (site.description || `Book a room at ${site.name} directly with the hotel.`)
		.replace(/\s+/g, ' ')
		.trim();
	if (text.length <= 160) return text;
	const cut = text.slice(0, 157);
	return `${cut.slice(0, cut.lastIndexOf(' ') > 100 ? cut.lastIndexOf(' ') : 157)}…`;
}

/** The hero as a link preview's image: a WebP, which every preview reader takes, about 1200 px wide. */
export function shareImage(site: SiteView): string | null {
	return site.hero ? (fallback(site.hero.images, 1200)?.url ?? null) : null;
}

/** schema.org `Hotel` for the site at `url`. */
export function hotelData(site: SiteView, url: string): Record<string, unknown> {
	const image = shareImage(site);
	return {
		'@context': 'https://schema.org',
		'@type': 'Hotel',
		name: site.name,
		url,
		...(site.description && { description: site.description }),
		...(image && { image }),
		...(site.contact_phone && { telephone: site.contact_phone }),
		...(site.contact_email && { email: site.contact_email }),
		checkinTime: site.check_in_time,
		checkoutTime: site.check_out_time
	};
}

/** `data` as a JSON-LD `<script>`, with `<` escaped so no text in it can close the element. */
export function jsonLdScript(data: unknown): string {
	return `<script type="application/ld+json">${JSON.stringify(data).replace(/</g, '\\u003c')}</script>`;
}
