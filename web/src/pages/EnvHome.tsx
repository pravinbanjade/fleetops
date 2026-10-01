import { useQuery } from "@tanstack/react-query";
import { getRouteApi } from "@tanstack/react-router";
import { lazy, Suspense } from "react";
import { envsQuery } from "@/api/queries";
import { isKube } from "@/lib/env";

const DashboardPage = lazy(() => import("@/pages/DashboardPage").then((m) => ({ default: m.DashboardPage })));
const KubeOverviewPage = lazy(() => import("@/pages/kube/KubeOverviewPage").then((m) => ({ default: m.KubeOverviewPage })));
const route = getRouteApi("/authed/shell/envs/$env/");

/** `/envs/$env`: the Docker dashboard or the Kubernetes overview. */
export function EnvHome() {
  const { env } = route.useParams();
  const { data: envs } = useQuery(envsQuery);
  if (!envs) return null;
  const kube = isKube(envs.find((e) => e.name === env)?.kind);
  return <Suspense fallback={null}>{kube ? <KubeOverviewPage env={env} /> : <DashboardPage />}</Suspense>;
}
