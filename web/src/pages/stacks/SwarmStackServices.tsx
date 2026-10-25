import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { stackServicesQuery } from "@/api/queries";
import { Badge } from "@/components/ui/badge";
import { errorMessage } from "@/lib/errors";
import { useSwarmEvents } from "@/live/swarmEvents";

/** A Swarm stack's services: how many tasks run, and what changed on them outside the stack. */
export function SwarmStackServices({ env, stack }: { env: string; stack: string }) {
  useSwarmEvents(env);
  const q = useQuery(stackServicesQuery(env, stack));
  if (q.error)
    return (
      <p role="alert" className="text-sm text-destructive">
        {errorMessage(q.error)}
      </p>
    );
  if (!q.data) return <p className="text-sm text-muted-foreground">Loading…</p>;
  if (q.data.length === 0) return <p className="text-sm text-muted-foreground">No services.</p>;
  return (
    <table aria-label="Services" className="w-full text-sm">
      <thead className="text-left text-muted-foreground">
        <tr>
          <th className="py-1 font-medium">Service</th>
          <th className="py-1 font-medium">Tasks</th>
          <th className="py-1 font-medium">Update</th>
          <th className="py-1 font-medium" />
        </tr>
      </thead>
      <tbody>
        {q.data.map((s) => (
          <tr key={s.key} className="border-t">
            <td className="py-2">
              <Link
                to="/envs/$env/swarm/services/$id"
                params={{ env, id: s.service.id }}
                className="font-ident font-medium hover:underline"
              >
                {s.key}
              </Link>
            </td>
            <td className="py-2">
              {s.service.tasks_running}/{s.service.tasks_desired} running
            </td>
            <td className="py-2 text-muted-foreground">{s.service.update_state ?? ""}</td>
            <td className="py-2">
              <span className="flex gap-2">
                {!s.managed && <Badge variant="outline">Not deployed by FleetOps</Badge>}
                {s.drift.length > 0 && (
                  <Badge variant="outline" title={`Changed outside the stack: ${s.drift.join(", ")}; the next deploy overwrites it`}>
                    Drift
                  </Badge>
                )}
              </span>
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}
