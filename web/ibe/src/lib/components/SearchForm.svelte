<script lang="ts">
	import { goto } from '$app/navigation';
	import type { SiteRoomType } from '$lib/api';
	import { addDays, addMonths, guestsText } from '$lib/format';
	import {
		canAddRoom,
		MAX_NIGHTS,
		MONTHS_AHEAD,
		pickerSteps,
		RESIDENCY_QUESTION,
		roomsParam,
		stayOf,
		stayProblem,
		stayQuery,
		type SearchForm
	} from '$lib/search';

	/**
	 * The search: the residency question first, then the dates and each room's guests. A plain `GET` form to the
	 * results, so it works before the page's script has loaded; with it, nothing is sent until the stay is valid.
	 */
	let {
		roomTypes,
		businessDate,
		base,
		initial,
		submitLabel = 'Show rooms and prices'
	}: {
		roomTypes: readonly SiteRoomType[];
		businessDate: string;
		base: string;
		initial: SearchForm;
		submitLabel?: string;
	} = $props();

	// The form starts from the address once; the guest edits it from there.
	// svelte-ignore state_referenced_locally
	let form: SearchForm = $state(structuredClone(initial));
	let problem: string | null = $state(null);

	const lastCheckIn = $derived(addMonths(businessDate, MONTHS_AHEAD));
	const firstCheckOut = $derived(addDays(form.checkIn || businessDate, 1));
	const lastCheckOut = $derived(addDays(form.checkIn || businessDate, MAX_NIGHTS));

	function setCheckIn(value: string) {
		form.checkIn = value;
		if (!form.checkOut || form.checkOut <= value) form.checkOut = addDays(value, 1);
	}

	function submit(event: SubmitEvent) {
		event.preventDefault();
		problem = stayProblem(form, businessDate);
		const stay = stayOf(form, businessDate);
		if (problem || !stay) return;
		goto(`${base}/rooms?${stayQuery(stay)}`);
	}
</script>

