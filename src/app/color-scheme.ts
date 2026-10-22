import type { NavigatorReaderPreferences } from "epub-navigator/react";

/** The reader's interface choice themes the whole application, not only the reading surface. */
export function applyApplicationColorScheme(
  colorScheme: NavigatorReaderPreferences["colorScheme"],
): void {
  document.querySelector(".elephant-app")?.setAttribute("data-navigator-color-scheme", colorScheme);
}
