import { type ChildProcess, spawn } from "node:child_process";

const origin = `http://${process.env.MEMEX_HTTP_BIND ?? ""}`;
const pause = (ms: number) => new Promise((done) => setTimeout(done, ms));
let child: ChildProcess | null = null;

const healthy = () => fetch(`${origin}/api/v1/health`).then((r) => r.ok).catch(() => false);

export const startServer = async () => {
  if (await healthy()) throw new Error(`something is already serving ${origin}, and these tests must be able to stop it`);
  child = spawn(process.env.MEMEX_E2E_BIN ?? "", ["serve"], { stdio: "ignore" });
  const started = Date.now();
  while (!(await healthy())) {
    if (child.exitCode !== null) throw new Error(`the server exited with ${child.exitCode}`);
    if (Date.now() - started > 90_000) throw new Error("the server didn't start in 90 s");
    await pause(100);
  }
  return Date.now() - started;
};

/** Only ever the server these tests started. */
export const stopServer = async () => {
  const running = child;
  child = null;
  if (running === null || running.exitCode !== null) return;
  const gone = new Promise((done) => running.once("exit", done));
  running.kill("SIGKILL");
  await gone;
};
