package logfields

import (
	"github.com/bottlerocket-os/bottlerocket-update-operator/pkg/intent"

	"github.com/sirupsen/logrus"
)

func Intent(i *intent.Intent) logrus.Fields {
	return logrus.Fields{
		"node":   i.GetName(),
		"intent": i.DisplayString(),
	}
}
