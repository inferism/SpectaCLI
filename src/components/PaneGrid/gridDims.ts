/** Columns and rows for `n` panes: 1 full, 2 = 1x2, 3-4 = 2x2, 5-6 = 3x2, 7+ = 3x3 and scroll. */
export function gridDims(n: number): { cols: number; rows: number } {
  if (n <= 1) return { cols: 1, rows: 1 };
  if (n === 2) return { cols: 2, rows: 1 };
  if (n <= 4) return { cols: 2, rows: 2 };
  if (n <= 6) return { cols: 3, rows: 2 };
  return { cols: 3, rows: Math.ceil(n / 3) };
}
