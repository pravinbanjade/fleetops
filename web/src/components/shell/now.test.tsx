import { act, fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { NowHost, useNow, useNowRow } from "./now";

let api: ReturnType<typeof useNow>;
const keep = (a: ReturnType<typeof useNow>) => (api = a);
function Probe({ rows }: { rows?: { id: string }[] }) {
  const now = useNow();
  keep(now);
  const row = useNowRow(rows, (r) => r.id);
  return <output>{now.id ?? "none"}:{row?.id ?? "-"}</output>;
}

describe("now panel host", () => {
  it("closes by itself when the active row is no longer in the list", () => {
    const { rerender } = render(
      <NowHost path="/a">
        <Probe rows={[{ id: "a" }, { id: "b" }]} />
      </NowHost>,
    );
    act(() => api.open("a"));
    expect(screen.getByRole("status")).toHaveTextContent("a:a");
    rerender(
      <NowHost path="/a">
        <Probe rows={[{ id: "b" }]} />
      </NowHost>,
    );
    expect(screen.getByRole("status")).toHaveTextContent("none:-");
  });

  it("keeps the selection while the list is still loading", () => {
    render(
      <NowHost path="/a">
        <Probe rows={undefined} />
      </NowHost>,
    );
    act(() => api.open("a"));
    expect(screen.getByRole("status")).toHaveTextContent("a:-");
  });

  it("closes on Escape", () => {
    render(
      <NowHost path="/a">
        <Probe rows={[{ id: "a" }]} />
      </NowHost>,
    );
    act(() => api.open("a"));
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.getByRole("status")).toHaveTextContent("none:-");
  });

  it("closes when the page changes", () => {
    const { rerender } = render(
      <NowHost path="/a">
        <Probe rows={[{ id: "a" }]} />
      </NowHost>,
    );
    act(() => api.open("a"));
    rerender(
      <NowHost path="/b">
        <Probe rows={[{ id: "a" }]} />
      </NowHost>,
    );
    expect(screen.getByRole("status")).toHaveTextContent("none:-");
  });
});
