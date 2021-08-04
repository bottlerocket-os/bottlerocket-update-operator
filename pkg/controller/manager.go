package controller

import (
	"context"
	"fmt"
	"math/rand"
	"time"

	"github.com/bottlerocket-os/bottlerocket-update-operator/pkg/intent"
	intentcache "github.com/bottlerocket-os/bottlerocket-update-operator/pkg/intent/cache"
	"github.com/bottlerocket-os/bottlerocket-update-operator/pkg/internal/logfields"
	"github.com/bottlerocket-os/bottlerocket-update-operator/pkg/logging"
	"github.com/bottlerocket-os/bottlerocket-update-operator/pkg/marker"
	"github.com/bottlerocket-os/bottlerocket-update-operator/pkg/nodestream"

	"github.com/pkg/errors"
	"github.com/sirupsen/logrus"
	v1 "k8s.io/api/core/v1"
	"k8s.io/client-go/kubernetes"
	corev1 "k8s.io/client-go/kubernetes/typed/core/v1"
	"k8s.io/client-go/tools/cache"
)

const (
	// maxQueuedIntents controls the number of queued Intents that are waiting
	// to be handled.
	maxQueuedIntents   = 100
	maxQueuedInputs    = maxQueuedIntents * (1 / 4)
	queueSkipThreshold = maxQueuedIntents / 2
)

var _ nodestream.Handler = (*actionManager)(nil)

var randDropIntFunc func(int) int = rand.Intn

// actionManager handles node changes according to policy and runs a node update
// flow to completion as allowed by policy.
type actionManager struct {
	log       logging.Logger
	kube      kubernetes.Interface
	policy    Policy
	inputs    chan *intent.Intent
	storer    storer
	poster    poster
	nodem     nodeManager
	lastCache intentcache.LastCache
}

// poster is the implementation of the intent poster that publishes the provided
// intent.
type poster interface {
	Post(*intent.Intent) error
}

// nodeManager is the implementation that interfaces the interactions with nodes
// to accomplish tasks.
type nodeManager interface {
	Cordon(string) error
	Uncordon(string) error
	Drain(string) error
}

type storer interface {
	GetStore() cache.Store
}

func newManager(log logging.Logger, kube kubernetes.Interface, nodeName string) *actionManager {
	var nodeclient corev1.NodeInterface
	if kube != nil {
		nodeclient = kube.CoreV1().Nodes()
	}

	return &actionManager{
		log:       log,
		kube:      kube,
		policy:    &defaultPolicy{log: log.WithField(logging.SubComponentField, "policy-check")},
		inputs:    make(chan *intent.Intent, maxQueuedInputs),
		poster:    &k8sPoster{log, nodeclient},
		nodem:     &k8sNodeManager{log, kube},
		lastCache: intentcache.NewLastCache(),
	}
}

func (am *actionManager) Run(ctx context.Context) error {
	am.log.Debug("starting")
	defer am.log.Debug("finished")

	queuedIntents := make(chan *intent.Intent, maxQueuedIntents)

	// TODO: split out accepted intent handler - it should handle its
	// prioritization as needed to ensure that active nodes' events reach it.

	for {
		// Handle active intents
		select {
		case <-ctx.Done():
			return nil

		case qin, ok := <-queuedIntents:
			log := am.log.WithFields(logfields.Intent(qin))
			log.Debug("checking with policy")
			// TODO: make policy checking and consideration richer
			pview, err := am.makePolicyCheck(qin)
			if err != nil {
				log.WithError(err).Error("policy unenforceable")
				continue
			}
			proceed, err := am.policy.Check(pview)
			if err != nil {
				log.WithError(err).Error("policy check errored")
				continue
			}
			if !proceed {
				log.Debug("policy denied intent")
				continue
			}
			if !ok {
				break
			}
			log.Debug("handling permitted intent")
			am.takeAction(qin)

		case input, ok := <-am.inputs:
			if !ok {
				am.log.Error("input channel closed")
				break
			}

			queued := len(queuedIntents)
			log := am.log.WithFields(logfields.Intent(input)).
				WithFields(logrus.Fields{
					"queue":        "process",
					"queue-length": fmt.Sprintf("%d", queued),
				})
			if queued < queueSkipThreshold {
				queuedIntents <- input
				continue
			}
			// Start dropping if its not possible to queue at all.
			if queued+1 > maxQueuedIntents {
				log.Warn("queue full, dropping intent this try")
				continue
			}

			// TODO: handle backpressure better with rescheduling instead of drops

			// Queue is getting full, let's be more selective about events that
			// are propagated.

			if isClusterActive(input) {
				log.Info("queue active intent")
				queuedIntents <- input
				continue
			}

			if isLowPriority(input) {
				n := randDropIntFunc(10)
				willDrop := n%2 == 0
				if willDrop {
					// Intent is picked up again when cached Intent expires &
					// Informer syncs OR if the Intent is changed (from update
					// or otherwise by Node). This provides indirect
					// backpressure by delaying the next time the Intent will be
					// handled.
					log.Warn("queue backlog high, randomly dropping intent")
					continue
				}
			}
			log.Debug("queue intent")
			queuedIntents <- input

		}
	}
}

func isLowPriority(in *intent.Intent) bool {
	stabilizing := in.Wanted == marker.NodeActionStabilize
	unknown := in.Wanted == marker.NodeActionUnknown || in.Wanted == ""
	hasUpdate := in.UpdateAvailable == marker.NodeUpdateAvailable
	return (stabilizing && !hasUpdate) || unknown
}

