<!--
	A responsive photo: AVIF for browsers that take it, WebP otherwise. `thumb` shows the 320 px
	square crop; `display` lets the browser pick from the long-edge sizes by `sizes`. Width and height
	reserve the photo's box before it loads, so nothing shifts.
-->
<script lang="ts">
	import { DISPLAY_SIZES, renditionsOf, srcset, type Rendition } from '$lib/media';

	interface Props {
		renditions: readonly Rendition[];
		alt: string;
		/** The rendered width, as the `sizes` attribute takes it. */
		sizes: string;
		variant?: 'thumb' | 'display';
	}

	let { renditions, alt, sizes, variant = 'display' }: Props = $props();

	const chosen = $derived(variant === 'thumb' ? (['THUMB'] as const) : DISPLAY_SIZES);
	const avif = $derived(renditionsOf(renditions, 'AVIF', chosen));
	const webp = $derived(renditionsOf(renditions, 'WEBP', chosen));
	/** The largest WebP sets the aspect ratio; every size shares it. */
	const box = $derived(webp.at(-1));
</script>

{#if box}
	<picture>
		{#if avif.length > 0}
			<source type="image/avif" srcset={srcset(avif)} {sizes} />
		{/if}
		<img
			src={webp[0].url}
			srcset={srcset(webp)}
			{sizes}
			width={box.width}
			height={box.height}
			{alt}
			loading="lazy"
			decoding="async"
		/>
	</picture>
{/if}

<style>
	picture {
		display: contents;
	}
	img {
		display: block;
		width: 100%;
		height: auto;
	}
</style>
