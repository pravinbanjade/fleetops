import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { ImageRef } from "./ImageRef";

describe("ImageRef", () => {
  it("shows a tag as is and cuts a digest", () => {
    render(
      <>
        <ImageRef value="ghcr.io/rust-cross/cargo-zigbuild:0.23.4" />
        <ImageRef value={`kindest/node@sha256:${"a1ed56cfb0e7".padEnd(64, "0")}`} />
      </>,
    );
    expect(screen.getByText("ghcr.io/rust-cross/cargo-zigbuild:0.23.4")).toBeInTheDocument();
    expect(screen.getByText("kindest/node@sha256:a1ed56cfb0e7…")).toBeInTheDocument();
  });
});
