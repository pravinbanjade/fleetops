import {
  columnFilteringFeature,
  columnVisibilityFeature,
  createColumnHelper,
  createFilteredRowModel,
  createSortedRowModel,
  filterFn_includesString,
  globalFilteringFeature,
  rowSelectionFeature,
  rowSortingFeature,
  sortFn_alphanumeric,
  sortFn_basic,
  sortFn_text,
  tableFeatures,
  useTable,
  type ColumnDef,
  type RowData,
  type RowSelectionState,
  type SortingState,
  type ColumnVisibilityState,
} from "@tanstack/react-table";
import { ArrowDown, ArrowUp, ArrowUpDown, Columns3, Search } from "lucide-react";
import { useEffect, useRef, useState, type CSSProperties, type KeyboardEvent, type PointerEvent, type ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { errorMessage } from "@/lib/errors";
import { cn } from "@/lib/utils";
import { MIN_WIDTH, NEW_COLUMN_WIDTH, useColumnWidths, type Widths } from "./columnWidths";

export const features = tableFeatures({
  rowSortingFeature,
  sortedRowModel: createSortedRowModel(),
  sortFns: { alphanumeric: sortFn_alphanumeric, text: sortFn_text, basic: sortFn_basic },
  columnFilteringFeature,
  globalFilteringFeature,
  filteredRowModel: createFilteredRowModel(),
  filterFns: { includesString: filterFn_includesString },
  columnVisibilityFeature,
  rowSelectionFeature,
});

export type Features = typeof features;
export type Column<T extends RowData> = ColumnDef<Features, T, any>; // eslint-disable-line @typescript-eslint/no-explicit-any

/** Column definitions for a row type: `columns<Container>()`. */
export const columns = <T extends RowData>() => createColumnHelper<Features, T>();

interface DataTableProps<T extends RowData> {
  /** Accessible name of the table, e.g. "Containers". */
  label: string;
  data: T[] | undefined;
  columns: Column<T>[];
  getRowId: (row: T) => string;
  /** The list query's error: shown in place of the rows. */
  error?: unknown;
  /** Text when there are no rows at all. */
  empty: string;
  /** Adds a checkbox column; `actions` gets the selected rows. */
  selectable?: boolean;
  actions?: (selected: T[], clear: () => void) => ReactNode;
  /** Buttons beside the filter that do not depend on the selection. */
  toolbar?: ReactNode;
  initialSorting?: SortingState;
  /** Columns hidden until the user shows them. */
  initialHidden?: string[];
  /** The filter text the list starts with. */
  initialFilter?: string;
  /**
   * Racks rows into labelled bays while the table is sorted by `column`
   * (first, ascending): each run of rows with the same bay gets its own
   * row group, headed by the bay's label.
   */
  bays?: { column: string; of: (row: T) => { id: string; label: string } };
  /** The columns kept on a phone; the rest hide below 640px. */
  phoneColumns?: string[];
}

const EMPTY: never[] = [];

/** One keypress on a column edge: 16px, or 64px with Shift. */
const STEP = 16;

const clampWidth = (w: number) => Math.max(MIN_WIDTH, Math.ceil(w));

/**
 * A resource list: text filter over every column, sorting on header click,
 * column visibility, resizable columns, and optional row selection for bulk
 * actions.
 */
export function DataTable<T extends RowData>(props: DataTableProps<T>) {
  const [sorting, setSorting] = useState<SortingState>(props.initialSorting ?? []);
  const [globalFilter, setGlobalFilter] = useState(props.initialFilter ?? "");
  const [columnVisibility, setColumnVisibility] = useState<ColumnVisibilityState>(() =>
    Object.fromEntries((props.initialHidden ?? []).map((id) => [id, false])),
  );
  const [rowSelection, setRowSelection] = useState<RowSelectionState>({});
  const [widths, setWidths] = useColumnWidths(props.label);
  const tableRef = useRef<HTMLTableElement>(null);
  /** Ends the drag in progress: set while the pointer is down on a column edge. */
  const endDrag = useRef<(() => void) | null>(null);
  const [dragging, setDragging] = useState<string | null>(null);
  useEffect(() => () => endDrag.current?.(), []);
  const data = props.data ?? (EMPTY as T[]);
  const table = useTable({
    features,
    data,
    columns: props.columns,
    getRowId: props.getRowId,
    state: { sorting, globalFilter, columnVisibility, rowSelection },
    onSortingChange: setSorting,
    onGlobalFilterChange: setGlobalFilter,
    onColumnVisibilityChange: setColumnVisibility,
    onRowSelectionChange: setRowSelection,
    globalFilterFn: "includesString",
    enableRowSelection: !!props.selectable,
  });
  const rows = table.getRowModel().rows;
  // Rows that disappeared (removed by a bulk action) are no longer selected.
  const selected = rows.filter((r) => r.getIsSelected()).map((r) => r.original);
  const clear = () => setRowSelection({});
  const hideable = table.getAllLeafColumns().filter((c) => c.getCanHide());
  const visible = table.getVisibleLeafColumns();
  // A frozen table adds a spacer column after the last, so each column stays
  // exactly as wide as it was dragged.
  const width = visible.length + (props.selectable ? 1 : 0) + (widths ? 1 : 0);
  const headerOf = (id: string) => {
    const h = visible.find((c) => c.id === id)?.columnDef.header;
    return typeof h === "string" ? h : id;
  };

  /** Every visible column at the width it is drawn now: what the first resize freezes. */
  const measured = (): Widths => {
    const out: Widths = {};
    for (const c of visible) {
      const th = tableRef.current?.querySelector<HTMLElement>(`th[data-col="${CSS.escape(c.id)}"]`);
      out[c.id] = clampWidth(widths?.[c.id] ?? th?.getBoundingClientRect().width ?? 0);
    }
    return out;
  };
  const frozen = (): Widths => ({ ...measured(), ...widths });
  const resize = (id: string, w: number, from: Widths = frozen()) => setWidths({ ...from, [id]: clampWidth(w) });
  /** Fits a column to its widest cell, header included. */
  const fit = (id: string) => {
    const cells = tableRef.current?.querySelectorAll<HTMLElement>(`[data-col="${CSS.escape(id)}"]`) ?? [];
    // scrollWidth rounds down and leaves out the cell's left rule.
    const widest = Math.max(0, ...Array.from(cells, (el) => el.scrollWidth));
    resize(id, widest + 2);
  };
  const edge = (id: string) => ({
    // The drag follows the window, not the handle: the pointer leaves the
    // handle at once, and capture can be lost while the rows update live.
    onPointerDown: (e: PointerEvent<HTMLElement>) => {
      if (e.button !== 0) return;
      e.preventDefault();
      e.stopPropagation();
      endDrag.current?.();
      const from = frozen();
      const x = e.clientX;
      const root = document.documentElement;
      const move = (ev: globalThis.PointerEvent) => setWidths({ ...from, [id]: clampWidth(from[id] + ev.clientX - x) });
      const end = () => {
        window.removeEventListener("pointermove", move);
        window.removeEventListener("pointerup", end);
        window.removeEventListener("pointercancel", end);
        root.style.removeProperty("cursor");
        root.style.removeProperty("user-select");
        endDrag.current = null;
        setDragging(null);
      };
      window.addEventListener("pointermove", move);
      window.addEventListener("pointerup", end);
      window.addEventListener("pointercancel", end);
      root.style.cursor = "col-resize";
      root.style.userSelect = "none";
      endDrag.current = end;
      setDragging(id);
    },
    onDoubleClick: () => fit(id),
    onKeyDown: (e: KeyboardEvent<HTMLElement>) => {
      const step = e.shiftKey ? STEP * 4 : STEP;
      const now = frozen();
      if (e.key === "ArrowRight") resize(id, now[id] + step, now);
      else if (e.key === "ArrowLeft") resize(id, now[id] - step, now);
      else if (e.key === "Enter") fit(id);
      else return;
      e.preventDefault();
    },
  });
  const sized = widths ? visible.map((c) => widths[c.id] ?? NEW_COLUMN_WIDTH) : [];
  const tableStyle = widths
    ? ({ "--table-min": `${sized.reduce((a, b) => a + b, 0) + (props.selectable ? 40 : 0)}px` } as CSSProperties)
    : undefined;
  const phone = (id: string) =>
    !props.phoneColumns ? undefined : props.phoneColumns.includes(id) ? "max-sm:whitespace-normal max-sm:break-words" : "max-sm:hidden";
  const bays = props.bays;
  const racked = !!bays && sorting[0]?.id === bays.column && !sorting[0]?.desc;
  const groups: { id: string; label: string; rows: typeof rows }[] = [];
  for (const row of rows) {
    const bay = racked ? bays.of(row.original) : { id: "all", label: "" };
    const last = groups.at(-1);
    if (last?.id === bay.id) last.rows.push(row);
    else groups.push({ ...bay, rows: [row] });
  }

  return (
    <div className="grid grid-cols-[minmax(0,1fr)] gap-3">
      <div className="flex flex-wrap items-center gap-2">
        <div className="relative w-full max-w-xs">
          <Search aria-hidden className="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground" />
          <Input
            type="search"
            placeholder="Filter…"
            aria-label={`Filter ${props.label.toLowerCase()}`}
            data-filter
            className="h-8 pr-8 pl-8"
            value={globalFilter}
            onChange={(e) => setGlobalFilter(e.target.value)}
          />
          <kbd
            aria-hidden
            className="pointer-events-none absolute top-1/2 right-2 -translate-y-1/2 rounded-xs border bg-muted px-1 font-mono text-[10px] text-muted-foreground"
          >
            /
          </kbd>
        </div>
        {props.data && (
          <span className="text-xs text-muted-foreground tabular-nums">
            {rows.length === data.length ? data.length : `${rows.length} of ${data.length}`}
          </span>
        )}
        {props.selectable && selected.length > 0 && (
          <div
            className="flex flex-wrap items-center gap-2 rounded-md border border-primary/40 bg-cord-soft py-1 pr-1 pl-2.5"
            role="group"
            aria-label="Selected"
          >
            <span className="text-sm font-medium tabular-nums">{selected.length} selected</span>
            {props.actions?.(selected, clear)}
          </div>
        )}
        <div className="ml-auto flex items-center gap-2">
          {props.toolbar}
          {hideable.length > 0 && (
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button variant="outline" size="icon-sm" aria-label="Columns">
                  <Columns3 />
                </Button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                <DropdownMenuLabel>Columns</DropdownMenuLabel>
                {hideable.map((c) => (
                  <DropdownMenuCheckboxItem
                    key={c.id}
                    checked={c.getIsVisible()}
                    onCheckedChange={(v) => c.toggleVisibility(!!v)}
                    onSelect={(e) => e.preventDefault()}
                  >
                    {typeof c.columnDef.header === "string" ? c.columnDef.header : c.id}
                  </DropdownMenuCheckboxItem>
                ))}
                {widths && (
                  <>
                    <DropdownMenuSeparator />
                    <DropdownMenuItem onSelect={() => setWidths(null)}>Reset widths</DropdownMenuItem>
                  </>
                )}
              </DropdownMenuContent>
            </DropdownMenu>
          )}
        </div>
      </div>
      <div>
        <Table
          ref={tableRef}
          aria-label={props.label}
          style={tableStyle}
          className={widths ? "sm:min-w-(--table-min) sm:table-fixed [&_td]:sm:overflow-hidden [&_td]:sm:text-ellipsis [&_th]:sm:overflow-hidden [&_th]:sm:text-ellipsis" : undefined}
        >
          {widths && (
            <colgroup>
              {props.selectable && <col className="sm:w-10" />}
              {visible.map((c, i) => (
                <col key={c.id} className="sm:w-(--w)" style={{ "--w": `${sized[i]}px` } as CSSProperties} />
              ))}
              <col />
            </colgroup>
          )}
          <TableHeader>
            {table.getHeaderGroups().map((group) => (
              <TableRow key={group.id}>
                {props.selectable && (
                  <TableHead className="w-8">
                    <Checkbox
                      aria-label="Select all"
                      checked={
                        table.getIsAllRowsSelected() ? true : table.getIsSomeRowsSelected() ? "indeterminate" : false
                      }
                      onCheckedChange={(v) => table.toggleAllRowsSelected(!!v)}
                    />
                  </TableHead>
                )}
                {group.headers.map((header) => {
                  const sorted = header.column.getIsSorted();
                  return (
                    <TableHead
                      key={header.id}
                      data-col={header.column.id}
                      // Named by its header alone: the resize handle inside is not part of the name.
                      aria-label={typeof header.column.columnDef.header === "string" ? header.column.columnDef.header : undefined}
                      className={cn("relative", phone(header.column.id))}
                      aria-sort={sorted === "asc" ? "ascending" : sorted === "desc" ? "descending" : undefined}
                    >
                      {header.isPlaceholder ? null : header.column.getCanSort() ? (
                        <button
                          type="button"
                          className="group/sort -mx-1 inline-flex items-center gap-1 rounded-xs px-1 hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
                          onClick={header.column.getToggleSortingHandler()}
                        >
                          <table.FlexRender header={header} />
                          {sorted === "asc" ? (
                            <ArrowUp className="size-3 text-primary" />
                          ) : sorted === "desc" ? (
                            <ArrowDown className="size-3 text-primary" />
                          ) : (
                            <ArrowUpDown aria-hidden className="size-3 opacity-0 group-hover/sort:opacity-60" />
                          )}
                        </button>
                      ) : (
                        <table.FlexRender header={header} />
                      )}
                      <span
                        role="separator"
                        aria-orientation="vertical"
                        aria-label={`Resize ${headerOf(header.column.id)} column`}
                        aria-valuemin={MIN_WIDTH}
                        aria-valuenow={widths ? (widths[header.column.id] ?? NEW_COLUMN_WIDTH) : undefined}
                        tabIndex={0}
                        data-active={dragging === header.column.id || undefined}
                        title="Drag to resize · double-click to fit"
                        className="absolute inset-y-0 right-0 z-10 w-2 cursor-col-resize touch-none outline-none select-none after:absolute after:inset-y-1.5 after:right-0 after:w-0.5 after:rounded-full after:transition-colors hover:after:bg-muted-foreground/60 focus-visible:after:bg-primary data-active:after:bg-primary max-sm:hidden"
                        {...edge(header.column.id)}
                      />
                    </TableHead>
                  );
                })}
                {widths && <TableHead aria-hidden className="max-sm:hidden" />}
              </TableRow>
            ))}
          </TableHeader>
          {props.error || !props.data || rows.length === 0 ? (
            <TableBody>
              {props.error ? (
                <TableRow>
                  <TableCell colSpan={width} role="alert" className="h-16 whitespace-normal text-destructive">
                    {errorMessage(props.error)}
                  </TableCell>
                </TableRow>
              ) : !props.data ? (
                <TableRow>
                  <TableCell colSpan={width} className="text-muted-foreground">
                    <span className="sr-only">Loading…</span>
                    <span aria-hidden className="grid gap-2 py-1">
                      {[72, 54, 64].map((w) => (
                        <span key={w} className="h-3 rounded-xs bg-muted motion-safe:animate-pulse" style={{ width: `${w}%` }} />
                      ))}
                    </span>
                  </TableCell>
                </TableRow>
              ) : (
                <TableRow>
                  <TableCell colSpan={width} className="h-24 text-center whitespace-normal text-muted-foreground">
                    {data.length === 0 ? props.empty : "Nothing matches the filter."}
                  </TableCell>
                </TableRow>
              )}
            </TableBody>
          ) : (
            groups.map((g) => (
              <TableBody key={g.id} aria-label={racked ? g.label : undefined} className="[&_tr:last-child]:border-b [&:last-child_tr:last-child]:border-0">
                {racked && (
                  // The bay's label is the row group's name; drawn here for sighted readers.
                  <tr aria-hidden={props.selectable ? undefined : true} data-bay={g.id} className="border-b bg-muted/30">
                    {props.selectable && (
                      <td className="h-7 py-0 pr-0 pl-3 align-middle">
                        <Checkbox
                          aria-label={`Select all in ${g.label}`}
                          checked={g.rows.every((r) => r.getIsSelected()) ? true : g.rows.some((r) => r.getIsSelected()) ? "indeterminate" : false}
                          onCheckedChange={(v) =>
                            setRowSelection((sel) => {
                              const next = { ...sel };
                              for (const r of g.rows) {
                                if (v) next[r.id] = true;
                                else delete next[r.id];
                              }
                              return next;
                            })
                          }
                        />
                      </td>
                    )}
                    <td
                      aria-hidden={props.selectable ? true : undefined}
                      colSpan={width - (props.selectable ? 1 : 0)}
                      className="h-7 px-3 text-xs font-medium text-muted-foreground"
                    >
                      {g.label} <span className="ml-1 font-normal tabular-nums">{g.rows.length}</span>
                    </td>
                  </tr>
                )}
                {g.rows.map((row) => (
                  <TableRow key={row.id} data-state={row.getIsSelected() ? "selected" : undefined}>
                    {props.selectable && (
                      <TableCell>
                        <Checkbox
                          aria-label={`Select ${row.id}`}
                          checked={row.getIsSelected()}
                          onCheckedChange={(v) => row.toggleSelected(!!v)}
                        />
                      </TableCell>
                    )}
                    {row.getVisibleCells().map((cell) => (
                      <TableCell key={cell.id} data-col={cell.column.id} className={phone(cell.column.id)}>
                        <table.FlexRender cell={cell} />
                      </TableCell>
                    ))}
                    {widths && <TableCell aria-hidden className="max-sm:hidden" />}
                  </TableRow>
                ))}
              </TableBody>
            ))
          )}
        </Table>
      </div>
    </div>
  );
}
