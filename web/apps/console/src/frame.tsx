import { PROBE_INTERVAL_MS } from "@memex/core";
import { Button } from "@memex/ui";
import { useEffect } from "react";
import type { ReactNode } from "react";
import { useConnectivity, useEngine } from "./engine-context";

/** A hidden tab has nothing to warn about, so it stops asking, and asks at once when it's looked at again. */
const useConnectivityProbe = () => {
  const engine = useEngine();
  useEffect(() => {
    const tick = () => {
      if (!document.hidden) void engine.probe();
    };
    tick();
    const timer = setInterval(tick, PROBE_INTERVAL_MS);
    document.addEventListener("visibilitychange", tick);
    window.addEventListener("online", tick);
    return () => {
      clearInterval(timer);
      document.removeEventListener("visibilitychange", tick);
      window.removeEventListener("online", tick);
    };
  }, [engine]);
};

export const OfflineBanner = () => {
  const engine = useEngine();
  const connectivity = useConnectivity();
  return (
    // The live region stays mounted, so screen readers announce the change when it fills.
    <div role="status" aria-live="polite">
      {connectivity.status === "offline" && (
        <div data-testid="offline-banner" className="flex items-center justify-between gap-4 border-b border-warning bg-warning-surface px-4 py-2 text-sm text-warning">
          <span>Can't reach the server. Changes can't be saved, and this page will reconnect by itself.</span>
          <Button onClick={() => void engine.probe()}>Reconnect now</Button>
        </div>
      )}
    </div>
  );
};

/** Every signed-in and sign-in page sits in this: it watches the connection and shows the banner above the content. */
export const Frame = ({ children }: { readonly children: ReactNode }) => {
  useConnectivityProbe();
  return (
    <div className="flex h-dvh flex-col">
      <OfflineBanner />
      <div className="flex min-h-0 flex-1">{children}</div>
    </div>
  );
};

export const CenteredPage = ({ children }: { readonly children: ReactNode }) => (
  <main className="flex min-w-0 flex-1 items-center justify-center overflow-auto p-6">
    <div className="flex w-full max-w-96 flex-col gap-4">
      <h1 className="text-2xl font-medium">memexvault</h1>
      {children}
    </div>
  </main>
);
