import { useQuery } from "@tanstack/react-query";
import type { SlotProps } from "@/ee-contract";
import { envTeamsQuery, userEffectiveAccessQuery } from "./teams";

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
          <li key={e.env_id} className="flex flex-wrap items-baseline justify-between gap-2 rounded-md bg-raised px-3 py-1.5">
            <span>
              <span className="font-medium">{e.env_name}</span> · {e.role}
              {e.namespaces && <span className="font-mono text-xs"> {e.namespaces.join(", ")}</span>}
            </span>
            <span className="text-muted-foreground">
              {e.sources
                .filter((s) => s.team_name !== null)
                .map((s) => (s.role_name ? `${s.team_name} (${s.role_name})` : s.team_name))
                .join(", ")}
            </span>
          </li>
        ))}
      </ul>
      <p className="text-xs text-muted-foreground">Change team access under Admin → Teams.</p>
    </section>
  );
}

/** In an environment's Access dialog: the teams with access, and their members. */
export function EnvTeamAccess({ env }: SlotProps["env.access"]) {
  const { data } = useQuery(envTeamsQuery(env));
  if (!data || data.length === 0) return null;
  return (
    <section aria-labelledby="env-through-teams" className="grid gap-2">
      <h3 id="env-through-teams" className="text-sm font-medium">
        Through teams
      </h3>
      <ul className="grid gap-1 text-sm">
        {data.map((t) => (
          <li key={t.team_id} className="grid gap-0.5 rounded-md bg-raised px-3 py-1.5">
            <span>
              <span className="font-medium">{t.team_name}</span> · {t.role_name ?? t.role}
              <span className="font-mono text-xs"> {t.namespaces ? t.namespaces.join(", ") : "Entire environment"}</span>
            </span>
            <span className="text-muted-foreground">{t.members.length ? t.members.join(", ") : "No members"}</span>
          </li>
        ))}
      </ul>
    </section>
  );
}