<form class="search" method="GET" action="{base}/rooms" onsubmit={submit} novalidate>
	<fieldset class="residency">
		<legend>{RESIDENCY_QUESTION}</legend>
		<p class="muted hint">Residents and visitors have different rates.</p>
		<div class="choices">
			<label class="choice">
				<input
					type="radio"
					name="residency"
					value="resident"
					bind:group={form.residency}
					required
				/>
				Yes, I live in Sri Lanka
			</label>
			<label class="choice">
				<input
					type="radio"
					name="residency"
					value="non_resident"
					bind:group={form.residency}
					required
				/>
				No, I am visiting
			</label>
		</div>
	</fieldset>

	<div class="dates">
		<label>
			Check-in
			<input
				type="date"
				name="check_in"
				min={businessDate}
				max={lastCheckIn}
				value={form.checkIn}
				oninput={(event) => setCheckIn(event.currentTarget.value)}
				required
			/>
		</label>
		<label>
			Check-out
			<input
				type="date"
				name="check_out"
				min={firstCheckOut}
				max={lastCheckOut}
				bind:value={form.checkOut}
				required
			/>
		</label>
	</div>

	<fieldset class="rooms">
		<legend>Rooms and guests</legend>
		<input type="hidden" name="rooms" value={roomsParam(form.rooms)} />
		{#each form.rooms as room, index (index)}
			{@const steps = pickerSteps(roomTypes, room)}
			<div class="room">
				<p class="room-name">
					Room {index + 1}
					<span class="muted">{guestsText(room.adults, room.children)}</span>
				</p>
				<div class="stepper" role="group" aria-label="Adults in room {index + 1}">
					<span>Adults</span>
					<button
						type="button"
						class="step"
						aria-label="One adult fewer in room {index + 1}"
						disabled={!steps.removeAdult}
						onclick={() => room.adults--}>−</button
					>
					<output aria-live="polite">{room.adults}</output>
					<button
						type="button"
						class="step"
						aria-label="One adult more in room {index + 1}"
						disabled={!steps.addAdult}
						onclick={() => room.adults++}>+</button
					>
				</div>
				<div class="stepper" role="group" aria-label="Children in room {index + 1}">
					<span>Children</span>
					<button
						type="button"
						class="step"
						aria-label="One child fewer in room {index + 1}"
						disabled={!steps.removeChild}
						onclick={() => room.children--}>−</button
					>
					<output aria-live="polite">{room.children}</output>
					<button
						type="button"
						class="step"
						aria-label="One child more in room {index + 1}"
						disabled={!steps.addChild}
						onclick={() => room.children++}>+</button
					>
				</div>
				{#if form.rooms.length > 1}
					<button
						type="button"
						class="link"
						onclick={() => form.rooms.splice(index, 1)}
						aria-label="Remove room {index + 1}">Remove</button
					>
				{/if}
			</div>
		{/each}
		{#if canAddRoom(form.rooms)}
			<button
				type="button"
				class="button quiet add"
				onclick={() => form.rooms.push({ adults: 1, children: 0 })}>Add a room</button
			>
		{/if}
	</fieldset>

	{#if problem}
		<p class="notice error" role="alert">{problem}</p>
	{/if}

	<button class="button submit" type="submit">{submitLabel}</button>
</form>

<style>
	.search {
		display: grid;
		gap: 1.5rem;
		padding: 1.5rem;
		background: var(--surface);
		border: 1px solid var(--line);
		border-radius: var(--radius);
	}

	fieldset {
		border: 0;
		margin: 0;
		padding: 0;
		min-width: 0;
	}

	legend {
		font-weight: 600;
		padding: 0;
	}

	.residency legend {
		font-family: var(--serif);
		font-size: 1.3rem;
	}

	.hint {
		margin-top: 0.2rem;
		font-size: 0.95rem;
	}

	.choices {
		display: flex;
		flex-wrap: wrap;
		gap: 0.75rem;
		margin-top: 0.75rem;
	}

	.choice {
		display: flex;
		align-items: center;
		gap: 0.6rem;
		min-height: 2.75rem;
		padding: 0.5rem 1rem;
		border: 1px solid #8a948f;
		border-radius: var(--radius);
		font-weight: 500;
		cursor: pointer;
	}

	.choice:has(input:checked) {
		border-color: var(--accent);
		box-shadow: inset 0 0 0 1px var(--accent);
	}

	.choice input {
		accent-color: var(--accent);
		width: 1.15rem;
		height: 1.15rem;
		margin: 0;
	}

	.dates {
		display: grid;
		grid-template-columns: 1fr 1fr;
		gap: 1rem;
	}

	.rooms {
		display: grid;
		gap: 0.75rem;
	}

	.room {
		display: flex;
		flex-wrap: wrap;
		gap: 0.5rem 2rem;
		align-items: center;
		padding-block: 0.75rem;
		border-bottom: 1px solid var(--line);
	}

	.room-name {
		flex-basis: 100%;
		font-weight: 600;
		display: flex;
		gap: 0.75rem;
		align-items: baseline;
	}

	.room-name .muted {
		font-weight: 400;
	}

	.stepper {
		display: flex;
		align-items: center;
		gap: 0.5rem;
	}

	.stepper span {
		min-width: 4.5rem;
	}

	.step {
		width: 2.75rem;
		height: 2.75rem;
		border: 1px solid #8a948f;
		border-radius: 50%;
		background: var(--surface);
		font-size: 1.25rem;
		line-height: 1;
		cursor: pointer;
	}

	.step:disabled {
		opacity: 0.4;
		cursor: not-allowed;
	}

	output {
		min-width: 1.5rem;
		text-align: center;
		font-variant-numeric: tabular-nums;
	}

	.link {
		border: 0;
		background: none;
		padding: 0.5rem 0;
		min-height: 2.75rem;
		color: var(--accent);
		text-decoration: underline;
		cursor: pointer;
	}

	.add {
		justify-self: start;
	}

	.submit {
		width: 100%;
		font-size: 1.05rem;
	}
</style>
