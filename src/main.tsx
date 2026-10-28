import React from "react";
import ReactDOM from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { FiltersProvider } from "./state/filters";
import { NavProvider } from "./state/nav";
import { ThemeProvider } from "./state/theme";
import App from "./App";
import TrayApp from "./screens/TrayApp";
import "./index.css";

const queryClient = new QueryClient({
  defaultOptions: { queries: { staleTime: 30_000, refetchOnWindowFocus: false } },
});

// The menu bar panel is a second window on the same page (tray.rs opens #tray).
const tray = window.location.hash === "#tray";

ReactDOM.createRoot(document.getElementById("root")!).render(
  tray ? (
    <React.StrictMode>
      <ThemeProvider>
        <TrayApp />
      </ThemeProvider>
    </React.StrictMode>
  ) : (
  <React.StrictMode>
    <ThemeProvider>
      <QueryClientProvider client={queryClient}>
        <FiltersProvider>
          <NavProvider>
            <App />
          </NavProvider>
        </FiltersProvider>
      </QueryClientProvider>
    </ThemeProvider>
  </React.StrictMode>
  ),
);
