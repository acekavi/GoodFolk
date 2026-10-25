<script lang="ts">
	import { resolve } from '$app/paths';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { rest } from '$lib/api/rest';
	import { ApiError, errorMessage, toApiError } from '$lib/api/problem';
	import { fetchMe, ownerOf } from '$lib/session';
	import { wipeDevice } from '$lib/hk-sync.svelte';
	import { grantLabel, inviteSignInHref, isInvitee, landingProperty } from '$lib/staff';

	const client = useQueryClient();
	const token = $derived(page.params.token ?? '');

	// A failed preview counts toward the server's throttle, so it is never retried or refetched on focus.
	const preview = createQuery(() => ({
		queryKey: ['invitation', token],
		queryFn: async () => {
			const { data, error, response } = await rest.POST('/api/v1/invitations/preview', {
				body: { token }
			});
			if (!data) throw toApiError(error, response.status);
			return data;
		},
		retry: false,
		staleTime: Infinity,
		refetchOnWindowFocus: false
	}));
	// 401 is the expected answer when signed out.
	const me = createQuery(() => ({ queryKey: ['me'], queryFn: fetchMe, retry: false }));

	const invitation = $derived(preview.data);
	const profile = $derived(me.data);
	const loading = $derived(preview.isPending || me.isPending);
	// Only a 401 means signed out; any other failure to read the session is shown as an error.
	const meFailed = $derived(
		me.error && !(me.error instanceof ApiError && me.error.status === 401) ? me.error : null
	);
	let needsSignIn = $state(false);

	let password = $state('');
	let error = $state('');
	let busy = $state(false);

	async function accept(event: SubmitEvent) {
		event.preventDefault();
		busy = true;
		error = '';
		needsSignIn = false;
		try {
			const {
				data,
				error: problem,
				response
			} = await rest.POST('/api/v1/invitations/accept', {
				body: { token, password: profile ? undefined : password }
			});
			if (!data) {
				const failure = toApiError(problem, response.status);
				error = failure.message;
				needsSignIn = failure.status === 409;
				return;
			}
			client.setQueryData(['me'], data);
			// The session now belongs to the new tenant: drop the previous one's cached data. The invitation's
			// own query is left alone: its token is used now, and refetching it would be a 410.
			void client.resetQueries({
				predicate: (query) => query.queryKey[0] !== 'me' && query.queryKey[0] !== 'invitation'
			});
			const property = landingProperty(data);
			await goto(property ? resolve('/(app)/p/[property]', { property }) : resolve('/'));
		} catch (err) {
			error = errorMessage(err);
		} finally {
			busy = false;
		}
	}

	async function signOut() {
		busy = true;
		error = '';
		try {
			const { error: problem, response } = await rest.POST('/api/v1/auth/logout');
			// 401: the session had already ended, which is what signing out wants.
			if (!response.ok && response.status !== 401) {
				error = toApiError(problem, response.status).message;
				return;
			}
			const owner = profile ? ownerOf(profile) : null;
			client.clear();
			if (owner) await wipeDevice(owner);
			// The invitee signs in next, and sign-in returns here.
			// eslint-disable-next-line svelte/no-navigation-without-resolve -- same-origin path built by inviteSignInHref
			await goto(inviteSignInHref(token));
		} catch (err) {
			error = errorMessage(err);
		} finally {
			busy = false;
		}
	}
</script>

<main class="form" style="margin: 10vh auto">
	{#if loading}
		<p>Loading…</p>
	{:else if preview.error}
		<h1>Invitation</h1>
		<p class="error" role="alert">
			{preview.error instanceof ApiError && preview.error.status === 410
				? 'This invitation is no longer valid'
				: errorMessage(preview.error)}
		</p>
		<p><a href={resolve('/login')}>Sign in</a></p>
	{:else if meFailed}
		<p class="error" role="alert">{errorMessage(meFailed)}</p>
		<button onclick={() => me.refetch()}>Retry</button>
	{:else if invitation}
		<h1>Join {invitation.tenant_name}</h1>
		<p>
			{#if invitation.inviter}{invitation.inviter} invited{:else}You are invited{/if}
			<strong>{invitation.email}</strong> as:
		</p>
		<ul>
			{#each invitation.grants as grant, i (i)}
				<li>{grantLabel(grant)}</li>
			{/each}
		</ul>

		{#if profile && !isInvitee(profile, invitation.email)}
			<p role="alert">
				You're signed in as {profile.email}; this invitation is for {invitation.email}.
			</p>
			<button disabled={busy} onclick={signOut}>Sign out to accept</button>
		{:else if profile}
			<form class="form" onsubmit={accept}>
				<p>Signed in as {profile.email}.</p>
				{#if error}<p class="error" role="alert">{error}</p>{/if}
				<button disabled={busy}>Accept</button>
			</form>
		{:else if invitation.existing_account}
			<p>An account already uses {invitation.email}. Sign in as it to accept.</p>
			<!-- eslint-disable-next-line svelte/no-navigation-without-resolve -- same-origin path built by inviteSignInHref -->
			<p><a href={inviteSignInHref(token)}>Sign in to accept</a></p>
		{:else}
			<form class="form" onsubmit={accept}>
				<label>
					Set a password (at least 12 characters)
					<input
						type="password"
						autocomplete="new-password"
						required
						minlength="12"
						maxlength="128"
						bind:value={password}
					/>
				</label>
				{#if error}<p class="error" role="alert">{error}</p>{/if}
				{#if needsSignIn}
					<!-- eslint-disable-next-line svelte/no-navigation-without-resolve -- same-origin path built by inviteSignInHref -->
					<p><a href={inviteSignInHref(token)}>Sign in to accept</a></p>
				{/if}
				<button disabled={busy}>Accept</button>
			</form>
		{/if}
		{#if error && profile && !isInvitee(profile, invitation.email)}
			<p class="error" role="alert">{error}</p>
		{/if}
	{/if}
</main>
