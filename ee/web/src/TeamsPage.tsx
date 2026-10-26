import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Pencil, ShieldCheck, Trash2, Users } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";
import type { EnvView } from "@/api/gen/types.gen";
import { envsQuery, usersQuery } from "@/api/queries";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { columns, DataTable, type Column } from "@/components/DataTable";
import { Page } from "@/components/Page";
import type { QuickItem } from "@/components/RowMenu";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { NativeSelect } from "@/components/ui/native-select";
import { useFeatures } from "@/ee-slots";
import { FrozenNotice } from "./FrozenNotice";
import { errorMessage } from "@/lib/errors";
import { accessBody, EnvRow, fromEntry, NONE, same, type Row } from "@/pages/admin/EnvAccessRow";
import { FormDialog } from "@/pages/admin/FormDialog";
import {
  addTeamMember,
  createTeam,
  deleteTeam,
  removeTeamMember,
  revokeTeamAccess,
  setTeamAccess,
  teamKeys,
  teamQuery,
  teamsQuery,
  updateTeam,
  type TeamDetail,
  type TeamSummary,
} from "./teams";

type Dialog = { kind: "new" } | { kind: "members" | "access" | "rename" | "delete"; team: TeamSummary } | null;

const col = columns<TeamSummary>();

/** Admin → Teams: groups of users that carry environment access. */
export function TeamsPage() {
  const frozen = !useFeatures().includes("teams");
  const qc = useQueryClient();
  const teams = useQuery(teamsQuery);
  const [dialog, setDialog] = useState<Dialog>(null);
  const close = () => setDialog(null);
  const refresh = () => qc.invalidateQueries({ queryKey: teamKeys.all });
  const cols: Column<TeamSummary>[] = [
    col.accessor("name", { header: "Name", cell: (c) => <span className="font-medium">{c.getValue()}</span> }),
    col.accessor("description", { header: "Description", cell: (c) => c.getValue() ?? "" }),
    col.accessor("members", { header: "Members" }),
    col.accessor("environments", { header: "Environments" }),
    col.display({
      id: "open",
      header: () => <span className="sr-only">Members</span>,
      cell: (c) => (
        <Button variant="outline" size="sm" onClick={() => setDialog({ kind: "members", team: c.row.original })}>
          <Users aria-hidden />
          Members
        </Button>
      ),
    }),
  ];
  const menu = (t: TeamSummary): QuickItem[] => [
    { id: "access", label: "Environment access", icon: ShieldCheck, onSelect: () => setDialog({ kind: "access", team: t }) },
    { id: "rename", label: "Rename", icon: Pencil, disabled: frozen, onSelect: () => setDialog({ kind: "rename", team: t }) },
    { id: "delete", label: "Delete", icon: Trash2, danger: true, onSelect: () => setDialog({ kind: "delete", team: t }) },
  ];
  return (
    <Page
      title="Teams"
      meta="Members of a team get its environment access."
      actions={
        <Button disabled={frozen} onClick={() => setDialog({ kind: "new" })}>
          New team
        </Button>
      }
    >
      {frozen && (
        <FrozenNotice>Teams are read-only until a license is installed. Members keep their access.</FrozenNotice>
      )}
      <DataTable
        label="Teams"
        data={teams.data}
        error={teams.error}
        columns={cols}
        getRowId={(t) => String(t.id)}
        empty="No teams yet."
        initialSorting={[{ id: "name", desc: false }]}
        phoneColumns={["name", "members", "open"]}
        rowMenu={{ label: (t) => t.name, items: menu }}
      />
      {dialog?.kind === "new" && <TeamForm onClose={close} onDone={refresh} />}
      {dialog?.kind === "rename" && <TeamForm team={dialog.team} onClose={close} onDone={refresh} />}
      {dialog?.kind === "members" && <MembersDialog team={dialog.team} frozen={frozen} onClose={close} />}
      {dialog?.kind === "access" && <TeamAccessDialog team={dialog.team} frozen={frozen} onClose={close} />}
      <ConfirmDialog
        open={dialog?.kind === "delete"}
        onOpenChange={(open) => !open && close()}
        title={dialog?.kind === "delete" ? `Delete team ${dialog.team.name}?` : ""}
        description="Its members lose the access the team gave them. Their own grants and other teams are unaffected."
        confirmLabel="Delete team"
        destructive
        onConfirm={async () => {
          if (dialog?.kind !== "delete") return;
          await deleteTeam(dialog.team.id);
          await Promise.all([refresh(), qc.invalidateQueries({ queryKey: teamKeys.effective })]);
          toast.success(`Deleted team ${dialog.team.name}`);
          close();
        }}
      />
    </Page>
  );
}

