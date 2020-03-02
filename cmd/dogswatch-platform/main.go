package main

import (
	"fmt"

	"github.com/bottlerocket-os/bottlerocket-update-operator/pkg/marker"
)

func main() {
	fmt.Println(marker.PlatformVersionBuild)
}
