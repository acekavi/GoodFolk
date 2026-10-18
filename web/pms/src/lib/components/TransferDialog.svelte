<!--
	Moves a line to another folio of the reservation: the line, then the folio it goes to, then why. The line
	is reversed here and posted again there, both kept.
-->
<script lang="ts">
	import { useQueryClient } from '@tanstack/svelte-query';
	import { errorMessage } from '$lib/api/problem';
	import { formKeys, rest, unwrap } from '$lib/api/rest';
	import { allFoliosKey, canCorrect, money, reversedLines, type Folio } from '$lib/folio';

	interface Props {
		propertyId: string;
		/** The folio the line is on. */
		folio: Folio;
		/** Every folio of the reservation; the open ones but this are offered as targets. */
		folios: readonly Folio[];
		labels: ReadonlyMap<string, string>;
		/** The line chosen when the dialog opens. */
		lineId: string;
		open: boolean;
	}

	let { propertyId, folio, folios, labels, lineId, open = $bindable() }: Props = $props();

	const client = useQueryClient();
	const keys = formKeys();
	let dialog = $state<HTMLDialogElement>();
	let form = $state({ lineId: '', toFolio: '', reason: '' });
	let problem = $state('');
	let busy = $state(false);

	const lines = $derived.by(() => {
		const reversed = reversedLines(folio.lines);
		return folio.lines.filter((line) => canCorrect(line, reversed));
	});
	const targets = $derived(
		folios.filter((other) => other.id !== folio.id && other.status === 'OPEN')
	);

	$effect(() => {
		if (!dialog) return;
		if (open && !dialog.open) {
			form = { lineId: lineId || (lines[0]?.id ?? ''), toFolio: targets[0]?.id ?? '', reason: '' };
			problem = '';
			keys.reset();
			dialog.showModal();
		} else if (!open && dialog.open) {
			dialog.close();
		}
	});

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		busy = true;
		problem = '';
		const body = { to_folio: form.toFolio, reason: form.reason.trim() };
		try {
			unwrap(
				await rest.POST('/api/v1/properties/{property}/folios/{folio}/lines/{line}/transfer', {
					params: {
						path: { property: propertyId, folio: folio.id, line: form.lineId },
						header: { 'Idempotency-Key': keys.keyFor({ line: form.lineId, ...body }) }
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
	aria-labelledby="transfer-title"
	onclose={() => (open = false)}
>
	<form class="form" onsubmit={submit}>
		<h2 id="transfer-title">Transfer a line · {labels.get(folio.id)}</h2>
		{#if targets.length === 0}
			<p>This reservation has no other open folio to move a line to.</p>
			<div class="actions">
				<button type="button" class="secondary" onclick={() => (open = false)}>Close</button>
			</div>
		{:else}
			<label>
				Line
				<select required bind:value={form.lineId}>
					{#each lines as line (line.id)}
						<option value={line.id}>
							{line.businessDate} · {line.description} · {money(line.total, line.currency)}
						</option>
					{/each}
				</select>
			</label>
			<label>
				To folio
				<select required bind:value={form.toFolio}>
					{#each targets as target (target.id)}
						<option value={target.id}>{labels.get(target.id)}</option>
					{/each}
				</select>
			</label>
			<label>Reason <input required maxlength="500" bind:value={form.reason} /></label>
			{#if problem}<p class="error" role="alert">{problem}</p>{/if}
			<div class="actions">
				<button disabled={busy || !form.lineId}>Transfer line</button>
				<button type="button" class="secondary" onclick={() => (open = false)}>Cancel</button>
			</div>
		{/if}
	</form>
</dialog>
