<script lang="ts">
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { createQuery } from '@tanstack/svelte-query';
	import { can, fetchMe } from '$lib/session';

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
</script>

<nav class="property-nav" aria-label="Property">
	{#each links as link (link.href)}
		<a href={link.href} aria-current={page.url.pathname === link.href ? 'page' : undefined}
			>{link.label}</a
		>
	{/each}
</nav>
{@render children()}

<style>
	.property-nav {
		display: flex;
		gap: var(--space);
		margin-bottom: var(--space);
	}
	.property-nav a[aria-current='page'] {
		font-weight: 600;
	}
</style>
