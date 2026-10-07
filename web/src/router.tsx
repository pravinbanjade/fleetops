import type { QueryClient } from "@tanstack/react-query";
import {
  createRootRouteWithContext,
  createRoute,
  createRouter,
  lazyRouteComponent,
  Outlet,
  redirect,
  type RouterHistory,
} from "@tanstack/react-router";
import { ApiError } from "@/api/client";
import { authStatusQuery, envsQuery, meQuery } from "@/api/queries";
import { Shell } from "@/components/Shell";
import { isKube } from "@/lib/env";
import { LiveProvider } from "@/live/LiveProvider";
import { TaskProvider } from "@/live/TaskProvider";
import { LoginPage } from "@/pages/LoginPage";
import { NoEnvironmentsPage, NotFoundPage } from "@/pages/misc";
import { CONTAINER_TABS, type ContainerTab } from "@/pages/containers/tabs";
import { HELM_TABS, type HelmTab } from "@/pages/helm/tabs";
import { KUBE_TABS, type KubeTab } from "@/pages/kube/tabs";
import { SetupPage } from "@/pages/SetupPage";

export interface RouterContext {
  queryClient: QueryClient;
}

/** A local path to return to after login; anything else is dropped. */
export function safeRedirect(target: unknown): string | undefined {
  return typeof target === "string" && target.startsWith("/") && !target.startsWith("//") ? target : undefined;
}

/** The session, or null when there is none (a 401 from `/auth/me`). */
async function session(qc: QueryClient, fresh = false) {
  try {
    return await (fresh ? qc.fetchQuery({ ...meQuery, staleTime: 0 }) : qc.ensureQueryData(meQuery));
  } catch (e) {
    if (e instanceof ApiError && e.status === 401) return null;
    throw e;
  }
}

// Pages below the shell load on first visit (`lazyRouteComponent`), so
// each is its own chunk of the bundle.

const rootRoute = createRootRouteWithContext<RouterContext>()({
  component: Outlet,
  notFoundComponent: NotFoundPage,
});

const setupRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/setup",
  beforeLoad: async ({ context }) => {
    const status = await context.queryClient.fetchQuery(authStatusQuery);
    if (!status.setup_required) throw redirect({ to: "/login" });
    return { setupOpen: status.setup_open };
  },
  component: SetupPage,
});

const loginRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/login",
  validateSearch: (s: Record<string, unknown>): { redirect?: string } => ({ redirect: safeRedirect(s.redirect) }),
  beforeLoad: async ({ context, search }) => {
    const status = await context.queryClient.fetchQuery(authStatusQuery);
    if (status.setup_required) throw redirect({ to: "/setup" });
    // Asks the server: the cached session is the one that just ended.
    if (await session(context.queryClient, true)) throw redirect({ to: search.redirect ?? "/" });
  },
  component: LoginPage,
});

/** Everything behind a session: sends visitors to setup or login first. */
const authedRoute = createRoute({
  getParentRoute: () => rootRoute,
  id: "authed",
  beforeLoad: async ({ context, location }) => {
    const status = await context.queryClient.fetchQuery(authStatusQuery);
    if (status.setup_required) throw redirect({ to: "/setup" });
    if (!(await session(context.queryClient))) {
      throw redirect({ to: "/login", search: { redirect: location.href } });
    }
  },
  component: () => (
    <LiveProvider>
      <TaskProvider>
        <Outlet />
      </TaskProvider>
    </LiveProvider>
  ),
});

const indexRoute = createRoute({
  getParentRoute: () => authedRoute,
  path: "/",
  beforeLoad: async ({ context }) => {
    const envs = await context.queryClient.fetchQuery(envsQuery);
    if (envs.length > 0) throw redirect({ to: "/envs/$env", params: { env: envs[0].name } });
  },
  component: NoEnvironmentsPage,
});

const shellRoute = createRoute({
  getParentRoute: () => authedRoute,
  id: "shell",
  component: Shell,
});

export const envRoute = createRoute({
  getParentRoute: () => shellRoute,
  path: "/envs/$env",
  // A Kubernetes environment has no Docker pages and a Docker one no
  // Kubernetes pages: an old link or a changed kind lands on the home page.
  beforeLoad: async ({ context, params, location }) => {
    const envs = await context.queryClient.ensureQueryData(envsQuery);
    const env = envs.find((e) => e.name === params.env);
    if (!env) return;
    const rest = location.pathname.slice(`/envs/${encodeURIComponent(params.env)}`.length);
    const home = rest === "" || rest === "/";
    const kubePage = rest.startsWith("/k8s/");
    if (!home && kubePage !== isKube(env.kind)) throw redirect({ to: "/envs/$env", params: { env: params.env } });
  },
  component: Outlet,
});

const dashboardRoute = createRoute({
  getParentRoute: () => envRoute,
  path: "/",
  component: lazyRouteComponent(() => import("@/pages/EnvHome"), "EnvHome"),
});

const helmReleasesRoute = createRoute({
  getParentRoute: () => envRoute,
  path: "k8s/helm",
  component: lazyRouteComponent(() => import("@/pages/helm/ReleasesPage"), "ReleasesPage"),
});

const helmReleaseRoute = createRoute({
  getParentRoute: () => envRoute,
  path: "k8s/helm/$ns/$name",
  validateSearch: (s: Record<string, unknown>): { tab?: HelmTab } => ({ tab: HELM_TABS.find((t) => t === s.tab) }),
  component: lazyRouteComponent(() => import("@/pages/helm/ReleasePage"), "ReleasePage"),
});

const kubeListRoute = createRoute({
  getParentRoute: () => envRoute,
  path: "k8s/$kind",
  component: lazyRouteComponent(() => import("@/pages/kube/KubeListPage"), "KubeListPage"),
});

