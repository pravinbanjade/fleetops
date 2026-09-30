import { describe, expect, it } from "vitest";
import { ApiError } from "@/api/client";
import { errorMessage } from "./errors";
import { formatBytes, formatPercent } from "./format";

describe("formatBytes", () => {
  it("scales by 1000 with one decimal", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(999)).toBe("999 B");
    expect(formatBytes(1500)).toBe("1.5 KB");
    expect(formatBytes(2_340_000_000)).toBe("2.3 GB");
  });

  it("shows a dash for missing or invalid sizes", () => {
    expect(formatBytes(undefined)).toBe("—");
    expect(formatBytes(-1)).toBe("—");
    expect(formatBytes(Number.NaN)).toBe("—");
  });
});

describe("formatPercent", () => {
  it("keeps one decimal below 10% and none above", () => {
    expect(formatPercent(0.04)).toBe("0.0%");
    expect(formatPercent(3.26)).toBe("3.3%");
    expect(formatPercent(12.6)).toBe("13%");
    expect(formatPercent(250)).toBe("250%");
    expect(formatPercent(Number.NaN)).toBe("—");
  });
});

describe("errorMessage", () => {
  it("adds the request id to internal errors only", () => {
    expect(errorMessage(new ApiError(500, { code: "internal", message: "internal error", request_id: "r9" }))).toBe(
      "internal error (request r9)",
    );
    expect(errorMessage(new ApiError(409, { code: "conflict", message: "name in use", request_id: "r9" }))).toBe(
      "name in use",
    );
    expect(errorMessage(new TypeError("Failed to fetch"))).toBe("Failed to fetch");
  });
});
