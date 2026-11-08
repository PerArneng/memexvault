import { createServer, request as httpRequest } from "node:http";
import type { IncomingMessage, Server, ServerResponse } from "node:http";
import type { Socket } from "node:net";
import { Transform } from "node:stream";

/** Answers a request itself when it returns true. */
export type Script = (req: IncomingMessage, res: ServerResponse) => boolean;

/**
 * Sits between the browser and the real server, so a test can cut the connection while both stay up, slow an upload down, or replace a route.
 * Only ever connects to `MEMEX_HTTP_BIND`, and nothing it does touches the server or Postgres.
 */
export class Proxy {
  private readonly sockets = new Set<Socket>();
  private server: Server | null = null;
  private refusing = false;
  private script: Script | null = null;
  private uploadBytesPerS: number | null = null;
  /** What each stream's request asked to resume from. */
  readonly resumes: string[] = [];

  constructor(private readonly target: string, private readonly port: number) {}

  start = () =>
    new Promise<void>((done) => {
      const server = createServer((req, res) => this.handle(req, res));
      server.on("connection", (socket) => {
        if (this.refusing) {
          socket.destroy();
          return;
        }
        this.sockets.add(socket);
        socket.on("close", () => this.sockets.delete(socket));
      });
      server.listen(this.port, "127.0.0.1", () => done());
      this.server = server;
    });

  private handle(req: IncomingMessage, res: ServerResponse) {
    if (req.url !== undefined && /\/activity(\?|$)/.test(req.url)) this.resumes.push(String(req.headers["last-event-id"] ?? ""));
    if (this.script?.(req, res) === true) return;
    const [host, port] = this.target.split(":");
    const upstream = httpRequest({ host, port: Number(port), path: req.url, method: req.method, headers: req.headers, agent: false }, (answer) => {
      res.writeHead(answer.statusCode ?? 502, answer.headers);
      answer.pipe(res);
      res.on("close", () => answer.destroy());
    });
    upstream.on("error", () => res.destroy());
    req.on("error", () => upstream.destroy());
    res.on("close", () => upstream.destroy());
    if (this.uploadBytesPerS !== null && req.method === "POST" && /\/sources\?/.test(req.url ?? "")) req.pipe(this.throttled(this.uploadBytesPerS)).pipe(upstream);
    else req.pipe(upstream);
  }

  /** Holds each chunk for as long as it would take at this rate, so an upload is still going when a test acts. */
  private throttled(bytesPerS: number) {
    return new Transform({
      highWaterMark: 16 * 1024,
      transform(chunk: Buffer, _encoding, done) {
        setTimeout(() => done(null, chunk), (chunk.length * 1000) / bytesPerS);
      },
    });
  }

  /** Cuts every connection now open. The browser sees them drop, and the server sees its clients go. */
  kill = () => {
    const count = this.sockets.size;
    for (const socket of this.sockets) socket.destroy();
    return count;
  };

  /** While set, new connections are dropped as they arrive, so it looks as if the network has gone. */
  refuse = (on: boolean) => {
    this.refusing = on;
  };

  setScript = (script: Script | null) => {
    this.script = script;
  };

  slowUploads = (bytesPerS: number | null) => {
    this.uploadBytesPerS = bytesPerS;
  };

  stop = () =>
    new Promise<void>((done) => {
      this.kill();
      if (this.server === null) done();
      else this.server.close(() => done());
    });
}
