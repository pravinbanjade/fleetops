import { MutationCache, QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider, type RouterHistory } from "@tanstack/react-router";
import { useState } from "react";
import { Toaster } from "sonner";
import { configureApi } from "@/api/client";
import { keys } from "@/api/queries";
import { toastError } from "@/lib/errors";
import { createAppRouter } from "@/router";
import { ThemeProvider, useTheme } from "@/theme";

/**
 * Mutations with no error handling of their own (no `onError`, and not
 * shown inline via `meta.inline`) report failures as a toast.
 */
export function createQueryClient() {
  return new QueryClient({
    defaultOptions: { queries: { retry: 1, refetchOnWindowFocus: false } },
    mutationCache: new MutationCache({
      onError: (err, _vars, _ctx, mutation) => {
        if (!mutation.options.onError && !mutation.meta?.inline) toastError(err);
      },
    }),
  });
}

export function createApp(history?: RouterHistory) {
  const queryClient = createQueryClient();
  const router = createAppRouter(queryClient, history);
  configureApi({
    onUnauthorized: () => {
      queryClient.removeQueries({ queryKey: keys.me });
      const here = router.state.location.href;
      void router.navigate({ to: "/login", search: { redirect: here } });
    },
  });
  return { queryClient, router };
}

export function App({ history }: { history?: RouterHistory }) {
  const [{ queryClient, router }] = useState(() => createApp(history));
  return (
    <ThemeProvider>
      <QueryClientProvider client={queryClient}>
        <RouterProvider router={router} />
        <ThemedToaster />
      </QueryClientProvider>
    </ThemeProvider>
  );
}

function ThemedToaster() {
  const { theme } = useTheme();
  return <Toaster theme={theme} richColors closeButton />;
}
