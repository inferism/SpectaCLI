import type { Usage } from "./ipc";

/** "412 MB", "1.2 GB". */
export function fmtBytes(n: number): string {
  const mb = n / (1024 * 1024);
  if (mb < 1000) return `${Math.round(mb)} MB`;
  return `${(mb / 1024).toFixed(1)} GB`;
}

export function fmtCpu(p: number): string {
  return `${p < 10 ? p.toFixed(1) : Math.round(p)}%`;
}

/** "412 MB · 3.4%" */
export function fmtUsage(u: Usage): string {
  return `${fmtBytes(u.privateBytes)} · ${fmtCpu(u.cpu)}`;
}

export function usageTitle(u: Usage): string {
  return `Private bytes ${fmtBytes(u.privateBytes)}, working set ${fmtBytes(u.workingSet)}, ${u.processes} processes`;
}
