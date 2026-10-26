import { Eye, EyeOff } from "lucide-react";
import { useMutation } from "@tanstack/react-query";
import { revealSecret } from "@/api/gen/sdk.gen";
import { Button } from "@/components/ui/button";

/**
 * A Secret's real values, fetched only on request (each request is audited)
 * and kept only while this page is open.
 */
export function SecretValues({ env, ns, name }: { env: string; ns: string; name: string }) {
  const reveal = useMutation({
    meta: { inline: true },
    mutationFn: async () => (await revealSecret({ path: { env, kind: "secrets", ns, name } })).data,
  });
  if (!reveal.data) {
    return (
      <div className="flex items-center gap-3">
        <Button variant="outline" size="sm" disabled={reveal.isPending} onClick={() => reveal.mutate()}>
          <Eye aria-hidden />
          Show values
        </Button>
        <span className="text-xs text-muted-foreground">Each time values are shown, it is written to the audit log.</span>
        {reveal.error && <span className="text-sm text-destructive">{reveal.error.message}</span>}
      </div>
    );
  }
  return (
    <section aria-label="Secret values" className="grid gap-2">
      <div className="flex items-center justify-between">
        <h3 className="text-sm font-medium">Values</h3>
        <Button variant="outline" size="sm" onClick={() => reveal.reset()}>
          <EyeOff aria-hidden />
          Hide values
        </Button>
      </div>
      {reveal.data.length === 0 ? (
        <p className="text-sm text-muted-foreground">This Secret has no data.</p>
      ) : (
        <dl className="grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1 rounded-md bg-[#1c1c1c] p-3 text-sm">
          {reveal.data.map((v) => (
            <div key={v.key} className="contents">
              <dt className="font-ident text-muted-foreground">{v.key}</dt>
              <dd className="break-all font-mono text-xs">
                {v.value}
                {v.binary && <span className="ml-2 text-muted-foreground">(base64)</span>}
              </dd>
            </div>
          ))}
        </dl>
      )}
    </section>
  );
}
