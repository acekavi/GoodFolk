<script lang="ts">
	import { page } from '$app/state';
	import Gallery from '$lib/components/Gallery.svelte';
	import Picture from '$lib/components/Picture.svelte';
	import SearchForm from '$lib/components/SearchForm.svelte';
	import { bedsText, plural } from '$lib/format';
	import { formFromUrl } from '$lib/search';
	import { hotelData, jsonLdScript, metaDescription, shareImage } from '$lib/seo';

	let { data } = $props();

	const site = $derived(data.site);
	const canonical = $derived(`${data.origin}${data.base || '/'}`);
	const image = $derived(shareImage(site));
	const description = $derived(metaDescription(site));
</script>

<svelte:head>
	<title>{site.name} | Book your stay direct</title>
	<meta name="description" content={description} />
	<link rel="canonical" href={canonical} />
	<meta property="og:type" content="website" />
	<meta property="og:title" content={site.name} />
	<meta property="og:description" content={description} />
	<meta property="og:url" content={canonical} />
	{#if image}<meta property="og:image" content={image} />{/if}
	<!-- eslint-disable-next-line svelte/no-at-html-tags -- JSON with `<` escaped, built from the site's own data. -->
	{@html jsonLdScript(hotelData(site, canonical))}
</svelte:head>

<section class="hero" class:bare={!site.hero}>
	{#if site.hero}
		<Picture
			images={site.hero.images}
			alt=""
			sizes="100vw"
			shownWidth={1600}
			priority
			class="hero-image"
		/>
	{/if}
	<div class="wrap hero-text">
		<h1>{site.name}</h1>
	</div>
</section>

<main class="wrap landing">
	<div class="intro">
		{#if site.description}
			<p class="description">{site.description}</p>
		{/if}
		<p class="muted times">
			Check-in from {site.check_in_time}, check-out by {site.check_out_time}.
		</p>
	</div>

	<section class="book" aria-labelledby="search-heading">
		<h2 id="search-heading">Find a room</h2>
		<SearchForm
			roomTypes={site.room_types}
			businessDate={site.business_date}
			base={data.base}
			initial={formFromUrl(page.url, site.business_date)}
		/>
	</section>

	{#if site.room_types.length}
		<section class="rooms" aria-labelledby="rooms-heading">
			<h2 id="rooms-heading">The rooms</h2>
			<ul>
				{#each site.room_types as type (type.id)}
					<li class="room">
						<Gallery media={type.media} name={type.name} />
						<div class="room-text">
							<h3>{type.name}</h3>
							{#if type.description}<p>{type.description}</p>{/if}
							<p class="muted">
								{#if type.beds.length}{bedsText(type.beds)}.
								{/if}Sleeps up to {plural(type.max_occupancy, 'guest')}.
							</p>
							{#if type.amenities.length}
								<ul class="amenities" aria-label="Amenities">
									{#each type.amenities as amenity (amenity)}<li>{amenity}</li>{/each}
								</ul>
							{/if}
						</div>
					</li>
				{/each}
			</ul>
		</section>
	{/if}
</main>

<style>
	.hero {
		position: relative;
		display: grid;
		align-items: end;
		min-height: min(70vh, 38rem);
		color: #fff;
		background: var(--accent);
		overflow: hidden;
	}

	.hero.bare {
		min-height: 14rem;
	}

	.hero :global(.hero-image) {
		position: absolute;
		inset: 0;
	}

	.hero :global(.hero-image img) {
		width: 100%;
		height: 100%;
		object-fit: cover;
	}

	/* A scrim under the name, so white text keeps its contrast on any photo. */
	.hero::after {
		content: '';
		position: absolute;
		inset: 0;
		background: linear-gradient(
			to top,
			rgb(12 18 16 / 0.78),
			rgb(12 18 16 / 0.15) 55%,
			transparent
		);
	}

	.hero.bare::after {
		display: none;
	}

	.hero-text {
		position: relative;
		z-index: 1;
		padding-block: 2.5rem;
	}

	h1 {
		max-width: 16ch;
		text-wrap: balance;
	}

	.landing {
		display: grid;
		gap: 3.5rem;
		padding-block: 2.5rem 4rem;
	}

	.intro {
		display: grid;
		gap: 0.75rem;
		max-width: var(--measure);
	}

	.description {
		font-family: var(--serif);
		font-size: clamp(1.15rem, 2.4vw, 1.4rem);
		line-height: 1.6;
	}

	.book,
	.rooms {
		display: grid;
		gap: 1.25rem;
	}

	.book {
		max-width: 40rem;
	}

	.rooms > ul {
		list-style: none;
		margin: 0;
		padding: 0;
		display: grid;
		gap: 3rem;
	}

	.room {
		display: grid;
		gap: 1.25rem;
	}

	.room-text {
		display: grid;
		gap: 0.6rem;
		align-content: start;
		max-width: var(--measure);
	}

	.amenities {
		display: flex;
		flex-wrap: wrap;
		gap: 0.4rem;
		list-style: none;
		margin: 0.25rem 0 0;
		padding: 0;
	}

	.amenities li {
		padding: 0.2rem 0.65rem;
		border: 1px solid var(--line);
		border-radius: 999px;
		font-size: 0.9rem;
		background: var(--surface);
	}

	@media (min-width: 60rem) {
		.landing {
			grid-template-columns: minmax(0, 1fr) minmax(0, 28rem);
			column-gap: 4rem;
		}

		.intro {
			grid-column: 1;
			align-self: start;
		}

		.book {
			grid-column: 2;
			grid-row: 1 / span 2;
			align-self: start;
			position: sticky;
			top: 1.5rem;
		}

		.rooms {
			grid-column: 1;
		}
	}
</style>
