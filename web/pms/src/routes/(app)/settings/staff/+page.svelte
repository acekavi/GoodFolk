<!--
	The tenant's staff (`StaffManage`): its members, with their roles and when they joined, and the invitations still
	waiting. Invite, Change roles, Remove, Resend and Revoke show only where the manager rules let the user act (a
	manager acts on members whose every grant is a junior role at a property they manage); the server enforces them
	regardless. Live through the server's `staff:<tenant>` events.
-->
<script lang="ts">
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { errorMessage } from '$lib/api/problem';
	import { rest, unwrap } from '$lib/api/rest';
	import InviteDialog from '$lib/components/InviteDialog.svelte';
	import MemberGrantsDialog from '$lib/components/MemberGrantsDialog.svelte';
	import { Pending } from '$lib/pending.svelte';
	import { fetchProperties, propertiesKey } from '$lib/properties';
	import { canAnywhere, fetchMe } from '$lib/session';
	import {
		fetchMembers,
		fetchOpenInvitations,
		grantFromNode,
		grantText,
		invitationsKey,
		mayActOn,
		mayEditMember,
		membersKey,
		newGrantRow,
		staffKey,
		type InvitationRow,
		type MemberRow
	} from '$lib/staff';

	const client = useQueryClient();
	const me = createQuery(() => ({ queryKey: ['me'], queryFn: fetchMe }));
	const tenantId = $derived(me.data?.current_tenant ?? '');
	const manage = $derived(!!me.data && canAnywhere(me.data, 'staffManage'));
	const actor = $derived(me.data?.grants ?? []);
	const properties = createQuery(() => ({
		queryKey: propertiesKey,
		queryFn: ({ signal }) => fetchProperties(signal)
	}));
	const propertyList = $derived(properties.data ?? []);
	const names = $derived(new Map(propertyList.map((property) => [property.id, property.name])));
	const members = createQuery(() => ({
		queryKey: membersKey(tenantId),
		queryFn: ({ signal }) => fetchMembers(signal),
		enabled: manage && !!tenantId
	}));
	const invitations = createQuery(() => ({
		queryKey: invitationsKey(tenantId),
		queryFn: ({ signal }) => fetchOpenInvitations(signal),
		enabled: manage && !!tenantId
	}));
	const mayInvite = $derived(
		newGrantRow(
			actor,
			propertyList.map((property) => property.id)
		) !== null
	);

	const pending = new Pending();
	let error = $state('');
	let notice = $state('');
	let inviting = $state(false);
	let editingId = $state<string | null>(null);
	let editing = $state(false);
	const editingMember = $derived(members.data?.find((member) => member.userId === editingId));

	function grantsOf(row: { grants: readonly { role: string; propertyId?: string | null }[] }) {
		return row.grants.map(grantFromNode);
	}

	function day(at: string): string {
		return new Date(at).toLocaleDateString(undefined, { dateStyle: 'medium' });
	}

	async function refresh() {
		await client.invalidateQueries({ queryKey: staffKey(tenantId) });
	}

	function changeRoles(member: MemberRow) {
		error = '';
		notice = '';
		editingId = member.userId;
		editing = true;
	}

	async function remove(member: MemberRow) {
		if (
			!confirm(
				`Remove ${member.displayName} (${member.email})? They lose access to every property at once and are signed out here.`
			)
		)
			return;
		error = '';
		notice = '';
		try {
			await pending.run(member.userId, async () =>
				unwrap(
					await rest.DELETE('/api/v1/members/{user}', { params: { path: { user: member.userId } } })
				)
			);
			notice = `${member.displayName} has been removed.`;
		} catch (err) {
			error = errorMessage(err);
		} finally {
			await refresh();
		}
	}

	async function resend(invitation: InvitationRow) {
		error = '';
		notice = '';
		try {
			await pending.run(invitation.id, async () =>
				unwrap(
					await rest.POST('/api/v1/invitations/{invitation}/resend', {
						params: { path: { invitation: invitation.id } }
					})
				)
			);
			notice = `The invitation to ${invitation.email} was sent again; the earlier link no longer works.`;
		} catch (err) {
			error = errorMessage(err);
		} finally {
			await refresh();
		}
	}

	async function revoke(invitation: InvitationRow) {
		if (!confirm(`Revoke the invitation to ${invitation.email}? Its link will stop working.`))
			return;
		error = '';
		notice = '';
		try {
			await pending.run(invitation.id, async () =>
				unwrap(
					await rest.POST('/api/v1/invitations/{invitation}/revoke', {
						params: { path: { invitation: invitation.id } }
					})
				)
			);
			notice = `The invitation to ${invitation.email} was revoked.`;
		} catch (err) {
			error = errorMessage(err);
		} finally {
			await refresh();
		}
	}
