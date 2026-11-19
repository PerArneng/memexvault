// Zod 4, under @ag-ui/core's schemas, probes for eval with `Function("")` when each object schema is built. The console's CSP blocks
// eval, so the probe fails, which is caught, but the browser reports it as a CSP violation. Jitless parsing never probes, and this module
// is imported first so it runs before those schemas are built.
import { config } from "zod/v4";

config({ jitless: true });
