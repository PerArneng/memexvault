import { QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider } from "@tanstack/react-router";
import { useState } from "react";
import { useEngine } from "./engine-context";
import { ErrorBoundary } from "./error-boundary";
import { createQueryClient } from "./query";
import { createAppRouter } from "./router";

export const App = () => {
  const engine = useEngine();
  const [queryClient] = useState(() => createQueryClient(engine));
  const [router] = useState(createAppRouter);
  return (
    <ErrorBoundary>
      <QueryClientProvider client={queryClient}>
        <RouterProvider router={router} />
      </QueryClientProvider>
    </ErrorBoundary>
  );
};
