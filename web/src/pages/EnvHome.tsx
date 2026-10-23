import { useQuery } from "@tanstack/react-query";
import { getRouteApi } from "@tanstack/react-router";
import { lazy, Suspense } from "react";
import { envsQuery } from "@/api/queries";
import { envFamily, findEnv } from "@/lib/env";

const DashboardPage = lazy(() => import("@/pages/DashboardPage").then((m) => ({ default: m.DashboardPage })));
const KubeOverviewPage = lazy(() => import("@/pages/kube/KubeOverviewPage").then((m) => ({ default: m.KubeOverviewPage })));
const SwarmHome = lazy(() => import("@/pages/swarm/SwarmHome").then((m) => ({ default: m.SwarmHome })));
const route = getRouteApi("/authed/shell/envs/$env/");

/** `/envs/$env`: the Docker dashboard, the Kubernetes overview, or a Swarm's nodes. */
export function EnvHome() {
  const { env } = route.useParams();
  const { data: envs } = useQuery(envsQuery);
  if (!envs) return null;
  const family = envFamily(findEnv(envs, env)?.kind);
  return (
    <Suspense fallback={null}>
      {family === "kube" ? <KubeOverviewPage env={env} /> : family === "swarm" ? <SwarmHome env={env} /> : <DashboardPage />}
    </Suspense>
  );
}
