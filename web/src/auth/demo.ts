import { useQuery } from "@tanstack/react-query";
import { authStatusQuery } from "@/api/queries";
import type { DemoLogin } from "@/api/gen/types.gen";

/** The public demo account, when this server is the hosted demo. */
export function useDemoLogin(): DemoLogin | null {
  const { data } = useQuery(authStatusQuery);
  return data?.demo ?? null;
}

/** Whether `username` is signed in as the shared demo account. */
export function useIsDemo(username: string): boolean {
  const demo = useDemoLogin();
  return !!demo && demo.username.toLowerCase() === username.toLowerCase();
}
