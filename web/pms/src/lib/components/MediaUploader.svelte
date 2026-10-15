<!--
	A drop zone with a file picker. It only hands over the chosen files; the caller uploads them and
	shows their progress. The picker suggests the purpose's types; the upload checks them properly.
-->
<script lang="ts">
	import { LIMITS, type Purpose } from '$lib/upload';

	interface Props {
		purpose: Purpose;
		/** What the picker's button says, such as "Add photos". */
		label: string;
		disabled?: boolean;
		/** Opens a phone's camera, as `capture` does on a file input. */
		capture?: 'environment' | 'user';
		onfiles: (files: File[]) => void;
	}

	let { purpose, label, disabled = false, capture, onfiles }: Props = $props();

	let input = $state<HTMLInputElement>();
	let over = $state(false);
	const accept = $derived(LIMITS[purpose].types.join(','));

	function take(files: FileList | null | undefined) {
		if (files && files.length > 0) onfiles([...files]);
	}
</script>

<div
	class="drop"
	class:over
	role="group"
	aria-label={label}
	ondragenter={(event) => {
		if (disabled) return;
		event.preventDefault();
		over = true;
	}}
	ondragover={(event) => {
		if (disabled) return;
		event.preventDefault();
	}}
	ondragleave={(event) => {
		if (!event.currentTarget.contains(event.relatedTarget as Node | null)) over = false;
	}}
	ondrop={(event) => {
		over = false;
		if (disabled) return;
		event.preventDefault();
		take(event.dataTransfer?.files);
	}}
>
	<button type="button" class="secondary" {disabled} onclick={() => input?.click()}>{label}</button>
	<span class="hint">or drop files here</span>
	<input
		bind:this={input}
		class="visually-hidden"
		type="file"
		multiple
		{accept}
		{capture}
		{disabled}
		tabindex="-1"
		aria-hidden="true"
		onchange={(event) => {
			take(event.currentTarget.files);
			// The same file can be chosen again after a failure.
			event.currentTarget.value = '';
		}}
	/>
</div>

<style>
	.drop {
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 0.5rem;
		padding: var(--space);
		border: 1px dashed var(--border);
		border-radius: var(--radius);
	}
	.drop.over {
		border-color: var(--accent);
		background: var(--surface);
	}
</style>