func (am *actionManager) takeAction(pin *intent.Intent) error {
	log := am.log.WithFields(logfields.Intent(pin))
	successCheckRun := successfulUpdate(pin)
	if successCheckRun {
		log.Debug("handling successful update")
	}

	if pin.Intrusive() && !successCheckRun {
		log.Info("starting cordon")
		err := am.nodem.Cordon(pin.NodeName)
		if err != nil {
			log.WithError(err).Error("could not cordon")
			return err
		}
		log.Info("cordon successful")

		log.Info("starting drain")
		err = am.nodem.Drain(pin.NodeName)
		if err != nil {
			log.WithError(err).Error("could not drain")
			log.Info("attempting to uncordon after drain fail")
			// allow scheduling on node
			err = am.nodem.Uncordon(pin.NodeName)
			if err != nil {
				log.WithError(err).Error("could not uncordon after drain fail")
				log.Warn("workload will not return")
				return err
			}
			// Reset the state, so other nodes can be considered for update.
			pin = pin.Reset()
			log.WithFields(logfields.Intent(pin)).Info("resetting the node labels after drain fail")
		} else {
			log.Info("drain successful")
		}
	}

	// Handle successful node reconnection.
	if successCheckRun {
		// Reset the state to begin its stabilization.
		pin = pin.Reset()
		log.Info("starting uncordon")
		err := am.nodem.Uncordon(pin.NodeName)
		if err != nil {
			log.WithError(err).Error("could not uncordon")
			log.Warn("workload will not return")
			return err
		}
		log.Info("uncordon successful")

		log.Info("performing health check after update")
		err = am.checkNode(pin.NodeName)
		if err != nil {
			log.WithError(err).Error("unable to perform node health check after update, node will be updated in next run")
		}

		log.Info("waiting for workload to be scheduled on node")
		// 1 minute is just a reasonable estimate
		time.Sleep(1 * time.Minute)

		log.Info("Update successful")
	}

	err := am.poster.Post(pin)
	if err != nil {
		log.WithError(err).Error("unable to post intent")
	}
	return err
}

// makePolicyCheck collects cluster information as a PolicyCheck for which to be
// provided to a policy checker.
func (am *actionManager) makePolicyCheck(in *intent.Intent) (*PolicyCheck, error) {
	if am.storer == nil {
		return nil, errors.Errorf("manager has no store to access, needed for policy check")
	}
	return newPolicyCheck(in, am.storer.GetStore())
}

func (am *actionManager) SetStoreProvider(storer storer) {
	am.storer = storer
}

func (am *actionManager) handle(node intent.Input) {
	log := am.log.WithField("node", node.GetName())
	log.Debug("handling event")

	in := am.intentFor(node)
	if in == nil {
		return // no actionable intent signaled
	}
	log = log.WithFields(logfields.Intent(in))

	lastQueued := am.lastCache.Last(in)
	if logging.Debuggable && lastQueued != nil {
		log.WithField("last-intent", lastQueued.DisplayString()).
			Debug("retrieved cached queued intent to dedupe")
	}
	if intent.Equivalent(lastQueued, in) {
		log.Debug("not queuing duplicate intent")
		return
	}

	record := in.Clone()
	select {
	case am.inputs <- in:
		log.Debug("queue intent")
		am.lastCache.Record(record)
	default:
		log.WithFields(logrus.Fields{
			"queue":        "input",
			"queue-length": len(am.inputs),
		}).Warn("unable to queue intent (back pressure)")
	}
}

// intentFor interprets the intention given the Node's annotations.
func (am *actionManager) intentFor(node intent.Input) *intent.Intent {
	in := intent.Given(node)
	log := am.log.WithFields(logfields.Intent(in))

	if in.Stuck() {
		reset := in.Reset()
		log.WithField("intent-reset", reset.DisplayString()).Debug("node intent indicates stuck")
		log.Warn("stabilizing stuck node")
		return reset
	}
	// TODO: add per-node bucketed backoff for error handling and retries.
	if in.Errored() {
		log.Debug("intent errored")
		log.Warn("action errored on node, resetting to stabilize")
		in = in.Reset()
		return in.Projected()
	}
	next := in.Projected()
	if (in.Actionable() || next.Actionable()) && in.Realized() && !in.InProgress() {
		log.Debug("intent needs action")
		log.Debug("needs action towards next step")
		return next
	}
	if !in.Realized() {
		log.Debug("intent is not yet realized")
		return nil
	}

	if successfulUpdate(in) {
		return in
	}

	if in.HasUpdateAvailable() && in.Waiting() && !in.Errored() {
		log.Debug("intent starts update")
		return in.SetBeginUpdate()
	}

	log.Debug("no action needed")
	return nil
}

func successfulUpdate(in *intent.Intent) bool {
	atFinalTerm := intent.FallbackNodeAction != in.Wanted && !in.Stuck()
	return atFinalTerm && in.Waiting() && in.Terminal() && in.Realized()
}

// OnAdd is a Handler implementation for nodestream
func (am *actionManager) OnAdd(node *v1.Node) {
	am.handle(node)
}

// OnDelete is a Handler implementation for nodestream
func (am *actionManager) OnDelete(node *v1.Node) {
	am.handle(node)
}

// OnUpdate is a Handler implementation for nodestream
func (am *actionManager) OnUpdate(_ *v1.Node, node *v1.Node) {
	am.handle(node)
}
