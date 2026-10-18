<!--
	One folio of the reservation: its balances per currency with their base equivalents, its lines (each opens
	to its taxes), payments and invoices, and the actions each permission allows. Lines never change: a mistake
	is reversed (with a reason) or moved to another folio. An invoice's PDF opens through a fresh, audited link
	each time, in a new tab, or as a link here when the browser blocks the tab; it is never kept.
-->
<script lang="ts">
	import { useQueryClient } from '@tanstack/svelte-query';
	import { errorMessage } from '$lib/api/problem';
	import { formKeys, rest, unwrap } from '$lib/api/rest';
	import {
		allFoliosKey,
		balanceState,
		balanceText,
		canCorrect,
		categoryLabel,
		folioActions,
		invoiceBlocker,
		invoiceLink,
		methodLabel,
		money,
		paymentDetails,
		ratePercent,
		reversedLines,
		transferCounterpart,
		uncredited,
		type Folio,
		type FolioPermissions,
		type Invoice,
		type Outlet
	} from '$lib/folio';
	import { Pending } from '$lib/pending.svelte';
	import PostCharge from './PostCharge.svelte';
	import RefundDialog from './RefundDialog.svelte';
	import TakePayment from './TakePayment.svelte';
	import TransferDialog from './TransferDialog.svelte';

	interface Props {
		propertyId: string;
		folio: Folio;
		/** Every folio of the reservation, for transfers, refunds and naming the other side of a move. */
		folios: readonly Folio[];
		labels: ReadonlyMap<string, string>;
		baseCurrency: string;
		currencies: readonly string[];
		outlets: readonly Outlet[];
		can: FolioPermissions;
		hasAccount: boolean;
	}

	const {
		propertyId,
		folio,
		folios,
		labels,
		baseCurrency,
		currencies,
		outlets,
		can,
		hasAccount
	}: Props = $props();

	const client = useQueryClient();
	const pending = new Pending();
	const invoiceKeys = formKeys();
	const reverseKeys = formKeys();
	const creditKeys = formKeys();
	let problem = $state('');

	const label = $derived(labels.get(folio.id) ?? 'Folio');
	const actions = $derived(folioActions(folio, can));
	const blocker = $derived(invoiceBlocker(folio));
	const reversed = $derived(reversedLines(folio.lines));
	const descriptions = $derived(new Map(folio.lines.map((line) => [line.id, line.description])));
	const creditable = $derived(uncredited(folio));

	let charging = $state(false);
	let paying = $state(false);
	let refunding = $state(false);
	let transferring = $state(false);
	let transferLine = $state('');
	let expanded = $state<string | null>(null);
	let reversing = $state<string | null>(null);
	let reverseReason = $state('');
	let crediting = $state<string | null>(null);
	let creditReason = $state('');
	/** A PDF link the browser would not open in a tab, kept only while its dialog is open. */
	let blocked = $state<{ serial: string; url: string } | null>(null);
	let viewer = $state<HTMLDialogElement>();

	function day(at: string): string {
		return new Date(at).toLocaleDateString(undefined, { dateStyle: 'medium' });
	}

	async function run(key: string, send: () => Promise<unknown>): Promise<boolean> {
		problem = '';
		try {
			await pending.run(key, send);
			return true;
		} catch (err) {
			problem = errorMessage(err);
			return false;
		} finally {
			void client.invalidateQueries({ queryKey: allFoliosKey(propertyId) });
		}
	}

	async function issueInvoice() {
		const key = invoiceKeys.keyFor({ folio: folio.id, version: folio.version });
		const done = await run('invoice', async () => {
			try {
				return unwrap(
					await rest.POST('/api/v1/properties/{property}/folios/{folio}/invoice', {
						params: {
							path: { property: propertyId, folio: folio.id },
							header: { 'Idempotency-Key': key }
						}
					})
				);
			} catch (err) {
				invoiceKeys.failed(err);
				throw err;
			}
		});
		if (done) invoiceKeys.reset();
	}

	function openReverse(lineId: string) {
		problem = '';
		reverseReason = '';
		reverseKeys.reset();
		reversing = lineId;
	}

	async function reverse(event: SubmitEvent, lineId: string) {
		event.preventDefault();
		const body = { reason: reverseReason.trim() };
		const key = reverseKeys.keyFor({ line: lineId, ...body });
		const done = await run(`reverse:${lineId}`, async () => {
			try {
				return unwrap(
					await rest.POST('/api/v1/properties/{property}/folios/{folio}/lines/{line}/reversal', {
						params: {
							path: { property: propertyId, folio: folio.id, line: lineId },
							header: { 'Idempotency-Key': key }
						},
						body
					})
				);
			} catch (err) {
				reverseKeys.failed(err);
				throw err;
			}
		});
		if (done) {
			reverseKeys.reset();
			reversing = null;
		}
	}

	function openCredit(invoiceId: string) {
		problem = '';
		creditReason = '';
		creditKeys.reset();
		crediting = invoiceId;
	}

	async function creditNote(event: SubmitEvent, invoice: Invoice) {
		event.preventDefault();
		const body = { reason: creditReason.trim() };
		const key = creditKeys.keyFor({ invoice: invoice.id, ...body });
		const done = await run(`credit:${invoice.id}`, async () => {
			try {
				return unwrap(
					await rest.POST('/api/v1/properties/{property}/invoices/{invoice}/credit-note', {
						params: {
							path: { property: propertyId, invoice: invoice.id },
							header: { 'Idempotency-Key': key }
						},
						body
					})
				);
			} catch (err) {
				creditKeys.failed(err);
				throw err;
			}
		});
		if (done) {
			creditKeys.reset();
			crediting = null;
		}
	}

	async function viewInvoice(invoice: Invoice) {
		problem = '';
		try {
			const { url } = await pending.run(`view:${invoice.id}`, () =>
				invoiceLink(propertyId, invoice.id)
			);
			const tab = window.open(url, '_blank');
			if (tab) {
				// The PDF must not reach back into the app.
				tab.opener = null;
				return;
			}
			blocked = { serial: invoice.serial, url };
			viewer?.showModal();
		} catch (err) {
			problem = errorMessage(err);
		}
	}

	function openTransfer(lineId: string) {
		transferLine = lineId;
		transferring = true;
	}
