<script lang="ts">
	import { resolve } from '$app/paths';
	import { goto } from '$app/navigation';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { rest } from '$lib/api/rest';
	import { errorMessage, toApiError } from '$lib/api/problem';

	const client = useQueryClient();
	let form = $state({ email: '', password: '', display_name: '', tenant_name: '' });
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
			} = await rest.POST('/api/v1/auth/signup', {
				body: form
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
	<h1>Create your account</h1>
	<form class="form" onsubmit={submit}>
		<label>Your name <input required maxlength="200" bind:value={form.display_name} /></label>
		<label>
			Hotel or group name <input required maxlength="200" bind:value={form.tenant_name} />
		</label>
		<label
			>Email <input type="email" autocomplete="username" required bind:value={form.email} /></label
		>
		<label>
			Password (at least 12 characters)
			<input
				type="password"
				autocomplete="new-password"
				required
				minlength="12"
				maxlength="128"
				bind:value={form.password}
			/>
		</label>
		{#if error}<p class="error" role="alert">{error}</p>{/if}
		<button disabled={busy}>Create account</button>
	</form>
	<p><a href={resolve('/login')}>Already have an account? Sign in</a></p>
</main>
