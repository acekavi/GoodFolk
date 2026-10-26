import { QueryClient } from '@tanstack/svelte-query';
import { describe, expect, it } from 'vitest';
import { applyEvent } from './events';
import {
	countedVideos,
	DISPLAY_SIZES,
	dropIndex,
	formatDuration,
	LARGE_SIZES,
	largestOf,
	mediaKey,
	posterOf,
	renditionsOf,
	srcset,
	videoSources,
	type MediaItem,
	type Rendition,
	type VideoSource
} from './media';

const rendition = (
	size: Rendition['size'],
	format: Rendition['format'],
	width: number,
	height: number
): Rendition => ({
	size,
	format,
	width,
	height,
	url: `https://cdn/a/${size.toLowerCase()}.${format.toLowerCase()}`
});

const renditions = [
	rendition('XL', 'WEBP', 2400, 1600),
	rendition('THUMB', 'AVIF', 320, 320),
	rendition('SM', 'AVIF', 640, 427),
	rendition('MD', 'WEBP', 1024, 683),
	rendition('THUMB', 'WEBP', 320, 320),
	rendition('SM', 'WEBP', 640, 427)
];

describe('srcset', () => {
	it('lists one format’s display sizes narrowest first, each with its width', () => {
		expect(srcset(renditionsOf(renditions, 'WEBP', DISPLAY_SIZES))).toBe(
			'https://cdn/a/sm.webp 640w, https://cdn/a/md.webp 1024w, https://cdn/a/xl.webp 2400w'
		);
		expect(srcset(renditionsOf(renditions, 'AVIF', DISPLAY_SIZES))).toBe(
			'https://cdn/a/sm.avif 640w'
		);
	});

	it('keeps the square thumbnail apart from the display sizes', () => {
		expect(srcset(renditionsOf(renditions, 'AVIF', ['THUMB']))).toBe(
			'https://cdn/a/thumb.avif 320w'
		);
	});

	it('is empty without renditions, as while processing', () => {
		expect(srcset(renditionsOf([], 'WEBP', DISPLAY_SIZES))).toBe('');
	});
});

describe('largestOf', () => {
	it('picks LG when the photo has it', () => {
		const withLarge = [...renditions, rendition('LG', 'WEBP', 1600, 1067)];
		expect(largestOf(withLarge, 'WEBP', LARGE_SIZES).map((r) => r.size)).toEqual(['LG']);
	});

	it('falls back to the largest size a smaller photo has, never past LG', () => {
		expect(largestOf(renditions, 'WEBP', LARGE_SIZES).map((r) => r.size)).toEqual(['MD']);
		expect(largestOf(renditions, 'AVIF', LARGE_SIZES).map((r) => r.size)).toEqual(['SM']);
	});

	it('shows the square thumbnail of a photo too small for anything else', () => {
		const tiny = [rendition('THUMB', 'WEBP', 300, 300)];
		expect(largestOf(tiny, 'WEBP', LARGE_SIZES).map((r) => r.size)).toEqual(['THUMB']);
		expect(largestOf([], 'WEBP', LARGE_SIZES)).toEqual([]);
	});
});

describe('mediaKey', () => {
	it('is refetched by the server’s media:<property> event', () => {
		const client = new QueryClient();
		client.setQueryData(mediaKey('p1'), {});
		client.setQueryData(mediaKey('p2'), {});

		applyEvent(client, 'invalidate', '["media:p1"]');

		expect(client.getQueryState(mediaKey('p1'))?.isInvalidated).toBe(true);
		expect(client.getQueryState(mediaKey('p2'))?.isInvalidated).toBe(false);
	});
});

describe('dropIndex', () => {
	// Two rows of two 100 px tiles with 10 px gaps.
	const boxes = [
		{ left: 0, top: 0, right: 100, bottom: 100 },
		{ left: 110, top: 0, right: 210, bottom: 100 },
		{ left: 0, top: 110, right: 100, bottom: 210 },
		{ left: 110, top: 110, right: 210, bottom: 210 }
	];

	it('picks the tile under the pointer', () => {
		expect(dropIndex(boxes, 150, 150)).toBe(3);
		expect(dropIndex(boxes, 10, 120)).toBe(2);
	});

	it('picks the nearest tile from a gap or outside the grid', () => {
		expect(dropIndex(boxes, 104, 50)).toBe(0);
		expect(dropIndex(boxes, 400, 20)).toBe(1);
		expect(dropIndex(boxes, 50, 900)).toBe(2);
	});

	it('is -1 without tiles', () => {
		expect(dropIndex([], 0, 0)).toBe(-1);
	});
});

describe('formatDuration', () => {
	it('writes minutes and seconds', () => {
		expect(formatDuration(28_000)).toBe('0:28');
		expect(formatDuration(60_000)).toBe('1:00');
		expect(formatDuration(4_000)).toBe('0:04');
		expect(formatDuration(3_725_000)).toBe('62:05');
	});

	it('rounds to the nearest second', () => {
		expect(formatDuration(28_400)).toBe('0:28');
		expect(formatDuration(28_600)).toBe('0:29');
	});
});

describe('videoSources', () => {
	const source = (
		format: VideoSource['format'],
		size: VideoSource['size'],
		width: number
	): VideoSource => ({
		format,
		size,
		width,
		height: Math.round((width * 9) / 16),
		url: `https://cdn/v/${format}-${size}.mp4`,
		mime:
			format === 'AV1'
				? 'video/mp4; codecs="av01.0.08M.10, opus"'
				: 'video/mp4; codecs="avc1.640028, mp4a.40.2"'
	});

	it('lists AV1 before H.264, largest first within a codec, whatever order they arrive in', () => {
		const node = {
			sources: [
				source('AV1', 'V1080', 1920),
				source('H264', 'V1080', 1920),
				source('AV1', 'V720', 1280),
				source('H264', 'V720', 1280)
			]
		};
		const expected = [
			{ src: 'https://cdn/v/AV1-V1080.mp4', type: node.sources[0].mime },
			{ src: 'https://cdn/v/AV1-V720.mp4', type: node.sources[2].mime },
			{ src: 'https://cdn/v/H264-V1080.mp4', type: node.sources[1].mime },
			{ src: 'https://cdn/v/H264-V720.mp4', type: node.sources[3].mime }
		];
		expect(videoSources(node)).toEqual(expected);
		expect(videoSources({ sources: [...node.sources].reverse() })).toEqual(expected);
	});

	it('is empty for a photo', () => {
		expect(videoSources({ sources: [] })).toEqual([]);
	});
});

describe('posterOf', () => {
	it('shows the MD poster', () => {
		expect(posterOf(renditions)).toBe('https://cdn/a/md.webp');
	});

	it('falls back to the largest poster a small clip has', () => {
		const small = renditions.filter((r) => r.size === 'THUMB' || r.size === 'SM');
		expect(posterOf(small)).toBe('https://cdn/a/sm.webp');
		expect(posterOf([rendition('THUMB', 'WEBP', 320, 320)])).toBe('https://cdn/a/thumb.webp');
		expect(posterOf([])).toBeUndefined();
	});
});

describe('countedVideos', () => {
	it('counts the videos the server counts, not photos or failed videos', () => {
		const item = (kind: 'IMAGE' | 'VIDEO', status: MediaItem['status']) => ({ kind, status });
		const media = [
			item('VIDEO', 'READY'),
			item('VIDEO', 'PROCESSING'),
			item('VIDEO', 'FAILED'),
			item('IMAGE', 'READY')
		];
		expect(countedVideos(media)).toBe(2);
	});
});
