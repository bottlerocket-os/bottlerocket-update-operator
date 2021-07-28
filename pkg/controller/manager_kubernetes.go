package controller

import (
	"context"
	"time"

	"github.com/bottlerocket-os/bottlerocket-update-operator/pkg/intent"
	"github.com/bottlerocket-os/bottlerocket-update-operator/pkg/k8sutil"
	"github.com/bottlerocket-os/bottlerocket-update-operator/pkg/logging"
	"github.com/pkg/errors"
	"github.com/sirupsen/logrus"
	v1 "k8s.io/api/core/v1"
	v1meta "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/client-go/kubernetes"
	corev1 "k8s.io/client-go/kubernetes/typed/core/v1"
	"k8s.io/kubectl/pkg/drain"
)

type k8sNodeManager struct {
	log  logging.Logger
	kube kubernetes.Interface
}

func (k *k8sNodeManager) forNode(nodeName string) (*v1.Node, *drain.Helper, error) {
	var drainer *drain.Helper
	node, err := k.kube.CoreV1().Nodes().Get(nodeName, v1meta.GetOptions{})
	if err != nil {
		return nil, nil, errors.WithMessage(err, "unable to retrieve node from api")
	}
	drainer = &drain.Helper{
		Ctx:    context.TODO(),
		Client: k.kube,
		Out:    k.log.WriterLevel(logrus.InfoLevel),
		ErrOut: k.log.WriterLevel(logrus.ErrorLevel),
		// Ignore daemon-set drain so that agent running on node is not drained. Also, Kubernetes
		// daemon-set ignores unschedulable markings and gets immediately replaced, so there is no
		// point in draining pods managed by daemon-sets
		IgnoreAllDaemonSets: true,
		// Continue even if there are pods using emptyDir (local data that will be deleted when the node is drained).
		DeleteEmptyDirData: true,
		// The length of time to wait before giving up, default is infinite.
		// 15 minutes is just a reasonable estimate
		Timeout: time.Duration(15) * time.Minute,
		// Custom logic to prevent drain of update operator controller
		AdditionalFilters: []drain.PodFilter{
			k.skipController,
		},
	}
	return node, drainer, err
}

func (k *k8sNodeManager) skipController(pod v1.Pod) drain.PodDeleteStatus {
	if pod.GetLabels()["update-operator"] == "controller" {
		return drain.MakePodDeleteStatusWithWarning(false, "ignoring update operator controller pod")
	}
	return drain.MakePodDeleteStatusOkay()
}

func (k *k8sNodeManager) setCordon(nodeName string, cordoned bool) error {
	node, drainer, err := k.forNode(nodeName)
	if err != nil {
		return errors.WithMessage(err, "unable to operate")
	}
	return drain.RunCordonOrUncordon(drainer, node, cordoned)
}

func (k *k8sNodeManager) Uncordon(nodeName string) error {
	return k.setCordon(nodeName, false)
}

func (k *k8sNodeManager) Cordon(nodeName string) error {
	return k.setCordon(nodeName, true)
}

func (k *k8sNodeManager) Drain(nodeName string) error {
	_, drainer, err := k.forNode(nodeName)
	if err != nil {
		return errors.WithMessage(err, "unable to operate")
	}
	return drain.RunNodeDrain(drainer, nodeName)
}

func (am *actionManager) checkNode(nodeName string) error {
	node, err := am.kube.CoreV1().Nodes().Get(context.TODO(), nodeName, v1meta.GetOptions{})
	if err != nil {
		return errors.WithMessage(err, "unable to retrieve node from api")
	}
	// Retry node condition Ready for max 5 minutes
	lastCondition := v1.NodeCondition{}
	for i := 0; i < 30; i++ {
		for _, condition := range node.Status.Conditions {
			if condition.Type == v1.NodeReady {
				if condition.Status == v1.ConditionTrue {
					return nil
				}
				lastCondition = condition
			}
		}
		// delay before checking again
		time.Sleep(10 * time.Second)
	}
	return errors.Errorf("node did not come up healthy: %q", lastCondition)
}

type k8sPoster struct {
	log        logging.Logger
	nodeclient corev1.NodeInterface
}

func (k *k8sPoster) Post(i *intent.Intent) error {
	nodeName := i.GetName()
	err := k8sutil.PostMetadata(k.nodeclient, nodeName, i)
	if err != nil {
		return err
	}
	k.log.WithFields(logrus.Fields{
		"node":   nodeName,
		"intent": i.DisplayString(),
	}).Debugf("posted intent")
	return nil
}
