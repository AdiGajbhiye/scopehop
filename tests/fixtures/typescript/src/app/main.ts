import { Circle, makeCircle, Colour, DEFAULT, Palette, loadCircles, type Shape, type Point } from "../models";
import * as strings from "@/util/strings";
import whisper, { countWords as words } from "../util/strings";
import { Circle as Decoy } from "@/other/shapes";
import { helper } from "./helper";

let counter = 0;

export class Canvas {
  shapes: Shape[] = [];
  private circle?: Circle;

  add(s: Shape): void {
    this.shapes.push(s);
    counter++;
  }

  total(): number {
    let sum = 0;
    for (const s of this.shapes) {
      sum += s.area();
    }
    return sum;
  }

  first(): Circle | undefined {
    return this.circle;
  }
}

function scopes(items: Circle[], limit = 10): number {
  let total = 0;
  for (let i = 0; i < items.length; i++) {
    total += i + items[i].radius;
  }
  {
    const total = 1;
    console.log(total);
  }
  if (total > limit) {
    var late = total;
  }
  try {
    JSON.parse("x");
  } catch (err) {
    console.log(err, late);
  }
  const twice = (v: number) => v * 2;
  const { radius, center: { x } } = items[0];
  outer: for (const item of items) {
    if (item.radius > limit) break outer;
  }
  return twice(total) + radius + x;
}

async function main() {
  const c = makeCircle(2);
  console.log(c.area(), c.radius, c.name);
  const grown = c.grow(1);
  console.log(grown.radius, grown.diameter);
  console.log(c.center.x, Circle.unit().radius, DEFAULT.area());
  const p: Point = { x: 1, y: 2 };
  const pts: Point[] = [{ x: 3, y: 4 }];
  const config = { port: 8080, db: { host: "localhost" }, start() { return 1; } };
  console.log(p.x, pts[0].y, config.db.host, config.start(), config.port);
  const circles = await loadCircles();
  for (const one of circles) {
    console.log(one.describe());
  }
  console.log(Colour.Red, new Palette().first(), strings.shout("hi"), words("a b"), whisper("X"));
  console.log(new Decoy().area());
  [c].forEach((shape) => shape.area());
  helper(new Canvas());
  return scopes([c]) + counter;
}

main();
