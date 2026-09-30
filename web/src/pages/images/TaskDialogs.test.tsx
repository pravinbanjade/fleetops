import { act, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { buildImage } from "@/api/gen/sdk.gen";
import { json, mockFetch } from "@/test/fetch";
import { FakeWebSocket } from "@/test/fake-ws";
import { loggedIn, renderApp } from "@/test/app";
import { BUILD_UPLOAD_MAX } from "./TaskDialogs";

// jsdom 30's File cannot go through Node's Request (vitest's bridge between
// the two no longer finds jsdom's Blob bytes), so build uploads are checked
// at the generated client: the fields it gets, not the multipart encoding.
vi.mock("@/api/gen/sdk.gen", async (actual) => ({
  ...(await actual<typeof import("@/api/gen/sdk.gen")>()),
  buildImage: vi.fn(),
}));

const routes = { ...loggedIn, "GET /api/v1/envs/local/images": { json: [] } };

/** The socket, opened, and the task subscription the page made for `id`. */
async function taskSub(id: string) {
  const sock = FakeWebSocket.latest();
  if (sock.readyState !== FakeWebSocket.OPEN) act(() => sock.open());
  let sub = 0;
  await waitFor(() => {
    sub = sock.frames().find((f) => f.topic === "task" && f.id === id)?.sub;
    expect(sub).toBeTruthy();
  });
  return { sock, sub };
}

describe("pull", () => {
  it("pulls an image with layer progress and reports the result", async () => {
    const calls = mockFetch({ ...routes, "POST /api/v1/envs/local/images/pull": { status: 202, json: { task_id: "t1" } } });
    const { screen } = renderApp("/envs/local/images");
    await userEvent.click(await screen.findByRole("button", { name: "Pull" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText("Image"), "nginx:1.27");
    await userEvent.click(within(dialog).getByRole("button", { name: "Pull" }));
    const { sock, sub } = await taskSub("t1");
    expect(calls.find((c) => c.method === "POST")?.body).toEqual({ image: "nginx:1.27" });
    act(() => {
      sock.receive({ sub, type: "data", data: { status: "Pulling from library/nginx", id: "1.27" } });
      sock.receive({
        sub,
        type: "data",
        data: { status: "Downloading", id: "aaaaaaaaaaaa", progressDetail: { current: 2_000_000, total: 4_000_000 } },
      });
    });
    expect(await within(dialog).findByText("Downloading 2.0 MB / 4.0 MB")).toBeInTheDocument();
    expect(within(dialog).getByLabelText("Layer aaaaaaaaaaaa")).toHaveAttribute("aria-valuenow", "50");
    expect(within(dialog).getByRole("status")).toHaveTextContent("Running");
    const before = calls.filter((c) => c.path === "/api/v1/envs/local/images").length;
    act(() => sock.receive({ sub, type: "end", reason: "succeeded", task: { status: "succeeded" } }));
    expect(await within(dialog).findByText("Done.")).toBeInTheDocument();
    expect(await screen.findByText("Pull nginx:1.27: done")).toBeInTheDocument();
    await waitFor(() => expect(calls.filter((c) => c.path === "/api/v1/envs/local/images").length).toBeGreaterThan(before));
  });

  it("still reports the result after the dialog is closed", async () => {
    mockFetch({ ...routes, "POST /api/v1/envs/local/images/pull": { status: 202, json: { task_id: "t2" } } });
    const { screen } = renderApp("/envs/local/images");
    await userEvent.click(await screen.findByRole("button", { name: "Pull" }));
    await userEvent.type(within(await screen.findByRole("dialog")).getByLabelText("Image"), "nope:1");
    await userEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Pull" }));
    const { sock, sub } = await taskSub("t2");
    // The footer's Close, not the dialog's corner one.
    await userEvent.click(within(screen.getByRole("dialog")).getAllByRole("button", { name: "Close" })[0]);
    act(() =>
      sock.receive({ sub, type: "end", reason: "failed", task: { status: "failed", error: "manifest unknown" } }),
    );
    expect(await screen.findByText("Pull nope:1 failed: manifest unknown")).toBeInTheDocument();
  });

  it("shows a refused pull in the dialog", async () => {
    mockFetch({
      ...routes,
      "POST /api/v1/envs/local/images/pull": () => json(409, { code: "conflict", message: "8 tasks are already running" }),
    });
    const { screen } = renderApp("/envs/local/images");
    await userEvent.click(await screen.findByRole("button", { name: "Pull" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText("Image"), "nginx");
    await userEvent.click(within(dialog).getByRole("button", { name: "Pull" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("8 tasks are already running");
  });
});

describe("build", () => {
  const started = () => {
    vi.mocked(buildImage).mockResolvedValue({ data: { task_id: "b1" } } as Awaited<ReturnType<typeof buildImage>>);
    return vi.mocked(buildImage);
  };

  it("uploads a Dockerfile and streams the build log", async () => {
    mockFetch(routes);
    const build = started();
    const { screen } = renderApp("/envs/local/images");
    await userEvent.click(await screen.findByRole("button", { name: "Build" }));
    const dialog = await screen.findByRole("dialog");
    const file = new File(["FROM alpine\n"], "Dockerfile", { type: "text/plain" });
    await userEvent.upload(within(dialog).getByLabelText("Dockerfile or context"), file);
    expect(within(dialog).queryByLabelText("Dockerfile path in the context")).not.toBeInTheDocument();
    await userEvent.type(within(dialog).getByLabelText("Tag"), "app:1");
    await userEvent.click(within(dialog).getByRole("button", { name: "Build" }));
    const { sock, sub } = await taskSub("b1");
    expect(build.mock.calls[0][0]).toMatchObject({ path: { env: "local" }, body: { dockerfile: file, tag: "app:1" } });
    act(() => sock.receive({ sub, type: "data", data: { stream: "Step 1/1 : FROM alpine\n" } }));
    expect(await within(dialog).findByLabelText("Output")).toHaveTextContent("Step 1/1 : FROM alpine");
  });

  it("sends an archive as the context, with its Dockerfile path", async () => {
    mockFetch(routes);
    const build = started();
    const { screen } = renderApp("/envs/local/images");
    await userEvent.click(await screen.findByRole("button", { name: "Build" }));
    const dialog = await screen.findByRole("dialog");
    const file = new File(["tar"], "ctx.tar.gz");
    await userEvent.upload(within(dialog).getByLabelText("Dockerfile or context"), file);
    await userEvent.type(within(dialog).getByLabelText("Dockerfile path in the context"), "docker/Dockerfile");
    await userEvent.click(within(dialog).getByRole("button", { name: "Build" }));
    await waitFor(() => expect(build).toHaveBeenCalled());
    expect(build.mock.calls[0][0].body).toEqual({ context: file, tag: null, dockerfile_path: "docker/Dockerfile" });
  });

  it("refuses a file over the upload limit before sending it", async () => {
    mockFetch(routes);
    const build = started();
    const { screen } = renderApp("/envs/local/images");
    await userEvent.click(await screen.findByRole("button", { name: "Build" }));
    const dialog = await screen.findByRole("dialog");
    const big = new File(["x"], "ctx.tar");
    Object.defineProperty(big, "size", { value: BUILD_UPLOAD_MAX + 1 });
    await userEvent.upload(within(dialog).getByLabelText("Dockerfile or context"), big);
    await userEvent.click(within(dialog).getByRole("button", { name: "Build" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("the limit is 104.9 MB");
    expect(build).not.toHaveBeenCalled();
  });
});
