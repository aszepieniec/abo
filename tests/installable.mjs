// Whether Chrome would install the page at the URL given as an app: its
// installability errors, none expected, and the manifest as Chrome reads it.
// Not run by cargo test, as it needs Chrome:
//   abo demo --port 8801 &  node tests/installable.mjs http://127.0.0.1:8801/
// ponytail: Chrome's check only; a phone's Add to Home Screen, and iOS Safari, are tried by hand.

import { spawn } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, existsSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const url = process.argv[2];
if (!url) { console.error("usage: node tests/installable.mjs <url>"); process.exit(2); }

const profile = mkdtempSync(join(tmpdir(), "abo-chrome-"));
const chrome = spawn(process.env.CHROME ?? "google-chrome",
  ["--headless=new", "--remote-debugging-port=0", `--user-data-dir=${profile}`, "--no-first-run", "about:blank"],
  { stdio: "ignore" });
const done = (code) => { chrome.kill(); setTimeout(() => { rmSync(profile, { recursive: true, force: true }); process.exit(code); }, 300); };

const portFile = join(profile, "DevToolsActivePort");
for (let i = 0; i < 100 && !existsSync(portFile); i++) await new Promise((r) => setTimeout(r, 100));
const port = readFileSync(portFile, "utf8").split("\n")[0];
const target = (await (await fetch(`http://127.0.0.1:${port}/json/list`)).json()).find((t) => t.type === "page");

const ws = new WebSocket(target.webSocketDebuggerUrl);
await new Promise((r) => ws.addEventListener("open", r));
let id = 0;
const waiting = new Map();
const events = [];
ws.addEventListener("message", (m) => {
  const msg = JSON.parse(m.data);
  if (msg.id) waiting.get(msg.id)?.(msg); else events.push(msg.method);
});
const send = (method, params = {}) => new Promise((resolve) => {
  waiting.set(++id, resolve);
  ws.send(JSON.stringify({ id, method, params }));
});

await send("Page.enable");
await send("Page.navigate", { url });
for (let i = 0; i < 100 && !events.includes("Page.loadEventFired"); i++) await new Promise((r) => setTimeout(r, 100));

const manifest = (await send("Page.getAppManifest")).result;
const errors = (await send("Page.getInstallabilityErrors")).result.installabilityErrors;
console.log("manifest:", manifest.url || "(none)");
const parsed = manifest.data ? JSON.parse(manifest.data) : {};
console.log("  name:", parsed.name, "| start:", parsed.start_url ?? parsed.startUrl, "| scope:", parsed.scope);
for (const e of manifest.errors ?? []) console.log("  manifest error:", e.message);
for (const e of errors) console.log("not installable:", e.errorId, JSON.stringify(e.errorArguments));
done(errors.length || manifest.errors?.some((e) => e.critical) ? 1 : 0);
