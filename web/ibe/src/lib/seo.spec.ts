import { describe, expect, it } from 'vitest';
import type { SiteView } from './api';
import { hotelData, jsonLdScript, metaDescription } from './seo';

const site: SiteView = {
	code: 'GAL',
	name: 'Galle Fort Hotel',
	timezone: 'Asia/Colombo',
	business_date: '2026-11-01',
	check_in_time: '14:00',
	check_out_time: '11:00',
	accent: '#1f5c4a',
	description: 'A restored merchant’s house inside the ramparts.',
	logo: [],
	hero: {
		kind: 'image',
		images: [
			{ size: 'lg', format: 'webp', width: 1600, height: 1067, url: 'https://cdn/h-lg.webp' },
			{ size: 'lg', format: 'avif', width: 1600, height: 1067, url: 'https://cdn/h-lg.avif' }
		],
		sources: []
	},
	contact_phone: '+94 91 223 2870',
	contact_email: null,
	terms: '',
	privacy: '',
	room_types: []
};

describe('structured data', () => {
	it('describes the property as a schema.org Hotel with its WebP hero', () => {
		expect(hotelData(site, 'https://book.example.lk/GAL')).toEqual({
			'@context': 'https://schema.org',
			'@type': 'Hotel',
			name: 'Galle Fort Hotel',
			url: 'https://book.example.lk/GAL',
			description: 'A restored merchant’s house inside the ramparts.',
			image: 'https://cdn/h-lg.webp',
			telephone: '+94 91 223 2870',
			checkinTime: '14:00',
			checkoutTime: '11:00'
		});
	});

	it('cannot be closed early by text in it', () => {
		const script = jsonLdScript({ name: '</script><script>alert(1)</script>' });

		expect(script.match(/<\/script>/g)).toHaveLength(1);
		expect(JSON.parse(script.slice(35, -9))).toEqual({
			name: '</script><script>alert(1)</script>'
		});
	});

	it('keeps the meta description short', () => {
		const long = { ...site, description: 'word '.repeat(60) };

		expect(metaDescription(long).length).toBeLessThanOrEqual(160);
		expect(metaDescription({ ...site, description: '' })).toBe(
			'Book a room at Galle Fort Hotel directly with the hotel.'
		);
	});
});