const kubeObjectRoute = createRoute({
  getParentRoute: () => envRoute,
  path: "k8s/$kind/$ns/$name",
  validateSearch: (s: Record<string, unknown>): { tab?: KubeTab } => ({ tab: KUBE_TABS.find((t) => t === s.tab) }),
  component: lazyRouteComponent(() => import("@/pages/kube/KubeObjectPage"), "KubeObjectPage"),
});

const containersRoute = createRoute({
  getParentRoute: () => envRoute,
  path: "containers",
  // `q` starts the list's filter, for links such as the dashboard's "2 exited".
  validateSearch: (s: Record<string, unknown>): { q?: string } => ({
    q: typeof s.q === "string" && s.q ? s.q : undefined,
  }),
  component: lazyRouteComponent(() => import("@/pages/containers/ContainersPage"), "ContainersPage"),
});

const containerRoute = createRoute({
  getParentRoute: () => envRoute,
  path: "containers/$id",
  validateSearch: (s: Record<string, unknown>): { tab?: ContainerTab; path?: string } => ({
    tab: CONTAINER_TABS.find((t) => t === s.tab),
    path: typeof s.path === "string" && s.path.startsWith("/") ? s.path : undefined,
  }),
  component: lazyRouteComponent(() => import("@/pages/containers/ContainerPage"), "ContainerPage"),
});

const newContainerRoute = createRoute({
  getParentRoute: () => envRoute,
  path: "containers/new",
  validateSearch: (s: Record<string, unknown>): { from?: string } => ({
    from: typeof s.from === "string" && s.from ? s.from : undefined,
  }),
  component: lazyRouteComponent(() => import("@/pages/containers/ContainerFormPage"), "NewContainerPage"),
});

const editContainerRoute = createRoute({
  getParentRoute: () => envRoute,
  path: "containers/$id/edit",
  component: lazyRouteComponent(() => import("@/pages/containers/ContainerFormPage"), "EditContainerPage"),
});

const imagesRoute = createRoute({
  getParentRoute: () => envRoute,
  path: "images",
  component: lazyRouteComponent(() => import("@/pages/images/ImagesPage"), "ImagesPage"),
});

const imageRoute = createRoute({
  getParentRoute: () => envRoute,
  path: "images/$id",
  component: lazyRouteComponent(() => import("@/pages/images/ImagePage"), "ImagePage"),
});

const volumesRoute = createRoute({
  getParentRoute: () => envRoute,
  path: "volumes",
  component: lazyRouteComponent(() => import("@/pages/volumes/VolumesPage"), "VolumesPage"),
});

const volumeRoute = createRoute({
  getParentRoute: () => envRoute,
  path: "volumes/$name",
  component: lazyRouteComponent(() => import("@/pages/volumes/VolumePage"), "VolumePage"),
});

const networksRoute = createRoute({
  getParentRoute: () => envRoute,
  path: "networks",
  component: lazyRouteComponent(() => import("@/pages/networks/NetworksPage"), "NetworksPage"),
});

const networkRoute = createRoute({
  getParentRoute: () => envRoute,
  path: "networks/$id",
  component: lazyRouteComponent(() => import("@/pages/networks/NetworkPage"), "NetworkPage"),
});

const composeRoute = createRoute({
  getParentRoute: () => envRoute,
  path: "compose",
  component: lazyRouteComponent(() => import("@/pages/compose/ComposePage"), "ComposePage"),
});

/** Administration: admins only; anyone else is sent to the start page. */
const adminRoute = createRoute({
  getParentRoute: () => shellRoute,
  path: "/admin",
  beforeLoad: async ({ context }) => {
    const me = await context.queryClient.ensureQueryData(meQuery);
    if (me.user.role !== "admin") throw redirect({ to: "/" });
  },
  component: Outlet,
});

const usersRoute = createRoute({
  getParentRoute: () => adminRoute,
  path: "users",
  component: lazyRouteComponent(() => import("@/pages/admin/UsersPage"), "UsersPage"),
});

const envsAdminRoute = createRoute({
  getParentRoute: () => adminRoute,
  path: "envs",
  component: lazyRouteComponent(() => import("@/pages/admin/EnvironmentsPage"), "EnvironmentsPage"),
});

const auditRoute = createRoute({
  getParentRoute: () => adminRoute,
  path: "audit",
  component: lazyRouteComponent(() => import("@/pages/admin/AuditPage"), "AuditPage"),
});

const accountRoute = createRoute({
  getParentRoute: () => shellRoute,
  path: "/account",
  component: lazyRouteComponent(() => import("@/pages/AccountPage"), "AccountPage"),
});

const routeTree = rootRoute.addChildren([
  setupRoute,
  loginRoute,
  authedRoute.addChildren([
    indexRoute,
    shellRoute.addChildren([
      envRoute.addChildren([
        dashboardRoute,
        containersRoute,
        newContainerRoute,
        containerRoute,
        editContainerRoute,
        imagesRoute,
        imageRoute,
        volumesRoute,
        volumeRoute,
        networksRoute,
        networkRoute,
        composeRoute,
        helmReleasesRoute,
        helmReleaseRoute,
        kubeListRoute,
        kubeObjectRoute,
      ]),
      adminRoute.addChildren([usersRoute, envsAdminRoute, auditRoute]),
      accountRoute,
    ]),
  ]),
]);

export function createAppRouter(queryClient: QueryClient, history?: RouterHistory) {
  return createRouter({
    routeTree,
    context: { queryClient },
    history,
    defaultPreload: "intent",
    defaultPreloadStaleTime: 0,
  });
}

declare module "@tanstack/react-router" {
  interface Register {
    router: ReturnType<typeof createAppRouter>;
  }
}
