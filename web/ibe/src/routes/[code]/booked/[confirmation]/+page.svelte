<script lang="ts">
	import { page } from '$app/state';
	import { takeBooked } from '$lib/booking';
	import Contact from '$lib/components/Contact.svelte';
	import { dateText, mealPlanText, money, nights, plural, totalsText } from '$lib/format';

	let { data } = $props();

	const site = $derived(data.site);
	// Handed over in memory by the book page, once; a reload shows what the email has instead.
	const booked = takeBooked(page.params.confirmation ?? '');
</script>

<svelte:head>
	<title>Booking confirmed | {site.name}</title>
	<meta name="robots" content="noindex" />
	<meta name="referrer" content="no-referrer" />
</svelte:head>

<main class="wrap booked">
	<h1>Your booking is confirmed</h1>
	<p class="number">
		Confirmation number <strong>{page.params.confirmation}</strong>
	</p>

	{#if booked}
		<p>
			We have sent the details to <strong>{booked.email}</strong>. You pay at the hotel.
		</p>
		<section class="summary" aria-labelledby="stay-heading">
			<h2 id="stay-heading">Your stay at {site.name}</h2>
			<p>
				<strong>{dateText(booked.stay.checkIn)} to {dateText(booked.stay.checkOut)}</strong>,
				{plural(nights(booked.stay.checkIn, booked.stay.checkOut), 'night')}
			</p>
			<p class="muted">
				Check-in from {site.check_in_time}, check-out by {site.check_out_time}.
			</p>
			<ol>
				{#each booked.rooms as room, index (index)}
					<li>
						{room.name}, {room.plan}, {mealPlanText(room.mealPlan)}:
						{money(room.total, room.currency)}
					</li>
				{/each}
			</ol>
			<p class="total">Total {totalsText(booked.totals)}</p>
		</section>
		{#if booked.manageUrl}
			<p>
				<a class="button" href={booked.manageUrl} rel="noreferrer">View or cancel your booking</a>
			</p>
			<p class="muted">The same link is in your confirmation email.</p>
		{:else}
			<p class="muted">The link to view or cancel your booking is in your confirmation email.</p>
		{/if}
	{:else}
		<p>
			Your confirmation email has the details of your stay and the link to view or cancel your
			booking.
		</p>
	{/if}

	<Contact
		lead="To change your booking, contact the hotel."
		phone={site.contact_phone}
		email={site.contact_email}
	/>
</main>

<style>
	.booked {
		display: grid;
		gap: 1.25rem;
		max-width: 44rem;
		padding-block: 3rem 4rem;
	}

	h1 {
		font-size: clamp(2rem, 6vw, 3rem);
	}

	.number {
		font-size: 1.2rem;
	}

	.number strong {
		font-variant-numeric: tabular-nums;
		letter-spacing: 0.02em;
	}

	.summary {
		display: grid;
		gap: 0.6rem;
		padding: 1.5rem;
		background: var(--surface);
		border: 1px solid var(--line);
		border-radius: var(--radius);
	}

	ol {
		margin: 0;
		padding-left: 1.25rem;
	}

	.total {
		font-weight: 600;
	}
</style>
