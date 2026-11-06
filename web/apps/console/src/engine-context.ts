import type { Engine } from "@memex/core";
import { createContext } from "react";

/** React context carries the engine and nothing else. */
export const EngineContext = createContext<Engine | null>(null);
