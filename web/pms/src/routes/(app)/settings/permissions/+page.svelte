<!--
	What each role allows: the server's permission matrix, generated from its role table, as a table with a column
	per role. Read-only, and open to every member.
-->
<script lang="ts">
	import { createQuery } from '@tanstack/svelte-query';
	import { errorMessage } from '$lib/api/problem';
	import {
		fetchPermissionMatrix,
		matrixRows,
		permissionMatrixKey,
		roleLabel,
		ROLES
	} from '$lib/staff';

	const matrix = createQuery(() => ({
		queryKey: permissionMatrixKey,
		queryFn: ({ signal }) => fetchPermissionMatrix(signal),
		staleTime: Infinity
	}));
	const rows = $derived(matrixRows(matrix.data ?? []));
</script>

<h1>Permissions</h1>
<p class="hint">
	What each role allows. Roles are fixed; a grant gives a role at one property or at all of them.
</p>

{#if matrix.error && !matrix.data}
	<p class="error" role="alert">{errorMessage(matrix.error)}</p>
	<button onclick={() => matrix.refetch()}>Retry</button>
{:else if matrix.data}
	<div class="scroll">
		<table aria-label="Permissions">
			<thead>
				<tr>
					<th>Permission</th>
					{#each ROLES as role (role)}
						<th class="role">{roleLabel(role)}</th>
					{/each}
				</tr>
			</thead>
			<tbody>
				{#each rows as row (row.permission)}
					<tr>
						<th scope="row">
							{row.permission}
							<span class="allows">Allows: {row.allows}</span>
						</th>
						{#each row.holds as holds, index (ROLES[index])}
							<td class="role">
								{#if holds}<span aria-label="Yes">✓</span>{:else}<span class="visually-hidden"
										>No</span
									>{/if}
							</td>
						{/each}
					</tr>
				{/each}
			</tbody>
		</table>
	</div>
{:else}
	<p>Loading…</p>
{/if}

<style>
	.scroll {
		overflow-x: auto;
	}
	th[scope='row'] {
		text-align: left;
		font-weight: 600;
	}
	.allows {
		display: block;
		font-weight: normal;
		color: var(--muted);
		font-size: 0.9em;
	}
	.role {
		text-align: center;
	}
</style>
