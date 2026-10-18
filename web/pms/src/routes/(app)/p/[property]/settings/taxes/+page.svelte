<!--
	The property's tax settings and tax rules. Everyone with `FolioView` reads them; only `TaxSettingsManage` (the
	owner) changes them. Rules are never deleted: one is ended on the business date, and applying the Sri Lanka
	preset ends the current rules tomorrow and starts the preset's then. Lines already posted keep their taxes.
-->
<script lang="ts">
	import { page } from '$app/state';
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { ApiError, errorMessage } from '$lib/api/problem';
	import { formKeys, ifMatch, rest, unwrap } from '$lib/api/rest';
	import TaxRuleDialog from '$lib/components/TaxRuleDialog.svelte';
	import { folioContextKey } from '$lib/folio';
	import { addDays } from '$lib/inventory';
	import { Pending } from '$lib/pending.svelte';
	import { fetchProperties, propertiesKey } from '$lib/properties';
	import { can, fetchMe } from '$lib/session';
	import {
		appliesToLabel,
		bpToPercent,
		conditionLabel,
		fetchTaxSettings,
		kindLabel,
		parseCurrencies,
		rulePeriods,
		taxSettingsKey,
		type TaxRule,
		type TaxSettings
	} from '$lib/settings';

	const propertyId = $derived(page.params.property ?? '');
	const client = useQueryClient();
	const me = createQuery(() => ({ queryKey: ['me'], queryFn: fetchMe }));
	const manage = $derived(!!me.data && can(me.data, 'taxSettingsManage', propertyId));
	const properties = createQuery(() => ({
		queryKey: propertiesKey,
		queryFn: ({ signal }) => fetchProperties(signal)
	}));
	const businessDate = $derived(
		properties.data?.find((property) => property.id === propertyId)?.businessDate ?? ''
	);
	const settings = createQuery(() => ({
		queryKey: taxSettingsKey(propertyId),
		queryFn: ({ signal }) => fetchTaxSettings(propertyId, signal)
	}));
	const periods = $derived(rulePeriods(settings.data?.taxRules ?? [], businessDate));

	function draftOf(settings: TaxSettings) {
		return {
			vatRegistered: settings.vatRegistered,
			tin: settings.tin ?? '',
			tdlSmallTurnover: settings.tdlSmallTurnover,
			legalName: settings.legalName ?? '',
			address: settings.address ?? '',
			currencies: settings.acceptedCurrencies.join(', ')
		};
	}

	let draft = $state<ReturnType<typeof draftOf> | null>(null);
	let draftVersion = -1;
	// The form shows the stored settings whenever a new version of them arrives.
	$effect(() => {
		const current = settings.data?.propertyTaxSettings;
		if (current && current.version !== draftVersion) {
			draftVersion = current.version;
			draft = draftOf(current);
		}
	});

	const pending = new Pending();
	const presetKeys = formKeys();
	let settingsError = $state('');
	let settingsSaved = $state(false);
	let rulesError = $state('');
	let rulesNote = $state('');
	let editingRule = $state<TaxRule | null>(null);
	let ruleOpen = $state(false);

	async function refresh() {
		await Promise.all([
			client.invalidateQueries({ queryKey: taxSettingsKey(propertyId) }),
			client.invalidateQueries({ queryKey: folioContextKey(propertyId) })
		]);
	}

	async function saveSettings(event: SubmitEvent) {
		event.preventDefault();
		const current = settings.data?.propertyTaxSettings;
		if (!draft || !current) return;
		settingsError = '';
		settingsSaved = false;
		const body = {
			vat_registered: draft.vatRegistered,
			tin: draft.tin.trim() || null,
			tdl_small_turnover: draft.tdlSmallTurnover,
			legal_name: draft.legalName.trim() || null,
			address: draft.address.trim() || null,
			accepted_currencies: parseCurrencies(draft.currencies)
		};
		try {
			await pending.run('settings', async () =>
				unwrap(
					await rest.PATCH('/api/v1/properties/{property}/tax-settings', {
						params: { path: { property: propertyId }, header: ifMatch(current.version) },
						body
					})
				)
			);
			settingsSaved = true;
		} catch (err) {
			if (err instanceof ApiError && err.status === 412) {
				// Show the latest settings rather than let a stale save overwrite someone else's change.
				draftVersion = -1;
				settingsError =
					'Someone else changed these settings. The form now shows the latest version; make your change again.';
			} else {
				settingsError = errorMessage(err);
			}
		} finally {
			// The property's version is shared with its other settings.
			void client.invalidateQueries({ queryKey: propertiesKey });
			await refresh();
		}
	}

	function addRule() {
		rulesError = '';
		editingRule = null;
		ruleOpen = true;
	}

	function editRule(rule: TaxRule) {
		rulesError = '';
		editingRule = rule;
		ruleOpen = true;
	}

	/** Offered for a rule in force on the business date that started before it; the server refuses others. */
	function endable(rule: TaxRule): boolean {
		return (
			rule.effectiveFrom < businessDate &&
			(rule.effectiveTo == null || rule.effectiveTo > businessDate)
		);
	}

	async function endRule(rule: TaxRule) {
		if (
			!confirm(
				`End ${rule.code} (${rule.name})? It will not apply to charges dated ${businessDate} or later. Charges already posted keep their taxes.`
			)
		)
			return;
		rulesError = '';
		rulesNote = '';
		try {
			await pending.run(rule.id, async () =>
				unwrap(
					await rest.DELETE('/api/v1/properties/{property}/tax-rules/{rule}', {
						params: { path: { property: propertyId, rule: rule.id }, header: ifMatch(rule.version) }
					})
				)
			);
			rulesNote = `${rule.code} ends on ${businessDate}.`;
		} catch (err) {
			rulesError =
				err instanceof ApiError && err.status === 412
					? 'Someone else changed this rule. The table now shows the latest version.'
					: errorMessage(err);
		} finally {
			await refresh();
		}
	}

	async function applyPreset() {
		const tomorrow = businessDate ? addDays(businessDate, 1) : 'tomorrow';
		if (
			!confirm(
				`Apply the Sri Lanka preset? It takes effect tomorrow (${tomorrow}): the rules in force then end, and the preset's rules start. Charges already posted keep their taxes.`
			)
		)
			return;
		rulesError = '';
		rulesNote = '';
		try {
			const result = await pending.run('preset', async () => {
				try {
					return unwrap(
						await rest.POST('/api/v1/properties/{property}/tax-rules/apply-preset', {
							params: {
								path: { property: propertyId },
								header: { 'Idempotency-Key': presetKeys.keyFor({ businessDate }) }
							}
						})
					);
				} catch (err) {
					presetKeys.failed(err);
					throw err;
				}
			});
			presetKeys.reset();
			rulesNote = `The Sri Lanka preset takes effect on ${result.effective_from}: ${result.ended} ${result.ended === 1 ? 'rule ends' : 'rules end'} then.`;
		} catch (err) {
			rulesError = errorMessage(err);
		} finally {
			await refresh();
		}
	}
