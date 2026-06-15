/**
 * Shared logic for the global interface zoom.
 *
 * The zoom level is a multiplier applied to the document root's font size
 * (`1` = 100%). It is persisted in `uiState.global.zoom` and applied both via
 * keyboard shortcuts (see `ZoomShortcutHandler`) and the appearance settings.
 */

export const MIN_ZOOM = 0.375;
export const MAX_ZOOM = 3;
export const DEFAULT_ZOOM = 1;
export const ZOOM_STEP = 0.0625;

/** Clamp a zoom multiplier to the supported range. */
export function clampZoom(zoom: number): number {
	return Math.min(Math.max(zoom, MIN_ZOOM), MAX_ZOOM);
}

/** Apply a zoom multiplier to the document root as a rem-based font size. */
export function applyDomZoom(zoom: number): void {
	document.documentElement.style.fontSize = zoom + "rem";
}

/** Convert a zoom multiplier (`1` = 100%) to a rounded whole percentage. */
export function zoomToPercent(zoom: number): number {
	return Math.round(zoom * 100);
}

/** Convert a whole percentage to a zoom multiplier. */
export function percentToZoom(percent: number): number {
	return percent / 100;
}

/**
 * Clamp `rawZoom`, apply it to the document root, and persist it through
 * `store`. Returns the clamped value. Shared by the keyboard shortcuts and the
 * appearance setting so both keep the DOM and persisted state in lockstep.
 */
export function setZoom(store: { set: (value: number) => void }, rawZoom: number): number {
	const clamped = clampZoom(rawZoom);
	applyDomZoom(clamped);
	store.set(clamped);
	return clamped;
}
