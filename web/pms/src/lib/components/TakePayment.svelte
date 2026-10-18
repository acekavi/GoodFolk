<!--
	Takes a payment on a folio: cash, a card on the terminal (its approval code, last 4 and brand, never the
	number), a bank transfer (its reference) or a charge to the reservation's account. Opens with the folio's
	first amount due. Refusals show here and the dialog stays open.
-->
<script lang="ts">
	import { useQueryClient } from '@tanstack/svelte-query';
	import { errorMessage } from '$lib/api/problem';
	import { formKeys, rest, unwrap } from '$lib/api/rest';
	import {
		allFoliosKey,
		formatAmount,
		methodLabel,
		parseAmount,
		PAYMENT_METHODS,
		type Folio,
		type PaymentMethod
	} from '$lib/folio';

	interface Props {
		propertyId: string;
		folio: Folio;
		folioLabel: string;
		baseCurrency: string;
		currencies: readonly string[];
		/** The reservation is billed to an account, so a payment can be charged to it. */
		hasAccount: boolean;
		open: boolean;
	}

	let {
		propertyId,
		folio,
		folioLabel,
		baseCurrency,
		currencies,
		hasAccount,
		open = $bindable()
	}: Props = $props();

	const client = useQueryClient();
	const keys = formKeys();
	let dialog = $state<HTMLDialogElement>();
	let form = $state({
		method: 'cash' as PaymentMethod,
		currency: '',
		amount: '',
		approval: '',
		last4: '',
		brand: '',
		reference: '',
		evidence: ''
	});
	let problem = $state('');
	let busy = $state(false);

	const methods = $derived(
		PAYMENT_METHODS.filter(
			(method) => method !== 'account' || (hasAccount && folio.kind !== 'ACCOUNT')
		)
	);
	const amount = $derived(parseAmount(form.amount, form.currency));

	$effect(() => {
		if (!dialog) return;
		if (open && !dialog.open) {
			const due = folio.balances.find((balance) => balance.amount > 0);
			const currency = due?.currency ?? baseCurrency;
			form = {
				method: 'cash',
				currency,
				amount: due ? formatAmount(due.amount, currency) : '',
				approval: '',
				last4: '',
				brand: '',
				reference: '',
				evidence: ''
			};
			problem = '';
			keys.reset();
			dialog.showModal();
		} else if (!open && dialog.open) {
			dialog.close();
		}
	});

	function details() {
		if (form.method === 'card_terminal') {
			return {
				approval: form.approval.trim() || null,
				last4: form.last4.trim() || null,
				brand: form.brand.trim() || null
			};
		}
		if (form.method === 'bank_transfer') return { reference: form.reference.trim() || null };
		return null;
	}

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		if (!amount) {
			problem = `Enter an amount in ${form.currency} above zero, such as 1,250.00.`;
			return;
		}
		busy = true;
		problem = '';
		const body = {
			method: form.method,
			amount,
			currency: form.currency,
			details: details(),
			evidence: form.evidence.trim() || null
		};
		try {
			unwrap(
				await rest.POST('/api/v1/properties/{property}/folios/{folio}/payments', {
					params: {
						path: { property: propertyId, folio: folio.id },
						header: { 'Idempotency-Key': keys.keyFor({ folio: folio.id, ...body }) }
					},
					body
				})
			);
			keys.reset();
			void client.invalidateQueries({ queryKey: allFoliosKey(propertyId) });
			open = false;
		} catch (err) {
			keys.failed(err);
			problem = errorMessage(err);
		} finally {
			busy = false;
		}
	}
</script>

<dialog
	class="folio-dialog"
	bind:this={dialog}
	aria-labelledby="payment-title"
	onclose={() => (open = false)}
>
	<form class="form" onsubmit={submit}>
		<h2 id="payment-title">Take a payment · {folioLabel}</h2>
		<label>
			Method
			<select bind:value={form.method}>
				{#each methods as method (method)}
					<option value={method}>{methodLabel(method)}</option>
				{/each}
			</select>
		</label>
		<div class="amount">
			<label>
				Amount
				<input required inputmode="decimal" autocomplete="off" bind:value={form.amount} />
			</label>
			<label>
				Currency
				<select bind:value={form.currency}>
					{#each currencies as currency (currency)}
						<option value={currency}>{currency}</option>
					{/each}
				</select>
			</label>
		</div>
		{#if form.method === 'card_terminal'}
			<label>Approval code <input maxlength="20" bind:value={form.approval} /></label>
			<label>
				Last 4 digits
				<input inputmode="numeric" pattern="[0-9]{'{4}'}" maxlength="4" bind:value={form.last4} />
			</label>
			<label>Brand <input maxlength="20" placeholder="Visa" bind:value={form.brand} /></label>
		{:else if form.method === 'bank_transfer'}
			<label>Reference <input maxlength="100" bind:value={form.reference} /></label>
		{:else if form.method === 'account'}
			<p class="hint">Moves the amount to the account's folio, to be invoiced to the company.</p>
		{/if}
		<label>
			Evidence (optional)
			<textarea
				maxlength="500"
				placeholder="Such as a bank encashment receipt"
				bind:value={form.evidence}></textarea>
		</label>
		{#if problem}<p class="error" role="alert">{problem}</p>{/if}
		<div class="actions">
			<button disabled={busy}>Take payment</button>
			<button type="button" class="secondary" onclick={() => (open = false)}>Cancel</button>
		</div>
	</form>
</dialog>
