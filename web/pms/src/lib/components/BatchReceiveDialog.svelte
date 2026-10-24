<!--
	Counts a laundry batch back in. `receive` counts what came back (each item defaulting to what is out), showing the
	difference from what was sent before confirming; what is short stays at the laundry. A received batch with items
	still out is settled later: `return` counts items found (to the store or pantry chosen), `write_off` writes items
	off with a reason (`LinenManage`). A stale version (412) reloads the batch and its counts.
-->
<script lang="ts">
	import { tick } from 'svelte';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { ApiError, errorMessage } from '$lib/api/problem';
	import { ifMatch, rest, unwrap } from '$lib/api/rest';
	import {
		batchLocations,
		countDifferences,
		countItems,
		linenKey,
		type BatchCountMode,
		type LaundryBatch,
		type LinenItem,
		type LinenLocation
	} from '$lib/linen';

	interface Props {
		propertyId: string;
		/** The batch, from the latest list, so a refetch brings its new version. */
		batch: LaundryBatch | undefined;
		mode: BatchCountMode;
		items: readonly LinenItem[];
		locations: readonly LinenLocation[];
		open: boolean;
	}

	let { propertyId, batch, mode, items, locations, open = $bindable() }: Props = $props();

	const client = useQueryClient();
	let dialog = $state<HTMLDialogElement>();
	let counts = $state<Record<string, number>>({});
	let backTo = $state('');
	let reason = $state('');
	let problem = $state('');
	let busy = $state(false);

	const names = $derived(new Map(items.map((item) => [item.id, item.name])));
	const destinations = $derived(batchLocations(locations).from);
	const lines = $derived(countDifferences(batch?.lines ?? [], counts));
	const short = $derived(lines.reduce((sum, line) => sum + Math.max(0, -line.difference), 0));
	const counted = $derived(lines.reduce((sum, line) => sum + line.counted, 0));
	const titles: Record<BatchCountMode, string> = {
		receive: 'Receive a batch',
		return: 'Return found items',
		write_off: 'Write off outstanding items'
	};
	const confirms: Record<BatchCountMode, string> = {
		receive: 'Confirm receipt',
		return: 'Return',
		write_off: 'Write off'
	};

	/** Counts start at what is out, except found items, which start at none. */
	function reset() {
		counts = Object.fromEntries(
			(batch?.lines ?? [])
				.filter((line) => line.outstanding > 0)
				.map((line) => [line.itemId, mode === 'return' ? 0 : line.outstanding])
		);
		backTo = batch?.fromLocationId ?? '';
	}

	$effect(() => {
		if (!dialog) return;
		if (open && !dialog.open) {
			reset();
			reason = '';
			problem = '';
			dialog.showModal();
		} else if (!open && dialog.open) {
			dialog.close();
		}
	});

	function request(current: LaundryBatch) {
		const params = {
			path: { property: propertyId, batch: current.id },
			header: ifMatch(current.version)
		};
		const itemsBody = countItems(counts);
		const back = backTo && backTo !== current.fromLocationId ? backTo : null;
		if (mode === 'receive') {
			return rest.POST('/api/v1/properties/{property}/laundry-batches/{batch}/receive', {
				params,
				body: { back_to: back, items: itemsBody }
			});
		}
		return rest.POST('/api/v1/properties/{property}/laundry-batches/{batch}/settle', {
			params,
			body:
				mode === 'return'
					? { how: 'return', back_to: back, items: itemsBody }
					: { how: 'write_off', reason: reason.trim(), items: itemsBody }
		});
	}

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		if (!batch) return;
		if (mode !== 'receive' && counted === 0) {
			problem = 'Enter at least one item.';
			return;
		}
		busy = true;
		problem = '';
		try {
			unwrap(await request(batch));
			open = false;
			void client.invalidateQueries({ queryKey: linenKey(propertyId) });
		} catch (err) {
			if (err instanceof ApiError && err.status === 412) {
				await client.invalidateQueries({ queryKey: linenKey(propertyId) });
				await tick();
				reset();
				problem =
					'Someone else changed this batch. The counts now show what is still out; check them and confirm again.';
			} else {
				problem = errorMessage(err);
			}
		} finally {
			busy = false;
		}
	}
</script>

<dialog
	class="folio-dialog"
	bind:this={dialog}
	aria-labelledby="batch-count-title"
	onclose={() => (open = false)}
>
	<form class="form" onsubmit={submit}>
		<h2 id="batch-count-title">{titles[mode]}</h2>
		{#if lines.length === 0}
			<p>Nothing of this batch is still out.</p>
			<div class="actions">
				<button type="button" class="secondary" onclick={() => (open = false)}>Close</button>
			</div>
		{:else}
			<table>
				<thead>
					<tr>
						<th>Item</th>
						<th>Out</th>
						<th>{mode === 'receive' ? 'Counted' : mode === 'return' ? 'Found' : 'Write off'}</th>
						{#if mode === 'receive'}<th>Difference</th>{/if}
					</tr>
				</thead>
				<tbody>
					{#each lines as line (line.itemId)}
						<tr class:short={mode === 'receive' && line.difference < 0}>
							<td>{names.get(line.itemId) ?? 'Unknown item'}</td>
							<td>{line.outstanding}</td>
							<td>
								<input
									type="number"
									required
									min="0"
									max={line.outstanding}
									aria-label="{names.get(line.itemId) ?? 'Item'} count"
									bind:value={counts[line.itemId]}
								/>
							</td>
							{#if mode === 'receive'}
								<td>{line.difference === 0 ? 'None' : line.difference}</td>
							{/if}
						</tr>
					{/each}
				</tbody>
			</table>
			{#if mode === 'receive'}
				<p role="status">
					{short === 0
						? 'Everything sent is counted.'
						: `${short} short: they stay at the laundry until returned or written off.`}
				</p>
			{/if}
			{#if mode !== 'write_off'}
				<label>
					Back to
					<select bind:value={backTo}>
						{#each destinations as location (location.id)}
							<option value={location.id}>{location.name}</option>
						{/each}
					</select>
				</label>
			{:else}
				<label>Reason <input required maxlength="500" bind:value={reason} /></label>
			{/if}
			{#if problem}<p class="error" role="alert">{problem}</p>{/if}
			<div class="actions">
				<button disabled={busy}>{confirms[mode]}</button>
				<button type="button" class="secondary" onclick={() => (open = false)}>Cancel</button>
			</div>
		{/if}
	</form>
</dialog>

<style>
	tr.short td {
		color: var(--danger);
	}
</style>
