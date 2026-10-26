import { describe, expect, it } from "vitest";
import { diffDigits } from "./tick";

describe("diffDigits", () => {
  it("marks only the digit that changed", () => {
    expect(diffDigits("12.4%", "12.7%")).toEqual([false, false, false, true, false]);
  });

  it("aligns from the right when the value grows a digit", () => {
    expect(diffDigits("9.8%", "10.2%")).toEqual([true, true, false, true, false]);
  });

  it("marks a changed unit as well as the digits", () => {
    expect(diffDigits("999 MiB", "1.00 GiB")).toEqual([true, true, true, true, false, true, false, false]);
  });

  it("marks everything new against nothing", () => {
    expect(diffDigits("", "3")).toEqual([true]);
  });

  it("marks nothing when the text is the same", () => {
    expect(diffDigits("5", "5")).toEqual([false]);
  });
});
