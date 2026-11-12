import { describe, expect, it } from "vitest";
import { shouldToast, TOAST_DEBOUNCE_MS } from "./toast";

const base = { windowFocused: false, paneShown: false, lastToastAt: undefined, now: 100_000 };

describe("shouldToast", () => {
  it("toasts when the window is unfocused or the pane is hidden", () => {
    expect(shouldToast(base)).toBe(true);
    expect(shouldToast({ ...base, windowFocused: true })).toBe(true);
    expect(shouldToast({ ...base, paneShown: true })).toBe(true);
  });
  it("stays quiet while the user is looking at the pane", () => {
    expect(shouldToast({ ...base, windowFocused: true, paneShown: true })).toBe(false);
  });
  it("debounces per agent", () => {
    expect(shouldToast({ ...base, lastToastAt: base.now - TOAST_DEBOUNCE_MS + 1 })).toBe(false);
    expect(shouldToast({ ...base, lastToastAt: base.now - TOAST_DEBOUNCE_MS })).toBe(true);
  });
});