</script>

<h1>Taxes</h1>
{#if !manage}
	<p class="hint">Only the owner can change the property's taxes.</p>
{/if}

{#if settings.error && !settings.data}
	<p class="error" role="alert">{errorMessage(settings.error)}</p>
	<button onclick={() => settings.refetch()}>Retry</button>
{:else if settings.data && draft}
	<section aria-labelledby="settings-title">
		<h2 id="settings-title">Tax settings</h2>
		<form class="form settings" aria-label="Tax settings" onsubmit={saveSettings}>
			<fieldset disabled={!manage}>
				<label class="check">
					<input type="checkbox" bind:checked={draft.vatRegistered} /> VAT-registered
				</label>
				<label>
					TIN
					<input maxlength="20" pattern={'[0-9A-Z-]{5,20}'} bind:value={draft.tin} />
					<span class="hint">Printed on invoices; a tax invoice needs it.</span>
				</label>
				<label class="check">
					<input type="checkbox" bind:checked={draft.tdlSmallTurnover} /> Small-turnover TDL band (0.5%)
				</label>
				<p class="hint note">
					Takes effect only when the Sri Lanka preset is applied again; the current rules keep their
					rate.
				</p>
				<label>
					Legal name
					<input maxlength="200" placeholder="The property's name" bind:value={draft.legalName} />
				</label>
				<label>Address <input maxlength="500" bind:value={draft.address} /></label>
				<label>
					Accepted currencies
					<input required bind:value={draft.currencies} />
					<span class="hint"
						>ISO codes, such as LKR, USD; must include {settings.data.propertyTaxSettings
							.baseCurrency}.</span
					>
				</label>
				{#if manage}
					<div class="actions">
						<button disabled={pending.has('settings')}>Save settings</button>
					</div>
				{/if}
			</fieldset>
			{#if settingsError}<p class="error" role="alert">{settingsError}</p>{/if}
			{#if settingsSaved}<p class="hint" role="status">Saved.</p>{/if}
		</form>
	</section>

	<section aria-labelledby="rules-title">
		<h2 id="rules-title">Tax rules</h2>
		{#if manage}
			<div class="toolbar">
				<button type="button" onclick={addRule}>Add rule</button>
				<button
					type="button"
					class="secondary"
					disabled={pending.has('preset') || !businessDate}
					onclick={applyPreset}>Apply Sri Lanka preset</button
				>
			</div>
		{/if}
		{#if rulesError}<p class="error" role="alert">{rulesError}</p>{/if}
		{#if rulesNote}<p class="hint" role="status">{rulesNote}</p>{/if}
		{#if periods.length === 0}
			<p class="hint">No tax rules: charges are posted without taxes.</p>
		{/if}
		{#each periods as period (`${period.from}/${period.to}`)}
			<h3>
				{period.state === 'current'
					? 'In force'
					: period.state === 'upcoming'
						? 'Upcoming'
						: 'Ended'} · from {period.from}{period.to ? ` to ${period.to}` : ''}
			</h3>
			<div class="scroll">
				<table aria-label="Rules from {period.from}">
					<thead>
						<tr>
							<th>Code</th>
							<th>Name</th>
							<th>Kind</th>
							<th class="number">Rate</th>
							<th>Applies to</th>
							<th>Base</th>
							<th class="number">Sequence</th>
							<th>Presentation</th>
							<th>Condition</th>
							<th>From</th>
							<th>To</th>
							{#if manage}<th><span class="visually-hidden">Actions</span></th>{/if}
						</tr>
					</thead>
					<tbody>
						{#each period.rules as rule (rule.id)}
							<tr class:inactive={period.state === 'ended'}>
								<td>{rule.code}</td>
								<td>{rule.name}</td>
								<td>{kindLabel(rule.kind)}</td>
								<td class="number">{bpToPercent(rule.rateBp)}%</td>
								<td>{rule.appliesTo.map(appliesToLabel).join(', ')}</td>
								<td>{rule.base.map((part) => (part === 'net' ? 'Net' : part)).join(' + ')}</td>
								<td class="number">{rule.sequence}</td>
								<td>{rule.presentation === 'ABSORBED' ? 'Absorbed' : 'On the bill'}</td>
								<td>{conditionLabel(rule.condition)}</td>
								<td class="date">{rule.effectiveFrom}</td>
								<td class="date">{rule.effectiveTo ?? '—'}</td>
								{#if manage}
									<td class="actions">
										<button
											type="button"
											class="secondary"
											aria-label="Change {rule.code} from {rule.effectiveFrom}"
											disabled={pending.has(rule.id)}
											onclick={() => editRule(rule)}>Change</button
										>
										{#if endable(rule)}
											<button
												type="button"
												class="secondary"
												aria-label="End {rule.code}"
												disabled={pending.has(rule.id)}
												onclick={() => endRule(rule)}>End</button
											>
										{/if}
									</td>
								{/if}
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{/each}
	</section>

	{#if manage}
		<TaxRuleDialog
			{propertyId}
			rule={editingRule}
			rules={settings.data.taxRules}
			outlets={settings.data.outlets}
			{businessDate}
			bind:open={ruleOpen}
			onchange={() => void refresh()}
		/>
	{/if}
{:else}
	<p>Loading…</p>
{/if}

<style>
	.settings fieldset {
		display: grid;
		gap: var(--space);
		border: none;
		padding: 0;
		margin: 0;
	}
	.check {
		display: flex;
		gap: 0.4rem;
		align-items: center;
	}
	.note {
		margin: -0.5rem 0 0;
		font-size: 0.9em;
	}
	.toolbar {
		display: flex;
		flex-wrap: wrap;
		gap: 0.5rem;
		margin-bottom: var(--space);
	}
	h3 {
		margin: var(--space) 0 0;
		font-size: 1rem;
	}
	.scroll {
		overflow-x: auto;
	}
	.number {
		text-align: right;
		white-space: nowrap;
	}
	.date {
		white-space: nowrap;
	}
</style>
