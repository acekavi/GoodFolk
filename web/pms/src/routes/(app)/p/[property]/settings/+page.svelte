<!--
	The property's Settings index: every settings page the user may open, by group. Sections live on Rooms and
	cancellation policies on Rate plans; Staff and Permissions are the tenant's.
-->
<script lang="ts">
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { createQuery } from '@tanstack/svelte-query';
	import { fetchMe } from '$lib/session';
	import { settingsIndex, type SettingsLink } from '$lib/settings';

	const property = $derived(page.params.property ?? '');
	const me = createQuery(() => ({ queryKey: ['me'], queryFn: fetchMe }));
	const hrefs: Record<SettingsLink, string> = $derived({
		general: resolve('/(app)/p/[property]/settings/general', { property }),
		taxes: resolve('/(app)/p/[property]/settings/taxes', { property }),
		'exchange-rates': resolve('/(app)/p/[property]/settings/exchange-rates', { property }),
		sections: resolve('/(app)/p/[property]/rooms', { property }),
		'block-reasons': resolve('/(app)/p/[property]/settings/block-reasons', { property }),
		outlets: resolve('/(app)/p/[property]/settings/outlets', { property }),
		'laundry-prices': resolve('/(app)/p/[property]/settings/laundry-prices', { property }),
		'cancellation-policies': resolve('/(app)/p/[property]/rate-plans', { property }),
		staff: resolve('/(app)/settings/staff'),
		permissions: resolve('/(app)/settings/permissions')
	});
	const groups = $derived(me.data ? settingsIndex(me.data, property) : []);
</script>

<h1>Settings</h1>
<div class="groups">
	{#each groups as group (group.group)}
		<section aria-labelledby="group-{group.group}">
			<h2 id="group-{group.group}">{group.group}</h2>
			<ul>
				{#each group.links as entry (entry.link)}
					<li>
						<!-- eslint-disable-next-line svelte/no-navigation-without-resolve -- every href is resolved in `hrefs` -->
						<a href={hrefs[entry.link]}>{entry.label}</a>
					</li>
				{/each}
			</ul>
		</section>
	{/each}
</div>

<style>
	.groups {
		display: grid;
		grid-template-columns: repeat(auto-fill, minmax(14rem, 1fr));
		gap: var(--space);
	}
	h2 {
		font-size: 1.1rem;
	}
</style>
