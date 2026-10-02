<!--
	One reservation, in a modal over the reservations table (the layout stays mounted beneath it). Closing it
	(Escape, the Close button or a click on the backdrop) goes back to the list with its filters: Back when
	the list is the previous entry, otherwise to the list URL, as after a deep link. The modal itself is
	`ReservationModal`, which the tape chart also opens.
-->
<script lang="ts">
	import { afterNavigate, goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import ReservationModal from '$lib/components/ReservationModal.svelte';

	const LIST_ROUTE = '/(app)/p/[property]/reservations/(list)';
	const DETAIL_ROUTE = '/(app)/p/[property]/reservations/(list)/[id]';

	const propertyId = $derived(page.params.property ?? '');
	const id = $derived(page.params.id ?? '');

	/** Whether the previous history entry is the list, so closing can go Back to it. */
	let fromList = false;

	afterNavigate(({ from }) => {
		fromList = from?.route.id === LIST_ROUTE && from.params?.property === propertyId;
	});

	function closed() {
		// Already leaving (Back, or a link elsewhere): the navigation under way decides where to.
		if (page.route.id !== DETAIL_ROUTE) return;
		if (fromList) {
			history.back();
		} else {
			void goto(resolve(`/p/${propertyId}/reservations${page.url.search}`), {
				replaceState: true,
				noScroll: true
			});
		}
	}
</script>

<ReservationModal {propertyId} {id} onclose={closed} />
