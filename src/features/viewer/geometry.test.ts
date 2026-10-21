import { expect, it } from "vitest";
import { fitScale, zoomAround } from "./imageGeometry";

it("横、竖与超长图按可用画布适配，1:1 始终是原像素比例", () => {
  expect(fitScale(1600, 2400, 1000, 600)).toBe(0.25);
  expect(fitScale(2400, 1600, 1000, 600)).toBe(0.375);
  expect(fitScale(500, 10000, 1000, 600)).toBe(0.06);
  expect(fitScale(1600, 2400, 600, 300)).toBe(0.125);
});
it("鼠标位置对应的图片像素在缩放前后保持不动", () => {
  const before = { scale: 0.25, x: 15, y: -20 },
    anchor = { x: 140, y: 90 };
  const next = zoomAround(
    before.scale,
    1,
    before.x,
    before.y,
    anchor.x,
    anchor.y,
  );
  expect((anchor.x - next.x) / 1).toBe((anchor.x - before.x) / before.scale);
  expect((anchor.y - next.y) / 1).toBe((anchor.y - before.y) / before.scale);
});
