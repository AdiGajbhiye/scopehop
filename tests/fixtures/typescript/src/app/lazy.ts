// Modules loaded on demand: names taken from them are the module's own.
export async function lazy(): Promise<number> {
  const { makeCircle, Circle: Round } = await import("../models/shapes.js");
  const shapes = await import(`../models/shapes`);
  const c = makeCircle(1);
  return shapes.makeCircle(2).area() + c.area() + new Round(3).radius;
}

// A module outside the project: nothing to go to.
export async function external() {
  const { readFile } = await import("node:fs/promises");
  return readFile;
}
