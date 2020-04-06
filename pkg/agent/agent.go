package agent

import (
	"context"
	"os"
	"time"

	"github.com/bottlerocket-os/bottlerocket-update-operator/pkg/intent"
	"github.com/bottlerocket-os/bottlerocket-update-operator/pkg/intent/cache"
	"github.com/bottlerocket-os/bottlerocket-update-operator/pkg/internal/logfields"
	"github.com/bottlerocket-os/bottlerocket-update-operator/pkg/k8sutil"
	"github.com/bottlerocket-os/bottlerocket-update-operator/pkg/logging"
	"github.com/bottlerocket-os/bottlerocket-update-operator/pkg/marker"
	"github.com/bottlerocket-os/bottlerocket-update-operator/pkg/nodestream"
	"github.com/bottlerocket-os/bottlerocket-update-operator/pkg/platform"
	"github.com/bottlerocket-os/bottlerocket-update-operator/pkg/workgroup"

	"github.com/pkg/errors"
	"github.com/sirupsen/logrus"
	v1 "k8s.io/api/core/v1"
	v1meta "k8s.io/apimachinery/pkg/apis/meta/v1"
	"k8s.io/client-go/kubernetes"
	corev1 "k8s.io/client-go/kubernetes/typed/core/v1"
)

const (
	initialPollDelay   = updatePollInterval / 2
	updatePollInterval = time.Minute * 30
)

var (
	errInvalidProgress = errors.New("intended to make invalid progress")
)

// Agent is a privileged on-host process that acts on communicated Intents from
// the controller. Its event loop hinges off of a Kubernetes Informer which
// feeds it metadata and Intent data.
//
// The Agent only acts as directed, its logic covers safety checks and related
// on-host responsibilities. Larger coordination and gating is handled by the
// controller.
type Agent struct {
	log      logging.Logger
	kube     kubernetes.Interface
	platform platform.Platform
	nodeName string

	poster poster
	proc   proc

	lastCache cache.LastCache
	tracker   *postTracker

	progress progression
}

// poster implements the logic for updating, or posting, a provided Intent for
// the appropriate resource.
type poster interface {
	Post(*intent.Intent) error
}

// proc interposes the self-terminate kill signaling allowing for an Agent to
// terminate itself from the outside. Signals are trapped and handled elsewhere
// within the application.
type proc interface {
	KillProcess() error
}

func New(log logging.Logger, kube kubernetes.Interface, plat platform.Platform, nodeName string) (*Agent, error) {
	if nodeName == "" {
		return nil, errors.New("nodeName must be provided for Agent to manage")
	}
	var nodeclient corev1.NodeInterface
	if kube != nil {
		nodeclient = kube.CoreV1().Nodes()
	}
	return &Agent{
		log:       log,
		kube:      kube,
		platform:  plat,
		poster:    &k8sPoster{log, nodeclient},
		proc:      &osProc{},
		nodeName:  nodeName,
		lastCache: cache.NewLastCache(),
		tracker:   newPostTracker(),
	}, nil
}

func (a *Agent) checkProviders() error {
	switch {
	case a.kube == nil:
		return errors.New("kubernetes client is nil")
	case a.platform == nil:
		return errors.New("supporting platform is nil")
	}
	return nil
}

// TODO: add regular update checks

func (a *Agent) Run(ctx context.Context) error {
	if err := a.checkProviders(); err != nil {
		return errors.WithMessage(err, "misconfigured")
	}
	a.log.Debug("starting")
	defer a.log.Debug("finished")
	group := workgroup.WithContext(ctx)

	ns := nodestream.New(a.log.WithField("worker", "informer"), a.kube, nodestream.Config{
		NodeName: a.nodeName,
	}, a.handler())

	err := a.checkNodePreflight()
	if err != nil {
		return err
	}

	group.Work(ns.Run)
	group.Work(a.periodicUpdateChecker)

	<-ctx.Done()
	a.log.Info("waiting on workers to finish")
	return group.Wait()
}

// periodicUpdateChecker regularly checks for available updates and posts this
// status on the Node resource.
func (a *Agent) periodicUpdateChecker(ctx context.Context) error {
	timer := time.NewTimer(initialPollDelay)
	defer timer.Stop()

	log := a.log.WithField("worker", "update-checker")

	for {
		select {
		case <-ctx.Done():
			log.Debug("finished")
			return nil
		case <-timer.C:
			log.Info("checking for update")
			err := a.checkPostUpdate(a.log)
			if err != nil {
				log.WithError(err).Error("periodic check error")
			}
		}
		timer.Reset(updatePollInterval)
	}
}

