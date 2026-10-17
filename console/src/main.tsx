import "@fontsource-variable/schibsted-grotesk";
import "@fontsource/ibm-plex-mono/400.css";
import "@fontsource/ibm-plex-mono/500.css";
import "./styles.css";

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider } from "@tanstack/react-router";
import { StrictMode, useMemo } from "react";
import { createRoot } from "react-dom/client";
import { ConnectionProvider, useConnection } from "./connection";
import { router } from "./router";

/** A fresh cache per connection, so two servers' data never mix. */
function App() {
  const { target } = useConnection();
  const key = target.kind === "demo" ? "demo" : `${target.url}|${target.token ?? ""}`;
  const client = useMemo(
    () => new QueryClient({ defaultOptions: { queries: { retry: 2, refetchOnWindowFocus: false } } }),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [key],
  );
  return (
    <QueryClientProvider client={client}>
      <RouterProvider router={router} />
    </QueryClientProvider>
  );
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <ConnectionProvider>
      <App />
    </ConnectionProvider>
  </StrictMode>,
);
