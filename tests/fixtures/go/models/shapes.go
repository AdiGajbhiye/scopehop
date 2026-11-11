// Package models holds shapes.
package models

import (
	"fmt"
	"math"

	"example.com/shop/util"
)

// Shape is anything with an area.
type Shape interface {
	Area() float64
	Named
}

type Named interface {
	Name() string
}

type Base struct {
	label string
}

func (b Base) Name() string {
	return b.label
}

type Point struct {
	X, Y float64
}

func (p Point) Moved(dx float64) Point {
	return Point{X: p.X + dx, Y: p.Y}
}

type Circle struct {
	Base
	Center Point
	Radius float64
}

func NewCircle(r float64) *Circle {
	return &Circle{Base: Base{label: "circle"}, Radius: r}
}

func (c *Circle) Area() float64 {
	return math.Pi * c.Radius * c.Radius
}

func (c *Circle) Describe() string {
	return util.Shout(fmt.Sprintf("%s %v", c.Name(), c.Area()))
}

var Default = NewCircle(1)

type Color int

const (
	Red Color = iota
	Green
)

func (c Color) String() string {
	return [...]string{"red", "green"}[c]
}
