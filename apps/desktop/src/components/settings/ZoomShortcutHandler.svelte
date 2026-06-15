<script lang="ts">
	import { SHORTCUT_SERVICE } from "$lib/shortcuts/shortcutService";
	import { UI_STATE } from "$lib/state/uiState.svelte";
	import { DEFAULT_ZOOM, ZOOM_STEP, applyDomZoom, setZoom } from "$lib/zoom";
	import { inject } from "@gitbutler/core/context";
	import { mergeUnlisten } from "@gitbutler/ui/utils/mergeUnlisten";
	import { onMount } from "svelte";

	const uiState = inject(UI_STATE);
	const shortcutService = inject(SHORTCUT_SERVICE);
	const zoom = uiState.global.zoom;

	function updateZoom(newZoom: number) {
		setZoom(zoom, newZoom);
	}

	$effect(() =>
		mergeUnlisten(
			shortcutService.on("zoom-in", () => {
				updateZoom(zoom.current + ZOOM_STEP);
			}),
			shortcutService.on("zoom-out", () => {
				updateZoom(zoom.current - ZOOM_STEP);
			}),
			shortcutService.on("zoom-reset", () => {
				updateZoom(DEFAULT_ZOOM);
			}),
		),
	);

	onMount(() => {
		const currentZoom = zoom.current;
		if (currentZoom !== DEFAULT_ZOOM) {
			applyDomZoom(currentZoom);
		}
	});
</script>
