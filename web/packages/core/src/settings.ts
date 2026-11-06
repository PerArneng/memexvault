export interface Settings {
  readonly apiBaseUrl: string;
  readonly activityBufferEvents: number;
}

export const DEFAULT_SETTINGS: Settings = { apiBaseUrl: "/api/v1", activityBufferEvents: 5000 };

export type Lookup = (name: string) => string | undefined;

export type SettingsResult =
  | { readonly ok: true; readonly settings: Settings }
  | { readonly ok: false; readonly error: string };

/** Pure: only main.tsx reads the runtime config, and passes it in as a lookup. */
export const settingsFrom = (lookup: Lookup): SettingsResult => {
  const apiBaseUrl = lookup("MEMEX_API_BASE_URL") ?? DEFAULT_SETTINGS.apiBaseUrl;
  if (!apiBaseUrl.startsWith("/")) {
    return { ok: false, error: `MEMEX_API_BASE_URL must be a same-origin path: ${apiBaseUrl}` };
  }
  const rawBuffer = lookup("MEMEX_ACTIVITY_BUFFER_EVENTS");
  const activityBufferEvents = rawBuffer === undefined ? DEFAULT_SETTINGS.activityBufferEvents : Number(rawBuffer);
  if (!Number.isInteger(activityBufferEvents) || activityBufferEvents < 1) {
    return { ok: false, error: `MEMEX_ACTIVITY_BUFFER_EVENTS must be a positive integer: ${rawBuffer ?? ""}` };
  }
  return { ok: true, settings: { apiBaseUrl, activityBufferEvents } };
};
