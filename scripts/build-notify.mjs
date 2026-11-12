// Builds the spectacli-notify hook helper and puts it where Tauri's
// `externalBin` expects it: src-tauri/binaries/spectacli-notify-<triple>.exe.
// Tauri then copies it next to the app exe (in dev and in the bundle).
// Usage: node scripts/build-notify.mjs [--release]
import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync } from "node:fs";

const release = process.argv.includes("--release");
const cwd = new URL("../src-tauri/", import.meta.url);
execFileSync("cargo", ["build", "-p", "spectacli-notify", ...(release ? ["--release"] : [])], {
  cwd,
  stdio: "inherit",
});
const triple = /^host: (\S+)$/m.exec(execFileSync("rustc", ["-vV"]).toString())[1];
mkdirSync(new URL("binaries/", cwd), { recursive: true });
copyFileSync(
  new URL(`target/${release ? "release" : "debug"}/spectacli-notify.exe`, cwd),
  new URL(`binaries/spectacli-notify-${triple}.exe`, cwd),
);
