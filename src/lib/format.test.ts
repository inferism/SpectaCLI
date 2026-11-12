import { describe, expect, it } from "vitest";
import { fmtBytes, fmtCpu, fmtUsage } from "./format";

describe("format", () => {
  it("formats bytes", () => {
    expect(fmtBytes(412 * 1024 * 1024)).toBe("412 MB");
    expect(fmtBytes(1.2 * 1024 ** 3)).toBe("1.2 GB");
  });
  it("formats cpu", () => {
    expect(fmtCpu(3.44)).toBe("3.4%");
    expect(fmtCpu(42.2)).toBe("42%");
  });
  it("drops idle cpu from usage", () => {
    const u = { privateBytes: 412 * 1024 * 1024, workingSet: 0, cpu: 0, processes: 1 };
    expect(fmtUsage(u)).toBe("412 MB");
    expect(fmtUsage({ ...u, cpu: 3.44 })).toBe("412 MB · 3.4%");
  });
});
