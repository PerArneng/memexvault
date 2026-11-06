/** Epoch milliseconds, so a raw number can't be mistaken for a duration. */
export type EpochMs = number & { readonly __brand: "EpochMs" };

export const epochMs = (value: number): EpochMs => value as EpochMs;
