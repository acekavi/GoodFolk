<!--
	The link of the email sent at sign-up: `/verify-email#<token>`. The token is in the fragment, which the browser
	never sends to a server, and it is dropped from the address as soon as it is read. Confirming needs no session, so
	the link works in any browser.
-->
<script lang="ts">
	import { resolve } from '$app/paths';
	import { goto } from '$app/navigation';
	import { onMount } from 'svelte';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { rest } from '$lib/api/rest';
	import { ApiError, errorMessage, toApiError } from '$lib/api/problem';

	const client = useQueryClient();
	let phase = $state<'confirming' | 'confirmed' | 'gone' | 'failed'>('confirming');
	let error = $state('');

	onMount(() => {
		const token = location.hash.slice(1);
		void goto(resolve('/verify-email'), { replaceState: true, keepFocus: true, noScroll: true });
		void confirm(token);
	});

	async function confirm(token: string) {
		if (!token) {
			phase = 'gone';
			return;
		}
		try {
			const { error: problem, response } = await rest.POST('/api/v1/auth/verify-email', {
				body: { token }
			});
			if (!response.ok) throw toApiError(problem, response.status);
			phase = 'confirmed';
			await client.invalidateQueries({ queryKey: ['me'] });
		} catch (err) {
			if (err instanceof ApiError && err.status === 410) {
				phase = 'gone';
			} else {
				error = errorMessage(err);
				phase = 'failed';
			}
		}
	}
</script>

<svelte:head>
	<!-- The token was in this page's address: never send it to another site as a referrer. -->
	<meta name="referrer" content="no-referrer" />
</svelte:head>

<main class="form" style="margin: 10vh auto">
	<h1>Confirm your email</h1>
	{#if phase === 'confirming'}
		<p>Confirming…</p>
	{:else if phase === 'confirmed'}
		<p role="status">Your email address is confirmed. You can now invite your staff.</p>
		<p><a href={resolve('/settings/staff')}>Go to Staff</a></p>
	{:else if phase === 'gone'}
		<p class="error" role="alert">This verification link is no longer valid.</p>
		<p>Sign in and ask for a new link on the Staff page.</p>
		<p><a href={resolve('/login')}>Sign in</a></p>
	{:else}
		<p class="error" role="alert">{error}</p>
		<p>Open the link from the email again to retry.</p>
	{/if}
</main>
