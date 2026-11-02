import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { ENVS, loggedIn, renderApp } from "@/test/app";
import { container, inspect } from "@/test/docker";
import { mockFetch } from "@/test/fetch";
import { coversWindow, restartsWithin } from "./ContainerNow";

const WEB = container({ id: "c1", name: "web", state: "running", status: "Up 6 days" });
const routes = {
  ...loggedIn,
  "GET /api/v1/envs/local/containers": { json: [WEB] },
  "GET /api/v1/envs/local/containers/c1": { json: inspect({ Id: "c1", Name: "/web" }) },
};

async function openPanel() {
  renderApp("/envs/local/containers");
  const user = userEvent.setup();
  await user.click(await screen.findByRole("cell", { name: "Up 6 days" }));
  return screen.findByRole("complementary", { name: "Selected" });
}

describe("container panel", () => {
  it("gives a viewer only Open", async () => {
    mockFetch({ ...routes, "GET /api/v1/envs": { json: ENVS.map((e) => ({ ...e, access: { role: "viewer", namespaces: null } })) } });
    const panel = await openPanel();
    expect(await within(panel).findByRole("link", { name: "Open" })).toBeInTheDocument();
    expect(within(panel).queryByRole("button", { name: "Restart" })).toBeNull();
    expect(within(panel).queryByRole("button", { name: "Stop" })).toBeNull();
    expect(within(panel).queryByRole("link", { name: "Shell" })).toBeNull();
  });

  it("gives an operator Open, Shell, Restart and Stop, each with its icon", async () => {
    mockFetch(routes);
    const panel = await openPanel();
    for (const [role, name] of [["link", "Open"], ["link", "Shell"], ["button", "Restart"], ["button", "Stop"]] as const) {
      const el = await within(panel).findByRole(role, { name });
      expect(el.querySelector("svg.lucide")).not.toBeNull();
    }
  });
});

describe("restartsWithin", () => {
  const die = (time: number, id = "c1") => ({ action: "die", attributes: {}, id, kind: "container", time });
  it("counts the container's deaths in the window and ignores older ones and other containers", () => {
    const now = 1_000;
    expect(restartsWithin([die(990), die(700), die(500), die(300), die(995, "c2")], "c1", now, 600)).toBe(3);
  });
  it("leaves out a die that follows a kill or stop: a restart that was asked for", () => {
    const ev = (action: string, time: number) => ({ action, attributes: {}, id: "c1", kind: "container", time });
    // Newest first: an asked restart at 900 (kill, die, start), then a crash at 700.
    const events = [ev("start", 902), ev("die", 901), ev("kill", 900), ev("start", 701), ev("die", 700)];
    expect(restartsWithin(events, "c1", 1_000, 600)).toBe(1);
  });
});

describe("coversWindow", () => {
  it("is a floor only when the feed is full and its oldest event is inside the window", () => {
    const at = (n: number, from: number) => Array.from({ length: n }, (_, i) => ({ time: from - i }));
    expect(coversWindow(at(10, 1_000), 1_000, 600)).toBe(true);
    expect(coversWindow(at(50, 1_000), 1_000, 600)).toBe(false);
    expect(coversWindow(at(50, 300), 1_000, 600)).toBe(true);
  });
});
