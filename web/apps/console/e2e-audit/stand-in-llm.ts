import { createServer } from "node:http";
import type { Server } from "node:http";

/**
 * A stand-in for a model's server on this machine, so audits and extraction run their real paths and nothing calls a provider. An
 * entailment check is answered `not_entailed` when the request carries `WRONG_MARK`, and `entailed` otherwise. Anything else is
 * answered as extraction, with one claim quoting the start of the first segment, which is also what a blind re-extraction gets. It
 * says nothing about what a model does.
 */
export const WRONG_MARK = "unsupported-by-the-quote";

export class StandInLlm {
  private server: Server | null = null;
  calls = 0;
  entailmentCalls = 0;

  start = () =>
    new Promise<string>((done) => {
      const server = createServer((req, res) => {
        const chunks: Buffer[] = [];
        req.on("data", (c: Buffer) => chunks.push(c));
        req.on("end", () => {
          this.calls += 1;
          const body = Buffer.concat(chunks).toString();
          res.writeHead(200, { "content-type": "text/event-stream" });
          res.end(this.reply(body));
        });
      });
      server.listen(0, "127.0.0.1", () => {
        const address = server.address();
        done(`http://127.0.0.1:${String(typeof address === "object" && address !== null ? address.port : 0)}/v1`);
      });
      this.server = server;
    });

  private reply(raw: string): string {
    let answer: string;
    if (raw.includes("not_entailed")) {
      this.entailmentCalls += 1;
      answer = JSON.stringify(raw.includes(WRONG_MARK) ? { verdict: "not_entailed", reason: "The quote doesn't say this." } : { verdict: "entailed", reason: "The quote says this." });
    } else {
      answer = JSON.stringify({ claims: this.claimsFrom(raw), entities: {} });
    }
    const chunk = (delta: unknown, finish: unknown, usage: unknown) => `data: ${JSON.stringify({ id: "1", object: "chat.completion.chunk", created: 1, model: "stand-in", choices: [{ index: 0, delta, finish_reason: finish }], usage })}\n\n`;
    return `${chunk({ role: "assistant", content: "" }, null, null)}${chunk({ content: answer }, null, null)}${chunk({}, "stop", { prompt_tokens: 40, completion_tokens: 10, total_tokens: 50 })}data: [DONE]\n\n`;
  }

  private claimsFrom(raw: string): readonly unknown[] {
    const request = JSON.parse(raw) as { messages?: readonly { role?: string; content?: unknown }[] };
    for (const message of [...(request.messages ?? [])].reverse()) {
      const content = typeof message.content === "string" ? message.content : JSON.stringify(message.content ?? "");
      const unwrapped = content.includes('{"segments"') ? content : content.replace(/\\"/g, '"');
      const from = unwrapped.indexOf('{"segments"');
      if (from < 0) continue;
      const first = this.segmentsIn(unwrapped.slice(from))[0];
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
