<script lang="ts">
	import { page } from '$app/state';
</script>

<svelte:head>
	<title>{page.status === 404 ? 'Page not found' : 'Something went wrong'}</title>
	<meta name="robots" content="noindex" />
</svelte:head>

<main class="wrap error">
	{#if page.status === 404}
		<h1>Page not found</h1>
		<p>There is no booking site at this address. Check the link the hotel gave you.</p>
	{:else}
		<h1>Something went wrong</h1>
		<p>The booking site could not load this page. Try again in a moment.</p>
	{/if}
</main>

<style>
	.error {
		padding-block: 5rem;
		display: grid;
		gap: 1rem;
		max-width: 40rem;
	}

	h1 {
		font-size: clamp(2rem, 6vw, 3rem);
	}
</style>
