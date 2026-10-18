<!--
	Refunds part or all of a payment of the reservation: the payment, then the amount (what is left of it at
	most), then the folio the money goes back from, which is the payment's own unless the credit moved, such as
	a deposit that moved to the stay at check-in.
-->
<script lang="ts">
	import { useQueryClient } from '@tanstack/svelte-query';
	import { errorMessage } from '$lib/api/problem';
	import { formKeys, rest, unwrap } from '$lib/api/rest';
	import {
		allFoliosKey,
		formatAmount,
		methodLabel,
		money,
		parseAmount,
		refundable,
		type Folio
	} from '$lib/folio';

	interface Props {
		propertyId: string;
		/** Every folio of the reservation, the payments of all of them offered. */
		folios: readonly Folio[];
		labels: ReadonlyMap<string, string>;
		/** The folio the dialog was opened from: its payments come first. */
		folioId: string;
		open: boolean;
	}

	let { propertyId, folios, labels, folioId, open = $bindable() }: Props = $props();

	const client = useQueryClient();
	const keys = formKeys();
	let dialog = $state<HTMLDialogElement>();
	let form = $state({ paymentId: '', amount: '', fromFolio: '', reason: '' });
	let problem = $state('');
	let busy = $state(false);

	const choices = $derived(
		folios
			.flatMap((folio) => folio.payments.map((payment) => ({ folio, payment })))
			.map((entry) => ({ ...entry, left: refundable(entry.payment, folios) }))
			.filter((entry) => entry.left > 0)
			.sort((a, b) => Number(b.folio.id === folioId) - Number(a.folio.id === folioId))
	);
	const chosen = $derived(choices.find((entry) => entry.payment.id === form.paymentId));
	const sources = $derived(
		folios.filter(
			(folio) => folio.id === chosen?.folio.id || (folio.status === 'OPEN' && !folio.linesOmitted)
		)
	);
	const amount = $derived(chosen ? parseAmount(form.amount, chosen.payment.currency) : null);

	function choose(paymentId: string) {
		const entry = choices.find((choice) => choice.payment.id === paymentId);
		form.paymentId = paymentId;
		form.amount = entry ? formatAmount(entry.left, entry.payment.currency) : '';
		form.fromFolio = entry?.folio.id ?? '';
	}

	$effect(() => {
		if (!dialog) return;
		if (open && !dialog.open) {
			form = { paymentId: '', amount: '', fromFolio: '', reason: '' };
			choose(choices[0]?.payment.id ?? '');
			problem = '';
			keys.reset();
			dialog.showModal();
		} else if (!open && dialog.open) {
			dialog.close();
		}
	});

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		if (!chosen) return;
		if (!amount || amount > chosen.left) {
			problem = `Enter an amount above zero and at most ${money(chosen.left, chosen.payment.currency)}.`;
			return;
		}
		busy = true;
		problem = '';
		const body = {
			amount,
			reason: form.reason.trim(),
			from_folio: form.fromFolio === chosen.folio.id ? null : form.fromFolio
		};
		try {
			unwrap(
				await rest.POST('/api/v1/properties/{property}/payments/{payment}/refunds', {
					params: {
						path: { property: propertyId, payment: chosen.payment.id },
						header: { 'Idempotency-Key': keys.keyFor({ payment: chosen.payment.id, ...body }) }
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
	aria-labelledby="refund-title"
	onclose={() => (open = false)}
>
	<form class="form" onsubmit={submit}>
		<h2 id="refund-title">Refund a payment</h2>
		{#if choices.length === 0}
			<p>No payment of this reservation has anything left to refund.</p>
			<div class="actions">
				<button type="button" class="secondary" onclick={() => (open = false)}>Close</button>
			</div>
		{:else}
			<label>
				Payment
				<select
					required
					value={form.paymentId}
					onchange={(event) => choose(event.currentTarget.value)}
				>
					{#each choices as entry (entry.payment.id)}
						<option value={entry.payment.id}>
							{labels.get(entry.folio.id)} · {methodLabel(entry.payment.method)}
							{money(entry.payment.amount, entry.payment.currency)} · {entry.payment.businessDate}
							· {money(entry.left, entry.payment.currency)} left
						</option>
					{/each}
				</select>
			</label>
			<label>
				Amount{chosen ? ` (${chosen.payment.currency})` : ''}
				<input required inputmode="decimal" autocomplete="off" bind:value={form.amount} />
			</label>
			<label>
				From folio
				<select bind:value={form.fromFolio}>
					{#each sources as folio (folio.id)}
						<option value={folio.id}>{labels.get(folio.id)}</option>
					{/each}
				</select>
			</label>
			<p class="hint">
				The money goes back in the payment's currency and by its method. Choose another folio when
				the credit moved there, such as a deposit that moved to the stay at check-in.
			</p>
			<label>Reason <input required maxlength="500" bind:value={form.reason} /></label>
			{#if problem}<p class="error" role="alert">{problem}</p>{/if}
			<div class="actions">
				<button disabled={busy || !chosen}>Refund</button>
				<button type="button" class="secondary" onclick={() => (open = false)}>Cancel</button>
			</div>
		{/if}
	</form>
</dialog>