// checkUpdate queries for an available update from the host.
func (a *Agent) checkUpdate(log logging.Logger) (bool, error) {
	available, err := a.platform.ListAvailable()
	if err != nil {
		log.WithError(err).Error("unable to query available updates")
		return false, err
	}
	hasUpdate := len(available.Updates()) > 0
	log = log.WithField("update-available", hasUpdate)
	if hasUpdate {
		log.Info("an update is available")
	} else {
		log.Info("no update available")
	}
	return hasUpdate, nil
}

// checkPostUpdate checks for and posts the status of an available update.
func (a *Agent) checkPostUpdate(log logging.Logger) error {
	hasUpdate, err := a.checkUpdate(log)
	if err != nil {
		return err
	}
	log = log.WithField("update-available", hasUpdate)
	log.Debug("posting update status")
	err = a.postUpdateAvailable(hasUpdate)
	if err != nil {
		log.WithError(err).Error("could not post update status")
		return err
	}
	log.Debug("posted update status")
	return nil
}

// postUpdateAvailable posts the available update status to the Kubernetes Node
// resource.
func (a *Agent) postUpdateAvailable(available bool) error {
	// TODO: handle brief race condition internally - this needs to be improved,
	// though the kubernetes control plane will reject out of order updates by
	// way of resource versioning C-A-S operations.
	if a.kube == nil {
		return errors.New("kubernetes client is required to fetch node resource")
	}
	node, err := a.kube.CoreV1().Nodes().Get(a.nodeName, v1meta.GetOptions{})
	if err != nil {
		return errors.WithMessage(err, "unable to get node")
	}
	in := intent.Given(node).SetUpdateAvailable(available)
	return a.postIntent(in)
}

// handler is the entrypoint for the Kubernetes Informer to schedule handling of
// events for the Node to act on.
func (a *Agent) handler() nodestream.Handler {
	return &nodestream.HandlerFuncs{
		OnAddFunc: func(n *v1.Node) {
			a.handleEvent(n)
		},
		// we don't mind the diff between old and new, so handle the new
		// resource.
		OnUpdateFunc: func(_, n *v1.Node) {
			a.handleEvent(n)
		},
		OnDeleteFunc: func(_ *v1.Node) {
			panic("we were deleted, panic. everyone panic. 😱")
		},
	}
}

// handleEvent handles a coalesced Node resource received from a nodestream
// callback.
func (a *Agent) handleEvent(node intent.Input) {
	in := intent.Given(node)

	log := a.log.WithFields(logfields.Intent(in))

	if a.skipIntentEvent(in) {
		return
	}

	if activeIntent(in) {
		a.lastCache.Record(in)
		log.Debug("active intent received")
		if err := a.realize(in); err != nil {
			log.WithError(err).Error("unable to realize intent")
		}
		return
	}
	log.Debug("inactive intent received")
}

func (a *Agent) skipIntentEvent(in *intent.Intent) bool {
	log := a.log.WithFields(logfields.Intent(in))
	if a.tracker.matchesPost(in) {
		log.Debug("skipping emitted intent as event")
		return true
	}
	if intent.Equivalent(a.lastCache.Last(in), in) {
		log.Debug("skipping duplicate received event")
		return true
	}
	if logging.Debuggable {
		log.Debug("clearing tracked posted intents")
	}
	a.tracker.clear()

	return false
}

// activeIntent filters an intent as an active intent which must be handled by
// the Agent.
func activeIntent(i *intent.Intent) bool {
	wanted := i.InProgress() && !i.DegradedPath()
	empty := i.Wanted == "" || i.Active == "" || i.State == ""
	unknown := i.Wanted == marker.NodeActionUnknown
	return wanted && !empty && !unknown
}

