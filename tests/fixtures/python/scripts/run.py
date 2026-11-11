import util.strings as s
from models.shapes import Circle
from other.shapes import Circle as Decoy

print(s.shout("hi"))
Circle(1.0).area()
Decoy().area()
