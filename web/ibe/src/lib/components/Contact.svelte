<script lang="ts">
	/** How to reach the hotel, for anything the site cannot do, such as changing a booking. */
	let {
		lead,
		phone,
		email
	}: { lead: string; phone: string | null | undefined; email: string | null | undefined } =
		$props();
</script>

{#if phone || email}
	<div class="contact">
		<p>{lead}</p>
		<ul>
			{#if phone}<li>Phone <a href="tel:{phone.replace(/\s+/g, '')}">{phone}</a></li>{/if}
			{#if email}<li>Email <a href="mailto:{email}">{email}</a></li>{/if}
		</ul>
	</div>
{/if}

<style>
	.contact {
		display: grid;
		gap: 0.35rem;
	}

	ul {
		list-style: none;
		margin: 0;
		padding: 0;
	}
</style>
