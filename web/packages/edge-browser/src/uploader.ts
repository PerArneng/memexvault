import { failureFromWire, uploadResultFromWire } from "@memex/api";
import type { components } from "@memex/api";
import { failed, ok } from "@memex/core";
import type { ApiResult, UploadFile, Uploader } from "@memex/core";
import type { UploadedSource } from "@memex/types";

const parsed = (text: string): unknown => {
  try {
    return JSON.parse(text) as unknown;
  } catch {
    return null;
  }
};

/** XHR, as `fetch` still can't report how much of a request has gone. `apiBaseUrl` is same-origin, so the cookie and the CSP apply. */
export const createXhrUploader = ({ apiBaseUrl }: { readonly apiBaseUrl: string }): Uploader => ({
  start: (vault, file: UploadFile, options, onProgress) => {
    const xhr = new XMLHttpRequest();
    const query = new URLSearchParams({ name: file.name, trust_tier: options.trustTier });
    if (options.language !== null) query.set("language", options.language);
    const result = new Promise<ApiResult<UploadedSource>>((resolve) => {
      xhr.open("POST", `${apiBaseUrl}/vaults/${encodeURIComponent(vault)}/sources?${query.toString()}`);
      xhr.setRequestHeader("x-memex-csrf", "1");
      xhr.setRequestHeader("content-type", "application/octet-stream");
      xhr.upload.onprogress = (e) => onProgress(e.loaded);
      const noAnswer = (message: string) => resolve(failed({ kind: "unreachable", message }));
      xhr.onerror = () => noAnswer("the connection was lost");
      xhr.ontimeout = () => noAnswer("no answer in time");
      xhr.onabort = () => noAnswer("cancelled");
      xhr.onload = () => {
        const body = parsed(xhr.responseText);
        if (xhr.status >= 200 && xhr.status < 300 && body !== null) resolve(ok(uploadResultFromWire(body as components["schemas"]["UploadResult"])));
        else resolve(failed(failureFromWire(xhr.status, body, { get: (name) => xhr.getResponseHeader(name) })));
      };
      // The queue gets back the object it was given, which in a browser is a `File`.
      xhr.send(file as File);
    });
    return { result, abort: () => xhr.abort() };
  },
});
