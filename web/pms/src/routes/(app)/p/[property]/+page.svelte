<script lang="ts">
	import { page } from '$app/state';
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { ApiError, errorMessage } from '$lib/api/problem';
	import { ifMatch, rest, unwrap } from '$lib/api/rest';
	import { Pending } from '$lib/pending.svelte';
	import { fetchProperties, propertiesKey } from '$lib/properties';
	import { can, fetchMe } from '$lib/session';

	const client = useQueryClient();
	const me = createQuery(() => ({ queryKey: ['me'], queryFn: fetchMe }));
	const properties = createQuery(() => ({
		queryKey: propertiesKey,
		queryFn: ({ signal }) => fetchProperties(signal)
	}));
	const property = $derived(properties.data?.find((p) => p.id === page.params.property));
	const manage = $derived(!!me.data && !!property && can(me.data, 'propertiesManage', property.id));

	const pending = new Pending();
	let auditTime = $state<string | null>(null);
	let problem = $state('');

	async function saveAuditTime(event: SubmitEvent) {
		event.preventDefault();
		if (!property || auditTime === null) return;
		problem = '';
		const id = property.id;
		try {
			await pending.run('auditTime', async () =>
				unwrap(
					await rest.PATCH('/api/v1/properties/{property}', {
						params: { path: { property: id }, header: ifMatch(property.version) },
						body: { audit_time: auditTime }
					})
				)
			);
			auditTime = null;
		} catch (err) {
			problem =
				err instanceof ApiError && err.status === 412
					? 'Someone else changed this property. It now shows the latest version; try again.'
					: errorMessage(err);
			if (err instanceof ApiError && err.status === 412) auditTime = null;
		} finally {
			await client.invalidateQueries({ queryKey: propertiesKey });
		}
	}
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
		<dt>Audit time</dt>
		<dd>
			{#if auditTime !== null}
				<form class="inline-form" aria-label="Audit time" onsubmit={saveAuditTime}>
					<label>
						<span class="visually-hidden">Audit time</span>
						<input type="time" required bind:value={auditTime} />
					</label>
					<button disabled={pending.has('auditTime')}>Save</button>
					<button type="button" class="secondary" onclick={() => (auditTime = null)}>Cancel</button>
				</form>
			{:else}
				{property.auditTime}
				{#if manage}
					<button class="secondary" onclick={() => (auditTime = property.auditTime)}>Edit</button>
				{/if}
			{/if}
			<p class="hint">
				When the day closes by itself if nobody has closed it: on the business date at 12:00 or
				later, the next morning when earlier.
			</p>
			{#if problem}<p class="error" role="alert">{problem}</p>{/if}
		</dd>
	</dl>
{:else if properties.isSuccess}
	<p>Property not found.</p>
{/if}
