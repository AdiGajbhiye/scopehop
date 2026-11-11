from dataclasses import dataclass
from typing import Optional

from util.strings import shout


class Shape:
    sides = 0

    def __init__(self, name: str):
        self.name = name

    def area(self) -> float:
        return 0.0

    def describe(self) -> str:
        return shout(self.name)


class Circle(Shape):
    def __init__(self, radius: float, origin: "Point" = None):
        super().__init__("circle")
        self.radius = radius
        self.origin = origin

    def area(self) -> float:
        return 3.14 * self.radius * self.radius

    def grow(self, by: float) -> "Circle":
        return Circle(self.radius + by)

    @property
    def center(self) -> Optional["Point"]:
        return self.origin

    @staticmethod
    def unit() -> "Circle":
        return Circle(1.0)


@dataclass
class Point:
    x: float
    y: float = 0.0

    def moved(self, dx: float) -> "Point":
        return Point(x=self.x + dx, y=self.y)


def make_circle(r) -> Circle:
    return Circle(r)


DEFAULT = Circle(2.0)
