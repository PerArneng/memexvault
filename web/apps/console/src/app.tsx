import { Gallery } from "./gallery";

// Routing arrives with spec 040. Until then the gallery is the only other view.
export const App = () =>
  window.location.pathname === "/gallery" ? (
    <Gallery />
  ) : (
    <main className="p-6">
      <h1 className="text-2xl font-medium">memexvault</h1>
    </main>
  );