/** Create a team, or rename one when `team` is given. */
function TeamForm({ team, onClose, onDone }: { team?: TeamSummary; onClose: () => void; onDone: () => Promise<unknown> }) {
  const [name, setName] = useState(team?.name ?? "");
  const [description, setDescription] = useState(team?.description ?? "");
  return (
    <FormDialog
      title={team ? `Rename ${team.name}` : "New team"}
      submitLabel={team ? "Save" : "Create"}
      onClose={onClose}
      onSubmit={async () => {
        const d = description.trim() || null;
        if (team) await updateTeam(team.id, name, d);
        else await createTeam(name, d);
        await onDone();
      }}
    >
      <div className="grid gap-2">
        <Label htmlFor="team-name">Name</Label>
        <Input id="team-name" value={name} onChange={(e) => setName(e.target.value)} required maxLength={64} autoFocus />
      </div>
      <div className="grid gap-2">
        <Label htmlFor="team-description">Description</Label>
        <Input id="team-description" value={description} onChange={(e) => setDescription(e.target.value)} maxLength={256} />
      </div>
    </FormDialog>
  );
}

function useTeamChanged(id: number) {
  const qc = useQueryClient();
  return () =>
    Promise.all([
      qc.invalidateQueries({ queryKey: teamKeys.all }),
      qc.invalidateQueries({ queryKey: teamKeys.team(id) }),
      qc.invalidateQueries({ queryKey: teamKeys.effective }),
    ]);
}

