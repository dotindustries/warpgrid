package main

import (
	"fmt"
	"sort"
)

func main() {
	// sort.Ints
	ints := []int{5, 3, 1, 4, 2}
	sort.Ints(ints)
	fmt.Println("sorted ints:", ints)

	// sort.Strings
	strs := []string{"banana", "apple", "cherry"}
	sort.Strings(strs)
	fmt.Println("sorted strings:", strs)

	// sort.Slice
	type item struct {
		name  string
		value int
	}
	items := []item{
		{"c", 3},
		{"a", 1},
		{"b", 2},
	}
	sort.Slice(items, func(i, j int) bool {
		return items[i].value < items[j].value
	})
	fmt.Println("sorted by value:", items[0].name, items[1].name, items[2].name)
}
