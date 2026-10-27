<!--
	The liability reports, from the closed days: SLTDA's Form A (TDL, by fiscal quarter), SSCL (by calendar quarter,
	month by month), VAT (by the property's return period, with IRD's Schedule 01 and 04 files) and the service
	charge (by month). Opening a period shows its report with Download PDF. A report the server refuses, such as
	VAT for a property that is not registered, shows the server's own message.
-->
<script lang="ts">
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { createQuery } from '@tanstack/svelte-query';
	import { errorMessage } from '$lib/api/problem';
	import DownloadPdf from '$lib/components/DownloadPdf.svelte';
	import { fetchProperties, propertiesKey } from '$lib/properties';
	import {
		downloadSchedule,
		fetchServiceChargeReport,
		fetchSsclReport,
		fetchTdlReport,
		fetchVatReport,
		formatLkr,
		formatRupees,
		monthName,
		recentFiscalQuarters,
		recentMonths,
		recentQuarters,
		recentVatPeriods,
		reportKey,
		type RenderReportRequest
	} from '$lib/reports';
	import { fetchTaxSettings, taxSettingsKey } from '$lib/settings';

	type Kind = 'tdl' | 'sscl' | 'vat' | 'service_charge';
	const KINDS: { kind: Kind; label: string }[] = [
		{ kind: 'tdl', label: 'TDL' },
		{ kind: 'sscl', label: 'SSCL' },
		{ kind: 'vat', label: 'VAT' },
		{ kind: 'service_charge', label: 'Service charge' }
	];

	const propertyId = $derived(page.params.property ?? '');
	const properties = createQuery(() => ({
		queryKey: propertiesKey,
		queryFn: ({ signal }) => fetchProperties(signal)
	}));
	const today = $derived(
		properties.data?.find((entry) => entry.id === propertyId)?.businessDate ?? ''
	);
	const settings = createQuery(() => ({
		queryKey: taxSettingsKey(propertyId),
		queryFn: ({ signal }) => fetchTaxSettings(propertyId, signal)
	}));

	/** A period to open: what the report is asked for, and how its list entry reads. */
	interface Period {
		id: string;
		label: string;
		request: RenderReportRequest;
	}

	let kind = $state<Kind>('tdl');
	let selected = $state('');
	const periods = $derived.by((): Period[] => {
		if (!today) return [];
		switch (kind) {
			case 'tdl':
				return recentFiscalQuarters(today, 8).map((entry) => ({
					id: `${entry.fyStartYear}-${entry.quarter}`,
					label: entry.label,
					request: {
						report: 'tdl',
						params: { fy_start_year: entry.fyStartYear, quarter: entry.quarter }
					}
				}));
			case 'sscl':
				return recentQuarters(today, 8).map((entry) => ({
					id: `${entry.year}-${entry.quarter}`,
					label: entry.label,
					request: { report: 'sscl', params: { year: entry.year, quarter: entry.quarter } }
				}));
			case 'vat':
				return recentVatPeriods(
					today,
					settings.data?.propertyTaxSettings.vatReturnPeriod === 'monthly'
						? 'monthly'
						: 'quarterly',
					8
				).map((entry) => ({
					id: entry.code,
					label: `${entry.label} · ${entry.code}`,
					request: { report: 'vat', params: { period: entry.code } }
				}));
			case 'service_charge':
				return recentMonths(today, 12).map((entry) => ({
					id: `${entry.year}-${entry.month}`,
					label: entry.label,
					request: { report: 'service_charge', params: { year: entry.year, month: entry.month } }
				}));
		}
	});
	const period = $derived(periods.find((entry) => entry.id === selected));
	const request = $derived(period?.request);

	function choose(next: Kind) {
		kind = next;
		selected = '';
	}

	const tdl = createQuery(() => ({
		queryKey: reportKey(propertyId, 'tdl', selected),
		queryFn: ({ signal }) => {
			if (request?.report !== 'tdl') throw new Error('not a TDL period');
			return fetchTdlReport(
				propertyId,
				request.params.fy_start_year,
				request.params.quarter,
				signal
			);
		},
		enabled: request?.report === 'tdl'
	}));
	const sscl = createQuery(() => ({
		queryKey: reportKey(propertyId, 'sscl', selected),
		queryFn: ({ signal }) => {
			if (request?.report !== 'sscl') throw new Error('not an SSCL period');
			return fetchSsclReport(propertyId, request.params.year, request.params.quarter, signal);
		},
		enabled: request?.report === 'sscl'
	}));
	const vat = createQuery(() => ({
		queryKey: reportKey(propertyId, 'vat', selected),
		queryFn: ({ signal }) => {
			if (request?.report !== 'vat') throw new Error('not a VAT period');
			return fetchVatReport(propertyId, request.params.period, signal);
		},
		enabled: request?.report === 'vat',
		retry: false
	}));
	const serviceCharge = createQuery(() => ({
		queryKey: reportKey(propertyId, 'service-charge', selected),
		queryFn: ({ signal }) => {
			if (request?.report !== 'service_charge') throw new Error('not a service charge period');
			return fetchServiceChargeReport(
				propertyId,
				request.params.year,
				request.params.month,
				signal
			);
		},
		enabled: request?.report === 'service_charge'
	}));

	let scheduleProblem = $state('');
	let downloading = $state(false);

	async function schedule(which: '01' | '04') {
		if (request?.report !== 'vat') return;
		scheduleProblem = '';
		downloading = true;
		try {
			await downloadSchedule(propertyId, which, request.params.period);
		} catch (err) {
			scheduleProblem = errorMessage(err);
		} finally {
			downloading = false;
		}
	}

	/** A basis-point rate as a percentage: 100 is `1%`, 50 is `0.5%`. */
	const percent = (bp: number) => `${bp / 100}%`;