</script>

<section class="folio" aria-label="Folio {label}">
	{#if folio.status === 'INVOICED'}
		<p class="hint">Invoiced: it takes no charges or payments until a credit note reopens it.</p>
	{/if}
	{#if folio.linesOmitted}
		<p class="hint">This company's ledger is shared; open the account to see its lines.</p>
	{/if}
	{#if folio.truncated}
		<p class="hint">Showing the newest 500.</p>
	{/if}

	<h4>Balance</h4>
	{#if folio.balances.length === 0}
		<p class="hint">Nothing posted yet.</p>
	{:else}
		<ul class="balances" aria-label="Balances">
			{#each folio.balances as balance (balance.currency)}
				<li class={balanceState(balance)}>
					{balanceText(balance, baseCurrency)}
					<span class="hint">
						{balanceState(balance) === 'due'
							? 'due'
							: balanceState(balance) === 'credit'
								? 'in credit'
								: 'settled'}
					</span>
				</li>
			{/each}
		</ul>
	{/if}

	{#if problem}<p class="error" role="alert">{problem}</p>{/if}

	<div class="toolbar">
		{#if actions.postCharge}
			<button type="button" onclick={() => (charging = true)}>Post charge</button>
		{/if}
		{#if actions.takePayment}
			<button type="button" onclick={() => (paying = true)}>Take payment</button>
		{/if}
		{#if actions.refund}
			<button type="button" class="secondary" onclick={() => (refunding = true)}>Refund</button>
		{/if}
		{#if actions.correct && folio.lines.length > 0}
			<button type="button" class="secondary" onclick={() => openTransfer('')}>Transfer</button>
		{/if}
		{#if actions.issueInvoice}
			<button
				type="button"
				class="secondary"
				disabled={!!blocker || pending.has('invoice')}
				aria-describedby={blocker ? `blocker-${folio.id}` : undefined}
				onclick={issueInvoice}>Issue invoice</button
			>
		{/if}
	</div>
	{#if actions.issueInvoice && blocker}
		<p class="hint" id="blocker-{folio.id}">{blocker}</p>
	{/if}

	{#if !folio.linesOmitted}
		<h4>Lines</h4>
		{#if folio.lines.length === 0}
			<p class="hint">No charges.</p>
		{:else}
			<div class="scroll">
				<table aria-label="Lines of {label}">
					<thead>
						<tr>
							<th>Date</th>
							<th>Description</th>
							<th class="number">Qty</th>
							<th class="number">Total</th>
							<th><span class="visually-hidden">Actions</span></th>
						</tr>
					</thead>
					<tbody>
						{#each folio.lines as line (line.id)}
							{@const counterpart = transferCounterpart(line, folio.id, folios)}
							<tr class:reversed={reversed.has(line.id)}>
								<td>{line.businessDate}</td>
								<td>
									{#if line.reverses}
										Reversal of {descriptions.get(line.reverses) ?? 'an earlier line'}
									{:else}
										{line.description}
									{/if}
									<span class="hint small">{categoryLabel(line.category, outlets)}</span>
									{#if line.reason}<span class="hint small">Reason: {line.reason}</span>{/if}
									{#if reversed.has(line.id)}<span class="tag">Reversed</span>{/if}
									{#if counterpart}
										<span class="hint small">
											{counterpart.direction === 'to' ? 'Moved to' : 'Moved from'}
											{labels.get(counterpart.folioId) ?? 'another folio'}
										</span>
									{:else if line.transferId && !line.reverses}
										<span class="hint small">Moved from another folio</span>
									{/if}
								</td>
								<td class="number">{line.quantity}</td>
								<td class="number">{money(line.total, line.currency)}</td>
								<td>
									<span class="actions">
										{#if line.taxes.length > 0}
											<button
												type="button"
												class="secondary"
												aria-expanded={expanded === line.id}
												aria-controls="taxes-{line.id}"
												onclick={() => (expanded = expanded === line.id ? null : line.id)}
												>Taxes</button
											>
										{/if}
										{#if actions.correct && canCorrect(line, reversed)}
											<button
												type="button"
												class="secondary"
												aria-label="Reverse {line.description}"
												onclick={() => openReverse(line.id)}>Reverse</button
											>
											<button
												type="button"
												class="secondary"
												aria-label="Transfer {line.description}"
												onclick={() => openTransfer(line.id)}>Transfer</button
											>
										{/if}
									</span>
								</td>
							</tr>
							{#if expanded === line.id}
								<tr class="detail" id="taxes-{line.id}">
									<td></td>
									<td colspan="4">
										<dl class="taxes">
											<dt>Net</dt>
											<dd>{money(line.net, line.currency)}</dd>
											{#each line.taxes as tax (tax.code)}
												<dt>
													{tax.name}
													{ratePercent(tax.rateBp)}{tax.presentation === 'ABSORBED'
														? ' (absorbed)'
														: ''}
												</dt>
												<dd>{money(tax.amount, line.currency)}</dd>
											{/each}
										</dl>
									</td>
								</tr>
							{/if}
							{#if reversing === line.id}
								<tr class="detail">
									<td></td>
									<td colspan="4">
										<form class="inline-form" onsubmit={(event) => reverse(event, line.id)}>
											<label>
												Why reverse {line.description}?
												<input required maxlength="500" bind:value={reverseReason} />
											</label>
											<button disabled={pending.has(`reverse:${line.id}`)}>Reverse line</button>
											<button type="button" class="secondary" onclick={() => (reversing = null)}
												>Keep it</button
											>
										</form>
									</td>
								</tr>
							{/if}
						{/each}
					</tbody>
				</table>
			</div>
		{/if}

		<h4>Payments</h4>
		{#if folio.payments.length === 0}
			<p class="hint">No payments.</p>
		{:else}
			<div class="scroll">
				<table aria-label="Payments of {label}">
					<thead>
						<tr>
							<th>Date</th>
							<th>Method</th>
							<th>Details</th>
							<th class="number">Amount</th>
						</tr>
					</thead>
					<tbody>
						{#each folio.payments as payment (payment.id)}
							<tr>
								<td>{payment.businessDate}</td>
								<td
									>{payment.refundOf
										? `Refund · ${methodLabel(payment.method)}`
										: methodLabel(payment.method)}</td
								>
								<td>
									{paymentDetails(payment, labels)}
									{#if payment.evidence}<span class="hint small">Evidence: {payment.evidence}</span
										>{/if}
								</td>
								<td class="number">{money(payment.amount, payment.currency)}</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{/if}
	{/if}

	{#if folio.invoices.length > 0}
		<h4>Invoices</h4>
		<ul class="invoices" aria-label="Invoices of {label}">
			{#each folio.invoices as invoice (invoice.id)}
				<li>
					<span class="what">
						<strong>{invoice.serial}</strong>
						<span class="hint">
							{invoice.kind === 'CREDIT_NOTE' ? 'Credit note' : 'Invoice'} · LKR
							{invoice.totalLkr.toLocaleString('en')} · {day(invoice.issuedAt)}
						</span>
					</span>
					<span class="actions">
						{#if actions.viewInvoice}
							{#if invoice.pdfReady}
								<button
									type="button"
									class="secondary"
									aria-label="View invoice {invoice.serial}"
									disabled={pending.has(`view:${invoice.id}`)}
									onclick={() => viewInvoice(invoice)}>View invoice</button
								>
							{:else}
								<span class="hint">Preparing PDF…</span>
							{/if}
						{/if}
						{#if actions.creditNote && creditable.has(invoice.id) && crediting !== invoice.id}
							<button
								type="button"
								class="secondary"
								aria-label="Credit note for {invoice.serial}"
								onclick={() => openCredit(invoice.id)}>Credit note</button
							>
						{/if}
					</span>
					{#if crediting === invoice.id}
						<form class="inline-form" onsubmit={(event) => creditNote(event, invoice)}>
							<label>
								Why credit {invoice.serial}?
								<input required maxlength="500" bind:value={creditReason} />
							</label>
							<button disabled={pending.has(`credit:${invoice.id}`)}>Issue credit note</button>
							<button type="button" class="secondary" onclick={() => (crediting = null)}
								>Cancel</button
							>
						</form>
					{/if}
				</li>
			{/each}
		</ul>
	{/if}
</section>

{#if actions.postCharge}
	<PostCharge
		{propertyId}
		folioId={folio.id}
		folioLabel={label}
		{baseCurrency}
		{currencies}
		{outlets}
		bind:open={charging}
	/>
{/if}
{#if actions.takePayment}
	<TakePayment
		{propertyId}
		{folio}
		folioLabel={label}
		{baseCurrency}
		{currencies}
		{hasAccount}
		bind:open={paying}
	/>
{/if}
{#if actions.refund}
	<RefundDialog {propertyId} {folios} {labels} folioId={folio.id} bind:open={refunding} />
{/if}
{#if actions.correct}
	<TransferDialog
		{propertyId}
		{folio}
		{folios}
		{labels}
		lineId={transferLine}
		bind:open={transferring}
	/>
{/if}

<!-- Clicking the backdrop closes it; from the keyboard, Escape does. -->
<dialog
	class="folio-dialog"
	bind:this={viewer}
	aria-label="Invoice {blocked?.serial ?? ''}"
	onclose={() => (blocked = null)}
	onclick={(event) => {
		if (event.target === viewer) viewer.close();
	}}
>
	{#if blocked}
		<p>The browser blocked the new tab.</p>
		<p>
			<!-- eslint-disable-next-line svelte/no-navigation-without-resolve -- a signed storage URL, not an app route -->
			<a href={blocked.url} target="_blank" rel="noopener noreferrer"
				>Open invoice {blocked.serial} (PDF)</a
			>
		</p>
		<p class="hint">The link works for 5 minutes.</p>
		<button type="button" class="secondary" onclick={() => viewer?.close()}>Close</button>
	{/if}
</dialog>

<style>
	.folio {
		padding: 0.5rem 0;
	}
	h4 {
		margin: var(--space) 0 0.25rem;
	}
	.balances {
		list-style: none;
		margin: 0;
		padding: 0;
		display: flex;
		flex-wrap: wrap;
		gap: 0.25rem 1.25rem;
		font-weight: 600;
	}
	.balances .credit {
		color: var(--stay-checked-in);
	}
	.balances .hint {
		font-weight: normal;
	}
	.toolbar {
		display: flex;
		flex-wrap: wrap;
		gap: 0.5rem;
		margin: var(--space) 0 0.25rem;
	}
	.scroll {
		overflow-x: auto;
	}
	table {
		width: 100%;
		margin: 0.25rem 0;
	}
	.number {
		text-align: right;
		white-space: nowrap;
	}
	td .actions {
		flex-wrap: wrap;
		justify-content: flex-end;
	}
	td .actions button {
		padding: 0.2rem 0.5rem;
	}
	.small {
		display: block;
		font-size: 0.85em;
	}
	tr.reversed td {
		color: var(--muted);
	}
	.tag {
		display: inline-block;
		margin-left: 0.35rem;
		padding: 0 0.35rem;
		border: 1px solid var(--border);
		border-radius: var(--radius);
		font-size: 0.8em;
		color: var(--muted);
	}
	.detail td {
		background: var(--surface);
	}
	.taxes {
		display: grid;
		grid-template-columns: max-content max-content;
		gap: 0.15rem 1.5rem;
		margin: 0;
	}
	.taxes dt {
		color: var(--muted);
	}
	.taxes dd {
		margin: 0;
		text-align: right;
	}
	.invoices {
		list-style: none;
		margin: 0;
		padding: 0;
	}
	.invoices li {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 0.5rem;
		padding: 0.35rem 0;
		border-bottom: 1px solid var(--border);
	}
	.what {
		flex: 1;
		min-width: 10rem;
		display: grid;
	}
</style>
