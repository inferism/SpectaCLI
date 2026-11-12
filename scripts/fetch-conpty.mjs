// Fetches Microsoft's newer ConPTY (MIT, NuGet package
// Microsoft.Windows.Console.ConPTY) into src-tauri/conpty/:
//   conpty.dll           portable-pty loads this instead of kernel32's ConPTY
//   x64/OpenConsole.exe  the console host conpty.dll starts (package layout)
// Optional (PLAN.md 3.9): without these files the app uses the OS ConPTY.
// Bundle them with `npm run build:conpty`.
// Usage: node scripts/fetch-conpty.mjs [--dev]
//   --dev  also copy them next to the debug exe for `tauri dev`.
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const VERSION = "1.25.260930003";
const SHA256 = "02b07b349af66d801159bdf9e440d4a1ce78bb951f37fc8609731665afdae7ee";
const ARCH = "x64";
const FILES = [
  [`runtimes/win-${ARCH}/native/conpty.dll`, "conpty.dll"],
  [`build/native/runtimes/${ARCH}/OpenConsole.exe`, `${ARCH}/OpenConsole.exe`],
];

const tauriDir = fileURLToPath(new URL("../src-tauri/", import.meta.url));
const out = join(tauriDir, "conpty");
const dests = [out];
if (process.argv.includes("--dev")) dests.push(join(tauriDir, "target", "debug"));

if (FILES.every(([, to]) => existsSync(join(out, to))) && !process.argv.includes("--force")) {
  console.log(`conpty ${VERSION} already in ${out} (pass --force to refetch)`);
} else {
  const id = "microsoft.windows.console.conpty";
  const url = `https://api.nuget.org/v3-flatcontainer/${id}/${VERSION}/${id}.${VERSION}.nupkg`;
  const res = await fetch(url);
  if (!res.ok) throw new Error(`GET ${url}: ${res.status}`);
  const pkg = Buffer.from(await res.arrayBuffer());
  const hash = createHash("sha256").update(pkg).digest("hex");
  if (hash !== SHA256) throw new Error(`conpty package hash mismatch: got ${hash}`);

  const tmp = mkdtempSync(join(tmpdir(), "spectacli-conpty-"));
  try {
    writeFileSync(join(tmp, "pkg.zip"), pkg);
    // Windows' own bsdtar reads zip files (Git Bash's GNU tar does not).
    const tar = join(process.env.SystemRoot ?? "C:\\Windows", "System32", "tar.exe");
    execFileSync(tar, ["-xf", "pkg.zip", ...FILES.map(([from]) => from)], { cwd: tmp, stdio: "inherit" });
    for (const [from, to] of FILES) {
      mkdirSync(join(out, to, ".."), { recursive: true });
      copyFileSync(join(tmp, from), join(out, to));
    }
  } finally {
    rmSync(tmp, { recursive: true, force: true });
  }
  console.log(`conpty ${VERSION} -> ${out}`);
}

for (const dest of dests.slice(1)) {
  for (const [, to] of FILES) {
    mkdirSync(join(dest, to, ".."), { recursive: true });
    copyFileSync(join(out, to), join(dest, to));
  }
  console.log(`copied to ${dest}`);
}
