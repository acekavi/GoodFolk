<script lang="ts">
	import { page } from '$app/state';
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import {
		accountKindLabel,
		accountsKey,
		contactSummary,
		fetchAccounts,
		formatCreditLimit,
		type Account
	} from '$lib/accounts';
	import { ApiError, errorMessage } from '$lib/api/problem';
	import type { components } from '$lib/api/openapi';
	import { formKeys, ifMatch, rest, unwrap } from '$lib/api/rest';
	import { Pending } from '$lib/pending.svelte';
	import { formatMoney, parseMoney } from '$lib/rates';
	import { can, fetchMe } from '$lib/session';

	type Schemas = components['schemas'];
	type AccountKind = Schemas['AccountKind'];

	const SEARCH_DELAY_MS = 300;
	const KINDS: { value: AccountKind; label: string }[] = [
		{ value: 'company', label: 'Company' },
		{ value: 'travel_agent', label: 'Travel agent' }
	];

	const propertyId = $derived(page.params.property ?? '');
	const client = useQueryClient();
	const me = createQuery(() => ({ queryKey: ['me'], queryFn: fetchMe }));
	const manage = $derived(!!me.data && can(me.data, 'manageReservations', propertyId));

	// The search box is typed into directly; the query re-runs a moment after typing stops.
	let searchText = $state('');
	let searched = $state('');
	let searchTimer: ReturnType<typeof setTimeout> | undefined;
	$effect(() => () => clearTimeout(searchTimer));
	function findAccounts() {
		clearTimeout(searchTimer);
		searchTimer = setTimeout(() => (searched = searchText.trim()), SEARCH_DELAY_MS);
	}
	let includeInactive = $state(false);

	const accounts = createQuery(() => ({
		queryKey: accountsKey(propertyId, searched, includeInactive),
		queryFn: ({ signal }) => fetchAccounts(propertyId, searched, includeInactive, signal)
	}));

	interface Draft {
		id: string;
		name: string;
		kind: AccountKind;
		email: string;
		phone: string;
		address: string;
		contactName: string;
		creditLimit: string;
		currency: string;
	}

	function emptyDraft() {
		return {
			name: '',
			kind: 'company' as AccountKind,
			email: '',
			phone: '',
			address: '',
			contactName: '',
			creditLimit: '',
			currency: ''
		};
	}

	function editDraft(account: Account): Draft {
		return {
			id: account.id,
			name: account.name,
			kind: account.kind === 'COMPANY' ? 'company' : 'travel_agent',
			email: account.contact.email ?? '',
			phone: account.contact.phone ?? '',
			address: account.contact.address ?? '',
			contactName: account.contact.contactName ?? '',
			creditLimit:
				account.creditLimit == null
					? ''
					: formatMoney(account.creditLimit, account.currency).replaceAll(',', ''),
			currency: account.currency
		};
	}

	let addDraft = $state(emptyDraft());
	let editing = $state<Draft | null>(null);
	let error = $state('');
	const pending = new Pending();
	const addForm = formKeys();

	/** The text typed for a credit limit, in `currency`: `null` for no limit, blank for none typed. */
	function creditLimitValue(text: string, currency: string): number | null {
		const trimmed = text.trim();
		if (!trimmed) return null;
		const value = parseMoney(trimmed, currency);
		if (value === null) {
			throw new Error(
				'Enter the credit limit as an amount, e.g. 1,000.00, or leave it blank for no limit.'
			);
		}
		return value;
	}

	async function add(event: SubmitEvent) {
		event.preventDefault();
		const currency = addDraft.currency.toUpperCase();
		let creditLimit: number | null;
		try {
			creditLimit = creditLimitValue(addDraft.creditLimit, currency);
		} catch (err) {
			error = (err as Error).message;
			return;
		}
		error = '';
		const body = {
			kind: addDraft.kind,
			name: addDraft.name,
			contact: {
				email: addDraft.email || undefined,
				phone: addDraft.phone || undefined,
				address: addDraft.address || undefined,
				contact_name: addDraft.contactName || undefined
			},
			credit_limit: creditLimit,
			currency
		};
		try {
			await pending.run('add', async () => {
				try {
					unwrap(
						await rest.POST('/api/v1/properties/{property}/accounts', {
							params: {
								path: { property: propertyId },
								header: { 'Idempotency-Key': addForm.keyFor(body) }
							},
							body
						})
					);
				} catch (err) {
					addForm.failed(err);
					throw err;
				}
			});
			addDraft = emptyDraft();
			addForm.reset();
		} catch (err) {
			error = err instanceof Error && !('problem' in err) ? err.message : errorMessage(err);
		} finally {
			await client.invalidateQueries({ queryKey: accountsKey(propertyId) });
		}
	}

	function edit(account: Account) {
		error = '';
		editing = editDraft(account);
	}

	/** PATCHes `account` with `body`, handling a stale `If-Match` (412) as the rate plans page does: reload
	 * the row from the server rather than let a stale save overwrite someone else's change. */
	async function update(account: Account, body: Record<string, unknown>, onSaved?: () => void) {
		error = '';
		try {
			await pending.run(account.id, async () =>
				unwrap(
					await rest.PATCH('/api/v1/properties/{property}/accounts/{account}', {
						params: {
							path: { property: propertyId, account: account.id },
							header: ifMatch(account.version)
						},
						body
					})
				)
			);
			onSaved?.();
		} catch (err) {
			if (err instanceof ApiError && err.status === 412) {
				const fresh = await fetchAccounts(propertyId, searched, includeInactive);
				client.setQueryData(accountsKey(propertyId, searched, includeInactive), fresh);
				const freshAccount = fresh.find((a) => a.id === account.id);
				if (editing?.id === account.id) editing = freshAccount ? editDraft(freshAccount) : null;
				error =
					'Someone else changed this account. The row now shows the latest version; make your change again.';
			} else {
				error = err instanceof Error && !('problem' in err) ? err.message : errorMessage(err);
			}
		} finally {
			await client.invalidateQueries({ queryKey: accountsKey(propertyId) });
		}
	}

	async function save(account: Account) {
		if (!editing) return;
		const d = editing;
		const currency = d.currency.toUpperCase();
		let creditLimit: number | null;
		try {
			creditLimit = creditLimitValue(d.creditLimit, currency);
		} catch (err) {
			error = (err as Error).message;
			return;
		}
		await update(
			account,
			{
				kind: d.kind,
				name: d.name,
				email: d.email || null,
				phone: d.phone || null,
				address: d.address || null,
				contact_name: d.contactName || null,
				credit_limit: creditLimit,
				currency
			},
			() => (editing = null)
		);
	}

	function toggleActive(account: Account) {
		void update(account, { active: !account.active });
	}
