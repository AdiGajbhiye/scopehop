//go:build slow

package util

import "strings"

// Speed is the `-tags slow` build's.
func Speed() string {
	return strings.ToUpper("slow")
}
