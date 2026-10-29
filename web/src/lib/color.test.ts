import { describe, expect, it } from "vitest";
import { withAlpha } from "./color";

describe("withAlpha", () => {
  it("fades a hex colour with an alpha byte", () => {
    expect(withAlpha("#2fe08a", 0.1)).toBe("#2fe08a1a");
    expect(withAlpha("#ffffff", 0.08)).toBe("#ffffff14");
  });

  it("fades an oklch colour", () => {
    expect(withAlpha("oklch(0.8 0.1 150)", 0.5)).toBe("oklch(0.8 0.1 150 / 0.5)");
  });

  it("leaves a colour it cannot fade as it is", () => {
    expect(withAlpha("red", 0.5)).toBe("red");
  });
});
