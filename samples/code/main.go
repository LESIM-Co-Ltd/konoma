// Sample Go program — syntax highlighting demo.
package main

import (
	"fmt"
	"strings"
)

// Shape is anything with an area.
type Shape interface {
	Area() float64
}

type Rect struct {
	W, H float64
}

func (r Rect) Area() float64 {
	return r.W * r.H
}

func main() {
	shapes := []Shape{Rect{W: 3, H: 4}, Rect{W: 2, H: 2}}
	var names []string
	for i, s := range shapes {
		names = append(names, fmt.Sprintf("shape%d=%.1f", i, s.Area()))
	}
	fmt.Println(strings.Join(names, ", "))
}
