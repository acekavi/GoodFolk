<script lang="ts">
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { createQuery } from '@tanstack/svelte-query';
	import { can, fetchMe } from '$lib/session';
	import { fetchFxAlert, fxAlertKey, seesFxBanner, settingsAccess } from '$lib/settings';

	let { children } = $props();

	const me = createQuery(() => ({ queryKey: ['me'], queryFn: fetchMe }));
	const property = $derived(page.params.property ?? '');
	// The profile is cached by the app layout; until it loads, the link is not offered.
	const seesHousekeeping = $derived(!!me.data && can(me.data, 'housekeepingView', property));
	const links = $derived([
		{ href: resolve('/(app)/p/[property]', { property }), label: 'Overview' },
		{ href: resolve('/(app)/p/[property]/tape', { property }), label: 'Tape chart' },
		{
			href: resolve('/(app)/p/[property]/reservations/(list)', { property }),
			label: 'Reservations'
		},
		...(seesHousekeeping
			? [{ href: resolve('/(app)/p/[property]/housekeeping', { property }), label: 'Housekeeping' }]
			: []),
		{ href: resolve('/(app)/p/[property]/accounts', { property }), label: 'Accounts' },
		{ href: resolve('/(app)/p/[property]/room-types', { property }), label: 'Room types' },
		{ href: resolve('/(app)/p/[property]/rooms', { property }), label: 'Rooms' },
		{ href: resolve('/(app)/p/[property]/inventory', { property }), label: 'Inventory' },
		{ href: resolve('/(app)/p/[property]/rate-plans', { property }), label: 'Rate plans' },
		{ href: resolve('/(app)/p/[property]/rates', { property }), label: 'Rates' },
		{ href: resolve('/(app)/p/[property]/meal-plans', { property }), label: 'Meal plans' }
	]);
	const settingsLinks = $derived(
		me.data
			? settingsAccess(me.data, property).map((entry) => ({
					href: resolve(`/(app)/p/[property]/settings/${entry.page}`, { property }),
					label: entry.label
				}))
			: []
	);

	// Owners and managers, who can set a rate by hand, see why the last import left rates missing or stale.
	const fxBanner = $derived(!!me.data && seesFxBanner(me.data, property));
	const fxAlert = createQuery(() => ({
		queryKey: fxAlertKey(property),
		queryFn: ({ signal }) => fetchFxAlert(property, signal),
		enabled: fxBanner
	}));
	const exchangeRates = $derived(
		resolve('/(app)/p/[property]/settings/exchange-rates', { property })
	);
</script>

<nav class="property-nav" aria-label="Property">
	{#each links as link (link.href)}
		<a href={link.href} aria-current={page.url.pathname === link.href ? 'page' : undefined}
			>{link.label}</a
		>
	{/each}
	{#if settingsLinks.length > 0}
		<span class="group" aria-hidden="true">Settings:</span>
		{#each settingsLinks as link (link.href)}
			<a href={link.href} aria-current={page.url.pathname === link.href ? 'page' : undefined}
				>{link.label}</a
			>
		{/each}
	{/if}
</nav>
<!-- The Exchange rates page shows the alert itself. -->
{#if fxBanner && fxAlert.data && page.url.pathname !== exchangeRates}
	<div class="banner" role="alert">
		<span>Exchange rates: {fxAlert.data.message}</span>
		<a href={exchangeRates}>Go to exchange rates</a>
	</div>
{/if}
{@render children()}

<style>
	.property-nav {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space);
		margin-bottom: var(--space);
	}
	.property-nav a[aria-current='page'] {
		font-weight: 600;
	}
	.group {
		color: var(--muted);
		padding-left: var(--space);
		border-left: 1px solid var(--border);
	}
	.banner {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space);
		align-items: baseline;
		margin-bottom: var(--space);
		padding: 0.5rem 0.75rem;
		border: 1px solid var(--danger);
		border-radius: var(--radius);
		color: var(--danger);
	}
</style>
