import { expect, test } from "vitest";
import { gridDims, lastSpan } from "./gridDims";

test("grid follows the layout table", () => {
  expect(gridDims(1)).toEqual({ cols: 1, rows: 1 });
  expect(gridDims(2)).toEqual({ cols: 2, rows: 1 });
  expect(gridDims(3)).toEqual({ cols: 2, rows: 2 });
  expect(gridDims(4)).toEqual({ cols: 2, rows: 2 });
  expect(gridDims(5)).toEqual({ cols: 3, rows: 2 });
  expect(gridDims(6)).toEqual({ cols: 3, rows: 2 });
  expect(gridDims(7)).toEqual({ cols: 3, rows: 3 });
  expect(gridDims(10)).toEqual({ cols: 3, rows: 4 });
});

test("the last pane fills a part-filled row", () => {
  expect([1, 2, 3, 4, 5, 6, 7, 8, 9].map(lastSpan)).toEqual([1, 1, 2, 1, 2, 1, 3, 2, 1]);
});
