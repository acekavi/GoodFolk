<script lang="ts">
	import type { Image } from '$lib/api';
	import { fallback, srcset } from '$lib/media';

	/**
	 * A delivered image in AVIF, then WebP, at the width the layout shows it (`sizes`). Lazy unless it is the page's
	 * largest paint (`priority`), which the browser fetches first: preloaded from the head, before the page's inlined
	 * CSS, as AVIF only (`type` makes a browser without it skip the preload rather than fetch a second format), and
	 * offering the small `phone` rendition, which a phone's screen picks.
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
	const avif = $derived(srcset(images, 'avif', { phone: priority }));
	const webp = $derived(srcset(images, 'webp', { phone: priority }));
</script>

<svelte:head>
	{#if priority && avif}
		<link
			rel="preload"
			as="image"
			type="image/avif"
			imagesrcset={avif}
			imagesizes={sizes}
			fetchpriority="high"
		/>
	{/if}
</svelte:head>

{#if img}
	<picture class={className}>
		<source type="image/avif" srcset={avif} {sizes} />
		<source type="image/webp" srcset={webp} {sizes} />
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
