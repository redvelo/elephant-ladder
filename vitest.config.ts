import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

export default defineConfig({
  plugins: [react({ compiler: { logDiagnostics: true } })],
  resolve: {
    // Linked Navigator dependencies must share Elephant's React dispatcher and jotai store, and
    // the Base Rhea primitives Navigator exports must be the ones Elephant renders into: a second
    // copy resolves its own React and renders with a null dispatcher.
    dedupe: ["react", "react-dom", "jotai", "jotai-effect", "@base-ui/react"],
  },
  test: {
    environment: "node",
    include: ["tests/**/*.test.{ts,tsx}"],
    // Externalized dependencies resolve React beside themselves, bypassing the dedupe above.
    server: { deps: { inline: true } },
  },
});
