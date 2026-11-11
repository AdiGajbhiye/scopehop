import models.shapes as ms
import util.strings
from models import Circle, Point, RED, make_circle
from models.color import Color as Colour
from util.strings import *
from .other import helper, Config

LIMIT = 10
counter = 0


def bump():
    global counter
    counter += 1
    return counter


def scopes(items, limit=LIMIT):
    total = 0
    for i, item in enumerate(items):
        total += i + item
    squares = [n * n for n in items if n]
    double = lambda v: v * 2
    with open("f") as fh:
        fh.read()
    try:
        double(total)
    except ValueError as err:
        print(err)
    if (size := len(items)) > limit:
        return size

    def inner():
        nonlocal total
        total = 1
        return total + limit

    return inner() + double(total) + len(squares)


class Main:
    LIMIT = 5
    TWICE = LIMIT * 2

    def __init__(self, circle: Circle):
        self.circle = circle
        self.points = []
        self.best: Point = Point(1.0)

    def run(self):
        c = self.circle
        c.area()
        self.circle.radius
        self.best.moved(1.0).x
        grown = c.grow(1.0)
        grown.radius
        made = make_circle(3)
        made.area()
        made.center.y
        Circle.unit().radius
        ms.make_circle(1).radius
        ms.DEFAULT.area()
        util.strings.shout("x")
        shout("y")
        count_words("a b")
        Colour("blue").name
        RED.name
        print(LIMIT)
        p = Point(x=1.0, y=2.0)
        p.moved(dx=0.5)
        Circle(radius=2.0)
        helper(Config())
        return self.size()

    def size(self) -> int:
        return len(self.points)


class Named(Main):
    def run(self):
        super().run()
        return self.circle.describe()

    def label(self):
        return f"{self.circle.name}: {self.size()}"


def any_area(shape):
    return shape.area()


def classify(shape):
    match shape:
        case Circle(radius=r) if r > 1:
            return r
        case Point(x=px, y=0) as pt:
            return px + pt.y
        case [first, *rest]:
            return first, rest
    return None
