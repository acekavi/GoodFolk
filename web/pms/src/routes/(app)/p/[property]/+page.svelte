<script lang="ts">
	import { page } from '$app/state';
	import { createQuery } from '@tanstack/svelte-query';
	import { fetchProperties, propertiesKey } from '$lib/properties';

	const properties = createQuery(() => ({
		queryKey: propertiesKey,
		queryFn: ({ signal }) => fetchProperties(signal)
	}));
	const property = $derived(properties.data?.find((p) => p.id === page.params.property));
</script>

{#if property}
	<h1>{property.name}</h1>
	<dl>
		<dt>Code</dt>
		<dd>{property.code}</dd>
		<dt>Time zone</dt>
		<dd>{property.timezone}</dd>
		<dt>Base currency</dt>
		<dd>{property.baseCurrency}</dd>
		<dt>Business date</dt>
		<dd>{property.businessDate}</dd>
		<dt>Check-in / check-out</dt>
		<dd>{property.checkInTime} / {property.checkOutTime}</dd>
	</dl>
{:else if properties.isSuccess}
	<p>Property not found.</p>
{/if}
