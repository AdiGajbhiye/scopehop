import { shout } from "../util/strings";

/** Anything with an area. */
export interface Shape extends Named {
  area(): number;
}

export interface Named {
  name: string;
}

export type Point = { x: number; y: number };

export class Circle implements Shape {
  name = "circle";
  center: Point = { x: 0, y: 0 };

  constructor(public radius: number) {}

  area(): number {
    return 3.14 * this.radius * this.radius;
  }

  grow(by: number): Circle {
    return new Circle(this.radius + by);
  }

  get diameter(): number {
    return this.radius * 2;
  }

  describe(): string {
    return shout(this.name);
  }

  static unit(): Circle {
    return new Circle(1);
  }
}

export class Ring extends Circle {
  constructor(radius: number, public inner: number) {
    super(radius);
  }

  area(): number {
    return super.area() - 3.14 * this.inner * this.inner;
  }
}

export function makeCircle(r: number): Circle {
  return new Circle(r);
}

export const DEFAULT = makeCircle(2);

export async function loadCircles(): Promise<Circle[]> {
  return [Circle.unit()];
}
