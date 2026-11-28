import { describe, expect, it } from "vitest";
import { isNewer } from "./update";

describe("comparar versões", () => {
  it("compara número por número, não como texto", () => {
    expect(isNewer("0.10.0", "0.9.1")).toBe(true);
    expect(isNewer("0.8.0", "0.8.0")).toBe(false);
    expect(isNewer("0.7.0", "0.8.0")).toBe(false);
    expect(isNewer("v1.0.0", "0.99.9")).toBe(true);
    expect(isNewer("0.8.1", "0.8")).toBe(true);
  });
});
