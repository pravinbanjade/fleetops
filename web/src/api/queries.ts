import { infiniteQueryOptions, queryOptions } from "@tanstack/react-query";
import { ApiError, setCsrfToken } from "./client";
import { refKind, type Ref } from "@/pages/kube/generic/ref";
import type { ContainerInspect, ImageInspect, ImageLayer, NetworkInspect, VolumeInspect } from "./docker";
import {
  getAuthStatus,
  getEdition,
  helmCapabilities,
  workloadPods,
  getDiskUsage,
  getEngineInfo,
  getEnvStatus,
  getImageHistory,
  getContainerSpec,
  getMe,
  inspectContainer,
  inspectImage,
  inspectNetwork,
  inspectVolume,
  listContainerFiles,
  listAudit,
  listComposeProjects,
  getStack,
  listStackRevisions,
  getStackGit,
  listStackFiles,
  listStacks,
  listContainers,
  listEnvs,
  listImages,
  listKube,
  getKube,
  kubeEvents,
  kubeDiscovery,
  listGenericKube,
  getGenericKube,
  genericKubeEvents,
  kubeUsage,
  kubeCounts,
  listHelmReleases,
  getHelmRelease,
  helmHistory,
  listNetworks,
  listSessions,
  listUsers,
  listUserAccess,
  listEnvAccess,
  listRecentEvents,
  listVolumes,
} from "./gen/sdk.gen";
import type { DockerEvent, KubeFilter, KubeKind } from "./gen/types.gen";
import { mergeEvents } from "@/live/noise";

/**
 * Query keys. Everything scoped to an environment starts with
 * `["env", env]`, so one prefix invalidates all of it; a resource's
 * detail key extends its list key.
 */
export const keys = {
  authStatus: ["auth", "status"] as const,
  edition: ["edition"] as const,
  me: ["auth", "me"] as const,
  sessions: ["auth", "sessions"] as const,
  envs: ["envs"] as const,
  users: ["users"] as const,
  userAccess: (id: number) => ["users", id, "access"] as const,
  envAccess: (env: string) => ["envs", env, "access"] as const,
  audit: ["audit"] as const,
  env: (env: string) => ["env", env] as const,
  envStatus: (env: string) => ["env", env, "status"] as const,
  info: (env: string) => ["env", env, "info"] as const,
  df: (env: string) => ["env", env, "df"] as const,
  containers: (env: string) => ["env", env, "containers"] as const,
  container: (env: string, id: string) => ["env", env, "containers", id] as const,
  /**
   * A directory listing inside a container. Outside the containers prefix
   * on purpose: a listing downloads the directory's whole subtree, so it is
   * not refetched on every container event.
   */
  containerFiles: (env: string, id: string, dir: string) => ["env", env, "container-files", id, dir] as const,
  images: (env: string) => ["env", env, "images"] as const,
  image: (env: string, id: string) => ["env", env, "images", id] as const,
  volumes: (env: string) => ["env", env, "volumes"] as const,
  volume: (env: string, name: string) => ["env", env, "volumes", name] as const,
  networks: (env: string) => ["env", env, "networks"] as const,
  network: (env: string, id: string) => ["env", env, "networks", id] as const,
  composeProjects: (env: string) => ["env", env, "compose-projects"] as const,
  stacks: (env: string) => ["env", env, "stacks"] as const,
  stack: (env: string, name: string) => ["env", env, "stacks", name] as const,
  stackRevisions: (env: string, name: string) => ["env", env, "stacks", name, "revisions"] as const,
  stackGit: (env: string, name: string) => ["env", env, "stacks", name, "git"] as const,
  stackFiles: (env: string, name: string) => ["env", env, "stacks", name, "files"] as const,
  /** A filtered list (`filter`) is cached apart from the whole one. */
  kube: (env: string, kind: string, ns?: string, filter?: KubeFilter) =>
    (filter ? ["env", env, "k8s", kind, ns ?? "*", filter] : ["env", env, "k8s", kind, ns ?? "*"]) as readonly string[],
  kubeCounts: (env: string, kinds: readonly KubeKind[], ns?: string) => ["env", env, "k8s-counts", kinds.join(","), ns ?? "*"] as const,
  kubeObject: (env: string, kind: string, ns: string, name: string) => ["env", env, "k8s", kind, ns, name] as const,
  /** Outside the `k8s` prefix: a write's invalidation should not refetch usage. */
  kubeUsage: (env: string, kind: "nodes" | "pods", ns?: string) => ["env", env, "k8s-usage", kind, ns ?? "*"] as const,
  helm: (env: string, ns?: string) => ["env", env, "helm", ns ?? "*"] as const,
  helmCapabilities: (env: string) => ["env", env, "helm-capabilities"] as const,
  helmRelease: (env: string, ns: string, name: string) => ["env", env, "helm", ns, name] as const,
  /** The newest Docker events seen over the websocket; filled by the live layer, never fetched. */
  recentEvents: (env: string) => ["live", env, "events"] as const,
};

