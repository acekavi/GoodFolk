<script lang="ts">
	import { resolve } from '$app/paths';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { rest } from '$lib/api/rest';
	import { ApiError, errorMessage, toApiError } from '$lib/api/problem';
	import { connectEvents } from '$lib/events';
	import { fetchProperties, propertiesKey } from '$lib/properties';
	import { fetchMe } from '$lib/session';

	let { children } = $props();

	const client = useQueryClient();
	const me = createQuery(() => ({ queryKey: ['me'], queryFn: fetchMe }));
	const properties = createQuery(() => ({
		queryKey: propertiesKey,
		queryFn: ({ signal }) => fetchProperties(signal),
		enabled: !!me.data?.current_tenant
	}));

	$effect(() => {
		if (me.error instanceof ApiError && me.error.status === 401) void goto(resolve('/login'));
	});

	$effect(() => {
		if (me.data) return connectEvents(client);
	});

	let actionError = $state('');
	let busy = $state(false);

	async function switchTenant(event: Event) {
		const select = event.currentTarget as HTMLSelectElement;
		busy = true;
		actionError = '';
		try {
			const {
				data,
				error: problem,
				response
			} = await rest.PUT('/api/v1/session/tenant', {
				body: { tenant_id: select.value }
			});
			if (!data) {
				select.value = me.data?.current_tenant ?? '';
				actionError = toApiError(problem, response.status).message;
				return;
			}
			client.setQueryData(['me'], data);
			// Clear the previous tenant's cached data so it is never shown under the new tenant.
			// Unlike removeQueries, a reset also clears what mounted queries show, and refetches them.
			void client.resetQueries({ predicate: (query) => query.queryKey[0] !== 'me' });
			await goto(resolve('/'));
		} catch (err) {
			select.value = me.data?.current_tenant ?? '';
			actionError = errorMessage(err);
		} finally {
			busy = false;
		}
	}

	async function logout() {
		busy = true;
		actionError = '';
		try {
			const { error: problem, response } = await rest.POST('/api/v1/auth/logout');
			// 401: the session had already ended, which is what signing out wants.
			if (!response.ok && response.status !== 401) {
				actionError = toApiError(problem, response.status).message;
				return;
			}
			client.clear();
			await goto(resolve('/login'));
		} catch (err) {
			actionError = errorMessage(err);
		} finally {
			busy = false;
		}
	}
</script>

{#if me.data}
	<header>
		{#if me.data.tenants.length > 1}
			<select
				aria-label="Tenant"
				value={me.data.current_tenant}
				disabled={busy}
				onchange={switchTenant}
			>
				{#each me.data.tenants as tenant (tenant.id)}
					<option value={tenant.id}>{tenant.name}</option>
				{/each}
			</select>
		{:else}
			<strong>{me.data.tenants[0]?.name}</strong>
		{/if}
		{#if actionError}<span class="error" role="alert">{actionError}</span>{/if}
		<nav aria-label="Properties">
			{#each properties.data ?? [] as property (property.id)}
				<a
					href={resolve('/(app)/p/[property]', { property: property.id })}
					aria-current={page.params.property === property.id ? 'page' : undefined}
				>
					{property.code}
				</a>
			{/each}
		</nav>
		<span class="spacer"></span>
		<span>{me.data.display_name}</span>
		<button disabled={busy} onclick={logout}>Sign out</button>
	</header>
	<main>{@render children()}</main>
{:else if me.error && !(me.error instanceof ApiError && me.error.status === 401)}
	<main>
		<p class="error" role="alert">{errorMessage(me.error)}</p>
		<button onclick={() => me.refetch()}>Retry</button>
	</main>
{:else}
	<!-- Also shown for a 401 while the redirect to sign-in happens. -->
	<main><p>Loading…</p></main>
{/if}

<style>
	header {
		display: flex;
		align-items: center;
		gap: var(--space);
		padding: 0.5rem 1rem;
		border-bottom: 1px solid var(--border);
		background: var(--surface);
	}
	nav {
		display: flex;
		gap: 0.5rem;
	}
	nav a[aria-current='page'] {
		font-weight: 600;
	}
	.spacer {
		flex: 1;
	}
	main {
		padding: 1rem;
	}
</style>
