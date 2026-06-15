import {
	clampZoom,
	percentToZoom,
	setZoom,
	zoomToPercent,
	MIN_ZOOM,
	MAX_ZOOM,
	DEFAULT_ZOOM,
} from "$lib/zoom";
import { describe, expect, test, vi } from "vitest";

describe("clampZoom", () => {
	test("leaves in-range values unchanged", () => {
		expect(clampZoom(DEFAULT_ZOOM)).toBe(DEFAULT_ZOOM);
		expect(clampZoom(1.5)).toBe(1.5);
	});

	test("clamps below the minimum", () => {
		expect(clampZoom(0.1)).toBe(MIN_ZOOM);
	});

	test("clamps above the maximum", () => {
		expect(clampZoom(10)).toBe(MAX_ZOOM);
	});

	test("keeps the range boundaries", () => {
		expect(clampZoom(MIN_ZOOM)).toBe(MIN_ZOOM);
		expect(clampZoom(MAX_ZOOM)).toBe(MAX_ZOOM);
	});
});

describe("zoom/percent conversions", () => {
	test("zoomToPercent rounds to a whole percentage", () => {
		expect(zoomToPercent(1)).toBe(100);
		expect(zoomToPercent(0.375)).toBe(38);
		expect(zoomToPercent(1.5)).toBe(150);
	});

	test("percentToZoom inverts a whole percentage", () => {
		expect(percentToZoom(100)).toBe(1);
		expect(percentToZoom(150)).toBe(1.5);
	});

	test("round-trips a whole percentage through a zoom multiplier", () => {
		for (const percent of [zoomToPercent(MIN_ZOOM), 50, 100, 150, zoomToPercent(MAX_ZOOM)]) {
			expect(zoomToPercent(percentToZoom(percent))).toBe(percent);
		}
	});
});

describe("setZoom", () => {
	test("clamps, persists, and returns the clamped value", () => {
		const store = { set: vi.fn() };
		expect(setZoom(store, 1.5)).toBe(1.5);
		expect(store.set).toHaveBeenCalledWith(1.5);
	});

	test("clamps out-of-range input before persisting", () => {
		const store = { set: vi.fn() };
		expect(setZoom(store, 10)).toBe(MAX_ZOOM);
		expect(store.set).toHaveBeenCalledWith(MAX_ZOOM);

		expect(setZoom(store, 0.01)).toBe(MIN_ZOOM);
		expect(store.set).toHaveBeenCalledWith(MIN_ZOOM);
	});
});
