package sample

import "fmt"

// Greeter can greet.
type Greeter interface {
	Greet() string
}

func Add(a, b int) int {
	return helper(a, b)
}

func helper(a, b int) int {
	return a + b
}
