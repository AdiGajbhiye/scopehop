package models

// Grow is a Circle method in another file of the package.
func (c *Circle) Grow(by float64) *Circle {
	c.Radius += by
	return c
}

func unit() *Circle {
	return NewCircle(1)
}

type Box[T any] struct {
	items []T
}

func (b *Box[T]) Put(item T) {
	b.items = append(b.items, item)
}
