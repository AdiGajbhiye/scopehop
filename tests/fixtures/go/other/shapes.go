// Package other has a decoy Circle.
package other

type Circle struct {
	Radius int
}

func (c Circle) Area() float64 {
	return 0
}
