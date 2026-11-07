// The one composition root: it builds the edges and the engine, and nothing else may.
import { createEngine, settingsFrom } from "@memex/core";
import { browserDraftStorage, browserIds, browserPreferences, systemClock } from "@memex/edge-browser";
import { createFetchApi } from "@memex/edge-http";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import "./app.css";
import { App } from "./app";
import { EngineContext } from "./engine-context";

const settings = settingsFrom(() => undefined);
if (!settings.ok) throw new Error(settings.error);
const engine = createEngine({
  clock: systemClock,
  settings: settings.settings,
  api: createFetchApi({ apiBaseUrl: settings.settings.apiBaseUrl }),
  preferences: browserPreferences,
  drafts: browserDraftStorage,
  ids: browserIds,
});
void engine.restoreSession();

const root = document.getElementById("root");
if (root === null) throw new Error("missing #root");
createRoot(root).render(
  <StrictMode>
    <EngineContext.Provider value={engine}>
      <App />
    </EngineContext.Provider>
  </StrictMode>,
);