export const authStatusQuery = queryOptions({
  queryKey: keys.authStatus,
  queryFn: async () => (await getAuthStatus()).data,
});

export const editionQuery = queryOptions({
  queryKey: keys.edition,
  queryFn: async () => (await getEdition()).data,
  staleTime: 60_000,
});

export const meQuery = queryOptions({
  queryKey: keys.me,
  queryFn: async () => {
    const { data } = await getMe();
    setCsrfToken(data.csrf_token);
    return data;
  },
  staleTime: Infinity,
  retry: false,
});

export const sessionsQuery = queryOptions({
  queryKey: keys.sessions,
  queryFn: async () => (await listSessions()).data,
});

export const envsQuery = queryOptions({
  queryKey: keys.envs,
  queryFn: async () => (await listEnvs()).data,
});

export const usersQuery = queryOptions({
  queryKey: keys.users,
  queryFn: async () => (await listUsers()).data,
});

export const userAccessQuery = (id: number) =>
  queryOptions({ queryKey: keys.userAccess(id), queryFn: async () => (await listUserAccess({ path: { id } })).data });

export const envAccessQuery = (env: string) =>
  queryOptions({ queryKey: keys.envAccess(env), queryFn: async () => (await listEnvAccess({ path: { env } })).data });

/** Audit entries a page asks for at once. */
export const AUDIT_PAGE = 100;

/** The audit log, newest first, a page at a time: each page asks for entries older than the last one seen. */
export const auditQuery = infiniteQueryOptions({
  queryKey: keys.audit,
  queryFn: async ({ pageParam }) => (await listAudit({ query: { limit: AUDIT_PAGE, before: pageParam } })).data,
  initialPageParam: undefined as number | undefined,
  getNextPageParam: (last) => (last.length < AUDIT_PAGE ? undefined : last[last.length - 1].id),
});

const path = (env: string) => ({ path: { env } });

/** How often a down environment's status is checked, to notice it coming back. */
export const ENV_DOWN_POLL_MS = 5000;

/** How often a Kubernetes environment's status is read; nothing pushes it. */
export const KUBE_STATUS_POLL_MS = 15000;

export const envStatusQuery = (env: string, kube = false) =>
  queryOptions({
    queryKey: keys.envStatus(env),
    queryFn: async () => (await getEnvStatus(path(env))).data,
    refetchInterval: (q) =>
      q.state.data?.state === "down" ? ENV_DOWN_POLL_MS : kube ? KUBE_STATUS_POLL_MS : false,
  });

export const kubeListQuery = (env: string, kind: KubeKind, ns?: string, filter?: KubeFilter) =>
  queryOptions({
    queryKey: keys.kube(env, kind, ns, filter),
    queryFn: async () =>
      (await listKube({ path: { env, kind }, query: { ...(ns ? { namespace: ns } : {}), ...(filter ? { filter } : {}) } })).data,
    retry: (failures, err) => !(err instanceof ApiError && err.status === 403) && failures < 1,
  });

/** metrics-server samples every 15s; a cluster without it is asked again each minute. */
export const kubeUsageQuery = (env: string, kind: "nodes" | "pods", ns?: string) =>
  queryOptions({
    queryKey: keys.kubeUsage(env, kind, ns),
    queryFn: async () => (await kubeUsage({ path: { env, kind }, query: ns ? { namespace: ns } : {} })).data,
    refetchInterval: (q) => (q.state.data?.available === false ? 60_000 : 15_000),
    retry: (failures, err) => !(err instanceof ApiError && err.status === 403) && failures < 1,
  });

/** Counts without bodies, for an overview's inventory; they change slowly. */
export const kubeCountsQuery = (env: string, kinds: readonly KubeKind[], ns?: string) =>
  queryOptions({
    queryKey: keys.kubeCounts(env, kinds, ns),
    queryFn: async () => (await kubeCounts({ path: { env }, query: { kinds: kinds.join(","), ...(ns ? { namespace: ns } : {}) } })).data,
    staleTime: 30_000,
    refetchInterval: 60_000,
  });

/** The pods a workload's selector matches, for the terminal picker. */
export const workloadPodsQuery = (env: string, kind: KubeKind, ns: string, name: string) =>
  queryOptions({
    queryKey: [...keys.kubeObject(env, kind, ns, name), "pods"] as const,
    queryFn: async () => (await workloadPods({ path: { env, kind, ns, name } })).data,
  });

