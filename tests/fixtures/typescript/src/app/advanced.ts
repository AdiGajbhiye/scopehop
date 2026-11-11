import { Circle, makeCircle } from "../models";

// Call signatures on separate lines with no `;` between them: the grammar
// needs a repaired parse to see the rest of this file.
export interface Factory {
  <T extends Circle>(seed: T): T
  (radius: number): Circle
}

export type Unwrap<P> = P extends { value: infer V } ? V : never;

export type Flags<T> = { [K in keyof T]: T[K] };

export class Registry {
  find(name: string): Circle;
  find(name: string, fallback: Circle): Circle;
  find(name: string, fallback?: Circle): Circle {
    return fallback ?? makeCircle(1);
  }

  load = (): Circle | Promise<Circle> => makeCircle(2);

  build: (radius: number) => Circle = makeCircle;
}

export async function use(reg: Registry, cached?: Circle) {
  const found = reg.find("a");
  const loaded = await reg.load();
  const built = reg.build(3);
  const either = cached ?? found;
  const settings = { headers: { accept: "json" } };
  return [found.radius, loaded.radius, built.radius, either.radius, settings.headers.accept];
}

export function untyped(res: any, other) {
  return [res.headers, other.headers];
}
