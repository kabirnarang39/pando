import "@fontsource-variable/schibsted-grotesk";
import "@fontsource/ibm-plex-mono/400.css";
import "@fontsource/ibm-plex-mono/500.css";
import "./styles.css";

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { RouterProvider } from "@tanstack/react-router";
import { StrictMode, useState } from "react";
import { createRoot } from "react-dom/client";
import { ConnectionProvider, useConnection } from "./connection";
import { router } from "./router";

/**
 * A fresh cache per connection, so two servers' data (or the demo's) never mix. The provider is
 * remounted by key: a memo would not do, because nothing inside it reads the key and the React
 * Compiler is free to keep one cache for ever.
 */
function App() {
  const { target } = useConnection();
  const key = target.kind === "demo" ? "demo" : `${target.url}|${target.token ?? ""}`;
  return <Scoped key={key} />;
}

function Scoped() {
  const [client] = useState(() => new QueryClient({ defaultOptions: { queries: { retry: 2, refetchOnWindowFocus: false } } }));
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
