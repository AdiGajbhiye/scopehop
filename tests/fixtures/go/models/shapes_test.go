package models_test

import (
	"testing"

	"example.com/shop/models"
)

func TestArea(t *testing.T) {
	c := models.NewCircle(2)
	if c.Area() <= 0 {
		t.Fatal("no area")
	}
}
