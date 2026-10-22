/** The runner checks the shapes it is handed itself: it shares no code with the product. */
export function hasExactKeys(
  value: unknown,
  keys: ReadonlySet<string>,
): value is Record<string, unknown> {
  return (
    isRecord(value) &&
    Object.keys(value).length === keys.size &&
    Object.keys(value).every((key) => keys.has(key))
  );
}

export function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function isExactOrigin(value: string): boolean {
  try {
    const url = new URL(value);
    return (
      url.origin === value && url.protocol === "http:" && url.username === "" && url.password === ""
    );
  } catch {
    return false;
  }
}
