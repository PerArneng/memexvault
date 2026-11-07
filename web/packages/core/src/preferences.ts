/** Small per-browser conveniences. Losing one must never break anything, so a read can always come back empty. */
export interface Preferences {
  get(key: string): string | null;
  set(key: string, value: string): void;
}

export const LAST_VAULT_KEY = "memex.lastVault";
