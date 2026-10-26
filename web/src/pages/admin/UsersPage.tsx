import { useQuery, useQueryClient } from "@tanstack/react-query";
import { KeyRound, ShieldCheck, Trash2, UserCheck, UserCog, UserX } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import { keys, usersQuery } from "@/api/queries";
import { createUser, deleteUser, updateUser } from "@/api/gen/sdk.gen";
import type { Role, User } from "@/api/gen/types.gen";
import { useSession } from "@/auth/session";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { columns, DataTable, type Column } from "@/components/DataTable";
import { Page } from "@/components/Page";
import { QuickButtons, type QuickItem } from "@/components/RowMenu";
import { StateBadge } from "@/components/StateBadge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { NativeSelect } from "@/components/ui/native-select";
import { toastError } from "@/lib/errors";
import { formatTime } from "@/lib/format";
import { AccessDialog, BulkAccessDialog } from "./AccessDialog";
import { FormDialog } from "./FormDialog";
import { UserAvatar } from "@/components/UserAvatar";

type Dialog =
  | { kind: "add" }
  | { kind: "password" | "role" | "delete" | "access" | "disable"; user: User }
  | { kind: "bulk"; users: User[] }
  | null;

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
          {c.row.original.disabled_at != null && (
            <span title={`Disabled ${formatTime(c.row.original.disabled_at)}`}>
              <StateBadge state="disabled" tone="fail" />
            </span>
          )}
          {c.row.original.sso && (
            <span className="rounded border px-1.5 py-0.5 text-xs font-normal text-muted-foreground">
              Signed in with {c.row.original.sso}
            </span>
          )}
        </span>
      ),
    }),
    col.accessor("role", { header: "Role", cell: (c) => (c.getValue() === "admin" ? "Admin" : "User") }),
    col.accessor("created_at", { header: "Created", cell: (c) => formatTime(c.getValue()) }),
    col.display({
      id: "access",
      header: () => <span className="sr-only">Access</span>,
      cell: (c) =>
        c.row.original.role === "admin" ? (
          <span className="text-xs text-muted-foreground">Every environment</span>
        ) : (
          <Button variant="outline" size="sm" onClick={() => setDialog({ kind: "access", user: c.row.original })}>
            <ShieldCheck aria-hidden />
            Access
          </Button>
        ),
    }),
  ]);
  /** What can be done to one user, from its ⋮ menu; SSO users' role and password belong to the provider. */
  const menu = (u: User): QuickItem[] => [
    ...(u.sso
      ? []
      : [
          { id: "role", label: "Change role", icon: UserCog, onSelect: () => setDialog({ kind: "role", user: u }) },
          { id: "password", label: "Reset password", icon: KeyRound, onSelect: () => setDialog({ kind: "password", user: u }) },
        ]),
    ...(u.id === me.id
      ? []
      : u.disabled_at != null
        ? [{ id: "enable", label: "Enable", icon: UserCheck, onSelect: () => void enable(u) }]
        : [{ id: "disable", label: "Disable and sign out", icon: UserX, danger: true, onSelect: () => setDialog({ kind: "disable", user: u }) }]),
    { id: "delete", label: "Delete", icon: Trash2, danger: true, onSelect: () => setDialog({ kind: "delete", user: u }) },
  ];
  const enable = async (u: User) => {
    try {
      await updateUser({ path: { id: u.id }, body: { disabled: false }, throwOnError: true });
      toast.success(`${u.username} can sign in again`);
      await refresh();
    } catch (e) {
      toastError(e);
    }
  };
  /** The selection bar: access for the users who aren't admins (admins have it everywhere). */
  const bulk = (selected: User[]): QuickItem[] => {
    const users = selected.filter((u) => u.role !== "admin");
    return [
      {
        id: "access",
        label: "Access",
        icon: ShieldCheck,
        count: users.length,
        disabled: users.length === 0,
        onSelect: () => setDialog({ kind: "bulk", users }),
      },
    ];
  };
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
        phoneColumns={["username", "access"]}
        selectable
        actions={(selected) => <QuickButtons items={bulk(selected)} />}
        rowMenu={{ label: (u) => u.username, items: menu }}
      />
      {dialog?.kind === "bulk" && <BulkAccessDialog users={dialog.users} onClose={close} />}
      {dialog?.kind === "add" && <AddUser onClose={close} onDone={refresh} />}
      {dialog?.kind === "access" && <AccessDialog user={dialog.user} onClose={close} />}
      {dialog?.kind === "role" && <ChangeRole user={dialog.user} self={dialog.user.id === me.id} onClose={close} onDone={refresh} />}
      {dialog?.kind === "password" && <ResetPassword user={dialog.user} self={dialog.user.id === me.id} onClose={close} />}
      {dialog?.kind === "disable" && (
        <ConfirmDialog
          open
          onOpenChange={(open) => !open && close()}
          title={`Disable ${dialog.user.username}?`}
          description="They're signed out everywhere now, and can't sign in with a password, a directory or single sign-on until you enable the account again. Their access, teams and audit history are kept."
          confirmLabel="Disable and sign out"
          onConfirm={async () => {
            await updateUser({ path: { id: dialog.user.id }, body: { disabled: true }, throwOnError: true });
            toast.success(`${dialog.user.username} is disabled and signed out`);
            await refresh();
          }}
        />
      )}
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

function ChangeRole({
  user,
  self,
  onClose,
  onDone,
}: {
  user: User;
  self: boolean;
  onClose: () => void;
  onDone: () => Promise<unknown>;
}) {
  const [role, setRole] = useState<Role>(user.role);
  const demotingSelf = self && user.role === "admin" && role !== "admin";
  return (
    <FormDialog
      title={`Change role of ${user.username}`}
      submitLabel={demotingSelf ? "Give up admin" : "Save"}
      onClose={onClose}
      onSubmit={async () => {
        await updateUser({ path: { id: user.id }, body: { role } });
        await onDone();
        if (role !== user.role) toast.success(`${user.username} is now ${role === "admin" ? "an admin" : "a user"}`);
      }}
    >
      <RoleSelect value={role} onChange={setRole} />
      {demotingSelf && (
        <p role="alert" className="text-sm text-state-fail-text">
          You lose access to Admin as soon as you save. Only another admin can give it back.
        </p>
      )}
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
        <option value="user">User: works in the environments granted under Access</option>
        <option value="admin">Admin: also manages users and environments</option>
      </NativeSelect>
    </div>
  );
}
