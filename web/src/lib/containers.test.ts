import { describe, expect, it } from "vitest";
import { ApiError } from "@/api/client";
import { bulkMessage, runBulk } from "./bulk";
import { formatAgo, shortId } from "./format";
import { portLinks } from "./ports";

describe("portLinks", () => {
  it("links published TCP ports to the UI's host, once per port", () => {
    const links = portLinks(
      [
        { ip: "0.0.0.0", private_port: 80, public_port: 8080, protocol: "tcp" },
        { ip: "::", private_port: 80, public_port: 8080, protocol: "tcp" },
        { private_port: 443, protocol: "tcp" },
      ],
      "docker.lan",
    );
    expect(links).toEqual([{ label: "8080→80", href: "http://docker.lan:8080" }]);
  });

  it("lists exposed ports when none is published", () => {
    expect(portLinks([{ private_port: 443, protocol: "tcp" }, { private_port: 53, protocol: "udp" }], "h")).toEqual([
      { label: "443/tcp" },
      { label: "53/udp" },
    ]);
  });

  it("uses a specific bind address and leaves UDP unlinked", () => {
    expect(
      portLinks(
        [
          { ip: "127.0.0.1", private_port: 5432, public_port: 5432, protocol: "tcp" },
          { ip: "0.0.0.0", private_port: 53, public_port: 5353, protocol: "udp" },
          { ip: "::1", private_port: 6379, public_port: 6379, protocol: "tcp" },
        ],
        "h",
      ),
    ).toEqual([
      { label: "5432→5432", href: "http://127.0.0.1:5432" },
      { label: "5353→53/udp" },
      { label: "6379→6379", href: "http://[::1]:6379" },
    ]);
  });

  it("does not repeat a published port as unpublished", () => {
    expect(
      portLinks(
        [
          { private_port: 80, protocol: "tcp" },
          { ip: "0.0.0.0", private_port: 80, public_port: 8080, protocol: "tcp" },
        ],
        "h",
      ),
    ).toEqual([{ label: "8080→80", href: "http://h:8080" }]);
  });
});

describe("runBulk", () => {
  it("runs every item and names the failures", async () => {
    const r = await runBulk(["web", "db", "cache"], (n) => n, async (n) => {
      if (n === "db") throw new ApiError(409, { code: "conflict", message: "is not running" });
    });
    expect(r.ok).toBe(2);
    expect(bulkMessage("Stopped", "container", r)).toBe("Stopped 2 of 3 containers. db: is not running");
    expect(bulkMessage("Stopped", "container", { ok: 1, failed: [] })).toBe("Stopped 1 container");
  });
});

describe("formatAgo", () => {
  const now = 1_790_000_000_000;
  it("says how long ago", () => {
    expect(formatAgo(now / 1000 - 5, now)).toBe("just now");
    expect(formatAgo(now / 1000 - 60, now)).toBe("1 minute ago");
    expect(formatAgo(now / 1000 - 7200, now)).toBe("2 hours ago");
    expect(formatAgo(now / 1000 - 3 * 86400, now)).toBe("3 days ago");
    expect(formatAgo(now / 1000 - 90 * 86400, now)).toBe(new Date(now - 90 * 86400_000).toLocaleDateString());
  });
});

describe("shortId", () => {
  it("drops the digest prefix and keeps 12 characters", () => {
    expect(shortId("sha256:0123456789abcdef0123")).toBe("0123456789ab");
    expect(shortId("fedcba9876543210")).toBe("fedcba987654");
  });
});
