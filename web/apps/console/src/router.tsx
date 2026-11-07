import { createRootRoute, createRoute, createRouter, lazyRouteComponent, Link, Outlet } from "@tanstack/react-router";
import { EmptyState } from "@memex/ui";
import { CrashedPage } from "./error-boundary";
import { Guard } from "./guard";
import { LoginScreen } from "./login";

const NotFoundPage = () => (
  <main className="p-6">
    <h1 className="mb-4 text-2xl font-medium">memexvault</h1>
    <EmptyState title="There's no page at this address.">
      <Link to="/" className="text-accent underline">Go to the console</Link>
    </EmptyState>
  </main>
);

const root = createRootRoute({ component: Outlet });

const login = createRoute({ getParentRoute: () => root, path: "/login", component: LoginScreen });
const gallery = createRoute({ getParentRoute: () => root, path: "/gallery", component: lazyRouteComponent(() => import("./gallery"), "Gallery") });

// The shell reads its own location with `parseVaultPath`, since the same rules decide its links, its palette and its permission messages.
const home = createRoute({ getParentRoute: () => root, path: "/", component: Guard });
const vaults = createRoute({ getParentRoute: () => root, path: "/vaults/$", component: Guard });

const routeTree = root.addChildren([login, gallery, home, vaults]);

export const createAppRouter = () =>
  createRouter({
    routeTree,
    defaultNotFoundComponent: NotFoundPage,
    defaultErrorComponent: CrashedPage,
  });
