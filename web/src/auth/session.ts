import { useMutation, useQuery } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { setCsrfToken } from "@/api/client";
import { meQuery } from "@/api/queries";
import { logout } from "@/api/gen/sdk.gen";

/** The logged-in session; only used under the authenticated routes, which load it first. */
export function useSession() {
  const { data } = useQuery(meQuery);
  if (!data) throw new Error("useSession outside the authenticated routes");
  return data;
}

export function useLogout() {
  const navigate = useNavigate();
  return useMutation({
    mutationFn: () => logout(),
    // The login page clears the cache once this page is gone (see LoginPage).
    onSettled: () => {
      setCsrfToken(undefined);
      void navigate({ to: "/login" });
    },
  });
}
