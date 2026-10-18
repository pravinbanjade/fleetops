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
import { AccessDialog } from "./AccessDialog";
import { FormDialog } from "./FormDialog";
import { UserAvatar } from "@/components/UserAvatar";

type Dialog = { kind: "add" } | { kind: "password" | "role" | "delete" | "access"; user: User } | null;

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
        <span className="flex items-center gap-2.5 font-medium">
          <UserAvatar name={c.getValue()} />
          {c.getValue()}
          {c.row.original.id === me.id && <span className="ml-2 text-xs text-muted-foreground">(you)</span>}
          {c.row.original.sso && (
            <span className="rounded border px-1.5 py-0.5 text-xs font-normal text-muted-foreground">
              Signed in with {c.row.original.sso}
            </span>
          )}
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
          {c.row.original.role !== "admin" && (
            <Button variant="outline" size="sm" onClick={() => setDialog({ kind: "access", user: c.row.original })}>
              Access
            </Button>
          )}
          {c.row.original.sso ? (
            <span className="self-center text-xs text-muted-foreground">Role managed by single sign-on</span>
          ) : (
            <>
              <Button variant="outline" size="sm" onClick={() => setDialog({ kind: "role", user: c.row.original })}>
                Change role
              </Button>
              <Button variant="outline" size="sm" onClick={() => setDialog({ kind: "password", user: c.row.original })}>
                Reset password
              </Button>
            </>
          )}
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
      {dialog?.kind === "access" && <AccessDialog user={dialog.user} onClose={close} />}
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
        <option value="user">user: works in the environments granted under Access</option>
        <option value="admin">admin: also manages users and environments</option>
      </NativeSelect>
    </div>
  );
}
