package util

// No plan9 build has `import "strings"` in slow.go, so this cannot clash.
var strings = []string{"plan9"}

func Speed() string {
	return strings[0]
}
