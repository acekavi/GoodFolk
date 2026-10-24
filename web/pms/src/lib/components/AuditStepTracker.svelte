<!--
	A close's five steps as pending, running, done or failed, read from the run (`auditRuns`, refetched on the
	server's `audit:<property>` events). A failed run shows its error, the stays that failed (room and reason) and
	Retry; a completed one names the new business date. Announced politely as it changes.
-->
<script lang="ts">
	import { addDays } from '$lib/inventory';
	import { failedStays, stepStates, type AuditRun } from '$lib/night-audit';

	interface Props {
		run: AuditRun;
		/** Retries the failed run; offered only when given. */
		onretry?: () => void;
		retrying?: boolean;
	}

	let { run, onretry, retrying = false }: Props = $props();

	const steps = $derived(stepStates(run));
	const stays = $derived(failedStays(run));
	const STATE_LABELS = {
		pending: 'Pending',
		running: 'Running',
		done: 'Done',
		failed: 'Failed'
	} as const;
</script>

<section class="tracker" aria-labelledby="tracker-{run.id}" aria-live="polite">
	<h2 id="tracker-{run.id}">Closing {run.businessDate}</h2>
	<ol>
		{#each steps as entry (entry.step)}
			<li class={entry.state}>
				<span>{entry.label}</span>
				<span class="state">{STATE_LABELS[entry.state]}</span>
			</li>
		{/each}
	</ol>
	{#if run.status === 'COMPLETED'}
		<p>Business date is now {addDays(run.businessDate, 1)}</p>
	{:else if run.status === 'FAILED'}
		{#if run.error}<p class="error">{run.error}</p>{/if}
		{#if stays.length > 0}
			<table>
				<thead><tr><th>Room</th><th>Reason</th></tr></thead>
				<tbody>
					{#each stays as stay, index (index)}
						<tr><td>{stay.room ?? 'Unassigned'}</td><td>{stay.error}</td></tr>
					{/each}
				</tbody>
			</table>
		{/if}
		{#if onretry}
			<button disabled={retrying} onclick={onretry}>Retry</button>
		{/if}
	{/if}
</section>

<style>
	.tracker {
		margin-bottom: var(--space);
		padding: var(--space);
		border: 1px solid var(--border);
		border-radius: var(--radius);
		background: var(--surface);
	}
	ol {
		display: grid;
		gap: 0.25rem;
		margin: 0 0 var(--space);
		padding-left: 1.5rem;
	}
	li {
		display: flex;
		gap: var(--space);
		justify-content: space-between;
		max-width: 28rem;
	}
	.state {
		color: var(--muted);
	}
	li.done .state {
		color: var(--stay-checked-in);
	}
	li.running {
		font-weight: 600;
	}
	li.running .state {
		color: var(--accent);
	}
	li.failed,
	li.failed .state {
		color: var(--danger);
	}
</style>
