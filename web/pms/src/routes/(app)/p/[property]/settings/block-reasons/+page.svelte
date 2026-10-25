<!--
	Why a room can be blocked: the property's reasons, which the block dialog offers. `RoomsManage` adds a reason,
	renames one and deactivates it (existing blocks keep it; new blocks can't use it). A code never changes once made.
-->
<script lang="ts">
	import { page } from '$app/state';
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { ApiError, errorMessage } from '$lib/api/problem';
	import { formKeys, ifMatch, rest, unwrap } from '$lib/api/rest';
	import type { components } from '$lib/api/openapi';
	import { Pending } from '$lib/pending.svelte';
	import { roomsKey } from '$lib/rooms';
	import { can, fetchMe } from '$lib/session';
	import { blockReasonsKey, fetchBlockReasons, type BlockReasonRow } from '$lib/settings';

	type BlockKind = components['schemas']['BlockKind'];

	const propertyId = $derived(page.params.property ?? '');
	const client = useQueryClient();
	const me = createQuery(() => ({ queryKey: ['me'], queryFn: fetchMe }));
	const manage = $derived(!!me.data && can(me.data, 'manageRooms', propertyId));
	const reasons = createQuery(() => ({
		queryKey: blockReasonsKey(propertyId),
		queryFn: ({ signal }) => fetchBlockReasons(propertyId, signal)
	}));

	const emptyDraft = () => ({ code: '', label: '', kind: 'out_of_order' as BlockKind });
	let draft = $state(emptyDraft());
	let editing = $state<{ id: string; label: string } | null>(null);
	let error = $state('');
	const pending = new Pending();
	const addForm = formKeys();

	function kindLabel(kind: BlockReasonRow['defaultKind']): string {
		return kind === 'OUT_OF_ORDER' ? 'Out of order' : 'Out of service';
	}

	/** The reasons are read with the rooms too (the block dialog), under the same event. */
	async function refresh() {
		await client.invalidateQueries({ queryKey: roomsKey(propertyId) });
	}

	async function add(event: SubmitEvent) {
		event.preventDefault();
		error = '';
		const body = {
			code: draft.code.trim().toUpperCase(),
			label: draft.label.trim(),
			default_kind: draft.kind
		};
		try {
			await pending.run('add', () =>
				addForm.submit(body, async (key) =>
					unwrap(
						await rest.POST('/api/v1/properties/{property}/block-reasons', {
							params: { path: { property: propertyId }, header: { 'Idempotency-Key': key } },
							body
						})
					)
				)
			);
			draft = emptyDraft();
		} catch (err) {
			error = errorMessage(err);
		} finally {
			await refresh();
		}
	}

	/** PATCHes the reason; a stale `If-Match` (412) reloads the list rather than overwrite someone else's change. */
	async function update(reason: BlockReasonRow, body: { label?: string; active?: boolean }) {
		error = '';
		try {
			await pending.run(reason.id, async () =>
				unwrap(
					await rest.PATCH('/api/v1/properties/{property}/block-reasons/{reason}', {
						params: {
							path: { property: propertyId, reason: reason.id },
							header: ifMatch(reason.version)
						},
						body
					})
				)
			);
			if (editing?.id === reason.id) editing = null;
		} catch (err) {
			if (err instanceof ApiError && err.status === 412) {
				editing = null;
				error =
					'Someone else changed this reason. The list now shows the latest version; make your change again.';
			} else {
				error = errorMessage(err);
			}
		} finally {
			await refresh();
		}
	}
</script>

<h1>Block reasons</h1>
<p class="hint">Why a room is taken out of service; the block dialog offers the active ones.</p>
{#if !manage}
	<p class="hint">Only owners and managers can change the block reasons.</p>
{/if}
{#if error}<p class="error" role="alert">{error}</p>{/if}

{#if reasons.error && !reasons.data}
	<p class="error" role="alert">{errorMessage(reasons.error)}</p>
	<button onclick={() => reasons.refetch()}>Retry</button>
{:else if reasons.data}
	<table aria-label="Block reasons">
		<thead>
			<tr>
				<th>Code</th>
				<th>Label</th>
				<th>Suggested kind</th>
				<th>Active</th>
				{#if manage}<th><span class="visually-hidden">Actions</span></th>{/if}
			</tr>
		</thead>
		<tbody>
			{#each reasons.data as reason (reason.id)}
				<tr class:inactive={!reason.active}>
					<td>{reason.code}</td>
					{#if editing?.id === reason.id}
						<td>
							<input
								aria-label="Label for {reason.code}"
								required
								maxlength="100"
								bind:value={editing.label}
							/>
						</td>
						<td>{kindLabel(reason.defaultKind)}</td>
						<td>{reason.active ? 'Active' : 'Inactive'}</td>
						<td class="actions">
							<button
								aria-label="Save {reason.code}"
								disabled={pending.has(reason.id) || !editing.label.trim()}
								onclick={() => editing && update(reason, { label: editing.label.trim() })}
								>Save</button
							>
							<button class="secondary" onclick={() => (editing = null)}>Cancel</button>
						</td>
					{:else}
						<td>{reason.label}</td>
						<td>{kindLabel(reason.defaultKind)}</td>
						<td>{reason.active ? 'Active' : 'Inactive'}</td>
						{#if manage}
							<td class="actions">
								<button
									class="secondary"
									aria-label="Rename {reason.code}"
									disabled={pending.has(reason.id)}
									onclick={() => (editing = { id: reason.id, label: reason.label })}>Rename</button
								>
								<button
									class="secondary"
									aria-label="{reason.active ? 'Deactivate' : 'Activate'} {reason.code}"
									disabled={pending.has(reason.id)}
									onclick={() => update(reason, { active: !reason.active })}
									>{reason.active ? 'Deactivate' : 'Activate'}</button
								>
							</td>
						{/if}
					{/if}
				</tr>
			{:else}
				<tr><td colspan={manage ? 5 : 4}>No block reasons yet.</td></tr>
			{/each}
		</tbody>
	</table>

	{#if manage}
		<h2>Add a reason</h2>
		<form class="inline-form" aria-label="New block reason" onsubmit={add}>
			<label>
				Code
				<input
					required
					maxlength="20"
					pattern={'[A-Za-z0-9_]{1,20}'}
					placeholder="PAINTING"
					bind:value={draft.code}
				/>
			</label>
			<label>Label <input required maxlength="100" bind:value={draft.label} /></label>
			<label>
				Suggested kind
				<select bind:value={draft.kind}>
					<option value="out_of_order">Out of order (not sellable)</option>
					<option value="out_of_service">Out of service (sellable, flagged)</option>
				</select>
			</label>
			<button disabled={pending.has('add')}>Add reason</button>
		</form>
		<p class="hint">The code can't be changed later.</p>
	{/if}
{:else}
	<p>Loading…</p>
{/if}

<style>
	td input {
		width: 14rem;
	}
</style>
