import { loginPath } from "@memex/core";
import { Button, Skeleton } from "@memex/ui";
import { useRouter } from "@tanstack/react-router";
import { lazy, Suspense, useEffect } from "react";
import { useEngine, useSession } from "./engine-context";
import { CenteredPage, Frame } from "./frame";

// Separate, so the sign-in page doesn't download what only a signed-in user needs (spec 040, verification 6).
const Shell = lazy(() => import("./shell/shell"));

const Waiting = () => (
  <Frame>
    <main className="flex min-w-0 flex-1 flex-col gap-3 p-6">
      <p role="status" className="sr-only">Loading</p>
      <Skeleton className="h-6 w-48" />
      <Skeleton className="h-40 w-full" />
    </main>
  </Frame>
);

/** Everything but the sign-in page and the gallery sits behind this. */
export const Guard = () => {
  const engine = useEngine();
  const session = useSession();
  const router = useRouter();

  useEffect(() => {
    if (session.status === "signed_out") {
      const here = window.location.pathname + window.location.search + window.location.hash;
      router.history.replace(session.reason === "signed_out" ? "/login" : loginPath(here));
    }
  }, [router, session]);

  if (session.status === "signed_in") {
    return (
      <Suspense fallback={<Waiting />}>
        <Shell />
      </Suspense>
    );
  }
  if (session.status === "unknown") {
    return (
      <Frame>
        <CenteredPage>
          <p role="alert" className="text-sm text-text-muted">Can't reach the server yet, so it isn't known whether you're signed in. This page will carry on by itself when it's back.</p>
          <Button onClick={() => void engine.probe()}>Try now</Button>
        </CenteredPage>
      </Frame>
    );
  }
  return <Waiting />;
};
