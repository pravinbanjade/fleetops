/**
 * The fields of Docker's own inspect bodies that the UI reads. The API
 * passes these through verbatim, so the generated types only say `object`.
 */

export interface ContainerInspect {
  Id: string;
  /** With Docker's leading slash: `/web`. */
  Name: string;
  Created: string;
  State: {
    Status: string;
    Running?: boolean;
    Paused?: boolean;
    ExitCode?: number;
    StartedAt?: string;
    FinishedAt?: string;
    Error?: string;
    Health?: { Status: string } | null;
  };
  Config: {
    Image: string;
    Hostname?: string;
    User?: string;
    WorkingDir?: string;
    Env?: string[] | null;
    Cmd?: string[] | null;
    Entrypoint?: string[] | null;
    Labels?: Record<string, string> | null;
  };
  HostConfig: {
    RestartPolicy?: { Name: string; MaximumRetryCount?: number };
    Privileged?: boolean;
  };
  Mounts?: {
    Type: string;
    Name?: string;
    Source: string;
    Destination: string;
    Mode?: string;
    RW: boolean;
  }[];
  NetworkSettings: {
    Ports?: Record<string, { HostIp: string; HostPort: string }[] | null> | null;
    Networks?: Record<
      string,
      { IPAddress?: string; Gateway?: string; MacAddress?: string; Aliases?: string[] | null; NetworkID?: string }
    > | null;
  };
}

export interface ImageInspect {
  Id: string;
  RepoTags?: string[] | null;
  RepoDigests?: string[] | null;
  Created: string;
  Size: number;
  Architecture?: string;
  Os?: string;
  Config?: {
    Env?: string[] | null;
    Cmd?: string[] | null;
    Entrypoint?: string[] | null;
    ExposedPorts?: Record<string, object> | null;
    Labels?: Record<string, string> | null;
    WorkingDir?: string;
    User?: string;
  } | null;
}

/** One layer of `GET /images/{id}/history`, newest first. */
export interface ImageLayer {
  /** `<missing>` for layers built elsewhere. */
  Id: string;
  /** Unix seconds. */
  Created: number;
  CreatedBy: string;
  Size: number;
  Tags?: string[] | null;
  Comment?: string;
}

export interface VolumeInspect {
  Name: string;
  Driver: string;
  Mountpoint: string;
  CreatedAt?: string;
  Scope?: string;
  Labels?: Record<string, string> | null;
  Options?: Record<string, string> | null;
}

export interface NetworkInspect {
  Id: string;
  Name: string;
  Driver: string;
  Scope?: string;
  Internal?: boolean;
  Attachable?: boolean;
  Created?: string;
  IPAM?: { Config?: { Subnet?: string; Gateway?: string }[] | null } | null;
  /** Attached containers by id. */
  Containers?: Record<string, { Name: string; IPv4Address?: string; IPv6Address?: string; MacAddress?: string }> | null;
  Labels?: Record<string, string> | null;
  Options?: Record<string, string> | null;
}
