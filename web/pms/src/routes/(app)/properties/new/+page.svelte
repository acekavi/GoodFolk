<script lang="ts">
	import { resolve } from '$app/paths';
	import { goto } from '$app/navigation';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { idempotencyKey, rest } from '$lib/api/rest';
	import { toApiError } from '$lib/api/problem';
	import { propertiesKey } from '$lib/properties';

	const client = useQueryClient();
	// One key per form: a double-click or network retry cannot create two properties.
	const key = idempotencyKey();
	let form = $state({ code: '', name: '', timezone: 'Asia/Colombo', base_currency: 'LKR' });
	let error = $state('');
	let busy = $state(false);

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		busy = true;
		error = '';
		const {
			data,
			error: problem,
			response
		} = await rest.POST('/api/v1/properties', {
			body: {
				...form,
				code: form.code.toUpperCase(),
				base_currency: form.base_currency.toUpperCase()
			},
			params: { header: { 'Idempotency-Key': key } }
		});
		busy = false;
		if (!data) {
			error = toApiError(problem, response.status).message;
			return;
		}
		await client.invalidateQueries({ queryKey: propertiesKey });
		await goto(resolve('/(app)/p/[property]', { property: data.id }));
	}
</script>

<h1>Add a property</h1>
<form class="form" onsubmit={submit}>
	<label>
		Code (2–10 letters or digits)
		<input required pattern={'[A-Za-z0-9]{2,10}'} bind:value={form.code} />
	</label>
	<label>Name <input required maxlength="200" bind:value={form.name} /></label>
	<label>Time zone <input required bind:value={form.timezone} /></label>
	<label>
		Base currency
		<input required pattern={'[A-Za-z]{3}'} maxlength="3" bind:value={form.base_currency} />
	</label>
	{#if error}<p class="error" role="alert">{error}</p>{/if}
	<button disabled={busy}>Create property</button>
</form>