</script>

<h1>Tax reports</h1>
<p>
	<a href={resolve('/(app)/p/[property]/reports', { property: propertyId })}>Reports</a>
</p>

<div role="tablist" aria-label="Tax" class="tabs">
	{#each KINDS as entry (entry.kind)}
		<button
			type="button"
			role="tab"
			aria-selected={kind === entry.kind}
			class:secondary={kind !== entry.kind}
			onclick={() => choose(entry.kind)}>{entry.label}</button
		>
	{/each}
</div>

<section aria-labelledby="periods">
	<h2 id="periods">Periods</h2>
	<ul class="periods">
		{#each periods as entry (entry.id)}
			<li>
				<button
					type="button"
					class:secondary={selected !== entry.id}
					aria-pressed={selected === entry.id}
					onclick={() => (selected = entry.id)}>{entry.label}</button
				>
			</li>
		{/each}
	</ul>
</section>

{#if period && request}
	<section aria-labelledby="report">
		<h2 id="report">{period.label}</h2>

		{#if request.report === 'tdl'}
			{#if tdl.error && !tdl.data}
				<p class="error" role="alert">{errorMessage(tdl.error)}</p>
			{:else if tdl.data}
				{@const form = tdl.data}
				{#if form.notClosed}
					<p role="status">Not closed yet: figures cover the closed days only</p>
				{/if}
				<p>
					{form.name}{form.registrationNo ? ` · TDL registration ${form.registrationNo}` : ''} · {form.periodLabel}
				</p>
				<table aria-label="Form A">
					<tbody>
						<tr><th scope="row">Room Sales</th><td>{formatLkr(form.room)}</td></tr>
						<tr><th scope="row">Food Sales</th><td>{formatLkr(form.food)}</td></tr>
						<tr><th scope="row">Bar Sales</th><td>{formatLkr(form.bar)}</td></tr>
						<tr><th scope="row">Other Sales</th><td>{formatLkr(form.other)}</td></tr>
						<tr><th scope="row">Total Turnover</th><td>{formatLkr(form.totalTurnover)}</td></tr>
						<tr>
							<th scope="row">Deduction: Service Charge</th>
							<td>{formatLkr(form.serviceCharge)}</td>
						</tr>
						<tr><th scope="row">Deduction: VAT</th><td>{formatLkr(form.vat)}</td></tr>
						<tr>
							<th scope="row">Total Turnover less deductions</th>
							<td>{formatLkr(form.base)}</td>
						</tr>
						<tr>
							<th scope="row">TDL at {percent(form.rateBp)}</th>
							<td>{formatLkr(form.tdl)}</td>
						</tr>
					</tbody>
				</table>
				{#if form.creditExceedsSales}
					<p class="hint">The credits exceed the sales: no TDL is due.</p>
				{/if}
				<DownloadPdf {propertyId} {request} />
			{/if}
		{:else if request.report === 'sscl'}
			{#if sscl.error && !sscl.data}
				<p class="error" role="alert">{errorMessage(sscl.error)}</p>
			{:else if sscl.data}
				{@const report = sscl.data}
				{#if report.months.some((month) => month.notClosed)}
					<p role="status">Not closed yet: figures cover the closed days only</p>
				{/if}
				<div class="scroll">
					<table aria-label="SSCL">
						<thead>
							<tr>
								<th>Month</th>
								<th>Turnover</th>
								<th>Exempt, folios settled this month</th>
								<th>Exempt, turnover of earlier months</th>
								<th>Bad debts</th>
								<th>Liable turnover</th>
								<th>SSCL</th>
								<th>Foreign cash</th>
								<th>Payment due</th>
							</tr>
						</thead>
						<tbody>
							{#each report.months as month (month.month)}
								<tr>
									<td>{monthName(month.month)}{month.notClosed ? ' (not closed yet)' : ''}</td>
									<td>{formatLkr(month.turnover)}</td>
									<td>{formatLkr(month.exemptSameMonth)}</td>
									<td>{formatLkr(month.exemptEarlierMonths)}</td>
									<td>{formatLkr(month.badDebts)}</td>
									<td>{formatLkr(month.liable)}</td>
									<td>{formatLkr(month.levy)}</td>
									<td>{formatLkr(month.foreignCash)}</td>
									<td>{month.due}</td>
								</tr>
							{/each}
						</tbody>
						<tfoot>
							<tr>
								<th scope="row">Quarter</th>
								<td>{formatLkr(report.quarterTotals.turnover)}</td>
								<td>{formatLkr(report.quarterTotals.exemptSameMonth)}</td>
								<td>{formatLkr(report.quarterTotals.exemptEarlierMonths)}</td>
								<td>{formatLkr(report.quarterTotals.badDebts)}</td>
								<td>{formatLkr(report.quarterTotals.liable)}</td>
								<td>{formatLkr(report.quarterTotals.levy)}</td>
								<td>{formatLkr(report.quarterTotals.foreignCash)}</td>
								<td>Return due {report.returnDue}</td>
							</tr>
						</tfoot>
					</table>
				</div>
				{#if report.exemptionNote}<p class="hint">{report.exemptionNote}</p>{/if}
				<p class="hint">{report.methodNote}</p>
				<DownloadPdf {propertyId} {request} />
			{/if}
		{:else if request.report === 'vat'}
			{#if vat.error && !vat.data}
				<p class="error" role="alert">{errorMessage(vat.error)}</p>
			{:else if vat.data}
				{@const report = vat.data}
				{#if report.notClosed}
					<p role="status">Not closed yet: figures cover the closed days only</p>
				{/if}
				<p class="hint">Whole rupees. The totals are already net of the period's credit notes.</p>
				<table aria-label="VAT return">
					<tbody>
						<tr
							><th scope="row">Value of supply</th><td>{formatRupees(report.valueOfSupply)}</td></tr
						>
						<tr><th scope="row">Output VAT</th><td>{formatRupees(report.outputVat)}</td></tr>
						<tr>
							<th scope="row">Credit notes issued</th>
							<td
								>{formatRupees(report.creditNotesValue)} (VAT {formatRupees(
									report.creditNotesVat
								)})</td
							>
						</tr>
						<tr><th scope="row">Net value (cage A)</th><td>{formatRupees(report.netValue)}</td></tr>
						<tr><th scope="row">Net VAT (cage 0)</th><td>{formatRupees(report.netVat)}</td></tr>
					</tbody>
				</table>
				{#if report.timingDifference.length > 0}
					<h3>Tax invoices issued this period for earlier supplies</h3>
					<ul>
						{#each report.timingDifference as row (row.rateBp)}
							<li>
								{percent(row.rateBp)}: {formatRupees(row.value)} (VAT {formatRupees(row.vat)})
							</li>
						{/each}
					</ul>
				{/if}
				<h3>Schedule 01 ({report.schedule01.length})</h3>
				<div class="scroll">
					<table aria-label="Schedule 01">
						<thead>
							<tr>
								<th>Serial No</th>
								<th>Invoice date</th>
								<th>Tax invoice no</th>
								<th>Purchaser's TIN</th>
								<th>Name of the purchaser</th>
								<th>Description</th>
								<th>Value of supply</th>
								<th>VAT amount</th>
							</tr>
						</thead>
						<tbody>
							{#each report.schedule01 as row (row.serialNo)}
								<tr>
									<td>{row.serialNo}</td>
									<td>{row.invoiceDate}</td>
									<td>{row.invoiceNo}</td>
									<td>{row.purchaserTin}</td>
									<td>{row.purchaserName}</td>
									<td>{row.description}</td>
									<td>{formatRupees(row.valueOfSupply)}</td>
									<td>{formatRupees(row.vatAmount)}</td>
								</tr>
							{:else}
								<tr><td colspan="8" class="hint">No tax invoices in the closed days.</td></tr>
							{/each}
						</tbody>
					</table>
				</div>
				<h3>Schedule 04 ({report.schedule04.length})</h3>
				<div class="scroll">
					<table aria-label="Schedule 04">
						<thead>
							<tr>
								<th>Serial No</th>
								<th>TIN</th>
								<th>Invoice date</th>
								<th>Invoice no</th>
								<th>Credit note date</th>
								<th>Credit note no</th>
								<th>Value</th>
								<th>VAT amount</th>
							</tr>
						</thead>
						<tbody>
							{#each report.schedule04 as row (row.serialNo)}
								<tr>
									<td>{row.serialNo}</td>
									<td>{row.tin}</td>
									<td>{row.invoiceDate}</td>
									<td>{row.invoiceNo}</td>
									<td>{row.noteDate}</td>
									<td>{row.noteNo}</td>
									<td>{formatRupees(row.value)}</td>
									<td>{formatRupees(row.vatAmount)}</td>
								</tr>
							{:else}
								<tr><td colspan="8" class="hint">No credit notes in the closed days.</td></tr>
							{/each}
						</tbody>
					</table>
				</div>
				<div class="actions">
					<DownloadPdf {propertyId} {request} />
					<button type="button" disabled={downloading} onclick={() => schedule('01')}
						>Download Schedule 01 (CSV)</button
					>
					<button type="button" disabled={downloading} onclick={() => schedule('04')}
						>Download Schedule 04 (CSV)</button
					>
				</div>
				{#if scheduleProblem}<p class="error" role="alert">{scheduleProblem}</p>{/if}
			{/if}
		{:else if request.report === 'service_charge'}
			{#if serviceCharge.error && !serviceCharge.data}
				<p class="error" role="alert">{errorMessage(serviceCharge.error)}</p>
			{:else if serviceCharge.data}
				{@const report = serviceCharge.data}
				{#if report.notClosed}
					<p role="status">Not closed yet: figures cover the closed days only</p>
				{/if}
				<table aria-label="Service charge">
					<thead>
						<tr>
							<th></th>
							<th>Collected (on the bill)</th>
							<th>Absorbed (in the price)</th>
						</tr>
					</thead>
					<tbody>
						<tr>
							<th scope="row">Rooms</th>
							<td>{formatLkr(report.rooms.collected)}</td>
							<td>{formatLkr(report.rooms.absorbed)}</td>
						</tr>
						{#each report.byOutlet as outlet (outlet.outlet)}
							<tr>
								<th scope="row">Outlet {outlet.outlet}</th>
								<td>{formatLkr(outlet.collected)}</td>
								<td>{formatLkr(outlet.absorbed)}</td>
							</tr>
						{/each}
						<tr>
							<th scope="row">Everything else</th>
							<td>{formatLkr(report.other.collected)}</td>
							<td>{formatLkr(report.other.absorbed)}</td>
						</tr>
					</tbody>
					<tfoot>
						<tr>
							<th scope="row">Total</th>
							<td>{formatLkr(report.total.collected)}</td>
							<td>{formatLkr(report.total.absorbed)}</td>
						</tr>
					</tfoot>
				</table>
				<DownloadPdf {propertyId} {request} />
			{/if}
		{/if}
	</section>
{/if}

<style>
	.tabs {
		display: flex;
		flex-wrap: wrap;
		gap: 0.25rem;
		margin-bottom: var(--space);
	}
	.periods {
		display: flex;
		flex-wrap: wrap;
		gap: 0.5rem;
		margin: 0;
		padding: 0;
		list-style: none;
	}
	.actions {
		display: flex;
		flex-wrap: wrap;
		gap: var(--space);
		margin: var(--space) 0;
	}
	.scroll {
		overflow-x: auto;
	}
</style>
