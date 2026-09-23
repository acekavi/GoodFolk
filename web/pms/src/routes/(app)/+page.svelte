<script lang="ts">
	import { resolve } from '$app/paths';
	import { createQuery } from '@tanstack/svelte-query';
	import { fetchProperties, propertiesKey } from '$lib/properties';
	import { fetchMe, isTenantOwner } from '$lib/session';

	const me = createQuery(() => ({ queryKey: ['me'], queryFn: fetchMe }));
	const properties = createQuery(() => ({
		queryKey: propertiesKey,
		queryFn: ({ signal }) => fetchProperties(signal),
		enabled: !!me.data?.current_tenant
	}));
</script>

<h1>Properties</h1>
{#if properties.data?.length === 0}
	<p>No properties yet.</p>
{/if}
<ul>
	{#each properties.data ?? [] as property (property.id)}
		<li>
			<a href={resolve('/(app)/p/[property]', { property: property.id })}
				>{property.code} · {property.name}</a
			>
		</li>
	{/each}
</ul>
{#if me.data && isTenantOwner(me.data)}
	<p><a href={resolve('/properties/new')}>Add a property</a></p>
{/if}
