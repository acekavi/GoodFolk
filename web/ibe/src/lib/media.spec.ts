import { describe, expect, it } from 'vitest';
import type { Image } from './api';
import { fallback, srcset } from './media';

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

	it('falls back to the thumbnail when a photo has nothing larger', () => {
		expect(srcset([image('thumb', 'webp', 320)], 'webp')).toBe('https://cdn/thumb.webp 320w');
	});

	it('gives the <img> the WebP nearest the width it is shown at', () => {
		expect(fallback(images, 800)?.url).toBe('https://cdn/lg.webp');
		expect(fallback(images, 2000)?.url).toBe('https://cdn/lg.webp');
		expect(fallback(images, 500)?.url).toBe('https://cdn/sm.webp');
	});
});
