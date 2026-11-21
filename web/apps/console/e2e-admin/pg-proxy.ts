import { connect, createServer } from "node:net";
import type { Server, Socket } from "node:net";

/**
 * A TCP proxy between the server and Postgres, so a test can cut Postgres off while both keep running. Only ever forwards to the
 * address it was given, and touches nothing in Postgres.
 */
export class PgProxy {
  private readonly sockets = new Set<Socket>();
  private server: Server | null = null;
  private refusing = false;

  constructor(private readonly target: { readonly host: string; readonly port: number }, readonly port: number) {}

  start = () =>
    new Promise<void>((done) => {
      const server = createServer((client) => {
        if (this.refusing) {
          client.destroy();
          return;
        }
        const upstream = connect(this.target.port, this.target.host);
        for (const s of [client, upstream]) {
          this.sockets.add(s);
          s.on("close", () => this.sockets.delete(s));
          s.on("error", () => undefined);
        }
        client.pipe(upstream);
        upstream.pipe(client);
        client.on("close", () => upstream.destroy());
        upstream.on("close", () => client.destroy());
      });
      server.listen(this.port, "127.0.0.1", () => done());
      this.server = server;
    });

  /** Drops every open connection and refuses new ones, as a stopped Postgres would. */
  cut = () => {
    this.refusing = true;
    for (const s of this.sockets) s.destroy();
  };

  restore = () => {
    this.refusing = false;
  };

  stop = () =>
    new Promise<void>((done) => {
      this.cut();
      if (this.server === null) done();
      else this.server.close(() => done());
    });
}