// realize acts on an Intent to achieve, or realize, the Intent's intent.
func (a *Agent) realize(in *intent.Intent) error {
	log := a.log.WithFields(logrus.Fields{
		"worker": "handler",
		"intent": in.DisplayString(),
	})

	log.Debug("handling intent")

	var err error

	// TODO: Run a quick check of the Nodes posted progress before proceeding

	// ACK the wanted action.
	in.Active = in.Wanted
	in.State = marker.NodeStateBusy
	err = a.postIntent(in)
	if err != nil {
		return err
	}

	// TODO: Propagate status from realization and periodically
	switch in.Wanted {
	case marker.NodeActionReset:
		a.progress.Reset()

	case marker.NodeActionPrepareUpdate:
		var ups platform.Available
		ups, err = a.platform.ListAvailable()
		if err != nil {
			break
		}
		if len(ups.Updates()) == 0 {
			err = errInvalidProgress
			break
		}
		a.progress.SetTarget(ups.Updates()[0])
		log.Debug("preparing update")
		err = a.platform.Prepare(a.progress.GetTarget())

	case marker.NodeActionPerformUpdate:
		if !a.progress.Valid() {
			err = errInvalidProgress
			break
		}
		log.Debug("updating")
		err = a.platform.Update(a.progress.GetTarget())

	case marker.NodeActionUnknown, marker.NodeActionStabilize:
		log.Debug("sitrep")
		_, err = a.platform.Status()
		if err != nil {
			break
		}
		hasUpdate, err := a.checkUpdate(log)
		if err != nil {
			log.WithError(err).Error("sitrep update check errored")
		}
		in.SetUpdateAvailable(hasUpdate)

	case marker.NodeActionRebootUpdate:
		if !a.progress.Valid() {
			err = errInvalidProgress
			break
		}
		log.Debug("rebooting")
		log.Info("Rebooting Node to complete update")
		// TODO: ensure Node is setup to be validated on boot (ie: kubelet will
		// run agent again before we let other Pods get scheduled)
		err = a.platform.BootUpdate(a.progress.GetTarget(), true)
		// Shortcircuit to terminate.

		// TODO: actually handle shutdown.
		if err == nil {
			if a.proc != nil {
				defer a.proc.KillProcess()
			}
			return err
		}
	}

	if err != nil {
		log.WithError(err).Error("could not realize intent")
		in.State = marker.NodeStateError
	} else {
		log.Debug("realized intent")
		in.State = marker.NodeStateReady
	}

	postErr := a.postIntent(in)
	if postErr != nil {
		log.WithError(postErr).Error("could not update intent")
	}

	return err
}

func (a *Agent) postIntent(in *intent.Intent) error {
	err := a.poster.Post(in)
	if err != nil {
		a.tracker.recordPost(in)
	}
	return err
}

// checkNodePreflight runs checks against the current Node resource and prepares
// it for use by the Agent and Controller.
func (a *Agent) checkNodePreflight() error {
	// TODO: Run a check of the Node Resource and reset appropriately

	// TODO: Inform controller for taint removal

	n, err := a.kube.CoreV1().Nodes().Get(a.nodeName, v1meta.GetOptions{})
	if err != nil {
		return errors.WithMessage(err, "unable to retrieve Node for preflight check")
	}

	// Update our state to be "ready" for action, this shouldn't actually do so
	// unless its really done.
	in := intent.Given(n)
	// TODO: check that we're properly reseting, for now its not needed to mark
	// our work "done"
	switch {
	case in.Terminal(): // we're at a terminating point where there's no progress to make.
		in.State = marker.NodeStateReady
	case in.Waiting():
		// already in a holding pattern, no need to re-prime ourselves in
		// preflight.
	case in.Wanted == "" || in.Active == "":
		in = in.Reset()
	default:
		// there's not a good way to re-prime ourselves in the prior state.
		in = in.Reset()
	}

	postErr := a.postIntent(in)
	if postErr != nil {
		a.log.WithError(postErr).Error("could not update intent status")
		return postErr
	}

	return nil
}

// osProc encapsulates host interactions in order to kill the current process.
type osProc struct{}

// KillProcess kills the current process.
func (*osProc) KillProcess() error {
	p, _ := os.FindProcess(os.Getpid())
	go p.Kill()
	return nil
}

// k8sPoster captures the functionality of the posting of a Node resource
// modification - in the form of an Intent.
type k8sPoster struct {
	log        logging.Logger
	nodeclient corev1.NodeInterface
}

// Post writes out the Intent to the Kubernetes Node resource.
func (k *k8sPoster) Post(i *intent.Intent) error {
	nodeName := i.GetName()
	log := k.log.WithFields(logrus.Fields{
		"node":   nodeName,
		"intent": i.DisplayString(),
	})
	err := k8sutil.PostMetadata(k.nodeclient, nodeName, i)
	if err != nil {
		return err
	}
	log.Debugf("posted intent")
	return nil
}
