<script lang="ts">
	import { goto, invalidateAll } from '$app/navigation';
	import { onMount } from 'svelte';
	import { NETWORK_TROUBLE, publicApi, TOO_MANY_TRIES } from '$lib/api';
	import {
		attemptKeys,
		bookingPayload,
		handOver,
		depositLines,
		depositText,
		pricesAgree,
		sendBooking,
		totalsOf,
		type Details,
		type Outcome
	} from '$lib/booking';
	import { turnstileSiteKey } from '$lib/config';
	import { COUNTRIES } from '$lib/countries';
	import {
		dateText,
		guestsText,
		mealPlanText,
		money,
		nights,
		plural,
		totalsText
	} from '$lib/format';
	import { RESIDENCY_QUESTION, soldOutHref, stayQuery } from '$lib/search';

	const TURNSTILE_SCRIPT = 'https://challenges.cloudflare.com/turnstile/v0/api.js?render=explicit';
	const MAX_SPECIAL_REQUESTS = 500;

	let { data } = $props();

	const site = $derived(data.site);
	const stay = $derived(data.stay);
	const rooms = $derived(data.rooms);
	const resultsHref = $derived(`${data.base}/rooms?${stayQuery(stay)}`);
	function roomName(room: (typeof data.rooms)[number]) {
		return site.room_types.find((type) => type.id === room.pick.roomTypeId)?.name ?? 'Room';
	}

	const terms = $derived([...new Set(rooms.map((room) => room.plan.cancellation))]);

	// svelte-ignore state_referenced_locally
	let details: Details = $state({
		firstName: '',
		lastName: '',
		email: '',
		phone: '',
		country: data.stay.residency === 'resident' ? 'LK' : '',
		arrivalTime: '',
		occupants: data.rooms.map(() => ''),
		specialRequests: '',
		agreed: false
	});
	/** The honeypot: no person sees it, so whatever is in it was put there by a bot. */
	let website = $state('');

	/** What the guest agrees to pay: the search's totals, or the new ones once a changed price is shown. */
	let changedTotals: Record<string, number> | null = $state(null);
	const expected = $derived(changedTotals ?? totalsOf(rooms));
	/** The rooms' own prices and deposit, while they add up to the totals the guest is agreeing to. */
	const pricesCurrent = $derived(pricesAgree(rooms, expected));
	const deposit = $derived(pricesCurrent ? depositText(depositLines(rooms)) : null);

	let token = $state('');
	let widget: string | null = null;
	let widgetElement: HTMLDivElement | undefined = $state();
	let checkFailed = $state(false);

	let busy = $state(false);
	let message: string | null = $state(null);
	const keys = attemptKeys();
	/** While an attempt may still have been booked, the details stay as they were sent, so a retry replays it. */
	let locked = $state(false);

	/** The page paints before it hydrates; until then **Book** waits, so nothing is sent but by the booking code. */
	let hydrated = $state(false);

	onMount(() => {
		hydrated = true;
		const render = () => {
			if (!window.turnstile || !widgetElement) return;
			widget = window.turnstile.render(widgetElement, {
				sitekey: turnstileSiteKey(),
				action: 'booking',
				callback: (issued) => {
					token = issued;
					checkFailed = false;
				},
				'expired-callback': () => (token = ''),
				'error-callback': () => {
					token = '';
					checkFailed = true;
				}
			});
		};
		if (window.turnstile) {
			render();
		} else {
			const script = document.createElement('script');
			script.src = TURNSTILE_SCRIPT;
			script.async = true;
			script.onload = render;
			script.onerror = () => (checkFailed = true);
			document.head.append(script);
		}
		return () => {
			if (widget) window.turnstile?.remove(widget);
		};
	});

	/**
	 * A token may be spent once the API has seen it, so every attempt after the first takes a new one. A retry of the
	 * same booking keeps its key all the same: core-api leaves the token out of what it compares.
	 */
	function newCheck() {
		token = '';
		if (widget) window.turnstile?.reset(widget);
	}

	async function book(event: SubmitEvent) {
		event.preventDefault();
		message = null;
		if (!token) {
			message = checkFailed
				? 'The security check could not load. Reload the page and try again.'
				: 'Wait a moment for the security check to finish, then book.';
			return;
		}
		busy = true;
		const body = bookingPayload(stay, rooms, details, expected, token, website);
		const key = keys.keyFor();
		let outcome: Outcome;
		try {
			outcome = await sendBooking(key, async (sent) => {
				const {
					data: confirmed,
					error,
					response
				} = await publicApi().POST('/api/public/v1/ibe/{code}/bookings', {
					params: { path: { code: site.code }, header: { 'Idempotency-Key': sent } },
					body
				});
				return {
					status: response.status,
					body: confirmed ?? error,
					retryAfter: response.headers.get('retry-after')
				};
			});
		} catch {
			// The same booking goes again with the same key: if this one got through, the retry gets its answer.
			newCheck();
			busy = false;
			locked = keys.pending;
			message = NETWORK_TROUBLE;
			return;
		}
		keys.answered(outcome);
		if (outcome.kind !== 'booked') newCheck();
		busy = false;
		locked = keys.pending;
		switch (outcome.kind) {
			case 'booked':
				handOver({
					confirmationNo: outcome.confirmationNo,
					manageUrl: outcome.manageUrl,
					stay,
					rooms: rooms.map((room) => ({
						name: roomName(room),
						plan: room.plan.name,
						mealPlan: room.pick.mealPlan,
						currency: room.plan.currency,
						total: room.offer.total
					})),
					totals: expected,
					email: details.email.trim()
				});
				await goto(`${data.base}/booked/${encodeURIComponent(outcome.confirmationNo)}`);
				return;
			case 'price_changed':
				changedTotals = outcome.totals;
				// Search again, for the rooms' new prices and deposit, not just the new totals.
				await invalidateAll();
				return;
			case 'sold_out':
				// The attempt's key makes the search new to the edge's cache, which may still offer the room.
				await goto(soldOutHref(resultsHref, key));
				return;
			case 'in_progress':
				// The key is kept: booking again asks after the same attempt, so it can never book twice.
				message =
					'Your booking is still being processed. Check your email for its confirmation before you book again.';
				return;
			case 'too_many':
				message = TOO_MANY_TRIES;
				return;
			case 'bot_check':
				message = 'The security check did not pass. Wait for it to finish again, then book.';
				return;
			case 'invalid':
				message = outcome.message;
				return;
			case 'failed':
				message = 'The booking could not be completed. Try again in a moment.';
				return;
		}
	}
