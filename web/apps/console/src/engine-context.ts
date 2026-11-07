import type { ConnectivityState, Engine, Notice, SessionState } from "@memex/core";
import { createContext, use, useSyncExternalStore } from "react";

/** React context carries the engine and nothing else. */
export const EngineContext = createContext<Engine | null>(null);

export const useEngine = (): Engine => {
  const engine = use(EngineContext);
  if (engine === null) throw new Error("no engine: main.tsx provides it");
  return engine;
};

export const useSession = (): SessionState => {
  const engine = useEngine();
  return useSyncExternalStore(engine.subscribe, engine.session);
};

export const useConnectivity = (): ConnectivityState => {
  const engine = useEngine();
  return useSyncExternalStore(engine.subscribe, engine.connectivity);
};

export const useNotices = (): readonly Notice[] => {
  const engine = useEngine();
  return useSyncExternalStore(engine.subscribe, engine.notices);
};
