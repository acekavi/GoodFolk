import { error } from '@sveltejs/kit';
import { fetchSite } from '$lib/api';
import { onBookingDomain } from '$lib/host';
import type { RequestHandler } from './$types';

/** The site's one page worth indexing, its landing page, at the address it was asked by (booking domain or custom host). */
export const GET: RequestHandler = async ({ params, fetch, url }) => {
	const site = await fetchSite(fetch, params.code);
	if (!site) error(404, 'No booking site here');
	const landing = `${url.origin}${onBookingDomain(url) ? `/${site.code}` : '/'}`;
	const xml = `<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
	<url><loc>${escapeXml(landing)}</loc></url>
</urlset>
`;
	return new Response(xml, {
		headers: { 'content-type': 'application/xml', 'cache-control': 'public, max-age=3600' }
	});
};

function escapeXml(text: string): string {
	return text.replace(/[<>&'"]/g, (char) => `&#${char.charCodeAt(0)};`);
}
