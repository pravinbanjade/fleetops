import { useQuery } from "@tanstack/react-query";
import type { Feature } from "@/api/gen/types.gen";
import { editionQuery, meQuery } from "@/api/queries";
import { scheduleQuery, targetsQuery } from "./backup";
import { destinationsQuery } from "./notifications";
import { rolesQuery } from "./roles";
import { connectionsQuery } from "./sso";
import { teamsQuery } from "./teams";

/**
 * Enterprise data left from a lapsed license, by feature, so its pages stay
 * visible on Community. Asked only by admins on Community; a failed check
 * counts as nothing left.
 */
export function useLeftovers(): Partial<Record<Feature, boolean>> {
  const { data: me } = useQuery(meQuery);
  const { data: edition } = useQuery(editionQuery);
  const enabled = me?.user.role === "admin" && edition?.edition === "community";
  const opts = { enabled, retry: false } as const;
  const teams = useQuery({ ...teamsQuery, ...opts }).data;
  const roles = useQuery({ ...rolesQuery, ...opts }).data;
  const sso = useQuery({ ...connectionsQuery, ...opts }).data;
  const schedule = useQuery({ ...scheduleQuery, ...opts }).data;
  const targets = useQuery({ ...targetsQuery, ...opts }).data;
  const destinations = useQuery({ ...destinationsQuery, ...opts }).data;
  if (!enabled) return {};
  return {
    teams: (teams?.length ?? 0) > 0,
    rbac: (roles?.length ?? 0) > 0,
    sso: (sso?.length ?? 0) > 0,
    backup: schedule?.enabled === true || (targets?.length ?? 0) > 0,
    notifications: (destinations?.length ?? 0) > 0,
  };
}
