<!--
	A document's `thumb`, fetched when the row is first shown. The fetch is a view of the document: it takes
	a fresh audited link, which is held only by the <img>. Until it loads, and if the link or image fails,
	the fallback (the row's icon) is drawn instead.
-->
<script lang="ts">
	import { onMount, type Snippet } from 'svelte';
	import { documentLink } from '$lib/guest-documents';

	interface Props {
		propertyId: string;
		documentId: string;
		alt: string;
		fallback: Snippet;
	}

	let { propertyId, documentId, alt, fallback }: Props = $props();

	let url = $state<string | null>(null);
	let failed = $state(false);

	onMount(() => {
		let cancelled = false;
		documentLink(propertyId, documentId, 'thumb').then(
			(link) => {
				if (!cancelled) url = link.url;
			},
			() => {
				if (!cancelled) failed = true;
			}
		);
		return () => {
			cancelled = true;
		};
	});
</script>

{#if url && !failed}
	<img src={url} {alt} width="40" height="40" onerror={() => (failed = true)} />
{:else}
	{@render fallback()}
{/if}

<style>
	img {
		flex: none;
		object-fit: cover;
		border-radius: 4px;
		border: 1px solid var(--border);
	}
</style>
