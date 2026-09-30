import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { Button } from "@/components/ui/button";
import { columns, DataTable } from "./DataTable";

interface Box {
  id: string;
  name: string;
  size: number;
  color: string;
}

const col = columns<Box>();
const COLS = [
  col.accessor("name", { header: "Name" }),
  col.accessor("size", { header: "Size" }),
  col.accessor("color", { header: "Color" }),
];
const BOXES: Box[] = [
  { id: "b1", name: "beta", size: 2, color: "red" },
  { id: "b2", name: "alpha", size: 10, color: "blue" },
  { id: "b3", name: "gamma", size: 1, color: "red" },
];

const names = () =>
  within(screen.getByRole("table", { name: "Boxes" }))
    .getAllByRole("row")
    .slice(1)
    .map((r) => within(r).getAllByRole("cell").at(-3)?.textContent);

describe("DataTable", () => {
  it("filters rows by any column", async () => {
    render(<DataTable label="Boxes" data={BOXES} columns={COLS} getRowId={(b) => b.id} empty="No boxes" />);
    await userEvent.type(screen.getByLabelText("Filter boxes"), "red");
    expect(names()).toEqual(["beta", "gamma"]);
    await userEvent.clear(screen.getByLabelText("Filter boxes"));
    await userEvent.type(screen.getByLabelText("Filter boxes"), "zzz");
    expect(screen.getByText("Nothing matches the filter.")).toBeInTheDocument();
  });

  it("sorts by a column when its header is clicked", async () => {
    render(<DataTable label="Boxes" data={BOXES} columns={COLS} getRowId={(b) => b.id} empty="No boxes" />);
    await userEvent.click(screen.getByRole("button", { name: "Size" }));
    const sizes = () =>
      within(screen.getByRole("table")).getAllByRole("row").slice(1).map((r) => within(r).getAllByRole("cell")[1].textContent);
    const first = sizes();
    expect([first, [...first].reverse()]).toContainEqual(["1", "2", "10"]);
    await userEvent.click(screen.getByRole("button", { name: /Size/ }));
    expect(sizes()).toEqual([...first].reverse());
    expect(screen.getByRole("columnheader", { name: /Size/ })).toHaveAttribute("aria-sort");
  });

  it("hides a column from the columns menu", async () => {
    render(
      <DataTable label="Boxes" data={BOXES} columns={COLS} getRowId={(b) => b.id} empty="No boxes" initialHidden={["color"]} />,
    );
    expect(screen.queryByRole("columnheader", { name: "Color" })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Columns" }));
    await userEvent.click(await screen.findByRole("menuitemcheckbox", { name: "Color" }));
    await userEvent.keyboard("{Escape}");
    expect(screen.getByRole("columnheader", { name: "Color" })).toBeInTheDocument();
  });

  it("passes the selected rows to the bulk actions", async () => {
    const got: string[][] = [];
    render(
      <DataTable
        label="Boxes"
        data={BOXES}
        columns={COLS}
        getRowId={(b) => b.id}
        empty="No boxes"
        selectable
        actions={(sel, clear) => (
          <Button
            onClick={() => {
              got.push(sel.map((b) => b.name));
              clear();
            }}
          >
            Paint
          </Button>
        )}
      />,
    );
    expect(screen.queryByRole("button", { name: "Paint" })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("checkbox", { name: "Select b1" }));
    await userEvent.click(screen.getByRole("checkbox", { name: "Select b3" }));
    expect(screen.getByText("2 selected")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Paint" }));
    expect(got).toEqual([["beta", "gamma"]]);
    expect(screen.queryByText("2 selected")).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("checkbox", { name: "Select all" }));
    expect(screen.getByText("3 selected")).toBeInTheDocument();
  });

  it("forgets the selection of rows that are gone", async () => {
    const props = { label: "Boxes", columns: COLS, getRowId: (b: Box) => b.id, empty: "No boxes", selectable: true };
    const { rerender } = render(<DataTable {...props} data={BOXES} actions={() => <Button>Paint</Button>} />);
    await userEvent.click(screen.getByRole("checkbox", { name: "Select b1" }));
    rerender(<DataTable {...props} data={BOXES.slice(1)} actions={() => <Button>Paint</Button>} />);
    expect(screen.queryByRole("button", { name: "Paint" })).not.toBeInTheDocument();
  });

  it("shows loading, empty and error states", () => {
    const props = { label: "Boxes", columns: COLS, getRowId: (b: Box) => b.id, empty: "No boxes yet" };
    const { rerender } = render(<DataTable {...props} data={undefined} />);
    expect(screen.getByText("Loading…")).toBeInTheDocument();
    rerender(<DataTable {...props} data={[]} />);
    expect(screen.getByText("No boxes yet")).toBeInTheDocument();
    rerender(<DataTable {...props} data={undefined} error={new Error("engine gone")} />);
    expect(screen.getByRole("alert")).toHaveTextContent("engine gone");
  });
});
