<script lang="ts">
	import type { Image } from '$lib/api';
	import { fallback, sources } from '$lib/media';

	/**
	 * A delivered image in AVIF, then WebP, at the width the layout shows it (`sizes`). Lazy unless it is the page's
	 * largest paint (`priority`), which the browser fetches first: preloaded from the head, before the page's inlined
	 * CSS, as AVIF only (`type` makes a browser without it skip the preload rather than fetch a second format), one
	 * preload per screen width its sources are for, so a phone preloads the small `phone` rendition (see `sources`) and a
	 * wider screen never fetches it.
	 */
	let {
		images,
		alt,
		sizes,
		shownWidth = 1024,
		priority = false,
		class: className = ''
	}: {
		images: readonly Image[];
		alt: string;
		sizes: string;
		shownWidth?: number;
		priority?: boolean;
		class?: string;
	} = $props();

	const img = $derived(fallback(images, shownWidth));
	const offered = $derived(sources(images, { largestPaint: priority }));
	const preloads = $derived(
		priority ? offered.filter((source) => source.type === 'image/avif' && source.srcset) : []
	);
</script>

<svelte:head>
	{#each preloads as preload (preload.media ?? '')}
		<link
			rel="preload"
			as="image"
			type="image/avif"
			imagesrcset={preload.srcset}
			imagesizes={sizes}
			media={preload.media}
			fetchpriority="high"
		/>
	{/each}
</svelte:head>

{#if img}
	<picture class={className}>
		{#each offered as source (`${source.media ?? ''} ${source.type}`)}
			<source type={source.type} srcset={source.srcset} media={source.media} {sizes} />
		{/each}
		<img
			src={img.url}
			{alt}
			width={img.width}
			height={img.height}
			loading={priority ? 'eager' : 'lazy'}
			fetchpriority={priority ? 'high' : 'auto'}
			decoding={priority ? 'sync' : 'async'}
		/>
	</picture>
{/if}
