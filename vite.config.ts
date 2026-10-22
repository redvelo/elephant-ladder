import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import process from "node:process";
const host = process.env.TAURI_DEV_HOST;
const readerSerifStylesheet = new URL(
  import.meta.resolve("epub-navigator/reader-fonts/reader-serif.css"),
);
const readerSerifCss = readFileSync(readerSerifStylesheet, "utf8");
const thirdPartyNotices = readFileSync(
  new URL("./THIRD_PARTY_NOTICES.md", import.meta.url),
  "utf8",
);
const readerSerifFiles = [...readerSerifCss.matchAll(/\.\/files\/([^)'"\s]+)/gu)].map(
  (match) => match[1]!,
);

function readerSerifAssets(): Plugin {
  const assets = new Map<string, string | Uint8Array>([
    ["reader-serif.css", readerSerifCss],
    ...readerSerifFiles.map(
      (file) =>
        [`files/${file}`, readFileSync(new URL(`files/${file}`, readerSerifStylesheet))] as const,
    ),
  ]);
  const digest = createHash("sha256");
  for (const [path, source] of [...assets].sort(([left], [right]) => left.localeCompare(right))) {
    digest.update(path).update("\0").update(source).update("\0");
  }
  const bundlePath = `/public/reader-fonts/${digest.digest("hex")}`;
  return {
    name: "reader-serif-assets",
    buildStart() {
      this.emitFile({
        type: "asset",
        fileName: "THIRD_PARTY_NOTICES.md",
        source: thirdPartyNotices,
      });
      for (const [path, source] of assets) {
        this.emitFile({ type: "asset", fileName: `${bundlePath.slice(1)}/${path}`, source });
      }
    },
    configureServer(server) {
      server.middlewares.use((request, response, next) => {
        const prefix = `${bundlePath}/`;
        const path = (request as { url?: string }).url?.split("?", 1)[0];
        if (path === undefined || !path.startsWith(prefix)) return next();
        const asset = assets.get(path.slice(prefix.length));
        if (asset === undefined) {
          response.statusCode = 404;
          response.end("not found");
          return;
        }
        response.setHeader(
          "Content-Type",
          path.endsWith(".css") ? "text/css; charset=utf-8" : "font/woff2",
        );
        response.setHeader("Access-Control-Allow-Origin", "*");
        response.setHeader("Cross-Origin-Resource-Policy", "cross-origin");
        response.end(asset);
      });
    },
  };
}

// https://vite.dev/config/
export default defineConfig(({ mode }) => ({
  define: {
    __ELEPHANT_FIXTURE_RUNNER__: JSON.stringify(mode === "fixture-runner"),
  },
  plugins: [tailwindcss(), react({ compiler: { logDiagnostics: true } }), readerSerifAssets()],
  resolve: {
    // Linked Navigator dependencies must share Elephant's React dispatcher and jotai store, and
    // the Base Rhea primitives Navigator exports must be the ones Elephant renders into: a second
    // copy resolves its own React and renders with a null dispatcher.
    dedupe: ["react", "react-dom", "jotai", "jotai-effect", "@base-ui/react"],
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    ...(host ? { hmr: { protocol: "ws", host, port: 1421 } } : {}),
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`, and keep it out of build caches:
      // the Flatpak builder's object store contains symlink loops that crash the watcher.
      ignored: ["**/src-tauri/**", "**/.flatpak-builder/**", "**/target/**"],
    },
  },
}));
