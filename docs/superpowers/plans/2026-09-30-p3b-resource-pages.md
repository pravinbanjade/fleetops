# FleetOps P3b — Resource and Admin Pages Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build every resource page of the web UI on the P3a foundation:
- containers (list with bulk actions, detail tabs, file browser, and the create / edit-by-recreate / duplicate form);
- images (with pull and build progress over the websocket `task` topic);
- volumes, networks and compose projects;
- the admin pages for users, environments and the audit log;
- the ⌘K command palette, with each page loaded as its own chunk.

**Architecture:**
- **Every page is a lazily loaded route** (`lazyRouteComponent`) under `/envs/$env/…`, or `/admin/…` for admins. The bundle splits by page, and P3a's raised `chunkSizeWarningLimit` goes away.
- **`DataTable`** (TanStack Table v9) is the one list component. It gives every list:
  - a text filter over all columns;
  - header sorting;
  - column visibility;
  - row selection for bulk actions.
- **Lists stay live with no new code.** P3a's event layer already invalidates `["env", env, "containers" | "images" | "volumes" | "networks" | "compose-projects"]`. Detail keys extend the list keys, so a detail page refetches on the same events. When the resource is gone, it says so.
- **Mutations** go through the generated SDK:
  - Dialogs show failures inline and do not toast (`meta.inline`).
  - Bulk actions run in parallel and name each failure (`runBulk`, `bulkMessage`).
- **Image pull and build return `202 {task_id}`.** A tab-wide `TaskTracker` follows each task over the `task` topic until it ends, whether or not its dialog is still open. It reports the result as a toast. The dialog renders the tracker's progress: layers for a pull, the log for a build.
- **Admin pages sit under an `/admin` route.** It sends anyone who is not an admin back to `/`. The server enforces the same rule on every admin endpoint.

**Tech Stack:** P3a's stack, plus:
- `@tanstack/react-table` 9.2 (the v9 API: `useTable` and `tableFeatures`, not v8's `useReactTable`);
- `cmdk` 1.1 for the palette;
- vendored shadcn/ui dialog, checkbox, table, tabs, textarea and command components, and a styled native select.

**Spec:** `docs/superpowers/specs/2026-09-29-fleetops-core-docker-design.md`:
- §4.1: admin user CRUD, environment CRUD and the admin-only audit list.
- §4.2: containers, images, volumes, networks, compose projects, the command palette and keyboard shortcuts.
- §4.3: live lists from events.
- §5.1: the REST routes behind every page.
- §6: Docker's message is shown inline or in a toast.
- §7: env values that look like secrets are masked in the UI.

**Plan series for sub-project 1:**
- P1, P2a–P2c and P3a are done.
- **P3b (this plan):** the resource and admin pages, the command palette and route code-splitting.
- P3c: the log viewer, terminal and stats (including the containers list's sparklines).
- P3d: Playwright e2e, `make image` and CI.

## Global Constraints

- **Everything from P3a's Global Constraints still holds:**
  - Node ≥24 and pnpm 12.8.1; TypeScript `~6.0.3`.
  - The generated client is never edited by hand, and `pnpm check:api` stays clean. This plan changes no server code, so `src/api/gen` does not change.
  - The CSP and websocket-Origin rules.
  - Query keys start with `["env", env]`.
  - Errors come from `errorMessage` and show inline (`role="alert"`) or as a toast.
- **Routes:**
  - The resource routes are children of `envRoute`, with flat paths:
    - `containers`, `containers/new` (`?from=<id>` duplicates) and `containers/$id` (`?tab=`, `?path=`);
    - `containers/$id/edit`;
    - `images` and `images/$id`;
    - `volumes` and `volumes/$name`;
    - `networks` and `networks/$id`;
    - `compose`.
  - Admin routes: `/admin/users`, `/admin/envs` and `/admin/audit`.
  - Route ids have no trailing slash, for example `/authed/shell/envs/$env/containers/$id`.
  - Pages are exported by name and loaded with `lazyRouteComponent(() => import(...), "Name")`. Nothing that `router.tsx` imports eagerly may import a page module, or that page's chunk is folded back into the main bundle. For example, the container tab list lives in `pages/containers/tabs.ts`.
- **Dialogs and errors:**
  - A dialog that runs a mutation shows its error inline and stays open; it sets `meta: { inline: true }` so the global handler does not toast as well.
  - A bulk action toasts once: success, or `"<Done> <n> of <total> <noun>s. <name>: <error>; …"`.
- **Image ids:** image ids are `sha256:…`, so the router encodes the `:` in paths (`/images/sha256%3A…`). `mockFetch` routes match the raw, still-encoded path, so test keys use `%3A`.
- **Tests:**
  - `src/test/setup.ts` raises Testing Library's `asyncUtilTimeout` to 3 s. Every page is a lazy chunk now, so a first render waits on a dynamic import, and 1 s was flaky under a loaded full-suite run.
  - Build uploads are tested at the SDK boundary (`vi.mock("@/api/gen/sdk.gen")`), not through `fetch`. vitest's bridge from jsdom 30's `File` to Node's `Request` no longer finds jsdom's Blob bytes (`Cannot read properties of undefined (reading '_bytes')`).
- **Spec deltas (intentional):**
  - **Duplicate** fills the create form from `GET /containers/{id}/spec` and submits it with `POST /containers`. `POST /containers/{id}/duplicate` takes only a name, and the spec wants "the same pre-filled form".
  - **The create form's "advanced" JSON** is a textarea, validated on submit (it must be a JSON object). CodeMirror is not used. It arrives with stack editing in sub-project 3, where editing is the main job.
  - **Deferred to P3c:** the containers list's live CPU/memory sparklines and the Stats, Logs and Terminal tabs. They need the stats, logs and exec topics.
  - **Selects** are a styled native `<select>` (`ui/native-select.tsx`), not Radix Select. Keyboard and screen-reader behavior come free, and the tests can drive it with `userEvent.selectOptions`.
  - **Keyboard shortcuts** are ⌘K / Ctrl-K for the palette and `/` to focus the page's list filter. Neither fires while typing in a field.
  - **Volume prune** uses Docker's default, which removes only anonymous unused volumes on Docker 23 and later. The dialog says so.

## Review Focus

1. **A resource is removed elsewhere while its page is open.** This happens from the CLI, another tab or `compose down`. The page must say "This <thing> no longer exists." with a link back to the list, not spin or show stale data. A 404 must not be retried. Pinned in:
   - Task 3: `says so when the container is removed elsewhere`.
   - Task 6: `says so when the image is gone`.
2. **A bulk action where some items fail.** For example, stopping a mix of running and stopped containers. The user must learn which items failed and why, and the ones that succeeded must not be reported as failed. Pinned in:
   - Task 2: `stops the selected containers and reports a failure by name`, and `runBulk runs every item and names the failures`.
   - Task 8: `removes a volume, showing Docker's refusal for one in use`.
3. **A long pull or build outlives its dialog, or the websocket reconnects mid-task.** The result must still arrive as a toast after the dialog closes. After a reconnect, the server's replayed backlog must not duplicate the progress. Pinned in:
   - Task 7: `still reports the result after the dialog is closed`.
   - Task 7: `starts over when the server resends the backlog after a reconnect`.
4. **Secrets in container env.** Values whose keys look like `*PASS*`, `*SECRET*`, `*TOKEN*` or `*KEY*` must be masked until clicked. Pinned in Task 3: `masks secret-looking env values until clicked`, and `isSecretKey`.
5. **Form input the server would reject with a vague message, or accept wrongly.** Examples: a port that is not a number, an unclosed quote in a command, malformed `key=value` lines, invalid advanced JSON, or a build context over the 100 MB upload limit. The field must be named before anything is sent. Pinned in:
   - Task 5: `names the field that is wrong`, and `says which field is wrong without sending anything`.
   - Task 7: `refuses a file over the upload limit before sending it`.
   - Task 8: `rejects a malformed option line before sending`.

---

## File Structure

```
web/
  package.json, pnpm-lock.yaml             + @tanstack/react-table, cmdk
  vite.config.ts                           drops P3a's chunkSizeWarningLimit
  src/
    router.tsx                             lazy pages; resource, form and admin routes; admin guard
    api/queries.ts                         + detail/spec/files/history/audit queries, retryUnlessGone
    api/docker.ts                          the Docker inspect fields the pages read (passthrough bodies)
    components/
      DataTable.tsx                        filter, sort, column visibility, selection (TanStack Table v9)
      ConfirmDialog.tsx                    confirm → run → close, or stay open with the error
      Page.tsx, Details.tsx, JsonView.tsx  page header; label/value list + "no longer exists"; JSON viewer
      StateBadge.tsx                       container/project state colors
      CommandPalette.tsx, nav.ts           ⌘K palette and `/` shortcut; sidebar page lists
      Shell.tsx                            (modify) nav from nav.ts, admin section, palette button
      ui/{checkbox,dialog,table,tabs,textarea,native-select,command}.tsx   vendored
    lib/
      bulk.ts                              runBulk, bulkMessage
      ports.ts                             portLinks: published ports as links
      format.ts                            (modify) + formatAgo, shortId
      secrets.ts                           isSecretKey, splitEnv
      files.ts                             formatMode, joinPath, crumbs
      args.ts                              splitArgs, joinArgs (shell-style quoting)
      kv.ts                                parseKeyValues, formatKeyValues
      spec.ts                              SpecForm ⇄ ContainerSpec
      images.ts                            parseRef
      progress.ts                          pullProgress, buildLog
      usage.ts                             volumeUsers
    live/tasks.ts, live/TaskProvider.tsx   TaskTracker; useTaskTracker, useTask
    pages/containers/                      ContainersPage, ContainerPage, FilesTab, ContainerFormPage, actions, tabs
    pages/images/                          ImagesPage, ImagePage, dialogs, TaskDialogs
    pages/volumes/, pages/networks/        list page, detail page, dialogs
    pages/compose/ComposePage.tsx
    pages/admin/                           UsersPage, EnvironmentsPage, AuditPage, FormDialog
    test/docker.ts                         container/inspect/image fixtures
```

**Test commands.** `cd web && pnpm test <paths>` runs the named tests. The whole-suite gate is `pnpm test && pnpm typecheck && pnpm lint` from `web/`, or `make test-web` from the repo root, which also runs `check:api`.

---

### Task 1: The list table, confirm dialog, UI kit, and lazy pages

**Files:**
- Create:
  - `web/src/components/DataTable.tsx`, `web/src/components/ConfirmDialog.tsx` and `web/src/components/Page.tsx`;
  - `web/src/components/ui/{checkbox,dialog,native-select,table,tabs,textarea}.tsx`.
- Modify: `web/package.json` and `pnpm-lock.yaml` (via `pnpm add`), `web/vite.config.ts`, `web/src/router.tsx` and `web/src/test/setup.ts`.
- Test: `web/src/components/DataTable.test.tsx` and `web/src/components/ConfirmDialog.test.tsx`.

**Interfaces:**
- Consumes (P3a): `createQueryClient` from `@/App`, `errorMessage`, `FormError` from `@/pages/AuthLayout`, and the shadcn `Button`, `Input`, `Label` and `DropdownMenu*`.
- Produces `@/components/DataTable`:
  - `features`, and `type Features`.
  - `type Column<T extends RowData> = ColumnDef<Features, T, any>`.
  - `columns<T>()`, the v9 column helper: `col.accessor("key" | fn, { id?, header, cell?, enableHiding?, enableSorting? })` and `col.display({ id, header, cell })`.
  - `<DataTable label data columns getRowId empty error? selectable? actions?(selected, clear) toolbar? initialSorting? initialHidden? />`:
    - The filter input is labelled `Filter <label lowercased>` and carries `data-filter`.
    - The bulk area is `role="group"` named `Selected`.
    - The row checkboxes are named `Select <rowId>`, plus `Select all`.
    - The columns menu button is labelled `Columns`.
- Produces `@/components/ConfirmDialog`: `<ConfirmDialog open onOpenChange title description? confirmLabel onConfirm: () => Promise<unknown> destructive? children? />`. It closes when `onConfirm` resolves, and shows the rejection inline.
- Produces `@/components/Page`: `<Page title actions? children />`, whose grid is `minmax(0,1fr)` so a wide table scrolls inside the page instead of widening it.
- Produces the UI kit:
  - `Dialog`, `DialogContent` (`showCloseButton`), `DialogHeader`, `DialogFooter`, `DialogTitle` and `DialogDescription`;
  - `Checkbox`;
  - `Table`, `TableHeader`, `TableBody`, `TableRow`, `TableHead` and `TableCell`;
  - `Tabs`, `TabsList`, `TabsTrigger` and `TabsContent`;
  - `Textarea`;
  - `NativeSelect`, where `className` sizes the wrapper.
- Produces: every route below the shell loads its page with `lazyRouteComponent`, and later tasks add their pages the same way.

- [ ] **Step 1: Add the table dependency**

Run: `cd web && pnpm add @tanstack/react-table@^9.2.4`
Expected: `+ @tanstack/react-table 9.2.4`. `package.json` and `pnpm-lock.yaml` change.

- [ ] **Step 2: Write the failing tests**

`web/src/components/DataTable.test.tsx`:

```tsx
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
```

`web/src/components/ConfirmDialog.test.tsx`:

```tsx
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
```

- [ ] **Step 3: Run them to verify they fail**

Run: `cd web && pnpm test src/components/DataTable.test.tsx src/components/ConfirmDialog.test.tsx`
Expected: FAIL, 2 files, with `Failed to resolve import "./DataTable"` and `"./ConfirmDialog"`.

- [ ] **Step 4: Vendor the UI components**

These are shadcn/ui new-york sources with the imports fixed to `@/lib/utils` and the unified `radix-ui` package (see P3a's spec deltas). `native-select.tsx` is our own.

`web/src/components/ui/dialog.tsx`:

```tsx
"use client"

import * as React from "react"
import { cn } from "@/lib/utils"
import { XIcon } from "lucide-react"
import { Dialog as DialogPrimitive } from "radix-ui"

function Dialog({ ...props }: React.ComponentProps<typeof DialogPrimitive.Root>) {
  return <DialogPrimitive.Root data-slot="dialog" {...props} />
}

function DialogTrigger({ ...props }: React.ComponentProps<typeof DialogPrimitive.Trigger>) {
  return <DialogPrimitive.Trigger data-slot="dialog-trigger" {...props} />
}

function DialogPortal({ ...props }: React.ComponentProps<typeof DialogPrimitive.Portal>) {
  return <DialogPrimitive.Portal data-slot="dialog-portal" {...props} />
}

function DialogClose({ ...props }: React.ComponentProps<typeof DialogPrimitive.Close>) {
  return <DialogPrimitive.Close data-slot="dialog-close" {...props} />
}

function DialogOverlay({ className, ...props }: React.ComponentProps<typeof DialogPrimitive.Overlay>) {
  return (
    <DialogPrimitive.Overlay
      data-slot="dialog-overlay"
      className={cn(
        "fixed inset-0 z-50 bg-black/50 data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=open]:animate-in data-[state=open]:fade-in-0",
        className
      )}
      {...props}
    />
  )
}

function DialogContent({
  className,
  children,
  showCloseButton = true,
  ...props
}: React.ComponentProps<typeof DialogPrimitive.Content> & {
  showCloseButton?: boolean
}) {
  return (
    <DialogPortal data-slot="dialog-portal">
      <DialogOverlay />
      <DialogPrimitive.Content
        data-slot="dialog-content"
        className={cn(
          "fixed top-[50%] left-[50%] z-50 grid max-h-[calc(100%-2rem)] w-full max-w-[calc(100%-2rem)] translate-x-[-50%] translate-y-[-50%] gap-4 overflow-y-auto rounded-lg border bg-background p-6 shadow-lg duration-200 outline-none data-[state=closed]:animate-out data-[state=closed]:fade-out-0 data-[state=closed]:zoom-out-95 data-[state=open]:animate-in data-[state=open]:fade-in-0 data-[state=open]:zoom-in-95 sm:max-w-lg",
          className
        )}
        {...props}
      >
        {children}
        {showCloseButton && (
          <DialogPrimitive.Close
            data-slot="dialog-close"
            className="absolute top-4 right-4 rounded-xs opacity-70 ring-offset-background transition-opacity hover:opacity-100 focus:ring-2 focus:ring-ring focus:ring-offset-2 focus:outline-hidden disabled:pointer-events-none data-[state=open]:bg-accent data-[state=open]:text-muted-foreground [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4"
          >
            <XIcon />
            <span className="sr-only">Close</span>
          </DialogPrimitive.Close>
        )}
      </DialogPrimitive.Content>
    </DialogPortal>
  )
}

function DialogHeader({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="dialog-header"
      className={cn("flex flex-col gap-2 text-center sm:text-left", className)}
      {...props}
    />
  )
}

function DialogFooter({ className, ...props }: React.ComponentProps<"div">) {
  return (
    <div
      data-slot="dialog-footer"
      className={cn("flex flex-col-reverse gap-2 sm:flex-row sm:justify-end", className)}
      {...props}
    />
  )
}

function DialogTitle({ className, ...props }: React.ComponentProps<typeof DialogPrimitive.Title>) {
  return (
    <DialogPrimitive.Title
      data-slot="dialog-title"
      className={cn("text-lg leading-none font-semibold", className)}
      {...props}
    />
  )
}

function DialogDescription({ className, ...props }: React.ComponentProps<typeof DialogPrimitive.Description>) {
  return (
    <DialogPrimitive.Description
      data-slot="dialog-description"
      className={cn("text-sm text-muted-foreground", className)}
      {...props}
    />
  )
}

export {
  Dialog,
  DialogClose,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogOverlay,
  DialogPortal,
  DialogTitle,
  DialogTrigger,
}
```

`web/src/components/ui/checkbox.tsx`:

```tsx
"use client"

import * as React from "react"
import { cn } from "@/lib/utils"
import { CheckIcon, MinusIcon } from "lucide-react"
import { Checkbox as CheckboxPrimitive } from "radix-ui"

function Checkbox({ className, ...props }: React.ComponentProps<typeof CheckboxPrimitive.Root>) {
  return (
    <CheckboxPrimitive.Root
      data-slot="checkbox"
      className={cn(
        "peer size-4 shrink-0 rounded-[4px] border border-input shadow-xs transition-shadow outline-none focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 disabled:cursor-not-allowed disabled:opacity-50 aria-invalid:border-destructive aria-invalid:ring-destructive/20 data-[state=checked]:border-primary data-[state=checked]:bg-primary data-[state=checked]:text-primary-foreground data-[state=indeterminate]:border-primary data-[state=indeterminate]:bg-primary data-[state=indeterminate]:text-primary-foreground dark:bg-input/30 dark:aria-invalid:ring-destructive/40 dark:data-[state=checked]:bg-primary",
        className
      )}
      {...props}
    >
      <CheckboxPrimitive.Indicator
        data-slot="checkbox-indicator"
        className="grid place-content-center text-current transition-none"
      >
        {props.checked === "indeterminate" ? <MinusIcon className="size-3.5" /> : <CheckIcon className="size-3.5" />}
      </CheckboxPrimitive.Indicator>
    </CheckboxPrimitive.Root>
  )
}

export { Checkbox }
```

`web/src/components/ui/table.tsx`:

```tsx
import * as React from "react"
import { cn } from "@/lib/utils"

function Table({ className, ...props }: React.ComponentProps<"table">) {
  return (
    <div data-slot="table-container" className="relative w-full overflow-x-auto">
      <table data-slot="table" className={cn("w-full caption-bottom text-sm", className)} {...props} />
    </div>
  )
}

function TableHeader({ className, ...props }: React.ComponentProps<"thead">) {
  return <thead data-slot="table-header" className={cn("[&_tr]:border-b", className)} {...props} />
}

function TableBody({ className, ...props }: React.ComponentProps<"tbody">) {
  return <tbody data-slot="table-body" className={cn("[&_tr:last-child]:border-0", className)} {...props} />
}

function TableRow({ className, ...props }: React.ComponentProps<"tr">) {
  return (
    <tr
      data-slot="table-row"
      className={cn("border-b transition-colors hover:bg-muted/50 data-[state=selected]:bg-muted", className)}
      {...props}
    />
  )
}

function TableHead({ className, ...props }: React.ComponentProps<"th">) {
  return (
    <th
      data-slot="table-head"
      className={cn(
        "h-10 px-2 text-left align-middle font-medium whitespace-nowrap text-foreground [&:has([role=checkbox])]:pr-0",
        className
      )}
      {...props}
    />
  )
}

function TableCell({ className, ...props }: React.ComponentProps<"td">) {
  return (
    <td
      data-slot="table-cell"
      className={cn("p-2 align-middle whitespace-nowrap [&:has([role=checkbox])]:pr-0", className)}
      {...props}
    />
  )
}

export { Table, TableBody, TableCell, TableHead, TableHeader, TableRow }
```

`web/src/components/ui/tabs.tsx`:

```tsx
"use client"

import * as React from "react"
import { cn } from "@/lib/utils"
import { Tabs as TabsPrimitive } from "radix-ui"

function Tabs({ className, ...props }: React.ComponentProps<typeof TabsPrimitive.Root>) {
  return <TabsPrimitive.Root data-slot="tabs" className={cn("flex flex-col gap-2", className)} {...props} />
}

function TabsList({ className, ...props }: React.ComponentProps<typeof TabsPrimitive.List>) {
  return (
    <TabsPrimitive.List
      data-slot="tabs-list"
      className={cn(
        "inline-flex h-9 w-fit items-center justify-center rounded-lg bg-muted p-[3px] text-muted-foreground",
        className
      )}
      {...props}
    />
  )
}

function TabsTrigger({ className, ...props }: React.ComponentProps<typeof TabsPrimitive.Trigger>) {
  return (
    <TabsPrimitive.Trigger
      data-slot="tabs-trigger"
      className={cn(
        "inline-flex h-[calc(100%-1px)] flex-1 items-center justify-center gap-1.5 rounded-md border border-transparent px-2 py-1 text-sm font-medium whitespace-nowrap text-foreground transition-[color,box-shadow] focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 focus-visible:outline-1 focus-visible:outline-ring disabled:pointer-events-none disabled:opacity-50 data-[state=active]:bg-background data-[state=active]:shadow-sm dark:text-muted-foreground dark:data-[state=active]:border-input dark:data-[state=active]:bg-input/30 dark:data-[state=active]:text-foreground [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
        className
      )}
      {...props}
    />
  )
}

function TabsContent({ className, ...props }: React.ComponentProps<typeof TabsPrimitive.Content>) {
  return <TabsPrimitive.Content data-slot="tabs-content" className={cn("flex-1 outline-none", className)} {...props} />
}

export { Tabs, TabsContent, TabsList, TabsTrigger }
```

`web/src/components/ui/textarea.tsx`:

```tsx
import * as React from "react"
import { cn } from "@/lib/utils"

function Textarea({ className, ...props }: React.ComponentProps<"textarea">) {
  return (
    <textarea
      data-slot="textarea"
      className={cn(
        "flex field-sizing-content min-h-16 w-full rounded-md border border-input bg-transparent px-3 py-2 text-base shadow-xs transition-[color,box-shadow] outline-none placeholder:text-muted-foreground focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 disabled:cursor-not-allowed disabled:opacity-50 aria-invalid:border-destructive aria-invalid:ring-destructive/20 md:text-sm dark:bg-input/30 dark:aria-invalid:ring-destructive/40",
        className
      )}
      {...props}
    />
  )
}

export { Textarea }
```

`web/src/components/ui/native-select.tsx`:

```tsx
import * as React from "react"
import { cn } from "@/lib/utils"
import { ChevronDownIcon } from "lucide-react"

/**
 * A styled native `<select>`: keyboard and screen-reader behavior come from
 * the browser. `className` sizes the wrapper, which the select fills.
 */
function NativeSelect({ className, ...props }: React.ComponentProps<"select">) {
  return (
    <div className={cn("relative w-full", className)}>
      <select
        data-slot="native-select"
        className="h-9 w-full min-w-0 appearance-none rounded-md border border-input bg-transparent py-1 pr-8 pl-3 text-base shadow-xs transition-[color,box-shadow] outline-none focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-50 md:text-sm dark:bg-input/30 [&>option]:bg-popover [&>option]:text-popover-foreground"
        {...props}
      />
      <ChevronDownIcon className="pointer-events-none absolute top-1/2 right-2.5 size-4 -translate-y-1/2 opacity-50" />
    </div>
  )
}

export { NativeSelect }
```

- [ ] **Step 5: Implement the table, the confirm dialog and the page frame**

`web/src/components/DataTable.tsx`:

```tsx
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
}

const EMPTY: never[] = [];

/**
 * A resource list: text filter over every column, sorting on header click,
 * column visibility, and optional row selection for bulk actions.
 */
export function DataTable<T extends RowData>(props: DataTableProps<T>) {
  const [sorting, setSorting] = useState<SortingState>(props.initialSorting ?? []);
  const [globalFilter, setGlobalFilter] = useState("");
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
    <div className="grid gap-3">
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
```

`web/src/components/ConfirmDialog.tsx`:

```tsx
import { useMutation } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { errorMessage } from "@/lib/errors";
import { FormError } from "@/pages/AuthLayout";

interface ConfirmDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description?: ReactNode;
  confirmLabel: string;
  /** Runs on confirm; the dialog closes when it resolves and shows the error when it rejects. */
  onConfirm: () => Promise<unknown>;
  destructive?: boolean;
  /** Options shown above the buttons, such as a "force" checkbox. */
  children?: ReactNode;
}

/** Asks before an action and runs it, keeping the dialog open with the error if it fails. */
export function ConfirmDialog(props: ConfirmDialogProps) {
  const m = useMutation({ mutationFn: props.onConfirm, meta: { inline: true } });
  const change = (open: boolean) => {
    if (!open) m.reset();
    props.onOpenChange(open);
  };
  return (
    <Dialog open={props.open} onOpenChange={change}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{props.title}</DialogTitle>
          {props.description && <DialogDescription>{props.description}</DialogDescription>}
        </DialogHeader>
        {props.children}
        <FormError message={m.error ? errorMessage(m.error) : null} />
        <DialogFooter>
          <Button variant="outline" onClick={() => change(false)}>
            Cancel
          </Button>
          <Button
            variant={props.destructive === false ? "default" : "destructive"}
            disabled={m.isPending}
            onClick={() => m.mutate(undefined, { onSuccess: () => change(false) })}
          >
            {props.confirmLabel}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
```

`web/src/components/Page.tsx`:

```tsx
import type { ReactNode } from "react";

/** A page's title row, with its actions on the right. */
export function Page({ title, actions, children }: { title: ReactNode; actions?: ReactNode; children: ReactNode }) {
  return (
    <div className="grid grid-cols-[minmax(0,1fr)] gap-4 p-6">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h1 className="min-w-0 truncate text-2xl font-semibold">{title}</h1>
        {actions && <div className="flex flex-wrap items-center gap-2">{actions}</div>}
      </div>
      {children}
    </div>
  );
}
```

- [ ] **Step 6: Load pages lazily, and give async waits more time**

`web/src/router.tsx`: the dashboard and account pages load on first visit.

```diff
--- a/web/src/router.tsx
+++ b/web/src/router.tsx
@@ -3,6 +3,7 @@ import {
   createRootRouteWithContext,
   createRoute,
   createRouter,
+  lazyRouteComponent,
   Outlet,
   redirect,
   type RouterHistory,
@@ -11,8 +12,6 @@ import { ApiError } from "@/api/client";
 import { authStatusQuery, envsQuery, meQuery } from "@/api/queries";
 import { Shell } from "@/components/Shell";
 import { LiveProvider } from "@/live/LiveProvider";
-import { AccountPage } from "@/pages/AccountPage";
-import { DashboardPage } from "@/pages/DashboardPage";
 import { LoginPage } from "@/pages/LoginPage";
 import { NoEnvironmentsPage, NotFoundPage } from "@/pages/misc";
 import { SetupPage } from "@/pages/SetupPage";
@@ -36,6 +35,9 @@ async function session(qc: QueryClient) {
   }
 }
 
+// Pages below the shell load on first visit (`lazyRouteComponent`), so
+// each is its own chunk of the bundle.
+
 const rootRoute = createRootRouteWithContext<RouterContext>()({
   component: Outlet,
   notFoundComponent: NotFoundPage,
@@ -107,13 +109,13 @@ export const envRoute = createRoute({
 const dashboardRoute = createRoute({
   getParentRoute: () => envRoute,
   path: "/",
-  component: DashboardPage,
+  component: lazyRouteComponent(() => import("@/pages/DashboardPage"), "DashboardPage"),
 });
 
 const accountRoute = createRoute({
   getParentRoute: () => shellRoute,
   path: "/account",
-  component: AccountPage,
+  component: lazyRouteComponent(() => import("@/pages/AccountPage"), "AccountPage"),
 });
 
 const routeTree = rootRoute.addChildren([
```

`web/vite.config.ts`: the bundle now splits by page, so P3a's raised warning limit goes.

```diff
--- a/web/vite.config.ts
+++ b/web/vite.config.ts
@@ -16,8 +16,7 @@ export default defineConfig({
     port: 5173,
     proxy: { "/api": { target: api, ws: true } },
   },
-  // One bundle until the resource pages land (P3b splits them by route).
-  build: { outDir: "dist", emptyOutDir: true, chunkSizeWarningLimit: 1024 },
+  build: { outDir: "dist", emptyOutDir: true },
   test: {
     environment: "jsdom",
     globals: true,
```

`web/src/test/setup.ts`:

```diff
--- a/web/src/test/setup.ts
+++ b/web/src/test/setup.ts
@@ -1,8 +1,12 @@
 import "@testing-library/jest-dom/vitest";
-import { cleanup } from "@testing-library/react";
+import { cleanup, configure } from "@testing-library/react";
 import { afterEach, beforeEach, vi } from "vitest";
 import { FakeWebSocket } from "./fake-ws";
 
+// Pages load lazily (their own chunks), so a first render waits on a dynamic
+// import; under a loaded full-suite run 1 s (the default) is too tight.
+configure({ asyncUtilTimeout: 3000 });
+
 // jsdom lacks these browser APIs; sonner and Radix menus use them.
 window.matchMedia ??= (query: string) =>
   ({
```

- [ ] **Step 7: Run all checks, and look at the chunks**

Run: `cd web && pnpm test && pnpm typecheck && pnpm lint && pnpm build`
Expected:
- 79 passed (14 files); typecheck and lint are clean.
- The build lists `DashboardPage-*.js` and `AccountPage-*.js` as their own chunks, and prints no chunk-size warning.

- [ ] **Step 8: Commit**

```bash
git add web/package.json web/pnpm-lock.yaml web/vite.config.ts web/src/components web/src/router.tsx web/src/test/setup.ts
git commit -m "feat(web): list table, confirm dialog, dialog/table/tabs kit and per-page chunks"
```

---

### Task 2: The containers list

**Files:**
- Create:
  - `web/src/pages/containers/ContainersPage.tsx` and `web/src/pages/containers/actions.tsx`;
  - `web/src/components/StateBadge.tsx`;
  - `web/src/lib/ports.ts` and `web/src/lib/bulk.ts`;
  - `web/src/test/docker.ts`.
- Modify: `web/src/lib/format.ts`, `web/src/router.tsx` and `web/src/components/Shell.tsx`.
- Test: `web/src/lib/containers.test.ts` and `web/src/pages/containers/ContainersPage.test.tsx`.

**Interfaces:**
- Consumes: `DataTable`, `columns`, `ConfirmDialog` and `Page` (Task 1); `containersQuery` and `keys` (P3a); and the SDK functions `containerAction`, `deleteContainer` and `pruneContainers`.
- Produces `@/lib/ports`:
  - `portLinks(ports: PortMapping[], hostname: string): PortLink[]`, where `PortLink` is `{ label, href? }`.
  - There is one entry per published port, labelled `8080→80`, or `5353→53/udp` for other protocols. A TCP port gets `href`. A port bound to all addresses links to `hostname`; IPv6 addresses are bracketed.
  - When nothing is published, it returns the exposed ports as `80/tcp`.
- Produces `@/lib/bulk`:
  - `runBulk(items, name, fn): Promise<BulkResult>`, where `BulkResult` is `{ ok, failed: { name, error }[] }`.
  - `bulkMessage(done, noun, result)`.
- Produces `@/lib/format`: `formatAgo(unix, now?)` and `shortId(id)`.
- Produces `@/components/StateBadge`: `<StateBadge state />`.
- Produces `pages/containers/actions.tsx`:
  - `ACTIONS: { action, label, done }[]`.
  - `<RemoveDialog env containers={{id,name}[]} onDone(removed) />`, with `force` and `volumes` checkboxes.
- Produces `@/test/docker`: `container({ id, name, ...over }): ContainerSummary`.
- Produces: the route `/envs/$env/containers` (id `/authed/shell/envs/$env/containers`), and a `Containers` item in `NAV`.

- [ ] **Step 1: Write the failing tests**

`web/src/test/docker.ts`:

```ts
import type { ContainerSummary } from "@/api/gen/types.gen";

/** A container list entry: running nginx, created two hours ago, unless overridden. */
export function container(over: Partial<ContainerSummary> & { id: string; name: string }): ContainerSummary {
  return {
    image: "nginx:1.27",
    image_id: "sha256:img",
    created: Math.floor(Date.now() / 1000) - 7200,
    labels: {},
    ports: [],
    state: "running",
    status: "Up 2 hours",
    volumes: [],
    ...over,
  };
}
```

`web/src/lib/containers.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { ApiError } from "@/api/client";
import { bulkMessage, runBulk } from "./bulk";
import { formatAgo, shortId } from "./format";
import { portLinks } from "./ports";

describe("portLinks", () => {
  it("links published TCP ports to the UI's host, once per port", () => {
    const links = portLinks(
      [
        { ip: "0.0.0.0", private_port: 80, public_port: 8080, protocol: "tcp" },
        { ip: "::", private_port: 80, public_port: 8080, protocol: "tcp" },
        { private_port: 443, protocol: "tcp" },
      ],
      "docker.lan",
    );
    expect(links).toEqual([{ label: "8080→80", href: "http://docker.lan:8080" }]);
  });

  it("lists exposed ports when none is published", () => {
    expect(portLinks([{ private_port: 443, protocol: "tcp" }, { private_port: 53, protocol: "udp" }], "h")).toEqual([
      { label: "443/tcp" },
      { label: "53/udp" },
    ]);
  });

  it("uses a specific bind address and leaves UDP unlinked", () => {
    expect(
      portLinks(
        [
          { ip: "127.0.0.1", private_port: 5432, public_port: 5432, protocol: "tcp" },
          { ip: "0.0.0.0", private_port: 53, public_port: 5353, protocol: "udp" },
          { ip: "::1", private_port: 6379, public_port: 6379, protocol: "tcp" },
        ],
        "h",
      ),
    ).toEqual([
      { label: "5432→5432", href: "http://127.0.0.1:5432" },
      { label: "5353→53/udp" },
      { label: "6379→6379", href: "http://[::1]:6379" },
    ]);
  });

  it("does not repeat a published port as unpublished", () => {
    expect(
      portLinks(
        [
          { private_port: 80, protocol: "tcp" },
          { ip: "0.0.0.0", private_port: 80, public_port: 8080, protocol: "tcp" },
        ],
        "h",
      ),
    ).toEqual([{ label: "8080→80", href: "http://h:8080" }]);
  });
});

describe("runBulk", () => {
  it("runs every item and names the failures", async () => {
    const r = await runBulk(["web", "db", "cache"], (n) => n, async (n) => {
      if (n === "db") throw new ApiError(409, { code: "conflict", message: "is not running" });
    });
    expect(r.ok).toBe(2);
    expect(bulkMessage("Stopped", "container", r)).toBe("Stopped 2 of 3 containers. db: is not running");
    expect(bulkMessage("Stopped", "container", { ok: 1, failed: [] })).toBe("Stopped 1 container");
  });
});

describe("formatAgo", () => {
  const now = 1_790_000_000_000;
  it("says how long ago", () => {
    expect(formatAgo(now / 1000 - 5, now)).toBe("just now");
    expect(formatAgo(now / 1000 - 60, now)).toBe("1 minute ago");
    expect(formatAgo(now / 1000 - 7200, now)).toBe("2 hours ago");
    expect(formatAgo(now / 1000 - 3 * 86400, now)).toBe("3 days ago");
    expect(formatAgo(now / 1000 - 90 * 86400, now)).toBe(new Date(now - 90 * 86400_000).toLocaleDateString());
  });
});

describe("shortId", () => {
  it("drops the digest prefix and keeps 12 characters", () => {
    expect(shortId("sha256:0123456789abcdef0123")).toBe("0123456789ab");
    expect(shortId("fedcba9876543210")).toBe("fedcba987654");
  });
});
```

`web/src/pages/containers/ContainersPage.test.tsx`:

```tsx
import { waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { json, mockFetch } from "@/test/fetch";
import { loggedIn, renderApp } from "@/test/app";
import { container } from "@/test/docker";

const LIST = [
  container({
    id: "c1",
    name: "web",
    ports: [{ ip: "0.0.0.0", private_port: 80, public_port: 8080, protocol: "tcp" }],
    compose_project: "shop",
  }),
  container({ id: "c2", name: "db", image: "postgres:17", state: "exited", status: "Exited (0) 1 hour ago" }),
];

const rows = (screen: typeof import("@testing-library/react").screen) =>
  within(screen.getByRole("table", { name: "Containers" })).getAllByRole("row").slice(1);

describe("containers list", () => {
  it("lists containers with state, ports and age", async () => {
    mockFetch({ ...loggedIn, "GET /api/v1/envs/local/containers": { json: LIST } });
    const { screen } = renderApp("/envs/local/containers");
    await waitFor(() => expect(rows(screen)).toHaveLength(2));
    const web = rows(screen).find((r) => r.textContent?.includes("web"))!;
    expect(within(web).getByText("running")).toBeInTheDocument();
    expect(within(web).getByRole("link", { name: "8080→80" })).toHaveAttribute("href", "http://localhost:8080");
    expect(within(web).getByText("2 hours ago")).toBeInTheDocument();
    // Sorted by name: db first.
    expect(rows(screen)[0]).toHaveTextContent("db");
  });

  it("stops the selected containers and reports a failure by name", async () => {
    const calls = mockFetch({
      ...loggedIn,
      "GET /api/v1/envs/local/containers": { json: LIST },
      "POST /api/v1/envs/local/containers/c1/stop": { status: 204 },
      "POST /api/v1/envs/local/containers/c2/stop": () =>
        json(409, { code: "conflict", message: "container is not running" }),
    });
    const { screen } = renderApp("/envs/local/containers");
    await waitFor(() => expect(rows(screen)).toHaveLength(2));
    await userEvent.click(screen.getByRole("checkbox", { name: "Select all" }));
    await userEvent.click(within(screen.getByRole("group", { name: "Selected" })).getByRole("button", { name: "Stop" }));
    expect(await screen.findByText("Stopped 1 of 2 containers. db: container is not running")).toBeInTheDocument();
    expect(calls.filter((c) => c.method === "POST").map((c) => c.path).sort()).toEqual([
      "/api/v1/envs/local/containers/c1/stop",
      "/api/v1/envs/local/containers/c2/stop",
    ]);
    expect(calls.filter((c) => c.path === "/api/v1/envs/local/containers").length).toBeGreaterThan(1);
  });

  it("removes containers with force after confirming", async () => {
    const calls = mockFetch({
      ...loggedIn,
      "GET /api/v1/envs/local/containers": { json: LIST },
      "DELETE /api/v1/envs/local/containers/c1": { status: 204 },
    });
    const { screen } = renderApp("/envs/local/containers");
    await waitFor(() => expect(rows(screen)).toHaveLength(2));
    await userEvent.click(screen.getByRole("checkbox", { name: "Select c1" }));
    await userEvent.click(within(screen.getByRole("group", { name: "Selected" })).getByRole("button", { name: "Remove" }));
    const dialog = await screen.findByRole("dialog", { name: "Remove web?" });
    await userEvent.click(within(dialog).getByLabelText("Force (stop running containers first)"));
    await userEvent.click(within(dialog).getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(calls.find((c) => c.method === "DELETE")?.path).toBe("/api/v1/envs/local/containers/c1?force=true&volumes=false");
    expect(await screen.findByText("Removed 1 container")).toBeInTheDocument();
  });

  it("keeps the remove dialog open with Docker's error", async () => {
    mockFetch({
      ...loggedIn,
      "GET /api/v1/envs/local/containers": { json: LIST },
      "DELETE /api/v1/envs/local/containers/c1": () =>
        json(409, { code: "conflict", message: "cannot remove a running container" }),
    });
    const { screen } = renderApp("/envs/local/containers");
    await waitFor(() => expect(rows(screen)).toHaveLength(2));
    await userEvent.click(screen.getByRole("checkbox", { name: "Select c1" }));
    await userEvent.click(within(screen.getByRole("group", { name: "Selected" })).getByRole("button", { name: "Remove" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.click(within(dialog).getByRole("button", { name: "Remove" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("cannot remove a running container");
  });

  it("prunes stopped containers", async () => {
    mockFetch({
      ...loggedIn,
      "GET /api/v1/envs/local/containers": { json: LIST },
      "POST /api/v1/envs/local/containers/prune": { json: { deleted: ["c2"], space_reclaimed: 2_000_000 } },
    });
    const { screen } = renderApp("/envs/local/containers");
    await userEvent.click(await screen.findByRole("button", { name: "Prune" }));
    await userEvent.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Prune" }));
    expect(await screen.findByText("Removed 1 containers, reclaimed 2.0 MB")).toBeInTheDocument();
  });

  it("refreshes when Docker reports a container change", async () => {
    let list = LIST;
    mockFetch({ ...loggedIn, "GET /api/v1/envs/local/containers": () => json(200, list) });
    const { screen } = renderApp("/envs/local/containers");
    await waitFor(() => expect(rows(screen)).toHaveLength(2));
    list = [...LIST, container({ id: "c3", name: "cache" })];
    const { act } = await import("@testing-library/react");
    const { FakeWebSocket } = await import("@/test/fake-ws");
    const sock = FakeWebSocket.latest();
    act(() => sock.open());
    const sub = sock.frames().find((f) => f.topic === "events").sub;
    act(() => sock.receive({ sub, type: "data", data: { kind: "container", action: "create", id: "c3", attributes: {}, time: 1 } }));
    await waitFor(() => expect(rows(screen)).toHaveLength(3));
  });
});
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cd web && pnpm test src/lib/containers.test.ts src/pages/containers`
Expected: FAIL, 2 files. `containers.test.ts` fails with `Failed to resolve import "./bulk"`, and all 6 page tests fail with `Unable to find role="table" and name "Containers"` (or `"Prune"`).

- [ ] **Step 3: Implement the helpers**

`web/src/lib/ports.ts`:

```ts
import type { PortMapping } from "@/api/gen/types.gen";

export interface PortLink {
  label: string;
  /** Set for published TCP ports: where a browser can reach the port. */
  href?: string;
}

const ANY = ["0.0.0.0", "::", ""];

/**
 * A container's ports as the list shows them: one per published port
 * (Docker reports IPv4 and IPv6 bindings of the same port separately), or,
 * when nothing is published, the exposed ports. A port bound to all
 * addresses links to `hostname`, the host the UI was loaded from.
 */
export function portLinks(ports: PortMapping[], hostname: string): PortLink[] {
  const published = new Map<string, PortLink>();
  const exposed = new Map<string, PortLink>();
  for (const p of ports) {
    if (p.public_port == null) {
      const label = `${p.private_port}/${p.protocol}`;
      exposed.set(label, { label });
      continue;
    }
    const any = ANY.includes(p.ip ?? "");
    const host = any ? hostname : p.ip!;
    const key = `${any ? "*" : host}:${p.public_port}/${p.protocol}`;
    const label = `${p.public_port}→${p.private_port}${p.protocol === "tcp" ? "" : `/${p.protocol}`}`;
    const urlHost = host.includes(":") ? `[${host}]` : host;
    published.set(key, p.protocol === "tcp" ? { label, href: `http://${urlHost}:${p.public_port}` } : { label });
  }
  return [...(published.size ? published : exposed).values()];
}
```

`web/src/lib/bulk.ts`:

```ts
import { errorMessage } from "./errors";

export interface BulkResult {
  ok: number;
  failed: { name: string; error: unknown }[];
}

/** Runs `fn` on every item at once and collects which ones failed. */
export async function runBulk<T>(items: T[], name: (item: T) => string, fn: (item: T) => Promise<unknown>) {
  const results = await Promise.allSettled(items.map(fn));
  const out: BulkResult = { ok: 0, failed: [] };
  results.forEach((r, i) => {
    if (r.status === "fulfilled") out.ok++;
    else out.failed.push({ name: name(items[i]), error: r.reason });
  });
  return out;
}

/** "Stopped 3 containers", or "Stopped 2 of 3 containers. web: is not running". */
export function bulkMessage(done: string, noun: string, r: BulkResult) {
  const total = r.ok + r.failed.length;
  const what = `${total} ${noun}${total === 1 ? "" : "s"}`;
  if (r.failed.length === 0) return `${done} ${what}`;
  const errors = r.failed.map((f) => `${f.name}: ${errorMessage(f.error)}`).join("; ");
  return `${done} ${r.ok} of ${what}. ${errors}`;
}
```

`web/src/lib/format.ts`:

```diff
--- a/web/src/lib/format.ts
+++ b/web/src/lib/format.ts
@@ -15,3 +15,19 @@ export function formatBytes(n: number | null | undefined): string {
 export function formatTime(unix: number): string {
   return new Date(unix * 1000).toLocaleString();
 }
+
+/** How long ago a Unix-seconds time was: "just now", "5 minutes ago", "3 days ago"; older than a month, the date. */
+export function formatAgo(unix: number, now = Date.now()): string {
+  const s = Math.max(0, Math.round(now / 1000 - unix));
+  const ago = (n: number, unit: string) => `${n} ${unit}${n === 1 ? "" : "s"} ago`;
+  if (s < 60) return "just now";
+  if (s < 3600) return ago(Math.floor(s / 60), "minute");
+  if (s < 86400) return ago(Math.floor(s / 3600), "hour");
+  if (s < 30 * 86400) return ago(Math.floor(s / 86400), "day");
+  return new Date(unix * 1000).toLocaleDateString();
+}
+
+/** A Docker id as the CLI shows it: 12 hex characters, without the `sha256:` prefix. */
+export function shortId(id: string): string {
+  return id.replace(/^sha256:/, "").slice(0, 12);
+}
```

`web/src/components/StateBadge.tsx`:

```tsx
import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";

const TONE: Record<string, string> = {
  running: "bg-emerald-600 text-white",
  healthy: "bg-emerald-600 text-white",
  paused: "bg-amber-500 text-white",
  restarting: "bg-amber-500 text-white",
  partial: "bg-amber-500 text-white",
  unhealthy: "bg-destructive text-white",
  dead: "bg-destructive text-white",
};

/** A container's (or compose project's) state as a colored badge. */
export function StateBadge({ state }: { state: string }) {
  return (
    <Badge variant={TONE[state] ? "default" : "outline"} className={cn(TONE[state])}>
      {state}
    </Badge>
  );
}
```

- [ ] **Step 4: Implement the page**

`web/src/pages/containers/actions.tsx`:

```tsx
import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { toast } from "sonner";
import { keys } from "@/api/queries";
import { deleteContainer } from "@/api/gen/sdk.gen";
import type { ContainerActionName } from "@/api/gen/types.gen";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { Checkbox } from "@/components/ui/checkbox";
import { Label } from "@/components/ui/label";
import { bulkMessage, runBulk } from "@/lib/bulk";

/** What each bulk action is called before and after it runs. */
export const ACTIONS: { action: ContainerActionName; label: string; done: string }[] = [
  { action: "start", label: "Start", done: "Started" },
  { action: "stop", label: "Stop", done: "Stopped" },
  { action: "restart", label: "Restart", done: "Restarted" },
  { action: "kill", label: "Kill", done: "Killed" },
  { action: "pause", label: "Pause", done: "Paused" },
  { action: "unpause", label: "Unpause", done: "Unpaused" },
];

/** Removes the chosen containers, with Docker's `force` and `volumes` options. */
export function RemoveDialog({
  env,
  containers,
  onDone,
}: {
  env: string;
  containers: { id: string; name: string }[];
  onDone: (removed: boolean) => void;
}) {
  const qc = useQueryClient();
  const [force, setForce] = useState(false);
  const [volumes, setVolumes] = useState(false);
  const one = containers.length === 1;
  return (
    <ConfirmDialog
      open
      onOpenChange={(open) => !open && onDone(false)}
      title={one ? `Remove ${containers[0].name}?` : `Remove ${containers.length} containers?`}
      description={one ? undefined : containers.map((c) => c.name).join(", ")}
      confirmLabel="Remove"
      onConfirm={async () => {
        const r = await runBulk(containers, (c) => c.name, (c) =>
          deleteContainer({ path: { env, id: c.id }, query: { force, volumes } }),
        );
        await qc.invalidateQueries({ queryKey: keys.containers(env) });
        if (r.ok === 0) throw one ? r.failed[0].error : new Error(bulkMessage("Removed", "container", r));
        const msg = bulkMessage("Removed", "container", r);
        if (r.failed.length) toast.error(msg);
        else toast.success(msg);
        onDone(r.ok > 0);
      }}
    >
      <div className="grid gap-3">
        <div className="flex items-center gap-2">
          <Checkbox id="rm-force" checked={force} onCheckedChange={(v) => setForce(!!v)} />
          <Label htmlFor="rm-force">Force (stop running containers first)</Label>
        </div>
        <div className="flex items-center gap-2">
          <Checkbox id="rm-volumes" checked={volumes} onCheckedChange={(v) => setVolumes(!!v)} />
          <Label htmlFor="rm-volumes">Also remove anonymous volumes</Label>
        </div>
      </div>
    </ConfirmDialog>
  );
}
```

`web/src/pages/containers/ContainersPage.tsx`:

```tsx
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useParams } from "@tanstack/react-router";
import { useState } from "react";
import { toast } from "sonner";
import { containersQuery, keys } from "@/api/queries";
import { containerAction, pruneContainers } from "@/api/gen/sdk.gen";
import type { ContainerSummary } from "@/api/gen/types.gen";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { columns, DataTable, type Column } from "@/components/DataTable";
import { Page } from "@/components/Page";
import { StateBadge } from "@/components/StateBadge";
import { Button } from "@/components/ui/button";
import { bulkMessage, runBulk } from "@/lib/bulk";
import { formatAgo, formatBytes, shortId } from "@/lib/format";
import { portLinks } from "@/lib/ports";
import { ACTIONS, RemoveDialog } from "./actions";

const col = columns<ContainerSummary>();

function containerColumns(): Column<ContainerSummary>[] {
  return [
    col.accessor("name", {
      header: "Name",
      enableHiding: false,
      cell: (c) => <span className="font-medium">{c.getValue()}</span>,
    }),
    col.accessor("image", {
      header: "Image",
      // An image with no tag left is listed by its digest.
      cell: (c) => (
        <span className="block max-w-64 truncate">
          {c.getValue().startsWith("sha256:") ? shortId(c.getValue()) : c.getValue()}
        </span>
      ),
    }),
    col.accessor("state", { header: "State", cell: (c) => <StateBadge state={c.getValue()} /> }),
    col.accessor("status", { header: "Status" }),
    col.accessor((c) => c.ports.map((p) => `${p.public_port ?? ""} ${p.private_port}`).join(" "), {
      id: "ports",
      header: "Ports",
      enableSorting: false,
      cell: (c) => <Ports container={c.row.original} />,
    }),
    col.accessor("created", { header: "Created", cell: (c) => formatAgo(c.getValue()) }),
    col.accessor((c) => c.compose_project ?? "", { id: "compose", header: "Compose project" }),
  ];
}

function Ports({ container }: { container: ContainerSummary }) {
  const links = portLinks(container.ports, window.location.hostname);
  return (
    <span className="flex flex-wrap gap-x-2">
      {links.map((l) =>
        l.href ? (
          <a key={l.label} href={l.href} target="_blank" rel="noreferrer" className="text-primary hover:underline">
            {l.label}
          </a>
        ) : (
          <span key={l.label} className="text-muted-foreground">
            {l.label}
          </span>
        ),
      )}
    </span>
  );
}

export function ContainersPage() {
  const { env } = useParams({ from: "/authed/shell/envs/$env/containers" });
  const qc = useQueryClient();
  const list = useQuery(containersQuery(env));
  const [cols] = useState(() => containerColumns());
  const [removing, setRemoving] = useState<ContainerSummary[] | null>(null);
  const [pruning, setPruning] = useState(false);
  const refresh = () => qc.invalidateQueries({ queryKey: keys.containers(env) });

  const bulk = useMutation({
    mutationFn: async ({ items, action }: { items: ContainerSummary[]; action: (typeof ACTIONS)[number] }) => {
      const r = await runBulk(items, (c) => c.name, (c) =>
        containerAction({ path: { env, id: c.id, action: action.action } }),
      );
      const msg = bulkMessage(action.done, "container", r);
      if (r.failed.length) toast.error(msg);
      else toast.success(msg);
    },
    onSettled: refresh,
  });

  return (
    <Page title="Containers" actions={<Button variant="outline" onClick={() => setPruning(true)}>Prune</Button>}>
      <DataTable
        label="Containers"
        data={list.data}
        error={list.error}
        columns={cols}
        getRowId={(c) => c.id}
        empty="No containers in this environment."
        initialSorting={[{ id: "name", desc: false }]}
        initialHidden={["compose"]}
        selectable
        actions={(selected, clear) => (
          <>
            {ACTIONS.map((a) => (
              <Button
                key={a.action}
                variant="outline"
                size="sm"
                disabled={bulk.isPending}
                onClick={() => bulk.mutate({ items: selected, action: a }, { onSuccess: clear })}
              >
                {a.label}
              </Button>
            ))}
            <Button variant="destructive" size="sm" onClick={() => setRemoving(selected)}>
              Remove
            </Button>
          </>
        )}
      />
      {removing && <RemoveDialog env={env} containers={removing} onDone={() => setRemoving(null)} />}
      <ConfirmDialog
        open={pruning}
        onOpenChange={setPruning}
        title="Remove all stopped containers?"
        confirmLabel="Prune"
        onConfirm={async () => {
          const { data } = await pruneContainers({ path: { env } });
          toast.success(`Removed ${data.deleted.length} containers, reclaimed ${formatBytes(data.space_reclaimed)}`);
          await refresh();
        }}
      />
    </Page>
  );
}
```

`web/src/router.tsx`:

```diff
--- a/web/src/router.tsx
+++ b/web/src/router.tsx
@@ -112,6 +112,12 @@ const dashboardRoute = createRoute({
   component: lazyRouteComponent(() => import("@/pages/DashboardPage"), "DashboardPage"),
 });
 
+const containersRoute = createRoute({
+  getParentRoute: () => envRoute,
+  path: "containers",
+  component: lazyRouteComponent(() => import("@/pages/containers/ContainersPage"), "ContainersPage"),
+});
+
 const accountRoute = createRoute({
   getParentRoute: () => shellRoute,
   path: "/account",
@@ -121,7 +127,7 @@ const accountRoute = createRoute({
 const routeTree = rootRoute.addChildren([
   setupRoute,
   loginRoute,
-  authedRoute.addChildren([indexRoute, shellRoute.addChildren([envRoute.addChildren([dashboardRoute]), accountRoute])]),
+  authedRoute.addChildren([indexRoute, shellRoute.addChildren([envRoute.addChildren([dashboardRoute, containersRoute]), accountRoute])]),
 ]);
 
 export function createAppRouter(queryClient: QueryClient, history?: RouterHistory) {
```

`web/src/components/Shell.tsx`: add the nav item. Resource pages stay highlighted on their detail pages, so only the dashboard link matches exactly.

```diff
--- a/web/src/components/Shell.tsx
+++ b/web/src/components/Shell.tsx
@@ -1,6 +1,6 @@
 import { useQuery } from "@tanstack/react-query";
 import { Link, Outlet, useNavigate, useParams } from "@tanstack/react-router";
-import { ChevronsUpDown, LayoutDashboard, LogOut, Monitor, Moon, Sun, User } from "lucide-react";
+import { Boxes, ChevronsUpDown, LayoutDashboard, LogOut, Monitor, Moon, Sun, User } from "lucide-react";
 import { envsQuery, envStatusQuery } from "@/api/queries";
 import { useLogout, useSession } from "@/auth/session";
 import { Badge } from "@/components/ui/badge";
@@ -18,8 +18,11 @@ import {
 import { useEnvEvents, useLive } from "@/live/LiveProvider";
 import { useTheme, type Theme } from "@/theme";
 
-/** The environment-scoped pages, in sidebar order. P3b adds the resource pages. */
-const NAV = [{ to: "/envs/$env", label: "Dashboard", icon: LayoutDashboard }] as const;
+/** The environment-scoped pages, in sidebar order. */
+const NAV = [
+  { to: "/envs/$env", label: "Dashboard", icon: LayoutDashboard },
+  { to: "/envs/$env/containers", label: "Containers", icon: Boxes },
+] as const;
 
 export function Shell() {
   const { env } = useParams({ strict: false });
@@ -37,7 +40,7 @@ export function Shell() {
                 key={to}
                 to={to}
                 params={{ env }}
-                activeOptions={{ exact: true }}
+                activeOptions={{ exact: to === "/envs/$env" }}
                 className="flex items-center gap-2 rounded-md px-2 py-1.5 text-sm hover:bg-accent data-[status=active]:bg-accent data-[status=active]:font-medium"
               >
                 <Icon className="size-4" />
```

- [ ] **Step 5: Run all checks**

Run: `cd web && pnpm test && pnpm typecheck && pnpm lint`
Expected: 92 passed (16 files); typecheck and lint are clean.

- [ ] **Step 6: Commit**

```bash
git add web/src
git commit -m "feat(web): containers list with ports, bulk actions, remove and prune"
```

---

### Task 3: The container page

**Files:**
- Create:
  - `web/src/api/docker.ts`;
  - `web/src/components/Details.tsx` and `web/src/components/JsonView.tsx`;
  - `web/src/lib/secrets.ts`;
  - `web/src/pages/containers/ContainerPage.tsx` and `web/src/pages/containers/tabs.ts`.
- Modify:
  - `web/src/api/queries.ts`, `web/src/router.tsx` and `web/src/test/docker.ts`;
  - `web/src/pages/containers/ContainersPage.tsx`: names link to the page;
  - `web/src/pages/containers/actions.tsx`: `actionsFor`.
- Test: `web/src/lib/secrets.test.ts` and `web/src/pages/containers/ContainerPage.test.tsx`.

**Interfaces:**
- Consumes: Task 2's `ACTIONS`, `RemoveDialog`, `StateBadge` and `shortId`, and Task 1's `Page` and `Tabs`.
- Produces `@/api/docker`: `ContainerInspect`, the inspect fields the page reads. Tasks 6, 8 and 9 add `ImageInspect`, `ImageLayer`, `VolumeInspect` and `NetworkInspect` here.
- Produces `@/api/queries`:
  - `containerQuery(env, id)`, under `keys.container`.
  - `retryUnlessGone`, which is module-private: every detail query uses it, so a 404 is not retried.
- Produces `@/components/Details`:
  - `<Details items={[label, node][]} label? />`, which leaves out empty values.
  - `<DetailError error what back />`, which shows "This <what> no longer exists." for a 404 and `errorMessage` otherwise.
- Produces `@/components/JsonView`: `<JsonView value label />`, with a Copy button.
- Produces `@/lib/secrets`: `isSecretKey(key)` and `splitEnv(entry)`.
- Produces `pages/containers/actions.tsx`: `actionsFor(state): ContainerActionName[]`.
- Produces `pages/containers/tabs.ts`: `CONTAINER_TABS` and `ContainerTab`, which the router imports without pulling in the page.
- Produces:
  - the route `/envs/$env/containers/$id`, with `?tab=`;
  - `ContainerPage`, and `EnvTable` (exported for reuse);
  - `@/test/docker`'s `inspect(over)`.
- Accessible names: `Container details` (the overview `dl`), `Labels`, the `Environment`, `Mounts` and `Networks` tables, and `Container JSON`.

- [ ] **Step 1: Write the failing tests**

`web/src/test/docker.ts`: add the inspect fixture.

```diff
--- a/web/src/test/docker.ts
+++ b/web/src/test/docker.ts
@@ -1,4 +1,5 @@
 import type { ContainerSummary } from "@/api/gen/types.gen";
+import type { ContainerInspect } from "@/api/docker";
 
 /** A container list entry: running nginx, created two hours ago, unless overridden. */
 export function container(over: Partial<ContainerSummary> & { id: string; name: string }): ContainerSummary {
@@ -14,3 +15,16 @@ export function container(over: Partial<ContainerSummary> & { id: string; name:
     ...over,
   };
 }
+
+/** Docker's inspect body for a container, trimmed to what the UI reads. */
+export function inspect(over: Partial<ContainerInspect> & { Id: string; Name: string }): ContainerInspect {
+  return {
+    Created: "2026-09-30T10:00:00Z",
+    State: { Status: "running", Running: true, StartedAt: "2026-09-30T10:00:01Z", ExitCode: 0 },
+    Config: { Image: "nginx:1.27", Env: [], Cmd: ["nginx", "-g", "daemon off;"], Labels: {} },
+    HostConfig: { RestartPolicy: { Name: "unless-stopped" } },
+    Mounts: [],
+    NetworkSettings: { Ports: {}, Networks: {} },
+    ...over,
+  };
+}
```

`web/src/lib/secrets.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { isSecretKey, splitEnv } from "./secrets";

describe("isSecretKey", () => {
  it("matches password, secret, token and key names in any case", () => {
    for (const k of ["POSTGRES_PASSWORD", "db_pass", "AWS_SECRET_ACCESS_KEY", "GITHUB_TOKEN", "apiKey"]) {
      expect(isSecretKey(k)).toBe(true);
    }
    for (const k of ["PATH", "HOME", "NGINX_VERSION"]) expect(isSecretKey(k)).toBe(false);
  });
});

describe("splitEnv", () => {
  it("splits at the first equals sign", () => {
    expect(splitEnv("A=b=c")).toEqual(["A", "b=c"]);
    expect(splitEnv("EMPTY=")).toEqual(["EMPTY", ""]);
    expect(splitEnv("BARE")).toEqual(["BARE", ""]);
  });
});
```

`web/src/pages/containers/ContainerPage.test.tsx`:

```tsx
import { act, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { json, mockFetch } from "@/test/fetch";
import { FakeWebSocket } from "@/test/fake-ws";
import { loggedIn, renderApp } from "@/test/app";
import { container, inspect } from "@/test/docker";
import { actionsFor } from "./actions";

const WEB = inspect({
  Id: "c1abcdef0123456789",
  Name: "/web",
  Config: {
    Image: "nginx:1.27",
    Cmd: ["nginx", "-g", "daemon off;"],
    Env: ["PATH=/usr/bin", "DB_PASSWORD=hunter2", "GREETING=a=b"],
    Labels: { "com.docker.compose.project": "shop" },
  },
  Mounts: [{ Type: "volume", Name: "data", Source: "/var/lib/docker/volumes/data/_data", Destination: "/data", RW: true }],
  NetworkSettings: {
    Ports: { "80/tcp": [{ HostIp: "0.0.0.0", HostPort: "8080" }] },
    Networks: { bridge: { IPAddress: "172.17.0.2", Gateway: "172.17.0.1", MacAddress: "02:42:ac:11:00:02" } },
  },
});

const routes = { ...loggedIn, "GET /api/v1/envs/local/containers/c1": { json: WEB } };

describe("container page", () => {
  it("shows the overview: state, ports, restart policy and labels", async () => {
    mockFetch(routes);
    const { screen } = renderApp("/envs/local/containers/c1");
    expect(await screen.findByRole("heading", { name: /web/ })).toHaveTextContent("running");
    const overview = screen.getByLabelText("Container details");
    expect(within(overview).getByText("c1abcdef0123")).toBeInTheDocument();
    expect(within(overview).getByText("0.0.0.0:8080 → 80/tcp")).toBeInTheDocument();
    expect(within(overview).getByText("unless-stopped")).toBeInTheDocument();
    expect(within(screen.getByLabelText("Labels")).getByText("shop")).toBeInTheDocument();
  });

  it("opens from the containers list", async () => {
    mockFetch({ ...routes, "GET /api/v1/envs/local/containers": { json: [container({ id: "c1", name: "web" })] } });
    const { screen, history } = renderApp("/envs/local/containers");
    await userEvent.click(await screen.findByRole("link", { name: "web" }));
    expect(history.location.pathname).toBe("/envs/local/containers/c1");
    expect(await screen.findByRole("heading", { name: /web/ })).toBeInTheDocument();
  });

  it("masks secret-looking env values until clicked", async () => {
    mockFetch(routes);
    const { screen } = renderApp("/envs/local/containers/c1?tab=env");
    const table = await screen.findByRole("table", { name: "Environment" });
    expect(within(table).getByText("/usr/bin")).toBeInTheDocument();
    expect(within(table).getByText("a=b")).toBeInTheDocument();
    expect(within(table).queryByText("hunter2")).not.toBeInTheDocument();
    await userEvent.click(within(table).getByRole("button", { name: "Show DB_PASSWORD" }));
    expect(within(table).getByText("hunter2")).toBeInTheDocument();
  });

  it("switches tabs through the address", async () => {
    mockFetch(routes);
    const { screen, history } = renderApp("/envs/local/containers/c1");
    await screen.findByRole("heading", { name: /web/ });
    await userEvent.click(screen.getByRole("tab", { name: "Mounts" }));
    expect(history.location.search).toBe("?tab=mounts");
    const mounts = screen.getByRole("table", { name: "Mounts" });
    expect(within(mounts).getByText("data")).toBeInTheDocument();
    expect(within(mounts).getByText("read-write")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("tab", { name: "Networks" }));
    expect(within(screen.getByRole("table", { name: "Networks" })).getByText("172.17.0.2")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("tab", { name: "Inspect" }));
    expect(screen.getByLabelText("Container JSON")).toHaveTextContent('"Id": "c1abcdef0123456789"');
  });

  it("offers the actions that fit the state", () => {
    expect(actionsFor("running")).toEqual(["stop", "restart", "kill", "pause"]);
    expect(actionsFor("paused")).toEqual(["unpause", "kill"]);
    expect(actionsFor("exited")).toEqual(["start"]);
    expect(actionsFor("created")).toEqual(["start"]);
  });

  it("stops the container and follows its new state", async () => {
    let state = WEB;
    const calls = mockFetch({
      ...routes,
      "GET /api/v1/envs/local/containers/c1": () => json(200, state),
      "POST /api/v1/envs/local/containers/c1abcdef0123456789/stop": () => {
        state = { ...WEB, State: { Status: "exited", ExitCode: 0 } };
        return { status: 204 };
      },
    });
    const { screen } = renderApp("/envs/local/containers/c1");
    await userEvent.click(await screen.findByRole("button", { name: "Stop" }));
    expect(await screen.findByRole("button", { name: "Start" })).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: /web/ })).toHaveTextContent("exited");
    expect(calls.some((c) => c.method === "POST")).toBe(true);
  });

  it("renames the container, showing Docker's error inline", async () => {
    let tries = 0;
    const calls = mockFetch({
      ...routes,
      "POST /api/v1/envs/local/containers/c1abcdef0123456789/rename": () =>
        tries++ === 0 ? json(409, { code: "conflict", message: "name web2 is already in use" }) : { status: 204 },
    });
    const { screen } = renderApp("/envs/local/containers/c1");
    await userEvent.click(await screen.findByRole("button", { name: "Rename" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.clear(within(dialog).getByLabelText("New name"));
    await userEvent.type(within(dialog).getByLabelText("New name"), "web2");
    await userEvent.click(within(dialog).getByRole("button", { name: "Rename" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("name web2 is already in use");
    await userEvent.click(within(dialog).getByRole("button", { name: "Rename" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(calls.filter((c) => c.path.endsWith("/rename")).at(-1)?.body).toEqual({ name: "web2" });
  });

  it("goes back to the list after removing the container", async () => {
    mockFetch({
      ...routes,
      "DELETE /api/v1/envs/local/containers/c1abcdef0123456789": { status: 204 },
      "GET /api/v1/envs/local/containers": { json: [] },
    });
    const { screen, history } = renderApp("/envs/local/containers/c1");
    await userEvent.click(await screen.findByRole("button", { name: "Remove" }));
    await userEvent.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(history.location.pathname).toBe("/envs/local/containers"));
  });

  it("says so when the container is removed elsewhere", async () => {
    let gone = false;
    mockFetch({
      ...routes,
      "GET /api/v1/envs/local/containers/c1": () =>
        gone ? json(404, { code: "not_found", message: "no such container" }) : json(200, WEB),
    });
    const { screen } = renderApp("/envs/local/containers/c1");
    await screen.findByRole("heading", { name: /web/ });
    const sock = FakeWebSocket.latest();
    act(() => sock.open());
    const sub = sock.frames().find((f) => f.topic === "events").sub;
    gone = true;
    act(() =>
      sock.receive({ sub, type: "data", data: { kind: "container", action: "destroy", id: "c1", attributes: {}, time: 1 } }),
    );
    expect(await screen.findByRole("alert")).toHaveTextContent("This container no longer exists.");
    expect(screen.getByRole("link", { name: "Back to containers" })).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cd web && pnpm test src/lib/secrets.test.ts src/pages/containers/ContainerPage.test.tsx`
Expected: FAIL, 2 files. `secrets.test.ts` fails with `Failed to resolve import "./secrets"`. All 9 page tests fail: `actionsFor is not a function`, and `Unable to find role="heading" and name /web/` (or the `Stop`, `Rename` and `Remove` buttons).

- [ ] **Step 3: Implement the query and shared pieces**

`web/src/api/docker.ts`:

```ts
/**
 * The fields of Docker's own inspect bodies that the UI reads. The API
 * passes these through verbatim, so the generated types only say `object`.
 */

export interface ContainerInspect {
  Id: string;
  /** With Docker's leading slash: `/web`. */
  Name: string;
  Created: string;
  State: {
    Status: string;
    Running?: boolean;
    Paused?: boolean;
    ExitCode?: number;
    StartedAt?: string;
    FinishedAt?: string;
    Error?: string;
    Health?: { Status: string } | null;
  };
  Config: {
    Image: string;
    Hostname?: string;
    User?: string;
    WorkingDir?: string;
    Env?: string[] | null;
    Cmd?: string[] | null;
    Entrypoint?: string[] | null;
    Labels?: Record<string, string> | null;
  };
  HostConfig: {
    RestartPolicy?: { Name: string; MaximumRetryCount?: number };
    Privileged?: boolean;
  };
  Mounts?: {
    Type: string;
    Name?: string;
    Source: string;
    Destination: string;
    Mode?: string;
    RW: boolean;
  }[];
  NetworkSettings: {
    Ports?: Record<string, { HostIp: string; HostPort: string }[] | null> | null;
    Networks?: Record<
      string,
      { IPAddress?: string; Gateway?: string; MacAddress?: string; Aliases?: string[] | null; NetworkID?: string }
    > | null;
  };
}
```

`web/src/api/queries.ts`:

```diff
--- a/web/src/api/queries.ts
+++ b/web/src/api/queries.ts
@@ -1,11 +1,13 @@
 import { queryOptions } from "@tanstack/react-query";
-import { setCsrfToken } from "./client";
+import { ApiError, setCsrfToken } from "./client";
+import type { ContainerInspect } from "./docker";
 import {
   getAuthStatus,
   getDiskUsage,
   getEngineInfo,
   getEnvStatus,
   getMe,
+  inspectContainer,
   listContainers,
   listEnvs,
   listImages,
@@ -89,6 +91,17 @@ export const dfQuery = (env: string) =>
 export const containersQuery = (env: string) =>
   queryOptions({ queryKey: keys.containers(env), queryFn: async () => (await listContainers(path(env))).data });
 
+/** Retry a failed detail query once, but not when the resource is gone. */
+const retryUnlessGone = (failures: number, err: unknown) =>
+  !(err instanceof ApiError && err.status === 404) && failures < 1;
+
+export const containerQuery = (env: string, id: string) =>
+  queryOptions({
+    queryKey: keys.container(env, id),
+    queryFn: async () => (await inspectContainer({ path: { env, id } })).data as unknown as ContainerInspect,
+    retry: retryUnlessGone,
+  });
+
 export const imagesQuery = (env: string) =>
   queryOptions({ queryKey: keys.images(env), queryFn: async () => (await listImages(path(env))).data });
 
```

`web/src/lib/secrets.ts`:

```ts
const SECRET = /PASS|SECRET|TOKEN|KEY/i;

/** Env keys whose values the UI masks until clicked: `*PASS*`, `*SECRET*`, `*TOKEN*`, `*KEY*`. */
export function isSecretKey(key: string) {
  return SECRET.test(key);
}

/** `KEY=value` into its parts; the value may itself contain `=`. */
export function splitEnv(entry: string): [string, string] {
  const i = entry.indexOf("=");
  return i < 0 ? [entry, ""] : [entry.slice(0, i), entry.slice(i + 1)];
}
```

`web/src/components/Details.tsx`:

```tsx
import type { ReactNode } from "react";
import { ApiError } from "@/api/client";
import { errorMessage } from "@/lib/errors";

/** Label/value pairs in two columns. Empty values are left out. */
export function Details({ items, label }: { items: [string, ReactNode][]; label?: string }) {
  const shown = items.filter(([, v]) => v !== undefined && v !== null && v !== "");
  return (
    <dl aria-label={label} className="grid grid-cols-[max-content_1fr] gap-x-6 gap-y-2 text-sm">
      {shown.map(([k, v]) => (
        <div key={k} className="contents">
          <dt className="text-muted-foreground">{k}</dt>
          <dd className="min-w-0 break-words">{v}</dd>
        </div>
      ))}
    </dl>
  );
}

/** A detail page's failed query: "no longer exists" for a 404, the error otherwise. */
export function DetailError({ error, what, back }: { error: unknown; what: string; back: ReactNode }) {
  const gone = error instanceof ApiError && error.status === 404;
  return (
    <div className="grid gap-2 p-6">
      <p role="alert" className="text-sm text-destructive">
        {gone ? `This ${what} no longer exists.` : errorMessage(error)}
      </p>
      <div className="text-sm">{back}</div>
    </div>
  );
}
```

`web/src/components/JsonView.tsx`:

```tsx
import { Copy } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";

/** Raw JSON, pretty-printed, with a copy button. */
export function JsonView({ value, label }: { value: unknown; label: string }) {
  const text = JSON.stringify(value, null, 2);
  const copy = async () => {
    await navigator.clipboard.writeText(text);
    toast.success("Copied");
  };
  return (
    <div className="relative">
      <Button variant="outline" size="sm" className="absolute top-2 right-2" onClick={() => void copy()}>
        <Copy />
        Copy
      </Button>
      <pre aria-label={label} className="max-h-[70vh] overflow-auto rounded-md border bg-muted/40 p-4 font-mono text-xs">
        {text}
      </pre>
    </div>
  );
}
```

- [ ] **Step 4: Implement the page and its route**

`web/src/pages/containers/tabs.ts`:

```ts
/** The container page's tabs, in order; the router validates `?tab=` against them. */
export const CONTAINER_TABS = ["overview", "inspect", "env", "mounts", "networks"] as const;
export type ContainerTab = (typeof CONTAINER_TABS)[number];
```

`web/src/pages/containers/actions.tsx`:

```diff
--- a/web/src/pages/containers/actions.tsx
+++ b/web/src/pages/containers/actions.tsx
@@ -19,6 +19,20 @@ export const ACTIONS: { action: ContainerActionName; label: string; done: string
   { action: "unpause", label: "Unpause", done: "Unpaused" },
 ];
 
+/** The actions that make sense in the container's current state. */
+export function actionsFor(state: string): ContainerActionName[] {
+  switch (state) {
+    case "running":
+      return ["stop", "restart", "kill", "pause"];
+    case "paused":
+      return ["unpause", "kill"];
+    case "restarting":
+      return ["stop", "kill"];
+    default:
+      return ["start"];
+  }
+}
+
 /** Removes the chosen containers, with Docker's `force` and `volumes` options. */
 export function RemoveDialog({
   env,
```

`web/src/pages/containers/ContainerPage.tsx`:

```tsx
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate, useParams, useSearch } from "@tanstack/react-router";
import { useState, type FormEvent } from "react";
import { containerQuery, keys } from "@/api/queries";
import { containerAction, renameContainer } from "@/api/gen/sdk.gen";
import type { ContainerActionName } from "@/api/gen/types.gen";
import type { ContainerInspect } from "@/api/docker";
import { Details, DetailError } from "@/components/Details";
import { JsonView } from "@/components/JsonView";
import { Page } from "@/components/Page";
import { StateBadge } from "@/components/StateBadge";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { errorMessage } from "@/lib/errors";
import { shortId } from "@/lib/format";
import { isSecretKey, splitEnv } from "@/lib/secrets";
import { FormError } from "@/pages/AuthLayout";
import { ACTIONS, actionsFor, RemoveDialog } from "./actions";
import type { ContainerTab } from "./tabs";

const FROM = "/authed/shell/envs/$env/containers/$id";

export function ContainerPage() {
  const { env, id } = useParams({ from: FROM });
  const { tab = "overview" } = useSearch({ from: FROM });
  const navigate = useNavigate({ from: "/envs/$env/containers/$id" });
  const q = useQuery(containerQuery(env, id));
  const back = (
    <Link to="/envs/$env/containers" params={{ env }} className="underline">
      Back to containers
    </Link>
  );
  if (q.error) return <DetailError error={q.error} what="container" back={back} />;
  const c = q.data;
  if (!c) return <p className="p-6 text-sm text-muted-foreground">Loading…</p>;
  const name = c.Name.replace(/^\//, "");
  return (
    <Page
      title={
        <span className="flex items-center gap-3">
          {name} <StateBadge state={c.State.Health?.Status ?? c.State.Status} />
        </span>
      }
      actions={<ContainerActions env={env} c={c} name={name} />}
    >
      <Tabs value={tab} onValueChange={(t) => void navigate({ search: { tab: t as ContainerTab }, replace: true })}>
        <TabsList>
          <TabsTrigger value="overview">Overview</TabsTrigger>
          <TabsTrigger value="inspect">Inspect</TabsTrigger>
          <TabsTrigger value="env">Env</TabsTrigger>
          <TabsTrigger value="mounts">Mounts</TabsTrigger>
          <TabsTrigger value="networks">Networks</TabsTrigger>
        </TabsList>
        <TabsContent value="overview">
          <Overview c={c} />
        </TabsContent>
        <TabsContent value="inspect">
          <JsonView value={c} label="Container JSON" />
        </TabsContent>
        <TabsContent value="env">
          <EnvTable env={c.Config.Env ?? []} />
        </TabsContent>
        <TabsContent value="mounts">
          <Mounts c={c} />
        </TabsContent>
        <TabsContent value="networks">
          <Networks c={c} />
        </TabsContent>
      </Tabs>
    </Page>
  );
}

function ContainerActions({ env, c, name }: { env: string; c: ContainerInspect; name: string }) {
  const qc = useQueryClient();
  const navigate = useNavigate();
  const [renaming, setRenaming] = useState(false);
  const [removing, setRemoving] = useState(false);
  const act = useMutation({
    mutationFn: (action: ContainerActionName) => containerAction({ path: { env, id: c.Id, action } }),
    onSettled: () => qc.invalidateQueries({ queryKey: keys.containers(env) }),
  });
  return (
    <>
      {actionsFor(c.State.Status).map((a) => (
        <Button key={a} variant="outline" disabled={act.isPending} onClick={() => act.mutate(a)}>
          {ACTIONS.find((x) => x.action === a)!.label}
        </Button>
      ))}
      <Button variant="outline" onClick={() => setRenaming(true)}>
        Rename
      </Button>
      <Button variant="destructive" onClick={() => setRemoving(true)}>
        Remove
      </Button>
      {renaming && <RenameDialog env={env} id={c.Id} name={name} onClose={() => setRenaming(false)} />}
      {removing && (
        <RemoveDialog
          env={env}
          containers={[{ id: c.Id, name }]}
          onDone={(removed) => {
            setRemoving(false);
            if (removed) void navigate({ to: "/envs/$env/containers", params: { env } });
          }}
        />
      )}
    </>
  );
}

function RenameDialog({ env, id, name, onClose }: { env: string; id: string; name: string; onClose: () => void }) {
  const qc = useQueryClient();
  const [value, setValue] = useState(name);
  const m = useMutation({
    mutationFn: () => renameContainer({ path: { env, id }, body: { name: value } }),
    meta: { inline: true },
    onSuccess: async () => {
      await qc.invalidateQueries({ queryKey: keys.containers(env) });
      onClose();
    },
  });
  const submit = (e: FormEvent) => {
    e.preventDefault();
    m.mutate();
  };
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Rename {name}</DialogTitle>
        </DialogHeader>
        <form onSubmit={submit} className="grid gap-4">
          <div className="grid gap-2">
            <Label htmlFor="rename">New name</Label>
            <Input id="rename" required value={value} onChange={(e) => setValue(e.target.value)} />
          </div>
          <FormError message={m.error ? errorMessage(m.error) : null} />
          <DialogFooter>
            <Button type="submit" disabled={m.isPending}>
              Rename
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

/** A bind address as it is written before `:port`: IPv6 in brackets. */
const hostAddr = (ip: string) => (!ip ? "0.0.0.0" : ip.includes(":") ? `[${ip}]` : ip);

function Overview({ c }: { c: ContainerInspect }) {
  const restart = c.HostConfig.RestartPolicy;
  const ports = Object.entries(c.NetworkSettings.Ports ?? {}).flatMap(([port, binds]) =>
    binds?.length ? binds.map((b) => `${hostAddr(b.HostIp)}:${b.HostPort} → ${port}`) : [port],
  );
  const labels = Object.entries(c.Config.Labels ?? {});
  return (
    <div className="grid gap-6">
      <Details
        label="Container details"
        items={[
          ["ID", <code key="id">{shortId(c.Id)}</code>],
          ["Image", c.Config.Image],
          ["State", c.State.Status + (c.State.Status === "exited" ? ` (exit code ${c.State.ExitCode})` : "")],
          ["Error", c.State.Error],
          ["Started", c.State.Running ? new Date(c.State.StartedAt!).toLocaleString() : undefined],
          ["Created", new Date(c.Created).toLocaleString()],
          ["Command", [...(c.Config.Entrypoint ?? []), ...(c.Config.Cmd ?? [])].join(" ")],
          ["Restart policy", restart && restart.Name !== "no" && restart.Name ? restart.Name : "no"],
          ["Ports", ports.length ? ports.join(", ") : "none"],
        ]}
      />
      <section className="grid gap-2">
        <h2 className="font-medium">Labels</h2>
        {labels.length === 0 ? (
          <p className="text-sm text-muted-foreground">No labels.</p>
        ) : (
          <Details label="Labels" items={labels.map(([k, v]) => [k, v])} />
        )}
      </section>
    </div>
  );
}

/** The container's environment, with secret-looking values hidden until clicked. */
export function EnvTable({ env }: { env: string[] }) {
  const [shown, setShown] = useState<Set<string>>(new Set());
  if (env.length === 0) return <p className="text-sm text-muted-foreground">No environment variables.</p>;
  return (
    <Table aria-label="Environment">
      <TableHeader>
        <TableRow>
          <TableHead>Name</TableHead>
          <TableHead>Value</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {env.map(splitEnv).map(([k, v]) => (
          <TableRow key={k}>
            <TableCell className="font-mono text-xs">{k}</TableCell>
            <TableCell className="font-mono text-xs whitespace-normal break-all">
              {isSecretKey(k) && !shown.has(k) ? (
                <button
                  type="button"
                  className="text-muted-foreground hover:underline"
                  aria-label={`Show ${k}`}
                  onClick={() => setShown(new Set(shown).add(k))}
                >
                  ••••••••
                </button>
              ) : (
                v
              )}
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}

function Mounts({ c }: { c: ContainerInspect }) {
  const mounts = c.Mounts ?? [];
  if (mounts.length === 0) return <p className="text-sm text-muted-foreground">No mounts.</p>;
  return (
    <Table aria-label="Mounts">
      <TableHeader>
        <TableRow>
          <TableHead>Type</TableHead>
          <TableHead>Source</TableHead>
          <TableHead>Destination</TableHead>
          <TableHead>Mode</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {mounts.map((m) => (
          <TableRow key={m.Destination}>
            <TableCell>{m.Type}</TableCell>
            <TableCell className="font-mono text-xs">{m.Type === "volume" && m.Name ? m.Name : m.Source}</TableCell>
            <TableCell className="font-mono text-xs">{m.Destination}</TableCell>
            <TableCell>{m.RW ? "read-write" : "read-only"}</TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}

function Networks({ c }: { c: ContainerInspect }) {
  const nets = Object.entries(c.NetworkSettings.Networks ?? {});
  if (nets.length === 0) return <p className="text-sm text-muted-foreground">Not attached to any network.</p>;
  return (
    <Table aria-label="Networks">
      <TableHeader>
        <TableRow>
          <TableHead>Network</TableHead>
          <TableHead>IP address</TableHead>
          <TableHead>Gateway</TableHead>
          <TableHead>MAC address</TableHead>
          <TableHead>Aliases</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {nets.map(([name, n]) => (
          <TableRow key={name}>
            <TableCell className="font-medium">{name}</TableCell>
            <TableCell className="font-mono text-xs">{n.IPAddress || "—"}</TableCell>
            <TableCell className="font-mono text-xs">{n.Gateway || "—"}</TableCell>
            <TableCell className="font-mono text-xs">{n.MacAddress || "—"}</TableCell>
            <TableCell>{n.Aliases?.join(", ") || "—"}</TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}
```

`web/src/pages/containers/ContainersPage.tsx`: link each name to its page.

```diff
--- a/web/src/pages/containers/ContainersPage.tsx
+++ b/web/src/pages/containers/ContainersPage.tsx
@@ -1,5 +1,5 @@
 import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
-import { useParams } from "@tanstack/react-router";
+import { Link, useParams } from "@tanstack/react-router";
 import { useState } from "react";
 import { toast } from "sonner";
 import { containersQuery, keys } from "@/api/queries";
@@ -17,12 +17,20 @@ import { ACTIONS, RemoveDialog } from "./actions";
 
 const col = columns<ContainerSummary>();
 
-function containerColumns(): Column<ContainerSummary>[] {
+function containerColumns(env: string): Column<ContainerSummary>[] {
   return [
     col.accessor("name", {
       header: "Name",
       enableHiding: false,
-      cell: (c) => <span className="font-medium">{c.getValue()}</span>,
+      cell: (c) => (
+        <Link
+          to="/envs/$env/containers/$id"
+          params={{ env, id: c.row.original.id }}
+          className="font-medium hover:underline"
+        >
+          {c.getValue()}
+        </Link>
+      ),
     }),
     col.accessor("image", {
       header: "Image",
@@ -69,7 +77,7 @@ export function ContainersPage() {
   const { env } = useParams({ from: "/authed/shell/envs/$env/containers" });
   const qc = useQueryClient();
   const list = useQuery(containersQuery(env));
-  const [cols] = useState(() => containerColumns());
+  const [cols] = useState(() => containerColumns(env));
   const [removing, setRemoving] = useState<ContainerSummary[] | null>(null);
   const [pruning, setPruning] = useState(false);
   const refresh = () => qc.invalidateQueries({ queryKey: keys.containers(env) });
```

`web/src/router.tsx`:

```diff
--- a/web/src/router.tsx
+++ b/web/src/router.tsx
@@ -14,6 +14,7 @@ import { Shell } from "@/components/Shell";
 import { LiveProvider } from "@/live/LiveProvider";
 import { LoginPage } from "@/pages/LoginPage";
 import { NoEnvironmentsPage, NotFoundPage } from "@/pages/misc";
+import { CONTAINER_TABS, type ContainerTab } from "@/pages/containers/tabs";
 import { SetupPage } from "@/pages/SetupPage";
 
 export interface RouterContext {
@@ -118,6 +119,15 @@ const containersRoute = createRoute({
   component: lazyRouteComponent(() => import("@/pages/containers/ContainersPage"), "ContainersPage"),
 });
 
+const containerRoute = createRoute({
+  getParentRoute: () => envRoute,
+  path: "containers/$id",
+  validateSearch: (s: Record<string, unknown>): { tab?: ContainerTab } => ({
+    tab: CONTAINER_TABS.find((t) => t === s.tab),
+  }),
+  component: lazyRouteComponent(() => import("@/pages/containers/ContainerPage"), "ContainerPage"),
+});
+
 const accountRoute = createRoute({
   getParentRoute: () => shellRoute,
   path: "/account",
@@ -127,7 +137,7 @@ const accountRoute = createRoute({
 const routeTree = rootRoute.addChildren([
   setupRoute,
   loginRoute,
-  authedRoute.addChildren([indexRoute, shellRoute.addChildren([envRoute.addChildren([dashboardRoute, containersRoute]), accountRoute])]),
+  authedRoute.addChildren([indexRoute, shellRoute.addChildren([envRoute.addChildren([dashboardRoute, containersRoute, containerRoute]), accountRoute])]),
 ]);
 
 export function createAppRouter(queryClient: QueryClient, history?: RouterHistory) {
```

- [ ] **Step 5: Run all checks**

Run: `cd web && pnpm test && pnpm typecheck && pnpm lint`
Expected: 103 passed (18 files); typecheck and lint are clean.

- [ ] **Step 6: Commit**

```bash
git add web/src
git commit -m "feat(web): container page with overview, inspect, env, mounts and networks"
```

---

### Task 4: The container file browser

**Files:**
- Create: `web/src/lib/files.ts` and `web/src/pages/containers/FilesTab.tsx`.
- Modify: `web/src/api/queries.ts`, `web/src/pages/containers/tabs.ts`, `web/src/pages/containers/ContainerPage.tsx` and `web/src/router.tsx`.
- Test: `web/src/lib/files.test.ts` and `web/src/pages/containers/FilesTab.test.tsx`.

**Interfaces:**
- Consumes: `containerQuery` and the container page's tabs (Task 3), and the SDK's `listContainerFiles`.
- Produces `@/api/queries`:
  - `keys.containerFiles(env, id, dir)` is `["env", env, "container-files", id, dir]`. It sits outside the containers prefix on purpose: a listing downloads the directory's whole subtree, so it must not be refetched on every container event.
  - `containerFilesQuery(env, id, dir)`.
- Produces `@/lib/files`: `formatMode(mode, kind)`, `joinPath(dir, name)` and `crumbs(path)`.
- Produces `<FilesTab env id path />`, and `downloadUrl(env, id, path)`. A download is a plain GET link the browser follows with the session cookie.
- Produces: the container route accepts `?path=/absolute`, and `files` is in `CONTAINER_TABS`.

- [ ] **Step 1: Write the failing tests**

`web/src/lib/files.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { crumbs, formatMode, joinPath } from "./files";

describe("files", () => {
  it("formats permission bits like ls", () => {
    expect(formatMode(0o755, "dir")).toBe("drwxr-xr-x");
    expect(formatMode(0o644, "file")).toBe("-rw-r--r--");
    expect(formatMode(0o777, "symlink")).toBe("lrwxrwxrwx");
  });

  it("joins paths without doubling slashes", () => {
    expect(joinPath("/", "etc")).toBe("/etc");
    expect(joinPath("/etc", "hosts")).toBe("/etc/hosts");
    expect(joinPath("/etc/", "hosts")).toBe("/etc/hosts");
  });

  it("lists breadcrumbs from the root", () => {
    expect(crumbs("/")).toEqual([{ name: "/", path: "/" }]);
    expect(crumbs("/etc/nginx")).toEqual([
      { name: "/", path: "/" },
      { name: "etc", path: "/etc" },
      { name: "nginx", path: "/etc/nginx" },
    ]);
  });
});
```

`web/src/pages/containers/FilesTab.test.tsx`:

```tsx
import { within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { DirListing } from "@/api/gen/types.gen";
import { json, mockFetch } from "@/test/fetch";
import { loggedIn, renderApp } from "@/test/app";
import { inspect } from "@/test/docker";

const ROOT: DirListing = {
  path: "/",
  truncated: false,
  entries: [
    { name: "etc", kind: "dir", size: 0, mode: 0o755, modified: 1_790_000_000 },
    { name: "bin", kind: "symlink", size: 0, mode: 0o777, modified: 1_790_000_000, link_target: "usr/bin" },
    { name: "README", kind: "file", size: 1500, mode: 0o644, modified: 1_790_000_000 },
  ],
};
const ETC: DirListing = {
  path: "/etc",
  truncated: true,
  entries: [{ name: "hosts", kind: "file", size: 174, mode: 0o644, modified: 1_790_000_000 }],
};

const listing = (call: { path: string }) => {
  const dir = new URL(call.path, "http://x").searchParams.get("path");
  if (dir === "/") return json(200, ROOT);
  if (dir === "/etc") return json(200, ETC);
  return json(404, { code: "not_found", message: `'${dir}' not found` });
};

const routes = {
  ...loggedIn,
  "GET /api/v1/envs/local/containers/c1": { json: inspect({ Id: "c1", Name: "/web" }) },
  "GET /api/v1/envs/local/containers/c1/files": listing,
};

describe("files tab", () => {
  it("lists the root with sizes, modes and symlink targets", async () => {
    mockFetch(routes);
    const { screen } = renderApp("/envs/local/containers/c1?tab=files");
    const table = await screen.findByRole("table", { name: "Files" });
    expect(await within(table).findByText("README")).toBeInTheDocument();
    expect(within(table).getByText("1.5 KB")).toBeInTheDocument();
    expect(within(table).getByText("drwxr-xr-x")).toBeInTheDocument();
    expect(within(table).getByText("→ usr/bin")).toBeInTheDocument();
    expect(within(table).getByRole("link", { name: "Download README" })).toHaveAttribute(
      "href",
      "/api/v1/envs/local/containers/c1/files/download?path=%2FREADME",
    );
  });

  it("opens a directory and goes back through the breadcrumbs", async () => {
    mockFetch(routes);
    const { screen, history } = renderApp("/envs/local/containers/c1?tab=files");
    await userEvent.click(await screen.findByRole("link", { name: "etc" }));
    expect(history.location.search).toContain("path=%2Fetc");
    expect(await screen.findByText("hosts")).toBeInTheDocument();
    expect(screen.getByText(/too large to list in full/)).toBeInTheDocument();
    await userEvent.click(within(screen.getByRole("navigation", { name: "Path" })).getByRole("link", { name: "/" }));
    expect(await screen.findByText("README")).toBeInTheDocument();
  });

  it("shows the server's error for a path that does not exist", async () => {
    mockFetch(routes);
    const { screen } = renderApp("/envs/local/containers/c1?tab=files&path=%2Fnope");
    expect(await screen.findByRole("alert")).toHaveTextContent("'/nope' not found");
  });
});
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cd web && pnpm test src/lib/files.test.ts src/pages/containers/FilesTab.test.tsx`
Expected: FAIL, 2 files. `files.test.ts` fails with `Failed to resolve import "./files"`, and all 3 tab tests fail with `Unable to find role="table" and name "Files"`, `role="link" and name "etc"` and `role="alert"`.

- [ ] **Step 3: Implement**

`web/src/lib/files.ts`:

```ts
import type { EntryKind } from "@/api/gen/types.gen";

const TYPE: Record<EntryKind, string> = { dir: "d", symlink: "l", file: "-", other: "?" };

/** Permission bits as `ls -l` shows them: `drwxr-xr-x`. */
export function formatMode(mode: number, kind: EntryKind): string {
  let out = TYPE[kind];
  for (const shift of [6, 3, 0]) {
    const bits = (mode >> shift) & 7;
    out += (bits & 4 ? "r" : "-") + (bits & 2 ? "w" : "-") + (bits & 1 ? "x" : "-");
  }
  return out;
}

/** `dir` joined with `name`: `/` + `etc` is `/etc`, `/etc` + `hosts` is `/etc/hosts`. */
export function joinPath(dir: string, name: string): string {
  return `${dir.replace(/\/+$/, "")}/${name}`;
}

/** Each ancestor of `path` with its name, for breadcrumbs: `/etc/nginx` → `/`, `/etc`, `/etc/nginx`. */
export function crumbs(path: string): { name: string; path: string }[] {
  const parts = path.split("/").filter(Boolean);
  return [{ name: "/", path: "/" }, ...parts.map((name, i) => ({ name, path: "/" + parts.slice(0, i + 1).join("/") }))];
}
```

`web/src/api/queries.ts`:

```diff
--- a/web/src/api/queries.ts
+++ b/web/src/api/queries.ts
@@ -8,6 +8,7 @@ import {
   getEnvStatus,
   getMe,
   inspectContainer,
+  listContainerFiles,
   listContainers,
   listEnvs,
   listImages,
@@ -33,6 +34,12 @@ export const keys = {
   df: (env: string) => ["env", env, "df"] as const,
   containers: (env: string) => ["env", env, "containers"] as const,
   container: (env: string, id: string) => ["env", env, "containers", id] as const,
+  /**
+   * A directory listing inside a container. Outside the containers prefix
+   * on purpose: a listing downloads the directory's whole subtree, so it is
+   * not refetched on every container event.
+   */
+  containerFiles: (env: string, id: string, dir: string) => ["env", env, "container-files", id, dir] as const,
   images: (env: string) => ["env", env, "images"] as const,
   image: (env: string, id: string) => ["env", env, "images", id] as const,
   volumes: (env: string) => ["env", env, "volumes"] as const,
@@ -102,6 +109,13 @@ export const containerQuery = (env: string, id: string) =>
     retry: retryUnlessGone,
   });
 
+export const containerFilesQuery = (env: string, id: string, dir: string) =>
+  queryOptions({
+    queryKey: keys.containerFiles(env, id, dir),
+    queryFn: async () => (await listContainerFiles({ path: { env, id }, query: { path: dir } })).data,
+    retry: retryUnlessGone,
+  });
+
 export const imagesQuery = (env: string) =>
   queryOptions({ queryKey: keys.images(env), queryFn: async () => (await listImages(path(env))).data });
 
```

`web/src/pages/containers/FilesTab.tsx`:

```tsx
import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { Download, File, Folder, Link2 } from "lucide-react";
import { containerFilesQuery } from "@/api/queries";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { errorMessage } from "@/lib/errors";
import { crumbs, formatMode, joinPath } from "@/lib/files";
import { formatBytes, formatTime } from "@/lib/format";

/** Read-only browsing of the container's filesystem, with single-file download. */
export function FilesTab({ env, id, path }: { env: string; id: string; path: string }) {
  const q = useQuery(containerFilesQuery(env, id, path));
  const to = "/envs/$env/containers/$id" as const;
  const params = { env, id };
  return (
    <div className="grid gap-3">
      <nav aria-label="Path" className="flex flex-wrap items-center gap-1 font-mono text-sm">
        {crumbs(path).map((c, i) => (
          <span key={c.path} className="flex items-center gap-1">
            {i > 1 && <span className="text-muted-foreground">/</span>}
            <Link to={to} params={params} search={{ tab: "files", path: c.path }} className="hover:underline">
              {c.name}
            </Link>
          </span>
        ))}
      </nav>
      {q.error ? (
        <p role="alert" className="text-sm text-destructive">
          {errorMessage(q.error)}
        </p>
      ) : !q.data ? (
        <p className="text-sm text-muted-foreground">Loading…</p>
      ) : (
        <>
          {q.data.truncated && (
            <p className="text-sm text-muted-foreground">
              This directory is too large to list in full; only part of it is shown.
            </p>
          )}
          <Table aria-label="Files">
            <TableHeader>
              <TableRow>
                <TableHead>Name</TableHead>
                <TableHead>Size</TableHead>
                <TableHead>Modified</TableHead>
                <TableHead>Mode</TableHead>
                <TableHead>
                  <span className="sr-only">Download</span>
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {q.data.entries.length === 0 && (
                <TableRow>
                  <TableCell colSpan={5} className="text-muted-foreground">
                    This directory is empty.
                  </TableCell>
                </TableRow>
              )}
              {q.data.entries.map((e) => {
                const full = joinPath(path, e.name);
                return (
                  <TableRow key={e.name}>
                    <TableCell className="font-mono text-xs">
                      {e.kind === "dir" ? (
                        <Link
                          to={to}
                          params={params}
                          search={{ tab: "files", path: full }}
                          className="inline-flex items-center gap-2 hover:underline"
                        >
                          <Folder className="size-4 text-muted-foreground" />
                          {e.name}
                        </Link>
                      ) : (
                        <span className="inline-flex items-center gap-2">
                          {e.kind === "symlink" ? (
                            <Link2 className="size-4 text-muted-foreground" />
                          ) : (
                            <File className="size-4 text-muted-foreground" />
                          )}
                          {e.name}
                          {e.link_target && <span className="text-muted-foreground">→ {e.link_target}</span>}
                        </span>
                      )}
                    </TableCell>
                    <TableCell>{e.kind === "file" ? formatBytes(e.size) : ""}</TableCell>
                    <TableCell>{formatTime(e.modified)}</TableCell>
                    <TableCell className="font-mono text-xs">{formatMode(e.mode, e.kind)}</TableCell>
                    <TableCell>
                      {e.kind === "file" && (
                        <a
                          href={downloadUrl(env, id, full)}
                          download={e.name}
                          aria-label={`Download ${e.name}`}
                          className="text-muted-foreground hover:text-foreground"
                        >
                          <Download className="size-4" />
                        </a>
                      )}
                    </TableCell>
                  </TableRow>
                );
              })}
            </TableBody>
          </Table>
        </>
      )}
    </div>
  );
}

/** A plain GET the browser downloads itself; the session cookie authenticates it. */
export function downloadUrl(env: string, id: string, path: string) {
  const e = encodeURIComponent;
  return `/api/v1/envs/${e(env)}/containers/${e(id)}/files/download?path=${e(path)}`;
}
```

`web/src/pages/containers/tabs.ts`:

```diff
--- a/web/src/pages/containers/tabs.ts
+++ b/web/src/pages/containers/tabs.ts
@@ -1,3 +1,3 @@
 /** The container page's tabs, in order; the router validates `?tab=` against them. */
-export const CONTAINER_TABS = ["overview", "inspect", "env", "mounts", "networks"] as const;
+export const CONTAINER_TABS = ["overview", "inspect", "env", "mounts", "networks", "files"] as const;
 export type ContainerTab = (typeof CONTAINER_TABS)[number];
```

`web/src/pages/containers/ContainerPage.tsx`:

```diff
--- a/web/src/pages/containers/ContainerPage.tsx
+++ b/web/src/pages/containers/ContainerPage.tsx
@@ -20,13 +20,14 @@ import { shortId } from "@/lib/format";
 import { isSecretKey, splitEnv } from "@/lib/secrets";
 import { FormError } from "@/pages/AuthLayout";
 import { ACTIONS, actionsFor, RemoveDialog } from "./actions";
+import { FilesTab } from "./FilesTab";
 import type { ContainerTab } from "./tabs";
 
 const FROM = "/authed/shell/envs/$env/containers/$id";
 
 export function ContainerPage() {
   const { env, id } = useParams({ from: FROM });
-  const { tab = "overview" } = useSearch({ from: FROM });
+  const { tab = "overview", path = "/" } = useSearch({ from: FROM });
   const navigate = useNavigate({ from: "/envs/$env/containers/$id" });
   const q = useQuery(containerQuery(env, id));
   const back = (
@@ -54,6 +55,7 @@ export function ContainerPage() {
           <TabsTrigger value="env">Env</TabsTrigger>
           <TabsTrigger value="mounts">Mounts</TabsTrigger>
           <TabsTrigger value="networks">Networks</TabsTrigger>
+          <TabsTrigger value="files">Files</TabsTrigger>
         </TabsList>
         <TabsContent value="overview">
           <Overview c={c} />
@@ -70,6 +72,9 @@ export function ContainerPage() {
         <TabsContent value="networks">
           <Networks c={c} />
         </TabsContent>
+        <TabsContent value="files">
+          <FilesTab env={env} id={id} path={path} />
+        </TabsContent>
       </Tabs>
     </Page>
   );
```

`web/src/router.tsx`:

```diff
--- a/web/src/router.tsx
+++ b/web/src/router.tsx
@@ -122,8 +122,9 @@ const containersRoute = createRoute({
 const containerRoute = createRoute({
   getParentRoute: () => envRoute,
   path: "containers/$id",
-  validateSearch: (s: Record<string, unknown>): { tab?: ContainerTab } => ({
+  validateSearch: (s: Record<string, unknown>): { tab?: ContainerTab; path?: string } => ({
     tab: CONTAINER_TABS.find((t) => t === s.tab),
+    path: typeof s.path === "string" && s.path.startsWith("/") ? s.path : undefined,
   }),
   component: lazyRouteComponent(() => import("@/pages/containers/ContainerPage"), "ContainerPage"),
 });
```

- [ ] **Step 4: Run all checks**

Run: `cd web && pnpm test && pnpm typecheck && pnpm lint`
Expected: 109 passed (20 files); typecheck and lint are clean.

- [ ] **Step 5: Commit**

```bash
git add web/src
git commit -m "feat(web): container file browser with breadcrumbs and download"
```

---

### Task 5: Create, edit (recreate) and duplicate

**Files:**
- Create:
  - `web/src/lib/args.ts`, `web/src/lib/kv.ts` and `web/src/lib/spec.ts`;
  - `web/src/pages/containers/ContainerFormPage.tsx`.
- Modify:
  - `web/src/api/queries.ts` and `web/src/router.tsx`;
  - `web/src/pages/containers/ContainerPage.tsx`: Edit and Duplicate links, and the command shown with quoting;
  - `web/src/pages/containers/ContainersPage.tsx`: the Create button;
  - `web/src/pages/containers/ContainerPage.test.tsx`.
- Test: `web/src/lib/spec.test.ts` and `web/src/pages/containers/ContainerFormPage.test.tsx`.

**Interfaces:**
- Consumes: the SDK's `getContainerSpec`, `createContainer` and `recreateContainer`; `networksQuery`; and `DetailError`, `Page` and `NativeSelect`.
- Produces `@/lib/args`: `splitArgs(line)`, which follows shell quoting and throws `unclosed " quote`, and `joinArgs(args)`.
- Produces `@/lib/kv`:
  - `parseKeyValues(text, what)`, which throws `<what> "<line>" must look like key=value`.
  - `formatKeyValues(map)`.
- Produces `@/lib/spec`:
  - `RESTART_POLICIES`, and the types `SpecForm` and `PortRow`;
  - `EMPTY_FORM`;
  - `specToForm(spec)`;
  - `formToSpec(form)`, which returns a `ContainerSpec` or throws an Error naming the field.
- Produces `@/api/queries`: `containerSpecQuery(env, id)`, under `[...keys.container(env, id), "spec"]`.
- Produces:
  - the routes `/envs/$env/containers/new` (`NewContainerPage`, with `?from=<id>` to duplicate) and `/envs/$env/containers/$id/edit` (`EditContainerPage`);
  - the form is labelled `Container settings`, and its port rows are groups named `Port <n>`.

- [ ] **Step 1: Write the failing tests**

`web/src/lib/spec.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import type { ContainerSpec } from "@/api/gen/types.gen";
import { joinArgs, splitArgs } from "./args";
import { EMPTY_FORM, formToSpec, specToForm } from "./spec";

describe("splitArgs", () => {
  it("splits on whitespace and groups quotes", () => {
    expect(splitArgs(`nginx -g "daemon off;"`)).toEqual(["nginx", "-g", "daemon off;"]);
    expect(splitArgs(`sh -c 'echo "hi there"'`)).toEqual(["sh", "-c", 'echo "hi there"']);
    expect(splitArgs(`a\\ b "" c`)).toEqual(["a b", "", "c"]);
    expect(splitArgs("   ")).toEqual([]);
  });

  it("rejects an unclosed quote", () => {
    expect(() => splitArgs(`echo "oops`)).toThrow('unclosed " quote');
  });

  it("round-trips through joinArgs", () => {
    const args = ["sh", "-c", 'echo "a b" \\ c', ""];
    expect(splitArgs(joinArgs(args))).toEqual(args);
  });
});

const SPEC: ContainerSpec = {
  image: "nginx:1.27",
  name: "web",
  cmd: ["nginx", "-g", "daemon off;"],
  entrypoint: null,
  env: ["A=1", "B=x=y"],
  ports: [
    { container_port: 80, host_port: 8080, protocol: "tcp" },
    { container_port: 53, host_port: null, protocol: "udp" },
  ],
  binds: ["data:/data", "/srv:/srv:ro"],
  network: "shop",
  restart_policy: "unless-stopped",
  labels: { tier: "front", "a.b": "c=d" },
  memory_bytes: 512 * 1024 * 1024,
  nano_cpus: 1_500_000_000,
  cap_add: ["NET_ADMIN", "SYS_TIME"],
  privileged: false,
  advanced: { HostConfig: { ShmSize: 1024 } },
};

describe("container spec form", () => {
  it("round-trips a spec through the form", () => {
    expect(formToSpec(specToForm(SPEC))).toEqual(SPEC);
  });

  it("sends nulls for fields left empty", () => {
    expect(formToSpec({ ...EMPTY_FORM, image: " redis " })).toEqual({
      image: "redis",
      name: null,
      cmd: null,
      entrypoint: null,
      ports: [],
      env: [],
      binds: [],
      network: null,
      restart_policy: "no",
      labels: {},
      memory_bytes: null,
      nano_cpus: null,
      cap_add: [],
      privileged: false,
      advanced: null,
    });
  });

  it("names the field that is wrong", () => {
    const base = { ...EMPTY_FORM, image: "redis" };
    expect(() => formToSpec({ ...base, image: "" })).toThrow("Image is required");
    expect(() => formToSpec({ ...base, ports: [{ host: "80", container: "http", protocol: "tcp" }] })).toThrow(
      'Container port must be a number from 1 to 65535, not "http"',
    );
    expect(() => formToSpec({ ...base, ports: [{ host: "70000", container: "80", protocol: "tcp" }] })).toThrow(
      "Host port must be",
    );
    expect(() => formToSpec({ ...base, labels: "novalue" })).toThrow('Label "novalue" must look like key=value');
    expect(() => formToSpec({ ...base, memoryMb: "lots" })).toThrow('Memory must be a positive number, not "lots"');
    expect(() => formToSpec({ ...base, advanced: "{nope" })).toThrow("Advanced settings are not valid JSON");
    expect(() => formToSpec({ ...base, advanced: "[1]" })).toThrow("Advanced settings must be a JSON object");
    expect(() => formToSpec({ ...base, cmd: `echo "x` })).toThrow('Command: unclosed " quote');
  });

  it("skips empty port rows", () => {
    const f = { ...EMPTY_FORM, image: "redis", ports: [{ host: "", container: "", protocol: "tcp" as const }] };
    expect(formToSpec(f).ports).toEqual([]);
  });
});
```

`web/src/pages/containers/ContainerFormPage.test.tsx`:

```tsx
import { waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { ContainerSpec } from "@/api/gen/types.gen";
import { json, mockFetch } from "@/test/fetch";
import { loggedIn, renderApp } from "@/test/app";
import { inspect } from "@/test/docker";

const SPEC: ContainerSpec = {
  image: "nginx:1.27",
  name: "web",
  cmd: ["nginx", "-g", "daemon off;"],
  env: ["A=1"],
  ports: [{ container_port: 80, host_port: 8080, protocol: "tcp" }],
  binds: [],
  labels: {},
  cap_add: [],
  privileged: false,
  restart_policy: "always",
};

const routes = {
  ...loggedIn,
  "GET /api/v1/envs/local/networks": { json: [{ id: "n1", name: "shop", driver: "bridge", scope: "local", internal: false, attachable: false }] },
  "GET /api/v1/envs/local/containers/c1/spec": { json: SPEC },
  "GET /api/v1/envs/local/containers/c1": { json: inspect({ Id: "c1", Name: "/web" }) },
  "GET /api/v1/envs/local/containers/new1": { json: inspect({ Id: "new1", Name: "/api" }) },
};

describe("container form", () => {
  it("creates a container from the form and opens it", async () => {
    const calls = mockFetch({ ...routes, "POST /api/v1/envs/local/containers": { status: 201, json: { id: "new1" } } });
    const { screen, history } = renderApp("/envs/local/containers/new");
    await userEvent.type(await screen.findByLabelText("Image"), "redis:7");
    await userEvent.type(screen.getByLabelText("Name"), "api");
    await userEvent.type(screen.getByLabelText("Command"), 'redis-server --save ""');
    await userEvent.click(screen.getByRole("button", { name: "Add port" }));
    const port = screen.getByRole("group", { name: "Port 1" });
    await userEvent.type(within(port).getByLabelText("Host port"), "6380");
    await userEvent.type(within(port).getByLabelText("Container port"), "6379");
    await userEvent.type(screen.getByLabelText("Environment variables"), "A=1{enter}B=2");
    await userEvent.selectOptions(await screen.findByLabelText("Network"), "shop");
    await userEvent.selectOptions(screen.getByLabelText("Restart policy"), "unless-stopped");
    await userEvent.type(screen.getByLabelText("Memory limit (MB)"), "256");
    await userEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(history.location.pathname).toBe("/envs/local/containers/new1"));
    expect(calls.find((c) => c.method === "POST")?.body).toMatchObject({
      image: "redis:7",
      name: "api",
      cmd: ["redis-server", "--save", ""],
      ports: [{ container_port: 6379, host_port: 6380, protocol: "tcp" }],
      env: ["A=1", "B=2"],
      network: "shop",
      restart_policy: "unless-stopped",
      memory_bytes: 256 * 1024 * 1024,
      start: true,
    });
  });

  it("says which field is wrong without sending anything", async () => {
    const calls = mockFetch(routes);
    const { screen } = renderApp("/envs/local/containers/new");
    await userEvent.type(await screen.findByLabelText("Image"), "redis");
    await userEvent.type(screen.getByLabelText("Labels"), "oops");
    await userEvent.click(screen.getByRole("button", { name: "Create" }));
    expect(await screen.findByRole("alert")).toHaveTextContent('Label "oops" must look like key=value');
    expect(calls.some((c) => c.method === "POST")).toBe(false);
  });

  it("shows Docker's error inline", async () => {
    mockFetch({
      ...routes,
      "POST /api/v1/envs/local/containers": () => json(502, { code: "upstream", message: "No such image: redis:nope" }),
    });
    const { screen } = renderApp("/envs/local/containers/new");
    await userEvent.type(await screen.findByLabelText("Image"), "redis:nope");
    await userEvent.click(screen.getByRole("button", { name: "Create" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("No such image: redis:nope");
    // Inline only, not also as a toast.
    expect(screen.getAllByText("No such image: redis:nope")).toHaveLength(1);
  });

  it("duplicates a container from its spec under a new name", async () => {
    const calls = mockFetch({ ...routes, "POST /api/v1/envs/local/containers": { status: 201, json: { id: "new1" } } });
    const { screen, history } = renderApp("/envs/local/containers/c1");
    await userEvent.click(await screen.findByRole("link", { name: "Duplicate" }));
    expect(await screen.findByRole("heading", { name: "Duplicate web" })).toBeInTheDocument();
    expect(screen.getByLabelText("Name")).toHaveValue("web-copy");
    expect(screen.getByLabelText("Command")).toHaveValue('nginx -g "daemon off;"');
    const port = screen.getByRole("group", { name: "Port 1" });
    await userEvent.clear(within(port).getByLabelText("Host port"));
    await userEvent.type(within(port).getByLabelText("Host port"), "8081");
    await userEvent.click(screen.getByRole("button", { name: "Create" }));
    await waitFor(() => expect(history.location.pathname).toBe("/envs/local/containers/new1"));
    expect(calls.find((c) => c.method === "POST")?.body).toMatchObject({
      name: "web-copy",
      cmd: ["nginx", "-g", "daemon off;"],
      ports: [{ container_port: 80, host_port: 8081, protocol: "tcp" }],
      restart_policy: "always",
    });
  });

  it("recreates a container with changes and opens the replacement", async () => {
    const calls = mockFetch({
      ...routes,
      "POST /api/v1/envs/local/containers/c1/recreate": {
        json: { id: "new1", old_removed: true, backup_name: "web-fleetops-old-1" },
      },
    });
    const { screen, history } = renderApp("/envs/local/containers/c1");
    await userEvent.click(await screen.findByRole("link", { name: "Edit" }));
    expect(await screen.findByRole("heading", { name: "Edit web" })).toBeInTheDocument();
    expect(screen.queryByLabelText("Start the container after creating it")).not.toBeInTheDocument();
    await userEvent.clear(screen.getByLabelText("Image"));
    await userEvent.type(screen.getByLabelText("Image"), "nginx:1.28");
    await userEvent.click(screen.getByRole("button", { name: "Recreate" }));
    await waitFor(() => expect(history.location.pathname).toBe("/envs/local/containers/new1"));
    const body = calls.find((c) => c.path.endsWith("/recreate"))?.body as ContainerSpec;
    expect(body).toMatchObject({ image: "nginx:1.28", name: "web", env: ["A=1"] });
    expect(body).not.toHaveProperty("start");
    expect(await screen.findByText("Container recreated")).toBeInTheDocument();
  });

  it("warns when the old container was kept after a recreate", async () => {
    mockFetch({
      ...routes,
      "POST /api/v1/envs/local/containers/c1/recreate": {
        json: { id: "new1", old_removed: false, backup_name: "web-fleetops-old-1" },
      },
    });
    const { screen } = renderApp("/envs/local/containers/c1/edit");
    await userEvent.click(await screen.findByRole("button", { name: "Recreate" }));
    expect(await screen.findByText(/it is kept as web-fleetops-old-1/)).toBeInTheDocument();
  });
});
```

`web/src/pages/containers/ContainerPage.test.tsx`: the overview now shows the command with its quoting.

```diff
--- a/web/src/pages/containers/ContainerPage.test.tsx
+++ b/web/src/pages/containers/ContainerPage.test.tsx
@@ -33,6 +33,7 @@ describe("container page", () => {
     const overview = screen.getByLabelText("Container details");
     expect(within(overview).getByText("c1abcdef0123")).toBeInTheDocument();
     expect(within(overview).getByText("0.0.0.0:8080 → 80/tcp")).toBeInTheDocument();
+    expect(within(overview).getByText('nginx -g "daemon off;"')).toBeInTheDocument();
     expect(within(overview).getByText("unless-stopped")).toBeInTheDocument();
     expect(within(screen.getByLabelText("Labels")).getByText("shop")).toBeInTheDocument();
   });
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cd web && pnpm test src/lib/spec.test.ts src/pages/containers`
Expected: FAIL, 3 files, 7 failed and 8 passed. `spec.test.ts` fails with `Failed to resolve import "./args"`. The form tests fail with `Unable to find a label with the text of: Image` and `Unable to find role="link" and name "Duplicate"` / `"Edit"`. The overview test fails with `Unable to find an element with the text: nginx -g "daemon off;"`.

- [ ] **Step 3: Implement the spec conversion**

`web/src/lib/args.ts`:

```ts
/**
 * Splits a command line into arguments the way a shell would for simple
 * quoting: whitespace separates, and single or double quotes group
 * (`nginx -g "daemon off;"` is three arguments). A backslash escapes the
 * next character outside single quotes.
 */
export function splitArgs(line: string): string[] {
  const out: string[] = [];
  let cur = "";
  let inWord = false;
  let quote: '"' | "'" | null = null;
  for (let i = 0; i < line.length; i++) {
    const ch = line[i];
    if (quote) {
      if (ch === quote) quote = null;
      else if (ch === "\\" && quote === '"' && i + 1 < line.length) cur += line[++i];
      else cur += ch;
    } else if (ch === '"' || ch === "'") {
      quote = ch;
      inWord = true;
    } else if (ch === "\\" && i + 1 < line.length) {
      cur += line[++i];
      inWord = true;
    } else if (/\s/.test(ch)) {
      if (inWord) out.push(cur);
      cur = "";
      inWord = false;
    } else {
      cur += ch;
      inWord = true;
    }
  }
  if (quote) throw new Error(`unclosed ${quote} quote`);
  if (inWord) out.push(cur);
  return out;
}

/** The inverse of `splitArgs`: quotes arguments that need it. */
export function joinArgs(args: string[]): string {
  return args
    .map((a) => (a === "" ? '""' : /[\s"'\\]/.test(a) ? `"${a.replace(/(["\\])/g, "\\$1")}"` : a))
    .join(" ");
}
```

`web/src/lib/kv.ts`:

```ts
/**
 * `key=value` lines (blank lines skipped) into a map, or throws naming the
 * bad line: `Label "x" must look like key=value`.
 */
export function parseKeyValues(text: string, what: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const raw of text.split("\n")) {
    const l = raw.trim();
    if (!l) continue;
    const i = l.indexOf("=");
    if (i <= 0) throw new Error(`${what} "${l}" must look like key=value`);
    out[l.slice(0, i)] = l.slice(i + 1);
  }
  return out;
}

/** The inverse of `parseKeyValues`. */
export function formatKeyValues(map: Record<string, string> | null | undefined): string {
  return Object.entries(map ?? {})
    .map(([k, v]) => `${k}=${v}`)
    .join("\n");
}
```

`web/src/lib/spec.ts`:

```ts
import type { ContainerSpec } from "@/api/gen/types.gen";
import { joinArgs, splitArgs } from "./args";
import { formatKeyValues, parseKeyValues } from "./kv";

export const RESTART_POLICIES = ["no", "always", "unless-stopped", "on-failure"] as const;

export interface PortRow {
  host: string;
  container: string;
  protocol: "tcp" | "udp";
}

/** The container form's fields, as the inputs hold them (text, mostly). */
export interface SpecForm {
  image: string;
  name: string;
  cmd: string;
  entrypoint: string;
  ports: PortRow[];
  /** One `KEY=value` per line. */
  env: string;
  /** One `source:target[:mode]` per line. */
  binds: string;
  network: string;
  restart: (typeof RESTART_POLICIES)[number];
  /** One `key=value` per line. */
  labels: string;
  memoryMb: string;
  cpus: string;
  /** Capabilities separated by spaces or commas. */
  capAdd: string;
  privileged: boolean;
  /** A Docker `ContainerCreate` JSON object merged over the rest. */
  advanced: string;
}

export const EMPTY_FORM: SpecForm = {
  image: "",
  name: "",
  cmd: "",
  entrypoint: "",
  ports: [],
  env: "",
  binds: "",
  network: "",
  restart: "no",
  labels: "",
  memoryMb: "",
  cpus: "",
  capAdd: "",
  privileged: false,
  advanced: "",
};

const MB = 1024 * 1024;

/** The form pre-filled from an existing container's spec (recreate, duplicate). */
export function specToForm(s: ContainerSpec): SpecForm {
  const policy = RESTART_POLICIES.find((p) => p === s.restart_policy) ?? "no";
  return {
    image: s.image ?? "",
    name: s.name ?? "",
    cmd: s.cmd ? joinArgs(s.cmd) : "",
    entrypoint: s.entrypoint ? joinArgs(s.entrypoint) : "",
    ports: (s.ports ?? []).map((p) => ({
      host: p.host_port == null ? "" : String(p.host_port),
      container: String(p.container_port),
      protocol: p.protocol === "udp" ? "udp" : "tcp",
    })),
    env: (s.env ?? []).join("\n"),
    binds: (s.binds ?? []).join("\n"),
    network: s.network ?? "",
    restart: policy,
    labels: formatKeyValues(s.labels),
    memoryMb: s.memory_bytes ? String(Math.round(s.memory_bytes / MB)) : "",
    cpus: s.nano_cpus ? String(s.nano_cpus / 1e9) : "",
    capAdd: (s.cap_add ?? []).join(" "),
    privileged: !!s.privileged,
    advanced: s.advanced ? JSON.stringify(s.advanced, null, 2) : "",
  };
}

const lines = (text: string) =>
  text
    .split("\n")
    .map((l) => l.trim())
    .filter(Boolean);

function port(text: string, what: string): number {
  const n = Number(text);
  if (!Number.isInteger(n) || n < 1 || n > 65535) throw new Error(`${what} must be a number from 1 to 65535, not "${text}"`);
  return n;
}

function positive(text: string, what: string): number {
  const n = Number(text);
  if (!Number.isFinite(n) || n <= 0) throw new Error(`${what} must be a positive number, not "${text}"`);
  return n;
}

/** The spec to send, or throws an Error saying which field is wrong. */
export function formToSpec(f: SpecForm): ContainerSpec {
  if (!f.image.trim()) throw new Error("Image is required");
  const labels = parseKeyValues(f.labels, "Label");
  for (const e of lines(f.env)) {
    if (e.startsWith("=")) throw new Error(`Environment variable "${e}" has no name`);
  }
  let advanced: Record<string, unknown> | null = null;
  if (f.advanced.trim()) {
    let parsed: unknown;
    try {
      parsed = JSON.parse(f.advanced);
    } catch (e) {
      throw new Error(`Advanced settings are not valid JSON: ${(e as Error).message}`, { cause: e });
    }
    if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
      throw new Error("Advanced settings must be a JSON object");
    }
    advanced = parsed as Record<string, unknown>;
  }
  const args = (text: string, what: string) => {
    try {
      return text.trim() ? splitArgs(text) : null;
    } catch (e) {
      throw new Error(`${what}: ${(e as Error).message}`, { cause: e });
    }
  };
  return {
    image: f.image.trim(),
    name: f.name.trim() || null,
    cmd: args(f.cmd, "Command"),
    entrypoint: args(f.entrypoint, "Entrypoint"),
    ports: f.ports
      .filter((p) => p.container.trim() || p.host.trim())
      .map((p) => ({
        container_port: port(p.container.trim(), "Container port"),
        host_port: p.host.trim() ? port(p.host.trim(), "Host port") : null,
        protocol: p.protocol,
      })),
    env: lines(f.env),
    binds: lines(f.binds),
    network: f.network || null,
    restart_policy: f.restart,
    labels,
    memory_bytes: f.memoryMb.trim() ? Math.round(positive(f.memoryMb.trim(), "Memory") * MB) : null,
    nano_cpus: f.cpus.trim() ? Math.round(positive(f.cpus.trim(), "CPUs") * 1e9) : null,
    cap_add: f.capAdd.split(/[\s,]+/).filter(Boolean),
    privileged: f.privileged,
    advanced,
  };
}

```

- [ ] **Step 4: Implement the form and its routes**

`web/src/api/queries.ts`:

```diff
--- a/web/src/api/queries.ts
+++ b/web/src/api/queries.ts
@@ -6,6 +6,7 @@ import {
   getDiskUsage,
   getEngineInfo,
   getEnvStatus,
+  getContainerSpec,
   getMe,
   inspectContainer,
   listContainerFiles,
@@ -109,6 +110,14 @@ export const containerQuery = (env: string, id: string) =>
     retry: retryUnlessGone,
   });
 
+/** The container as the create form edits it (for recreate and duplicate). */
+export const containerSpecQuery = (env: string, id: string) =>
+  queryOptions({
+    queryKey: [...keys.container(env, id), "spec"] as const,
+    queryFn: async () => (await getContainerSpec({ path: { env, id } })).data,
+    retry: retryUnlessGone,
+  });
+
 export const containerFilesQuery = (env: string, id: string, dir: string) =>
   queryOptions({
     queryKey: keys.containerFiles(env, id, dir),
```

`web/src/pages/containers/ContainerFormPage.tsx`:

```tsx
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate, useParams, useSearch } from "@tanstack/react-router";
import { Plus, Trash2 } from "lucide-react";
import { useState, type FormEvent, type ReactNode } from "react";
import { toast } from "sonner";
import { containerSpecQuery, keys, networksQuery } from "@/api/queries";
import { createContainer, recreateContainer } from "@/api/gen/sdk.gen";
import { DetailError } from "@/components/Details";
import { Page } from "@/components/Page";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { NativeSelect } from "@/components/ui/native-select";
import { Textarea } from "@/components/ui/textarea";
import { errorMessage } from "@/lib/errors";
import { EMPTY_FORM, formToSpec, RESTART_POLICIES, specToForm, type SpecForm } from "@/lib/spec";
import { FormError } from "@/pages/AuthLayout";

type Mode = "create" | "duplicate" | "recreate";

/** `/containers/new` (optionally `?from=<id>` to duplicate). */
export function NewContainerPage() {
  const { env } = useParams({ from: "/authed/shell/envs/$env/containers/new" });
  const { from } = useSearch({ from: "/authed/shell/envs/$env/containers/new" });
  return from ? <Prefilled env={env} id={from} mode="duplicate" /> : <ContainerForm env={env} mode="create" initial={EMPTY_FORM} />;
}

/** `/containers/$id/edit`: recreate the container with changes. */
export function EditContainerPage() {
  const { env, id } = useParams({ from: "/authed/shell/envs/$env/containers/$id/edit" });
  return <Prefilled env={env} id={id} mode="recreate" />;
}

function Prefilled({ env, id, mode }: { env: string; id: string; mode: Mode }) {
  const q = useQuery(containerSpecQuery(env, id));
  if (q.error) {
    const back = (
      <Link to="/envs/$env/containers" params={{ env }} className="underline">
        Back to containers
      </Link>
    );
    return <DetailError error={q.error} what="container" back={back} />;
  }
  if (!q.data) return <p className="p-6 text-sm text-muted-foreground">Loading…</p>;
  const initial = specToForm(q.data);
  if (mode === "duplicate") initial.name = initial.name ? `${initial.name}-copy` : "";
  return <ContainerForm env={env} mode={mode} id={id} initial={initial} original={q.data.name ?? id} />;
}

const TITLES: Record<Mode, (name?: string) => string> = {
  create: () => "Create container",
  duplicate: (n) => `Duplicate ${n}`,
  recreate: (n) => `Edit ${n}`,
};

function ContainerForm({
  env,
  mode,
  id,
  initial,
  original,
}: {
  env: string;
  mode: Mode;
  id?: string;
  initial: SpecForm;
  original?: string;
}) {
  const qc = useQueryClient();
  const navigate = useNavigate();
  const networks = useQuery(networksQuery(env));
  const [f, setF] = useState(initial);
  const [start, setStart] = useState(true);
  const [invalid, setInvalid] = useState<string | null>(null);
  const set = <K extends keyof SpecForm>(k: K, v: SpecForm[K]) => setF((old) => ({ ...old, [k]: v }));

  const m = useMutation({
    meta: { inline: true },
    mutationFn: async (spec: ReturnType<typeof formToSpec>) => {
      if (mode === "recreate") {
        const { data } = await recreateContainer({ path: { env, id: id! }, body: spec });
        if (!data.old_removed) {
          toast.warning(`Recreated, but the old container could not be removed; it is kept as ${data.backup_name}`);
        } else {
          toast.success("Container recreated");
        }
        return data.id;
      }
      const { data } = await createContainer({ path: { env }, body: { ...spec, start } });
      toast.success("Container created");
      return data.id;
    },
    onSuccess: async (newId) => {
      await qc.invalidateQueries({ queryKey: keys.containers(env) });
      void navigate({ to: "/envs/$env/containers/$id", params: { env, id: newId } });
    },
  });

  const submit = (e: FormEvent) => {
    e.preventDefault();
    try {
      const spec = formToSpec(f);
      setInvalid(null);
      m.mutate(spec);
    } catch (err) {
      setInvalid((err as Error).message);
    }
  };

  return (
    <Page title={TITLES[mode](original)}>
      {mode === "recreate" && (
        <p className="max-w-3xl text-sm text-muted-foreground">
          Saving stops this container, creates a replacement with these settings and starts it, then removes the old
          one. If the replacement fails to start, the old container is put back.
        </p>
      )}
      <form onSubmit={submit} className="grid max-w-3xl gap-6" aria-label="Container settings">
        <Section title="Basics">
          <Field id="image" label="Image">
            <Input id="image" required placeholder="nginx:1.27" value={f.image} onChange={(e) => set("image", e.target.value)} />
          </Field>
          <Field id="name" label="Name" hint="Leave empty for a generated name.">
            <Input id="name" value={f.name} onChange={(e) => set("name", e.target.value)} />
          </Field>
          <Field id="cmd" label="Command" hint="Leave empty for the image's default. Quote arguments with spaces.">
            <Input id="cmd" className="font-mono" value={f.cmd} onChange={(e) => set("cmd", e.target.value)} />
          </Field>
          <Field id="entrypoint" label="Entrypoint">
            <Input
              id="entrypoint"
              className="font-mono"
              value={f.entrypoint}
              onChange={(e) => set("entrypoint", e.target.value)}
            />
          </Field>
        </Section>

        <Section title="Ports">
          <Ports rows={f.ports} onChange={(rows) => set("ports", rows)} />
        </Section>

        <Section title="Environment and storage">
          <Field id="env" label="Environment variables" hint="One KEY=value per line.">
            <Textarea id="env" className="font-mono" rows={4} value={f.env} onChange={(e) => set("env", e.target.value)} />
          </Field>
          <Field id="binds" label="Volumes and bind mounts" hint="One source:target[:ro] per line, e.g. data:/var/lib/data.">
            <Textarea id="binds" className="font-mono" rows={3} value={f.binds} onChange={(e) => set("binds", e.target.value)} />
          </Field>
        </Section>

        <Section title="Network and restart">
          <Field id="network" label="Network">
            <NativeSelect id="network" value={f.network} onChange={(e) => set("network", e.target.value)}>
              <option value="">Default (bridge)</option>
              {networks.data
                ?.filter((n) => n.name !== "bridge")
                .map((n) => (
                  <option key={n.id} value={n.name}>
                    {n.name}
                  </option>
                ))}
              {f.network && !networks.data?.some((n) => n.name === f.network) && <option value={f.network}>{f.network}</option>}
            </NativeSelect>
          </Field>
          <Field id="restart" label="Restart policy">
            <NativeSelect
              id="restart"
              value={f.restart}
              onChange={(e) => set("restart", e.target.value as SpecForm["restart"])}
            >
              {RESTART_POLICIES.map((p) => (
                <option key={p} value={p}>
                  {p}
                </option>
              ))}
            </NativeSelect>
          </Field>
          <Field id="labels" label="Labels" hint="One key=value per line.">
            <Textarea id="labels" className="font-mono" rows={3} value={f.labels} onChange={(e) => set("labels", e.target.value)} />
          </Field>
        </Section>

        <Section title="Resources and privileges">
          <div className="grid gap-4 sm:grid-cols-2">
            <Field id="memory" label="Memory limit (MB)">
              <Input id="memory" inputMode="numeric" value={f.memoryMb} onChange={(e) => set("memoryMb", e.target.value)} />
            </Field>
            <Field id="cpus" label="CPUs" hint="For example 0.5 or 2.">
              <Input id="cpus" inputMode="decimal" value={f.cpus} onChange={(e) => set("cpus", e.target.value)} />
            </Field>
          </div>
          <Field id="caps" label="Added capabilities" hint="For example NET_ADMIN SYS_TIME.">
            <Input id="caps" className="font-mono" value={f.capAdd} onChange={(e) => set("capAdd", e.target.value)} />
          </Field>
          <div className="flex items-center gap-2">
            <Checkbox id="privileged" checked={f.privileged} onCheckedChange={(v) => set("privileged", !!v)} />
            <Label htmlFor="privileged">Privileged (full access to the host)</Label>
          </div>
        </Section>

        <Section title="Advanced">
          <Field
            id="advanced"
            label="Docker create JSON"
            hint="A ContainerCreate object merged over the settings above, e.g. {&quot;HostConfig&quot;: {&quot;ShmSize&quot;: 268435456}}."
          >
            <Textarea
              id="advanced"
              className="font-mono"
              rows={4}
              value={f.advanced}
              onChange={(e) => set("advanced", e.target.value)}
            />
          </Field>
        </Section>

        {mode !== "recreate" && (
          <div className="flex items-center gap-2">
            <Checkbox id="start" checked={start} onCheckedChange={(v) => setStart(!!v)} />
            <Label htmlFor="start">Start the container after creating it</Label>
          </div>
        )}
        <FormError message={invalid ?? (m.error ? errorMessage(m.error) : null)} />
        <div className="flex gap-2">
          <Button type="submit" disabled={m.isPending}>
            {mode === "recreate" ? "Recreate" : "Create"}
          </Button>
          <Button type="button" variant="outline" onClick={() => history.back()}>
            Cancel
          </Button>
        </div>
      </form>
    </Page>
  );
}

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <fieldset className="grid gap-4">
      <legend className="mb-2 font-medium">{title}</legend>
      {children}
    </fieldset>
  );
}

function Field({ id, label, hint, children }: { id: string; label: string; hint?: string; children: ReactNode }) {
  return (
    <div className="grid gap-2">
      <Label htmlFor={id}>{label}</Label>
      {children}
      {hint && <p className="text-xs text-muted-foreground">{hint}</p>}
    </div>
  );
}

function Ports({ rows, onChange }: { rows: SpecForm["ports"]; onChange: (rows: SpecForm["ports"]) => void }) {
  const update = (i: number, patch: Partial<SpecForm["ports"][number]>) =>
    onChange(rows.map((r, j) => (j === i ? { ...r, ...patch } : r)));
  return (
    <div className="grid gap-2">
      {rows.map((r, i) => (
        <div key={i} className="flex items-center gap-2" role="group" aria-label={`Port ${i + 1}`}>
          <Input
            aria-label="Host port"
            placeholder="host (random)"
            inputMode="numeric"
            value={r.host}
            onChange={(e) => update(i, { host: e.target.value })}
          />
          <span className="text-muted-foreground">→</span>
          <Input
            aria-label="Container port"
            placeholder="container"
            inputMode="numeric"
            value={r.container}
            onChange={(e) => update(i, { container: e.target.value })}
          />
          <NativeSelect
            aria-label="Protocol"
            className="w-24 shrink-0"
            value={r.protocol}
            onChange={(e) => update(i, { protocol: e.target.value as "tcp" | "udp" })}
          >
            <option value="tcp">tcp</option>
            <option value="udp">udp</option>
          </NativeSelect>
          <Button
            type="button"
            variant="ghost"
            size="icon"
            aria-label="Remove port"
            onClick={() => onChange(rows.filter((_, j) => j !== i))}
          >
            <Trash2 />
          </Button>
        </div>
      ))}
      <Button
        type="button"
        variant="outline"
        size="sm"
        className="justify-self-start"
        onClick={() => onChange([...rows, { host: "", container: "", protocol: "tcp" }])}
      >
        <Plus />
        Add port
      </Button>
    </div>
  );
}
```

`web/src/pages/containers/ContainerPage.tsx`:

```diff
--- a/web/src/pages/containers/ContainerPage.tsx
+++ b/web/src/pages/containers/ContainerPage.tsx
@@ -15,6 +15,7 @@ import { Input } from "@/components/ui/input";
 import { Label } from "@/components/ui/label";
 import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
 import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
+import { joinArgs } from "@/lib/args";
 import { errorMessage } from "@/lib/errors";
 import { shortId } from "@/lib/format";
 import { isSecretKey, splitEnv } from "@/lib/secrets";
@@ -99,6 +100,16 @@ function ContainerActions({ env, c, name }: { env: string; c: ContainerInspect;
       <Button variant="outline" onClick={() => setRenaming(true)}>
         Rename
       </Button>
+      <Button variant="outline" asChild>
+        <Link to="/envs/$env/containers/$id/edit" params={{ env, id: c.Id }}>
+          Edit
+        </Link>
+      </Button>
+      <Button variant="outline" asChild>
+        <Link to="/envs/$env/containers/new" params={{ env }} search={{ from: c.Id }}>
+          Duplicate
+        </Link>
+      </Button>
       <Button variant="destructive" onClick={() => setRemoving(true)}>
         Remove
       </Button>
@@ -175,7 +186,7 @@ function Overview({ c }: { c: ContainerInspect }) {
           ["Error", c.State.Error],
           ["Started", c.State.Running ? new Date(c.State.StartedAt!).toLocaleString() : undefined],
           ["Created", new Date(c.Created).toLocaleString()],
-          ["Command", [...(c.Config.Entrypoint ?? []), ...(c.Config.Cmd ?? [])].join(" ")],
+          ["Command", joinArgs([...(c.Config.Entrypoint ?? []), ...(c.Config.Cmd ?? [])])],
           ["Restart policy", restart && restart.Name !== "no" && restart.Name ? restart.Name : "no"],
           ["Ports", ports.length ? ports.join(", ") : "none"],
         ]}
```

`web/src/pages/containers/ContainersPage.tsx`:

```diff
--- a/web/src/pages/containers/ContainersPage.tsx
+++ b/web/src/pages/containers/ContainersPage.tsx
@@ -95,7 +95,21 @@ export function ContainersPage() {
   });
 
   return (
-    <Page title="Containers" actions={<Button variant="outline" onClick={() => setPruning(true)}>Prune</Button>}>
+    <Page
+      title="Containers"
+      actions={
+        <>
+          <Button variant="outline" onClick={() => setPruning(true)}>
+            Prune
+          </Button>
+          <Button asChild>
+            <Link to="/envs/$env/containers/new" params={{ env }}>
+              Create container
+            </Link>
+          </Button>
+        </>
+      }
+    >
       <DataTable
         label="Containers"
         data={list.data}
```

`web/src/router.tsx`:

```diff
--- a/web/src/router.tsx
+++ b/web/src/router.tsx
@@ -129,6 +129,21 @@ const containerRoute = createRoute({
   component: lazyRouteComponent(() => import("@/pages/containers/ContainerPage"), "ContainerPage"),
 });
 
+const newContainerRoute = createRoute({
+  getParentRoute: () => envRoute,
+  path: "containers/new",
+  validateSearch: (s: Record<string, unknown>): { from?: string } => ({
+    from: typeof s.from === "string" && s.from ? s.from : undefined,
+  }),
+  component: lazyRouteComponent(() => import("@/pages/containers/ContainerFormPage"), "NewContainerPage"),
+});
+
+const editContainerRoute = createRoute({
+  getParentRoute: () => envRoute,
+  path: "containers/$id/edit",
+  component: lazyRouteComponent(() => import("@/pages/containers/ContainerFormPage"), "EditContainerPage"),
+});
+
 const accountRoute = createRoute({
   getParentRoute: () => shellRoute,
   path: "/account",
@@ -138,7 +153,13 @@ const accountRoute = createRoute({
 const routeTree = rootRoute.addChildren([
   setupRoute,
   loginRoute,
-  authedRoute.addChildren([indexRoute, shellRoute.addChildren([envRoute.addChildren([dashboardRoute, containersRoute, containerRoute]), accountRoute])]),
+  authedRoute.addChildren([indexRoute, shellRoute.addChildren([envRoute.addChildren([
+        dashboardRoute,
+        containersRoute,
+        newContainerRoute,
+        containerRoute,
+        editContainerRoute,
+      ]), accountRoute])]),
 ]);
 
 export function createAppRouter(queryClient: QueryClient, history?: RouterHistory) {
```

- [ ] **Step 5: Run all checks**

Run: `cd web && pnpm test && pnpm typecheck && pnpm lint`
Expected: 122 passed (22 files); typecheck and lint are clean.

- [ ] **Step 6: Check it against the real server**

Run it in two terminals: `make web && make dev-server` in one, and a browser pointed at `http://127.0.0.1:9000/` in the other.

Setup: `docker run -d --name p3b-web -p 18080:80 nginx:alpine`.

Expected:
- **Containers:**
  - `p3b-web` is listed, and its port links to `http://127.0.0.1:18080`.
  - The page does not scroll sideways, and the table scrolls inside it.
  - Untagged images show as a short id.
- **Detail page:**
  - The overview shows `nginx -g "daemon off;"` and `0.0.0.0:18080 → 80/tcp`.
  - Files → `etc` → `nginx` lists `nginx.conf`, and its download link saves the file.
- **Edit:**
  - The form is pre-filled with the image, the command, the port, the env and the advanced JSON (which includes `HostConfig`).
  - Changing the restart policy and pressing Recreate lands on the new container's page.
  - `docker ps --filter name=p3b-web` shows one container, with the new id.
- Afterwards, run `docker rm -f p3b-web`.

- [ ] **Step 7: Commit**

```bash
git add web/src
git commit -m "feat(web): container create, edit by recreate, and duplicate"
```

---

### Task 6: Images

**Files:**
- Create:
  - `web/src/lib/images.ts`;
  - `web/src/pages/images/ImagesPage.tsx`, `web/src/pages/images/ImagePage.tsx` and `web/src/pages/images/dialogs.tsx`.
- Modify: `web/src/api/docker.ts`, `web/src/api/queries.ts`, `web/src/router.tsx`, `web/src/components/Shell.tsx` and `web/src/test/docker.ts`.
- Test: `web/src/lib/images.test.ts`, `web/src/pages/images/ImagesPage.test.tsx` and `web/src/pages/images/ImagePage.test.tsx`.

**Interfaces:**
- Consumes: `DataTable`, `ConfirmDialog`, `Details`, `DetailError`, `JsonView`, `runBulk`, `bulkMessage`, `joinArgs`, `formatAgo` and `shortId`; and the SDK's `inspectImage`, `getImageHistory`, `deleteImage`, `pruneImages` and `tagImage`.
- Produces `@/api/docker`: `ImageInspect` and `ImageLayer`.
- Produces `@/api/queries`: `imageQuery(env, id)` and `imageHistoryQuery(env, id)`.
- Produces `@/lib/images`: `parseRef(ref): { repo, tag }`. The tag defaults to `latest`, and a registry port is not taken for a tag.
- Produces `pages/images/dialogs.tsx`:
  - `imageName(tags, id)`;
  - `<RemoveImagesDialog env images onDone />`, with Force;
  - `<PruneImagesDialog env onDone />`, whose "all unused" option sets `all=true`;
  - `<TagDialog env id onClose />`.
- Produces:
  - the routes `/envs/$env/images` and `/envs/$env/images/$id`, and an `Images` nav item;
  - `@/test/docker`'s `image(over)`.

- [ ] **Step 1: Write the failing tests**

`web/src/test/docker.ts`:

```diff
--- a/web/src/test/docker.ts
+++ b/web/src/test/docker.ts
@@ -1,4 +1,4 @@
-import type { ContainerSummary } from "@/api/gen/types.gen";
+import type { ContainerSummary, ImageSummary } from "@/api/gen/types.gen";
 import type { ContainerInspect } from "@/api/docker";
 
 /** A container list entry: running nginx, created two hours ago, unless overridden. */
@@ -28,3 +28,7 @@ export function inspect(over: Partial<ContainerInspect> & { Id: string; Name: st
     ...over,
   };
 }
+
+export function image(over: Partial<ImageSummary> & { id: string }): ImageSummary {
+  return { repo_tags: [], size: 50_000_000, created: Math.floor(Date.now() / 1000) - 86400, in_use: false, ...over };
+}
```

`web/src/lib/images.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { parseRef } from "./images";

describe("parseRef", () => {
  it("splits the tag off, defaulting to latest", () => {
    expect(parseRef("app:1.2")).toEqual({ repo: "app", tag: "1.2" });
    expect(parseRef(" app ")).toEqual({ repo: "app", tag: "latest" });
    expect(parseRef("ghcr.io/org/app:2")).toEqual({ repo: "ghcr.io/org/app", tag: "2" });
  });

  it("does not take a registry port for a tag", () => {
    expect(parseRef("registry:5000/app")).toEqual({ repo: "registry:5000/app", tag: "latest" });
    expect(parseRef("registry:5000/app:3")).toEqual({ repo: "registry:5000/app", tag: "3" });
  });
});
```

`web/src/pages/images/ImagesPage.test.tsx`:

```tsx
import { waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { json, mockFetch } from "@/test/fetch";
import { loggedIn, renderApp } from "@/test/app";
import { image } from "@/test/docker";

const NGINX = image({ id: "sha256:aaa111222333444", repo_tags: ["nginx:1.27", "nginx:latest"], in_use: true, size: 190_000_000 });
const OLD = image({ id: "sha256:bbb111222333444", repo_tags: [], created: Math.floor(Date.now() / 1000) - 3 * 86400 });

const routes = { ...loggedIn, "GET /api/v1/envs/local/images": { json: [NGINX, OLD] } };
const rows = (screen: typeof import("@testing-library/react").screen) =>
  within(screen.getByRole("table", { name: "Images" })).getAllByRole("row").slice(1);

describe("images list", () => {
  it("lists tags, short ids, sizes and use, newest first", async () => {
    mockFetch(routes);
    const { screen } = renderApp("/envs/local/images");
    await waitFor(() => expect(rows(screen)).toHaveLength(2));
    const [first, second] = rows(screen);
    expect(first).toHaveTextContent("nginx:1.27, nginx:latest");
    expect(within(first).getByText("aaa111222333")).toBeInTheDocument();
    expect(within(first).getByText("190.0 MB")).toBeInTheDocument();
    expect(within(first).getByText("in use")).toBeInTheDocument();
    expect(second).toHaveTextContent("<none>");
    expect(within(second).getByText("3 days ago")).toBeInTheDocument();
  });

  it("removes selected images, forcing when asked", async () => {
    const calls = mockFetch({
      ...routes,
      "DELETE /api/v1/envs/local/images/sha256%3Abbb111222333444": { json: { removed: ["sha256:bbb"] } },
    });
    const { screen } = renderApp("/envs/local/images");
    await waitFor(() => expect(rows(screen)).toHaveLength(2));
    await userEvent.click(screen.getByRole("checkbox", { name: "Select sha256:bbb111222333444" }));
    await userEvent.click(within(screen.getByRole("group", { name: "Selected" })).getByRole("button", { name: "Remove" }));
    const dialog = await screen.findByRole("dialog", { name: "Remove bbb111222333?" });
    await userEvent.click(within(dialog).getByLabelText(/Force/));
    await userEvent.click(within(dialog).getByRole("button", { name: "Remove" }));
    expect(await screen.findByText("Removed 1 image")).toBeInTheDocument();
    expect(calls.find((c) => c.method === "DELETE")?.path).toBe(
      "/api/v1/envs/local/images/sha256%3Abbb111222333444?force=true",
    );
  });

  it("shows why an image cannot be removed", async () => {
    mockFetch({
      ...routes,
      "DELETE /api/v1/envs/local/images/sha256%3Aaaa111222333444": () =>
        json(409, { code: "conflict", message: "image is being used by running container web" }),
    });
    const { screen } = renderApp("/envs/local/images");
    await waitFor(() => expect(rows(screen)).toHaveLength(2));
    await userEvent.click(screen.getByRole("checkbox", { name: "Select sha256:aaa111222333444" }));
    await userEvent.click(within(screen.getByRole("group", { name: "Selected" })).getByRole("button", { name: "Remove" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.click(within(dialog).getByRole("button", { name: "Remove" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("image is being used by running container web");
  });

  it("prunes unused images when asked for all", async () => {
    const calls = mockFetch({
      ...routes,
      "POST /api/v1/envs/local/images/prune": { json: { deleted: ["a", "b"], space_reclaimed: 300_000_000 } },
    });
    const { screen } = renderApp("/envs/local/images");
    await userEvent.click(await screen.findByRole("button", { name: "Prune" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.click(within(dialog).getByLabelText("Also remove every image no container uses"));
    await userEvent.click(within(dialog).getByRole("button", { name: "Prune" }));
    expect(await screen.findByText("Removed 2 images, reclaimed 300.0 MB")).toBeInTheDocument();
    expect(calls.find((c) => c.method === "POST")?.path).toBe("/api/v1/envs/local/images/prune?all=true");
  });
});
```

`web/src/pages/images/ImagePage.test.tsx`:

```tsx
import { waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { ImageInspect, ImageLayer } from "@/api/docker";
import { json, mockFetch } from "@/test/fetch";
import { loggedIn, renderApp } from "@/test/app";

const IMG: ImageInspect = {
  Id: "sha256:aaa111222333444",
  RepoTags: ["nginx:1.27"],
  Created: "2026-09-01T00:00:00Z",
  Size: 190_000_000,
  Os: "linux",
  Architecture: "arm64",
  Config: { Cmd: ["nginx", "-g", "daemon off;"], ExposedPorts: { "80/tcp": {} }, Env: ["NGINX_VERSION=1.27.0"] },
};
const LAYERS: ImageLayer[] = [
  { Id: "sha256:aaa", Created: Math.floor(Date.now() / 1000) - 86400, CreatedBy: 'CMD ["nginx"]', Size: 0 },
  { Id: "<missing>", Created: Math.floor(Date.now() / 1000) - 86400, CreatedBy: "RUN apk add nginx", Size: 50_000_000 },
];

const routes = {
  ...loggedIn,
  "GET /api/v1/envs/local/images/sha256%3Aaaa111222333444": { json: IMG },
  "GET /api/v1/envs/local/images/sha256%3Aaaa111222333444/history": { json: LAYERS },
  "GET /api/v1/envs/local/images": { json: [] },
};
const PATH = "/envs/local/images/sha256%3Aaaa111222333444";

describe("image page", () => {
  it("shows the image's config", async () => {
    mockFetch(routes);
    const { screen } = renderApp(PATH);
    expect(await screen.findByRole("heading", { name: "nginx:1.27" })).toBeInTheDocument();
    const d = screen.getByLabelText("Image details");
    expect(within(d).getByText("linux/arm64")).toBeInTheDocument();
    expect(within(d).getByText('nginx -g "daemon off;"')).toBeInTheDocument();
    expect(within(d).getByText("80/tcp")).toBeInTheDocument();
    expect(within(d).getByText("NGINX_VERSION=1.27.0")).toBeInTheDocument();
  });

  it("lists the layers", async () => {
    mockFetch(routes);
    const { screen } = renderApp(PATH);
    await userEvent.click(await screen.findByRole("tab", { name: "History" }));
    const table = await screen.findByRole("table", { name: "Layers" });
    expect(within(table).getByText("RUN apk add nginx")).toBeInTheDocument();
    expect(within(table).getByText("50.0 MB")).toBeInTheDocument();
  });

  it("tags the image, splitting repo and tag", async () => {
    const calls = mockFetch({ ...routes, "POST /api/v1/envs/local/images/sha256%3Aaaa111222333444/tag": { status: 204 } });
    const { screen } = renderApp(PATH);
    await userEvent.click(await screen.findByRole("button", { name: "Tag" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText("New tag"), "registry:5000/web:v2");
    await userEvent.click(within(dialog).getByRole("button", { name: "Tag" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(calls.find((c) => c.path.endsWith("/tag"))?.body).toEqual({ repo: "registry:5000/web", tag: "v2" });
  });

  it("goes back to the list after removing the image", async () => {
    mockFetch({
      ...routes,
      "DELETE /api/v1/envs/local/images/sha256%3Aaaa111222333444": { json: { removed: ["sha256:aaa"] } },
    });
    const { screen, history } = renderApp(PATH);
    await userEvent.click(await screen.findByRole("button", { name: "Remove" }));
    await userEvent.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(history.location.pathname).toBe("/envs/local/images"));
  });

  it("says so when the image is gone", async () => {
    mockFetch({
      ...routes,
      "GET /api/v1/envs/local/images/sha256%3Aaaa111222333444": () => json(404, { code: "not_found", message: "no such image" }),
    });
    const { screen } = renderApp(PATH);
    expect(await screen.findByRole("alert")).toHaveTextContent("This image no longer exists.");
  });
});
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cd web && pnpm test src/lib/images.test.ts src/pages/images`
Expected: FAIL, 3 files. `images.test.ts` fails with `Failed to resolve import "./images"`, and all 9 page tests fail with `Unable to find role="table" and name "Images"`, `role="heading" and name "nginx:1.27"` and `role="alert"`.

- [ ] **Step 3: Implement**

`web/src/lib/images.ts`:

```ts
/**
 * A `repo[:tag]` reference split into its parts; the tag defaults to
 * `latest`. A colon before the last slash belongs to a registry port
 * (`registry:5000/app` has no tag).
 */
export function parseRef(ref: string): { repo: string; tag: string } {
  const r = ref.trim();
  const slash = r.lastIndexOf("/");
  const colon = r.lastIndexOf(":");
  return colon > slash ? { repo: r.slice(0, colon), tag: r.slice(colon + 1) } : { repo: r, tag: "latest" };
}
```

`web/src/api/docker.ts`:

```diff
--- a/web/src/api/docker.ts
+++ b/web/src/api/docker.ts
@@ -48,3 +48,34 @@ export interface ContainerInspect {
     > | null;
   };
 }
+
+export interface ImageInspect {
+  Id: string;
+  RepoTags?: string[] | null;
+  RepoDigests?: string[] | null;
+  Created: string;
+  Size: number;
+  Architecture?: string;
+  Os?: string;
+  Config?: {
+    Env?: string[] | null;
+    Cmd?: string[] | null;
+    Entrypoint?: string[] | null;
+    ExposedPorts?: Record<string, object> | null;
+    Labels?: Record<string, string> | null;
+    WorkingDir?: string;
+    User?: string;
+  } | null;
+}
+
+/** One layer of `GET /images/{id}/history`, newest first. */
+export interface ImageLayer {
+  /** `<missing>` for layers built elsewhere. */
+  Id: string;
+  /** Unix seconds. */
+  Created: number;
+  CreatedBy: string;
+  Size: number;
+  Tags?: string[] | null;
+  Comment?: string;
+}
```

`web/src/api/queries.ts`:

```diff
--- a/web/src/api/queries.ts
+++ b/web/src/api/queries.ts
@@ -1,14 +1,16 @@
 import { queryOptions } from "@tanstack/react-query";
 import { ApiError, setCsrfToken } from "./client";
-import type { ContainerInspect } from "./docker";
+import type { ContainerInspect, ImageInspect, ImageLayer } from "./docker";
 import {
   getAuthStatus,
   getDiskUsage,
   getEngineInfo,
   getEnvStatus,
+  getImageHistory,
   getContainerSpec,
   getMe,
   inspectContainer,
+  inspectImage,
   listContainerFiles,
   listContainers,
   listEnvs,
@@ -128,6 +130,20 @@ export const containerFilesQuery = (env: string, id: string, dir: string) =>
 export const imagesQuery = (env: string) =>
   queryOptions({ queryKey: keys.images(env), queryFn: async () => (await listImages(path(env))).data });
 
+export const imageQuery = (env: string, id: string) =>
+  queryOptions({
+    queryKey: keys.image(env, id),
+    queryFn: async () => (await inspectImage({ path: { env, id } })).data as unknown as ImageInspect,
+    retry: retryUnlessGone,
+  });
+
+export const imageHistoryQuery = (env: string, id: string) =>
+  queryOptions({
+    queryKey: [...keys.image(env, id), "history"] as const,
+    queryFn: async () => (await getImageHistory({ path: { env, id } })).data as unknown as ImageLayer[],
+    retry: retryUnlessGone,
+  });
+
 export const volumesQuery = (env: string) =>
   queryOptions({ queryKey: keys.volumes(env), queryFn: async () => (await listVolumes(path(env))).data });
 
```

`web/src/pages/images/dialogs.tsx`:

```tsx
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useState, type FormEvent } from "react";
import { toast } from "sonner";
import { keys } from "@/api/queries";
import { deleteImage, pruneImages, tagImage } from "@/api/gen/sdk.gen";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { bulkMessage, runBulk } from "@/lib/bulk";
import { errorMessage } from "@/lib/errors";
import { formatBytes, shortId } from "@/lib/format";
import { parseRef } from "@/lib/images";
import { FormError } from "@/pages/AuthLayout";

/** An image's first tag, or its short id when it has none. */
export function imageName(tags: string[] | null | undefined, id: string) {
  return tags?.[0] ?? shortId(id);
}

export function RemoveImagesDialog({
  env,
  images,
  onDone,
}: {
  env: string;
  images: { id: string; name: string }[];
  onDone: (removed: boolean) => void;
}) {
  const qc = useQueryClient();
  const [force, setForce] = useState(false);
  const one = images.length === 1;
  return (
    <ConfirmDialog
      open
      onOpenChange={(open) => !open && onDone(false)}
      title={one ? `Remove ${images[0].name}?` : `Remove ${images.length} images?`}
      description={one ? undefined : images.map((i) => i.name).join(", ")}
      confirmLabel="Remove"
      onConfirm={async () => {
        const r = await runBulk(images, (i) => i.name, (i) => deleteImage({ path: { env, id: i.id }, query: { force } }));
        await qc.invalidateQueries({ queryKey: keys.images(env) });
        if (r.ok === 0) throw one ? r.failed[0].error : new Error(bulkMessage("Removed", "image", r));
        const msg = bulkMessage("Removed", "image", r);
        if (r.failed.length) toast.error(msg);
        else toast.success(msg);
        onDone(true);
      }}
    >
      <div className="flex items-center gap-2">
        <Checkbox id="img-force" checked={force} onCheckedChange={(v) => setForce(!!v)} />
        <Label htmlFor="img-force">Force (remove every tag, even if a stopped container uses it)</Label>
      </div>
    </ConfirmDialog>
  );
}

export function PruneImagesDialog({ env, onDone }: { env: string; onDone: () => void }) {
  const qc = useQueryClient();
  const [all, setAll] = useState(false);
  return (
    <ConfirmDialog
      open
      onOpenChange={(open) => !open && onDone()}
      title="Prune images?"
      description="Removes dangling images: untagged layers no container uses."
      confirmLabel="Prune"
      onConfirm={async () => {
        const { data } = await pruneImages({ path: { env }, query: { all } });
        await qc.invalidateQueries({ queryKey: keys.images(env) });
        toast.success(`Removed ${data.deleted.length} images, reclaimed ${formatBytes(data.space_reclaimed)}`);
        onDone();
      }}
    >
      <div className="flex items-center gap-2">
        <Checkbox id="prune-all" checked={all} onCheckedChange={(v) => setAll(!!v)} />
        <Label htmlFor="prune-all">Also remove every image no container uses</Label>
      </div>
    </ConfirmDialog>
  );
}

/** Adds a `repo:tag` to an image. */
export function TagDialog({ env, id, onClose }: { env: string; id: string; onClose: () => void }) {
  const qc = useQueryClient();
  const [ref, setRef] = useState("");
  const m = useMutation({
    meta: { inline: true },
    mutationFn: () => tagImage({ path: { env, id }, body: parseRef(ref) }),
    onSuccess: async () => {
      await qc.invalidateQueries({ queryKey: keys.images(env) });
      toast.success(`Tagged ${ref}`);
      onClose();
    },
  });
  const submit = (e: FormEvent) => {
    e.preventDefault();
    m.mutate();
  };
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Tag image</DialogTitle>
        </DialogHeader>
        <form onSubmit={submit} className="grid gap-4">
          <div className="grid gap-2">
            <Label htmlFor="tag-ref">New tag</Label>
            <Input id="tag-ref" required placeholder="app:1.2" value={ref} onChange={(e) => setRef(e.target.value)} />
          </div>
          <FormError message={m.error ? errorMessage(m.error) : null} />
          <DialogFooter>
            <Button type="submit" disabled={m.isPending}>
              Tag
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
```

`web/src/pages/images/ImagesPage.tsx`:

```tsx
import { useQuery } from "@tanstack/react-query";
import { Link, useParams } from "@tanstack/react-router";
import { useState } from "react";
import { imagesQuery } from "@/api/queries";
import type { ImageSummary } from "@/api/gen/types.gen";
import { columns, DataTable, type Column } from "@/components/DataTable";
import { Page } from "@/components/Page";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { formatAgo, formatBytes, shortId } from "@/lib/format";
import { imageName, PruneImagesDialog, RemoveImagesDialog } from "./dialogs";

const col = columns<ImageSummary>();

function imageColumns(env: string): Column<ImageSummary>[] {
  return [
    col.accessor((i) => imageName(i.repo_tags, i.id), {
      id: "tags",
      header: "Tags",
      enableHiding: false,
      cell: (c) => (
        <Link
          to="/envs/$env/images/$id"
          params={{ env, id: c.row.original.id }}
          className="font-medium hover:underline"
        >
          {c.row.original.repo_tags.length ? c.row.original.repo_tags.join(", ") : <span className="text-muted-foreground">&lt;none&gt;</span>}
        </Link>
      ),
    }),
    col.accessor((i) => shortId(i.id), { id: "id", header: "ID", cell: (c) => <code className="text-xs">{c.getValue()}</code> }),
    col.accessor("size", { header: "Size", cell: (c) => formatBytes(c.getValue()) }),
    col.accessor("created", { header: "Created", cell: (c) => formatAgo(c.getValue()) }),
    col.accessor((i) => (i.in_use ? "in use" : "unused"), {
      id: "in_use",
      header: "Used",
      cell: (c) => (c.row.original.in_use ? <Badge variant="secondary">in use</Badge> : <span className="text-muted-foreground">unused</span>),
    }),
  ];
}

export function ImagesPage() {
  const { env } = useParams({ from: "/authed/shell/envs/$env/images" });
  const list = useQuery(imagesQuery(env));
  const [cols] = useState(() => imageColumns(env));
  const [removing, setRemoving] = useState<ImageSummary[] | null>(null);
  const [pruning, setPruning] = useState(false);
  return (
    <Page
      title="Images"
      actions={
        <Button variant="outline" onClick={() => setPruning(true)}>
          Prune
        </Button>
      }
    >
      <DataTable
        label="Images"
        data={list.data}
        error={list.error}
        columns={cols}
        getRowId={(i) => i.id}
        empty="No images in this environment."
        initialSorting={[{ id: "created", desc: true }]}
        selectable
        actions={(selected, clear) => (
          <Button
            variant="destructive"
            size="sm"
            onClick={() => {
              setRemoving(selected);
              clear();
            }}
          >
            Remove
          </Button>
        )}
      />
      {removing && (
        <RemoveImagesDialog
          env={env}
          images={removing.map((i) => ({ id: i.id, name: imageName(i.repo_tags, i.id) }))}
          onDone={() => setRemoving(null)}
        />
      )}
      {pruning && <PruneImagesDialog env={env} onDone={() => setPruning(false)} />}
    </Page>
  );
}
```

`web/src/pages/images/ImagePage.tsx`:

```tsx
import { useQuery } from "@tanstack/react-query";
import { Link, useNavigate, useParams } from "@tanstack/react-router";
import { useState } from "react";
import { imageHistoryQuery, imageQuery } from "@/api/queries";
import type { ImageInspect } from "@/api/docker";
import { Details, DetailError } from "@/components/Details";
import { JsonView } from "@/components/JsonView";
import { Page } from "@/components/Page";
import { Button } from "@/components/ui/button";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { joinArgs } from "@/lib/args";
import { errorMessage } from "@/lib/errors";
import { formatAgo, formatBytes, shortId } from "@/lib/format";
import { imageName, RemoveImagesDialog, TagDialog } from "./dialogs";

export function ImagePage() {
  const { env, id } = useParams({ from: "/authed/shell/envs/$env/images/$id" });
  const navigate = useNavigate();
  const q = useQuery(imageQuery(env, id));
  const [tagging, setTagging] = useState(false);
  const [removing, setRemoving] = useState(false);
  const back = (
    <Link to="/envs/$env/images" params={{ env }} className="underline">
      Back to images
    </Link>
  );
  if (q.error) return <DetailError error={q.error} what="image" back={back} />;
  const img = q.data;
  if (!img) return <p className="p-6 text-sm text-muted-foreground">Loading…</p>;
  const name = imageName(img.RepoTags, img.Id);
  return (
    <Page
      title={name}
      actions={
        <>
          <Button variant="outline" onClick={() => setTagging(true)}>
            Tag
          </Button>
          <Button variant="destructive" onClick={() => setRemoving(true)}>
            Remove
          </Button>
        </>
      }
    >
      <Tabs defaultValue="overview">
        <TabsList>
          <TabsTrigger value="overview">Overview</TabsTrigger>
          <TabsTrigger value="history">History</TabsTrigger>
          <TabsTrigger value="inspect">Inspect</TabsTrigger>
        </TabsList>
        <TabsContent value="overview">
          <Overview img={img} />
        </TabsContent>
        <TabsContent value="history">
          <History env={env} id={id} />
        </TabsContent>
        <TabsContent value="inspect">
          <JsonView value={img} label="Image JSON" />
        </TabsContent>
      </Tabs>
      {tagging && <TagDialog env={env} id={img.Id} onClose={() => setTagging(false)} />}
      {removing && (
        <RemoveImagesDialog
          env={env}
          images={[{ id: img.Id, name }]}
          onDone={(removed) => {
            setRemoving(false);
            if (removed) void navigate({ to: "/envs/$env/images", params: { env } });
          }}
        />
      )}
    </Page>
  );
}

function Overview({ img }: { img: ImageInspect }) {
  const cfg = img.Config ?? {};
  return (
    <Details
      label="Image details"
      items={[
        ["ID", <code key="id">{shortId(img.Id)}</code>],
        ["Tags", img.RepoTags?.join(", ") || "none"],
        ["Digests", img.RepoDigests?.join(", ")],
        ["Size", formatBytes(img.Size)],
        ["Created", new Date(img.Created).toLocaleString()],
        ["Platform", [img.Os, img.Architecture].filter(Boolean).join("/")],
        ["Entrypoint", cfg.Entrypoint ? joinArgs(cfg.Entrypoint) : undefined],
        ["Command", cfg.Cmd ? joinArgs(cfg.Cmd) : undefined],
        ["Working directory", cfg.WorkingDir],
        ["User", cfg.User],
        ["Exposed ports", Object.keys(cfg.ExposedPorts ?? {}).join(", ")],
        ["Environment", cfg.Env?.length ? <EnvList key="env" env={cfg.Env} /> : undefined],
        [
          "Labels",
          Object.keys(cfg.Labels ?? {}).length ? (
            <EnvList key="labels" env={Object.entries(cfg.Labels!).map(([k, v]) => `${k}=${v}`)} />
          ) : undefined,
        ],
      ]}
    />
  );
}

function EnvList({ env }: { env: string[] }) {
  return (
    <ul className="grid gap-0.5 font-mono text-xs">
      {env.map((e) => (
        <li key={e} className="break-all">
          {e}
        </li>
      ))}
    </ul>
  );
}

function History({ env, id }: { env: string; id: string }) {
  const q = useQuery(imageHistoryQuery(env, id));
  if (q.error)
    return (
      <p role="alert" className="text-sm text-destructive">
        {errorMessage(q.error)}
      </p>
    );
  if (!q.data) return <p className="text-sm text-muted-foreground">Loading…</p>;
  return (
    <Table aria-label="Layers">
      <TableHeader>
        <TableRow>
          <TableHead>Created</TableHead>
          <TableHead>Size</TableHead>
          <TableHead>Created by</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {q.data.map((l, i) => (
          <TableRow key={i}>
            <TableCell>{formatAgo(l.Created)}</TableCell>
            <TableCell>{formatBytes(l.Size)}</TableCell>
            <TableCell className="max-w-3xl font-mono text-xs whitespace-normal">
              <span className="line-clamp-3 break-all" title={l.CreatedBy}>
                {l.CreatedBy}
              </span>
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}
```

`web/src/router.tsx`:

```diff
--- a/web/src/router.tsx
+++ b/web/src/router.tsx
@@ -144,6 +144,18 @@ const editContainerRoute = createRoute({
   component: lazyRouteComponent(() => import("@/pages/containers/ContainerFormPage"), "EditContainerPage"),
 });
 
+const imagesRoute = createRoute({
+  getParentRoute: () => envRoute,
+  path: "images",
+  component: lazyRouteComponent(() => import("@/pages/images/ImagesPage"), "ImagesPage"),
+});
+
+const imageRoute = createRoute({
+  getParentRoute: () => envRoute,
+  path: "images/$id",
+  component: lazyRouteComponent(() => import("@/pages/images/ImagePage"), "ImagePage"),
+});
+
 const accountRoute = createRoute({
   getParentRoute: () => shellRoute,
   path: "/account",
@@ -159,6 +171,8 @@ const routeTree = rootRoute.addChildren([
         newContainerRoute,
         containerRoute,
         editContainerRoute,
+        imagesRoute,
+        imageRoute,
       ]), accountRoute])]),
 ]);
 
```

`web/src/components/Shell.tsx`:

```diff
--- a/web/src/components/Shell.tsx
+++ b/web/src/components/Shell.tsx
@@ -1,6 +1,6 @@
 import { useQuery } from "@tanstack/react-query";
 import { Link, Outlet, useNavigate, useParams } from "@tanstack/react-router";
-import { Boxes, ChevronsUpDown, LayoutDashboard, LogOut, Monitor, Moon, Sun, User } from "lucide-react";
+import { Boxes, ChevronsUpDown, Layers, LayoutDashboard, LogOut, Monitor, Moon, Sun, User } from "lucide-react";
 import { envsQuery, envStatusQuery } from "@/api/queries";
 import { useLogout, useSession } from "@/auth/session";
 import { Badge } from "@/components/ui/badge";
@@ -22,6 +22,7 @@ import { useTheme, type Theme } from "@/theme";
 const NAV = [
   { to: "/envs/$env", label: "Dashboard", icon: LayoutDashboard },
   { to: "/envs/$env/containers", label: "Containers", icon: Boxes },
+  { to: "/envs/$env/images", label: "Images", icon: Layers },
 ] as const;
 
 export function Shell() {
```

- [ ] **Step 4: Run all checks**

Run: `cd web && pnpm test && pnpm typecheck && pnpm lint`
Expected: 133 passed (25 files); typecheck and lint are clean.

- [ ] **Step 5: Commit**

```bash
git add web/src
git commit -m "feat(web): images list and page with tag, remove, prune and history"
```

---

### Task 7: Pull and build, with progress over the `task` topic

**Files:**
- Create:
  - `web/src/live/tasks.ts` and `web/src/live/TaskProvider.tsx`;
  - `web/src/lib/progress.ts`;
  - `web/src/pages/images/TaskDialogs.tsx`.
- Modify: `web/src/router.tsx` (mount `TaskProvider`) and `web/src/pages/images/ImagesPage.tsx`.
- Test: `web/src/live/tasks.test.ts`, `web/src/lib/progress.test.ts` and `web/src/pages/images/TaskDialogs.test.tsx`.

**Interfaces:**
- Consumes:
  - `WsClient.subscribe` and `SubHandlers`, including `onResubscribe` (P3a Task 2), and `useLive` (P3a Task 6);
  - the SDK's `pullImage` and `buildImage`;
  - the server's `task` topic, which sends `data` frames carrying Docker's progress JSON, then one `end` with `reason` `succeeded` or `failed` and `task.error`.
- Produces `@/live/tasks`:
  - `TASK_MESSAGES_MAX` (5000);
  - `TrackedTask`, which is `{ id, env, label, messages, status, error? }`;
  - `class TaskTracker(ws, onDone)`, with `track(id, env, label)`, `get(id)` and `subscribe(listener)` (for `useSyncExternalStore`).
- Produces `@/live/TaskProvider`: `<TaskProvider>`, `useTaskTracker()` and `useTask(id | null)`.
- Produces `@/lib/progress`: `pullProgress(messages)`, which returns `{ layers: LayerProgress[], lines }`, and `buildLog(messages)`.
- Produces `pages/images/TaskDialogs.tsx`: `<PullDialog env onClose />`, `<BuildDialog env onClose />` and `BUILD_UPLOAD_MAX`, the server's 100 MB limit.

- [ ] **Step 1: Write the failing tests**

`web/src/live/tasks.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { FakeWebSocket } from "@/test/fake-ws";
import { TaskTracker, type TrackedTask } from "./tasks";
import { WsClient } from "./ws";

function setup() {
  const ws = new WsClient({ url: "ws://x/api/v1/ws", backoff: () => 0 });
  ws.start();
  const sock = FakeWebSocket.latest();
  sock.open();
  const done: TrackedTask[] = [];
  const tracker = new TaskTracker(ws, (t) => done.push(t));
  return { ws, sock, tracker, done };
}

describe("TaskTracker", () => {
  it("collects progress and reports success once", () => {
    const { sock, tracker, done } = setup();
    tracker.track("t1", "local", "Pull nginx");
    tracker.track("t1", "local", "Pull nginx");
    const subs = sock.frames().filter((f) => f.op === "sub");
    expect(subs).toHaveLength(1);
    expect(subs[0]).toMatchObject({ topic: "task", id: "t1" });
    const sub = subs[0].sub;
    sock.receive({ sub, type: "data", data: { status: "Downloading" } });
    expect(tracker.get("t1")?.messages).toEqual([{ status: "Downloading" }]);
    sock.receive({ sub, type: "end", reason: "succeeded", task: { status: "succeeded" } });
    expect(tracker.get("t1")?.status).toBe("succeeded");
    expect(done.map((t) => t.status)).toEqual(["succeeded"]);
  });

  it("reports the task's error when it fails", () => {
    const { sock, tracker, done } = setup();
    tracker.track("t1", "local", "Pull nope");
    const sub = sock.frames()[0].sub;
    sock.receive({ sub, type: "end", reason: "failed", task: { status: "failed", error: "manifest unknown" } });
    expect(done[0]).toMatchObject({ status: "failed", error: "manifest unknown" });
  });

  it("fails a task the server will not follow", () => {
    const { sock, tracker, done } = setup();
    tracker.track("gone", "local", "Build app");
    const sub = sock.frames()[0].sub;
    sock.receive({ sub, type: "error", code: "not_found", message: "task 'gone' not found" });
    expect(done[0]).toMatchObject({ status: "failed", error: "task 'gone' not found" });
  });

  it("starts over when the server resends the backlog after a reconnect", () => {
    const { sock, tracker } = setup();
    tracker.track("t1", "local", "Pull nginx");
    sock.receive({ sub: sock.frames()[0].sub, type: "data", data: { status: "a" } });
    sock.drop();
    const next = FakeWebSocket.latest();
    return new Promise<void>((resolve) =>
      setTimeout(() => {
        next.open();
        expect(tracker.get("t1")?.messages).toEqual([]);
        next.receive({ sub: next.frames()[0].sub, type: "data", data: { status: "a" } });
        expect(tracker.get("t1")?.messages).toEqual([{ status: "a" }]);
        resolve();
      }, 5),
    );
  });
});
```

`web/src/lib/progress.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { buildLog, pullProgress } from "./progress";

describe("pullProgress", () => {
  it("keeps each layer's latest status and bytes, and the other lines in order", () => {
    const p = pullProgress([
      { status: "Pulling from library/nginx", id: "1.27" },
      { status: "Pulling fs layer", id: "aaaaaaaaaaaa" },
      { status: "Pulling fs layer", id: "bbbbbbbbbbbb" },
      { status: "Downloading", id: "aaaaaaaaaaaa", progressDetail: { current: 500, total: 1000 } },
      { status: "Downloading", id: "aaaaaaaaaaaa", progressDetail: { current: 1000, total: 1000 } },
      { status: "Download complete", id: "aaaaaaaaaaaa", progressDetail: {} },
      { status: "Digest: sha256:123" },
      { status: "Status: Downloaded newer image for nginx:1.27" },
    ]);
    expect(p.layers).toEqual([
      { id: "aaaaaaaaaaaa", status: "Download complete", current: 1000, total: 1000 },
      { id: "bbbbbbbbbbbb", status: "Pulling fs layer", current: undefined, total: undefined },
    ]);
    expect(p.lines).toEqual([
      "Pulling from library/nginx 1.27",
      "Digest: sha256:123",
      "Status: Downloaded newer image for nginx:1.27",
    ]);
  });

  it("includes Docker's error", () => {
    expect(pullProgress([{ error: "manifest unknown", errorDetail: { message: "manifest unknown" } }]).lines).toEqual([
      "manifest unknown",
    ]);
  });
});

describe("buildLog", () => {
  it("splits stream output into lines and notes the result", () => {
    expect(
      buildLog([
        { stream: "Step 1/2 : FROM alpine\n" },
        { stream: "\n" },
        { stream: " ---> abc\nStep 2/2 : RUN true\n" },
        { aux: { ID: "sha256:def" } },
        { error: "The command '/bin/sh -c false' returned a non-zero code: 1" },
      ]),
    ).toEqual([
      "Step 1/2 : FROM alpine",
      " ---> abc",
      "Step 2/2 : RUN true",
      "Built sha256:def",
      "The command '/bin/sh -c false' returned a non-zero code: 1",
    ]);
  });
});
```

`web/src/pages/images/TaskDialogs.test.tsx` (see Global Constraints for why uploads are checked at `buildImage`):

```tsx
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
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cd web && pnpm test src/live/tasks.test.ts src/lib/progress.test.ts src/pages/images/TaskDialogs.test.tsx`
Expected: FAIL, 3 files, with `Failed to resolve import "./tasks"`, `"./progress"` and `"./TaskDialogs"`.

- [ ] **Step 3: Implement the tracker**

`web/src/live/tasks.ts`:

```ts
import type { TaskInfo } from "@/api/gen/types.gen";
import type { WsClient } from "./ws";

/** Progress messages kept per task; the oldest go first. */
export const TASK_MESSAGES_MAX = 5000;

export interface TrackedTask {
  id: string;
  env: string;
  /** What the user started, for the result toast: "Pull nginx:1.27". */
  label: string;
  /** Docker's progress messages, as the task topic sends them. */
  messages: unknown[];
  status: TaskInfo["status"];
  error?: string;
}

/**
 * Follows background tasks (image pull and build) over the websocket
 * `task` topic until they end, whether or not a dialog is still showing
 * them, and reports each result once through `onDone`.
 */
export class TaskTracker {
  private readonly ws: WsClient;
  private readonly onDone: (t: TrackedTask) => void;
  private readonly tasks = new Map<string, TrackedTask>();
  private readonly listeners = new Set<() => void>();

  constructor(ws: WsClient, onDone: (t: TrackedTask) => void) {
    this.ws = ws;
    this.onDone = onDone;
  }

  track(id: string, env: string, label: string) {
    if (this.tasks.has(id)) return;
    this.tasks.set(id, { id, env, label, messages: [], status: "running" });
    this.emit();
    const { unsubscribe } = this.ws.subscribe(
      { topic: "task", id },
      {
        onData: (d) =>
          this.update(id, (t) => ({ ...t, messages: [...t.messages, d].slice(-TASK_MESSAGES_MAX) })),
        // The server sends everything so far again on a new subscription.
        onResubscribe: () => this.update(id, (t) => ({ ...t, messages: [] })),
        onEnd: (m) => {
          const failed = m.reason !== "succeeded";
          this.finish(id, failed ? "failed" : "succeeded", failed ? (m.task?.error ?? `task ${m.reason}`) : undefined);
        },
        onError: (m) => {
          this.finish(id, "failed", m.message);
          unsubscribe();
        },
      },
    );
  }

  get(id: string): TrackedTask | undefined {
    return this.tasks.get(id);
  }

  /** For `useSyncExternalStore`. */
  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => void this.listeners.delete(listener);
  };

  private finish(id: string, status: TrackedTask["status"], error?: string) {
    const t = this.tasks.get(id);
    if (!t || t.status !== "running") return;
    this.update(id, (old) => ({ ...old, status, error }));
    this.onDone(this.tasks.get(id)!);
  }

  private update(id: string, f: (t: TrackedTask) => TrackedTask) {
    const t = this.tasks.get(id);
    if (!t) return;
    this.tasks.set(id, f(t));
    this.emit();
  }

  private emit() {
    for (const l of this.listeners) l();
  }
}
```

`web/src/live/TaskProvider.tsx`:

```tsx
import { useQueryClient } from "@tanstack/react-query";
import { createContext, useContext, useState, useSyncExternalStore, type ReactNode } from "react";
import { toast } from "sonner";
import { keys } from "@/api/queries";
import { useLive } from "./LiveProvider";
import { TaskTracker, type TrackedTask } from "./tasks";

const TaskContext = createContext<TaskTracker | null>(null);

/** Tracks the tab's background tasks; each result is a toast, and refreshes the image list. */
export function TaskProvider({ children }: { children: ReactNode }) {
  const { ws } = useLive();
  const qc = useQueryClient();
  const [tracker] = useState(
    () =>
      new TaskTracker(ws!, (t) => {
        void qc.invalidateQueries({ queryKey: keys.images(t.env) });
        if (t.status === "succeeded") toast.success(`${t.label}: done`);
        else toast.error(`${t.label} failed: ${t.error}`);
      }),
  );
  return <TaskContext value={tracker}>{children}</TaskContext>;
}

export function useTaskTracker() {
  const t = useContext(TaskContext);
  if (!t) throw new Error("useTaskTracker outside TaskProvider");
  return t;
}

/** A tracked task, re-rendering as its progress arrives. */
export function useTask(id: string | null): TrackedTask | undefined {
  const tracker = useTaskTracker();
  return useSyncExternalStore(tracker.subscribe, () => (id ? tracker.get(id) : undefined));
}
```

`web/src/router.tsx`:

```diff
--- a/web/src/router.tsx
+++ b/web/src/router.tsx
@@ -12,6 +12,7 @@ import { ApiError } from "@/api/client";
 import { authStatusQuery, envsQuery, meQuery } from "@/api/queries";
 import { Shell } from "@/components/Shell";
 import { LiveProvider } from "@/live/LiveProvider";
+import { TaskProvider } from "@/live/TaskProvider";
 import { LoginPage } from "@/pages/LoginPage";
 import { NoEnvironmentsPage, NotFoundPage } from "@/pages/misc";
 import { CONTAINER_TABS, type ContainerTab } from "@/pages/containers/tabs";
@@ -80,7 +81,9 @@ const authedRoute = createRoute({
   },
   component: () => (
     <LiveProvider>
-      <Outlet />
+      <TaskProvider>
+        <Outlet />
+      </TaskProvider>
     </LiveProvider>
   ),
 });
```

- [ ] **Step 4: Implement the progress views and dialogs**

`web/src/lib/progress.ts`:

```ts
export interface LayerProgress {
  id: string;
  status: string;
  current?: number;
  total?: number;
}

interface DockerProgress {
  id?: string;
  status?: string;
  progressDetail?: { current?: number; total?: number };
  stream?: string;
  error?: string;
  aux?: { ID?: string };
}

const LAYER = /^[0-9a-f]{12}$/;

/**
 * An image pull's progress messages as one row per layer (its latest
 * status and byte counts) plus the other status lines in order.
 */
export function pullProgress(messages: unknown[]): { layers: LayerProgress[]; lines: string[] } {
  const layers = new Map<string, LayerProgress>();
  const lines: string[] = [];
  for (const raw of messages) {
    const m = raw as DockerProgress;
    if (m.error) lines.push(m.error);
    else if (m.id && LAYER.test(m.id) && m.status) {
      const d = m.progressDetail;
      const old = layers.get(m.id);
      layers.set(m.id, {
        id: m.id,
        status: m.status,
        current: d?.total ? d.current : m.status.endsWith("complete") ? old?.total : undefined,
        total: d?.total || (m.status.endsWith("complete") ? old?.total : undefined),
      });
    } else if (m.status) lines.push(m.id ? `${m.status} ${m.id}`.trim() : m.status);
  }
  return { layers: [...layers.values()], lines };
}

/** A build's progress messages as log lines, without Docker's blank separator lines. */
export function buildLog(messages: unknown[]): string[] {
  const out: string[] = [];
  for (const raw of messages) {
    const m = raw as DockerProgress;
    if (m.stream) out.push(...m.stream.split("\n").filter((l) => l.trim()));
    else if (m.error) out.push(m.error);
    else if (m.aux?.ID) out.push(`Built ${m.aux.ID}`);
    else if (m.status) out.push(m.id ? `${m.id}: ${m.status}` : m.status);
  }
  return out;
}
```

`web/src/pages/images/TaskDialogs.tsx`:

```tsx
import { useMutation } from "@tanstack/react-query";
import { useEffect, useRef, useState, type FormEvent, type ReactNode } from "react";
import { buildImage, pullImage } from "@/api/gen/sdk.gen";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { useTask, useTaskTracker } from "@/live/TaskProvider";
import type { TrackedTask } from "@/live/tasks";
import { errorMessage } from "@/lib/errors";
import { formatBytes } from "@/lib/format";
import { buildLog, pullProgress } from "@/lib/progress";
import { FormError } from "@/pages/AuthLayout";

/** The server's upload limit for a build context. */
export const BUILD_UPLOAD_MAX = 100 * 1024 * 1024;

export function PullDialog({ env, onClose }: { env: string; onClose: () => void }) {
  const tracker = useTaskTracker();
  const [ref, setRef] = useState("");
  const [taskId, setTaskId] = useState<string | null>(null);
  const task = useTask(taskId);
  const m = useMutation({
    meta: { inline: true },
    mutationFn: () => pullImage({ path: { env }, body: { image: ref.trim() } }),
    onSuccess: ({ data }) => {
      tracker.track(data.task_id, env, `Pull ${ref.trim()}`);
      setTaskId(data.task_id);
    },
  });
  const submit = (e: FormEvent) => {
    e.preventDefault();
    m.mutate();
  };
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>Pull image</DialogTitle>
          <DialogDescription>From Docker Hub or any registry the engine can reach without logging in.</DialogDescription>
        </DialogHeader>
        {task ? (
          <TaskView task={task} onClose={onClose}>
            <PullView messages={task.messages} />
          </TaskView>
        ) : (
          <form onSubmit={submit} className="grid gap-4">
            <div className="grid gap-2">
              <Label htmlFor="pull-ref">Image</Label>
              <Input
                id="pull-ref"
                required
                placeholder="nginx:1.27 or ghcr.io/org/app:2"
                value={ref}
                onChange={(e) => setRef(e.target.value)}
              />
            </div>
            <FormError message={m.error ? errorMessage(m.error) : null} />
            <DialogFooter>
              <Button type="submit" disabled={m.isPending}>
                Pull
              </Button>
            </DialogFooter>
          </form>
        )}
      </DialogContent>
    </Dialog>
  );
}

const isArchive = (name: string) => /\.(tar|tar\.gz|tgz)$/i.test(name);

export function BuildDialog({ env, onClose }: { env: string; onClose: () => void }) {
  const tracker = useTaskTracker();
  const [file, setFile] = useState<File | null>(null);
  const [tag, setTag] = useState("");
  const [path, setPath] = useState("");
  const [invalid, setInvalid] = useState<string | null>(null);
  const [taskId, setTaskId] = useState<string | null>(null);
  const task = useTask(taskId);
  const archive = !!file && isArchive(file.name);
  const m = useMutation({
    meta: { inline: true },
    mutationFn: (f: File) =>
      buildImage({
        path: { env },
        body: archive
          ? { context: f, tag: tag.trim() || null, dockerfile_path: path.trim() || null }
          : { dockerfile: f, tag: tag.trim() || null },
      }),
    onSuccess: ({ data }) => {
      tracker.track(data.task_id, env, `Build ${tag.trim() || file!.name}`);
      setTaskId(data.task_id);
    },
  });
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (!file) return setInvalid("Choose a Dockerfile or a tar of the build context.");
    if (file.size > BUILD_UPLOAD_MAX) {
      return setInvalid(`${file.name} is ${formatBytes(file.size)}; the limit is ${formatBytes(BUILD_UPLOAD_MAX)}.`);
    }
    setInvalid(null);
    m.mutate(file);
  };
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>Build image</DialogTitle>
          <DialogDescription>
            Upload a lone Dockerfile, or a .tar / .tar.gz of the whole build context.
          </DialogDescription>
        </DialogHeader>
        {task ? (
          <TaskView task={task} onClose={onClose}>
            <LogView lines={buildLog(task.messages)} />
          </TaskView>
        ) : (
          <form onSubmit={submit} className="grid gap-4">
            <div className="grid gap-2">
              <Label htmlFor="build-file">Dockerfile or context</Label>
              <Input id="build-file" type="file" onChange={(e) => setFile(e.target.files?.[0] ?? null)} />
            </div>
            <div className="grid gap-2">
              <Label htmlFor="build-tag">Tag</Label>
              <Input id="build-tag" placeholder="app:1.2" value={tag} onChange={(e) => setTag(e.target.value)} />
            </div>
            {archive && (
              <div className="grid gap-2">
                <Label htmlFor="build-path">Dockerfile path in the context</Label>
                <Input id="build-path" placeholder="Dockerfile" value={path} onChange={(e) => setPath(e.target.value)} />
              </div>
            )}
            <FormError message={invalid ?? (m.error ? errorMessage(m.error) : null)} />
            <DialogFooter>
              <Button type="submit" disabled={m.isPending}>
                Build
              </Button>
            </DialogFooter>
          </form>
        )}
      </DialogContent>
    </Dialog>
  );
}

function TaskView({ task, onClose, children }: { task: TrackedTask; onClose: () => void; children: ReactNode }) {
  return (
    <div className="grid gap-4">
      {children}
      <p role="status" className="text-sm">
        {task.status === "running" ? (
          <span className="text-muted-foreground">Running… You can close this; you will be told when it finishes.</span>
        ) : task.status === "succeeded" ? (
          "Done."
        ) : (
          <span className="text-destructive">Failed: {task.error}</span>
        )}
      </p>
      <DialogFooter>
        <Button variant="outline" onClick={onClose}>
          Close
        </Button>
      </DialogFooter>
    </div>
  );
}

function PullView({ messages }: { messages: unknown[] }) {
  const { layers, lines } = pullProgress(messages);
  return (
    <div className="grid gap-3">
      {layers.length > 0 && (
        <ul aria-label="Layers" className="grid gap-1.5 text-xs">
          {layers.map((l) => (
            <li key={l.id} className="grid grid-cols-[7rem_1fr_9rem] items-center gap-2">
              <code>{l.id}</code>
              <Bar
                label={`Layer ${l.id}`}
                percent={
                  l.total
                    ? Math.round(((l.current ?? 0) / l.total) * 100)
                    : l.status.endsWith("complete") || l.status === "Already exists"
                      ? 100
                      : 0
                }
              />
              <span className="truncate text-muted-foreground">
                {l.status}
                {l.total && l.status === "Downloading" ? ` ${formatBytes(l.current)} / ${formatBytes(l.total)}` : ""}
              </span>
            </li>
          ))}
        </ul>
      )}
      <LogView lines={lines} />
    </div>
  );
}

function Bar({ label, percent }: { label: string; percent: number }) {
  return (
    <div
      role="progressbar"
      aria-label={label}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={percent}
      className="h-1.5 overflow-hidden rounded-full bg-muted"
    >
      <div className="h-full bg-primary transition-[width]" style={{ width: `${percent}%` }} />
    </div>
  );
}

/** Output lines, kept scrolled to the end as they arrive. */
function LogView({ lines }: { lines: string[] }) {
  const ref = useRef<HTMLPreElement>(null);
  useEffect(() => {
    if (ref.current) ref.current.scrollTop = ref.current.scrollHeight;
  }, [lines.length]);
  if (lines.length === 0) return null;
  return (
    <pre
      ref={ref}
      aria-label="Output"
      className="max-h-80 overflow-auto rounded-md border bg-muted/40 p-3 font-mono text-xs whitespace-pre-wrap"
    >
      {lines.join("\n")}
    </pre>
  );
}
```

`web/src/pages/images/ImagesPage.tsx`:

```diff
--- a/web/src/pages/images/ImagesPage.tsx
+++ b/web/src/pages/images/ImagesPage.tsx
@@ -9,6 +9,7 @@ import { Badge } from "@/components/ui/badge";
 import { Button } from "@/components/ui/button";
 import { formatAgo, formatBytes, shortId } from "@/lib/format";
 import { imageName, PruneImagesDialog, RemoveImagesDialog } from "./dialogs";
+import { BuildDialog, PullDialog } from "./TaskDialogs";
 
 const col = columns<ImageSummary>();
 
@@ -45,13 +46,21 @@ export function ImagesPage() {
   const [cols] = useState(() => imageColumns(env));
   const [removing, setRemoving] = useState<ImageSummary[] | null>(null);
   const [pruning, setPruning] = useState(false);
+  const [pulling, setPulling] = useState(false);
+  const [building, setBuilding] = useState(false);
   return (
     <Page
       title="Images"
       actions={
-        <Button variant="outline" onClick={() => setPruning(true)}>
-          Prune
-        </Button>
+        <>
+          <Button variant="outline" onClick={() => setPruning(true)}>
+            Prune
+          </Button>
+          <Button variant="outline" onClick={() => setBuilding(true)}>
+            Build
+          </Button>
+          <Button onClick={() => setPulling(true)}>Pull</Button>
+        </>
       }
     >
       <DataTable
@@ -84,6 +93,8 @@ export function ImagesPage() {
         />
       )}
       {pruning && <PruneImagesDialog env={env} onDone={() => setPruning(false)} />}
+      {pulling && <PullDialog env={env} onClose={() => setPulling(false)} />}
+      {building && <BuildDialog env={env} onClose={() => setBuilding(false)} />}
     </Page>
   );
 }
```

- [ ] **Step 5: Run all checks**

Run: `cd web && pnpm test && pnpm typecheck && pnpm lint`
Expected: 146 passed (28 files); typecheck and lint are clean.

- [ ] **Step 6: Check it against the real server**

With `make web && make dev-server` running:
- **Pull:** Images → Pull `busybox:1.37`.
  - Layer rows appear with themed bars, and the lines end with `Status: Downloaded newer image…`.
  - The dialog says `Done.`, a `Pull busybox:1.37: done` toast appears, and the image is listed.
- **Build:** Images → Build.
  - Upload a Dockerfile containing `FROM busybox:1.37` / `RUN echo hello > /hello`, and tag it `p3b-hello:1`.
  - The log shows the `Step n/3` lines with no blank lines, then `Successfully tagged p3b-hello:1`.
- **Closed dialog:** close a pull while it is running. The toast still arrives when it finishes.
- **Audit:** the audit log (Task 12) has `image.pull` and `image.build`.
- Afterwards, run `docker rmi p3b-hello:1 busybox:1.37`, if busybox was not there before.

- [ ] **Step 7: Commit**

```bash
git add web/src
git commit -m "feat(web): image pull and build with live progress and result toasts"
```

---

### Task 8: Volumes

**Files:**
- Create:
  - `web/src/lib/usage.ts`;
  - `web/src/pages/volumes/VolumesPage.tsx`, `web/src/pages/volumes/VolumePage.tsx` and `web/src/pages/volumes/dialogs.tsx`.
- Modify: `web/src/api/docker.ts`, `web/src/api/queries.ts`, `web/src/router.tsx` and `web/src/components/Shell.tsx`.
- Test: `web/src/pages/volumes/volumes.test.tsx`.

**Interfaces:**
- Consumes: `volumesQuery` and `containersQuery` (P3a); `parseKeyValues` and `formatKeyValues` (Task 5); and the SDK's `inspectVolume`, `createVolume`, `deleteVolume` and `pruneVolumes`.
- Produces `@/api/docker`: `VolumeInspect`.
- Produces `@/api/queries`: `volumeQuery(env, name)`.
- Produces `@/lib/usage`: `volumeUsers(containers)`, a `Map` from volume name to the containers that mount it.
- Produces `pages/volumes/dialogs.tsx`: `RemoveVolumesDialog`, `PruneVolumesDialog` and `CreateVolumeDialog`.
- Produces: the routes `/envs/$env/volumes` and `/envs/$env/volumes/$name`, and a `Volumes` nav item.

- [ ] **Step 1: Write the failing test**

`web/src/pages/volumes/volumes.test.tsx`:

```tsx
import { waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { VolumeSummary } from "@/api/gen/types.gen";
import { json, mockFetch } from "@/test/fetch";
import { loggedIn, renderApp } from "@/test/app";
import { container } from "@/test/docker";

const vol = (name: string, over: Partial<VolumeSummary> = {}): VolumeSummary => ({
  name,
  driver: "local",
  mountpoint: `/var/lib/docker/volumes/${name}/_data`,
  labels: {},
  in_use: false,
  created_at: "2026-09-01T00:00:00Z",
  ...over,
});

const routes = {
  ...loggedIn,
  "GET /api/v1/envs/local/volumes": { json: [vol("pgdata", { in_use: true }), vol("scratch")] },
  "GET /api/v1/envs/local/containers": { json: [container({ id: "c1", name: "db", volumes: ["pgdata"] })] },
};
const rows = (screen: typeof import("@testing-library/react").screen) =>
  within(screen.getByRole("table", { name: "Volumes" })).getAllByRole("row").slice(1);

describe("volumes", () => {
  it("lists volumes with the containers using them", async () => {
    mockFetch(routes);
    const { screen } = renderApp("/envs/local/volumes");
    await waitFor(() => expect(rows(screen)).toHaveLength(2));
    const [pg, scratch] = rows(screen);
    expect(within(pg).getByRole("link", { name: "db" })).toHaveAttribute("href", "/envs/local/containers/c1");
    expect(within(scratch).getByText("unused")).toBeInTheDocument();
  });

  it("creates a volume with driver options", async () => {
    const calls = mockFetch({
      ...routes,
      "POST /api/v1/envs/local/volumes": { status: 201, json: vol("nfs1") },
      "GET /api/v1/envs/local/volumes/nfs1": { json: { Name: "nfs1", Driver: "local", Mountpoint: "/x" } },
    });
    const { screen, history } = renderApp("/envs/local/volumes");
    await userEvent.click(await screen.findByRole("button", { name: "Create volume" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText("Name"), "nfs1");
    await userEvent.type(within(dialog).getByLabelText("Driver options"), "type=nfs{enter}o=addr=10.0.0.5");
    await userEvent.click(within(dialog).getByRole("button", { name: "Create" }));
    await waitFor(() => expect(history.location.pathname).toBe("/envs/local/volumes/nfs1"));
    expect(calls.find((c) => c.method === "POST")?.body).toEqual({
      name: "nfs1",
      driver: "local",
      driver_opts: { type: "nfs", o: "addr=10.0.0.5" },
      labels: {},
    });
  });

  it("rejects a malformed option line before sending", async () => {
    const calls = mockFetch(routes);
    const { screen } = renderApp("/envs/local/volumes");
    await userEvent.click(await screen.findByRole("button", { name: "Create volume" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText("Driver options"), "nonsense");
    await userEvent.click(within(dialog).getByRole("button", { name: "Create" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent('Option "nonsense" must look like key=value');
    expect(calls.some((c) => c.method === "POST")).toBe(false);
  });

  it("removes a volume, showing Docker's refusal for one in use", async () => {
    mockFetch({
      ...routes,
      "DELETE /api/v1/envs/local/volumes/pgdata": () =>
        json(409, { code: "conflict", message: "volume is in use - [c1]" }),
      "DELETE /api/v1/envs/local/volumes/scratch": { status: 204 },
    });
    const { screen } = renderApp("/envs/local/volumes");
    await waitFor(() => expect(rows(screen)).toHaveLength(2));
    await userEvent.click(screen.getByRole("checkbox", { name: "Select all" }));
    await userEvent.click(within(screen.getByRole("group", { name: "Selected" })).getByRole("button", { name: "Remove" }));
    await userEvent.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Remove" }));
    expect(await screen.findByText("Removed 1 of 2 volumes. pgdata: volume is in use - [c1]")).toBeInTheDocument();
  });

  it("prunes unused volumes", async () => {
    mockFetch({ ...routes, "POST /api/v1/envs/local/volumes/prune": { json: { deleted: ["a"], space_reclaimed: 1000 } } });
    const { screen } = renderApp("/envs/local/volumes");
    await userEvent.click(await screen.findByRole("button", { name: "Prune" }));
    await userEvent.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Prune" }));
    expect(await screen.findByText("Removed 1 volumes, reclaimed 1.0 KB")).toBeInTheDocument();
  });

  it("shows a volume's details and users", async () => {
    mockFetch({
      ...routes,
      "GET /api/v1/envs/local/volumes/pgdata": {
        json: { Name: "pgdata", Driver: "local", Mountpoint: "/var/lib/docker/volumes/pgdata/_data", Scope: "local", Labels: { app: "shop" } },
      },
    });
    const { screen } = renderApp("/envs/local/volumes/pgdata");
    const d = await screen.findByLabelText("Volume details");
    expect(within(d).getByText("/var/lib/docker/volumes/pgdata/_data")).toBeInTheDocument();
    expect(within(d).getByText("app=shop")).toBeInTheDocument();
    expect(await within(d).findByRole("link", { name: "db" })).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cd web && pnpm test src/pages/volumes`
Expected: FAIL: all 6 tests, with `Unable to find role="table" and name "Volumes"` and `Unable to find role="button" and name "Create volume"`.

- [ ] **Step 3: Implement**

`web/src/api/docker.ts`:

```diff
--- a/web/src/api/docker.ts
+++ b/web/src/api/docker.ts
@@ -79,3 +79,13 @@ export interface ImageLayer {
   Tags?: string[] | null;
   Comment?: string;
 }
+
+export interface VolumeInspect {
+  Name: string;
+  Driver: string;
+  Mountpoint: string;
+  CreatedAt?: string;
+  Scope?: string;
+  Labels?: Record<string, string> | null;
+  Options?: Record<string, string> | null;
+}
```

`web/src/api/queries.ts`:

```diff
--- a/web/src/api/queries.ts
+++ b/web/src/api/queries.ts
@@ -1,6 +1,6 @@
 import { queryOptions } from "@tanstack/react-query";
 import { ApiError, setCsrfToken } from "./client";
-import type { ContainerInspect, ImageInspect, ImageLayer } from "./docker";
+import type { ContainerInspect, ImageInspect, ImageLayer, VolumeInspect } from "./docker";
 import {
   getAuthStatus,
   getDiskUsage,
@@ -11,6 +11,7 @@ import {
   getMe,
   inspectContainer,
   inspectImage,
+  inspectVolume,
   listContainerFiles,
   listContainers,
   listEnvs,
@@ -147,6 +148,13 @@ export const imageHistoryQuery = (env: string, id: string) =>
 export const volumesQuery = (env: string) =>
   queryOptions({ queryKey: keys.volumes(env), queryFn: async () => (await listVolumes(path(env))).data });
 
+export const volumeQuery = (env: string, name: string) =>
+  queryOptions({
+    queryKey: keys.volume(env, name),
+    queryFn: async () => (await inspectVolume({ path: { env, name } })).data as unknown as VolumeInspect,
+    retry: retryUnlessGone,
+  });
+
 export const networksQuery = (env: string) =>
   queryOptions({ queryKey: keys.networks(env), queryFn: async () => (await listNetworks(path(env))).data });
 
```

`web/src/lib/usage.ts`:

```ts
import type { ContainerSummary } from "@/api/gen/types.gen";

/** The containers mounting each named volume, from the container list. */
export function volumeUsers(containers: ContainerSummary[] | undefined): Map<string, ContainerSummary[]> {
  const out = new Map<string, ContainerSummary[]>();
  for (const c of containers ?? []) {
    for (const v of c.volumes) out.set(v, [...(out.get(v) ?? []), c]);
  }
  return out;
}
```

`web/src/pages/volumes/dialogs.tsx`:

```tsx
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useState, type FormEvent } from "react";
import { toast } from "sonner";
import { keys } from "@/api/queries";
import { createVolume, deleteVolume, pruneVolumes } from "@/api/gen/sdk.gen";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { bulkMessage, runBulk } from "@/lib/bulk";
import { errorMessage } from "@/lib/errors";
import { formatBytes } from "@/lib/format";
import { parseKeyValues } from "@/lib/kv";
import { FormError } from "@/pages/AuthLayout";

export function RemoveVolumesDialog({
  env,
  names,
  onDone,
}: {
  env: string;
  names: string[];
  onDone: (removed: boolean) => void;
}) {
  const qc = useQueryClient();
  const [force, setForce] = useState(false);
  const one = names.length === 1;
  return (
    <ConfirmDialog
      open
      onOpenChange={(open) => !open && onDone(false)}
      title={one ? `Remove volume ${names[0]}?` : `Remove ${names.length} volumes?`}
      description={
        <>
          The data in {one ? "it" : "them"} is deleted for good.{one ? "" : ` ${names.join(", ")}`}
        </>
      }
      confirmLabel="Remove"
      onConfirm={async () => {
        const r = await runBulk(names, (n) => n, (name) => deleteVolume({ path: { env, name }, query: { force } }));
        await qc.invalidateQueries({ queryKey: keys.volumes(env) });
        if (r.ok === 0) throw one ? r.failed[0].error : new Error(bulkMessage("Removed", "volume", r));
        const msg = bulkMessage("Removed", "volume", r);
        if (r.failed.length) toast.error(msg);
        else toast.success(msg);
        onDone(true);
      }}
    >
      <div className="flex items-center gap-2">
        <Checkbox id="vol-force" checked={force} onCheckedChange={(v) => setForce(!!v)} />
        <Label htmlFor="vol-force">Force (remove even if the driver reports it in use)</Label>
      </div>
    </ConfirmDialog>
  );
}

export function PruneVolumesDialog({ env, onDone }: { env: string; onDone: () => void }) {
  const qc = useQueryClient();
  return (
    <ConfirmDialog
      open
      onOpenChange={(open) => !open && onDone()}
      title="Prune volumes?"
      description="Removes anonymous volumes no container uses, and their data. Named volumes are kept."
      confirmLabel="Prune"
      onConfirm={async () => {
        const { data } = await pruneVolumes({ path: { env } });
        await qc.invalidateQueries({ queryKey: keys.volumes(env) });
        toast.success(`Removed ${data.deleted.length} volumes, reclaimed ${formatBytes(data.space_reclaimed)}`);
        onDone();
      }}
    />
  );
}

export function CreateVolumeDialog({ env, onClose }: { env: string; onClose: () => void }) {
  const qc = useQueryClient();
  const navigate = useNavigate();
  const [name, setName] = useState("");
  const [driver, setDriver] = useState("local");
  const [opts, setOpts] = useState("");
  const [labels, setLabels] = useState("");
  const [invalid, setInvalid] = useState<string | null>(null);
  const m = useMutation({
    meta: { inline: true },
    mutationFn: (body: Parameters<typeof createVolume>[0]["body"]) => createVolume({ path: { env }, body }),
    onSuccess: async ({ data }) => {
      await qc.invalidateQueries({ queryKey: keys.volumes(env) });
      toast.success(`Created volume ${data.name}`);
      onClose();
      void navigate({ to: "/envs/$env/volumes/$name", params: { env, name: data.name } });
    },
  });
  const submit = (e: FormEvent) => {
    e.preventDefault();
    try {
      const body = {
        name: name.trim() || undefined,
        driver: driver.trim() || null,
        driver_opts: parseKeyValues(opts, "Option"),
        labels: parseKeyValues(labels, "Label"),
      };
      setInvalid(null);
      m.mutate(body);
    } catch (err) {
      setInvalid((err as Error).message);
    }
  };
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Create volume</DialogTitle>
        </DialogHeader>
        <form onSubmit={submit} className="grid gap-4">
          <div className="grid gap-2">
            <Label htmlFor="vol-name">Name</Label>
            <Input id="vol-name" placeholder="Leave empty for a generated name" value={name} onChange={(e) => setName(e.target.value)} />
          </div>
          <div className="grid gap-2">
            <Label htmlFor="vol-driver">Driver</Label>
            <Input id="vol-driver" value={driver} onChange={(e) => setDriver(e.target.value)} />
          </div>
          <div className="grid gap-2">
            <Label htmlFor="vol-opts">Driver options</Label>
            <Textarea
              id="vol-opts"
              className="font-mono"
              placeholder={"type=nfs\no=addr=10.0.0.5,rw"}
              value={opts}
              onChange={(e) => setOpts(e.target.value)}
            />
            <p className="text-xs text-muted-foreground">One key=value per line.</p>
          </div>
          <div className="grid gap-2">
            <Label htmlFor="vol-labels">Labels</Label>
            <Textarea id="vol-labels" className="font-mono" value={labels} onChange={(e) => setLabels(e.target.value)} />
          </div>
          <FormError message={invalid ?? (m.error ? errorMessage(m.error) : null)} />
          <DialogFooter>
            <Button type="submit" disabled={m.isPending}>
              Create
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
```

`web/src/pages/volumes/VolumesPage.tsx`:

```tsx
import { useQuery } from "@tanstack/react-query";
import { Link, useParams } from "@tanstack/react-router";
import { useMemo, useState } from "react";
import { containersQuery, volumesQuery } from "@/api/queries";
import type { ContainerSummary, VolumeSummary } from "@/api/gen/types.gen";
import { columns, DataTable, type Column } from "@/components/DataTable";
import { Page } from "@/components/Page";
import { Button } from "@/components/ui/button";
import { volumeUsers } from "@/lib/usage";
import { CreateVolumeDialog, PruneVolumesDialog, RemoveVolumesDialog } from "./dialogs";

type Row = VolumeSummary & { users: ContainerSummary[] };
const col = columns<Row>();

function volumeColumns(env: string): Column<Row>[] {
  return [
    col.accessor("name", {
      header: "Name",
      enableHiding: false,
      cell: (c) => (
        <Link
          to="/envs/$env/volumes/$name"
          params={{ env, name: c.getValue() }}
          className="block max-w-80 truncate font-medium hover:underline"
        >
          {c.getValue()}
        </Link>
      ),
    }),
    col.accessor("driver", { header: "Driver" }),
    col.accessor((v) => v.users.map((u) => u.name).join(", "), {
      id: "users",
      header: "Used by",
      cell: (c) =>
        c.row.original.users.length ? (
          <span className="flex flex-wrap gap-x-2">
            {c.row.original.users.map((u) => (
              <Link key={u.id} to="/envs/$env/containers/$id" params={{ env, id: u.id }} className="hover:underline">
                {u.name}
              </Link>
            ))}
          </span>
        ) : (
          <span className="text-muted-foreground">{c.row.original.in_use ? "in use" : "unused"}</span>
        ),
    }),
    col.accessor("mountpoint", {
      header: "Mount point",
      cell: (c) => <span className="block max-w-80 truncate font-mono text-xs">{c.getValue()}</span>,
    }),
    col.accessor((v) => v.created_at ?? "", {
      id: "created",
      header: "Created",
      cell: (c) => (c.getValue() ? new Date(c.getValue()).toLocaleString() : "—"),
    }),
  ];
}

export function VolumesPage() {
  const { env } = useParams({ from: "/authed/shell/envs/$env/volumes" });
  const list = useQuery(volumesQuery(env));
  const containers = useQuery(containersQuery(env));
  const [cols] = useState(() => volumeColumns(env));
  const rows = useMemo(() => {
    if (!list.data) return undefined;
    const users = volumeUsers(containers.data);
    return list.data.map((v) => ({ ...v, users: users.get(v.name) ?? [] }));
  }, [list.data, containers.data]);
  const [removing, setRemoving] = useState<string[] | null>(null);
  const [pruning, setPruning] = useState(false);
  const [creating, setCreating] = useState(false);
  return (
    <Page
      title="Volumes"
      actions={
        <>
          <Button variant="outline" onClick={() => setPruning(true)}>
            Prune
          </Button>
          <Button onClick={() => setCreating(true)}>Create volume</Button>
        </>
      }
    >
      <DataTable
        label="Volumes"
        data={rows}
        error={list.error}
        columns={cols}
        getRowId={(v) => v.name}
        empty="No volumes in this environment."
        initialSorting={[{ id: "name", desc: false }]}
        initialHidden={["mountpoint"]}
        selectable
        actions={(selected, clear) => (
          <Button
            variant="destructive"
            size="sm"
            onClick={() => {
              setRemoving(selected.map((v) => v.name));
              clear();
            }}
          >
            Remove
          </Button>
        )}
      />
      {removing && <RemoveVolumesDialog env={env} names={removing} onDone={() => setRemoving(null)} />}
      {pruning && <PruneVolumesDialog env={env} onDone={() => setPruning(false)} />}
      {creating && <CreateVolumeDialog env={env} onClose={() => setCreating(false)} />}
    </Page>
  );
}
```

`web/src/pages/volumes/VolumePage.tsx`:

```tsx
import { useQuery } from "@tanstack/react-query";
import { Link, useNavigate, useParams } from "@tanstack/react-router";
import { useState } from "react";
import { containersQuery, volumeQuery } from "@/api/queries";
import { Details, DetailError } from "@/components/Details";
import { JsonView } from "@/components/JsonView";
import { Page } from "@/components/Page";
import { Button } from "@/components/ui/button";
import { formatKeyValues } from "@/lib/kv";
import { volumeUsers } from "@/lib/usage";
import { RemoveVolumesDialog } from "./dialogs";

export function VolumePage() {
  const { env, name } = useParams({ from: "/authed/shell/envs/$env/volumes/$name" });
  const navigate = useNavigate();
  const q = useQuery(volumeQuery(env, name));
  const containers = useQuery(containersQuery(env));
  const [removing, setRemoving] = useState(false);
  const back = (
    <Link to="/envs/$env/volumes" params={{ env }} className="underline">
      Back to volumes
    </Link>
  );
  if (q.error) return <DetailError error={q.error} what="volume" back={back} />;
  const v = q.data;
  if (!v) return <p className="p-6 text-sm text-muted-foreground">Loading…</p>;
  const users = volumeUsers(containers.data).get(v.Name) ?? [];
  return (
    <Page
      title={v.Name}
      actions={
        <Button variant="destructive" onClick={() => setRemoving(true)}>
          Remove
        </Button>
      }
    >
      <Details
        label="Volume details"
        items={[
          ["Driver", v.Driver],
          ["Scope", v.Scope],
          ["Mount point", <code key="mp" className="text-xs">{v.Mountpoint}</code>],
          ["Created", v.CreatedAt ? new Date(v.CreatedAt).toLocaleString() : undefined],
          [
            "Used by",
            users.length ? (
              <span key="users" className="flex flex-wrap gap-x-2">
                {users.map((u) => (
                  <Link key={u.id} to="/envs/$env/containers/$id" params={{ env, id: u.id }} className="underline">
                    {u.name}
                  </Link>
                ))}
              </span>
            ) : (
              "no container"
            ),
          ],
          ["Options", formatKeyValues(v.Options) || undefined],
          ["Labels", formatKeyValues(v.Labels) || undefined],
        ]}
      />
      <section className="grid gap-2">
        <h2 className="font-medium">Inspect</h2>
        <JsonView value={v} label="Volume JSON" />
      </section>
      {removing && (
        <RemoveVolumesDialog
          env={env}
          names={[v.Name]}
          onDone={(removed) => {
            setRemoving(false);
            if (removed) void navigate({ to: "/envs/$env/volumes", params: { env } });
          }}
        />
      )}
    </Page>
  );
}
```

`web/src/router.tsx`:

```diff
--- a/web/src/router.tsx
+++ b/web/src/router.tsx
@@ -159,6 +159,18 @@ const imageRoute = createRoute({
   component: lazyRouteComponent(() => import("@/pages/images/ImagePage"), "ImagePage"),
 });
 
+const volumesRoute = createRoute({
+  getParentRoute: () => envRoute,
+  path: "volumes",
+  component: lazyRouteComponent(() => import("@/pages/volumes/VolumesPage"), "VolumesPage"),
+});
+
+const volumeRoute = createRoute({
+  getParentRoute: () => envRoute,
+  path: "volumes/$name",
+  component: lazyRouteComponent(() => import("@/pages/volumes/VolumePage"), "VolumePage"),
+});
+
 const accountRoute = createRoute({
   getParentRoute: () => shellRoute,
   path: "/account",
@@ -176,6 +188,8 @@ const routeTree = rootRoute.addChildren([
         editContainerRoute,
         imagesRoute,
         imageRoute,
+        volumesRoute,
+        volumeRoute,
       ]), accountRoute])]),
 ]);
 
```

`web/src/components/Shell.tsx`:

```diff
--- a/web/src/components/Shell.tsx
+++ b/web/src/components/Shell.tsx
@@ -1,6 +1,6 @@
 import { useQuery } from "@tanstack/react-query";
 import { Link, Outlet, useNavigate, useParams } from "@tanstack/react-router";
-import { Boxes, ChevronsUpDown, Layers, LayoutDashboard, LogOut, Monitor, Moon, Sun, User } from "lucide-react";
+import { Boxes, ChevronsUpDown, Database, Layers, LayoutDashboard, LogOut, Monitor, Moon, Sun, User } from "lucide-react";
 import { envsQuery, envStatusQuery } from "@/api/queries";
 import { useLogout, useSession } from "@/auth/session";
 import { Badge } from "@/components/ui/badge";
@@ -23,6 +23,7 @@ const NAV = [
   { to: "/envs/$env", label: "Dashboard", icon: LayoutDashboard },
   { to: "/envs/$env/containers", label: "Containers", icon: Boxes },
   { to: "/envs/$env/images", label: "Images", icon: Layers },
+  { to: "/envs/$env/volumes", label: "Volumes", icon: Database },
 ] as const;
 
 export function Shell() {
```

- [ ] **Step 4: Run all checks**

Run: `cd web && pnpm test && pnpm typecheck && pnpm lint`
Expected: 152 passed (29 files); typecheck and lint are clean.

- [ ] **Step 5: Commit**

```bash
git add web/src
git commit -m "feat(web): volumes list and page with create, remove and prune"
```

---

### Task 9: Networks

**Files:**
- Create: `web/src/pages/networks/NetworksPage.tsx`, `web/src/pages/networks/NetworkPage.tsx` and `web/src/pages/networks/dialogs.tsx`.
- Modify: `web/src/api/docker.ts`, `web/src/api/queries.ts`, `web/src/router.tsx` and `web/src/components/Shell.tsx`.
- Test: `web/src/pages/networks/networks.test.tsx`.

**Interfaces:**
- Consumes: `networksQuery` and `containersQuery`, and the SDK's `inspectNetwork`, `createNetwork`, `deleteNetwork`, `pruneNetworks`, `connectNetwork` and `disconnectNetwork`.
- Produces `@/api/docker`: `NetworkInspect`.
- Produces `@/api/queries`: `networkQuery(env, id)`.
- Produces `pages/networks/dialogs.tsx`:
  - `BUILTIN_NETWORKS`: `bridge`, `host` and `none`, whose pages offer no Remove;
  - `RemoveNetworksDialog`, `PruneNetworksDialog` and `CreateNetworkDialog`.
- Produces:
  - the routes `/envs/$env/networks` and `/envs/$env/networks/$id`, and a `Networks` nav item;
  - on the network page, the `Attached containers` table, and the `Container to connect` select.

- [ ] **Step 1: Write the failing test**

`web/src/pages/networks/networks.test.tsx`:

```tsx
import { waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { NetworkInspect } from "@/api/docker";
import type { NetworkSummary } from "@/api/gen/types.gen";
import { json, mockFetch } from "@/test/fetch";
import { loggedIn, renderApp } from "@/test/app";
import { container } from "@/test/docker";

const net = (id: string, name: string, over: Partial<NetworkSummary> = {}): NetworkSummary => ({
  id,
  name,
  driver: "bridge",
  scope: "local",
  internal: false,
  attachable: false,
  ...over,
});

const SHOP: NetworkInspect = {
  Id: "n1",
  Name: "shop",
  Driver: "bridge",
  Scope: "local",
  IPAM: { Config: [{ Subnet: "172.30.0.0/16", Gateway: "172.30.0.1" }] },
  Containers: { c1: { Name: "web", IPv4Address: "172.30.0.2/16", MacAddress: "02:42:ac:1e:00:02" } },
};

const routes = {
  ...loggedIn,
  "GET /api/v1/envs/local/networks": {
    json: [net("n0", "bridge", { subnet: "172.17.0.0/16" }), net("n1", "shop", { internal: true })],
  },
  "GET /api/v1/envs/local/networks/n1": { json: SHOP },
  "GET /api/v1/envs/local/containers": {
    json: [container({ id: "c1", name: "web" }), container({ id: "c2", name: "worker" })],
  },
};

describe("networks", () => {
  it("lists networks with subnet and flags", async () => {
    mockFetch(routes);
    const { screen } = renderApp("/envs/local/networks");
    const table = await screen.findByRole("table", { name: "Networks" });
    await waitFor(() => expect(within(table).getAllByRole("row")).toHaveLength(3));
    expect(within(table).getByText("172.17.0.0/16")).toBeInTheDocument();
    expect(within(table).getByText("internal")).toBeInTheDocument();
  });

  it("creates a network with a subnet", async () => {
    const calls = mockFetch({ ...routes, "POST /api/v1/envs/local/networks": { status: 201, json: { id: "n1" } } });
    const { screen, history } = renderApp("/envs/local/networks");
    await userEvent.click(await screen.findByRole("button", { name: "Create network" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText("Name"), "shop");
    await userEvent.type(within(dialog).getByLabelText("Subnet"), "172.30.0.0/16");
    await userEvent.type(within(dialog).getByLabelText("Gateway"), "172.30.0.1");
    await userEvent.click(within(dialog).getByLabelText(/Internal/));
    await userEvent.click(within(dialog).getByRole("button", { name: "Create" }));
    await waitFor(() => expect(history.location.pathname).toBe("/envs/local/networks/n1"));
    expect(calls.find((c) => c.method === "POST")?.body).toEqual({
      name: "shop",
      driver: "bridge",
      subnet: "172.30.0.0/16",
      gateway: "172.30.0.1",
      internal: true,
      attachable: false,
      labels: {},
    });
  });

  it("asks for a subnet when a gateway is given", async () => {
    mockFetch(routes);
    const { screen } = renderApp("/envs/local/networks");
    await userEvent.click(await screen.findByRole("button", { name: "Create network" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText("Name"), "x");
    await userEvent.type(within(dialog).getByLabelText("Gateway"), "10.0.0.1");
    await userEvent.click(within(dialog).getByRole("button", { name: "Create" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("A gateway needs a subnet");
  });

  it("shows attached containers and disconnects one", async () => {
    const calls = mockFetch({ ...routes, "POST /api/v1/envs/local/networks/n1/disconnect": { status: 204 } });
    const { screen } = renderApp("/envs/local/networks/n1");
    const table = await screen.findByRole("table", { name: "Attached containers" });
    expect(within(table).getByText("172.30.0.2/16")).toBeInTheDocument();
    expect(within(screen.getByLabelText("Network details")).getByText("172.30.0.0/16")).toBeInTheDocument();
    await userEvent.click(within(table).getByRole("button", { name: "Disconnect" }));
    await waitFor(() => expect(calls.find((c) => c.path.endsWith("/disconnect"))?.body).toEqual({ container: "c1" }));
  });

  it("connects a container that is not attached yet", async () => {
    const calls = mockFetch({ ...routes, "POST /api/v1/envs/local/networks/n1/connect": { status: 204 } });
    const { screen } = renderApp("/envs/local/networks/n1");
    const select = await screen.findByLabelText("Container to connect");
    await waitFor(() => expect(within(select).getAllByRole("option")).toHaveLength(2));
    expect(within(select).queryByRole("option", { name: "web" })).not.toBeInTheDocument();
    await userEvent.selectOptions(select, "worker");
    await userEvent.click(screen.getByRole("button", { name: "Connect" }));
    await waitFor(() => expect(calls.find((c) => c.path.endsWith("/connect"))?.body).toEqual({ container: "c2" }));
  });

  it("shows Docker's error when connecting fails", async () => {
    mockFetch({
      ...routes,
      "POST /api/v1/envs/local/networks/n1/connect": () =>
        json(409, { code: "conflict", message: "container sharing network namespace with another container" }),
    });
    const { screen } = renderApp("/envs/local/networks/n1");
    const select = await screen.findByLabelText("Container to connect");
    await waitFor(() => expect(within(select).getAllByRole("option")).toHaveLength(2));
    await userEvent.selectOptions(select, "worker");
    await userEvent.click(screen.getByRole("button", { name: "Connect" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("sharing network namespace");
  });

  it("offers no remove for Docker's own networks", async () => {
    mockFetch({ ...routes, "GET /api/v1/envs/local/networks/n0": { json: { ...SHOP, Id: "n0", Name: "bridge" } } });
    const { screen } = renderApp("/envs/local/networks/n0");
    expect(await screen.findByRole("heading", { name: "bridge" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Remove" })).not.toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cd web && pnpm test src/pages/networks`
Expected: FAIL: all 7 tests, with `Unable to find role="table" and name "Networks"`, `role="button" and name "Create network"` and `a label with the text of: Container to connect`.

- [ ] **Step 3: Implement**

`web/src/api/docker.ts`:

```diff
--- a/web/src/api/docker.ts
+++ b/web/src/api/docker.ts
@@ -89,3 +89,18 @@ export interface VolumeInspect {
   Labels?: Record<string, string> | null;
   Options?: Record<string, string> | null;
 }
+
+export interface NetworkInspect {
+  Id: string;
+  Name: string;
+  Driver: string;
+  Scope?: string;
+  Internal?: boolean;
+  Attachable?: boolean;
+  Created?: string;
+  IPAM?: { Config?: { Subnet?: string; Gateway?: string }[] | null } | null;
+  /** Attached containers by id. */
+  Containers?: Record<string, { Name: string; IPv4Address?: string; IPv6Address?: string; MacAddress?: string }> | null;
+  Labels?: Record<string, string> | null;
+  Options?: Record<string, string> | null;
+}
```

`web/src/api/queries.ts`:

```diff
--- a/web/src/api/queries.ts
+++ b/web/src/api/queries.ts
@@ -1,6 +1,6 @@
 import { queryOptions } from "@tanstack/react-query";
 import { ApiError, setCsrfToken } from "./client";
-import type { ContainerInspect, ImageInspect, ImageLayer, VolumeInspect } from "./docker";
+import type { ContainerInspect, ImageInspect, ImageLayer, NetworkInspect, VolumeInspect } from "./docker";
 import {
   getAuthStatus,
   getDiskUsage,
@@ -11,6 +11,7 @@ import {
   getMe,
   inspectContainer,
   inspectImage,
+  inspectNetwork,
   inspectVolume,
   listContainerFiles,
   listContainers,
@@ -158,6 +159,13 @@ export const volumeQuery = (env: string, name: string) =>
 export const networksQuery = (env: string) =>
   queryOptions({ queryKey: keys.networks(env), queryFn: async () => (await listNetworks(path(env))).data });
 
+export const networkQuery = (env: string, id: string) =>
+  queryOptions({
+    queryKey: keys.network(env, id),
+    queryFn: async () => (await inspectNetwork({ path: { env, id } })).data as unknown as NetworkInspect,
+    retry: retryUnlessGone,
+  });
+
 export const recentEventsQuery = (env: string) =>
   queryOptions<DockerEvent[]>({
     queryKey: keys.recentEvents(env),
```

`web/src/pages/networks/dialogs.tsx`:

```tsx
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useState, type FormEvent } from "react";
import { toast } from "sonner";
import { keys } from "@/api/queries";
import { createNetwork, deleteNetwork, pruneNetworks } from "@/api/gen/sdk.gen";
import type { NetworkSpec } from "@/api/gen/types.gen";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Dialog, DialogContent, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { bulkMessage, runBulk } from "@/lib/bulk";
import { errorMessage } from "@/lib/errors";
import { parseKeyValues } from "@/lib/kv";
import { FormError } from "@/pages/AuthLayout";

/** Docker's own networks, which cannot be removed. */
export const BUILTIN_NETWORKS = ["bridge", "host", "none"];

export function RemoveNetworksDialog({
  env,
  networks,
  onDone,
}: {
  env: string;
  networks: { id: string; name: string }[];
  onDone: (removed: boolean) => void;
}) {
  const qc = useQueryClient();
  const one = networks.length === 1;
  return (
    <ConfirmDialog
      open
      onOpenChange={(open) => !open && onDone(false)}
      title={one ? `Remove network ${networks[0].name}?` : `Remove ${networks.length} networks?`}
      description={one ? undefined : networks.map((n) => n.name).join(", ")}
      confirmLabel="Remove"
      onConfirm={async () => {
        const r = await runBulk(networks, (n) => n.name, (n) => deleteNetwork({ path: { env, id: n.id } }));
        await qc.invalidateQueries({ queryKey: keys.networks(env) });
        if (r.ok === 0) throw one ? r.failed[0].error : new Error(bulkMessage("Removed", "network", r));
        const msg = bulkMessage("Removed", "network", r);
        if (r.failed.length) toast.error(msg);
        else toast.success(msg);
        onDone(true);
      }}
    />
  );
}

export function PruneNetworksDialog({ env, onDone }: { env: string; onDone: () => void }) {
  const qc = useQueryClient();
  return (
    <ConfirmDialog
      open
      onOpenChange={(open) => !open && onDone()}
      title="Prune networks?"
      description="Removes every network no container is attached to. Docker's own networks are kept."
      confirmLabel="Prune"
      onConfirm={async () => {
        const { data } = await pruneNetworks({ path: { env } });
        await qc.invalidateQueries({ queryKey: keys.networks(env) });
        toast.success(`Removed ${data.deleted.length} networks`);
        onDone();
      }}
    />
  );
}

export function CreateNetworkDialog({ env, onClose }: { env: string; onClose: () => void }) {
  const qc = useQueryClient();
  const navigate = useNavigate();
  const [f, setF] = useState({ name: "", driver: "bridge", subnet: "", gateway: "", labels: "" });
  const [internal, setInternal] = useState(false);
  const [attachable, setAttachable] = useState(false);
  const [invalid, setInvalid] = useState<string | null>(null);
  const set = (k: keyof typeof f) => (e: { target: { value: string } }) => setF({ ...f, [k]: e.target.value });
  const m = useMutation({
    meta: { inline: true },
    mutationFn: (body: NetworkSpec) => createNetwork({ path: { env }, body }),
    onSuccess: async ({ data }) => {
      await qc.invalidateQueries({ queryKey: keys.networks(env) });
      toast.success(`Created network ${f.name}`);
      onClose();
      void navigate({ to: "/envs/$env/networks/$id", params: { env, id: data.id } });
    },
  });
  const submit = (e: FormEvent) => {
    e.preventDefault();
    try {
      if (f.gateway.trim() && !f.subnet.trim()) throw new Error("A gateway needs a subnet");
      const body: NetworkSpec = {
        name: f.name.trim(),
        driver: f.driver.trim() || null,
        subnet: f.subnet.trim() || null,
        gateway: f.gateway.trim() || null,
        internal,
        attachable,
        labels: parseKeyValues(f.labels, "Label"),
      };
      setInvalid(null);
      m.mutate(body);
    } catch (err) {
      setInvalid((err as Error).message);
    }
  };
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Create network</DialogTitle>
        </DialogHeader>
        <form onSubmit={submit} className="grid gap-4">
          <div className="grid gap-2">
            <Label htmlFor="net-name">Name</Label>
            <Input id="net-name" required value={f.name} onChange={set("name")} />
          </div>
          <div className="grid gap-2">
            <Label htmlFor="net-driver">Driver</Label>
            <Input id="net-driver" value={f.driver} onChange={set("driver")} />
          </div>
          <div className="grid gap-4 sm:grid-cols-2">
            <div className="grid gap-2">
              <Label htmlFor="net-subnet">Subnet</Label>
              <Input id="net-subnet" placeholder="172.30.0.0/16" value={f.subnet} onChange={set("subnet")} />
            </div>
            <div className="grid gap-2">
              <Label htmlFor="net-gateway">Gateway</Label>
              <Input id="net-gateway" placeholder="172.30.0.1" value={f.gateway} onChange={set("gateway")} />
            </div>
          </div>
          <div className="grid gap-2">
            <Label htmlFor="net-labels">Labels</Label>
            <Textarea id="net-labels" className="font-mono" value={f.labels} onChange={set("labels")} />
          </div>
          <div className="flex items-center gap-2">
            <Checkbox id="net-internal" checked={internal} onCheckedChange={(v) => setInternal(!!v)} />
            <Label htmlFor="net-internal">Internal (no access to outside networks)</Label>
          </div>
          <div className="flex items-center gap-2">
            <Checkbox id="net-attachable" checked={attachable} onCheckedChange={(v) => setAttachable(!!v)} />
            <Label htmlFor="net-attachable">Attachable</Label>
          </div>
          <FormError message={invalid ?? (m.error ? errorMessage(m.error) : null)} />
          <DialogFooter>
            <Button type="submit" disabled={m.isPending}>
              Create
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
```

`web/src/pages/networks/NetworksPage.tsx`:

```tsx
import { useQuery } from "@tanstack/react-query";
import { Link, useParams } from "@tanstack/react-router";
import { useState } from "react";
import { networksQuery } from "@/api/queries";
import type { NetworkSummary } from "@/api/gen/types.gen";
import { columns, DataTable, type Column } from "@/components/DataTable";
import { Page } from "@/components/Page";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { CreateNetworkDialog, PruneNetworksDialog, RemoveNetworksDialog } from "./dialogs";

const col = columns<NetworkSummary>();

function networkColumns(env: string): Column<NetworkSummary>[] {
  return [
    col.accessor("name", {
      header: "Name",
      enableHiding: false,
      cell: (c) => (
        <Link
          to="/envs/$env/networks/$id"
          params={{ env, id: c.row.original.id }}
          className="font-medium hover:underline"
        >
          {c.getValue()}
        </Link>
      ),
    }),
    col.accessor("driver", { header: "Driver" }),
    col.accessor("scope", { header: "Scope" }),
    col.accessor((n) => n.subnet ?? "", { id: "subnet", header: "Subnet", cell: (c) => c.getValue() || "—" }),
    col.accessor((n) => n.gateway ?? "", { id: "gateway", header: "Gateway", cell: (c) => c.getValue() || "—" }),
    col.accessor((n) => [n.internal && "internal", n.attachable && "attachable"].filter(Boolean).join(" "), {
      id: "flags",
      header: "Flags",
      enableSorting: false,
      cell: (c) => (
        <span className="flex gap-1">
          {c.row.original.internal && <Badge variant="outline">internal</Badge>}
          {c.row.original.attachable && <Badge variant="outline">attachable</Badge>}
        </span>
      ),
    }),
  ];
}

export function NetworksPage() {
  const { env } = useParams({ from: "/authed/shell/envs/$env/networks" });
  const list = useQuery(networksQuery(env));
  const [cols] = useState(() => networkColumns(env));
  const [removing, setRemoving] = useState<NetworkSummary[] | null>(null);
  const [pruning, setPruning] = useState(false);
  const [creating, setCreating] = useState(false);
  return (
    <Page
      title="Networks"
      actions={
        <>
          <Button variant="outline" onClick={() => setPruning(true)}>
            Prune
          </Button>
          <Button onClick={() => setCreating(true)}>Create network</Button>
        </>
      }
    >
      <DataTable
        label="Networks"
        data={list.data}
        error={list.error}
        columns={cols}
        getRowId={(n) => n.id}
        empty="No networks in this environment."
        initialSorting={[{ id: "name", desc: false }]}
        selectable
        actions={(selected, clear) => (
          <Button
            variant="destructive"
            size="sm"
            onClick={() => {
              setRemoving(selected);
              clear();
            }}
          >
            Remove
          </Button>
        )}
      />
      {removing && (
        <RemoveNetworksDialog
          env={env}
          networks={removing.map((n) => ({ id: n.id, name: n.name }))}
          onDone={() => setRemoving(null)}
        />
      )}
      {pruning && <PruneNetworksDialog env={env} onDone={() => setPruning(false)} />}
      {creating && <CreateNetworkDialog env={env} onClose={() => setCreating(false)} />}
    </Page>
  );
}
```

`web/src/pages/networks/NetworkPage.tsx`:

```tsx
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate, useParams } from "@tanstack/react-router";
import { useState, type FormEvent } from "react";
import { containersQuery, keys, networkQuery } from "@/api/queries";
import { connectNetwork, disconnectNetwork } from "@/api/gen/sdk.gen";
import type { NetworkInspect } from "@/api/docker";
import { Details, DetailError } from "@/components/Details";
import { JsonView } from "@/components/JsonView";
import { Page } from "@/components/Page";
import { Button } from "@/components/ui/button";
import { NativeSelect } from "@/components/ui/native-select";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { errorMessage } from "@/lib/errors";
import { formatKeyValues } from "@/lib/kv";
import { FormError } from "@/pages/AuthLayout";
import { BUILTIN_NETWORKS, RemoveNetworksDialog } from "./dialogs";

export function NetworkPage() {
  const { env, id } = useParams({ from: "/authed/shell/envs/$env/networks/$id" });
  const navigate = useNavigate();
  const q = useQuery(networkQuery(env, id));
  const [removing, setRemoving] = useState(false);
  const back = (
    <Link to="/envs/$env/networks" params={{ env }} className="underline">
      Back to networks
    </Link>
  );
  if (q.error) return <DetailError error={q.error} what="network" back={back} />;
  const n = q.data;
  if (!n) return <p className="p-6 text-sm text-muted-foreground">Loading…</p>;
  const ipam = n.IPAM?.Config ?? [];
  return (
    <Page
      title={n.Name}
      actions={
        !BUILTIN_NETWORKS.includes(n.Name) && (
          <Button variant="destructive" onClick={() => setRemoving(true)}>
            Remove
          </Button>
        )
      }
    >
      <Details
        label="Network details"
        items={[
          ["Driver", n.Driver],
          ["Scope", n.Scope],
          ["Subnet", ipam.map((c) => c.Subnet).filter(Boolean).join(", ")],
          ["Gateway", ipam.map((c) => c.Gateway).filter(Boolean).join(", ")],
          ["Internal", n.Internal ? "yes" : "no"],
          ["Attachable", n.Attachable ? "yes" : "no"],
          ["Options", formatKeyValues(n.Options) || undefined],
          ["Labels", formatKeyValues(n.Labels) || undefined],
        ]}
      />
      <Attached env={env} n={n} />
      <section className="grid gap-2">
        <h2 className="font-medium">Inspect</h2>
        <JsonView value={n} label="Network JSON" />
      </section>
      {removing && (
        <RemoveNetworksDialog
          env={env}
          networks={[{ id: n.Id, name: n.Name }]}
          onDone={(removed) => {
            setRemoving(false);
            if (removed) void navigate({ to: "/envs/$env/networks", params: { env } });
          }}
        />
      )}
    </Page>
  );
}

/** The attached containers, with disconnect, and a picker to connect another. */
function Attached({ env, n }: { env: string; n: NetworkInspect }) {
  const qc = useQueryClient();
  const containers = useQuery(containersQuery(env));
  const attached = Object.entries(n.Containers ?? {});
  const others = (containers.data ?? []).filter((c) => !n.Containers?.[c.id]);
  const [pick, setPick] = useState("");
  const refresh = () => qc.invalidateQueries({ queryKey: keys.networks(env) });
  const connect = useMutation({
    meta: { inline: true },
    mutationFn: (container: string) => connectNetwork({ path: { env, id: n.Id }, body: { container } }),
    onSuccess: () => setPick(""),
    onSettled: refresh,
  });
  const disconnect = useMutation({
    meta: { inline: true },
    mutationFn: (container: string) => disconnectNetwork({ path: { env, id: n.Id }, body: { container } }),
    onSettled: refresh,
  });
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (pick) connect.mutate(pick);
  };
  const error = connect.error ?? disconnect.error;
  return (
    <section className="grid gap-3">
      <h2 className="font-medium">Containers</h2>
      {attached.length === 0 ? (
        <p className="text-sm text-muted-foreground">No container is attached.</p>
      ) : (
        <Table aria-label="Attached containers">
          <TableHeader>
            <TableRow>
              <TableHead>Container</TableHead>
              <TableHead>IPv4 address</TableHead>
              <TableHead>MAC address</TableHead>
              <TableHead>
                <span className="sr-only">Actions</span>
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {attached.map(([cid, c]) => (
              <TableRow key={cid}>
                <TableCell>
                  <Link to="/envs/$env/containers/$id" params={{ env, id: cid }} className="font-medium hover:underline">
                    {c.Name}
                  </Link>
                </TableCell>
                <TableCell className="font-mono text-xs">{c.IPv4Address || "—"}</TableCell>
                <TableCell className="font-mono text-xs">{c.MacAddress || "—"}</TableCell>
                <TableCell className="text-right">
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={disconnect.isPending}
                    onClick={() => disconnect.mutate(cid)}
                  >
                    Disconnect
                  </Button>
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      )}
      <form onSubmit={submit} className="flex max-w-md items-center gap-2">
        <NativeSelect aria-label="Container to connect" value={pick} onChange={(e) => setPick(e.target.value)}>
          <option value="">Choose a container…</option>
          {others.map((c) => (
            <option key={c.id} value={c.id}>
              {c.name}
            </option>
          ))}
        </NativeSelect>
        <Button type="submit" variant="outline" disabled={!pick || connect.isPending}>
          Connect
        </Button>
      </form>
      <FormError message={error ? errorMessage(error) : null} />
    </section>
  );
}
```

`web/src/router.tsx`:

```diff
--- a/web/src/router.tsx
+++ b/web/src/router.tsx
@@ -171,6 +171,18 @@ const volumeRoute = createRoute({
   component: lazyRouteComponent(() => import("@/pages/volumes/VolumePage"), "VolumePage"),
 });
 
+const networksRoute = createRoute({
+  getParentRoute: () => envRoute,
+  path: "networks",
+  component: lazyRouteComponent(() => import("@/pages/networks/NetworksPage"), "NetworksPage"),
+});
+
+const networkRoute = createRoute({
+  getParentRoute: () => envRoute,
+  path: "networks/$id",
+  component: lazyRouteComponent(() => import("@/pages/networks/NetworkPage"), "NetworkPage"),
+});
+
 const accountRoute = createRoute({
   getParentRoute: () => shellRoute,
   path: "/account",
@@ -190,6 +202,8 @@ const routeTree = rootRoute.addChildren([
         imageRoute,
         volumesRoute,
         volumeRoute,
+        networksRoute,
+        networkRoute,
       ]), accountRoute])]),
 ]);
 
```

`web/src/components/Shell.tsx`:

```diff
--- a/web/src/components/Shell.tsx
+++ b/web/src/components/Shell.tsx
@@ -1,6 +1,6 @@
 import { useQuery } from "@tanstack/react-query";
 import { Link, Outlet, useNavigate, useParams } from "@tanstack/react-router";
-import { Boxes, ChevronsUpDown, Database, Layers, LayoutDashboard, LogOut, Monitor, Moon, Sun, User } from "lucide-react";
+import { Boxes, ChevronsUpDown, Database, Layers, LayoutDashboard, Network, LogOut, Monitor, Moon, Sun, User } from "lucide-react";
 import { envsQuery, envStatusQuery } from "@/api/queries";
 import { useLogout, useSession } from "@/auth/session";
 import { Badge } from "@/components/ui/badge";
@@ -24,6 +24,7 @@ const NAV = [
   { to: "/envs/$env/containers", label: "Containers", icon: Boxes },
   { to: "/envs/$env/images", label: "Images", icon: Layers },
   { to: "/envs/$env/volumes", label: "Volumes", icon: Database },
+  { to: "/envs/$env/networks", label: "Networks", icon: Network },
 ] as const;
 
 export function Shell() {
```

- [ ] **Step 4: Run all checks**

Run: `cd web && pnpm test && pnpm typecheck && pnpm lint`
Expected: 159 passed (30 files); typecheck and lint are clean.

- [ ] **Step 5: Commit**

```bash
git add web/src
git commit -m "feat(web): networks list and page with create, connect and disconnect"
```

---

### Task 10: Compose projects

**Files:**
- Create: `web/src/pages/compose/ComposePage.tsx`.
- Modify: `web/src/api/queries.ts`, `web/src/router.tsx` and `web/src/components/Shell.tsx`.
- Test: `web/src/pages/compose/ComposePage.test.tsx`.

**Interfaces:**
- Consumes: `keys.composeProjects` (P3a, invalidated by container events), `containersQuery`, and the SDK's `listComposeProjects` and `composeAction`, which returns `ComposeOutcome`: `{ affected, errors }`.
- Produces `@/api/queries`: `composeProjectsQuery(env)`.
- Produces:
  - the route `/envs/$env/compose`, and a `Compose` nav item;
  - one card per project, `role="region"` named by the project, with its containers listed under `Containers of <name>`.

- [ ] **Step 1: Write the failing test**

`web/src/pages/compose/ComposePage.test.tsx`:

```tsx
import { waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { json, mockFetch } from "@/test/fetch";
import { loggedIn, renderApp } from "@/test/app";
import { container } from "@/test/docker";

const routes = {
  ...loggedIn,
  "GET /api/v1/envs/local/compose-projects": {
    json: [{ name: "shop", status: "partial", containers: 2, running: 1, services: ["web", "db"] }],
  },
  "GET /api/v1/envs/local/containers": {
    json: [
      container({ id: "c1", name: "shop-web-1", compose_project: "shop" }),
      container({ id: "c2", name: "shop-db-1", compose_project: "shop", state: "exited", status: "Exited (1)" }),
      container({ id: "c3", name: "loner" }),
    ],
  },
};

describe("compose projects", () => {
  it("shows each project's status and containers", async () => {
    mockFetch(routes);
    const { screen } = renderApp("/envs/local/compose");
    const card = await screen.findByRole("region", { name: "shop" });
    expect(within(card).getByText("partial")).toBeInTheDocument();
    expect(within(card).getByText("1 of 2 running")).toBeInTheDocument();
    expect(within(card).getByText("Services: web, db")).toBeInTheDocument();
    const list = within(card).getByRole("list", { name: "Containers of shop" });
    await waitFor(() => expect(within(list).getAllByRole("listitem")).toHaveLength(2));
    expect(within(list).queryByText("loner")).not.toBeInTheDocument();
  });

  it("starts a project and reports containers that failed", async () => {
    const calls = mockFetch({
      ...routes,
      "POST /api/v1/envs/local/compose-projects/shop/start": {
        json: { affected: 1, errors: ["shop-db-1: port is already allocated"] },
      },
    });
    const { screen } = renderApp("/envs/local/compose");
    const card = await screen.findByRole("region", { name: "shop" });
    await userEvent.click(within(card).getByRole("button", { name: "Start" }));
    expect(
      await screen.findByText("Started 1 container of shop. shop-db-1: port is already allocated"),
    ).toBeInTheDocument();
    expect(calls.some((c) => c.method === "POST" && c.path.endsWith("/shop/start"))).toBe(true);
  });

  it("takes a project down after confirming", async () => {
    const calls = mockFetch({
      ...routes,
      "POST /api/v1/envs/local/compose-projects/shop/down": { json: { affected: 2, errors: [] } },
    });
    const { screen } = renderApp("/envs/local/compose");
    const card = await screen.findByRole("region", { name: "shop" });
    await userEvent.click(within(card).getByRole("button", { name: "Down" }));
    const dialog = await screen.findByRole("dialog", { name: "Take shop down?" });
    expect(calls.some((c) => c.path.endsWith("/down"))).toBe(false);
    await userEvent.click(within(dialog).getByRole("button", { name: "Down" }));
    expect(await screen.findByText("Removed 2 containers of shop")).toBeInTheDocument();
  });

  it("toasts Docker's error when an action fails outright", async () => {
    mockFetch({
      ...routes,
      "POST /api/v1/envs/local/compose-projects/shop/stop": () =>
        json(502, { code: "upstream", message: "Cannot connect to the Docker daemon" }),
    });
    const { screen } = renderApp("/envs/local/compose");
    const card = await screen.findByRole("region", { name: "shop" });
    await userEvent.click(within(card).getByRole("button", { name: "Stop" }));
    expect(await screen.findByText("Cannot connect to the Docker daemon")).toBeInTheDocument();
  });

  it("says when there are none", async () => {
    mockFetch({ ...routes, "GET /api/v1/envs/local/compose-projects": { json: [] } });
    const { screen } = renderApp("/envs/local/compose");
    expect(await screen.findByText("No compose projects in this environment.")).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cd web && pnpm test src/pages/compose`
Expected: FAIL: all 5 tests, with `Unable to find role="region" and name "shop"` and `Unable to find an element with the text: No compose projects in this environment.`

- [ ] **Step 3: Implement**

`web/src/api/queries.ts`:

```diff
--- a/web/src/api/queries.ts
+++ b/web/src/api/queries.ts
@@ -14,6 +14,7 @@ import {
   inspectNetwork,
   inspectVolume,
   listContainerFiles,
+  listComposeProjects,
   listContainers,
   listEnvs,
   listImages,
@@ -166,6 +167,12 @@ export const networkQuery = (env: string, id: string) =>
     retry: retryUnlessGone,
   });
 
+export const composeProjectsQuery = (env: string) =>
+  queryOptions({
+    queryKey: keys.composeProjects(env),
+    queryFn: async () => (await listComposeProjects(path(env))).data,
+  });
+
 export const recentEventsQuery = (env: string) =>
   queryOptions<DockerEvent[]>({
     queryKey: keys.recentEvents(env),
```

`web/src/pages/compose/ComposePage.tsx`:

```tsx
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useParams } from "@tanstack/react-router";
import { useState } from "react";
import { toast } from "sonner";
import { composeProjectsQuery, containersQuery, keys } from "@/api/queries";
import { composeAction } from "@/api/gen/sdk.gen";
import type { ComposeActionName, ComposeProject } from "@/api/gen/types.gen";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { Page } from "@/components/Page";
import { StateBadge } from "@/components/StateBadge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { errorMessage } from "@/lib/errors";

const DONE: Record<ComposeActionName, string> = {
  start: "Started",
  stop: "Stopped",
  restart: "Restarted",
  down: "Removed",
};

/** Compose projects, from the containers' `com.docker.compose.project` labels. */
export function ComposePage() {
  const { env } = useParams({ from: "/authed/shell/envs/$env/compose" });
  const projects = useQuery(composeProjectsQuery(env));
  return (
    <Page title="Compose projects">
      <p className="text-sm text-muted-foreground">
        Projects are read from the labels Docker Compose puts on containers. Editing and deploying stacks comes in a
        later release.
      </p>
      {projects.error ? (
        <p role="alert" className="text-sm text-destructive">
          {errorMessage(projects.error)}
        </p>
      ) : !projects.data ? (
        <p className="text-sm text-muted-foreground">Loading…</p>
      ) : projects.data.length === 0 ? (
        <p className="text-sm text-muted-foreground">No compose projects in this environment.</p>
      ) : (
        <div className="grid gap-4">
          {projects.data.map((p) => (
            <ProjectCard key={p.name} env={env} project={p} />
          ))}
        </div>
      )}
    </Page>
  );
}

function ProjectCard({ env, project }: { env: string; project: ComposeProject }) {
  const qc = useQueryClient();
  const containers = useQuery(containersQuery(env));
  const [downing, setDowning] = useState(false);
  const members = (containers.data ?? []).filter((c) => c.compose_project === project.name);
  const run = async (action: ComposeActionName) => {
    const { data } = await composeAction({ path: { env, project: project.name, action } });
    await qc.invalidateQueries({ queryKey: keys.containers(env) });
    await qc.invalidateQueries({ queryKey: keys.composeProjects(env) });
    const n = data.affected;
    const msg = `${DONE[action]} ${n} container${n === 1 ? "" : "s"} of ${project.name}`;
    if (data.errors.length) toast.error(`${msg}. ${data.errors.join("; ")}`);
    else toast.success(msg);
  };
  const act = useMutation({ mutationFn: run });
  return (
    <Card aria-label={project.name} role="region">
      <CardHeader className="flex flex-row flex-wrap items-center justify-between gap-2">
        <CardTitle className="flex items-center gap-3">
          {project.name}
          <StateBadge state={project.status} />
          <span className="text-sm font-normal text-muted-foreground">
            {project.running} of {project.containers} running
          </span>
        </CardTitle>
        <div className="flex flex-wrap gap-2">
          {(["start", "stop", "restart"] as const).map((a) => (
            <Button key={a} variant="outline" size="sm" disabled={act.isPending} onClick={() => act.mutate(a)}>
              {a[0].toUpperCase() + a.slice(1)}
            </Button>
          ))}
          <Button variant="destructive" size="sm" onClick={() => setDowning(true)}>
            Down
          </Button>
        </div>
      </CardHeader>
      <CardContent className="grid gap-2 text-sm">
        <div className="text-muted-foreground">Services: {project.services.join(", ")}</div>
        <ul className="grid gap-1" aria-label={`Containers of ${project.name}`}>
          {members.map((c) => (
            <li key={c.id} className="flex items-center gap-2">
              <Link to="/envs/$env/containers/$id" params={{ env, id: c.id }} className="font-medium hover:underline">
                {c.name}
              </Link>
              <StateBadge state={c.state} />
              <span className="text-muted-foreground">{c.status}</span>
            </li>
          ))}
        </ul>
      </CardContent>
      <ConfirmDialog
        open={downing}
        onOpenChange={setDowning}
        title={`Take ${project.name} down?`}
        description="Stops and removes the project's containers. Volumes, networks and images are kept."
        confirmLabel="Down"
        onConfirm={() => run("down")}
      />
    </Card>
  );
}
```

`web/src/router.tsx`:

```diff
--- a/web/src/router.tsx
+++ b/web/src/router.tsx
@@ -183,6 +183,12 @@ const networkRoute = createRoute({
   component: lazyRouteComponent(() => import("@/pages/networks/NetworkPage"), "NetworkPage"),
 });
 
+const composeRoute = createRoute({
+  getParentRoute: () => envRoute,
+  path: "compose",
+  component: lazyRouteComponent(() => import("@/pages/compose/ComposePage"), "ComposePage"),
+});
+
 const accountRoute = createRoute({
   getParentRoute: () => shellRoute,
   path: "/account",
@@ -204,6 +210,7 @@ const routeTree = rootRoute.addChildren([
         volumeRoute,
         networksRoute,
         networkRoute,
+        composeRoute,
       ]), accountRoute])]),
 ]);
 
```

`web/src/components/Shell.tsx`:

```diff
--- a/web/src/components/Shell.tsx
+++ b/web/src/components/Shell.tsx
@@ -1,6 +1,6 @@
 import { useQuery } from "@tanstack/react-query";
 import { Link, Outlet, useNavigate, useParams } from "@tanstack/react-router";
-import { Boxes, ChevronsUpDown, Database, Layers, LayoutDashboard, Network, LogOut, Monitor, Moon, Sun, User } from "lucide-react";
+import { Boxes, ChevronsUpDown, Database, Layers, LayoutDashboard, Network, Rows3, LogOut, Monitor, Moon, Sun, User } from "lucide-react";
 import { envsQuery, envStatusQuery } from "@/api/queries";
 import { useLogout, useSession } from "@/auth/session";
 import { Badge } from "@/components/ui/badge";
@@ -25,6 +25,7 @@ const NAV = [
   { to: "/envs/$env/images", label: "Images", icon: Layers },
   { to: "/envs/$env/volumes", label: "Volumes", icon: Database },
   { to: "/envs/$env/networks", label: "Networks", icon: Network },
+  { to: "/envs/$env/compose", label: "Compose", icon: Rows3 },
 ] as const;
 
 export function Shell() {
```

- [ ] **Step 4: Run all checks**

Run: `cd web && pnpm test && pnpm typecheck && pnpm lint`
Expected: 164 passed (31 files); typecheck and lint are clean.

- [ ] **Step 5: Commit**

```bash
git add web/src
git commit -m "feat(web): compose projects with start, stop, restart and down"
```

---

### Task 11: Users and environments (admin)

**Files:**
- Create: `web/src/pages/admin/FormDialog.tsx`, `web/src/pages/admin/UsersPage.tsx` and `web/src/pages/admin/EnvironmentsPage.tsx`.
- Modify: `web/src/api/queries.ts`, `web/src/router.tsx` and `web/src/components/Shell.tsx`.
- Test: `web/src/pages/admin/admin.test.tsx`.

**Interfaces:**
- Consumes:
  - `useSession` and `meQuery` (P3a), and `EnvStatusBadge` from `Shell`;
  - the SDK's `listUsers`, `createUser`, `updateUser`, `deleteUser`, `createEnv`, `updateEnv` and `deleteEnv`;
  - the server's rules: `admin` only; the last admin can be neither demoted nor deleted (409); and a password reset logs out the user's other sessions.
- Produces `@/api/queries`: `keys.users` (`["users"]`), `keys.audit` (`["audit"]`) and `usersQuery`.
- Produces `pages/admin/FormDialog.tsx`: `<FormDialog title description? submitLabel onSubmit onClose>`, which closes on success and shows the error inline.
- Produces:
  - `adminRoute`: path `/admin`. Its `beforeLoad` redirects anyone who is not an admin to `/`.
  - its children `/admin/users` and `/admin/envs`.
  - `Shell`'s `Administration` nav, for admins only.
  - `ADMIN_NAV`: Users and Environments; Task 12 adds the audit log.

- [ ] **Step 1: Write the failing test**

`web/src/pages/admin/admin.test.tsx`:

```tsx
import { waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { User } from "@/api/gen/types.gen";
import { json, mockFetch } from "@/test/fetch";
import { ADMIN, ENVS, loggedIn, renderApp } from "@/test/app";

const BOB: User = { id: 2, username: "bob", role: "user", created_at: 1_790_000_000, updated_at: 0 };
const routes = { ...loggedIn, "GET /api/v1/users": { json: [ADMIN.user, BOB] } };

const row = (screen: typeof import("@testing-library/react").screen, name: string) =>
  within(screen.getByRole("table", { name: /Users|Environments/ }))
    .getAllByRole("row")
    .find((r) => r.textContent?.includes(name))!;

describe("admin access", () => {
  it("shows the administration links to admins only", async () => {
    mockFetch(routes);
    const { screen } = renderApp("/envs/local");
    const nav = await screen.findByRole("navigation", { name: "Administration" });
    expect(within(nav).getByRole("link", { name: "Users" })).toHaveAttribute("href", "/admin/users");
  });

  it("sends a user who is not an admin away", async () => {
    mockFetch({ ...routes, "GET /api/v1/auth/me": { json: { ...ADMIN, user: BOB } } });
    const { screen, history } = renderApp("/admin/users");
    await waitFor(() => expect(history.location.pathname).toBe("/envs/local"));
    expect(screen.queryByRole("navigation", { name: "Administration" })).not.toBeInTheDocument();
  });
});

describe("users", () => {
  it("lists users and marks the current one", async () => {
    mockFetch(routes);
    const { screen } = renderApp("/admin/users");
    await waitFor(() => expect(row(screen, "admin")).toHaveTextContent("(you)"));
    expect(row(screen, "bob")).toHaveTextContent("user");
  });

  it("adds a user", async () => {
    let users = [ADMIN.user, BOB];
    const calls = mockFetch({
      ...routes,
      "GET /api/v1/users": () => json(200, users),
      "POST /api/v1/users": (c) => {
        const u = { ...BOB, id: 3, ...(c.body as object) } as User;
        users = [...users, u];
        return json(201, u);
      },
    });
    const { screen } = renderApp("/admin/users");
    await userEvent.click(await screen.findByRole("button", { name: "Add user" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText("Username"), "carol");
    await userEvent.type(within(dialog).getByLabelText("Password"), "a-long-password");
    await userEvent.selectOptions(within(dialog).getByLabelText("Role"), "admin");
    await userEvent.click(within(dialog).getByRole("button", { name: "Add" }));
    await waitFor(() => expect(row(screen, "carol")).toBeTruthy());
    expect(calls.find((c) => c.method === "POST")?.body).toEqual({
      username: "carol",
      password: "a-long-password",
      role: "admin",
    });
  });

  it("shows the server's reason a user cannot be added", async () => {
    mockFetch({
      ...routes,
      "POST /api/v1/users": () => json(400, { code: "invalid", message: "password must be 12-128 characters" }),
    });
    const { screen } = renderApp("/admin/users");
    await userEvent.click(await screen.findByRole("button", { name: "Add user" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText("Username"), "carol");
    await userEvent.type(within(dialog).getByLabelText("Password"), "short");
    await userEvent.click(within(dialog).getByRole("button", { name: "Add" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("password must be 12-128 characters");
  });

  it("changes a role and resets a password", async () => {
    const calls = mockFetch({ ...routes, "PATCH /api/v1/users/2": { json: BOB } });
    const { screen } = renderApp("/admin/users");
    await waitFor(() => expect(row(screen, "bob")).toBeTruthy());
    await userEvent.click(within(row(screen, "bob")).getByRole("button", { name: "Change role" }));
    let dialog = await screen.findByRole("dialog");
    await userEvent.selectOptions(within(dialog).getByLabelText("Role"), "admin");
    await userEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    await userEvent.click(within(row(screen, "bob")).getByRole("button", { name: "Reset password" }));
    dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText("New password"), "another-long-one");
    await userEvent.click(within(dialog).getByRole("button", { name: "Reset password" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(calls.filter((c) => c.method === "PATCH").map((c) => c.body)).toEqual([
      { role: "admin" },
      { password: "another-long-one" },
    ]);
  });

  it("refuses to delete the last admin with the server's reason", async () => {
    mockFetch({
      ...routes,
      "DELETE /api/v1/users/1": () => json(409, { code: "conflict", message: "cannot delete the last admin" }),
    });
    const { screen } = renderApp("/admin/users");
    await waitFor(() => expect(row(screen, "admin")).toBeTruthy());
    await userEvent.click(within(row(screen, "admin")).getByRole("button", { name: "Delete" }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("This is your own account");
    await userEvent.click(within(dialog).getByRole("button", { name: "Delete" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("cannot delete the last admin");
  });
});

describe("environments", () => {
  it("lists environments with their status", async () => {
    mockFetch(routes);
    const { screen } = renderApp("/admin/envs");
    await waitFor(() => expect(row(screen, "local")).toHaveTextContent("up"));
    expect(row(screen, "local")).toHaveTextContent("unix:///var/run/docker.sock");
  });

  it("adds an environment", async () => {
    const calls = mockFetch({
      ...routes,
      "POST /api/v1/envs": { status: 201, json: { ...ENVS[0], id: 2, name: "remote", url: "tcp://10.0.0.5:2375" } },
    });
    const { screen } = renderApp("/admin/envs");
    await userEvent.click(await screen.findByRole("button", { name: "Add environment" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText("Name"), "remote");
    await userEvent.clear(within(dialog).getByLabelText("Docker URL"));
    await userEvent.type(within(dialog).getByLabelText("Docker URL"), "tcp://10.0.0.5:2375");
    await userEvent.click(within(dialog).getByRole("button", { name: "Add" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(calls.find((c) => c.method === "POST")?.body).toEqual({
      name: "remote",
      url: "tcp://10.0.0.5:2375",
      kind: "docker-local",
    });
  });

  it("renames an environment, sending only what changed", async () => {
    const calls = mockFetch({ ...routes, "PATCH /api/v1/envs/local": { json: { ...ENVS[0], name: "laptop" } } });
    const { screen } = renderApp("/admin/envs");
    await waitFor(() => expect(row(screen, "local")).toBeTruthy());
    await userEvent.click(within(row(screen, "local")).getByRole("button", { name: "Edit" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.clear(within(dialog).getByLabelText("Name"));
    await userEvent.type(within(dialog).getByLabelText("Name"), "laptop");
    await userEvent.click(within(dialog).getByRole("button", { name: "Save" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(calls.find((c) => c.method === "PATCH")?.body).toEqual({ name: "laptop", url: null });
  });

  it("deletes an environment after confirming", async () => {
    const calls = mockFetch({ ...routes, "DELETE /api/v1/envs/local": { status: 204 } });
    const { screen } = renderApp("/admin/envs");
    await waitFor(() => expect(row(screen, "local")).toBeTruthy());
    await userEvent.click(within(row(screen, "local")).getByRole("button", { name: "Delete" }));
    await userEvent.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Delete" }));
    await waitFor(() => expect(calls.some((c) => c.method === "DELETE")).toBe(true));
  });
});
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cd web && pnpm test src/pages/admin`
Expected: FAIL: all 11 tests. The failures include `Unable to find role="navigation" and name "Administration"`, `expected '/admin/users' to be '/envs/local'` and `Unable to find role="button" and name "Add user"`.

- [ ] **Step 3: Implement**

`web/src/api/queries.ts`:

```diff
--- a/web/src/api/queries.ts
+++ b/web/src/api/queries.ts
@@ -20,6 +20,7 @@ import {
   listImages,
   listNetworks,
   listSessions,
+  listUsers,
   listVolumes,
 } from "./gen/sdk.gen";
 import type { DockerEvent } from "./gen/types.gen";
@@ -34,6 +35,8 @@ export const keys = {
   me: ["auth", "me"] as const,
   sessions: ["auth", "sessions"] as const,
   envs: ["envs"] as const,
+  users: ["users"] as const,
+  audit: ["audit"] as const,
   env: (env: string) => ["env", env] as const,
   envStatus: (env: string) => ["env", env, "status"] as const,
   info: (env: string) => ["env", env, "info"] as const,
@@ -83,6 +86,11 @@ export const envsQuery = queryOptions({
   queryFn: async () => (await listEnvs()).data,
 });
 
+export const usersQuery = queryOptions({
+  queryKey: keys.users,
+  queryFn: async () => (await listUsers()).data,
+});
+
 const path = (env: string) => ({ path: { env } });
 
 /** How often a down environment's status is checked, to notice it coming back. */
```

`web/src/pages/admin/FormDialog.tsx`:

```tsx
import { useMutation } from "@tanstack/react-query";
import type { FormEvent, ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { errorMessage } from "@/lib/errors";
import { FormError } from "@/pages/AuthLayout";

/** A dialog around a small form: submits `onSubmit`, closes when it resolves, shows its error when it fails. */
export function FormDialog(props: {
  title: string;
  description?: string;
  submitLabel: string;
  onSubmit: () => Promise<unknown>;
  onClose: () => void;
  children: ReactNode;
}) {
  const m = useMutation({ mutationFn: props.onSubmit, meta: { inline: true }, onSuccess: props.onClose });
  const submit = (e: FormEvent) => {
    e.preventDefault();
    m.mutate();
  };
  return (
    <Dialog open onOpenChange={(open) => !open && props.onClose()}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{props.title}</DialogTitle>
          {props.description && <DialogDescription>{props.description}</DialogDescription>}
        </DialogHeader>
        <form onSubmit={submit} className="grid gap-4">
          {props.children}
          <FormError message={m.error ? errorMessage(m.error) : null} />
          <DialogFooter>
            <Button type="submit" disabled={m.isPending}>
              {props.submitLabel}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
```

`web/src/pages/admin/UsersPage.tsx`:

```tsx
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { toast } from "sonner";
import { keys, usersQuery } from "@/api/queries";
import { createUser, deleteUser, updateUser } from "@/api/gen/sdk.gen";
import type { Role, User } from "@/api/gen/types.gen";
import { useSession } from "@/auth/session";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { columns, DataTable, type Column } from "@/components/DataTable";
import { Page } from "@/components/Page";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { NativeSelect } from "@/components/ui/native-select";
import { formatTime } from "@/lib/format";
import { FormDialog } from "./FormDialog";

type Dialog = { kind: "add" } | { kind: "password" | "role" | "delete"; user: User } | null;

const col = columns<User>();

export function UsersPage() {
  const { user: me } = useSession();
  const qc = useQueryClient();
  const users = useQuery(usersQuery);
  const [dialog, setDialog] = useState<Dialog>(null);
  const close = () => setDialog(null);
  const refresh = () => qc.invalidateQueries({ queryKey: keys.users });
  const [cols] = useState<Column<User>[]>(() => [
    col.accessor("username", {
      header: "Username",
      cell: (c) => (
        <span className="font-medium">
          {c.getValue()}
          {c.row.original.id === me.id && <span className="ml-2 text-xs text-muted-foreground">(you)</span>}
        </span>
      ),
    }),
    col.accessor("role", { header: "Role" }),
    col.accessor("created_at", { header: "Created", cell: (c) => formatTime(c.getValue()) }),
    col.display({
      id: "actions",
      header: () => <span className="sr-only">Actions</span>,
      cell: (c) => (
        <span className="flex justify-end gap-2">
          <Button variant="outline" size="sm" onClick={() => setDialog({ kind: "role", user: c.row.original })}>
            Change role
          </Button>
          <Button variant="outline" size="sm" onClick={() => setDialog({ kind: "password", user: c.row.original })}>
            Reset password
          </Button>
          <Button variant="outline" size="sm" onClick={() => setDialog({ kind: "delete", user: c.row.original })}>
            Delete
          </Button>
        </span>
      ),
    }),
  ]);
  return (
    <Page title="Users" actions={<Button onClick={() => setDialog({ kind: "add" })}>Add user</Button>}>
      <DataTable
        label="Users"
        data={users.data}
        error={users.error}
        columns={cols}
        getRowId={(u) => String(u.id)}
        empty="No users."
        initialSorting={[{ id: "username", desc: false }]}
      />
      {dialog?.kind === "add" && <AddUser onClose={close} onDone={refresh} />}
      {dialog?.kind === "role" && <ChangeRole user={dialog.user} onClose={close} onDone={refresh} />}
      {dialog?.kind === "password" && <ResetPassword user={dialog.user} self={dialog.user.id === me.id} onClose={close} />}
      {dialog?.kind === "delete" && (
        <ConfirmDialog
          open
          onOpenChange={(open) => !open && close()}
          title={`Delete ${dialog.user.username}?`}
          description={
            dialog.user.id === me.id
              ? "This is your own account: you will be logged out."
              : "They are logged out everywhere and can no longer sign in."
          }
          confirmLabel="Delete"
          onConfirm={async () => {
            await deleteUser({ path: { id: dialog.user.id } });
            toast.success(`Deleted ${dialog.user.username}`);
            await refresh();
          }}
        />
      )}
    </Page>
  );
}

function AddUser({ onClose, onDone }: { onClose: () => void; onDone: () => Promise<unknown> }) {
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [role, setRole] = useState<Role>("user");
  return (
    <FormDialog
      title="Add user"
      submitLabel="Add"
      onClose={onClose}
      onSubmit={async () => {
        await createUser({ body: { username: username.trim(), password, role } });
        toast.success(`Added ${username.trim()}`);
        await onDone();
      }}
    >
      <div className="grid gap-2">
        <Label htmlFor="user-name">Username</Label>
        <Input id="user-name" required autoComplete="off" value={username} onChange={(e) => setUsername(e.target.value)} />
      </div>
      <div className="grid gap-2">
        <Label htmlFor="user-password">Password</Label>
        <Input
          id="user-password"
          type="password"
          required
          autoComplete="new-password"
          value={password}
          onChange={(e) => setPassword(e.target.value)}
        />
      </div>
      <RoleSelect value={role} onChange={setRole} />
    </FormDialog>
  );
}

function ChangeRole({ user, onClose, onDone }: { user: User; onClose: () => void; onDone: () => Promise<unknown> }) {
  const [role, setRole] = useState<Role>(user.role);
  return (
    <FormDialog
      title={`Change role of ${user.username}`}
      submitLabel="Save"
      onClose={onClose}
      onSubmit={async () => {
        await updateUser({ path: { id: user.id }, body: { role } });
        await onDone();
      }}
    >
      <RoleSelect value={role} onChange={setRole} />
    </FormDialog>
  );
}

function ResetPassword({ user, self, onClose }: { user: User; self: boolean; onClose: () => void }) {
  const [password, setPassword] = useState("");
  return (
    <FormDialog
      title={`Reset password of ${user.username}`}
      description={
        self ? "Your other sessions are logged out." : "They are logged out everywhere and sign in with the new password."
      }
      submitLabel="Reset password"
      onClose={onClose}
      onSubmit={async () => {
        await updateUser({ path: { id: user.id }, body: { password } });
        toast.success(`Password of ${user.username} reset`);
      }}
    >
      <div className="grid gap-2">
        <Label htmlFor="reset-password">New password</Label>
        <Input
          id="reset-password"
          type="password"
          required
          autoComplete="new-password"
          value={password}
          onChange={(e) => setPassword(e.target.value)}
        />
      </div>
    </FormDialog>
  );
}

function RoleSelect({ value, onChange }: { value: Role; onChange: (r: Role) => void }) {
  return (
    <div className="grid gap-2">
      <Label htmlFor="user-role">Role</Label>
      <NativeSelect id="user-role" value={value} onChange={(e) => onChange(e.target.value as Role)}>
        <option value="user">user: manages Docker resources</option>
        <option value="admin">admin: also manages users and environments</option>
      </NativeSelect>
    </div>
  );
}
```

`web/src/pages/admin/EnvironmentsPage.tsx`:

```tsx
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { toast } from "sonner";
import { envsQuery, keys } from "@/api/queries";
import { createEnv, deleteEnv, updateEnv } from "@/api/gen/sdk.gen";
import type { EnvironmentRecord } from "@/api/gen/types.gen";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { columns, DataTable, type Column } from "@/components/DataTable";
import { Page } from "@/components/Page";
import { EnvStatusBadge } from "@/components/Shell";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { FormDialog } from "./FormDialog";

type Dialog = { kind: "add" } | { kind: "edit" | "delete"; env: EnvironmentRecord } | null;

const col = columns<EnvironmentRecord>();

export function EnvironmentsPage() {
  const qc = useQueryClient();
  const envs = useQuery(envsQuery);
  const [dialog, setDialog] = useState<Dialog>(null);
  const close = () => setDialog(null);
  // Renaming or removing one changes every environment-scoped key, so all are refetched.
  const refresh = () => Promise.all([qc.invalidateQueries({ queryKey: keys.envs }), qc.invalidateQueries({ queryKey: ["env"] })]);
  const [cols] = useState<Column<EnvironmentRecord>[]>(() => [
    col.accessor("name", { header: "Name", cell: (c) => <span className="font-medium">{c.getValue()}</span> }),
    col.display({ id: "status", header: "Status", cell: (c) => <EnvStatusBadge env={c.row.original.name} /> }),
    col.accessor("kind", { header: "Kind" }),
    col.accessor("url", { header: "URL", cell: (c) => <code className="text-xs">{c.getValue()}</code> }),
    col.display({
      id: "actions",
      header: () => <span className="sr-only">Actions</span>,
      cell: (c) => (
        <span className="flex justify-end gap-2">
          <Button variant="outline" size="sm" onClick={() => setDialog({ kind: "edit", env: c.row.original })}>
            Edit
          </Button>
          <Button variant="outline" size="sm" onClick={() => setDialog({ kind: "delete", env: c.row.original })}>
            Delete
          </Button>
        </span>
      ),
    }),
  ]);
  return (
    <Page title="Environments" actions={<Button onClick={() => setDialog({ kind: "add" })}>Add environment</Button>}>
      <DataTable
        label="Environments"
        data={envs.data}
        error={envs.error}
        columns={cols}
        getRowId={(e) => String(e.id)}
        empty="No environments."
        initialSorting={[{ id: "name", desc: false }]}
      />
      {dialog?.kind === "add" && <EnvForm onClose={close} onDone={refresh} />}
      {dialog?.kind === "edit" && <EnvForm env={dialog.env} onClose={close} onDone={refresh} />}
      {dialog?.kind === "delete" && (
        <ConfirmDialog
          open
          onOpenChange={(open) => !open && close()}
          title={`Delete environment ${dialog.env.name}?`}
          description="FleetOps stops managing it. Its containers and other resources are not touched."
          confirmLabel="Delete"
          onConfirm={async () => {
            await deleteEnv({ path: { env: dialog.env.name } });
            toast.success(`Deleted ${dialog.env.name}`);
            await refresh();
          }}
        />
      )}
    </Page>
  );
}

function EnvForm({ env, onClose, onDone }: { env?: EnvironmentRecord; onClose: () => void; onDone: () => Promise<unknown> }) {
  const [name, setName] = useState(env?.name ?? "");
  const [url, setUrl] = useState(env?.url ?? "unix:///var/run/docker.sock");
  return (
    <FormDialog
      title={env ? `Edit ${env.name}` : "Add environment"}
      description="A Docker engine this server can reach: a unix socket, or tcp:// / http(s):// for an engine exposing its API."
      submitLabel={env ? "Save" : "Add"}
      onClose={onClose}
      onSubmit={async () => {
        if (env) {
          const body = { name: name.trim() === env.name ? null : name.trim(), url: url.trim() === env.url ? null : url.trim() };
          if (!body.name && !body.url) return;
          await updateEnv({ path: { env: env.name }, body });
        } else {
          await createEnv({ body: { name: name.trim(), url: url.trim(), kind: "docker-local" } });
        }
        await onDone();
      }}
    >
      <div className="grid gap-2">
        <Label htmlFor="env-name">Name</Label>
        <Input id="env-name" required value={name} onChange={(e) => setName(e.target.value)} />
      </div>
      <div className="grid gap-2">
        <Label htmlFor="env-url">Docker URL</Label>
        <Input id="env-url" required className="font-mono" value={url} onChange={(e) => setUrl(e.target.value)} />
      </div>
    </FormDialog>
  );
}
```

`web/src/router.tsx`: the admin routes. The route tree is also written out one route per line from here on.

```diff
--- a/web/src/router.tsx
+++ b/web/src/router.tsx
@@ -189,6 +189,29 @@ const composeRoute = createRoute({
   component: lazyRouteComponent(() => import("@/pages/compose/ComposePage"), "ComposePage"),
 });
 
+/** Administration: admins only; anyone else is sent to the start page. */
+const adminRoute = createRoute({
+  getParentRoute: () => shellRoute,
+  path: "/admin",
+  beforeLoad: async ({ context }) => {
+    const me = await context.queryClient.ensureQueryData(meQuery);
+    if (me.user.role !== "admin") throw redirect({ to: "/" });
+  },
+  component: Outlet,
+});
+
+const usersRoute = createRoute({
+  getParentRoute: () => adminRoute,
+  path: "users",
+  component: lazyRouteComponent(() => import("@/pages/admin/UsersPage"), "UsersPage"),
+});
+
+const envsAdminRoute = createRoute({
+  getParentRoute: () => adminRoute,
+  path: "envs",
+  component: lazyRouteComponent(() => import("@/pages/admin/EnvironmentsPage"), "EnvironmentsPage"),
+});
+
 const accountRoute = createRoute({
   getParentRoute: () => shellRoute,
   path: "/account",
@@ -198,7 +221,10 @@ const accountRoute = createRoute({
 const routeTree = rootRoute.addChildren([
   setupRoute,
   loginRoute,
-  authedRoute.addChildren([indexRoute, shellRoute.addChildren([envRoute.addChildren([
+  authedRoute.addChildren([
+    indexRoute,
+    shellRoute.addChildren([
+      envRoute.addChildren([
         dashboardRoute,
         containersRoute,
         newContainerRoute,
@@ -211,7 +237,11 @@ const routeTree = rootRoute.addChildren([
         networksRoute,
         networkRoute,
         composeRoute,
-      ]), accountRoute])]),
+      ]),
+      adminRoute.addChildren([usersRoute, envsAdminRoute]),
+      accountRoute,
+    ]),
+  ]),
 ]);
 
 export function createAppRouter(queryClient: QueryClient, history?: RouterHistory) {
```

`web/src/components/Shell.tsx`:

```diff
--- a/web/src/components/Shell.tsx
+++ b/web/src/components/Shell.tsx
@@ -1,6 +1,21 @@
 import { useQuery } from "@tanstack/react-query";
 import { Link, Outlet, useNavigate, useParams } from "@tanstack/react-router";
-import { Boxes, ChevronsUpDown, Database, Layers, LayoutDashboard, Network, Rows3, LogOut, Monitor, Moon, Sun, User } from "lucide-react";
+import {
+  Boxes,
+  ChevronsUpDown,
+  Database,
+  Layers,
+  LayoutDashboard,
+  LogOut,
+  Monitor,
+  Moon,
+  Network,
+  Rows3,
+  Server,
+  Sun,
+  User,
+  Users,
+} from "lucide-react";
 import { envsQuery, envStatusQuery } from "@/api/queries";
 import { useLogout, useSession } from "@/auth/session";
 import { Badge } from "@/components/ui/badge";
@@ -18,6 +33,15 @@ import {
 import { useEnvEvents, useLive } from "@/live/LiveProvider";
 import { useTheme, type Theme } from "@/theme";
 
+const LINK =
+  "flex items-center gap-2 rounded-md px-2 py-1.5 text-sm hover:bg-accent data-[status=active]:bg-accent data-[status=active]:font-medium";
+
+/** Administration pages, shown to admins. */
+const ADMIN_NAV = [
+  { to: "/admin/users", label: "Users", icon: Users },
+  { to: "/admin/envs", label: "Environments", icon: Server },
+] as const;
+
 /** The environment-scoped pages, in sidebar order. */
 const NAV = [
   { to: "/envs/$env", label: "Dashboard", icon: LayoutDashboard },
@@ -45,7 +69,7 @@ export function Shell() {
                 to={to}
                 params={{ env }}
                 activeOptions={{ exact: to === "/envs/$env" }}
-                className="flex items-center gap-2 rounded-md px-2 py-1.5 text-sm hover:bg-accent data-[status=active]:bg-accent data-[status=active]:font-medium"
+                className={LINK}
               >
                 <Icon className="size-4" />
                 {label}
@@ -53,6 +77,7 @@ export function Shell() {
             ))}
           </nav>
         )}
+        <AdminNav />
         <div className="mt-auto">
           <UserMenu />
         </div>
@@ -66,6 +91,22 @@ export function Shell() {
   );
 }
 
+function AdminNav() {
+  const { user } = useSession();
+  if (user.role !== "admin") return null;
+  return (
+    <nav className="grid gap-1" aria-label="Administration">
+      <div className="px-2 text-xs font-medium text-muted-foreground">Administration</div>
+      {ADMIN_NAV.map(({ to, label, icon: Icon }) => (
+        <Link key={to} to={to} className={LINK}>
+          <Icon className="size-4" />
+          {label}
+        </Link>
+      ))}
+    </nav>
+  );
+}
+
 function EnvLive({ env }: { env: string }) {
   useEnvEvents(env);
   return null;
```

- [ ] **Step 4: Run all checks**

Run: `cd web && pnpm test && pnpm typecheck && pnpm lint`
Expected: 175 passed (32 files); typecheck and lint are clean.

- [ ] **Step 5: Commit**

```bash
git add web/src
git commit -m "feat(web): admin pages for users and environments"
```

---

### Task 12: The audit log

**Files:**
- Create: `web/src/pages/admin/AuditPage.tsx`.
- Modify: `web/src/api/queries.ts`, `web/src/router.tsx` and `web/src/components/Shell.tsx`.
- Test: `web/src/pages/admin/AuditPage.test.tsx`.

**Interfaces:**
- Consumes: `adminRoute` and `keys.audit` (Task 11), `envsQuery`, and the SDK's `listAudit` (`limit` 1–500 and `before`, an id cursor, newest first).
- Produces `@/api/queries`:
  - `AUDIT_PAGE` (100).
  - `auditQuery`, an `infiniteQueryOptions`. The next page asks for entries `before` the last id, and there are no more pages after a short page.
- Produces: the route `/admin/audit`, and an `Audit log` item in `ADMIN_NAV`.

- [ ] **Step 1: Write the failing test**

`web/src/pages/admin/AuditPage.test.tsx`:

```tsx
import { waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import type { AuditEntry } from "@/api/gen/types.gen";
import { json, mockFetch } from "@/test/fetch";
import { loggedIn, renderApp } from "@/test/app";
import { AUDIT_PAGE } from "@/api/queries";

const entry = (id: number, over: Partial<AuditEntry> = {}): AuditEntry => ({
  id,
  ts: 1_790_000_000 + id,
  action: "container.stop",
  result: "success",
  user_id: 1,
  username: "admin",
  environment_id: 1,
  target_type: "container",
  target_id: `c${id}`,
  ...over,
});

const rows = (screen: typeof import("@testing-library/react").screen) =>
  within(screen.getByRole("table", { name: "Audit log" })).getAllByRole("row").slice(1);

describe("audit log", () => {
  it("lists entries with environment names, failures and details", async () => {
    mockFetch({
      ...loggedIn,
      "GET /api/v1/audit": {
        json: [
          entry(2, { action: "exec", result: "failure", detail: { cmd: ["sh"] } }),
          entry(1, { environment_id: null, action: "user.create", target_type: "user", target_id: "2" }),
        ],
      },
    });
    const { screen } = renderApp("/admin/audit");
    await waitFor(() => expect(rows(screen)).toHaveLength(2));
    const [exec, create] = rows(screen);
    expect(exec).toHaveTextContent("local");
    expect(within(exec).getByText("failure")).toBeInTheDocument();
    await userEvent.click(within(exec).getByText("show"));
    expect(exec).toHaveTextContent('"sh"');
    expect(create).toHaveTextContent("user 2");
    expect(screen.queryByRole("button", { name: "Load older entries" })).not.toBeInTheDocument();
  });

  it("loads older entries a page at a time", async () => {
    const calls = mockFetch({
      ...loggedIn,
      "GET /api/v1/audit": (c) => {
        const before = new URL(c.path, "http://x").searchParams.get("before");
        const top = before ? Number(before) - 1 : 150;
        const n = Math.min(AUDIT_PAGE, top);
        return json(200, Array.from({ length: n }, (_, i) => entry(top - i)));
      },
    });
    const { screen } = renderApp("/admin/audit");
    await waitFor(() => expect(rows(screen)).toHaveLength(AUDIT_PAGE));
    await userEvent.click(screen.getByRole("button", { name: "Load older entries" }));
    await waitFor(() => expect(rows(screen)).toHaveLength(150));
    expect(calls.filter((c) => c.path.startsWith("/api/v1/audit")).map((c) => c.path)).toEqual([
      "/api/v1/audit?limit=100",
      "/api/v1/audit?limit=100&before=51",
    ]);
    expect(screen.queryByRole("button", { name: "Load older entries" })).not.toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cd web && pnpm test src/pages/admin/AuditPage.test.tsx`
Expected: FAIL: 2 tests, with `Unable to find role="table" and name "Audit log"`.

- [ ] **Step 3: Implement**

`web/src/api/queries.ts`:

```diff
--- a/web/src/api/queries.ts
+++ b/web/src/api/queries.ts
@@ -1,4 +1,4 @@
-import { queryOptions } from "@tanstack/react-query";
+import { infiniteQueryOptions, queryOptions } from "@tanstack/react-query";
 import { ApiError, setCsrfToken } from "./client";
 import type { ContainerInspect, ImageInspect, ImageLayer, NetworkInspect, VolumeInspect } from "./docker";
 import {
@@ -14,6 +14,7 @@ import {
   inspectNetwork,
   inspectVolume,
   listContainerFiles,
+  listAudit,
   listComposeProjects,
   listContainers,
   listEnvs,
@@ -91,6 +92,17 @@ export const usersQuery = queryOptions({
   queryFn: async () => (await listUsers()).data,
 });
 
+/** Audit entries a page asks for at once. */
+export const AUDIT_PAGE = 100;
+
+/** The audit log, newest first, a page at a time: each page asks for entries older than the last one seen. */
+export const auditQuery = infiniteQueryOptions({
+  queryKey: keys.audit,
+  queryFn: async ({ pageParam }) => (await listAudit({ query: { limit: AUDIT_PAGE, before: pageParam } })).data,
+  initialPageParam: undefined as number | undefined,
+  getNextPageParam: (last) => (last.length < AUDIT_PAGE ? undefined : last[last.length - 1].id),
+});
+
 const path = (env: string) => ({ path: { env } });
 
 /** How often a down environment's status is checked, to notice it coming back. */
```

`web/src/pages/admin/AuditPage.tsx`:

```tsx
import { useInfiniteQuery, useQuery } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import { auditQuery, envsQuery } from "@/api/queries";
import type { AuditEntry } from "@/api/gen/types.gen";
import { columns, DataTable, type Column } from "@/components/DataTable";
import { Page } from "@/components/Page";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { formatTime } from "@/lib/format";

type Row = AuditEntry & { envName: string };
const col = columns<Row>();

const COLUMNS: Column<Row>[] = [
  col.accessor("ts", { header: "Time", cell: (c) => formatTime(c.getValue()) }),
  col.accessor((e) => e.username ?? (e.user_id == null ? "system" : `user ${e.user_id}`), { id: "user", header: "User" }),
  col.accessor("action", { header: "Action", cell: (c) => <code className="text-xs">{c.getValue()}</code> }),
  col.accessor("envName", { header: "Environment" }),
  col.accessor((e) => [e.target_type, e.target_id].filter(Boolean).join(" "), {
    id: "target",
    header: "Target",
    cell: (c) => <span className="block max-w-72 truncate">{c.getValue()}</span>,
  }),
  col.accessor("result", {
    header: "Result",
    cell: (c) =>
      c.getValue() === "success" ? (
        <Badge variant="secondary">success</Badge>
      ) : (
        <Badge variant="destructive">{c.getValue()}</Badge>
      ),
  }),
  col.accessor((e) => (e.detail == null ? "" : JSON.stringify(e.detail)), {
    id: "detail",
    header: "Detail",
    enableSorting: false,
    cell: (c) =>
      c.getValue() && (
        <details>
          <summary className="cursor-pointer text-muted-foreground">show</summary>
          <pre className="max-w-md font-mono text-xs whitespace-pre-wrap break-all">
            {JSON.stringify(c.row.original.detail, null, 2)}
          </pre>
        </details>
      ),
  }),
];

/** Every mutating Docker and admin action, newest first. */
export function AuditPage() {
  const audit = useInfiniteQuery(auditQuery);
  const envs = useQuery(envsQuery);
  const rows = useMemo(() => {
    if (!audit.data) return undefined;
    const names = new Map(envs.data?.map((e) => [e.id, e.name]));
    return audit.data.pages.flat().map((e) => ({
      ...e,
      envName: e.environment_id == null ? "" : (names.get(e.environment_id) ?? `#${e.environment_id}`),
    }));
  }, [audit.data, envs.data]);
  const [cols] = useState(() => COLUMNS);
  return (
    <Page
      title="Audit log"
      actions={
        <Button variant="outline" disabled={audit.isFetching} onClick={() => void audit.refetch()}>
          Refresh
        </Button>
      }
    >
      <DataTable
        label="Audit log"
        data={rows}
        error={audit.error}
        columns={cols}
        getRowId={(e) => String(e.id)}
        empty="Nothing has been audited yet."
      />
      {audit.hasNextPage && (
        <Button
          variant="outline"
          className="justify-self-start"
          disabled={audit.isFetchingNextPage}
          onClick={() => void audit.fetchNextPage()}
        >
          Load older entries
        </Button>
      )}
    </Page>
  );
}
```

`web/src/router.tsx`:

```diff
--- a/web/src/router.tsx
+++ b/web/src/router.tsx
@@ -212,6 +212,12 @@ const envsAdminRoute = createRoute({
   component: lazyRouteComponent(() => import("@/pages/admin/EnvironmentsPage"), "EnvironmentsPage"),
 });
 
+const auditRoute = createRoute({
+  getParentRoute: () => adminRoute,
+  path: "audit",
+  component: lazyRouteComponent(() => import("@/pages/admin/AuditPage"), "AuditPage"),
+});
+
 const accountRoute = createRoute({
   getParentRoute: () => shellRoute,
   path: "/account",
@@ -238,7 +244,7 @@ const routeTree = rootRoute.addChildren([
         networkRoute,
         composeRoute,
       ]),
-      adminRoute.addChildren([usersRoute, envsAdminRoute]),
+      adminRoute.addChildren([usersRoute, envsAdminRoute, auditRoute]),
       accountRoute,
     ]),
   ]),
```

`web/src/components/Shell.tsx`:

```diff
--- a/web/src/components/Shell.tsx
+++ b/web/src/components/Shell.tsx
@@ -11,6 +11,7 @@ import {
   Moon,
   Network,
   Rows3,
+  ScrollText,
   Server,
   Sun,
   User,
@@ -40,6 +41,7 @@ const LINK =
 const ADMIN_NAV = [
   { to: "/admin/users", label: "Users", icon: Users },
   { to: "/admin/envs", label: "Environments", icon: Server },
+  { to: "/admin/audit", label: "Audit log", icon: ScrollText },
 ] as const;
 
 /** The environment-scoped pages, in sidebar order. */
```

- [ ] **Step 4: Run all checks**

Run: `cd web && pnpm test && pnpm typecheck && pnpm lint`
Expected: 177 passed (33 files); typecheck and lint are clean.

- [ ] **Step 5: Commit**

```bash
git add web/src
git commit -m "feat(web): admin audit log with paging"
```

---

### Task 13: The command palette and keyboard shortcuts

**Files:**
- Create: `web/src/components/ui/command.tsx`, `web/src/components/CommandPalette.tsx` and `web/src/components/nav.ts`.
- Modify: `web/package.json` and `pnpm-lock.yaml` (via `pnpm add`), and `web/src/components/Shell.tsx` (the nav lists move to `nav.ts`).
- Test: `web/src/components/CommandPalette.test.tsx`.

**Interfaces:**
- Consumes:
  - the list queries for the current environment, fetched only while the palette is open;
  - `envsQuery`;
  - `ACTIONS` and `actionsFor` (Tasks 2 and 3), and `containerAction`;
  - `useSession`.
- Produces `@/components/nav`: `NAV` and `ADMIN_NAV`, shared by the sidebar and the palette.
- Produces `@/components/CommandPalette`:
  - `<CommandPalette env? />`, which renders the sidebar's `Search` button and the dialog `Command palette`.
  - `typing(e)`.
  - ⌘K / Ctrl-K toggles the palette.
  - `/` focuses `[data-filter]` (the `DataTable` filter), except while typing in a field.
- Produces the palette's items:
  - the environment pages;
  - the admin pages, for admins only;
  - Account;
  - every container, and Start, Stop or Restart for each container as its state allows;
  - images, volumes and networks;
  - "Switch to <env>".

- [ ] **Step 1: Add cmdk**

Run: `cd web && pnpm add cmdk@^1.1.1`
Expected: `+ cmdk 1.1.1`.

- [ ] **Step 2: Write the failing test**

`web/src/components/CommandPalette.test.tsx`:

```tsx
import { waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";
import { json, mockFetch } from "@/test/fetch";
import { ADMIN, ENVS, loggedIn, renderApp } from "@/test/app";
import { container, image, inspect } from "@/test/docker";

const routes = {
  ...loggedIn,
  "GET /api/v1/envs": { json: [...ENVS, { ...ENVS[0], id: 2, name: "staging" }] },
  "GET /api/v1/envs/local/containers": {
    json: [container({ id: "c1", name: "web" }), container({ id: "c2", name: "db", state: "exited" })],
  },
  "GET /api/v1/envs/local/images": { json: [image({ id: "sha256:img1", repo_tags: ["nginx:1.27"] })] },
  "GET /api/v1/envs/local/containers/c1": { json: inspect({ Id: "c1", Name: "/web" }) },
};

async function openPalette(screen: typeof import("@testing-library/react").screen) {
  await screen.findByRole("heading", { level: 1 });
  await userEvent.keyboard("{Control>}k{/Control}");
  return screen.findByRole("dialog", { name: "Command palette" });
}

describe("command palette", () => {
  it("opens with Ctrl-K and jumps to a container", async () => {
    mockFetch(routes);
    const { screen, history } = renderApp("/envs/local");
    const dialog = await openPalette(screen);
    await userEvent.type(within(dialog).getByRole("combobox"), "container web");
    await waitFor(() => expect(within(dialog).getAllByRole("option")[0]).toHaveTextContent("web"));
    await userEvent.keyboard("{Enter}");
    await waitFor(() => expect(history.location.pathname).toBe("/envs/local/containers/c1"));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("opens from the sidebar button and finds images", async () => {
    mockFetch(routes);
    const { screen, history } = renderApp("/envs/local");
    await userEvent.click(await screen.findByRole("button", { name: /Search/ }));
    const dialog = await screen.findByRole("dialog", { name: "Command palette" });
    await userEvent.click(await within(dialog).findByRole("option", { name: "nginx:1.27" }));
    await waitFor(() => expect(history.location.pathname).toBe("/envs/local/images/sha256%3Aimg1"));
  });

  it("runs an action that fits the container's state", async () => {
    const calls = mockFetch({ ...routes, "POST /api/v1/envs/local/containers/c2/start": { status: 204 } });
    const { screen } = renderApp("/envs/local");
    const dialog = await openPalette(screen);
    expect(await within(dialog).findByRole("option", { name: "Stop web" })).toBeInTheDocument();
    expect(within(dialog).queryByRole("option", { name: "Stop db" })).not.toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole("option", { name: "Start db" }));
    expect(await screen.findByText("Started db")).toBeInTheDocument();
    expect(calls.some((c) => c.method === "POST" && c.path.endsWith("/c2/start"))).toBe(true);
  });

  it("reports a failed action", async () => {
    mockFetch({
      ...routes,
      "POST /api/v1/envs/local/containers/c1/stop": () => json(409, { code: "conflict", message: "is already stopped" }),
    });
    const { screen } = renderApp("/envs/local");
    const dialog = await openPalette(screen);
    await userEvent.click(await within(dialog).findByRole("option", { name: "Stop web" }));
    expect(await screen.findByText("Stop web: is already stopped")).toBeInTheDocument();
  });

  it("switches environment and lists admin pages for admins only", async () => {
    mockFetch({ ...routes, "GET /api/v1/envs/staging/status": { json: { state: "down", reason: "x" } } });
    const { screen, history } = renderApp("/envs/local");
    const dialog = await openPalette(screen);
    expect(within(dialog).getByRole("option", { name: "Users" })).toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole("option", { name: "Switch to staging" }));
    await waitFor(() => expect(history.location.pathname).toBe("/envs/staging"));
  });

  it("hides admin pages from other users", async () => {
    mockFetch({ ...routes, "GET /api/v1/auth/me": { json: { ...ADMIN, user: { ...ADMIN.user, role: "user" } } } });
    const { screen } = renderApp("/envs/local");
    const dialog = await openPalette(screen);
    expect(within(dialog).getByRole("option", { name: "Containers" })).toBeInTheDocument();
    expect(within(dialog).queryByRole("option", { name: "Users" })).not.toBeInTheDocument();
  });
});

describe("shortcuts", () => {
  it("focuses the list filter with /, but not while typing", async () => {
    mockFetch(routes);
    const { screen } = renderApp("/envs/local/containers");
    const filter = await screen.findByLabelText("Filter containers");
    await userEvent.keyboard("/");
    expect(filter).toHaveFocus();
    await userEvent.keyboard("a/b");
    expect(filter).toHaveValue("a/b");
  });
});
```

- [ ] **Step 3: Run it to verify it fails**

Run: `cd web && pnpm test src/components/CommandPalette.test.tsx`
Expected: FAIL: all 7 tests, with `Unable to find role="dialog" and name "Command palette"`, `Unable to find role="button" and name /Search/` and, for `/`, `expect(element).toHaveFocus()`.

- [ ] **Step 4: Implement**

`web/src/components/ui/command.tsx` (shadcn/ui, vendored):

```tsx
"use client"

import * as React from "react"
import { Command as CommandPrimitive } from "cmdk"
import { SearchIcon } from "lucide-react"
import { cn } from "@/lib/utils"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"

function Command({ className, ...props }: React.ComponentProps<typeof CommandPrimitive>) {
  return (
    <CommandPrimitive
      data-slot="command"
      className={cn(
        "flex h-full w-full flex-col overflow-hidden rounded-md bg-popover text-popover-foreground",
        className
      )}
      {...props}
    />
  )
}

function CommandDialog({
  title = "Command Palette",
  description = "Search for a command to run...",
  children,
  className,
  showCloseButton = true,
  ...props
}: React.ComponentProps<typeof Dialog> & {
  title?: string
  description?: string
  className?: string
  showCloseButton?: boolean
}) {
  return (
    <Dialog {...props}>
      <DialogHeader className="sr-only">
        <DialogTitle>{title}</DialogTitle>
        <DialogDescription>{description}</DialogDescription>
      </DialogHeader>
      <DialogContent className={cn("overflow-hidden p-0", className)} showCloseButton={showCloseButton}>
        <Command className="**:data-[slot=command-input-wrapper]:h-12 [&_[cmdk-group-heading]]:px-2 [&_[cmdk-group-heading]]:font-medium [&_[cmdk-group-heading]]:text-muted-foreground [&_[cmdk-group]]:px-2 [&_[cmdk-group]:not([hidden])_~[cmdk-group]]:pt-0 [&_[cmdk-input-wrapper]_svg]:h-5 [&_[cmdk-input-wrapper]_svg]:w-5 [&_[cmdk-input]]:h-12 [&_[cmdk-item]]:px-2 [&_[cmdk-item]]:py-3 [&_[cmdk-item]_svg]:h-5 [&_[cmdk-item]_svg]:w-5">
          {children}
        </Command>
      </DialogContent>
    </Dialog>
  )
}

function CommandInput({ className, ...props }: React.ComponentProps<typeof CommandPrimitive.Input>) {
  return (
    <div data-slot="command-input-wrapper" className="flex h-9 items-center gap-2 border-b px-3">
      <SearchIcon className="size-4 shrink-0 opacity-50" />
      <CommandPrimitive.Input
        data-slot="command-input"
        className={cn(
          "flex h-10 w-full rounded-md bg-transparent py-3 text-sm outline-hidden placeholder:text-muted-foreground disabled:cursor-not-allowed disabled:opacity-50",
          className
        )}
        {...props}
      />
    </div>
  )
}

function CommandList({ className, ...props }: React.ComponentProps<typeof CommandPrimitive.List>) {
  return (
    <CommandPrimitive.List
      data-slot="command-list"
      className={cn("max-h-[300px] scroll-py-1 overflow-x-hidden overflow-y-auto", className)}
      {...props}
    />
  )
}

function CommandEmpty({ ...props }: React.ComponentProps<typeof CommandPrimitive.Empty>) {
  return <CommandPrimitive.Empty data-slot="command-empty" className="py-6 text-center text-sm" {...props} />
}

function CommandGroup({ className, ...props }: React.ComponentProps<typeof CommandPrimitive.Group>) {
  return (
    <CommandPrimitive.Group
      data-slot="command-group"
      className={cn(
        "overflow-hidden p-1 text-foreground [&_[cmdk-group-heading]]:px-2 [&_[cmdk-group-heading]]:py-1.5 [&_[cmdk-group-heading]]:text-xs [&_[cmdk-group-heading]]:font-medium [&_[cmdk-group-heading]]:text-muted-foreground",
        className
      )}
      {...props}
    />
  )
}

function CommandItem({ className, ...props }: React.ComponentProps<typeof CommandPrimitive.Item>) {
  return (
    <CommandPrimitive.Item
      data-slot="command-item"
      className={cn(
        "relative flex cursor-default items-center gap-2 rounded-sm px-2 py-1.5 text-sm outline-hidden select-none data-[disabled=true]:pointer-events-none data-[disabled=true]:opacity-50 data-[selected=true]:bg-accent data-[selected=true]:text-accent-foreground [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4 [&_svg:not([class*='text-'])]:text-muted-foreground",
        className
      )}
      {...props}
    />
  )
}

function CommandShortcut({ className, ...props }: React.ComponentProps<"span">) {
  return (
    <span
      data-slot="command-shortcut"
      className={cn("ml-auto text-xs tracking-widest text-muted-foreground", className)}
      {...props}
    />
  )
}

export { Command, CommandDialog, CommandEmpty, CommandGroup, CommandInput, CommandItem, CommandList, CommandShortcut }
```

`web/src/components/nav.ts`:

```ts
import { Boxes, Database, Layers, LayoutDashboard, Network, Rows3, ScrollText, Server, Users } from "lucide-react";

/** The environment-scoped pages, in sidebar order. */
export const NAV = [
  { to: "/envs/$env", label: "Dashboard", icon: LayoutDashboard },
  { to: "/envs/$env/containers", label: "Containers", icon: Boxes },
  { to: "/envs/$env/images", label: "Images", icon: Layers },
  { to: "/envs/$env/volumes", label: "Volumes", icon: Database },
  { to: "/envs/$env/networks", label: "Networks", icon: Network },
  { to: "/envs/$env/compose", label: "Compose", icon: Rows3 },
] as const;

/** Administration pages, shown to admins. */
export const ADMIN_NAV = [
  { to: "/admin/users", label: "Users", icon: Users },
  { to: "/admin/envs", label: "Environments", icon: Server },
  { to: "/admin/audit", label: "Audit log", icon: ScrollText },
] as const;
```

`web/src/components/CommandPalette.tsx`:

```tsx
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { Box, Database, Layers, Network, Play, RotateCw, Search, Server, Square } from "lucide-react";
import { useEffect, useState } from "react";
import { toast } from "sonner";
import { containersQuery, envsQuery, imagesQuery, keys, networksQuery, volumesQuery } from "@/api/queries";
import { containerAction } from "@/api/gen/sdk.gen";
import type { ContainerActionName, ContainerSummary } from "@/api/gen/types.gen";
import { useSession } from "@/auth/session";
import { Button } from "@/components/ui/button";
import {
  CommandDialog,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
  CommandShortcut,
} from "@/components/ui/command";
import { errorMessage } from "@/lib/errors";
import { shortId } from "@/lib/format";
import { ACTIONS, actionsFor } from "@/pages/containers/actions";
import { ADMIN_NAV, NAV } from "./nav";

const MAC = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.userAgent);

const ACTION_ICON: Partial<Record<ContainerActionName, typeof Play>> = { start: Play, stop: Square, restart: RotateCw };

/** Whether a key press happened in a text field, where shortcuts must not fire. */
export function typing(e: KeyboardEvent) {
  const t = e.target as HTMLElement | null;
  return !!t && (t.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(t.tagName));
}

/**
 * The sidebar's search button; it and ⌘K / Ctrl-K open the palette: jump to a page, a container, image,
 * volume or network of the current environment, or another environment,
 * or start, stop or restart a container. `/` focuses the page's filter.
 */
export function CommandPalette({ env }: { env?: string }) {
  const [open, setOpen] = useState(false);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setOpen((o) => !o);
      } else if (e.key === "/" && !typing(e) && !e.metaKey && !e.ctrlKey) {
        const filter = document.querySelector<HTMLInputElement>("[data-filter]");
        if (filter) {
          e.preventDefault();
          filter.focus();
        }
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
  return (
    <>
      <Button variant="outline" className="justify-between text-muted-foreground" onClick={() => setOpen(true)}>
        <span className="flex items-center gap-2">
          <Search className="size-4" />
          Search
        </span>
        <kbd className="text-xs">{MAC ? "⌘K" : "Ctrl K"}</kbd>
      </Button>
      <CommandDialog open={open} onOpenChange={setOpen} title="Command palette" description="Jump to a page or resource">
        {open && <PaletteBody env={env} close={() => setOpen(false)} />}
      </CommandDialog>
    </>
  );
}

function PaletteBody({ env, close }: { env?: string; close: () => void }) {
  const navigate = useNavigate();
  const qc = useQueryClient();
  const { user } = useSession();
  const envs = useQuery(envsQuery);
  const on = !!env;
  const e = env ?? "";
  const containers = useQuery({ ...containersQuery(e), enabled: on });
  const images = useQuery({ ...imagesQuery(e), enabled: on });
  const volumes = useQuery({ ...volumesQuery(e), enabled: on });
  const networks = useQuery({ ...networksQuery(e), enabled: on });

  /** Closes the palette, then navigates. */
  const go = (nav: () => Promise<void>) => {
    close();
    void nav();
  };
  const run = async (c: ContainerSummary, action: ContainerActionName) => {
    close();
    const a = ACTIONS.find((x) => x.action === action)!;
    try {
      await containerAction({ path: { env: e, id: c.id, action } });
      toast.success(`${a.done} ${c.name}`);
    } catch (err) {
      toast.error(`${a.label} ${c.name}: ${errorMessage(err)}`);
    } finally {
      void qc.invalidateQueries({ queryKey: keys.containers(e) });
    }
  };

  return (
    <>
      <CommandInput placeholder="Search pages, containers, images…" />
      <CommandList>
        <CommandEmpty>Nothing found.</CommandEmpty>
        <CommandGroup heading="Pages">
          {on &&
            NAV.map((n) => (
              <CommandItem key={n.to} value={`page ${n.label}`} onSelect={() => go(() => navigate({ to: n.to, params: { env: e } }))}>
                <n.icon />
                {n.label}
              </CommandItem>
            ))}
          {user.role === "admin" &&
            ADMIN_NAV.map((n) => (
              <CommandItem key={n.to} value={`page ${n.label}`} onSelect={() => go(() => navigate({ to: n.to }))}>
                <n.icon />
                {n.label}
              </CommandItem>
            ))}
          <CommandItem value="page Account" onSelect={() => go(() => navigate({ to: "/account" }))}>
            Account
          </CommandItem>
        </CommandGroup>
        {on && (
          <CommandGroup heading="Containers">
            {containers.data?.map((c) => (
              <CommandItem key={c.id} value={`container ${c.name} ${c.image}`} onSelect={() => go(() => navigate({ to: "/envs/$env/containers/$id", params: { env: e, id: c.id } }))}>
                <Box />
                {c.name}
                <CommandShortcut>{c.state}</CommandShortcut>
              </CommandItem>
            ))}
          </CommandGroup>
        )}
        {on && (
          <CommandGroup heading="Container actions">
            {containers.data?.flatMap((c) =>
              actionsFor(c.state)
                .filter((a) => ACTION_ICON[a])
                .map((a) => {
                  const Icon = ACTION_ICON[a]!;
                  const label = ACTIONS.find((x) => x.action === a)!.label;
                  return (
                    <CommandItem key={`${c.id}-${a}`} value={`${label} ${c.name}`} onSelect={() => void run(c, a)}>
                      <Icon />
                      {label} {c.name}
                    </CommandItem>
                  );
                }),
            )}
          </CommandGroup>
        )}
        {on && (
          <CommandGroup heading="Images">
            {images.data?.map((i) => (
              <CommandItem
                key={i.id}
                value={`image ${i.repo_tags.join(" ") || shortId(i.id)}`}
                onSelect={() => go(() => navigate({ to: "/envs/$env/images/$id", params: { env: e, id: i.id } }))}
              >
                <Layers />
                {i.repo_tags[0] ?? shortId(i.id)}
              </CommandItem>
            ))}
          </CommandGroup>
        )}
        {on && (
          <CommandGroup heading="Volumes">
            {volumes.data?.map((v) => (
              <CommandItem key={v.name} value={`volume ${v.name}`} onSelect={() => go(() => navigate({ to: "/envs/$env/volumes/$name", params: { env: e, name: v.name } }))}>
                <Database />
                {v.name}
              </CommandItem>
            ))}
          </CommandGroup>
        )}
        {on && (
          <CommandGroup heading="Networks">
            {networks.data?.map((n) => (
              <CommandItem key={n.id} value={`network ${n.name}`} onSelect={() => go(() => navigate({ to: "/envs/$env/networks/$id", params: { env: e, id: n.id } }))}>
                <Network />
                {n.name}
              </CommandItem>
            ))}
          </CommandGroup>
        )}
        <CommandGroup heading="Environments">
          {envs.data
            ?.filter((x) => x.name !== env)
            .map((x) => (
              <CommandItem key={x.id} value={`environment ${x.name}`} onSelect={() => go(() => navigate({ to: "/envs/$env", params: { env: x.name } }))}>
                <Server />
                Switch to {x.name}
              </CommandItem>
            ))}
        </CommandGroup>
      </CommandList>
    </>
  );
}
```

`web/src/components/Shell.tsx`:

```diff
--- a/web/src/components/Shell.tsx
+++ b/web/src/components/Shell.tsx
@@ -1,22 +1,6 @@
 import { useQuery } from "@tanstack/react-query";
 import { Link, Outlet, useNavigate, useParams } from "@tanstack/react-router";
-import {
-  Boxes,
-  ChevronsUpDown,
-  Database,
-  Layers,
-  LayoutDashboard,
-  LogOut,
-  Monitor,
-  Moon,
-  Network,
-  Rows3,
-  ScrollText,
-  Server,
-  Sun,
-  User,
-  Users,
-} from "lucide-react";
+import { ChevronsUpDown, LogOut, Monitor, Moon, Sun, User } from "lucide-react";
 import { envsQuery, envStatusQuery } from "@/api/queries";
 import { useLogout, useSession } from "@/auth/session";
 import { Badge } from "@/components/ui/badge";
@@ -32,28 +16,13 @@ import {
   DropdownMenuTrigger,
 } from "@/components/ui/dropdown-menu";
 import { useEnvEvents, useLive } from "@/live/LiveProvider";
+import { CommandPalette } from "./CommandPalette";
+import { ADMIN_NAV, NAV } from "./nav";
 import { useTheme, type Theme } from "@/theme";
 
 const LINK =
   "flex items-center gap-2 rounded-md px-2 py-1.5 text-sm hover:bg-accent data-[status=active]:bg-accent data-[status=active]:font-medium";
 
-/** Administration pages, shown to admins. */
-const ADMIN_NAV = [
-  { to: "/admin/users", label: "Users", icon: Users },
-  { to: "/admin/envs", label: "Environments", icon: Server },
-  { to: "/admin/audit", label: "Audit log", icon: ScrollText },
-] as const;
-
-/** The environment-scoped pages, in sidebar order. */
-const NAV = [
-  { to: "/envs/$env", label: "Dashboard", icon: LayoutDashboard },
-  { to: "/envs/$env/containers", label: "Containers", icon: Boxes },
-  { to: "/envs/$env/images", label: "Images", icon: Layers },
-  { to: "/envs/$env/volumes", label: "Volumes", icon: Database },
-  { to: "/envs/$env/networks", label: "Networks", icon: Network },
-  { to: "/envs/$env/compose", label: "Compose", icon: Rows3 },
-] as const;
-
 export function Shell() {
   const { env } = useParams({ strict: false });
   return (
@@ -63,6 +32,7 @@ export function Shell() {
           FleetOps
         </Link>
         <EnvSwitcher current={env} />
+        <CommandPalette env={env} />
         {env && (
           <nav className="grid gap-1" aria-label="Environment">
             {NAV.map(({ to, label, icon: Icon }) => (
```

- [ ] **Step 5: Run all checks**

Run: `make test-web && pnpm -C web build`
Expected:
- 184 passed (34 files); typecheck, lint and `check:api` are clean.
- The build emits one chunk per page (`ContainersPage-*.js`, `ImagePage-*.js`, `AuditPage-*.js`, …) and no chunk-size warning. The main `index-*.js` is under 400 kB.

- [ ] **Step 6: Check it against the real server**

With `make web && make dev-server` running:
- **Palette:**
  - Ctrl-K (⌘K on a Mac) opens it. Typing a container's name shows the container and its actions.
  - Enter opens it. The sidebar's Search button opens the palette too.
- **`/` shortcut:** on the containers page, `/` focuses the filter.
- **Admin pages:** Users, Environments and Audit log are reachable from the sidebar. The audit log lists the actions taken in Tasks 5 and 7.
- **Compose:** the compose page lists your compose projects with their containers. Do not press Down on a project you need.

- [ ] **Step 7: Commit**

```bash
git add web/package.json web/pnpm-lock.yaml web/src/components
git commit -m "feat(web): command palette with navigation and container actions, and the / shortcut"
```

---

## Deferred (tracked)

- **P3c:**
  - the log viewer, terminal and stats charts;
  - the containers list's CPU/memory sparklines (a `stats` subscription for the visible rows);
  - the container page's Stats, Logs and Terminal tabs (`CONTAINER_TABS` grows).
- **P3d:**
  - Playwright e2e, which can rely on the accessible names listed in each task's Interfaces;
  - `make image`;
  - CI.
- **Sub-project 3:** CodeMirror for the advanced create JSON and stack editing, and compose authoring.
- **P3a deferred minors still open:**
  - React StrictMode in dev calls `createApp` twice.
  - Concurrent 401s nest `redirect=`.
  - Password changes do not refresh the sessions list.
  - An unknown environment in the URL shows an empty dashboard.
  - The unused `cn` dependency.
  - The dashboard only shows `info` errors.
  - A failed logout bounces back into the app.
  - `safeRedirect` does not reject `/\`.
  - The StrictMode dev websocket warning.
- **P2c minors:** unchanged (see P3a's Deferred section).
