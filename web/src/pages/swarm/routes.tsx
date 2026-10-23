import { NodesPage } from "./NodesPage";
import { useEnv } from "./shared";

/** `/envs/$env/swarm/nodes`. */
export function NodesRoute() {
  return <NodesPage env={useEnv()} />;
}
