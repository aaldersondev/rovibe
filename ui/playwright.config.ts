import os from "node:os";
import path from "node:path";

import { defineConfig } from "@playwright/test";

// The interface is tested against a real server, started for the occasion
// on a port and a settings folder of its own: nothing of the user's is read
// or written. Build the interface first (`npm run build`): the server embeds it.
const port = 34990;
export const dataDir = path.join(os.tmpdir(), "rovibe-ui-tests");

export default defineConfig({
  testDir: "tests",
  timeout: 60_000,
  // One server, one set of sessions: the scenarios share them in order.
  workers: 1,
  fullyParallel: false,
  reporter: [["list"]],
  use: {
    baseURL: `http://127.0.0.1:${port}`,
    viewport: { width: 1440, height: 900 },
    locale: "fr-FR",
  },
  webServer: {
    command: "node tests/server.mjs",
    url: `http://127.0.0.1:${port}/api/ping`,
    reuseExistingServer: false,
    timeout: 600_000,
    env: { ROVIBE_PORT: String(port), ROVIBE_DATA_DIR: dataDir },
  },
});
