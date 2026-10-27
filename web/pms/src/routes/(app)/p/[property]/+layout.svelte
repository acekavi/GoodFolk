<script lang="ts">
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { createQuery } from '@tanstack/svelte-query';
	import { can, fetchMe, posOnly } from '$lib/session';
	import { fetchOpsAlerts, opsAlertLabel, opsAlertsKey, seesOpsAlerts } from '$lib/night-audit';
	import { fetchFxAlert, fxAlertKey, seesFxBanner, settingsIndex } from '$lib/settings';

	let { children } = $props();

	const me = createQuery(() => ({ queryKey: ['me'], queryFn: fetchMe }));
	const property = $derived(page.params.property ?? '');
	// The profile is cached by the app layout; until it loads, the link is not offered.
	const seesHousekeeping = $derived(!!me.data && can(me.data, 'housekeepingView', property));
	const seesNightAudit = $derived(!!me.data && can(me.data, 'nightAudit', property));
	const seesLinen = $derived(!!me.data && can(me.data, 'linenView', property));
	const seesLaundry = $derived(!!me.data && can(me.data, 'laundryOrders', property));
	const seesReports = $derived(!!me.data && can(me.data, 'reportsView', property));
	const seesTaxReports = $derived(!!me.data && can(me.data, 'reportsLiability', property));
	const seesDailyLists = $derived(!!me.data && can(me.data, 'viewReservations', property));
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
		...(seesLinen
			? [{ href: resolve('/(app)/p/[property]/linen', { property }), label: 'Linen' }]
			: []),
		...(seesLaundry
			? [{ href: resolve('/(app)/p/[property]/laundry', { property }), label: 'Guest laundry' }]
			: []),
		...(seesNightAudit
			? [{ href: resolve('/(app)/p/[property]/night-audit', { property }), label: 'Night audit' }]
			: []),
		...(seesReports
			? [{ href: resolve('/(app)/p/[property]/reports', { property }), label: 'Reports' }]
			: []),
		...(seesDailyLists
			? [{ href: resolve('/(app)/p/[property]/reports/lists', { property }), label: 'Daily lists' }]
			: []),
		...(seesTaxReports
			? [{ href: resolve('/(app)/p/[property]/reports/tax', { property }), label: 'Tax reports' }]
			: []),
		{ href: resolve('/(app)/p/[property]/accounts', { property }), label: 'Accounts' },
		{ href: resolve('/(app)/p/[property]/room-types', { property }), label: 'Room types' },
		{ href: resolve('/(app)/p/[property]/rooms', { property }), label: 'Rooms' },
		{ href: resolve('/(app)/p/[property]/inventory', { property }), label: 'Inventory' },
		{ href: resolve('/(app)/p/[property]/rate-plans', { property }), label: 'Rate plans' },
		{ href: resolve('/(app)/p/[property]/rates', { property }), label: 'Rates' },
		{ href: resolve('/(app)/p/[property]/meal-plans', { property }), label: 'Meal plans' }
	]);
	// The Settings index lists each settings page the user may open; the link shows when there is one.
	// A till (`posOnly`) is shown no pages.
	const seesSettings = $derived(
		!!me.data && !posOnly(me.data) && settingsIndex(me.data, property).length > 0
	);
	const settings = $derived(resolve('/(app)/p/[property]/settings', { property }));

	// Owners and managers, who can set a rate by hand, see why the last import left rates missing or stale.
	const fxBanner = $derived(!!me.data && seesFxBanner(me.data, property));
	const fxAlert = createQuery(() => ({
		queryKey: fxAlertKey(property),
		queryFn: ({ signal }) => fetchFxAlert(property, signal),
		enabled: fxBanner
	}));
	// Owners and managers see what needs a person: a failed automatic close, drifted inventory counts, or linen
	// below par; housekeeping supervisors see the linen alert.
	const opsBanner = $derived(!!me.data && seesOpsAlerts(me.data, property));
	const opsAlerts = createQuery(() => ({
		queryKey: opsAlertsKey(property),
		queryFn: ({ signal }) => fetchOpsAlerts(property, signal),
		enabled: opsBanner
	}));
	const nightAudit = $derived(resolve('/(app)/p/[property]/night-audit', { property }));
	const linen = $derived(resolve('/(app)/p/[property]/linen', { property }));
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
	{#if seesSettings}
		<a
			class="group"
			href={settings}
			aria-current={page.url.pathname.startsWith(settings) ? 'page' : undefined}>Settings</a
		>
	{/if}
</nav>
<!-- The Exchange rates page shows the alert itself. -->
{#if fxBanner && fxAlert.data && page.url.pathname !== exchangeRates}
	<div class="banner" role="alert">
		<span>Exchange rates: {fxAlert.data.message}</span>
		<a href={exchangeRates}>Go to exchange rates</a>
	</div>
{/if}
{#if opsBanner}
	{#each opsAlerts.data ?? [] as alert (alert.kind)}
		<div class="banner" role="alert">
			<span>{opsAlertLabel(alert.kind)}: {alert.message}</span>
			{#if alert.kind === 'AUDIT' && seesNightAudit && page.url.pathname !== nightAudit}
				<a href={nightAudit}>Go to the night audit</a>
			{:else if alert.kind === 'LINEN' && seesLinen && page.url.pathname !== linen}
				<a href={linen}>Go to linen</a>
			{/if}
		</div>
	{/each}
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
