import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { Download, File, Folder, Link2 } from "lucide-react";
import { containerFilesQuery } from "@/api/queries";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { errorMessage } from "@/lib/errors";
import { crumbs, formatMode, joinPath } from "@/lib/files";
import { formatBytes, formatTime } from "@/lib/format";

/** Read-only browsing of the container's filesystem, with single-file download. */
export function FilesTab({ env, id, path }: { env: string; id: string; path: string }) {
  const q = useQuery(containerFilesQuery(env, id, path));
  const to = "/envs/$env/containers/$id" as const;
  const params = { env, id };
  return (
    <div className="grid gap-3">
      <nav aria-label="Path" className="flex flex-wrap items-center gap-1 font-mono text-sm">
        {crumbs(path).map((c, i) => (
          <span key={c.path} className="flex items-center gap-1">
            {i > 1 && <span className="text-muted-foreground">/</span>}
            <Link to={to} params={params} search={{ tab: "files", path: c.path }} className="hover:underline">
              {c.name}
            </Link>
          </span>
        ))}
      </nav>
      {q.error ? (
        <p role="alert" className="text-sm text-destructive">
          {errorMessage(q.error)}
        </p>
      ) : !q.data ? (
        <p className="text-sm text-muted-foreground">Loading…</p>
      ) : (
        <>
          {q.data.truncated && (
            <p className="text-sm text-muted-foreground">
              This directory is too large to list in full; only part of it is shown.
            </p>
          )}
          <Table aria-label="Files">
            <TableHeader>
              <TableRow>
                <TableHead>Name</TableHead>
                <TableHead>Size</TableHead>
                <TableHead>Modified</TableHead>
                <TableHead>Mode</TableHead>
                <TableHead>
                  <span className="sr-only">Download</span>
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {q.data.entries.length === 0 && (
                <TableRow>
                  <TableCell colSpan={5} className="text-muted-foreground">
                    This directory is empty.
                  </TableCell>
                </TableRow>
              )}
              {q.data.entries.map((e) => {
                const full = joinPath(path, e.name);
                return (
                  <TableRow key={e.name}>
                    <TableCell className="font-mono text-xs">
                      {e.kind === "dir" ? (
                        <Link
                          to={to}
                          params={params}
                          search={{ tab: "files", path: full }}
                          className="inline-flex items-center gap-2 hover:underline"
                        >
                          <Folder className="size-4 text-muted-foreground" />
                          {e.name}
                        </Link>
                      ) : (
                        <span className="inline-flex items-center gap-2">
                          {e.kind === "symlink" ? (
                            <Link2 className="size-4 text-muted-foreground" />
                          ) : (
                            <File className="size-4 text-muted-foreground" />
                          )}
                          {e.name}
                          {e.link_target && <span className="text-muted-foreground">→ {e.link_target}</span>}
                        </span>
                      )}
                    </TableCell>
                    <TableCell>{e.kind === "file" ? formatBytes(e.size) : ""}</TableCell>
                    <TableCell>{formatTime(e.modified)}</TableCell>
                    <TableCell className="font-mono text-xs">{formatMode(e.mode, e.kind)}</TableCell>
                    <TableCell>
                      {e.kind === "file" && (
                        <a
                          href={downloadUrl(env, id, full)}
                          download={e.name}
                          aria-label={`Download ${e.name}`}
                          className="text-muted-foreground hover:text-foreground"
                        >
                          <Download className="size-4" />
                        </a>
                      )}
                    </TableCell>
                  </TableRow>
                );
              })}
            </TableBody>
          </Table>
        </>
      )}
    </div>
  );
}

/** A plain GET the browser downloads itself; the session cookie authenticates it. */
export function downloadUrl(env: string, id: string, path: string) {
  const e = encodeURIComponent;
  return `/api/v1/envs/${e(env)}/containers/${e(id)}/files/download?path=${e(path)}`;
}
