import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useState } from "react";
import { toast } from "sonner";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { columns, DataTable, type Column } from "@/components/DataTable";
import { Page } from "@/components/Page";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { useFeatures } from "@/ee-slots";
import { FormDialog } from "@/pages/admin/FormDialog";
import { RoleGrid } from "./RoleGrid";
import { builtinPermissions, catalogQuery, createRole, deleteRole, roleKeys, rolesQuery, updateRole, type Catalog, type RoleSummary } from "./roles";

/** A list row: a custom role, or one of the two built-ins (negative ids). */
type ListRow = RoleSummary & { builtin?: "viewer" | "operator" };

type Dialog =
  | { kind: "new"; from?: RoleSummary }
  | { kind: "edit" | "delete"; role: RoleSummary }
  | { kind: "builtin"; role: "viewer" | "operator" }
  | null;

const col = columns<ListRow>();

const BUILTINS: ListRow[] = [
  { id: -1, name: "Viewer", description: "Reads everything. Fixed.", permissions: [], users: 0, teams: 0, builtin: "viewer" },
  { id: -2, name: "Operator", description: "Does everything but reveal Secret values. Fixed.", permissions: [], users: 0, teams: 0, builtin: "operator" },
];

/** Admin → Roles: custom roles built on the permission grid. */
export function RolesPage() {
  const frozen = !useFeatures().includes("rbac");
  const qc = useQueryClient();
  const roles = useQuery(rolesQuery);
  const catalog = useQuery(catalogQuery);
  const [dialog, setDialog] = useState<Dialog>(null);
  const close = () => setDialog(null);
  const refresh = () => qc.invalidateQueries({ queryKey: roleKeys.all });
  const cols: Column<ListRow>[] = [
    col.accessor("name", {
      header: "Name",
      cell: (c) => (
        <span className="flex items-center gap-2 font-medium">
          {c.getValue()}
          {c.row.original.builtin && <span className="rounded border px-1 text-[10px] font-normal text-muted-foreground">Built-in</span>}
        </span>
      ),
    }),
    col.accessor("description", { header: "Description", cell: (c) => c.getValue() ?? "" }),
    col.accessor("users", { header: "Users", cell: (c) => (c.row.original.builtin ? "" : c.getValue()) }),
    col.accessor("teams", { header: "Teams", cell: (c) => (c.row.original.builtin ? "" : c.getValue()) }),
    col.display({
      id: "actions",
      header: () => <span className="sr-only">Actions</span>,
      cell: (c) => {
        const r = c.row.original;
        if (r.builtin) {
          return (
            <span className="flex justify-end">
              <Button variant="outline" size="sm" onClick={() => setDialog({ kind: "builtin", role: r.builtin! })}>
                View
              </Button>
            </span>
          );
        }
        return (
          <span className="flex justify-end gap-2">
            <Button variant="outline" size="sm" onClick={() => setDialog({ kind: "edit", role: r })}>
              Edit
            </Button>
            <Button variant="outline" size="sm" disabled={frozen} onClick={() => setDialog({ kind: "new", from: r })}>
              Duplicate
            </Button>
            <Button variant="outline" size="sm" onClick={() => setDialog({ kind: "delete", role: r })}>
              Delete
            </Button>
          </span>
        );
      },
    }),
  ];
  const doomed = dialog?.kind === "delete" ? dialog.role : null;
  return (
    <Page
      title="Roles"
      meta="Build a role from what it may do to each kind of resource, then give it to users and teams like Viewer or Operator."
      actions={
        <Button disabled={frozen} onClick={() => setDialog({ kind: "new" })}>
          New role
        </Button>
      }
    >
      {frozen && (
        <div role="status" className="mb-4 rounded-md border bg-muted/50 px-4 py-2 text-sm">
          Roles are read-only until a license is installed. People keep their access.{" "}
          <Link to={"/admin/license" as never} className="font-medium underline underline-offset-2">
            Manage license
          </Link>
        </div>
      )}
      <DataTable
        label="Roles"
        data={roles.data && [...BUILTINS, ...roles.data]}
        error={roles.error}
        columns={cols}
        getRowId={(r) => String(r.id)}
        empty="No roles yet."
      />
      {catalog.data && dialog?.kind === "new" && <RoleForm catalog={catalog.data} from={dialog.from} frozen={frozen} onClose={close} onDone={refresh} />}
      {catalog.data && dialog?.kind === "edit" && <RoleForm catalog={catalog.data} role={dialog.role} frozen={frozen} onClose={close} onDone={refresh} />}
      {catalog.data && dialog?.kind === "builtin" && <BuiltinDialog catalog={catalog.data} role={dialog.role} onClose={close} />}
      <ConfirmDialog
        open={doomed !== null}
        onOpenChange={(open) => !open && close()}
        title={doomed ? `Delete role ${doomed.name}?` : ""}
        description={
          doomed
            ? `${doomed.users} user${doomed.users === 1 ? "" : "s"} and ${doomed.teams} team${doomed.teams === 1 ? "" : "s"} lose the access it gives.`
            : ""
        }
        confirmLabel="Delete role"
        destructive
        onConfirm={async () => {
          if (!doomed) return;
          await deleteRole(doomed.id);
          await refresh();
          toast.success(`Deleted role ${doomed.name}`);
          close();
        }}
      />
    </Page>
  );
}

