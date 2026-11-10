import { createServer } from "node:http";
import type { Server } from "node:http";

/**
 * A stand-in for a model's server on this machine, so extraction runs on the real ADK runner and nothing calls a provider or leaves. It always proposes
 * one claim quoting the start of the first segment it's given. It says nothing about what a model does.
 */
export class StandInLlm {
  private server: Server | null = null;
  /** When false, it answers as a provider that's away does, which is what puts a job in waiting. */
  up = true;
  calls = 0;
  /** Long enough for a person, or a test, to see a document in extraction. */
  delayMs = 0;

  start = () =>
    new Promise<string>((done) => {
      const server = createServer((req, res) => {
        const chunks: Buffer[] = [];
        req.on("data", (c: Buffer) => chunks.push(c));
        req.on("end", () => {
          this.calls += 1;
          if (!this.up) {
            res.writeHead(503, { "content-type": "application/json" });
            res.end(JSON.stringify({ error: { message: "the stand-in is away" } }));
            return;
          }
          const body = Buffer.concat(chunks).toString();
          setTimeout(() => {
            res.writeHead(200, { "content-type": "text/event-stream" });
            res.end(this.reply(body));
          }, this.delayMs);
        });
      });
      server.listen(0, "127.0.0.1", () => {
        const address = server.address();
        done(`http://127.0.0.1:${String(typeof address === "object" && address !== null ? address.port : 0)}/v1`);
      });
      this.server = server;
    });

  private reply(raw: string): string {
    const answer = JSON.stringify({ claims: this.claimsFrom(raw), entities: {} });
    const chunk = (delta: unknown, finish: unknown, usage: unknown) => `data: ${JSON.stringify({ id: "1", object: "chat.completion.chunk", created: 1, model: "stand-in", choices: [{ index: 0, delta, finish_reason: finish }], usage })}\n\n`;
    return `${chunk({ role: "assistant", content: "" }, null, null)}${chunk({ content: answer }, null, null)}${chunk({}, "stop", { prompt_tokens: 10, completion_tokens: 10, total_tokens: 20 })}data: [DONE]\n\n`;
  }

  private claimsFrom(raw: string): readonly unknown[] {
    const request = JSON.parse(raw) as { messages?: readonly { role?: string; content?: unknown }[] };
    for (const message of [...(request.messages ?? [])].reverse()) {
      const content = typeof message.content === "string" ? message.content : JSON.stringify(message.content ?? "");
      const start = content.indexOf('{"segments"');
      const unwrapped = start >= 0 ? content : content.replace(/\\"/g, '"');
      const from = unwrapped.indexOf('{"segments"');
      if (from < 0) continue;
      const segments = this.segmentsIn(unwrapped.slice(from));
      const first = segments[0];
      if (first === undefined) return [];
      const quote = first.text.split(/\s+/).filter((w) => w !== "").slice(0, 4).join(" ");
      return first.text.includes(quote) && quote !== "" ? [{ segment_id: first.id, quote, subject: "the document", predicate: "says", value: quote }] : [];
    }
    return [];
  }

  private segmentsIn(text: string): readonly { readonly id: string; readonly text: string }[] {
    for (let end = text.lastIndexOf("}"); end > 0; end = text.lastIndexOf("}", end - 1)) {
      try {
        const parsed = JSON.parse(text.slice(0, end + 1)) as { segments?: readonly { id: string; text: string }[] };
        return parsed.segments ?? [];
      } catch {
        // Not the end of that object yet.
      }
    }
    return [];
  }

  stop = () =>
    new Promise<void>((done) => {
      if (this.server === null) done();
      else {
        this.server.closeAllConnections();
        this.server.close(() => done());
      }
    });
}
