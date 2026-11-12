import { expect, test } from "vitest";
import { gridDims } from "./gridDims";

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
