import { useState } from "react";
import type { MirrorBody, MirrorView } from "@/api/gen/types.gen";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";

/** A saved mirror sent back unchanged: the password stays as it is. */
export const keepMirror = (m: MirrorView | null | undefined): MirrorBody | null =>
  m ? { trivy_image: m.trivy_image, db_repository: m.db_repository, java_db_repository: m.java_db_repository, username: m.username } : null;

/** The mirror form's values; the password only when typed. */
export function useMirrorDraft(saved: MirrorView | null | undefined) {
  const [v, setV] = useState({
    trivy_image: saved?.trivy_image ?? "",
    db_repository: saved?.db_repository ?? "",
    java_db_repository: saved?.java_db_repository ?? "",
    username: saved?.username ?? "",
    password: "",
    clear: false,
  });
  const body = (): MirrorBody => ({
    trivy_image: v.trivy_image.trim() || null,
    db_repository: v.db_repository.trim() || null,
    java_db_repository: v.java_db_repository.trim() || null,
    username: v.username.trim() || null,
    ...(v.password ? { password: v.password } : {}),
    clear_password: v.clear,
  });
  return { v, setV, body, passwordSet: !!saved?.password_set && !v.clear };
}

type Draft = ReturnType<typeof useMirrorDraft>;

/** Where Trivy and its databases come from, for air-gapped environments. */
export function MirrorFields({ draft, idPrefix, defaultImage }: { draft: Draft; idPrefix: string; defaultImage: string }) {
  const { v, setV, passwordSet } = draft;
  const field = (key: "trivy_image" | "db_repository" | "java_db_repository" | "username", label: string, placeholder: string) => (
    <div className="grid gap-1.5">
      <Label htmlFor={`${idPrefix}-${key}`}>{label}</Label>
      <Input
        id={`${idPrefix}-${key}`}
        className="font-ident"
        value={v[key]}
        placeholder={placeholder}
        onChange={(e) => setV({ ...v, [key]: e.target.value })}
      />
    </div>
  );
  return (
    <div className="grid gap-3">
      {field("trivy_image", "Trivy image", defaultImage)}
      {field("db_repository", "Vulnerability database", "Trivy's public database")}
      {field("java_db_repository", "Java database", "Trivy's public Java database")}
      <div className="grid gap-3 sm:grid-cols-2">
        {field("username", "Username", "None")}
        <div className="grid gap-1.5">
          <div className="flex items-center gap-2">
            <Label htmlFor={`${idPrefix}-password`}>Password</Label>
            {passwordSet && <span className="rounded-full bg-step px-2 py-0.5 text-[11px] text-ink-muted">Saved</span>}
          </div>
          <Input
            id={`${idPrefix}-password`}
            type="password"
            autoComplete="new-password"
            value={v.password}
            placeholder={passwordSet ? "Leave empty to keep it" : "None"}
            onChange={(e) => setV({ ...v, password: e.target.value, clear: false })}
          />
          {passwordSet && (
            <Button type="button" size="sm" variant="ghost" className="justify-self-start" onClick={() => setV({ ...v, password: "", clear: true })}>
              Remove password
            </Button>
          )}
        </div>
      </div>
    </div>
  );
}
