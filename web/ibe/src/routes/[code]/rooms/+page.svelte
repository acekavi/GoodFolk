<script lang="ts">
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import type { MealOffer, PlanOffer, RoomTypeResult } from '$lib/api';
	import { bookHref, pricePicks, totalsOf, type Pick } from '$lib/booking';
	import Gallery from '$lib/components/Gallery.svelte';
	import SearchForm from '$lib/components/SearchForm.svelte';
	import {
		bedsText,
		dateText,
		guestsText,
		mealPlanText,
		money,
		nights,
		plural,
		shortDateText,
		totalsText
	} from '$lib/format';
	import { formFromUrl, movedTo, stayQuery, type Stay } from '$lib/search';

	let { data } = $props();

	const site = $derived(data.site);
	const searched = $derived(data.searched);

	/** The guest's choice per room asked, in order; `null` until chosen. A new search starts them again. */
	let picks: (Pick | null)[] = $derived(
		searched.kind === 'found' ? searched.stay.rooms.map(() => null) : []
	);
	/** The room the offers are shown for: the first, after a new search. */
	let active = $derived.by(() => {
		void searched;
		return 0;
	});

	/** The results in the site's display order, with the room type's own content. */
	const results = $derived.by(() => {
		if (searched.kind !== 'found') return [];
		return site.room_types.flatMap((type) => {
			const result = searched.result.room_types.find((found) => found.room_type_id === type.id);
			return result ? [{ type, result }] : [];
		});
	});

	const chosen = $derived(
		searched.kind === 'found' && picks.every((pick) => pick !== null)
			? pricePicks(searched.result, picks as Pick[])
			: null
	);

	function nameOf(roomTypeId: string) {
		return site.room_types.find((type) => type.id === roomTypeId)?.name ?? 'Room';
	}

	/** Whether the room type is chosen for as many other rooms as it has free. */
	function full(result: RoomTypeResult) {
		const others = picks.filter(
			(pick, index) => index !== active && pick?.roomTypeId === result.room_type_id
		);
		return others.length >= result.available;
	}

	function offersFor(plan: PlanOffer): MealOffer[] {
		return plan.meal_plans.filter((offer) => offer.rooms.includes(active));
	}

	function choose(stay: Stay, result: RoomTypeResult, plan: PlanOffer, offer: MealOffer) {
		picks = picks.with(active, {
			roomTypeId: result.room_type_id,
			ratePlanId: plan.rate_plan_id,
			mealPlan: offer.meal_plan
		});
		const next = picks.findIndex((pick) => pick === null);
		if (next === -1 && stay.rooms.length === 1) {
			goto(bookHref(data.base, stay, picks as Pick[]));
			return;
		}
		if (next !== -1) active = next;
		document.getElementById(next === -1 ? 'your-rooms' : 'results-heading')?.focus();
	}
</script>

<svelte:head>
	<title>Rooms and prices | {site.name}</title>
	<meta name="robots" content="noindex" />
</svelte:head>

