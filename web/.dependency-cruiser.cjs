/** The layers from docs/frontend-architecture.md. */
const pure = "^packages/(types|core|api)/";
module.exports = {
  forbidden: [
    { name: "pure-no-edge-ui-app", severity: "error", from: { path: pure }, to: { path: "^(packages/(edge-[^/]+|ui)|apps)/" } },
    { name: "pure-no-react", severity: "error", from: { path: pure }, to: { path: "(^|/)(react|react-dom)(/|$)" } },
    { name: "types-is-a-leaf", severity: "error", from: { path: "^packages/types/" }, to: { path: "^packages/(?!types/)" } },
    { name: "edge-only-to-pure", severity: "error", from: { path: "^packages/edge-" }, to: { path: "^(packages/ui|apps)/" } },
    { name: "edges-not-sideways", severity: "error", from: { path: "^packages/(edge-[^/]+)/" }, to: { path: "^packages/edge-[^/]+/", pathNot: "^packages/$1/" } },
    { name: "ui-no-data-access", severity: "error", from: { path: "^packages/ui/" }, to: { path: "^(packages/(core|api|edge-[^/]+)|apps)/" } },
    { name: "only-main-builds-edges", severity: "error", from: { path: "^apps/console/src/", pathNot: "^apps/console/src/main\\.tsx$" }, to: { path: "^packages/edge-" } },
    { name: "ag-ui-only-in-edge-agui", severity: "error", from: { pathNot: "^packages/edge-agui/" }, to: { path: "node_modules/.*/(@ag-ui|rxjs)/" } },
  ],
  options: {
    doNotFollow: { path: "node_modules" },
    exclude: { path: "(dist|\\.d\\.ts$)" },
    tsConfig: { fileName: "tsconfig.base.json" },
    enhancedResolveOptions: { exportsFields: ["exports"], conditionNames: ["import", "default"], extensions: [".ts", ".tsx", ".js"] },
  },
};
