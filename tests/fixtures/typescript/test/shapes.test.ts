import { Circle } from "../src/models/shapes";

describe("Circle", () => {
  it("has an area", () => {
    expect(new Circle(1).area()).toBeCloseTo(3.14);
  });

  describe("grow", () => {
    it.only(`doubles the radius`, () => {
      expect(new Circle(1).grow(2).radius).toBe(2);
    });
  });
});

test("parses #1", async () => {
  const c = new Circle(2);
  expect(c.radius).toBe(2);
});

// Not test cases: a name built at run time, and a call nested in a function.
test(`case ${1}`, () => {});
function helper() {
  test("inside helper", () => {});
}
