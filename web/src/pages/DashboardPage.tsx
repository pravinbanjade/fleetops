import { useParams } from "@tanstack/react-router";

/** The environment's start page; its cards arrive in Task 7. */
export function DashboardPage() {
  const { env } = useParams({ strict: false });
  return (
    <div className="grid gap-4 p-6">
      <h1 className="text-2xl font-semibold">{env}</h1>
    </div>
  );
}
