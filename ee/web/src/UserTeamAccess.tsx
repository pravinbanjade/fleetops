import { useQuery } from "@tanstack/react-query";
import type { SlotProps } from "@/ee-contract";
import { userEffectiveAccessQuery } from "./teams";

/** In a user's Access dialog: what they get through teams, and from which. */
export function UserTeamAccess({ userId }: SlotProps["user.access"]) {
  const { data } = useQuery(userEffectiveAccessQuery(userId));
  const viaTeams = (data ?? []).filter((e) => e.sources.some((s) => s.team_id !== null));
  if (viaTeams.length === 0) return null;
  return (
    <section aria-labelledby="through-teams" className="grid gap-2">
      <h3 id="through-teams" className="text-sm font-medium">
        Through teams
      </h3>
      <ul className="grid gap-1 text-sm">
        {viaTeams.map((e) => (
          <li key={e.env_id} className="flex flex-wrap items-baseline justify-between gap-2 rounded-md border px-3 py-1.5">
            <span>
              <span className="font-medium">{e.env_name}</span> · {e.role}
              {e.namespaces && <span className="font-mono text-xs"> {e.namespaces.join(", ")}</span>}
            </span>
            <span className="text-muted-foreground">
              {e.sources
                .filter((s) => s.team_name !== null)
                .map((s) => s.team_name)
                .join(", ")}
            </span>
          </li>
        ))}
      </ul>
      <p className="text-xs text-muted-foreground">Change team access under Admin → Teams.</p>
    </section>
  );
}
