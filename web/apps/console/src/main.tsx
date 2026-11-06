// The one composition root: it builds the edges and the engine, and nothing else may.
import { createEngine, settingsFrom } from "@memex/core";
import { systemClock } from "@memex/edge-browser";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "./app.css";
import { App } from "./app";
import { EngineContext } from "./engine-context";

const settings = settingsFrom(() => undefined);
if (!settings.ok) throw new Error(settings.error);
const engine = createEngine({ clock: systemClock, settings: settings.settings });

const root = document.getElementById("root");
if (root === null) throw new Error("missing #root");
createRoot(root).render(
  <StrictMode>
    <EngineContext.Provider value={engine}>
      <App />
    </EngineContext.Provider>
  </StrictMode>,
);
