import { createServer } from "node:http";
import type { Server, ServerResponse } from "node:http";

/**
 * A stand-in for a model's OpenAI-compatible server on this machine, so chat runs on the real ADK runner and the real tools, and nothing
 * calls a provider. It follows a script named in the person's message, such as `SCENARIO:cite10`. It says nothing about what a model does.
 * Extraction's requests, which carry no tools, get no claims.
 */
export class StandInChat {
  private server: Server | null = null;
  /** Each chat request's body, in order. */
  readonly requests: unknown[] = [];
  /** When a streaming reply's client went away, in ms since the epoch: the server closing the upstream on a cancel. */
  readonly closedAt: number[] = [];
  /** For `cite10`: the segments it reads and cites. */
  citeSegments: readonly string[] = [];
  /** What `basic`, `propose` and `hostile` search for. */
  query = "";
  /** For `hostile`: the reply, which the test writes with the canary's address in it. */
  hostileReply = "";

  start = () =>
    new Promise<string>((done) => {
      const server = createServer((req, res) => {
        const chunks: Buffer[] = [];
        req.on("data", (c: Buffer) => chunks.push(c));
        req.on("end", () => {
          let body: Record<string, unknown> = {};
          try {
            body = JSON.parse(Buffer.concat(chunks).toString()) as Record<string, unknown>;
          } catch {
            // Answered as extraction below.
          }
          const tools = Array.isArray(body.tools) && body.tools.length > 0;
          if (!tools) {
            this.send(res, [this.textChunks([JSON.stringify({ claims: [], entities: {} })])]);
            return;
          }
          this.requests.push(body);
          this.chat(body, res);
        });
      });
      server.listen(0, "127.0.0.1", () => {
        const address = server.address();
        done(`http://127.0.0.1:${String(typeof address === "object" && address !== null ? address.port : 0)}/v1`);
      });
      this.server = server;
    });

  stop = () =>
    new Promise<void>((done) => {
      if (this.server === null) done();
      else {
        this.server.closeAllConnections();
        this.server.close(() => done());
      }
    });

  private chunk(delta: unknown, finish: unknown = null, usage: unknown = null) {
    return `data: ${JSON.stringify({ id: "1", object: "chat.completion.chunk", created: 1, model: "stand-in-chat", choices: [{ index: 0, delta, finish_reason: finish }], usage })}\n\n`;
  }

  private textChunks(parts: readonly string[]): string {
    return `${this.chunk({ role: "assistant", content: "" })}${parts.map((p) => this.chunk({ content: p })).join("")}${this.chunk({}, "stop", { prompt_tokens: 40, completion_tokens: 20, total_tokens: 60 })}data: [DONE]\n\n`;
  }

  private toolChunks(calls: readonly { readonly name: string; readonly args: unknown }[], turn: number): string {
    const toolCalls = calls.map((c, index) => ({ index, id: `call-${String(turn)}-${String(index)}`, type: "function", function: { name: c.name, arguments: JSON.stringify(c.args) } }));
    return `${this.chunk({ role: "assistant", tool_calls: toolCalls })}${this.chunk({}, "tool_calls", { prompt_tokens: 40, completion_tokens: 10, total_tokens: 50 })}data: [DONE]\n\n`;
  }

  private send(res: ServerResponse, parts: readonly string[]) {
    res.writeHead(200, { "content-type": "text/event-stream" });
    res.end(parts.join(""));
  }