</script>

<h1>Staff</h1>
{#if me.data && !manage}
	<p class="hint">Only owners and managers manage staff.</p>
{:else if me.data}
	{#if mayInvite}
		<div class="toolbar">
			<button
				onclick={() => {
					error = '';
					notice = '';
					inviting = true;
				}}>Invite</button
			>
		</div>
	{/if}
	{#if error}<p class="error" role="alert">{error}</p>{/if}
	{#if notice}<p class="hint" role="status">{notice}</p>{/if}

	<section aria-labelledby="members-title">
		<h2 id="members-title">Members</h2>
		{#if members.error && !members.data}
			<p class="error" role="alert">{errorMessage(members.error)}</p>
			<button onclick={() => members.refetch()}>Retry</button>
		{:else if members.data}
			<div class="scroll">
				<table aria-label="Members">
					<thead>
						<tr>
							<th>Name</th>
							<th>Email</th>
							<th>Roles</th>
							<th>Joined</th>
							<th><span class="visually-hidden">Actions</span></th>
						</tr>
					</thead>
					<tbody>
						{#each members.data as member (member.userId)}
							{@const grants = grantsOf(member)}
							<tr>
								<td>{member.displayName}</td>
								<td>{member.email}</td>
								<td>
									<ul class="grants">
										{#each grants as grant, index (index)}
											<li>{grantText(grant, names)}</li>
										{/each}
									</ul>
								</td>
								<td class="date">{day(member.joinedAt)}</td>
								<td class="actions">
									{#if mayEditMember(me.data, { userId: member.userId, grants })}
										<button
											class="secondary"
											aria-label="Change roles of {member.displayName}"
											disabled={pending.has(member.userId)}
											onclick={() => changeRoles(member)}>Change roles</button
										>
										<button
											class="secondary"
											aria-label="Remove {member.displayName}"
											disabled={pending.has(member.userId)}
											onclick={() => remove(member)}>Remove</button
										>
									{/if}
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{:else}
			<p>Loading…</p>
		{/if}
	</section>

	<section aria-labelledby="invitations-title">
		<h2 id="invitations-title">Invitations</h2>
		{#if invitations.error && !invitations.data}
			<p class="error" role="alert">{errorMessage(invitations.error)}</p>
			<button onclick={() => invitations.refetch()}>Retry</button>
		{:else if invitations.data}
			{#if invitations.data.length === 0}
				<p class="hint">No invitations waiting.</p>
			{:else}
				<div class="scroll">
					<table aria-label="Invitations">
						<thead>
							<tr>
								<th>Name</th>
								<th>Email</th>
								<th>Roles</th>
								<th>Status</th>
								<th><span class="visually-hidden">Actions</span></th>
							</tr>
						</thead>
						<tbody>
							{#each invitations.data as invitation (invitation.id)}
								{@const grants = grantsOf(invitation)}
								<tr>
									<td>{invitation.displayName}</td>
									<td>{invitation.email}</td>
									<td>
										<ul class="grants">
											{#each grants as grant, index (index)}
												<li>{grantText(grant, names)}</li>
											{/each}
										</ul>
									</td>
									<td>
										{invitation.status === 'EXPIRED'
											? `Expired ${day(invitation.expiresAt)}`
											: `Sent ${day(invitation.createdAt)}, expires ${day(invitation.expiresAt)}`}
									</td>
									<td class="actions">
										{#if mayActOn(actor, grants)}
											<button
												class="secondary"
												aria-label="Resend to {invitation.email}"
												disabled={pending.has(invitation.id)}
												onclick={() => resend(invitation)}>Resend</button
											>
											{#if invitation.status === 'PENDING'}
												<button
													class="secondary"
													aria-label="Revoke the invitation to {invitation.email}"
													disabled={pending.has(invitation.id)}
													onclick={() => revoke(invitation)}>Revoke</button
												>
											{/if}
										{/if}
									</td>
								</tr>
							{/each}
						</tbody>
					</table>
				</div>
			{/if}
		{:else}
			<p>Loading…</p>
		{/if}
	</section>

	<InviteDialog
		{actor}
		properties={propertyList}
		bind:open={inviting}
		oninvited={(email) => {
			notice = `An invitation was sent to ${email}.`;
			void refresh();
		}}
	/>
	<MemberGrantsDialog
		{tenantId}
		member={editingMember}
		{actor}
		properties={propertyList}
		bind:open={editing}
	/>
{:else}
	<p>Loading…</p>
{/if}

<style>
	.toolbar {
		margin-bottom: var(--space);
	}
	.scroll {
		overflow-x: auto;
	}
	.grants {
		margin: 0;
		padding-left: 1rem;
	}
	.date {
		white-space: nowrap;
	}
</style>
