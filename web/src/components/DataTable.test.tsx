import { fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it } from "vitest";
import { Paintbrush } from "lucide-react";
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

  describe("column widths", () => {
    beforeEach(() => localStorage.clear());
    const table = () => <DataTable label="Boxes" data={BOXES} columns={COLS} getRowId={(b) => b.id} empty="No boxes" />;
    const handle = (name: string) => screen.getByRole("separator", { name: `Resize ${name} column` });

    it("resizes a column from the keyboard and remembers it", async () => {
      const { unmount } = render(table());
      // Nothing is frozen until the operator resizes: the table keeps its natural layout.
      expect(screen.getByRole("table").querySelector("col")).toBeNull();
      handle("Name").focus();
      await userEvent.keyboard("{ArrowRight}{ArrowRight}");
      // jsdom lays nothing out, so the column starts from the 48px floor.
      expect(handle("Name")).toHaveAttribute("aria-valuenow", "80");
      expect(screen.getByRole("table")).toHaveClass("sm:table-fixed");
      expect(JSON.parse(localStorage.getItem("fleetops.columns.Boxes") ?? "{}")).toMatchObject({ name: 80 });
      unmount();
      render(table());
      expect(handle("Name")).toHaveAttribute("aria-valuenow", "80");
    });

    it("resizes a column by dragging its edge, never below the floor", () => {
      render(table());
      fireEvent.pointerDown(handle("Size"), { clientX: 100, pointerId: 1 });
      fireEvent.pointerMove(handle("Size"), { clientX: 160, pointerId: 1 });
      fireEvent.pointerUp(handle("Size"), { clientX: 160, pointerId: 1 });
      expect(handle("Size")).toHaveAttribute("aria-valuenow", "108");
      fireEvent.pointerDown(handle("Size"), { clientX: 160, pointerId: 1 });
      fireEvent.pointerMove(handle("Size"), { clientX: -400, pointerId: 1 });
      fireEvent.pointerUp(handle("Size"), { clientX: -400, pointerId: 1 });
      expect(handle("Size")).toHaveAttribute("aria-valuenow", "48");
    });

    it("dragging an edge does not sort the column", () => {
      render(table());
      fireEvent.pointerDown(handle("Name"), { clientX: 0, pointerId: 1 });
      fireEvent.pointerUp(handle("Name"), { clientX: 30, pointerId: 1 });
      fireEvent.click(handle("Name"));
      expect(screen.getByRole("columnheader", { name: /Name/ })).not.toHaveAttribute("aria-sort");
    });

    it("resets every width from the Columns menu", async () => {
      render(table());
      handle("Color").focus();
      await userEvent.keyboard("{ArrowRight}");
      await userEvent.click(screen.getByRole("button", { name: "Columns" }));
      await userEvent.click(await screen.findByRole("menuitem", { name: "Reset widths" }));
      expect(screen.getByRole("table").querySelector("col")).toBeNull();
      expect(localStorage.getItem("fleetops.columns.Boxes")).toBeNull();
    });
  });

  describe("row menu", () => {
    it("lists each row's own actions behind a menu named for the row", async () => {
      const picked: string[] = [];
      render(
        <DataTable
          label="Boxes"
          data={BOXES}
          columns={COLS}
          getRowId={(b) => b.id}
          empty="No boxes"
          rowMenu={{
            label: (b) => b.name,
            items: (b) =>
              b.color === "red"
                ? [
                    { id: "paint", label: "Paint", icon: Paintbrush, onSelect: () => picked.push(`paint ${b.name}`) },
                    { id: "bin", label: "Bin", icon: Paintbrush, danger: true, onSelect: () => picked.push(`bin ${b.name}`) },
                  ]
                : [],
          }}
        />,
      );
      // A row with nothing to do has no menu.
      expect(screen.queryByRole("button", { name: "Actions for alpha" })).not.toBeInTheDocument();
      await userEvent.click(screen.getByRole("button", { name: "Actions for beta" }));
      const menu = await screen.findByRole("menu");
      expect(within(menu).getAllByRole("menuitem").map((m) => m.textContent)).toEqual(["Paint", "Bin"]);
      await userEvent.click(within(menu).getByRole("menuitem", { name: "Paint" }));
      expect(picked).toEqual(["paint beta"]);
      // Its column is not one to hide, sort or resize.
      expect(screen.queryByRole("separator", { name: /Resize Actions/ })).not.toBeInTheDocument();
    });
  });

  it("activates a row when the row is clicked, but not when its link is", async () => {
    const hits: string[] = [];
    const LINKED = [
      col.accessor("name", { header: "Name", cell: (c) => <a href={`#${c.getValue()}`}>{c.getValue()}</a> }),
      col.accessor("size", { header: "Size" }),
    ];
    render(<DataTable label="Boxes" data={BOXES} columns={LINKED} getRowId={(b) => b.id} empty="No boxes" onActivate={(b) => hits.push(b.id)} />);
    await userEvent.click(screen.getByRole("cell", { name: "10" }));
    expect(hits).toEqual(["b2"]);
    await userEvent.click(screen.getByRole("link", { name: "beta" }));
    expect(hits).toEqual(["b2"]);
  });

  it("marks the active row", () => {
    render(<DataTable label="Boxes" data={BOXES} columns={COLS} getRowId={(b) => b.id} empty="No boxes" activeId="b3" />);
    const active = screen.getByRole("cell", { name: "gamma" }).closest("tr");
    expect(active).toHaveAttribute("data-active");
    expect(screen.getByRole("cell", { name: "beta" }).closest("tr")).not.toHaveAttribute("data-active");
  });
});
