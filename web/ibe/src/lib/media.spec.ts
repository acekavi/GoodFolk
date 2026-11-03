import { describe, expect, it } from 'vitest';
import type { Image, SiteView } from './api';
import { fallback, mediaOrigin, srcset } from './media';

const image = (size: string, format: string, width: number): Image => ({
	size,
	format,
	width,
	height: Math.round(width * 0.75),
	url: `https://cdn/${size}.${format}`
});
const images = [
	image('thumb', 'webp', 320),
	image('lg', 'webp', 1600),
	image('sm', 'webp', 640),
	image('sm', 'avif', 640),
	image('lg', 'avif', 1600)
];

describe('srcset', () => {
	it('lists one format’s scaled renditions narrowest first', () => {
		expect(srcset(images, 'avif')).toBe('https://cdn/sm.avif 640w, https://cdn/lg.avif 1600w');
		expect(srcset(images, 'webp')).toBe('https://cdn/sm.webp 640w, https://cdn/lg.webp 1600w');
	});

	it('offers the phone rendition only for a largest paint', () => {
		const withPhone = [...images, image('phone', 'avif', 800), image('phone', 'webp', 800)];
		expect(srcset(withPhone, 'avif')).toBe('https://cdn/sm.avif 640w, https://cdn/lg.avif 1600w');
		expect(srcset(withPhone, 'avif', { phone: true })).toBe(
			'https://cdn/sm.avif 640w, https://cdn/phone.avif 800w, https://cdn/lg.avif 1600w'
		);
		expect(fallback(withPhone, 700)?.url).toBe('https://cdn/lg.webp');
	});

	it('falls back to the thumbnail when a photo has nothing larger', () => {
		expect(srcset([image('thumb', 'webp', 320)], 'webp')).toBe('https://cdn/thumb.webp 320w');
	});

	it('gives the <img> the WebP nearest the width it is shown at', () => {
		expect(fallback(images, 800)?.url).toBe('https://cdn/lg.webp');
		expect(fallback(images, 2000)?.url).toBe('https://cdn/lg.webp');
		expect(fallback(images, 500)?.url).toBe('https://cdn/sm.webp');
	});
});

describe('mediaOrigin', () => {
	const site = (patch: Partial<SiteView>): SiteView => ({
		code: 'GAL',
		name: 'Galle Fort Hotel',
		timezone: 'Asia/Colombo',
		business_date: '2026-11-01',
		check_in_time: '14:00',
		check_out_time: '11:00',
		description: '',
		logo: [],
		terms: '',
		privacy: '',
		room_types: [],
		...patch
	});

	it('is where the site’s photos come from, for the page to connect to early', () => {
		const room = { id: 'r', name: 'Room', description: '', beds: [], amenities: [], max_adults: 2 };
		const gallery = { ...room, max_children: 0, max_occupancy: 2 };
		const photo = { kind: 'image' as const, images, sources: [] };

		expect(mediaOrigin(site({ room_types: [{ ...gallery, media: [photo] }] }))).toBe('https://cdn');
		expect(mediaOrigin(site({}))).toBeNull();
	});
});
