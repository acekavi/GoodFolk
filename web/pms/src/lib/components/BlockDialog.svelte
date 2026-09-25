<!--
	Blocks a room for [from, until): until is the first day the room is back. A 409 lists the blocks in
	the way, and the dialog stays open so the dates can be changed.
-->
<script lang="ts">
	import { useQueryClient } from '@tanstack/svelte-query';
	import { errorMessage } from '$lib/api/problem';
	import { formKeys, rest, unwrap } from '$lib/api/rest';
	import { addDays, conflictMessages, inventoryKey, monthOf, shiftMonth } from '$lib/inventory';
	import type { BlockReason, Room, RoomType } from '$lib/rooms';

	type Kind = 'out_of_order' | 'out_of_service';

	interface Props {
		propertyId: string;
		businessDate: string;
		rooms: Room[];
		roomTypes: RoomType[];
		reasons: BlockReason[];
		/** Set to open the dialog; cleared when it closes. */
		open: boolean;
		/** The first day suggested when the dialog opens. */
		from: string;
	}

	let {
		propertyId,
		businessDate,
		rooms,
		roomTypes,
		reasons,
		open = $bindable(),
		from
	}: Props = $props();

	const client = useQueryClient();
	let dialog = $state<HTMLDialogElement>();
	let form = $state({
		roomId: '',
		from: '',
		until: '',
		kind: 'out_of_order' as Kind,
		reasonId: '',
		note: ''
	});
	let problems = $state<string[]>([]);
	let busy = $state(false);
	const blockKeys = formKeys();

	const activeRooms = $derived(rooms.filter((room) => room.active));
	const activeReasons = $derived(reasons.filter((reason) => reason.active));
	const typeCode = $derived(new Map(roomTypes.map((type) => [type.id, type.code])));
	const roomNumber = (roomId: string) => rooms.find((room) => room.id === roomId)?.number ?? '?';

	$effect(() => {
		if (!dialog) return;
		if (open && !dialog.open) {
			const start = from < businessDate ? businessDate : from;
			form = {
				roomId: form.roomId || (activeRooms[0]?.id ?? ''),
				from: start,
				until: addDays(start, 1),
				kind: 'out_of_order',
				reasonId: '',
				note: ''
			};
			problems = [];
			blockKeys.reset();
			dialog.showModal();
		} else if (!open && dialog.open) {
			dialog.close();
		}
	});

	function chooseReason(reasonId: string) {
		form.reasonId = reasonId;
		const reason = reasons.find((r) => r.id === reasonId);
		if (reason)
			form.kind = reason.defaultKind === 'OUT_OF_ORDER' ? 'out_of_order' : 'out_of_service';
	}

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		busy = true;
		problems = [];
		try {
			const body = {
				from: form.from,
				to: form.until,
				kind: form.kind,
				reason_id: form.reasonId,
				note: form.note
			};
			unwrap(
				await rest.POST('/api/v1/properties/{property}/rooms/{room}/blocks', {
					params: {
						path: { property: propertyId, room: form.roomId },
						header: { 'Idempotency-Key': blockKeys.keyFor({ room: form.roomId, ...body }) }
					},
					body
				})
			);
			// The server's events refetch these too; invalidating now shows the change at once.
			const last = monthOf(addDays(form.until, -1));
			for (let month = monthOf(form.from); month <= last; month = shiftMonth(month, 1)) {
				void client.invalidateQueries({ queryKey: inventoryKey(propertyId, month) });
			}
			blockKeys.reset();
			open = false;
		} catch (err) {
			const conflicts = conflictMessages(err, roomNumber);
			problems = conflicts.length > 0 ? conflicts : [errorMessage(err)];
		} finally {
			busy = false;
		}
	}
</script>

<dialog bind:this={dialog} aria-labelledby="block-title" onclose={() => (open = false)}>
	<form class="form" onsubmit={submit}>
		<h2 id="block-title">Block a room</h2>
		<label>
			Room
			<select required bind:value={form.roomId}>
				{#each activeRooms as room (room.id)}
					<option value={room.id}>{room.number} · {typeCode.get(room.roomTypeId)}</option>
				{/each}
			</select>
		</label>
		<label>From <input type="date" required min={businessDate} bind:value={form.from} /></label>
		<label>
			Until (first day back)
			<input
				type="date"
				required
				min={form.from ? addDays(form.from, 1) : businessDate}
				bind:value={form.until}
			/>
		</label>
		<label>
			Reason
			<select
				required
				value={form.reasonId}
				onchange={(event) => chooseReason(event.currentTarget.value)}
			>
				<option value="" disabled>Choose a reason</option>
				{#each activeReasons as reason (reason.id)}
					<option value={reason.id}>{reason.label}</option>
				{/each}
			</select>
		</label>
		<label>
			Kind
			<select bind:value={form.kind}>
				<option value="out_of_order">Out of order (not sellable)</option>
				<option value="out_of_service">Out of service (sellable, flagged)</option>
			</select>
		</label>
		<label>Note <textarea maxlength="500" bind:value={form.note}></textarea></label>
		{#if problems.length > 0}
			<div class="error" role="alert">
				{#each problems as problem, index (index)}<p>{problem}</p>{/each}
			</div>
		{/if}
		<div class="actions">
			<button disabled={busy}>Block room</button>
			<button type="button" class="secondary" onclick={() => (open = false)}>Cancel</button>
		</div>
	</form>
</dialog>