export const kubeObjectQuery = (env: string, kind: KubeKind, ns: string, name: string) =>
  queryOptions({
    queryKey: keys.kubeObject(env, kind, ns, name),
    queryFn: async () => (await getKube({ path: { env, kind, ns, name } })).data,
    retry: retryUnlessGone,
  });

export const kubeEventsQuery = (env: string, kind: KubeKind, ns: string, name: string) =>
  queryOptions({
    queryKey: [...keys.kubeObject(env, kind, ns, name), "events"] as const,
    queryFn: async () => (await kubeEvents({ path: { env, kind, ns, name } })).data,
  });

/** The cluster's discovered kinds; the server caches them for a minute too. */
export const discoveryQuery = (env: string) =>
  queryOptions({
    queryKey: ["env", env, "k8s-discovery"] as const,
    queryFn: async () => (await kubeDiscovery({ path: { env } })).data,
    staleTime: 60_000,
  });

export const genericListQuery = (env: string, r: Ref, ns?: string) =>
  queryOptions({
    queryKey: keys.kube(env, refKind(r), ns),
    queryFn: async () => (await listGenericKube({ path: { env, ...r }, query: ns ? { namespace: ns } : {} })).data,
    retry: (failures, err) => !(err instanceof ApiError && (err.status === 403 || err.status === 404)) && failures < 1,
  });

export const genericObjectQuery = (env: string, r: Ref, ns: string, name: string) =>
  queryOptions({
    queryKey: keys.kubeObject(env, refKind(r), ns, name),
    queryFn: async () => (await getGenericKube({ path: { env, ...r, ns, name } })).data,
  });

export const genericEventsQuery = (env: string, r: Ref, ns: string, name: string) =>
  queryOptions({
    queryKey: [...keys.kubeObject(env, refKind(r), ns, name), "events"] as const,
    queryFn: async () => (await genericKubeEvents({ path: { env, ...r, ns, name } })).data,
  });

export const infoQuery = (env: string) =>
  queryOptions({ queryKey: keys.info(env), queryFn: async () => (await getEngineInfo(path(env))).data as EngineInfo });

export const dfQuery = (env: string) =>
  queryOptions({ queryKey: keys.df(env), queryFn: async () => (await getDiskUsage(path(env))).data as DiskUsage });

export const containersQuery = (env: string) =>
  queryOptions({ queryKey: keys.containers(env), queryFn: async () => (await listContainers(path(env))).data });

/** Retry a failed detail query once, but not when the resource is gone. */
const retryUnlessGone = (failures: number, err: unknown) =>
  !(err instanceof ApiError && err.status === 404) && failures < 1;

/** Releases are not watched; the list is read this often while visible (spec 3d §2.3). */
export const HELM_POLL_MS = 10000;

/** Whether this server can install and upgrade charts (spec 3j §5). */
export const helmCapabilitiesQuery = (env: string) =>
  queryOptions({
    queryKey: keys.helmCapabilities(env),
    queryFn: async () => (await helmCapabilities({ path: { env } })).data,
    staleTime: 5 * 60_000,
  });

export const helmReleasesQuery = (env: string, ns?: string) =>
  queryOptions({
    queryKey: keys.helm(env, ns),
    queryFn: async () => (await listHelmReleases({ path: { env }, query: ns ? { namespace: ns } : {} })).data,
    refetchInterval: HELM_POLL_MS,
  });

export const helmReleaseQuery = (env: string, ns: string, name: string, revision?: number) =>
  queryOptions({
    queryKey: [...keys.helmRelease(env, ns, name), revision ?? "latest"] as const,
    queryFn: async () => (await getHelmRelease({ path: { env, ns, name }, query: revision ? { revision } : {} })).data,
    retry: retryUnlessGone,
  });

export const helmHistoryQuery = (env: string, ns: string, name: string) =>
  queryOptions({
    queryKey: [...keys.helmRelease(env, ns, name), "history"] as const,
    queryFn: async () => (await helmHistory({ path: { env, ns, name } })).data,
  });

export const containerQuery = (env: string, id: string) =>
  queryOptions({
    queryKey: keys.container(env, id),
    queryFn: async () => (await inspectContainer({ path: { env, id } })).data as unknown as ContainerInspect,
    retry: retryUnlessGone,
  });

/** The container as the create form edits it (for recreate and duplicate). */
export const containerSpecQuery = (env: string, id: string) =>
  queryOptions({
    queryKey: [...keys.container(env, id), "spec"] as const,
    queryFn: async () => (await getContainerSpec({ path: { env, id } })).data,
    retry: retryUnlessGone,
  });

export const containerFilesQuery = (env: string, id: string, dir: string) =>
  queryOptions({
    queryKey: keys.containerFiles(env, id, dir),
    queryFn: async () => (await listContainerFiles({ path: { env, id }, query: { path: dir } })).data,
    retry: retryUnlessGone,
  });

