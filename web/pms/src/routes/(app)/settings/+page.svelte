<!--
	The tenant's settings: Staff and Permissions, which are not of one property, and each property's own Settings
	index. A link shows only when the user may open its page.
-->
<script lang="ts">
	import { resolve } from '$app/paths';
	import { createQuery } from '@tanstack/svelte-query';
	import { fetchProperties, propertiesKey } from '$lib/properties';
	import { canAnywhere, fetchMe, posOnly } from '$lib/session';
	import { settingsIndex } from '$lib/settings';

	const me = createQuery(() => ({ queryKey: ['me'], queryFn: fetchMe }));
	const properties = createQuery(() => ({
		queryKey: propertiesKey,
		queryFn: ({ signal }) => fetchProperties(signal)
	}));
	const staff = $derived(!!me.data && canAnywhere(me.data, 'staffManage'));
	/** The properties whose index has something besides the Staff group shown here. */
	const withSettings = $derived(
		(properties.data ?? []).filter(
			(property) =>
				!!me.data && settingsIndex(me.data, property.id).some((group) => group.group !== 'Staff')
		)
	);
</script>

<h1>Settings</h1>
<section aria-labelledby="staff-group">
	<h2 id="staff-group">Staff</h2>
	<ul>
		{#if staff}
			<li><a href={resolve('/(app)/settings/staff')}>Staff</a></li>
		{/if}
		{#if me.data && !posOnly(me.data)}
			<li><a href={resolve('/(app)/settings/permissions')}>Permissions</a></li>
		{/if}
	</ul>
</section>
{#if withSettings.length > 0}
	<section aria-labelledby="property-group">
		<h2 id="property-group">Properties</h2>
		<ul>
			{#each withSettings as property (property.id)}
				<li>
					<a href={resolve('/(app)/p/[property]/settings', { property: property.id })}
						>{property.code} · {property.name}</a
					>
				</li>
			{/each}
		</ul>
	</section>
{/if}
