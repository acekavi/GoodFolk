<script lang="ts">
	import '../app.css';
	import { QueryClient, QueryClientProvider } from '@tanstack/svelte-query';
	import { ApiError } from '$lib/api/problem';

	let { children } = $props();

	const queryClient = new QueryClient({
		defaultOptions: {
			queries: {
				staleTime: 30_000,
				// By default a query only re-renders for the fields read on its last render. Templates read
				// fields conditionally (`a.data && b.data` skips `b.data` while `a` loads), so a field first
				// read after it changed would stay stale; notify on every change instead.
				notifyOnChangeProps: 'all',
				// Client errors (401, 403, 404…) will not succeed on retry.
				retry: (count, error) => !(error instanceof ApiError && error.status < 500) && count < 2
			}
		}
	});
</script>

<QueryClientProvider client={queryClient}>
	{@render children()}
</QueryClientProvider>