<main class="wrap results">
	{#if data.soldOut}
		<p class="notice error" role="alert">
			That room was just booked; here is what is still available
		</p>
	{/if}

	{#if searched.kind === 'unanswered'}
		<section class="ask">
			<h1>See rooms and prices</h1>
			<SearchForm
				roomTypes={site.room_types}
				businessDate={site.business_date}
				base={data.base}
				initial={formFromUrl(page.url, site.business_date)}
			/>
		</section>
	{:else if searched.kind === 'refused'}
		<section class="ask">
			<h1>Change your search</h1>
			<p class="notice error" role="alert">{searched.message}</p>
			<SearchForm
				roomTypes={site.room_types}
				businessDate={site.business_date}
				base={data.base}
				initial={formFromUrl(page.url, site.business_date)}
			/>
		</section>
	{:else}
		{@const stay = searched.stay}
		<section class="stay" aria-label="Your search">
			<p>
				<strong>{dateText(stay.checkIn)} to {dateText(stay.checkOut)}</strong>
				<span class="muted">
					{plural(nights(stay.checkIn, stay.checkOut), 'night')}, {plural(
						stay.rooms.length,
						'room'
					)}
				</span>
			</p>
			<details class="change">
				<summary>Change search</summary>
				<SearchForm
					roomTypes={site.room_types}
					businessDate={site.business_date}
					base={data.base}
					initial={formFromUrl(page.url, site.business_date)}
				/>
			</details>
		</section>

		{#if results.length === 0}
			<section class="none">
				<h1>No rooms available for these dates</h1>
				{#if searched.result.next_available}
					{@const moved = movedTo(stay, searched.result.next_available)}
					<p>
						<a class="button" href="{data.base}/rooms?{stayQuery(moved)}"
							>Try {dateText(searched.result.next_available)}</a
						>
					</p>
				{:else}
					<p>Try other dates, or fewer rooms.</p>
				{/if}
			</section>
		{:else}
			<div class="layout">
				<section class="offers" aria-labelledby="results-heading">
					<h1 id="results-heading" tabindex="-1">
						{#if stay.rooms.length > 1}
							Choose room {active + 1} of {stay.rooms.length}
							<span class="muted">
								({guestsText(stay.rooms[active].adults, stay.rooms[active].children)})</span
							>
						{:else}
							Choose your room
						{/if}
					</h1>
					<p class="muted">Prices are for the whole stay, with taxes and service charge.</p>

					<ul class="types">
						{#each results as { type, result } (type.id)}
							<li class="type">
								<Gallery media={type.media} name={type.name} />
								<div class="type-text">
									<h2>{type.name}</h2>
									{#if type.description}<p>{type.description}</p>{/if}
									<p class="muted">
										{#if type.beds.length}{bedsText(type.beds)}.
										{/if}Sleeps up to {plural(type.max_occupancy, 'guest')}.
										{#if result.available <= 3}{plural(result.available, 'room')} left.{/if}
									</p>
									{#if type.amenities.length}
										<p class="muted amenities">{type.amenities.join(', ')}</p>
									{/if}
								</div>

								{#each result.plans as plan (plan.rate_plan_id)}
									{@const offers = offersFor(plan)}
									{#if offers.length}
										<div class="plan">
											<h3>{plan.name}</h3>
											<p class="muted small">{plan.cancellation}</p>
											<ul class="meals">
												{#each offers as offer (offer.meal_plan)}
													{@const picked =
														picks[active]?.roomTypeId === result.room_type_id &&
														picks[active]?.ratePlanId === plan.rate_plan_id &&
														picks[active]?.mealPlan === offer.meal_plan}
													<li class="meal" class:picked>
														<div>
															<p class="meal-name">{mealPlanText(offer.meal_plan)}</p>
															<p class="price">{money(offer.total, plan.currency)}</p>
															<details class="breakdown">
																<summary>Price per night</summary>
																<table>
																	<tbody>
																		{#each offer.nights as night (night.date)}
																			<tr>
																				<th scope="row">{shortDateText(night.date)}</th>
																				<td>{money(night.total, plan.currency)}</td>
																			</tr>
																		{/each}
																	</tbody>
																</table>
																{#if offer.taxes.length}
																	<p class="muted small">Included in the price:</p>
																	<ul class="taxes">
																		{#each offer.taxes as tax (tax.name)}
																			<li>{tax.name} {money(tax.amount, plan.currency)}</li>
																		{/each}
																	</ul>
																{/if}
															</details>
														</div>
														<button
															type="button"
															class="button"
															class:quiet={!picked}
															disabled={full(result)}
															aria-pressed={picked}
															aria-label="Choose {type.name}, {plan.name}, {mealPlanText(
																offer.meal_plan
															)}{stay.rooms.length > 1 ? ` for room ${active + 1}` : ''}"
															onclick={() => choose(stay, result, plan, offer)}
														>
															{picked ? 'Chosen' : 'Choose'}
														</button>
													</li>
												{/each}
											</ul>
										</div>
									{/if}
								{/each}
							</li>
						{/each}
					</ul>
				</section>

				{#if stay.rooms.length > 1}
					<aside class="summary" aria-labelledby="your-rooms">
						<h2 id="your-rooms" tabindex="-1">Your rooms</h2>
						<ol>
							{#each stay.rooms as room, index (index)}
								{@const pick = picks[index]}
								<li class:current={index === active}>
									<p>
										<strong>Room {index + 1}</strong>
										<span class="muted">{guestsText(room.adults, room.children)}</span>
									</p>
									{#if pick}
										<p>{nameOf(pick.roomTypeId)}, {mealPlanText(pick.mealPlan)}</p>
									{:else}
										<p class="muted">Not chosen yet</p>
									{/if}
									{#if index !== active}
										<button type="button" class="link" onclick={() => (active = index)}>
											{pick ? 'Change' : 'Choose'}<span class="visually-hidden">
												room {index + 1}</span
											>
										</button>
									{/if}
								</li>
							{/each}
						</ol>
						{#if chosen}
							<p class="total">Total {totalsText(totalsOf(chosen))}</p>
							<a class="button" href={bookHref(data.base, stay, picks as Pick[])}
								>Continue to your details</a
							>
						{:else if picks.every((pick) => pick !== null)}
							<p class="notice error" role="alert">
								Those rooms are not all free together; choose another for one of them.
							</p>
						{/if}
					</aside>
				{/if}
			</div>
		{/if}
	{/if}
</main>

<style>
	.results {
		display: grid;
		gap: 2rem;
		padding-block: 2rem 4rem;
	}

	.ask,
	.none {
		display: grid;
		gap: 1.25rem;
		max-width: 40rem;
	}

	h1 {
		font-size: clamp(1.75rem, 5vw, 2.5rem);
	}

	h1 .muted {
		font-family: var(--sans);
		font-size: 1rem;
		font-weight: 400;
	}

	h1:focus,
	h2:focus {
		outline: none;
	}

	.stay {
		display: grid;
		gap: 0.75rem;
		padding-bottom: 1.25rem;
		border-bottom: 1px solid var(--line);
	}

	.stay p {
		display: flex;
		flex-wrap: wrap;
		gap: 0.25rem 1rem;
	}

	.change summary {
		cursor: pointer;
		text-decoration: underline 2px var(--accent);
		text-underline-offset: 0.2em;
		min-height: 2.75rem;
		display: inline-flex;
		align-items: center;
	}

	.change :global(.search) {
		margin-top: 0.75rem;
		max-width: 40rem;
	}

	.layout {
		display: grid;
		gap: 2rem;
	}

	.offers {
		display: grid;
		gap: 0.75rem;
		align-content: start;
	}

	.types {
		list-style: none;
		margin: 1rem 0 0;
		padding: 0;
		display: grid;
		gap: 3rem;
	}

	.type {
		display: grid;
		gap: 1.25rem;
	}

	.type-text {
		display: grid;
		gap: 0.5rem;
		max-width: var(--measure);
	}

	.plan {
		display: grid;
		gap: 0.5rem;
		padding-top: 1rem;
		border-top: 1px solid var(--line);
	}

	.plan h3 {
		font-size: 1.15rem;
	}

	.small {
		font-size: 0.9rem;
	}

	.meals {
		list-style: none;
		margin: 0;
		padding: 0;
		display: grid;
		gap: 0.5rem;
	}

	.meal {
		display: flex;
		justify-content: space-between;
		align-items: start;
		gap: 1rem;
		padding: 0.9rem 1rem;
		background: var(--surface);
		border: 1px solid var(--line);
		border-radius: var(--radius);
	}

	.meal.picked {
		border-color: var(--accent);
		box-shadow: inset 4px 0 0 var(--accent);
	}

	.meal-name {
		font-weight: 600;
	}

	.price {
		font-size: 1.2rem;
		font-variant-numeric: tabular-nums;
	}

	.breakdown summary {
		cursor: pointer;
		color: var(--accent);
		font-size: 0.9rem;
		min-height: 2rem;
		display: inline-flex;
		align-items: center;
	}

	table {
		border-collapse: collapse;
		margin-block: 0.25rem 0.5rem;
		font-size: 0.9rem;
		font-variant-numeric: tabular-nums;
	}

	th,
	td {
		padding: 0.15rem 1rem 0.15rem 0;
		text-align: left;
		font-weight: 400;
	}

	.taxes {
		margin: 0.15rem 0 0;
		padding-left: 1.1rem;
		font-size: 0.9rem;
	}

	.summary {
		position: sticky;
		bottom: 0;
		display: grid;
		gap: 0.75rem;
		padding: 1.25rem;
		background: var(--surface);
		border: 1px solid var(--line);
		border-radius: var(--radius) var(--radius) 0 0;
		box-shadow: 0 -8px 24px rgb(30 40 38 / 0.08);
	}

	.summary ol {
		list-style: none;
		margin: 0;
		padding: 0;
		display: grid;
		gap: 0.6rem;
	}

	.summary li {
		padding-left: 0.75rem;
		border-left: 3px solid transparent;
	}

	.summary li.current {
		border-left-color: var(--accent);
	}

	.summary li p {
		display: flex;
		flex-wrap: wrap;
		gap: 0 0.6rem;
	}

	.link {
		border: 0;
		background: none;
		padding: 0.25rem 0;
		min-height: 2.25rem;
		color: var(--accent);
		text-decoration: underline;
		cursor: pointer;
	}

	.total {
		font-weight: 600;
		font-size: 1.1rem;
	}

	@media (min-width: 60rem) {
		.layout {
			grid-template-columns: minmax(0, 1fr) 20rem;
			gap: 3rem;
		}

		.type {
			grid-template-columns: minmax(0, 26rem) minmax(0, 1fr);
			column-gap: 2rem;
		}

		.type > :global(.gallery) {
			grid-row: span 3;
		}

		.plan {
			grid-column: 2;
		}

		.summary {
			bottom: auto;
			top: 1.5rem;
			align-self: start;
			border-radius: var(--radius);
			box-shadow: none;
		}
	}
</style>
