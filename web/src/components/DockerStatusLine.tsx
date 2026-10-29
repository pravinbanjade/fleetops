import { useQuery } from "@tanstack/react-query";
import type { ContainerSummary } from "@/api/gen/types.gen";
import { recentEventsQuery } from "@/api/queries";
import { dockerStatus } from "@/lib/findings";
import { useClock } from "./useClock";

/**
 * A Docker host's status line. It owns the event feed and the clock it
 * reads, so an event or a tick re-renders this line, not the page's table.
 */
export function DockerStatusLine({ env, containers }: { env: string; containers: ContainerSummary[] }) {
  const events = useQuery(recentEventsQuery(env));
  const clock = useClock();
  return <span>{dockerStatus(containers, events.data ?? [], clock)}</span>;
}
