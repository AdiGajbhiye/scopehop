package main

import (
	"fmt"

	m "example.com/shop/models"
	"example.com/shop/other"
	. "example.com/shop/util"
)

var counter int

type Canvas struct {
	shapes []m.Shape
	circle *m.Circle
}

func (cv *Canvas) Add(s m.Shape) {
	cv.shapes = append(cv.shapes, s)
	counter++
}

func (cv *Canvas) Total() float64 {
	sum := 0.0
	for _, s := range cv.shapes {
		sum += s.Area()
	}
	return sum
}

func sum(shapes []m.Shape, extra ...m.Circle) float64 {
	total := 0.0
	for _, sh := range shapes {
		total += sh.Area()
	}
	for _, e := range extra {
		total += e.Radius
	}
	return total
}

func scopes(items []m.Circle, limit int) int {
	total := 0
	for i, item := range items {
		total += i + int(item.Radius)
	}
	if n, err := parse("3"); err == nil {
		total += n
	} else {
		fmt.Println(err)
	}
	x, err := parse("1")
	y, err := parse("2")
	if err != nil {
		return x + y
	}
	twice := func(v int) int { return v * 2 }
	total = twice(total)
	switch v := any(total).(type) {
	case int:
		total = v
	}
	{
		total := 1
		_ = total
	}
	if total > limit {
		goto done
	}
done:
	return total
}

func parse(s string) (int, error) {
	return len(s), nil
}

func main() {
	c := m.NewCircle(2)
	fmt.Println(c.Area(), c.Radius, c.Name())
	grown := c.Grow(1)
	fmt.Println(grown.Radius)
	p := m.Point{X: 1, Y: 2}
	fmt.Println(p.Moved(0.5).X)
	fmt.Println(m.Default.Area())
	fmt.Println(c.Center.Moved(1).Y)
	cv := &Canvas{circle: c}
	cv.Add(c)
	fmt.Println(cv.circle.Describe(), cv.Total())
	pts := []m.Point{{X: 3}}
	fmt.Println(pts)
	d := other.Circle{Radius: 1}
	fmt.Println(d.Area())
	fmt.Println(Shout("hi"), CountWords("a b"))
	fmt.Println(m.Red.String(), m.Green.String())
	var box m.Box[int]
	box.Put(1)
	helper()
}