  /** The person's question is the last user message, as ADK sends its instruction as one too, before the history. */
  private chat(body: Record<string, unknown>, res: ServerResponse) {
    const messages = (Array.isArray(body.messages) ? body.messages : []) as { role?: string; content?: unknown }[];
    let last = -1;
    messages.forEach((m, i) => {
      if (m.role === "user") last = i;
    });
    const question = String(messages[last]?.content ?? "");
    const results = messages.slice(last + 1).filter((m) => m.role === "tool").map((m) => (typeof m.content === "string" ? m.content : JSON.stringify(m.content ?? "")));
    const turn = messages.slice(last + 1).filter((m) => m.role === "assistant").length;
    const scenario = /SCENARIO:(\w+)/.exec(question)?.[1] ?? "";
    const found = results.flatMap((r) => this.segmentsIn(r));
    switch (scenario) {
      case "basic":
        if (turn === 0) return this.send(res, [this.toolChunks([{ name: "search", args: { q: this.query } }], turn)]);
        return this.send(res, [this.textChunks(["The span ", `is described here [cite:${found[0]?.id ?? "none"}]`, `. Another passage says more [cite:${found[1]?.id ?? found[0]?.id ?? "none"}]. `, "This one cites nothing that was returned [cite:0000000000000000000000000000dead]."])]);
      case "cite10":
        if (turn === 0) return this.send(res, [this.toolChunks(this.citeSegments.map((id) => ({ name: "read", args: { kind: "segment", id } })), turn)]);
        return this.send(res, [this.textChunks(this.citeSegments.map((id, i) => `Point ${String(i + 1)} is on its page [cite:${id}]. `))]);
      case "propose": {
        if (turn === 0) return this.send(res, [this.toolChunks([{ name: "search", args: { q: this.query } }], turn)]);
        const first = found[0];
        if (turn === 1 && first !== undefined) {
          const quote = first.text.split(/\s+/).filter((w) => w !== "").slice(0, 6).join(" ");
          const op = { op: "add_assertion", id: `chat-claim-${String(Date.now())}`, subject: "the span", predicate: "is described as", value: quote, epistemic: "asserted", machine: false, confidence_permille: 800, valid_from_ms: null, valid_to_ms: null, evidence: [[`chat-ev-${String(Date.now())}`, { segment_id: first.id, quote, stance: "supports" }]] };
          return this.send(res, [this.toolChunks([{ name: "propose", args: { ops: [op], rationale: "the document says so" } }], turn)]);
        }
        return this.send(res, [this.textChunks([`I queued that for review [cite:${first?.id ?? "none"}].`])]);
      }
      case "hostile":
        if (turn === 0) return this.send(res, [this.toolChunks([{ name: "search", args: { q: this.query } }], turn)]);
        return this.send(res, [this.textChunks([this.hostileReply.slice(0, 40), this.hostileReply.slice(40)])]);
      case "slow":
        return this.slow(res);
      default:
        return this.send(res, [this.textChunks(["I found nothing in the vault about this."])]);
    }
  }

  /** One word every 100 ms for up to 60 s, which a person stops. */
  private slow(res: ServerResponse) {
    res.writeHead(200, { "content-type": "text/event-stream" });
    res.write(this.chunk({ role: "assistant", content: "" }));
    let n = 0;
    let open = true;
    res.on("close", () => {
      if (open) this.closedAt.push(Date.now());
      open = false;
    });
    const tick = () => {
      if (!open) return;
      n += 1;
      if (n > 600) {
        open = false;
        res.end(`${this.chunk({}, "stop", { prompt_tokens: 40, completion_tokens: 600, total_tokens: 640 })}data: [DONE]\n\n`);
        return;
      }
      res.write(this.chunk({ content: `word${String(n)} ` }));
      setTimeout(tick, 100);
    };
    tick();
  }

  /** The segments a tool's answer names, with their text when it has one. Its content may arrive as a JSON string inside JSON. */
  private segmentsIn(content: string): { readonly id: string; readonly text: string }[] {
    let value: unknown = content;
    for (let i = 0; i < 3 && typeof value === "string"; i += 1) {
      try {
        value = JSON.parse(value);
      } catch {
        break;
      }
    }
    const out: { id: string; text: string }[] = [];
    const walk = (v: unknown) => {
      if (Array.isArray(v)) v.forEach(walk);
      else if (typeof v === "object" && v !== null) {
        const o = v as Record<string, unknown>;
        if (o.kind === "segment" && typeof o.id === "string" && typeof o.text === "string") out.push({ id: o.id, text: o.text });
        Object.values(o).forEach(walk);
      }
    };
    walk(value);
    return out;
  }
}

/** Records every request that reaches it. Hostile content points here, so any hit is something loaded from outside. */
export class Canary {
  private server: Server | null = null;
  readonly hits: string[] = [];
  port = 0;

  start = () =>
    new Promise<void>((done) => {
      const server = createServer((req, res) => {
        this.hits.push(`${req.method ?? ""} ${req.url ?? ""}`);
        res.writeHead(200, { "content-type": "text/plain" });
        res.end("canary");
      });
      server.listen(0, "127.0.0.1", () => {
        const address = server.address();
        this.port = typeof address === "object" && address !== null ? address.port : 0;
        done();
      });
      this.server = server;
    });

  stop = () =>
    new Promise<void>((done) => {
      if (this.server === null) done();
      else {
        this.server.closeAllConnections();
        this.server.close(() => done());
      }
    });
}
