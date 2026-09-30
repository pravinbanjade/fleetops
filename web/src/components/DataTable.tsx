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
import { ArrowDown, ArrowUp, Columns3 } from "lucide-react";
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

  return (
    <div className="grid grid-cols-[minmax(0,1fr)] gap-3">
      <div className="flex flex-wrap items-center gap-2">
        <Input
          type="search"
          placeholder="Filter…"
          aria-label={`Filter ${props.label.toLowerCase()}`}
          data-filter
          className="max-w-xs"
          value={globalFilter}
          onChange={(e) => setGlobalFilter(e.target.value)}
        />
        {props.selectable && selected.length > 0 && (
          <div className="flex flex-wrap items-center gap-2" role="group" aria-label="Selected">
            <span className="text-sm text-muted-foreground">{selected.length} selected</span>
            {props.actions?.(selected, clear)}
          </div>
        )}
        <div className="ml-auto flex items-center gap-2">
          {props.toolbar}
          {hideable.length > 0 && (
            <DropdownMenu>
              <DropdownMenuTrigger asChild>
                <Button variant="outline" size="sm" aria-label="Columns">
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
      <div className="rounded-md border">
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
                      aria-sort={sorted === "asc" ? "ascending" : sorted === "desc" ? "descending" : undefined}
                    >
                      {header.isPlaceholder ? null : header.column.getCanSort() ? (
                        <button
                          type="button"
                          className="inline-flex items-center gap-1 hover:underline"
                          onClick={header.column.getToggleSortingHandler()}
                        >
                          <table.FlexRender header={header} />
                          {sorted === "asc" && <ArrowUp className="size-3" />}
                          {sorted === "desc" && <ArrowDown className="size-3" />}
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
          <TableBody>
            {props.error ? (
              <TableRow>
                <TableCell colSpan={width} role="alert" className="text-destructive">
                  {errorMessage(props.error)}
                </TableCell>
              </TableRow>
            ) : !props.data ? (
              <TableRow>
                <TableCell colSpan={width} className="text-muted-foreground">
                  Loading…
                </TableCell>
              </TableRow>
            ) : rows.length === 0 ? (
              <TableRow>
                <TableCell colSpan={width} className="text-muted-foreground">
                  {data.length === 0 ? props.empty : "Nothing matches the filter."}
                </TableCell>
              </TableRow>
            ) : (
              rows.map((row) => (
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
                    <TableCell key={cell.id}>
                      <table.FlexRender cell={cell} />
                    </TableCell>
                  ))}
                </TableRow>
              ))
            )}
          </TableBody>
        </Table>
      </div>
    </div>
  );
}
