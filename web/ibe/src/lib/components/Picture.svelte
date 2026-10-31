<script lang="ts">
	import type { Image } from '$lib/api';
	import { fallback, srcset } from '$lib/media';

	/**
	 * A delivered image in AVIF, then WebP, at the width the layout shows it (`sizes`). Lazy unless it is the page's
	 * largest paint (`priority`), which the browser fetches first.
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
</script>

{#if img}
	<picture class={className}>
		<source type="image/avif" srcset={srcset(images, 'avif')} {sizes} />
		<source type="image/webp" srcset={srcset(images, 'webp')} {sizes} />
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