/** New (optionally copying `from`) or, with `role`, edit. Frozen, an edit can only take permissions away. */
function RoleForm({
  catalog,
  role,
  from,
  frozen,
  onClose,
  onDone,
}: {
  catalog: Catalog;
  role?: RoleSummary;
  from?: RoleSummary;
  frozen: boolean;
  onClose: () => void;
  onDone: () => Promise<unknown>;
}) {
  const base = role ?? from;
  const [name, setName] = useState(role?.name ?? (from ? `${from.name} copy` : ""));
  const [description, setDescription] = useState(base?.description ?? "");
  const [permissions, setPermissions] = useState<string[]>(base?.permissions ?? []);
  return (
    <FormDialog
      title={role ? `Edit ${role.name}` : "New role"}
      submitLabel={role ? "Save" : "Create"}
      onClose={onClose}
      onSubmit={async () => {
        const d = description.trim() || null;
        if (!role) await createRole(name, d, permissions);
        else if (frozen) await updateRole(role.id, { permissions });
        else await updateRole(role.id, { name, description: d, permissions });
        await onDone();
      }}
    >
      {frozen && role && <p className="text-sm text-muted-foreground">Without a license you can take permissions away, not add them.</p>}
      <div className="grid gap-2">
        <Label htmlFor="role-name">Name</Label>
        <Input id="role-name" value={name} onChange={(e) => setName(e.target.value)} required maxLength={64} disabled={frozen} autoFocus />
      </div>
      <div className="grid gap-2">
        <Label htmlFor="role-description">Description</Label>
        <Input id="role-description" value={description} onChange={(e) => setDescription(e.target.value)} maxLength={256} disabled={frozen} />
      </div>
      <RoleGrid catalog={catalog} value={permissions} onChange={setPermissions} limit={frozen && role ? role.permissions : undefined} />
    </FormDialog>
  );
}

function BuiltinDialog({ catalog, role, onClose }: { catalog: Catalog; role: "viewer" | "operator"; onClose: () => void }) {
  const label = role === "viewer" ? "Viewer" : "Operator";
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{label}</DialogTitle>
          <DialogDescription>A built-in role. It can't be changed; build a custom role for anything in between.</DialogDescription>
        </DialogHeader>
        <RoleGrid catalog={catalog} value={builtinPermissions(role, catalog)} readOnly />
      </DialogContent>
    </Dialog>
  );
}
