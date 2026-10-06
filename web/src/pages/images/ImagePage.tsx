import { useQuery } from "@tanstack/react-query";
import { Link, useNavigate, useParams } from "@tanstack/react-router";
import { useState } from "react";
import { imageHistoryQuery, imageQuery } from "@/api/queries";
import type { ImageInspect } from "@/api/docker";
import { Details, DetailError } from "@/components/Details";
import { JsonView } from "@/components/JsonView";
import { Masked } from "@/components/Masked";
import { Page } from "@/components/Page";
import { useEnvAccess } from "@/api/access";
import { Button } from "@/components/ui/button";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { joinArgs } from "@/lib/args";
import { errorMessage } from "@/lib/errors";
import { formatAgo, formatBytes, shortId } from "@/lib/format";
import { isSecretKey, maskEnv, splitEnv } from "@/lib/secrets";
import { imageName, RemoveImagesDialog, TagDialog } from "./dialogs";

export function ImagePage() {
  const { env, id } = useParams({ from: "/authed/shell/envs/$env/images/$id" });
  const { canWrite } = useEnvAccess(env);
  const navigate = useNavigate();
  const q = useQuery(imageQuery(env, id));
  const [tagging, setTagging] = useState(false);
  const [removing, setRemoving] = useState(false);
  const back = (
    <Link to="/envs/$env/images" params={{ env }} className="underline">
      Back to images
    </Link>
  );
  if (q.error) return <DetailError error={q.error} what="image" back={back} />;
  const img = q.data;
  if (!img) return <p className="p-6 text-sm text-muted-foreground">Loading…</p>;
  const name = imageName(img.RepoTags, img.Id);
  return (
    <Page
      title={name}
      actions={
        canWrite && (
          <>
            <Button variant="outline" onClick={() => setTagging(true)}>
              Tag
            </Button>
            <Button variant="destructive" onClick={() => setRemoving(true)}>
              Remove
            </Button>
          </>
        )
      }
    >
      <Tabs defaultValue="overview">
        <TabsList>
          <TabsTrigger value="overview">Overview</TabsTrigger>
          <TabsTrigger value="history">History</TabsTrigger>
          <TabsTrigger value="inspect">Inspect</TabsTrigger>
        </TabsList>
        <TabsContent value="overview">
          <Overview img={img} />
        </TabsContent>
        <TabsContent value="history">
          <History env={env} id={id} />
        </TabsContent>
        <TabsContent value="inspect">
          <JsonView
            value={img}
            masked={img.Config?.Env ? { ...img, Config: { ...img.Config, Env: maskEnv(img.Config.Env) } } : undefined}
            label="Image JSON"
          />
        </TabsContent>
      </Tabs>
      {tagging && <TagDialog env={env} id={img.Id} onClose={() => setTagging(false)} />}
      {removing && (
        <RemoveImagesDialog
          env={env}
          images={[{ id: img.Id, name }]}
          onDone={(removed) => {
            setRemoving(false);
            if (removed) void navigate({ to: "/envs/$env/images", params: { env } });
          }}
        />
      )}
    </Page>
  );
}

function Overview({ img }: { img: ImageInspect }) {
  const cfg = img.Config ?? {};
  return (
    <Details
      label="Image details"
      items={[
        ["ID", <code key="id">{shortId(img.Id)}</code>],
        ["Tags", img.RepoTags?.join(", ") || "none"],
        ["Digests", img.RepoDigests?.join(", ")],
        ["Size", formatBytes(img.Size)],
        ["Created", new Date(img.Created).toLocaleString()],
        ["Platform", [img.Os, img.Architecture].filter(Boolean).join("/")],
        ["Entrypoint", cfg.Entrypoint ? joinArgs(cfg.Entrypoint) : undefined],
        ["Command", cfg.Cmd ? joinArgs(cfg.Cmd) : undefined],
        ["Working directory", cfg.WorkingDir],
        ["User", cfg.User],
        ["Exposed ports", Object.keys(cfg.ExposedPorts ?? {}).join(", ")],
        ["Environment", cfg.Env?.length ? <EnvList key="env" env={cfg.Env} /> : undefined],
        [
          "Labels",
          Object.keys(cfg.Labels ?? {}).length ? (
            <EnvList key="labels" env={Object.entries(cfg.Labels!).map(([k, v]) => `${k}=${v}`)} />
          ) : undefined,
        ],
      ]}
    />
  );
}

function EnvList({ env }: { env: string[] }) {
  return (
    <ul className="grid gap-0.5 font-mono text-xs">
      {env.map((e) => {
        const [k, v] = splitEnv(e);
        return (
          <li key={e} className="break-all">
            {isSecretKey(k) ? (
              <>
                {k}=<Masked name={k} value={v} />
              </>
            ) : (
              e
            )}
          </li>
        );
      })}
    </ul>
  );
}

function History({ env, id }: { env: string; id: string }) {
  const q = useQuery(imageHistoryQuery(env, id));
  if (q.error)
    return (
      <p role="alert" className="text-sm text-destructive">
        {errorMessage(q.error)}
      </p>
    );
  if (!q.data) return <p className="text-sm text-muted-foreground">Loading…</p>;
  return (
    <Table aria-label="Layers">
      <TableHeader>
        <TableRow>
          <TableHead>Created</TableHead>
          <TableHead>Size</TableHead>
          <TableHead>Created by</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {q.data.map((l, i) => (
          <TableRow key={i}>
            <TableCell>{formatAgo(l.Created)}</TableCell>
            <TableCell>{formatBytes(l.Size)}</TableCell>
            <TableCell className="max-w-3xl font-mono text-xs whitespace-normal">
              <span className="line-clamp-3 break-all" title={l.CreatedBy}>
                {l.CreatedBy}
              </span>
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}