export const imagesQuery = (env: string) =>
  queryOptions({ queryKey: keys.images(env), queryFn: async () => (await listImages(path(env))).data });

export const imageQuery = (env: string, id: string) =>
  queryOptions({
    queryKey: keys.image(env, id),
    queryFn: async () => (await inspectImage({ path: { env, id } })).data as unknown as ImageInspect,
    retry: retryUnlessGone,
  });

export const imageHistoryQuery = (env: string, id: string) =>
  queryOptions({
    queryKey: [...keys.image(env, id), "history"] as const,
    queryFn: async () => (await getImageHistory({ path: { env, id } })).data as unknown as ImageLayer[],
    retry: retryUnlessGone,
  });

export const volumesQuery = (env: string) =>
  queryOptions({ queryKey: keys.volumes(env), queryFn: async () => (await listVolumes(path(env))).data });

export const volumeQuery = (env: string, name: string) =>
  queryOptions({
    queryKey: keys.volume(env, name),
    queryFn: async () => (await inspectVolume({ path: { env, name } })).data as unknown as VolumeInspect,
    retry: retryUnlessGone,
  });

export const networksQuery = (env: string) =>
  queryOptions({ queryKey: keys.networks(env), queryFn: async () => (await listNetworks(path(env))).data });

export const networkQuery = (env: string, id: string) =>
  queryOptions({
    queryKey: keys.network(env, id),
    queryFn: async () => (await inspectNetwork({ path: { env, id } })).data as unknown as NetworkInspect,
    retry: retryUnlessGone,
  });

export const composeProjectsQuery = (env: string) =>
  queryOptions({
    queryKey: keys.composeProjects(env),
    queryFn: async () => (await listComposeProjects(path(env))).data,
  });

export const stacksQuery = (env: string) =>
  queryOptions({ queryKey: keys.stacks(env), queryFn: async () => (await listStacks(path(env))).data });

export const stackQuery = (env: string, name: string) =>
  queryOptions({
    queryKey: keys.stack(env, name),
    queryFn: async () => (await getStack({ path: { env, stack: name } })).data,
    retry: retryUnlessGone,
  });

export const stackRevisionsQuery = (env: string, name: string) =>
  queryOptions({
    queryKey: keys.stackRevisions(env, name),
    queryFn: async () => (await listStackRevisions({ path: { env, stack: name } })).data,
  });

export const stackGitQuery = (env: string, name: string) =>
  queryOptions({
    queryKey: keys.stackGit(env, name),
    queryFn: async () => (await getStackGit({ path: { env, stack: name } })).data,
  });

export const stackFilesQuery = (env: string, name: string) =>
  queryOptions({
    queryKey: keys.stackFiles(env, name),
    queryFn: async () => (await listStackFiles({ path: { env, stack: name } })).data,
  });

/** How far back the dashboard's event feed starts, in seconds. */
export const RECENT_EVENTS_WINDOW = 15 * 60;

/**
 * The dashboard's event feed. It starts from the events Docker still holds
 * from the last 15 minutes; after that the live stream adds to it (see
 * EventApplier), so it is never refetched.
 */
export const recentEventsQuery = (env: string) =>
  queryOptions<DockerEvent[]>({
    queryKey: keys.recentEvents(env),
    queryFn: async ({ client, queryKey }) => {
      const since = Math.floor(Date.now() / 1000) - RECENT_EVENTS_WINDOW;
      const past = (await listRecentEvents({ path: { env }, query: { since } })).data;
      // Live events that arrived while this request was out are kept.
      return mergeEvents(client.getQueryData<DockerEvent[]>(queryKey) ?? [], past);
    },
    staleTime: Infinity,
    gcTime: Infinity,
  });

/** The fields of Docker's `GET /info` the UI reads (the API passes it through). */
export interface EngineInfo {
  Name?: string;
  ServerVersion?: string;
  OperatingSystem?: string;
  Architecture?: string;
  NCPU?: number;
  MemTotal?: number;
  Containers?: number;
  ContainersRunning?: number;
  ContainersPaused?: number;
  ContainersStopped?: number;
  Images?: number;
}

/** Docker's `GET /system/df`, as the API passes it through: newer engines send `*Usage` totals, older ones item lists. */
export interface DiskUsage {
  ImageUsage?: UsageTotals;
  ContainerUsage?: UsageTotals;
  VolumeUsage?: UsageTotals;
  BuildCacheUsage?: UsageTotals;
  LayersSize?: number;
  Images?: { Size?: number }[] | null;
  Containers?: { SizeRw?: number }[] | null;
  Volumes?: { UsageData?: { Size?: number } | null }[] | null;
  BuildCache?: { Size?: number }[] | null;
}

export interface UsageTotals {
  TotalSize?: number;
  Reclaimable?: number;
}
