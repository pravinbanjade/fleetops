import { describe, expect, it } from "vitest";
import { buildLog, pullProgress } from "./progress";

describe("pullProgress", () => {
  it("keeps each layer's latest status and bytes, and the other lines in order", () => {
    const p = pullProgress([
      { status: "Pulling from library/nginx", id: "1.27" },
      { status: "Pulling fs layer", id: "aaaaaaaaaaaa" },
      { status: "Pulling fs layer", id: "bbbbbbbbbbbb" },
      { status: "Downloading", id: "aaaaaaaaaaaa", progressDetail: { current: 500, total: 1000 } },
      { status: "Downloading", id: "aaaaaaaaaaaa", progressDetail: { current: 1000, total: 1000 } },
      { status: "Download complete", id: "aaaaaaaaaaaa", progressDetail: {} },
      { status: "Digest: sha256:123" },
      { status: "Status: Downloaded newer image for nginx:1.27" },
    ]);
    expect(p.layers).toEqual([
      { id: "aaaaaaaaaaaa", status: "Download complete", current: 1000, total: 1000 },
      { id: "bbbbbbbbbbbb", status: "Pulling fs layer", current: undefined, total: undefined },
    ]);
    expect(p.lines).toEqual([
      "Pulling from library/nginx 1.27",
      "Digest: sha256:123",
      "Status: Downloaded newer image for nginx:1.27",
    ]);
  });

  it("includes Docker's error", () => {
    expect(pullProgress([{ error: "manifest unknown", errorDetail: { message: "manifest unknown" } }]).lines).toEqual([
      "manifest unknown",
    ]);
  });
});

describe("buildLog", () => {
  it("splits stream output into lines and notes the result", () => {
    expect(
      buildLog([
        { stream: "Step 1/2 : FROM alpine\n" },
        { stream: "\n" },
        { stream: " ---> abc\nStep 2/2 : RUN true\n" },
        { aux: { ID: "sha256:def" } },
        { error: "The command '/bin/sh -c false' returned a non-zero code: 1" },
      ]),
    ).toEqual([
      "Step 1/2 : FROM alpine",
      " ---> abc",
      "Step 2/2 : RUN true",
      "Built sha256:def",
      "The command '/bin/sh -c false' returned a non-zero code: 1",
    ]);
  });
});
