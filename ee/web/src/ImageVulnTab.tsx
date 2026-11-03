import { useQuery } from "@tanstack/react-query";
import { TabsContent, TabsTrigger } from "@/components/ui/tabs";
import { DigestSection } from "./SecurityImagePage";
import { envImageQuery, useSecurityLive } from "./security";

export function ImageVulnTrigger() {
  return <TabsTrigger value="vulnerabilities">Vulnerabilities</TabsTrigger>;
}

function Panel({ env, imageId }: { env: string; imageId: string }) {
  useSecurityLive();
  const q = useQuery(envImageQuery(env, imageId));
  if (q.error) return <p className="text-sm text-ink-muted">This image has not been inventoried yet, or scanning is off for {env}.</p>;
  return (
    <div className="grid gap-5">
      {q.data?.digests.map((d) => (
        <DigestSection key={d.key} d={d} image={q.data.image} scanEnv={env} />
      ))}
    </div>
  );
}

/** The image page's Vulnerabilities tab. */
export function ImageVulnContent({ env, imageId }: { env: string; imageId: string }) {
  return (
    <TabsContent value="vulnerabilities">
      <Panel env={env} imageId={imageId} />
    </TabsContent>
  );
}
