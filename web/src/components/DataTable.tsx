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
import { useState, type ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { errorMessage } from "@/lib/errors";

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

/**
 * A resource list: text filter over every column, sorting on header click,
 * column visibility, and optional row selection for bulk actions.
 */
export function DataTable<T extends RowData>(props: DataTableProps<T>) {
  const [sorting, setSorting] = useState<SortingState>(props.initialSorting ?? []);
  const [globalFilter, setGlobalFilter] = useState(props.initialFilter ?? "");
  const [columnVisibility, setColumnVisibility] = useState<ColumnVisibilityState>(() =>
    Object.fromEntries((props.initialHidden ?? []).map((id) => [id, false])),
  );
  const [rowSelection, setRowSelection] = useState<RowSelectionState>({});
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
  const width = table.getVisibleLeafColumns().length + (props.selectable ? 1 : 0);
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
              </DropdownMenuContent>
            </DropdownMenu>
          )}
        </div>
      </div>
      <div>
        <Table aria-label={props.label}>
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
                      className={phone(header.column.id)}
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
                    </TableHead>
                  );
                })}
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
                  <tr aria-hidden data-bay={g.id} className="border-b bg-muted/30">
                    <td colSpan={width} className="h-7 px-3 text-xs font-medium text-muted-foreground">
                      {g.label} <span className="ml-1 tabular-nums opacity-70">{g.rows.length}</span>
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
                      <TableCell key={cell.id} className={phone(cell.column.id)}>
                        <table.FlexRender cell={cell} />
                      </TableCell>
                    ))}
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
