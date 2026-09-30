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
