<!--
	Posts a charge by hand on a folio, on the property's business date, taxed by its rules. A charge is never a
	credit: a mistake is reversed. Refusals (such as a missing exchange rate) show here and the dialog stays open.
-->
<script lang="ts">
	import { useQueryClient } from '@tanstack/svelte-query';
	import { errorMessage } from '$lib/api/problem';
	import { formKeys, rest, unwrap } from '$lib/api/rest';
	import { allFoliosKey, categories, parseAmount, type Outlet } from '$lib/folio';

	interface Props {
		propertyId: string;
		folioId: string;
		/** The tab's name, for the heading. */
		folioLabel: string;
		baseCurrency: string;
		currencies: readonly string[];
		outlets: readonly Outlet[];
		/** Set to open the dialog; cleared when it closes. */
		open: boolean;
	}

	let {
		propertyId,
		folioId,
		folioLabel,
		baseCurrency,
		currencies,
		outlets,
		open = $bindable()
	}: Props = $props();

	const client = useQueryClient();
	const keys = formKeys();
	let dialog = $state<HTMLDialogElement>();
	let form = $state({ category: 'misc', description: '', quantity: 1, unitNet: '', currency: '' });
	let problem = $state('');
	let busy = $state(false);

	const unitNet = $derived(parseAmount(form.unitNet, form.currency));

	$effect(() => {
		if (!dialog) return;
		if (open && !dialog.open) {
			form = {
				category: 'misc',
				description: '',
				quantity: 1,
				unitNet: '',
				currency: baseCurrency
			};
			problem = '';
			keys.reset();
			dialog.showModal();
		} else if (!open && dialog.open) {
			dialog.close();
		}
	});

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		if (unitNet === null) {
			problem = `Enter the unit price in ${form.currency}, such as 1,250.00.`;
			return;
		}
		busy = true;
		problem = '';
		const body = {
			category: form.category,
			description: form.description.trim(),
			quantity: form.quantity,
			unit_net: unitNet,
			currency: form.currency
		};
		try {
			unwrap(
				await rest.POST('/api/v1/properties/{property}/folios/{folio}/charges', {
					params: {
						path: { property: propertyId, folio: folioId },
						header: { 'Idempotency-Key': keys.keyFor({ folioId, ...body }) }
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
	aria-labelledby="charge-title"
	onclose={() => (open = false)}
>
	<form class="form" onsubmit={submit}>
		<h2 id="charge-title">Post a charge · {folioLabel}</h2>
		<label>
			Category
			<select bind:value={form.category}>
				{#each categories(outlets) as entry (entry.value)}
					<option value={entry.value}>{entry.label}</option>
				{/each}
			</select>
		</label>
		<label>
			Description
			<input required maxlength="200" bind:value={form.description} />
		</label>
		<label>
			Quantity
			<input type="number" required min="1" max="9999" step="1" bind:value={form.quantity} />
		</label>
		<div class="amount">
			<label>
				Unit price before taxes
				<input required inputmode="decimal" autocomplete="off" bind:value={form.unitNet} />
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
		<p class="hint">Taxes and service charge are added by the property's rules.</p>
		{#if problem}<p class="error" role="alert">{problem}</p>{/if}
		<div class="actions">
			<button disabled={busy}>Post charge</button>
			<button type="button" class="secondary" onclick={() => (open = false)}>Cancel</button>
		</div>
	</form>
</dialog>
