import { CloudDownload, Eraser, Hammer, Trash2 } from "lucide-react";
import { useNow, useNowRow } from "@/components/shell/now";
import { ObjectNow } from "@/components/shell/ObjectNow";
import { useQuery } from "@tanstack/react-query";
import { Link, useParams } from "@tanstack/react-router";
import { useMemo, useState } from "react";
import { containersQuery, imagesQuery } from "@/api/queries";
import type { ImageSummary } from "@/api/gen/types.gen";
import { columns, DataTable, type Column } from "@/components/DataTable";
import { ImageRef } from "@/components/ImageRef";
import { Page } from "@/components/Page";
import { Badge } from "@/components/ui/badge";
import { useEnvAccess } from "@/api/access";
import { Button } from "@/components/ui/button";
import { formatAgo, formatBytes, shortId } from "@/lib/format";
import { imageName, PruneImagesDialog, RemoveImagesDialog } from "./dialogs";
import { BuildDialog, PullDialog } from "./TaskDialogs";

const col = columns<ImageSummary>();

function imageColumns(env: string): Column<ImageSummary>[] {
  return [
    col.accessor((i) => imageName(i.repo_tags, i.id), {
      id: "tags",
      header: "Tags",
      enableHiding: false,
      cell: (c) => (
        <Link
          to="/envs/$env/images/$id"
          params={{ env, id: c.row.original.id }}
          className="flex flex-wrap gap-x-3 gap-y-0.5 hover:underline"
        >
          {c.row.original.repo_tags.length ? (
            c.row.original.repo_tags.map((t, i) => (
              <span key={t}>
                {i > 0 && <span className="sr-only">, </span>}
                <ImageRef value={t} />
              </span>
            ))
          ) : (
            <span className="text-muted-foreground">&lt;none&gt;</span>
          )}
        </Link>
      ),
    }),
    col.accessor((i) => shortId(i.id), { id: "id", header: "ID", cell: (c) => <code className="text-xs">{c.getValue()}</code> }),
    col.accessor("size", { header: "Size", cell: (c) => formatBytes(c.getValue()) }),
    col.accessor("created", { header: "Created", cell: (c) => formatAgo(c.getValue()) }),
    col.accessor((i) => (i.in_use ? "in use" : "unused"), {
      id: "in_use",
      header: "Used",
      cell: (c) => (c.row.original.in_use ? <Badge variant="secondary">in use</Badge> : <span className="text-muted-foreground">unused</span>),
    }),
  ];
}

export function ImagesPage() {
  const { env } = useParams({ from: "/authed/shell/envs/$env/images" });
  const { can } = useEnvAccess(env);
  const canCreate = can("images.create");
  const canDelete = can("images.delete");
  const list = useQuery(imagesQuery(env));
  const now = useNow();
  const active = useNowRow(list.data, (i: ImageSummary) => i.id);
  const cols = useMemo(() => imageColumns(env), [env]);
  const [removing, setRemoving] = useState<ImageSummary[] | null>(null);
  const [pruning, setPruning] = useState(false);
  const [pulling, setPulling] = useState(false);
  const [building, setBuilding] = useState(false);
  return (
    <Page
      title="Images"
      actions={
        (canCreate || canDelete) && (
          <>
            {canDelete && (
              <Button variant="outline" onClick={() => setPruning(true)}>
                <Eraser aria-hidden />
                Prune
              </Button>
            )}
            {canCreate && (
              <>
                <Button variant="outline" onClick={() => setBuilding(true)}>
                  <Hammer aria-hidden />
                  Build
                </Button>
                <Button onClick={() => setPulling(true)}>
<CloudDownload aria-hidden />Pull</Button>
              </>
            )}
          </>
        )
      }
    >
      {active && <ImageNow key={active.id} env={env} image={active} />}
      <DataTable
        label="Images"
        data={list.data}
        error={list.error}
        columns={cols}
        getRowId={(i) => i.id}
        onActivate={(r) => now.open(((i: ImageSummary) => i.id)(r))}
        activeId={active ? ((i: ImageSummary) => i.id)(active) : undefined}
        empty="No images in this environment."
        initialSorting={[{ id: "created", desc: true }]}
        selectable={canDelete}
        actions={
          canDelete
            ? (selected, clear) => (
                <Button
                  variant="destructive"
                  size="sm"
                  onClick={() => {
                    setRemoving(selected);
                    clear();
                  }}
                >
                  <Trash2 aria-hidden />
                  Remove
                </Button>
              )
            : undefined
        }
      />
      {removing && (
        <RemoveImagesDialog
          env={env}
          images={removing.map((i) => ({ id: i.id, name: imageName(i.repo_tags, i.id) }))}
          onDone={() => setRemoving(null)}
        />
      )}
      {pruning && <PruneImagesDialog env={env} onDone={() => setPruning(false)} />}
      {pulling && <PullDialog env={env} onClose={() => setPulling(false)} />}
      {building && <BuildDialog env={env} onClose={() => setBuilding(false)} />}
    </Page>
  );
}

/** An image in the panel: its size and age, and the containers that run it. */
function ImageNow({ env, image }: { env: string; image: ImageSummary }) {
  const containers = useQuery(containersQuery(env));
  const users = (containers.data ?? []).filter((c) => c.image_id === image.id);
  return (
    <ObjectNow
      title={<span className="font-ident">{image.repo_tags[0] ?? shortId(image.id)}</span>}
      subtitle={image.repo_tags.length > 1 ? `${image.repo_tags.length} tags` : shortId(image.id)}
      facts={[
        { label: "Size", value: <span className="font-figure">{formatBytes(image.size)}</span> },
        { label: "Created", value: formatAgo(image.created) },
        { label: "Containers", value: <span className="font-figure">{users.length}</span> },
      ]}
      list={{
        title: "Used by",
        empty: "No container uses this image.",
        rows: users.map((c) => ({ key: c.id, label: c.name, aside: c.state, link: { to: "/envs/$env/containers/$id", params: { env, id: c.id } } })),
      }}
      open={{ to: "/envs/$env/images/$id", params: { env, id: image.id } }}
    />
  );
}
