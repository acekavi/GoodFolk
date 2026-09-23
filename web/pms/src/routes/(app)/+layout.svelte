<script lang="ts">
	import { resolve } from '$app/paths';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { rest } from '$lib/api/rest';
	import { ApiError, toApiError } from '$lib/api/problem';
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

	let tenantError = $state('');

	async function switchTenant(event: Event) {
		const select = event.currentTarget as HTMLSelectElement;
		tenantError = '';
		const {
			data,
			error: problem,
			response
		} = await rest.PUT('/api/v1/session/tenant', {
			body: { tenant_id: select.value }
		});
		if (!data) {
			select.value = me.data?.current_tenant ?? '';
			tenantError = toApiError(problem, response.status).message;
			return;
		}
		client.setQueryData(['me'], data);
		await client.invalidateQueries();
		await goto(resolve('/'));
	}

	async function logout() {
		await rest.POST('/api/v1/auth/logout');
		client.clear();
		await goto(resolve('/login'));
	}
</script>

{#if me.data}
	<header>
		{#if me.data.tenants.length > 1}
			<select aria-label="Tenant" value={me.data.current_tenant} onchange={switchTenant}>
				{#each me.data.tenants as tenant (tenant.id)}
					<option value={tenant.id}>{tenant.name}</option>
				{/each}
			</select>
		{:else}
			<strong>{me.data.tenants[0]?.name}</strong>
		{/if}
		{#if tenantError}<span class="error" role="alert">{tenantError}</span>{/if}
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
		<button onclick={logout}>Sign out</button>
	</header>
	<main>{@render children()}</main>
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
