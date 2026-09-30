import { describe, expect, it } from "vitest";
import { crumbs, formatMode, joinPath } from "./files";

describe("files", () => {
  it("formats permission bits like ls", () => {
    expect(formatMode(0o755, "dir")).toBe("drwxr-xr-x");
    expect(formatMode(0o644, "file")).toBe("-rw-r--r--");
    expect(formatMode(0o777, "symlink")).toBe("lrwxrwxrwx");
  });

  it("joins paths without doubling slashes", () => {
    expect(joinPath("/", "etc")).toBe("/etc");
    expect(joinPath("/etc", "hosts")).toBe("/etc/hosts");
    expect(joinPath("/etc/", "hosts")).toBe("/etc/hosts");
  });

  it("lists breadcrumbs from the root", () => {
    expect(crumbs("/")).toEqual([{ name: "/", path: "/" }]);
    expect(crumbs("/etc/nginx")).toEqual([
      { name: "/", path: "/" },
      { name: "etc", path: "/etc" },
      { name: "nginx", path: "/etc/nginx" },
    ]);
  });
});
