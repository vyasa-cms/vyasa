/**
 * Generates the typed API client from the running server's OpenAPI
 * document. Falls back to a checked-in snapshot when the server is not
 * running.
 *
 * Usage:
 *   cargo run -p vyasa-api -- serve   # in another terminal
 *   pnpm gen:api
 */
import { execFileSync } from "node:child_process";
import { mkdirSync, writeFileSync, existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const root = join(__dirname, "..");
const snapshot = join(root, "openapi.json");
const output = join(root, "src", "api", "schema.d.ts");

const url = process.env.VYASA_API_URL ?? "http://127.0.0.1:3000";

async function main() {
  let spec: string | undefined;
  try {
    const response = await fetch(`${url}/api/openapi.json`);
    if (response.ok) {
      spec = await response.text();
      // Refresh the snapshot for offline use.
      writeFileSync(snapshot, spec);
      process.stdout.write(`fetched OpenAPI from ${url}\n`);
    }
  } catch {
    // server not running; fall through to snapshot
  }

  if (!spec) {
    if (!existsSync(snapshot)) {
      process.stderr.write(
        "server not reachable and no openapi.json snapshot exists; " +
          "start the server (cargo run -p vyasa-api -- serve) and re-run\n",
      );
      process.exit(1);
    }
    spec = readFileSync(snapshot, "utf8");
    process.stdout.write("using checked-in openapi.json snapshot\n");
  }

  mkdirSync(dirname(output), { recursive: true });
  writeFileSync(join(root, ".openapi-tmp.json"), spec);
  execFileSync(
    process.platform === "win32" ? "npx.cmd" : "npx",
    [
      "openapi-typescript",
      ".openapi-tmp.json",
      "--output",
      "src/api/schema.d.ts",
    ],
    { cwd: root, stdio: "inherit" },
  );
  execFileSync("rm", ["-f", ".openapi-tmp.json"], { cwd: root });
  process.stdout.write(`wrote ${output}\n`);
}

main();