</script>

<svelte:head>
	<title>Your details | {site.name}</title>
	<meta name="robots" content="noindex" />
</svelte:head>

<main class="wrap book">
	<p><a href={resultsHref} data-sveltekit-preload-data="off">Back to the rooms</a></p>
	<h1>Your details</h1>

	<div class="layout">
		<section class="summary" aria-labelledby="summary-heading">
			<h2 id="summary-heading">Your stay</h2>
			<p>
				<strong>{dateText(stay.checkIn)} to {dateText(stay.checkOut)}</strong><br />
				<span class="muted">{plural(nights(stay.checkIn, stay.checkOut), 'night')}</span>
			</p>
			<ol class="rooms">
				{#each rooms as room, index (index)}
					<li>
						<p><strong>{roomName(room)}</strong></p>
						<p class="muted">
							{room.plan.name}, {mealPlanText(room.pick.mealPlan)}, {guestsText(
								stay.rooms[index].adults,
								stay.rooms[index].children
							)}
						</p>
						{#if pricesCurrent}<p>{money(room.offer.total, room.plan.currency)}</p>{/if}
					</li>
				{/each}
			</ol>
			{#if changedTotals}
				<div class="notice error" role="alert">
					<p><strong>The price has changed since you searched.</strong></p>
					<p>The new total is {totalsText(changedTotals)}. Book again to confirm it.</p>
				</div>
			{/if}
			<p class="total">Total {totalsText(expected)}</p>
			<p class="muted">Taxes and service charge included. You pay at the hotel.</p>
			{#if deposit}<p>{deposit}</p>{/if}
			<h3>Cancellation</h3>
			{#each terms as text (text)}<p class="muted">{text}</p>{/each}
		</section>

		<!-- `post` with no action: should the form ever be submitted natively, the guest's details stay out of the address
		     (and the logs). The booking itself is always `book`, once hydrated. -->
		<form class="details" method="post" onsubmit={book}>
			<fieldset class="sent" disabled={locked}>
				<fieldset>
					<legend>Who is booking</legend>
					<div class="pair">
						<label>
							First name
							<input
								type="text"
								bind:value={details.firstName}
								autocomplete="given-name"
								required
							/>
						</label>
						<label>
							Last name
							<input
								type="text"
								bind:value={details.lastName}
								autocomplete="family-name"
								required
							/>
						</label>
					</div>
					<label>
						Email
						<input type="email" bind:value={details.email} autocomplete="email" required />
					</label>
					<label>
						Phone
						<input type="tel" bind:value={details.phone} autocomplete="tel" required />
					</label>
					<label>
						Country of residence
						<select bind:value={details.country} autocomplete="country" required>
							<option value="" disabled>Choose a country</option>
							{#each COUNTRIES as country (country.code)}
								<option value={country.code}>{country.name}</option>
							{/each}
						</select>
					</label>
					<p class="residency">
						{RESIDENCY_QUESTION}
						<strong>{stay.residency === 'resident' ? 'Yes' : 'No'}</strong>
						<span class="muted">(from your search; the prices depend on it.)</span>
						<a href="{data.base}/rooms?{stayQuery(stay)}" data-sveltekit-preload-data="off"
							>Change</a
						>
					</p>
					<label>
						Arrival time <span class="muted">(optional)</span>
						<input type="time" bind:value={details.arrivalTime} />
					</label>
				</fieldset>

				<fieldset>
					<legend>Guests in each room</legend>
					{#each rooms as room, index (index)}
						<label>
							{rooms.length > 1 ? `Room ${index + 1}, ${roomName(room)}` : roomName(room)}: name of
							the guest
							<span class="muted">(optional; you, if left empty)</span>
							<input type="text" bind:value={details.occupants[index]} autocomplete="off" />
						</label>
					{/each}
				</fieldset>

				<label>
					Special requests <span class="muted">(optional)</span>
					<textarea
						bind:value={details.specialRequests}
						maxlength={MAX_SPECIAL_REQUESTS}
						aria-describedby="requests-count"></textarea>
					<span id="requests-count" class="muted small"
						>{details.specialRequests.length} of {MAX_SPECIAL_REQUESTS} characters</span
					>
				</label>

				<div class="honeypot" aria-hidden="true">
					<label>
						Website
						<input
							type="text"
							name="website"
							bind:value={website}
							tabindex="-1"
							autocomplete="off"
						/>
					</label>
				</div>

				<div class="agree">
					<label class="check">
						<input type="checkbox" bind:checked={details.agreed} required />
						I agree to the hotel’s booking terms and privacy notice
					</label>
					{#if site.terms}
						<details>
							<summary>Booking terms</summary>
							<p class="legal">{site.terms}</p>
						</details>
					{/if}
					{#if site.privacy}
						<details>
							<summary>Privacy notice</summary>
							<p class="legal">{site.privacy}</p>
						</details>
					{/if}
				</div>
			</fieldset>

			{#if locked}
				<p class="notice">
					Your details stay as you sent them until we know whether that booking went through, so
					trying again can never book twice.
				</p>
			{/if}

			<div class="check-widget" bind:this={widgetElement}></div>

			{#if message}
				<p class="notice error" role="alert">{message}</p>
			{/if}

			<button class="button submit" type="submit" disabled={busy || !hydrated}>
				{#if busy}
					Booking…
				{:else if changedTotals}
					Book at the new price
				{:else}
					Book now, pay at the hotel
				{/if}
			</button>
		</form>
	</div>
</main>

<style>
	.book {
		display: grid;
		gap: 1.25rem;
		padding-block: 2rem 4rem;
	}

	h1 {
		font-size: clamp(1.75rem, 5vw, 2.5rem);
	}

	.layout {
		display: grid;
		gap: 2rem;
	}

	.summary,
	.details {
		display: grid;
		gap: 1rem;
		align-content: start;
	}

	.summary {
		padding: 1.5rem;
		background: var(--surface);
		border: 1px solid var(--line);
		border-radius: var(--radius);
	}

	.rooms {
		margin: 0;
		padding: 0;
		list-style: none;
		display: grid;
		gap: 0.75rem;
	}

	.rooms li {
		padding-bottom: 0.75rem;
		border-bottom: 1px solid var(--line);
	}

	.total {
		font-size: 1.2rem;
		font-weight: 600;
	}

	fieldset {
		display: grid;
		gap: 1rem;
		border: 0;
		margin: 0;
		padding: 0;
		min-width: 0;
	}

	legend {
		font-family: var(--serif);
		font-size: 1.3rem;
		font-weight: 600;
		margin-bottom: 0.5rem;
		padding: 0;
	}

	.pair {
		display: grid;
		gap: 1rem;
	}

	label .muted {
		font-weight: 400;
	}

	.residency {
		display: flex;
		flex-wrap: wrap;
		gap: 0.25rem 0.5rem;
	}

	.small {
		font-size: 0.9rem;
		font-weight: 400;
	}

	.honeypot {
		position: absolute;
		left: -10000px;
		width: 1px;
		height: 1px;
		overflow: hidden;
	}

	.agree {
		display: grid;
		gap: 0.5rem;
	}

	.check {
		display: flex;
		gap: 0.6rem;
		align-items: start;
		font-weight: 500;
	}

	.check input {
		width: 1.25rem;
		height: 1.25rem;
		margin: 0.15rem 0 0;
		accent-color: var(--accent);
	}

	details summary {
		cursor: pointer;
		text-decoration: underline 2px var(--accent);
		text-underline-offset: 0.2em;
		min-height: 2.25rem;
		display: inline-flex;
		align-items: center;
	}

	.legal {
		white-space: pre-line;
		max-height: 16rem;
		overflow: auto;
		padding: 0.75rem;
		border: 1px solid var(--line);
		background: var(--surface);
		font-size: 0.95rem;
	}

	.check-widget {
		min-height: 65px;
	}

	.submit {
		width: 100%;
		font-size: 1.05rem;
	}

	@media (min-width: 40rem) {
		.pair {
			grid-template-columns: 1fr 1fr;
		}
	}

	@media (min-width: 60rem) {
		.layout {
			grid-template-columns: minmax(0, 1fr) 22rem;
			gap: 3rem;
		}

		.summary {
			grid-column: 2;
			grid-row: 1;
			position: sticky;
			top: 1.5rem;
		}

		.details {
			grid-column: 1;
			grid-row: 1;
		}
	}
</style>
