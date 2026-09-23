<script lang="ts">
	import { resolve } from '$app/paths';
	import { goto } from '$app/navigation';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { rest } from '$lib/api/rest';
	import { errorMessage, toApiError } from '$lib/api/problem';

	const client = useQueryClient();
	let email = $state('');
	let password = $state('');
	let error = $state('');
	let busy = $state(false);

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		busy = true;
		error = '';
		try {
			const {
				data,
				error: problem,
				response
			} = await rest.POST('/api/v1/auth/login', {
				body: { email, password }
			});
			if (!data) {
				error = toApiError(problem, response.status).message;
				return;
			}
			client.setQueryData(['me'], data);
			await goto(resolve('/'));
		} catch (err) {
			error = errorMessage(err);
		} finally {
			busy = false;
		}
	}
</script>

<main class="form" style="margin: 10vh auto">
	<h1>Sign in</h1>
	<form class="form" onsubmit={submit}>
		<label>Email <input type="email" autocomplete="username" required bind:value={email} /></label>
		<label>
			Password
			<input type="password" autocomplete="current-password" required bind:value={password} />
		</label>
		{#if error}<p class="error" role="alert">{error}</p>{/if}
		<button disabled={busy}>Sign in</button>
	</form>
	<p><a href={resolve('/signup')}>Create an account</a></p>
</main>
