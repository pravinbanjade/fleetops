import { useQuery } from "@tanstack/react-query";
import { Link, useParams } from "@tanstack/react-router";
import { useMemo, useState } from "react";
import { imagesQuery } from "@/api/queries";
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
                Prune
              </Button>
            )}
            {canCreate && (
              <>
                <Button variant="outline" onClick={() => setBuilding(true)}>
                  Build
                </Button>
                <Button onClick={() => setPulling(true)}>Pull</Button>
              </>
            )}
          </>
        )
      }
    >
      <DataTable
        label="Images"
        data={list.data}
        error={list.error}
        columns={cols}
        getRowId={(i) => i.id}
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
