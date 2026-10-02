<!--
	Picks the rooms the tape chart shows: a multi-select combobox (WAI-ARIA combobox with a listbox popup).
	Typing suggests room types, a `101-120` range and rooms; ArrowUp and ArrowDown move through the
	suggestions, Enter adds the highlighted one as a chip, Escape closes the list, and Backspace on an empty
	input removes the last chip. The first suggestion is highlighted as soon as there are any, so typing a
	range and pressing Enter is enough.
-->
<script lang="ts">
	import { suggest, type Chip, type RailRoom } from '$lib/tape';
	import type { RoomType } from '$lib/rooms';

	interface Props {
		rooms: RailRoom[];
		types: RoomType[];
		chips: Chip[];
		onchange: (chips: Chip[]) => void;
	}

	let { rooms, types, chips, onchange }: Props = $props();

	const id = $props.id();
	const listId = `${id}-list`;
	let text = $state('');
	let open = $state(false);
	let highlighted = $state(0);

	function identity(chip: Chip): string {
		switch (chip.kind) {
			case 'type':
				return `t:${chip.roomTypeId}`;
			case 'room':
				return `r:${chip.roomId}`;
			case 'range':
				return `n:${chip.from}-${chip.to}`;
		}
	}

	const suggestions = $derived(
		suggest(text, rooms, types).filter(
			(chip) => !chips.some((picked) => identity(picked) === identity(chip))
		)
	);
	const active = $derived(Math.min(highlighted, suggestions.length - 1));
	const showing = $derived(open && suggestions.length > 0);

	function optionId(index: number): string {
		return `${id}-option-${index}`;
	}

	function add(chip: Chip) {
		onchange([...chips, chip]);
		text = '';
		highlighted = 0;
	}

	function keydown(event: KeyboardEvent) {
		if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
			if (suggestions.length === 0) return;
			event.preventDefault();
			open = true;
			const step = event.key === 'ArrowDown' ? 1 : -1;
			highlighted = (active + step + suggestions.length) % suggestions.length;
		} else if (event.key === 'Enter') {
			if (!showing) return;
			event.preventDefault();
			add(suggestions[active]);
		} else if (event.key === 'Escape') {
			if (!open) return;
			event.preventDefault();
			event.stopPropagation();
			open = false;
		} else if (event.key === 'Backspace' && text === '' && chips.length > 0) {
			onchange(chips.slice(0, -1));
		}
	}
</script>

<div class="picker">
	<ul class="chips" aria-label="Chosen rooms">
		{#each chips as chip, index (identity(chip))}
			<li class="chip">
				{chip.label}
				<button
					type="button"
					class="remove"
					aria-label="Remove {chip.label}"
					onclick={() => onchange(chips.filter((_, at) => at !== index))}>×</button
				>
			</li>
		{/each}
	</ul>
	<input
		type="text"
		role="combobox"
		aria-label="Pick rooms"
		aria-autocomplete="list"
		aria-expanded={showing}
		aria-controls={listId}
		aria-activedescendant={showing ? optionId(active) : undefined}
		placeholder="Room, type or 101-120"
		autocomplete="off"
		bind:value={text}
		oninput={() => {
			open = true;
			highlighted = 0;
		}}
		onfocus={() => (open = true)}
		onblur={() => (open = false)}
		onkeydown={keydown}
	/>
	<ul id={listId} class="options" role="listbox" aria-label="Suggestions" hidden={!showing}>
		{#each suggestions as chip, index (identity(chip))}
			<!-- The input keeps focus; the listbox is driven from it, so options take no key events. -->
			<!-- svelte-ignore a11y_click_events_have_key_events -->
			<li
				id={optionId(index)}
				role="option"
				aria-selected={index === active}
				class:active={index === active}
				onmousedown={(event) => event.preventDefault()}
				onclick={() => add(chip)}
			>
				{chip.label}
			</li>
		{/each}
	</ul>
</div>

<style>
	.picker {
		position: relative;
		display: flex;
		flex-wrap: wrap;
		align-items: center;
		gap: 0.25rem;
	}
	.chips {
		display: contents;
		list-style: none;
		margin: 0;
		padding: 0;
	}
	.chip {
		display: inline-flex;
		align-items: center;
		gap: 0.25rem;
		padding: 0.15rem 0.2rem 0.15rem 0.5rem;
		border: 1px solid var(--border);
		border-radius: 999px;
		background: var(--surface);
	}
	.remove {
		padding: 0 0.4rem;
		border: none;
		border-radius: 999px;
		background: none;
		color: var(--muted);
		line-height: 1.4;
	}
	.options {
		position: absolute;
		top: 100%;
		right: 0;
		z-index: 10;
		min-width: 100%;
		margin: 0.25rem 0 0;
		padding: 0.25rem 0;
		list-style: none;
		border: 1px solid var(--border);
		border-radius: var(--radius);
		background: var(--bg);
		box-shadow: 0 4px 12px rgb(0 0 0 / 0.15);
	}
	.options[hidden] {
		display: none;
	}
	.options li {
		padding: 0.3rem 0.75rem;
		cursor: pointer;
		white-space: nowrap;
	}
	.options li.active {
		background: var(--accent);
		color: #fff;
	}
</style>
