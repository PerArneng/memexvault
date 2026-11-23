import { createServer } from "node:http";
import type { Server, ServerResponse } from "node:http";

/**
 * Unified local OpenAI-compatible stand-in LLM on 127.0.0.1 for spec 051 acceptance.
 * Handles all three backend callers without any external network call:
 * - Chat (requests with `tools`): runs `search` tool, emits `[cite:<id>]`, and on `SCENARIO:propose` calls `propose`.
 * - Audit entailment (requests containing `not_entailed`): returns `entailed`.
 * - Extraction (default): quotes the opening words of the first segment in the request.
 */
export class StandInAll {
  private server: Server | null = null;
  calls = 0;
  chatQuery = "turbine";
  private proposals = 0;

  start = () =>
    new Promise<string>((done) => {
      const server = createServer((req, res) => {
        const chunks: Buffer[] = [];
        req.on("data", (c: Buffer) => chunks.push(c));
        req.on("end", () => {
          this.calls += 1;
          const raw = Buffer.concat(chunks).toString("utf8");
          let body: Record<string, unknown> = {};
          try {
            body = JSON.parse(raw) as Record<string, unknown>;
          } catch {
            // Fall through to extraction.
          }
          if (Array.isArray(body.tools) && body.tools.length > 0) {
            this.handleChat(body, res);
            return;
          }
          if (raw.includes("not_entailed")) {
            this.send(res, this.textChunks([JSON.stringify({ verdict: "entailed", reason: "Supported by the cited passage." })]));
            return;
          }
          const claims = this.claimsFrom(raw);
          this.send(res, this.textChunks([JSON.stringify({ claims, entities: {} })]));
        });
      });
      server.listen(0, "127.0.0.1", () => {
        const addr = server.address();
        const port = typeof addr === "object" && addr !== null ? addr.port : 0;
        done(`http://127.0.0.1:${String(port)}/v1`);
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

  private chunk(delta: unknown, finish: unknown = null, usage: unknown = null): string {
    return `data: ${JSON.stringify({ id: "1", object: "chat.completion.chunk", created: 1, model: "stand-in", choices: [{ index: 0, delta, finish_reason: finish }], usage })}\n\n`;
  }

  private textChunks(parts: readonly string[]): string {
    return `${this.chunk({ role: "assistant", content: "" })}${parts.map((p) => this.chunk({ content: p })).join("")}${this.chunk({}, "stop", { prompt_tokens: 30, completion_tokens: 15, total_tokens: 45 })}data: [DONE]\n\n`;
  }

  private toolChunks(calls: readonly { readonly name: string; readonly args: unknown }[], turn: number): string {
    const toolCalls = calls.map((c, index) => ({
      index,
      id: `call-${String(turn)}-${String(index)}`,
      type: "function",
      function: { name: c.name, arguments: JSON.stringify(c.args) },
    }));
    return `${this.chunk({ role: "assistant", tool_calls: toolCalls })}${this.chunk({}, "tool_calls", { prompt_tokens: 30, completion_tokens: 10, total_tokens: 40 })}data: [DONE]\n\n`;
  }

  private send(res: ServerResponse, body: string) {
    res.writeHead(200, { "content-type": "text/event-stream" });
    res.end(body);
  }

  private handleChat(body: Record<string, unknown>, res: ServerResponse) {
    const messages = (Array.isArray(body.messages) ? body.messages : []) as { role?: string; content?: unknown }[];
    let last = -1;
    messages.forEach((m, i) => {
      if (m.role === "user") last = i;
    });
    const question = String(messages[last]?.content ?? "");
    const results = messages
      .slice(last + 1)
      .filter((m) => m.role === "tool")
      .map((m) => (typeof m.content === "string" ? m.content : JSON.stringify(m.content ?? "")));
    const turn = messages.slice(last + 1).filter((m) => m.role === "assistant").length;
    const found = results.flatMap((r) => this.toolSegmentsIn(r));

    if (turn === 0) {
      this.send(res, this.toolChunks([{ name: "search", args: { q: this.chatQuery } }], turn));
      return;
    }
    if (question.includes("SCENARIO:propose") && turn === 1 && found.length > 0) {
      const seg = found[this.proposals % found.length];
      if (!seg) return;
      this.proposals += 1;
      const quote = seg.text.split(/\s+/).filter((w) => w !== "").slice(0, 6).join(" ");
      const ts = String(Date.now());
      const op = {
        op: "add_assertion",
        id: `acc-chat-claim-${ts}`,
        subject: "turbine valve",
        predicate: "inspection note",
        value: `verified: ${quote}`,
        epistemic: "asserted",
        machine: false,
        confidence_permille: 850,
        valid_from_ms: null,
        valid_to_ms: null,
        evidence: [[`acc-chat-ev-${ts}`, { segment_id: seg.id, quote, stance: "supports" }]],
      };
      this.send(res, this.toolChunks([{ name: "propose", args: { ops: [op], rationale: "Proposed from acceptance chat" } }], turn));
      return;
    }
    const cite = found[0]?.id ?? "none";
    this.send(res, this.textChunks([`Based on the vault records [cite:${cite}], I have queued the proposal for review.`]));
  }

  private toolSegmentsIn(content: string): { readonly id: string; readonly text: string }[] {
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
        if (o.kind === "segment" && typeof o.id === "string" && typeof o.text === "string") {
          out.push({ id: o.id, text: o.text });
        }
        Object.values(o).forEach(walk);
      }
    };
    walk(value);
    return out;
  }

  private claimsFrom(raw: string): readonly unknown[] {
    let request: { messages?: readonly { role?: string; content?: unknown }[] } = {};
    try {
      request = JSON.parse(raw) as typeof request;
    } catch {
      return [];
    }
    for (const message of [...(request.messages ?? [])].reverse()) {
      const content = typeof message.content === "string" ? message.content : JSON.stringify(message.content ?? "");
      const unwrapped = content.includes('{"segments"') ? content : content.replace(/\\"/g, '"');
      const from = unwrapped.indexOf('{"segments"');
      if (from < 0) continue;
      const first = this.segmentsIn(unwrapped.slice(from))[0];
      if (first === undefined) return [];
      const quote = first.text.split(/\s+/).filter((w) => w !== "").slice(0, 5).join(" ");
      return first.text.includes(quote) && quote !== ""
        ? [{ segment_id: first.id, quote, subject: "the document", predicate: "states", value: quote }]
        : [];
    }
    return [];
  }

  private segmentsIn(text: string): readonly { readonly id: string; readonly text: string }[] {
    for (let end = text.lastIndexOf("}"); end > 0; end = text.lastIndexOf("}", end - 1)) {
      try {
        const parsed = JSON.parse(text.slice(0, end + 1)) as { segments?: readonly { id: string; text: string }[] };
        return parsed.segments ?? [];
      } catch {
        // Keep scanning backwards for valid JSON end.
      }
    }
    return [];
  }
}
