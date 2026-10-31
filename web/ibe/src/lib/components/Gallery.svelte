<script lang="ts">
	import type { SiteMedia } from '$lib/api';
	import Picture from './Picture.svelte';

	/**
	 * A room type's photos and tour videos, one at a time: swiped, or moved with the buttons. A video shows its poster
	 * until it is tapped, then plays with sound, so nothing loads or plays before the guest asks.
	 */
	let { media, name }: { media: readonly SiteMedia[]; name: string } = $props();

	let track: HTMLUListElement | undefined = $state();
	let current = $state(0);
	let playing: number | null = $state(null);

	function show(index: number) {
		if (!track) return;
		const target = Math.max(0, Math.min(media.length - 1, index));
		track.scrollTo({ left: target * track.clientWidth, behavior: 'smooth' });
	}

	function scrolled() {
		if (!track) return;
		current = Math.round(track.scrollLeft / Math.max(1, track.clientWidth));
		if (playing !== null && playing !== current) playing = null;
	}

	function label(item: SiteMedia, index: number) {
		const what = item.kind === 'video' ? 'Tour video' : 'Photo';
		return item.caption ?? `${what} ${index + 1} of ${name}`;
	}
</script>

{#if media.length}
	<div class="gallery" role="region" aria-roledescription="carousel" aria-label="Photos of {name}">
		<ul class="track" bind:this={track} onscroll={scrolled}>
			{#each media as item, index (index)}
				<li class="slide" aria-roledescription="slide" aria-label="{index + 1} of {media.length}">
					{#if item.kind === 'video' && playing === index}
						<video controls autoplay playsinline preload="auto" aria-label={label(item, index)}>
							{#each item.sources as source (source.url)}
								<source src={source.url} type={source.mime} />
							{/each}
						</video>
					{:else}
						<Picture
							images={item.images}
							alt={item.kind === 'video' ? '' : label(item, index)}
							sizes="(min-width: 60rem) 34rem, 100vw"
							shownWidth={1024}
						/>
						{#if item.kind === 'video'}
							<button type="button" class="play" onclick={() => (playing = index)}>
								<span aria-hidden="true">▶</span> Play the tour video
							</button>
						{/if}
					{/if}
				</li>
			{/each}
		</ul>
		{#if media.length > 1}
			<div class="controls">
				<button
					type="button"
					aria-label="Previous photo of {name}"
					disabled={current === 0}
					onclick={() => show(current - 1)}>‹</button
				>
				<span class="count" aria-live="polite">{current + 1} / {media.length}</span>
				<button
					type="button"
					aria-label="Next photo of {name}"
					disabled={current === media.length - 1}
					onclick={() => show(current + 1)}>›</button
				>
			</div>
		{/if}
	</div>
{/if}

<style>
	.gallery {
		position: relative;
	}

	.track {
		display: flex;
		overflow-x: auto;
		scroll-snap-type: x mandatory;
		scrollbar-width: none;
		list-style: none;
		margin: 0;
		padding: 0;
		border-radius: var(--radius);
		background: var(--line);
	}

	.track::-webkit-scrollbar {
		display: none;
	}

	.slide {
		position: relative;
		flex: 0 0 100%;
		aspect-ratio: 4 / 3;
		scroll-snap-align: start;
	}

	.slide :global(picture),
	.slide :global(img),
	.slide video {
		width: 100%;
		height: 100%;
		object-fit: cover;
		background: #000;
	}

	.play {
		position: absolute;
		left: 1rem;
		bottom: 1rem;
		display: inline-flex;
		gap: 0.5rem;
		align-items: center;
		min-height: 2.75rem;
		padding: 0.5rem 1rem;
		border: 0;
		border-radius: 999px;
		background: rgb(20 28 26 / 0.85);
		color: #fff;
		font-weight: 600;
		cursor: pointer;
	}

	.controls {
		position: absolute;
		right: 0.75rem;
		top: 0.75rem;
		display: flex;
		align-items: center;
		gap: 0.25rem;
		padding: 0.15rem;
		border-radius: 999px;
		background: rgb(20 28 26 / 0.85);
		color: #fff;
	}

	.controls button {
		width: 2.75rem;
		height: 2.75rem;
		border: 0;
		border-radius: 50%;
		background: transparent;
		color: #fff;
		font-size: 1.5rem;
		line-height: 1;
		cursor: pointer;
	}

	.controls button:disabled {
		opacity: 0.4;
		cursor: default;
	}

	.count {
		font-size: 0.9rem;
		font-variant-numeric: tabular-nums;
		min-width: 3rem;
		text-align: center;
	}
</style>
