<!--
	Adds a tax, levy or service charge, or changes one (all but its code). The rate is typed as a percentage and
	sent as exact basis points. The server checks the property's whole rule set; its refusal shows here and the
	dialog stays open. A rule already in force (from the business date or before) keeps what it charged: only its
	name and a later end can change, so the other fields are locked and the form says to end it and add a new rule.
	When someone else changed the rule first (412), the dialog closes and the page shows why.
-->
<script lang="ts">
	import { ApiError, errorMessage } from '$lib/api/problem';
	import { formKeys, ifMatch, rest, unwrap } from '$lib/api/rest';
	import {
		appliesToLabel,
		bpToPercent,
		parseCurrencies,
		percentToBp,
		readCondition,
		type RuleCondition,
		type TaxRule
	} from '$lib/settings';

	interface Props {
		propertyId: string;
		/** The rule to change, or `null` to add one. */
		rule: TaxRule | null;
		/** Every rule of the property, for the codes a base can name. */
		rules: readonly TaxRule[];
		outlets: readonly { code: string; name: string; active: boolean }[];
		businessDate: string;
		open: boolean;
		/** After a save, or with why the dialog closed after a version conflict, so the page refetches. */
		onchange: (message?: string) => void;
	}

	let {
		propertyId,
		rule,
		rules,
		outlets,
		businessDate,
		open = $bindable(),
		onchange
	}: Props = $props();

	const KINDS = [
		{ value: 'tax', label: 'Tax' },
		{ value: 'levy', label: 'Levy' },
		{ value: 'service_charge', label: 'Service charge' }
	] as const;
	const CONDITIONS = [
		{ value: 'always', label: 'Always' },
		{ value: 'vat_registered', label: 'When VAT-registered' },
		{ value: 'residency', label: 'By residency' },
		{ value: 'payment_currency', label: 'By payment currency' }
	] as const;

	type Kind = (typeof KINDS)[number]['value'];
	type ConditionType = RuleCondition['type'];

	const keys = formKeys();
	let dialog = $state<HTMLDialogElement>();
	let problem = $state('');
	let busy = $state(false);
	let form = $state(blank());
	/** In force on the business date or before it: the server lets only the name and a later end change. */
	const inForce = $derived(!!rule && !!businessDate && rule.effectiveFrom <= businessDate);

	function blank() {
		return {
			code: '',
			name: '',
			kind: 'tax' as Kind,
			rate: '',
			appliesTo: ['*'] as string[],
			base: ['net'] as string[],
			sequence: 10,
			presentation: 'on_bill' as 'on_bill' | 'absorbed',
			condition: 'always' as ConditionType,
			resident: true,
			currencies: '',
			from: '',
			to: ''
		};
	}

	function fromRule(rule: TaxRule) {
		const condition = readCondition(rule.condition);
		return {
			code: rule.code,
			name: rule.name,
			kind: rule.kind.toLowerCase() as Kind,
			rate: bpToPercent(rule.rateBp),
			appliesTo: [...rule.appliesTo],
			base: [...rule.base],
			sequence: rule.sequence,
			presentation: rule.presentation === 'ABSORBED' ? ('absorbed' as const) : ('on_bill' as const),
			condition: condition.type,
			resident: condition.type === 'residency' ? condition.resident : true,
			currencies: condition.type === 'payment_currency' ? condition.currencies.join(', ') : '',
			from: rule.effectiveFrom,
			to: rule.effectiveTo ?? ''
		};
	}

	/** Every category a rule can apply to, with whatever the rule already names. */
	const appliesOptions = $derived.by(() => {
		const values = [
			'*',
			'room',
			'meal_plan',
			'laundry',
			'misc',
			'fee',
			'pos:*',
			...outlets.map((outlet) => `pos:${outlet.code}`)
		];
		for (const value of form.appliesTo) if (!values.includes(value)) values.push(value);
		return values.map((value) => {
			const outlet = outlets.find((o) => `pos:${o.code}` === value);
			return { value, label: outlet ? `${outlet.name} (${value})` : appliesToLabel(value) };
		});
	});

	/** `net` and the codes of the property's other rules. */
	const baseOptions = $derived.by(() => {
		const codes = rules.map((r) => r.code).filter((code) => code !== form.code);
		const values = ['net', ...[...new Set(codes)].sort()];
		for (const value of form.base) if (!values.includes(value)) values.push(value);
		return values;
	});

	$effect(() => {
		if (!dialog) return;
		if (open && !dialog.open) {
			form = rule ? fromRule(rule) : { ...blank(), from: businessDate };
			problem = '';
			keys.reset();
			dialog.showModal();
		} else if (!open && dialog.open) {
			dialog.close();
		}
	});

	function toggle(list: string[], value: string, on: boolean): string[] {
		return on ? [...list, value] : list.filter((entry) => entry !== value);
	}

	function condition(): RuleCondition {
		switch (form.condition) {
			case 'residency':
				return { type: 'residency', resident: form.resident };
			case 'payment_currency':
				return { type: 'payment_currency', currencies: parseCurrencies(form.currencies) };
			default:
				return { type: form.condition };
		}
	}

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		const rateBp = percentToBp(form.rate);
		if (rateBp === null) {
			problem =
				'Enter the rate as a percentage from 0 to 100 with at most 2 decimals, such as 2.5.';
			return;
		}
		if (form.appliesTo.length === 0 || form.base.length === 0) {
			problem = 'Choose at least one category it applies to and one part of its base.';
			return;
		}
		const fields = {
			name: form.name.trim(),
			kind: form.kind,
			rate_bp: rateBp,
			applies_to: form.appliesTo,
			base: form.base,
			sequence: form.sequence,
			presentation: form.presentation,
			condition: condition(),
			effective_from: form.from,
			effective_to: form.to || null
		};
		busy = true;
		problem = '';
		try {
			if (rule) {
				unwrap(
					await rest.PATCH('/api/v1/properties/{property}/tax-rules/{rule}', {
						params: {
							path: { property: propertyId, rule: rule.id },
							header: ifMatch(rule.version)
						},
						body: fields
					})
				);
			} else {
				const body = { code: form.code.trim().toUpperCase(), ...fields };
				try {
					unwrap(
						await rest.POST('/api/v1/properties/{property}/tax-rules', {
							params: {
								path: { property: propertyId },
								header: { 'Idempotency-Key': keys.keyFor(body) }
							},
							body
						})
					);
				} catch (err) {
					keys.failed(err);
					throw err;
				}
				keys.reset();
			}
			open = false;
			onchange();
		} catch (err) {
			if (err instanceof ApiError && err.status === 412) {
				open = false;
				onchange(
					'Someone else changed this rule. The table now shows the latest version; make your change again.'
				);
			} else {
				problem = errorMessage(err);
			}
		} finally {
			busy = false;
		}
	}
