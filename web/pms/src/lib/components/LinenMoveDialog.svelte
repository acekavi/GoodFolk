<!--
	Records a linen movement by hand: a receive (bought in, to a store or pantry), a transfer between stores and
	pantries, or, for `LinenManage`, a write-off from any location with its reason. The locations offered follow the
	kind; more than a location holds is the server's 409, shown as it words it.
-->
<script lang="ts">
	import { useQueryClient } from '@tanstack/svelte-query';
	import { errorMessage } from '$lib/api/problem';
	import { formKeys, rest, unwrap } from '$lib/api/rest';
	import {
		linenKey,
		moveLocations,
		type LinenItem,
		type LinenLocation,
		type MoveKind
	} from '$lib/linen';

	interface Props {
		propertyId: string;
		items: readonly LinenItem[];
		locations: readonly LinenLocation[];
		/** Whether write-offs are offered (`LinenManage`). */
		canWriteOff: boolean;
		open: boolean;
	}

	let { propertyId, items, locations, canWriteOff, open = $bindable() }: Props = $props();

	const client = useQueryClient();
	const keys = formKeys();
	let dialog = $state<HTMLDialogElement>();
	const emptyForm = () => ({
		kind: 'receive' as MoveKind,
		itemId: '',
		from: '',
		to: '',
		quantity: 1,
		reason: ''
	});
	let form = $state(emptyForm());
	let problem = $state('');
	let busy = $state(false);

	const activeItems = $derived(items.filter((item) => item.active));
	const sides = $derived(moveLocations(form.kind, locations));
	const kinds = $derived<{ kind: MoveKind; label: string }[]>([
		{ kind: 'receive', label: 'Receive (bought in)' },
		{ kind: 'transfer', label: 'Transfer' },
		...(canWriteOff ? [{ kind: 'write_off' as const, label: 'Write off' }] : [])
	]);

	/** A new kind keeps a location only when the kind still allows it there. */
	function chooseKind(kind: MoveKind) {
		form.kind = kind;
		const next = moveLocations(kind, locations);
		if (!next.from.some((location) => location.id === form.from)) form.from = '';
		if (!next.to.some((location) => location.id === form.to)) form.to = '';
	}

	$effect(() => {
		if (!dialog) return;
		if (open && !dialog.open) {
			form = { ...emptyForm(), itemId: activeItems[0]?.id ?? '' };
			problem = '';
			keys.reset();
			dialog.showModal();
		} else if (!open && dialog.open) {
			dialog.close();
		}
	});

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		if (form.kind === 'transfer' && form.from === form.to) {
			problem = 'Choose two different locations.';
			return;
		}
		busy = true;
		problem = '';
		const body = {
			kind: form.kind,
			item_id: form.itemId,
			quantity: form.quantity,
			from_location_id: sides.from.length > 0 ? form.from : null,
			to_location_id: sides.to.length > 0 ? form.to : null,
			reason: form.kind === 'write_off' ? form.reason.trim() : null
		};
		try {
			await keys.submit(body, async (key) =>
				unwrap(
					await rest.POST('/api/v1/properties/{property}/linen-movements', {
						params: { path: { property: propertyId }, header: { 'Idempotency-Key': key } },
						body
					})
				)
			);
			open = false;
		} catch (err) {
			problem = errorMessage(err);
		} finally {
			busy = false;
			void client.invalidateQueries({ queryKey: linenKey(propertyId) });
		}
	}
</script>

<dialog
	class="folio-dialog"
	bind:this={dialog}
	aria-labelledby="linen-move-title"
	onclose={() => (open = false)}
>
	<form class="form" onsubmit={submit}>
		<h2 id="linen-move-title">Move linen</h2>
		<label>
			Kind
			<select
				value={form.kind}
				onchange={(event) => chooseKind(event.currentTarget.value as MoveKind)}
			>
				{#each kinds as entry (entry.kind)}
					<option value={entry.kind}>{entry.label}</option>
				{/each}
			</select>
		</label>
		<label>
			Item
			<select required bind:value={form.itemId}>
				{#each activeItems as item (item.id)}
					<option value={item.id}>{item.name} ({item.code})</option>
				{/each}
			</select>
		</label>
		{#if sides.from.length > 0}
			<label>
				From
				<select required bind:value={form.from}>
					<option value="" disabled>Choose a location</option>
					{#each sides.from as location (location.id)}
						<option value={location.id}>{location.name}</option>
					{/each}
				</select>
			</label>
		{/if}
		{#if sides.to.length > 0}
			<label>
				To
				<select required bind:value={form.to}>
					<option value="" disabled>Choose a location</option>
					{#each sides.to as location (location.id)}
						<option value={location.id}>{location.name}</option>
					{/each}
				</select>
			</label>
		{/if}
		<label>
			Quantity
			<input type="number" required min="1" max="100000" bind:value={form.quantity} />
		</label>
		{#if form.kind === 'write_off'}
			<label>Reason <input required maxlength="500" bind:value={form.reason} /></label>
		{/if}
		{#if problem}<p class="error" role="alert">{problem}</p>{/if}
		<div class="actions">
			<button disabled={busy || activeItems.length === 0}>Record</button>
			<button type="button" class="secondary" onclick={() => (open = false)}>Cancel</button>
		</div>
	</form>
</dialog>
