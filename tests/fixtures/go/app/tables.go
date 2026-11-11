package main

import (
	"fmt"

	m "example.com/shop/models"
	"example.com/shop/util"
)

// Ring ranges like the slice it is declared as.
type Ring []m.Circle

func (cv *Canvas) Rings() map[string]Ring {
	return nil
}

func tables(cv *Canvas, ch chan m.Circle) {
	tests := []struct {
		name  string
		input m.Point
	}{
		{name: "origin", input: m.Point{X: 0}},
	}
	for _, tt := range tests {
		fmt.Println(tt.name, tt.input.X)
	}
	for _, row := range []struct{ label string }{{label: "a"}} {
		fmt.Println(row.label)
	}
	for key, ring := range cv.Rings() {
		fmt.Println(key, ring[0].Radius)
		for _, rc := range ring {
			fmt.Println(rc.Area())
		}
	}
	first := cv.shapes[0]
	fmt.Println(first.Name())
	got := <-ch
	fmt.Println(got.Radius)
	for c := range ch {
		fmt.Println(c.Describe())
	}
	fmt.Println(util.Speed())
}

func describe(x any) {
	switch v := x.(type) {
	case m.Shape:
		fmt.Println(v.Area())
	}
}
