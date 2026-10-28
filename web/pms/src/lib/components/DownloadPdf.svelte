<!--
	A report's Download PDF button: opens the PDF the close stored for the period when it is ready, or else renders
	the report and waits for it, and opens its signed link in a new tab, as an invoice's PDF opens; a browser that
	blocks the tab is shown the link instead.
-->
<script lang="ts">
	import { onDestroy } from 'svelte';
	import { errorMessage } from '$lib/api/problem';
	import { reportPdfLink, type RenderReportRequest } from '$lib/reports';

	let {
		propertyId,
		request,
		label = 'Download PDF',
		disabled = false
	}: {
		propertyId: string;
		request: RenderReportRequest;
		label?: string;
		disabled?: boolean;
	} = $props();

	let busy = $state(false);
	const polling = new AbortController();
	onDestroy(() => polling.abort());
	let problem = $state('');
	/** A link the browser would not open in a tab, kept only while its dialog is open. */
	let blocked = $state<string | null>(null);
	let viewer = $state<HTMLDialogElement>();

	async function download() {
		problem = '';
		busy = true;
		try {
			const url = await reportPdfLink(propertyId, request, polling.signal);
			const tab = window.open(url, '_blank');
			if (tab) {
				// The PDF must not reach back into the app.
				tab.opener = null;
				return;
			}
			blocked = url;
			viewer?.showModal();
		} catch (err) {
			if (!polling.signal.aborted) problem = errorMessage(err);
		} finally {
			busy = false;
		}
	}
</script>

<button type="button" disabled={disabled || busy} onclick={download}>{label}</button>
{#if busy}<span class="hint" role="status">Preparing the PDF…</span>{/if}
{#if problem}<p class="error" role="alert">{problem}</p>{/if}

<dialog
	bind:this={viewer}
	aria-label="Report PDF"
	onclose={() => (blocked = null)}
	onclick={(event) => {
		if (event.target === viewer) viewer.close();
	}}
>
	{#if blocked}
		<p>The browser blocked the new tab.</p>
		<p>
			<!-- eslint-disable-next-line svelte/no-navigation-without-resolve -- a signed storage URL, not an app route -->
			<a href={blocked} target="_blank" rel="noopener noreferrer">Open the report (PDF)</a>
		</p>
		<p class="hint">The link works for 5 minutes.</p>
		<button type="button" class="secondary" onclick={() => viewer?.close()}>Close</button>
	{/if}
</dialog>