</script>

<h1>Accounts</h1>
<p class="hint">Companies and travel agents a reservation can be billed to.</p>
{#if error}<p class="error" role="alert">{error}</p>{/if}

<div class="toolbar">
	<label>
		Search
		<input
			type="search"
			maxlength="100"
			placeholder="Search by name"
			bind:value={searchText}
			oninput={findAccounts}
		/>
	</label>
	<label class="check">
		<input type="checkbox" bind:checked={includeInactive} /> Show inactive
	</label>
</div>

{#if accounts.error}
	<p class="error" role="alert">{errorMessage(accounts.error)}</p>
{:else if accounts.data}
	<table aria-label="Accounts">
		<thead>
			<tr>
				<th>Name</th>
				<th>Kind</th>
				<th>Contact</th>
				<th>Credit limit</th>
				<th>Currency</th>
				<th>Active</th>
				{#if manage}<th><span class="visually-hidden">Actions</span></th>{/if}
			</tr>
		</thead>
		<tbody>
			{#each accounts.data as account (account.id)}
				<tr class:inactive={!account.active}>
					{#if editing?.id === account.id}
						<td><input aria-label="Name for {account.name}" bind:value={editing.name} /></td>
						<td>
							<select aria-label="Kind for {account.name}" bind:value={editing.kind}>
								{#each KINDS as k (k.value)}
									<option value={k.value}>{k.label}</option>
								{/each}
							</select>
						</td>
						<td class="contact-fields">
							<input
								aria-label="Email for {account.name}"
								type="email"
								maxlength="254"
								bind:value={editing.email}
							/>
							<input
								aria-label="Phone for {account.name}"
								maxlength="30"
								bind:value={editing.phone}
							/>
							<input
								aria-label="Address for {account.name}"
								maxlength="500"
								bind:value={editing.address}
							/>
							<input
								aria-label="Contact name for {account.name}"
								maxlength="200"
								bind:value={editing.contactName}
							/>
						</td>
						<td>
							<input
								aria-label="Credit limit for {account.name}"
								inputmode="decimal"
								placeholder="No limit"
								bind:value={editing.creditLimit}
							/>
						</td>
						<td>
							<input
								aria-label="Currency for {account.name}"
								pattern={'[A-Za-z]{3}'}
								bind:value={editing.currency}
							/>
						</td>
						<td>{account.active ? 'Active' : 'Inactive'}</td>
						<td class="actions">
							<button
								aria-label="Save {account.name}"
								disabled={pending.has(account.id)}
								onclick={() => save(account)}>Save</button
							>
							<button class="secondary" onclick={() => (editing = null)}>Cancel</button>
						</td>
					{:else}
						<td>{account.name}</td>
						<td>{accountKindLabel(account.kind)}</td>
						<td>{contactSummary(account.contact) || '—'}</td>
						<td>{formatCreditLimit(account.creditLimit, account.currency)}</td>
						<td>{account.currency}</td>
						<td>{account.active ? 'Active' : 'Inactive'}</td>
						{#if manage}
							<td class="actions">
								<button
									class="secondary"
									aria-label="Edit {account.name}"
									disabled={pending.has(account.id)}
									onclick={() => edit(account)}>Edit</button
								>
								<button
									class="secondary"
									disabled={pending.has(account.id)}
									aria-label="{account.active ? 'Deactivate' : 'Activate'} {account.name}"
									onclick={() => toggleActive(account)}
									>{account.active ? 'Deactivate' : 'Activate'}</button
								>
							</td>
						{/if}
					{/if}
				</tr>
			{:else}
				<tr><td colspan={manage ? 7 : 6}>No accounts yet.</td></tr>
			{/each}
		</tbody>
	</table>

	{#if manage}
		<h2>Add an account</h2>
		<form class="inline-form" aria-label="New account" onsubmit={add}>
			<label>Name <input required maxlength="200" bind:value={addDraft.name} /></label>
			<label>
				Kind
				<select bind:value={addDraft.kind}>
					{#each KINDS as k (k.value)}
						<option value={k.value}>{k.label}</option>
					{/each}
				</select>
			</label>
			<label>Email <input type="email" maxlength="254" bind:value={addDraft.email} /></label>
			<label>Phone <input maxlength="30" bind:value={addDraft.phone} /></label>
			<label>Address <input maxlength="500" bind:value={addDraft.address} /></label>
			<label>Contact name <input maxlength="200" bind:value={addDraft.contactName} /></label>
			<label
				>Credit limit
				<input
					inputmode="decimal"
					placeholder="No limit"
					bind:value={addDraft.creditLimit}
				/></label
			>
			<label
				>Currency <input required pattern={'[A-Za-z]{3}'} bind:value={addDraft.currency} /></label
			>
			<button disabled={pending.has('add')}>Add account</button>
		</form>
	{/if}
{:else}
	<p>Loading…</p>
{/if}

<style>
	.toolbar {
		display: flex;
		gap: var(--space);
		align-items: center;
		margin-bottom: var(--space);
	}
	.check {
		display: flex;
		gap: 0.4rem;
		align-items: center;
	}
	.contact-fields {
		display: flex;
		flex-direction: column;
		gap: 0.25rem;
	}
	.contact-fields input {
		width: 100%;
	}
</style>
