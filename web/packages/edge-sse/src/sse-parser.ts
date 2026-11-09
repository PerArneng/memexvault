export interface SseMessage {
  readonly event: string;
  readonly data: string;
  readonly id: string | null;
}

/** The event-stream format, one chunk of text at a time. A chunk can end anywhere, including between `\r` and `\n`. */
export const createSseParser = (onMessage: (message: SseMessage) => void) => {
  let buffer = "";
  let event = "";
  let data: string[] = [];
  let id: string | null = null;

  const line = (text: string) => {
    if (text === "") {
      if (data.length > 0) onMessage({ event: event === "" ? "message" : event, data: data.join("\n"), id });
      event = "";
      data = [];
      id = null;
      return;
    }
    if (text.startsWith(":")) return;
    const colon = text.indexOf(":");
    const field = colon < 0 ? text : text.slice(0, colon);
    const raw = colon < 0 ? "" : text.slice(colon + 1);
    const value = raw.startsWith(" ") ? raw.slice(1) : raw;
    if (field === "event") event = value;
    else if (field === "data") data.push(value);
    else if (field === "id" && !value.includes("\0")) id = value;
  };

  return {
    feed: (chunk: string) => {
      buffer += chunk;
      let start = 0;
      for (;;) {
        const lf = buffer.indexOf("\n", start);
        const cr = buffer.indexOf("\r", start);
        const end = lf < 0 ? cr : cr < 0 ? lf : Math.min(lf, cr);
        if (end < 0) break;
        // A `\r` at the very end may be half of `\r\n`, so it waits for the next chunk.
        if (buffer[end] === "\r" && end + 1 === buffer.length) break;
        line(buffer.slice(start, end));
        start = buffer[end] === "\r" && buffer[end + 1] === "\n" ? end + 2 : end + 1;
      }
      buffer = buffer.slice(start);
    },
  };
};