/** Who is in the team; adding is frozen without a license, removing never is. */
function MembersDialog({ team, frozen, onClose }: { team: TeamSummary; frozen: boolean; onClose: () => void }) {
  const detail = useQuery(teamQuery(team.id));
  const users = useQuery(usersQuery);
  const changed = useTeamChanged(team.id);
  const [pick, setPick] = useState("");
  const [busy, setBusy] = useState(false);
  const run = async (f: () => Promise<unknown>) => {
    setBusy(true);
    try {
      await f();
      await changed();
    } catch (e) {
      toast.error(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };
  const [leaving, setLeaving] = useState<{ id: number; username: string } | null>(null);
  // Licensed, a removal is undone by adding back; frozen it can't be, so it asks first.
  const removeMember = (m: { id: number; username: string }) => {
    if (frozen) return setLeaving(m);
    void run(async () => {
      await removeTeamMember(team.id, m.id);
      toast.success(`Removed ${m.username} from ${team.name}`, {
        action: { label: "Undo", onClick: () => void run(() => addTeamMember(team.id, m.id)) },
      });
    });
  };
  const members = detail.data?.members ?? [];
  const candidates = (users.data ?? []).filter((u) => !members.some((m) => m.id === u.id));
  const chosen = candidates.find((u) => u.username === pick);
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Members of {team.name}</DialogTitle>
          <DialogDescription>Members get the team's environment access. Admins have full access already.</DialogDescription>
        </DialogHeader>
        <div className="flex gap-2">
          <NativeSelect aria-label="User to add" value={pick} onChange={(e) => setPick(e.target.value)} disabled={frozen} className="flex-1">
            <option value="">Choose a user…</option>
            {candidates.map((u) => (
              <option key={u.id} value={u.username}>
                {u.username}
              </option>
            ))}
          </NativeSelect>
          <Button
            disabled={frozen || busy || !chosen}
            onClick={() =>
              chosen &&
              run(async () => {
                await addTeamMember(team.id, chosen.id);
                setPick("");
              })
            }
          >
            Add
          </Button>
        </div>
        {detail.data && members.length === 0 && <p className="text-sm text-muted-foreground">No members yet.</p>}
        <ul className="grid gap-1">
          {members.map((m) => {
            const via = detail.data?.synced.find((s) => s.user_id === m.id)?.connection;
            return (
              <li key={m.id} className="flex items-center justify-between rounded-md border px-3 py-1.5 text-sm">
                <span>
                  {m.username}
                  {via && <span className="ml-2 text-xs text-muted-foreground">via {via}</span>}
                </span>
                {via ? (
                  <span className="text-xs text-muted-foreground">Synced: change the group mapping to remove</span>
                ) : (
                  <Button
                    variant="outline"
                    size="sm"
                    aria-label={`Remove ${m.username}`}
                    disabled={busy}
                    onClick={() => removeMember(m)}
                  >
                    Remove
                  </Button>
                )}
              </li>
            );
          })}
        </ul>
      </DialogContent>
      <ConfirmDialog
        open={!!leaving}
        onOpenChange={(o) => !o && setLeaving(null)}
        title={`Remove ${leaving?.username} from ${team.name}?`}
        description="They lose the team's environment access. Without a license they can't be added back until one is installed."
        confirmLabel="Remove member"
        onConfirm={async () => {
          if (leaving) await removeTeamMember(team.id, leaving.id);
          await changed();
        }}
      />
    </Dialog>
  );
}

/** The team's role in each environment; saves only the rows that changed. */
function TeamAccessDialog({ team, frozen, onClose }: { team: TeamSummary; frozen: boolean; onClose: () => void }) {
  const envs = useQuery(envsQuery);
  const detail = useQuery(teamQuery(team.id));
  if (!envs.data || !detail.data) return null;
  return <TeamAccessForm team={detail.data} envs={envs.data} frozen={frozen} onClose={onClose} />;
}

function TeamAccessForm({ team, envs, frozen, onClose }: { team: TeamDetail; envs: EnvView[]; frozen: boolean; onClose: () => void }) {
  const changed = useTeamChanged(team.id);
  const initial: Record<number, Row> = Object.fromEntries(envs.map((e) => [e.id, fromEntry(team.access.find((a) => a.env_id === e.id))]));
  const [rows, setRows] = useState<Record<number, Row>>(initial);
  return (
    <FormDialog
      title={`Access for ${team.name}`}
      description="Members get this access. Each of a member's grants (direct or through teams) is checked on its own: Operator on one namespace stays Operator on that namespace only."
      submitLabel="Save"
      onClose={onClose}
      onSubmit={async () => {
        for (const e of envs) {
          const row = rows[e.id];
          if (row.namespaces && row.namespaces.length === 0) {
            throw new Error(`Add at least one namespace on ${e.name}, or choose Entire cluster`);
          }
        }
        for (const e of envs) {
          const row = rows[e.id];
          if (same(row, initial[e.id])) continue;
          if (row.role === "none") await revokeTeamAccess(team.id, e.id);
          else await setTeamAccess(team.id, e.id, accessBody(row));
        }
        await changed();
        toast.success(`Saved access for ${team.name}`);
      }}
    >
      {frozen && <p className="text-sm text-muted-foreground">Without a license you can lower or remove access, not add it.</p>}
      {envs.length === 0 && <p className="text-sm text-muted-foreground">No environments yet.</p>}
      <div className="grid gap-3">
        {envs.map((e) => (
          <EnvRow
            key={e.id}
            env={e}
            row={rows[e.id]}
            limit={frozen ? (initial[e.id] ?? NONE) : undefined}
            onChange={(r) => setRows((all) => ({ ...all, [e.id]: r }))}
          />
        ))}
      </div>
    </FormDialog>
  );
}
