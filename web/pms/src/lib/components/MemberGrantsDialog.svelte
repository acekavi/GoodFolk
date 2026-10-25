<!--
	Changes a member's roles: the rows replace every grant they hold. The dialog edits the member as they were when it
	opened and sends that version as `If-Match`: a stale one (412) reloads the members and starts again from the
	latest. A grant the member already holds is kept as it is, even one the signed-in user could not give.
-->
<script lang="ts">
	import { tick, untrack } from 'svelte';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { ApiError, errorMessage } from '$lib/api/problem';
	import { ifMatch, rest, unwrap } from '$lib/api/rest';
	import GrantRows from '$lib/components/GrantRows.svelte';
	import {
		grantFromNode,
		grantRows,
		grantsBody,
		membersKey,
		type Grant,
		type GrantRowDraft,
		type MemberRow
	} from '$lib/staff';

	interface Props {
		tenantId: string;
		/** The member from the latest list; the dialog snapshots it on opening and after a 412. */
		member: MemberRow | undefined;
		actor: readonly Grant[];
		properties: readonly { id: string; code: string; name: string }[];
		open: boolean;
	}

	let { tenantId, member, actor, properties, open = $bindable() }: Props = $props();

	const client = useQueryClient();
	let dialog = $state<HTMLDialogElement>();
	let snapshot = $state.raw<{
		userId: string;
		name: string;
		version: number;
		held: Grant[];
	} | null>(null);
	let rows = $state<GrantRowDraft[]>([]);
	let problem = $state('');
	let busy = $state(false);

	/** Takes the member as they now are: their version and their grants. */
	function reset() {
		if (!member) {
			snapshot = null;
			rows = [];
			return;
		}
		const held = member.grants.map(grantFromNode);
		snapshot = { userId: member.userId, name: member.displayName, version: member.version, held };
		rows = grantRows(held);
	}

	$effect(() => {
		if (!dialog) return;
		if (open && !dialog.open) {
			// The snapshot is taken here only: a refetch while the dialog is open must not move its version.
			untrack(reset);
			problem = '';
			dialog.showModal();
		} else if (!open && dialog.open) {
			dialog.close();
		}
	});

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		if (!snapshot || rows.length === 0) return;
		busy = true;
		problem = '';
		try {
			unwrap(
				await rest.PUT('/api/v1/members/{user}/grants', {
					params: { path: { user: snapshot.userId }, header: ifMatch(snapshot.version) },
					body: { grants: grantsBody(rows) }
				})
			);
			open = false;
			void client.invalidateQueries({ queryKey: membersKey(tenantId) });
		} catch (err) {
			if (err instanceof ApiError && err.status === 412) {
				await client.invalidateQueries({ queryKey: membersKey(tenantId) });
				await tick();
				reset();
				problem = snapshot
					? 'Someone else changed this member. The roles now show the latest; make your change again.'
					: 'This member has been removed.';
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
	aria-labelledby="member-grants-title"
	onclose={() => (open = false)}
>
	<form class="form" onsubmit={submit}>
		<h2 id="member-grants-title">Change roles{snapshot ? ` · ${snapshot.name}` : ''}</h2>
		{#if snapshot}
			<GrantRows bind:rows {actor} held={snapshot.held} {properties} disabled={busy} />
			<p class="hint">Takes effect on their next request.</p>
		{/if}
		{#if problem}<p class="error" role="alert">{problem}</p>{/if}
		<div class="actions">
			{#if snapshot}
				<button disabled={busy || rows.length === 0}>Save roles</button>
			{/if}
			<button type="button" class="secondary" onclick={() => (open = false)}>Cancel</button>
		</div>
	</form>
</dialog>
