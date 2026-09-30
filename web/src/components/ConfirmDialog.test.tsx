import { QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { describe, expect, it, vi } from "vitest";
import { createQueryClient } from "@/App";
import { ConfirmDialog } from "./ConfirmDialog";

function Harness({ onConfirm }: { onConfirm: () => Promise<unknown> }) {
  const [open, setOpen] = useState(true);
  return (
    <QueryClientProvider client={createQueryClient()}>
      {open ? "open" : "closed"}
      <ConfirmDialog open={open} onOpenChange={setOpen} title="Remove box?" confirmLabel="Remove" onConfirm={onConfirm} />
    </QueryClientProvider>
  );
}

describe("ConfirmDialog", () => {
  it("runs the action and closes", async () => {
    const onConfirm = vi.fn().mockResolvedValue(undefined);
    render(<Harness onConfirm={onConfirm} />);
    await userEvent.click(screen.getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(screen.getByText("closed")).toBeInTheDocument());
    expect(onConfirm).toHaveBeenCalledOnce();
  });

  it("stays open with the error when the action fails", async () => {
    render(<Harness onConfirm={() => Promise.reject(new Error("box is in use"))} />);
    await userEvent.click(screen.getByRole("button", { name: "Remove" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("box is in use");
    expect(screen.getByText("open")).toBeInTheDocument();
  });

  it("does nothing on cancel", async () => {
    const onConfirm = vi.fn();
    render(<Harness onConfirm={onConfirm} />);
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.getByText("closed")).toBeInTheDocument();
    expect(onConfirm).not.toHaveBeenCalled();
  });
});
