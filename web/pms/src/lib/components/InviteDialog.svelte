<!--
	Invites someone by email: their name and the roles accepting gives, from what the signed-in user may give. The
	create is idempotent (`formKeys`), so a double click or a retry sends one invitation; the server's refusal (a
	pending invitation already, an existing member, a role the user may not give) shows here and the dialog stays open.
-->
<script lang="ts">
	import { errorMessage } from '$lib/api/problem';
	import { formKeys, rest, unwrap } from '$lib/api/rest';
	import GrantRows from '$lib/components/GrantRows.svelte';
	import { grantsBody, newGrantRow, type Grant, type GrantRowDraft } from '$lib/staff';

	interface Props {
		actor: readonly Grant[];
		properties: readonly { id: string; code: string; name: string }[];
		open: boolean;
		/** After an invitation is sent, with the address it went to. */
		oninvited: (email: string) => void;
	}

	let { actor, properties, open = $bindable(), oninvited }: Props = $props();

	const keys = formKeys();
	let dialog = $state<HTMLDialogElement>();
	let email = $state('');
	let name = $state('');
	let rows = $state<GrantRowDraft[]>([]);
	let problem = $state('');
	let busy = $state(false);

	$effect(() => {
		if (!dialog) return;
		if (open && !dialog.open) {
			const first = newGrantRow(
				actor,
				properties.map((property) => property.id)
			);
			email = '';
			name = '';
			rows = first ? [first] : [];
			problem = '';
			keys.reset();
			dialog.showModal();
		} else if (!open && dialog.open) {
			dialog.close();
		}
	});

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		if (rows.length === 0) return;
		problem = '';
		busy = true;
		const body = { email: email.trim(), display_name: name.trim(), grants: grantsBody(rows) };
		try {
			await keys.submit(body, async (key) =>
				unwrap(
					await rest.POST('/api/v1/invitations', {
						params: { header: { 'Idempotency-Key': key } },
						body
					})
				)
			);
			open = false;
			oninvited(body.email);
		} catch (err) {
			problem = errorMessage(err);
		} finally {
			busy = false;
		}
	}
</script>

<dialog
	class="folio-dialog"
	bind:this={dialog}
	aria-labelledby="invite-title"
	onclose={() => (open = false)}
>
	<form class="form" onsubmit={submit}>
		<h2 id="invite-title">Invite someone</h2>
		<label>Email <input type="email" required maxlength="254" bind:value={email} /></label>
		<label>Name <input required maxlength="200" bind:value={name} /></label>
		<GrantRows bind:rows {actor} {properties} disabled={busy} />
		<p class="hint">They get an email with a link to accept, valid for 7 days.</p>
		{#if problem}<p class="error" role="alert">{problem}</p>{/if}
		<div class="actions">
			<button disabled={busy || rows.length === 0}>Send invitation</button>
			<button type="button" class="secondary" onclick={() => (open = false)}>Cancel</button>
		</div>
	</form>
</dialog>