</script>

<dialog
	class="folio-dialog"
	bind:this={dialog}
	aria-labelledby="rule-title"
	onclose={() => (open = false)}
>
	<form class="form" onsubmit={submit}>
		<h2 id="rule-title">{rule ? `Change ${rule.code}` : 'Add a tax rule'}</h2>
		{#if !rule}
			<label>
				Code
				<input
					required
					maxlength="16"
					pattern="[A-Za-z][A-Za-z0-9_]*"
					placeholder="VAT"
					bind:value={form.code}
				/>
			</label>
		{/if}
		{#if inForce}
			<p class="hint" id="in-force">
				In force since {rule?.effectiveFrom}: only its name and a later end can change. To change
				its rate or anything else, end it and add a new rule.
			</p>
		{/if}
		<label>Name <input required maxlength="100" bind:value={form.name} /></label>
		<fieldset class="locked" disabled={inForce} aria-describedby={inForce ? 'in-force' : undefined}>
			<label>
				Kind
				<select bind:value={form.kind}>
					{#each KINDS as kind (kind.value)}
						<option value={kind.value}>{kind.label}</option>
					{/each}
				</select>
			</label>
			<label>
				Rate (%)
				<input
					required
					inputmode="decimal"
					autocomplete="off"
					placeholder="2.5"
					bind:value={form.rate}
				/>
			</label>
			<fieldset>
				<legend>Applies to</legend>
				{#each appliesOptions as option (option.value)}
					<label class="check">
						<input
							type="checkbox"
							checked={form.appliesTo.includes(option.value)}
							onchange={(event) =>
								(form.appliesTo = toggle(
									form.appliesTo,
									option.value,
									event.currentTarget.checked
								))}
						/>
						{option.label}
					</label>
				{/each}
			</fieldset>
			<fieldset>
				<legend>Base</legend>
				{#each baseOptions as value (value)}
					<label class="check">
						<input
							type="checkbox"
							checked={form.base.includes(value)}
							onchange={(event) =>
								(form.base = toggle(form.base, value, event.currentTarget.checked))}
						/>
						{value === 'net' ? 'Net' : value}
					</label>
				{/each}
				<p class="hint">A base can name only rules with a lower sequence.</p>
			</fieldset>
			<label>
				Sequence
				<input type="number" required step="1" bind:value={form.sequence} />
			</label>
			<label>
				Presentation
				<select bind:value={form.presentation}>
					<option value="on_bill">On the bill</option>
					<option value="absorbed">Absorbed by the property</option>
				</select>
			</label>
			<label>
				Condition
				<select bind:value={form.condition}>
					{#each CONDITIONS as entry (entry.value)}
						<option value={entry.value}>{entry.label}</option>
					{/each}
				</select>
			</label>
			{#if form.condition === 'residency'}
				<label>
					Applies to
					<select bind:value={form.resident}>
						<option value={true}>Residents</option>
						<option value={false}>Non-residents</option>
					</select>
				</label>
			{:else if form.condition === 'payment_currency'}
				<label>
					Currencies
					<input required placeholder="USD, EUR" bind:value={form.currencies} />
				</label>
			{/if}
			<label>Effective from <input type="date" required bind:value={form.from} /></label>
		</fieldset>
		<label>
			Effective to
			<input type="date" bind:value={form.to} />
			<span class="hint">The first day it no longer applies; leave blank for no end.</span>
		</label>
		{#if problem}<p class="error" role="alert">{problem}</p>{/if}
		<div class="actions">
			<button disabled={busy}>{rule ? 'Save rule' : 'Add rule'}</button>
			<button type="button" class="secondary" onclick={() => (open = false)}>Cancel</button>
		</div>
	</form>
</dialog>

<style>
	.locked {
		display: contents;
	}
	fieldset:not(.locked) {
		border: 1px solid var(--border);
		border-radius: var(--radius);
		display: flex;
		flex-wrap: wrap;
		gap: 0.25rem 0.75rem;
	}
	fieldset .hint {
		margin: 0;
		flex-basis: 100%;
		font-size: 0.85em;
	}
	.check {
		display: flex;
		gap: 0.4rem;
		align-items: center;
	}
</style>
