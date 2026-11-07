<script lang="ts">
	import { navigating } from '$app/state';
	import Picture from '$lib/components/Picture.svelte';
	import { apiOrigin } from '$lib/config';
	import { mediaOrigin } from '$lib/media';

	let { data, children } = $props();

	/** What the guest is told while the next page loads. */
	const loading = $derived.by(() => {
		const route = navigating.to?.route.id;
		if (route === '/[code=code]/rooms') return 'Finding rooms for your dates…';
		if (route === '/[code=code]/book') return 'Checking the prices…';
		return navigating.to ? 'Loading…' : null;
	});

	const media = $derived(mediaOrigin(data.site));
</script>

<svelte:head>
	<!-- The photos and the API's answers come from their own origins: connect early, before the page asks. -->
	{#if media}<link rel="preconnect" href={media} />{/if}
	<link rel="preconnect" href={apiOrigin()} crossorigin="anonymous" />
</svelte:head>

<div class="site" style:--accent={data.site.accent ?? undefined}>
	<header class="wrap top">
		<a class="brand" href={data.base || '/'}>
			{#if data.site.logo.length}
				<Picture images={data.site.logo} alt={data.site.name} sizes="10rem" shownWidth={320} />
			{:else}
				<span>{data.site.name}</span>
			{/if}
		</a>
	</header>

	<p class="loading" role="status" aria-live="polite">
		{#if loading}<span>{loading}</span>{/if}
	</p>

	{@render children()}

	<footer class="foot">
		<div class="wrap">
			<p class="name">{data.site.name}</p>
			{#if data.site.contact_phone || data.site.contact_email}
				<ul>
					{#if data.site.contact_phone}
						<li>
							<a href="tel:{data.site.contact_phone.replace(/\s+/g, '')}"
								>{data.site.contact_phone}</a
							>
						</li>
					{/if}
					{#if data.site.contact_email}
						<li><a href="mailto:{data.site.contact_email}">{data.site.contact_email}</a></li>
					{/if}
				</ul>
			{/if}
			<p class="muted">Booked direct with the hotel. You pay at the hotel.</p>
		</div>
	</footer>
</div>

<style>
	.site {
		min-height: 100vh;
		display: flex;
		flex-direction: column;
	}

	.top {
		display: flex;
		align-items: center;
		min-height: 4rem;
	}

	.brand {
		color: var(--ink);
		text-decoration: none;
		font-family: var(--serif);
		font-size: 1.25rem;
		font-weight: 600;
	}

	.brand :global(img) {
		height: 2.5rem;
		width: auto;
	}

	.loading {
		position: fixed;
		inset: 0 0 auto;
		z-index: 10;
		margin: 0;
		text-align: center;
		pointer-events: none;
	}

	.loading span {
		display: inline-block;
		margin-top: 0.75rem;
		padding: 0.5rem 1rem;
		border-radius: 999px;
		background: var(--ink);
		color: #fff;
		font-size: 0.95rem;
	}

	.foot {
		margin-top: auto;
		padding-block: 2.5rem;
		border-top: 1px solid var(--line);
		background: var(--surface);
	}

	.foot .wrap {
		display: grid;
		gap: 0.5rem;
	}

	.name {
		font-family: var(--serif);
		font-size: 1.15rem;
		font-weight: 600;
	}

	ul {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-wrap: wrap;
		gap: 0.25rem 1.5rem;
	}
</style>
