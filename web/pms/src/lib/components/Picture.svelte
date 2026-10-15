<!--
	A responsive photo: AVIF for browsers that take it, WebP otherwise. `thumb` shows the 320 px
	square crop; `large` the 1600 px long-edge size alone (or the largest a smaller photo has);
	`display` lets the browser pick from the long-edge sizes by `sizes`. Width and height
	reserve the photo's box before it loads, so nothing shifts.
-->
<script lang="ts">
	import {
		DISPLAY_SIZES,
		LARGE_SIZES,
		largestOf,
		renditionsOf,
		srcset,
		type Rendition
	} from '$lib/media';

	interface Props {
		renditions: readonly Rendition[];
		alt: string;
		/** The rendered width, as the `sizes` attribute takes it. */
		sizes: string;
		variant?: 'thumb' | 'large' | 'display';
	}

	let { renditions, alt, sizes, variant = 'display' }: Props = $props();

	const pick = (format: Rendition['format']) =>
		variant === 'thumb'
			? renditionsOf(renditions, format, ['THUMB'])
			: variant === 'large'
				? largestOf(renditions, format, LARGE_SIZES)
				: renditionsOf(renditions, format, DISPLAY_SIZES);
	const avif = $derived(pick('AVIF'));
	const webp = $derived(pick('WEBP'));
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
