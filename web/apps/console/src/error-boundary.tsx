import { ErrorState } from "@memex/ui";
import { Component } from "react";
import type { ReactNode } from "react";

interface State {
  readonly failed: boolean;
}

/** The last line for a bug or a chunk that won't load. It says nothing from the error, which could quote vault content. */
export class ErrorBoundary extends Component<{ readonly children: ReactNode }, State> {
  override state: State = { failed: false };

  static getDerivedStateFromError(): State {
    return { failed: true };
  }

  override render() {
    return this.state.failed ? <CrashedPage /> : this.props.children;
  }
}

export const CrashedPage = () => (
  <main className="p-6">
    <h1 className="mb-4 text-2xl font-medium">memexvault</h1>
    <ErrorState title="Something went wrong on this page." detail="Reloading usually fixes it." onRetry={() => window.location.reload()} />
  </main>
);
