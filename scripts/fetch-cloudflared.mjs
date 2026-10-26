// Baixa o cloudflared oficial para ser embutido no app como sidecar.
import { mkdir, writeFile, stat } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const out = resolve(root, "src-tauri/binaries/cloudflared-x86_64-pc-windows-msvc.exe");
const url =
  "https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-windows-amd64.exe";

if (process.argv.includes("--force") === false) {
  try {
    await stat(out);
    console.log("cloudflared já existe:", out);
    process.exit(0);
  } catch {}
}

console.log("Baixando", url);
const res = await fetch(url);
if (!res.ok) throw new Error(`Falha no download: HTTP ${res.status}`);
await mkdir(dirname(out), { recursive: true });
await writeFile(out, Buffer.from(await res.arrayBuffer()));
console.log("Salvo em", out);
