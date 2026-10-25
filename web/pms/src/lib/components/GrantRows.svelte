<!--
	The grant rows of the Invite and Change roles dialogs: each a role, and a property or "All properties". The
	choices are what the signed-in user may give (`roleOptions`, `scopeOptions`), plus whatever the member already
	holds, which is kept as it is. A UI hint only: the server applies the manager rules regardless.
-->
<script lang="ts">
	import {
		ALL,
		newGrantRow,
		roleLabel,
		roleOptions,
		scopeOptions,
		type Grant,
		type GrantRowDraft
	} from '$lib/staff';

	interface Props {
		rows: GrantRowDraft[];
		/** The signed-in user's grants. */
		actor: readonly Grant[];
		/** The member's grants as the dialog opened, or none for an invitation. */
		held?: readonly Grant[];
		properties: readonly { id: string; code: string; name: string }[];
		disabled?: boolean;
	}

	let { rows = $bindable(), actor, held = [], properties, disabled = false }: Props = $props();

	const ids = $derived(properties.map((property) => property.id));
	const roles = $derived(roleOptions(actor, held));
	const fresh = $derived(newGrantRow(actor, ids));

	function scopeLabel(scope: string): string {
		if (scope === ALL) return 'All properties';
		const property = properties.find((p) => p.id === scope);
		return property ? `${property.code} · ${property.name}` : 'Another property';
	}

	/** A new role may not be given where the row was: it moves to the first place it may be. */
	function roleChanged(row: GrantRowDraft) {
		const scopes = scopeOptions(actor, row.role, ids, held);
		if (!scopes.includes(row.scope)) row.scope = scopes[0] ?? ALL;
	}
</script>

<fieldset class="grants" {disabled}>
	<legend>Roles</legend>
	{#each rows as row, index (index)}
		<div class="row">
			<select aria-label="Role {index + 1}" bind:value={row.role} onchange={() => roleChanged(row)}>
				{#each roles as role (role)}
					<option value={role}>{roleLabel(role)}</option>
				{/each}
			</select>
			<select aria-label="Where {index + 1}" bind:value={row.scope}>
				{#each scopeOptions(actor, row.role, ids, held) as scope (scope)}
					<option value={scope}>{scopeLabel(scope)}</option>
				{/each}
			</select>
			{#if rows.length > 1}
				<button
					type="button"
					class="secondary"
					aria-label="Remove role {index + 1}"
					onclick={() => (rows = rows.filter((_, i) => i !== index))}>Remove</button
				>
			{/if}
		</div>
	{/each}
	{#if fresh}
		<button
			type="button"
			class="secondary"
			onclick={() => fresh && (rows = [...rows, { ...fresh }])}>Add a role</button
		>
	{/if}
</fieldset>

<style>
	.grants {
		display: grid;
		gap: 0.5rem;
		border: 1px solid var(--border);
		border-radius: var(--radius);
	}
	.row {
		display: flex;
		flex-wrap: wrap;
		gap: 0.5rem;
		align-items: center;
	}
</style>
