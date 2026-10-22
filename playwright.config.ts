import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./tests/native-browser",
  // Each test drives its own application instance, so tests in one file have nothing to
  // share and spread across workers.
  fullyParallel: true,
  // An instance is a browser and a native host at once: more workers than this contend for
  // the machine and make the heavier tests slower, not faster.
  workers: 4,
  timeout: 300_000,
  expect: { timeout: 30_000 },
  outputDir: "test-results/native-browser",
  reporter: [["list"]],
});
