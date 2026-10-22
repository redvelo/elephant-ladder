export interface RecoveryHistory {
  replaceState(data: unknown, unused: string, url?: string | URL | null): void;
}

export interface BootstrapElement {
  remove(): void;
}

export function isNavigationPath(value: unknown): value is string {
  return typeof value === "string" && /^\/window\/[0-9a-f]{64}\/start$/.test(value);
}

export function commitBootstrapRecovery(
  element: BootstrapElement,
  recoveryNavigationPath: unknown,
  navigationHistory: RecoveryHistory = history,
): void {
  if (!isNavigationPath(recoveryNavigationPath)) {
    throw new Error("Native bootstrap recovery path is invalid");
  }
  element.remove();
  navigationHistory.replaceState(null, "", recoveryNavigationPath);
}
