<!--
	The Batches tab's Send form, for `LinenMove`: a store or pantry, the laundry or vendor it goes to, and a count per
	item. More than the location holds is the server's 409, shown as it words it.
-->
<script lang="ts">
	import { useQueryClient } from '@tanstack/svelte-query';
	import { errorMessage } from '$lib/api/problem';
	import { formKeys, rest, unwrap } from '$lib/api/rest';
	import {
		batchLocations,
		countItems,
		linenKey,
		type LinenItem,
		type LinenLocation
	} from '$lib/linen';
	import { Pending } from '$lib/pending.svelte';

	interface Props {
		propertyId: string;
		/** The active items, in the order the form lists them. */
		items: readonly LinenItem[];
		locations: readonly LinenLocation[];
	}

	const { propertyId, items, locations }: Props = $props();

	const client = useQueryClient();
	const pending = new Pending();
	const keys = formKeys();
	const sides = $derived(batchLocations(locations));
	const emptySend = () => ({ from: '', to: '', note: '', counts: {} as Record<string, number> });
	let send = $state(emptySend());
	let problem = $state('');
	let notice = $state('');

	async function sendBatch(event: SubmitEvent) {
		event.preventDefault();
		problem = '';
		notice = '';
		const lines = countItems(send.counts);
		if (lines.length === 0) {
			problem = 'Enter a count for at least one item.';
			return;
		}
		const body = {
			from_location_id: send.from,
			to_location_id: send.to,
			items: lines,
			note: send.note.trim() || null
		};
		try {
			await pending.run('send', () =>
				keys.submit(body, async (key) =>
					unwrap(
						await rest.POST('/api/v1/properties/{property}/laundry-batches', {
							params: { path: { property: propertyId }, header: { 'Idempotency-Key': key } },
							body
						})
					)
				)
			);
			send = emptySend();
			notice = 'Batch sent.';
		} catch (err) {
			problem = errorMessage(err);
		} finally {
			await client.invalidateQueries({ queryKey: linenKey(propertyId) });
		}
	}
</script>

<section aria-labelledby="send-batch">
	<h2 id="send-batch">Send a batch</h2>
	{#if problem}<p class="error" role="alert">{problem}</p>{/if}
	{#if notice}<p role="status">{notice}</p>{/if}
	<form class="send" aria-label="Send a batch" onsubmit={sendBatch}>
		<div class="inline-form">
			<label>
				From
				<select required bind:value={send.from}>
					<option value="" disabled>Choose a store or pantry</option>
					{#each sides.from as location (location.id)}
						<option value={location.id}>{location.name}</option>
					{/each}
				</select>
			</label>
			<label>
				To
				<select required bind:value={send.to}>
					<option value="" disabled>Choose a laundry or vendor</option>
					{#each sides.to as location (location.id)}
						<option value={location.id}>{location.name}</option>
					{/each}
				</select>
			</label>
			<label>Note <input maxlength="500" bind:value={send.note} /></label>
		</div>
		<table aria-label="Counts to send">
			<thead><tr><th>Item</th><th>Count</th></tr></thead>
			<tbody>
				{#each items as item (item.id)}
					<tr>
						<td>{item.name}</td>
						<td>
							<input
								type="number"
								min="0"
								max="100000"
								aria-label="{item.name} to send"
								bind:value={send.counts[item.id]}
							/>
						</td>
					</tr>
				{:else}
					<tr><td colspan="2">No linen items yet.</td></tr>
				{/each}
			</tbody>
		</table>
		<button disabled={pending.has('send')}>Send</button>
	</form>
</section>
