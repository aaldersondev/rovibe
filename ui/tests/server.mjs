// Starts the server the interface is tested against, from nothing: no
// project, no settings, no session left over from the run before.
import { spawn } from "node:child_process";
import fs from "node:fs";

const dataDir = process.env.ROVIBE_DATA_DIR;
fs.rmSync(dataDir, { recursive: true, force: true });
fs.mkdirSync(dataDir, { recursive: true });

const server = spawn("cargo", ["run", "-p", "rovibe-core", "--bin", "rovibe-cli", "--", "serve"], { cwd: "..", stdio: "inherit" });
server.on("exit", (code) => process.exit(code ?? 0));
for (const signal of ["SIGINT", "SIGTERM"]) process.on(signal, () => server.kill());
