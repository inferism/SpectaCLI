import { describe, expect, it } from "vitest";
import { fmtBytes, fmtCpu } from "./format";

describe("format", () => {
  it("formats bytes", () => {
    expect(fmtBytes(412 * 1024 * 1024)).toBe("412 MB");
    expect(fmtBytes(1.2 * 1024 ** 3)).toBe("1.2 GB");
  });
  it("formats cpu", () => {
    expect(fmtCpu(3.44)).toBe("3.4%");
    expect(fmtCpu(42.2)).toBe("42%");
  });
});
