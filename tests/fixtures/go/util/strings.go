package util

import "strings"

const Prefix = ">> "

func Shout(s string) string {
	return Prefix + strings.ToUpper(s)
}

func CountWords(s string) int {
	return len(strings.Fields(s))
}
