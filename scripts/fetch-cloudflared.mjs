// Baixa o cloudflared oficial deste sistema para ser embutido no app como sidecar.
import { execFileSync } from "node:child_process";
import { chmod, mkdir, mkdtemp, rename, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

// sistema-processador do Node -> [arquivo na release, nome do sidecar (triple do Rust)]
const TARGETS = {
  "win32-x64": ["cloudflared-windows-amd64.exe", "cloudflared-x86_64-pc-windows-msvc.exe"],
  "darwin-arm64": ["cloudflared-darwin-arm64.tgz", "cloudflared-aarch64-apple-darwin"],
  "darwin-x64": ["cloudflared-darwin-amd64.tgz", "cloudflared-x86_64-apple-darwin"],
};

const key = `${process.platform}-${process.arch}`;
const target = TARGETS[key];
if (!target) throw new Error(`Sem cloudflared para este sistema (${key})`);
const [asset, sidecar] = target;

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const out = resolve(root, "src-tauri/binaries", sidecar);
const url = `https://github.com/cloudflare/cloudflared/releases/latest/download/${asset}`;

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
const data = Buffer.from(await res.arrayBuffer());
await mkdir(dirname(out), { recursive: true });
if (asset.endsWith(".tgz")) {
  // no macOS a release vem compactada, com o executável `cloudflared` dentro
  const tmp = await mkdtemp(join(tmpdir(), "cloudflared-"));
  try {
    const archive = join(tmp, asset);
    await writeFile(archive, data);
    execFileSync("tar", ["-xzf", archive, "-C", tmp, "cloudflared"]);
    await rename(join(tmp, "cloudflared"), out);
  } finally {
    await rm(tmp, { recursive: true, force: true });
  }
  await chmod(out, 0o755);
} else {
  await writeFile(out, data);
}
console.log("Salvo em", out);
