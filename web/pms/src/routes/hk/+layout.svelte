<!--
	The housekeeper app's shell: a phone-sized column outside the property navigation. It enforces the same
	sign-in as the `(app)` group, picks the property (the URL's `property`, else the last one used, else the
	only one the user can work at) and keeps the cache live for it. A user who may not see housekeeping at any
	property is told so. The property stays in the URL, so every link and a reload keep it.
-->
<script lang="ts">
	import { onMount } from 'svelte';
	import { resolve } from '$app/paths';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { createQuery, useQueryClient } from '@tanstack/svelte-query';
	import { ApiError, errorMessage } from '$lib/api/problem';
	import { connectEvents } from '$lib/events';
	import HkSyncBanner from '$lib/components/HkSyncBanner.svelte';
	import { sync } from '$lib/hk-sync.svelte';
	import { loadSnapshot, type Snapshot } from '$lib/hk-snapshot';
	import { fetchProperties, propertiesKey } from '$lib/properties';
	import { can, fetchMe } from '$lib/session';

	const REMEMBERED = 'goodfolk.hk.property';

	let { children } = $props();

	const client = useQueryClient();
	sync.attach(client);
	const me = createQuery(() => ({ queryKey: ['me'], queryFn: fetchMe }));
	const properties = createQuery(() => ({
		queryKey: propertiesKey,
		queryFn: ({ signal }) => fetchProperties(signal),
		enabled: !!me.data?.current_tenant
	}));

	$effect(() => {
		if (me.error instanceof ApiError && me.error.status === 401) void goto(resolve('/login'));
	});

	const eligible = $derived(
		me.data ? (properties.data ?? []).filter((p) => can(me.data!, 'housekeepingView', p.id)) : []
	);

	function remembered(): string | null {
		try {
			return localStorage.getItem(REMEMBERED);
		} catch {
			return null;
		}
	}

	const requested = $derived(page.url.searchParams.get('property'));
	const chosen = $derived(
		eligible.find((p) => p.id === requested) ??
			(eligible.length === 1 ? eligible[0] : undefined) ??
			(requested === null ? eligible.find((p) => p.id === remembered()) : undefined)
	);

	// Keep the choice in the URL, and remember it for the next visit.
	$effect(() => {
		if (!chosen) return;
		try {
			localStorage.setItem(REMEMBERED, chosen.id);
		} catch {
			// Remembering is a convenience only.
		}
		if (requested !== chosen.id) {
			const url = new URL(page.url);
			url.searchParams.set('property', chosen.id);
			void goto(here(url.searchParams.toString()), { replaceState: true });
		}
	});

	// Without a connection (or an API that cannot be reached) the shell comes from the last saved list.
	const unreachable = $derived(
		[me.error, properties.error].some((error) => !!error && !(error instanceof ApiError))
	);
	const offline = $derived(!sync.online || unreachable);
	// `null` once looked for and not found.
	let snapshot = $state<Snapshot | null>();
	$effect(() => {
		if (!offline || me.data) return;
		void loadSnapshot(requested ?? remembered()).then((saved) => (snapshot = saved ?? null));
	});
	$effect(() => {
		if (offline && !me.data && snapshot && requested !== snapshot.propertyId) {
			const url = new URL(page.url);
			url.searchParams.set('property', snapshot.propertyId);
			void goto(here(url.searchParams.toString()), { replaceState: true });
		}
	});

	// Waiting changes are sent when the connection returns and when the app is opened or focused again.
	onMount(() => {
		sync.online = navigator.onLine;
		void sync.replay();
		const goOnline = () => {
			sync.online = true;
			void sync.replay();
		};
		const goOffline = () => (sync.online = false);
		const focused = () => {
			if (document.visibilityState === 'visible') void sync.replay();
		};
		window.addEventListener('online', goOnline);
		window.addEventListener('offline', goOffline);
		window.addEventListener('focus', focused);
		document.addEventListener('visibilitychange', focused);
		return () => {
			window.removeEventListener('online', goOnline);
			window.removeEventListener('offline', goOffline);
			window.removeEventListener('focus', focused);
			document.removeEventListener('visibilitychange', focused);
		};
	});

	const signedIn = $derived(!!me.data);
	$effect(() => {
		if (signedIn && chosen) return connectEvents(client, chosen.id);
	});

	const onIssue = $derived(page.url.pathname === resolve('/hk/issue'));
	const here = (search: string) =>
		onIssue ? resolve(`/hk/issue?${search}`) : resolve(`/hk?${search}`);

	function choose(id: string) {
		const url = new URL(page.url);
		url.searchParams.set('property', id);
		void goto(here(url.searchParams.toString()));
	}
</script>

{#snippet shell(id: string, name: string)}
	<header>
		<strong>{name}</strong>
		<nav aria-label="Housekeeping">
			<a href={resolve(`/hk?property=${id}`)} aria-current={onIssue ? undefined : 'page'}
				>My rooms</a
			>
			<a href={resolve(`/hk/issue?property=${id}`)} aria-current={onIssue ? 'page' : undefined}
				>Report issue</a
			>
		</nav>
	</header>
	<main>
		<HkSyncBanner propertyId={id} />
		{@render children()}
	</main>
{/snippet}

<div class="shell">
	{#if me.data && properties.data}
		{#if chosen}
			{@render shell(chosen.id, chosen.name)}
		{:else if eligible.length > 1}
			<main>
				<h1>Choose a property</h1>
				<ul class="choices" aria-label="Properties">
					{#each eligible as property (property.id)}
						<li>
							<button type="button" onclick={() => choose(property.id)}>
								{property.name} ({property.code})
							</button>
						</li>
					{/each}
				</ul>
			</main>
		{:else}
			<main>
				<h1>No housekeeping access</h1>
				<p role="alert">Your account has no housekeeping role at any property.</p>
			</main>
		{/if}
	{:else if offline && snapshot}
		{@render shell(snapshot.propertyId, snapshot.propertyName)}
	{:else if offline && snapshot === null}
		<main>
			<h1>You are offline</h1>
			<p role="alert">
				There is no saved list on this device yet. Open My rooms once while connected.
			</p>
		</main>
	{:else if me.error && !(me.error instanceof ApiError && me.error.status === 401)}
		<main>
			<p class="error" role="alert">{errorMessage(me.error)}</p>
			<button onclick={() => me.refetch()}>Retry</button>
		</main>
	{:else if properties.error}
		<main>
			<p class="error" role="alert">{errorMessage(properties.error)}</p>
			<button onclick={() => properties.refetch()}>Retry</button>
		</main>
	{:else}
		<!-- Also shown for a 401 while the redirect to sign-in happens. -->
		<main><p>Loading…</p></main>
	{/if}
</div>

<style>
	.shell {
		max-width: 480px;
		margin: 0 auto;
		min-height: 100dvh;
	}
	header {
		display: grid;
		gap: 0.25rem;
		padding: 0.5rem 1rem 0;
		border-bottom: 1px solid var(--border);
		background: var(--surface);
	}
	nav {
		display: grid;
		grid-template-columns: 1fr 1fr;
	}
	nav a {
		display: flex;
		align-items: center;
		justify-content: center;
		min-height: 48px;
		color: var(--muted);
		text-decoration: none;
		border-bottom: 3px solid transparent;
	}
	nav a[aria-current='page'] {
		color: var(--text);
		font-weight: 600;
		border-bottom-color: var(--accent);
	}
	main {
		padding: 1rem;
	}
	.choices {
		display: grid;
		gap: var(--space);
		padding: 0;
		list-style: none;
	}
	.choices button {
		width: 100%;
		min-height: 48px;
	}
</style>
