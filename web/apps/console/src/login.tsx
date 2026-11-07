import { safeNext, signInErrorText } from "@memex/core";
import { Button, Input } from "@memex/ui";
import { useRouter, useSearch } from "@tanstack/react-router";
import { useEffect, useState } from "react";
import type { FormEvent } from "react";
import { useEngine, useSession } from "./engine-context";
import { CenteredPage, Frame } from "./frame";

const REASONS: Readonly<Record<string, string>> = {
  expired: "Your session has ended. Sign in again to carry on where you were.",
  signed_out: "You're signed out.",
};

export const LoginScreen = () => {
  const engine = useEngine();
  const session = useSession();
  const router = useRouter();
  const next = safeNext((useSearch({ strict: false }) as { readonly next?: unknown }).next);
  const [token, setToken] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (session.status === "signed_in") router.history.replace(next);
  }, [next, router, session.status]);

  const reason = session.status === "signed_out" ? REASONS[session.reason] : undefined;
  const submit = async (e: FormEvent) => {
    e.preventDefault();
    if (token.trim() === "") {
      setError("Paste an access token first.");
      return;
    }
    setBusy(true);
    setError(null);
    const result = await engine.signIn(token.trim());
    setBusy(false);
    // The token goes no further than this form, and the cookie takes over.
    setToken("");
    if (!result.ok) setError(signInErrorText(result.failure));
  };

  return (
    <Frame>
      <CenteredPage>
        {reason !== undefined && error === null && <p role="status" className="text-sm text-text-muted">{reason}</p>}
        <form onSubmit={(e) => void submit(e)} className="flex flex-col gap-3">
          <Input
            label="Access token"
            type="password"
            autoComplete="off"
            spellCheck={false}
            autoFocus={true}
            value={token}
            onChange={(e) => setToken(e.target.value)}
            {...(error === null ? {} : { error })}
          />
          <Button type="submit" variant="primary" disabled={busy} className="justify-center">
            {busy ? "Signing in" : "Sign in"}
          </Button>
        </form>
        <p className="text-xs text-text-muted">
          The token is exchanged once for a session cookie that page scripts can't read. It isn't kept in this browser.
        </p>
      </CenteredPage>
    </Frame>
  );
};
