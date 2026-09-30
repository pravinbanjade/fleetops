import { queryOptions } from "@tanstack/react-query";
import { ApiError, setCsrfToken } from "./client";
import type { ContainerInspect, ImageInspect, ImageLayer } from "./docker";
import {
  getAuthStatus,
  getDiskUsage,
  getEngineInfo,
  getEnvStatus,
  getImageHistory,
  getContainerSpec,
  getMe,
  inspectContainer,
  inspectImage,
  listContainerFiles,
  listContainers,
  listEnvs,
  listImages,
  listNetworks,
  listSessions,
  listVolumes,
} from "./gen/sdk.gen";
import type { DockerEvent } from "./gen/types.gen";

/**
 * Query keys. Everything scoped to an environment starts with
 * `["env", env]`, so one prefix invalidates all of it; a resource's
 * detail key extends its list key.
 */
export const keys = {
  authStatus: ["auth", "status"] as const,
  me: ["auth", "me"] as const,
  sessions: ["auth", "sessions"] as const,
  envs: ["envs"] as const,
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
  /** The newest Docker events seen over the websocket; filled by the live layer, never fetched. */
  recentEvents: (env: string) => ["live", env, "events"] as const,
};

export const authStatusQuery = queryOptions({
  queryKey: keys.authStatus,
  queryFn: async () => (await getAuthStatus()).data,
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

const path = (env: string) => ({ path: { env } });

/** How often a down environment's status is checked, to notice it coming back. */
export const ENV_DOWN_POLL_MS = 5000;

export const envStatusQuery = (env: string) =>
  queryOptions({
    queryKey: keys.envStatus(env),
    queryFn: async () => (await getEnvStatus(path(env))).data,
    refetchInterval: (q) => (q.state.data?.state === "down" ? ENV_DOWN_POLL_MS : false),
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

export const networksQuery = (env: string) =>
  queryOptions({ queryKey: keys.networks(env), queryFn: async () => (await listNetworks(path(env))).data });

export const recentEventsQuery = (env: string) =>
  queryOptions<DockerEvent[]>({
    queryKey: keys.recentEvents(env),
    queryFn: () => [],
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
