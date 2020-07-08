package api

import (
	"github.com/Masterminds/semver"
	"github.com/pkg/errors"

	"github.com/bottlerocket-os/bottlerocket-update-operator/pkg/logging"
	"github.com/bottlerocket-os/bottlerocket-update-operator/pkg/platform"
)

// Assert Update-API as a platform implementor.
var _ platform.Platform = (*apiPlatform)(nil)

type apiPlatform struct {
	log       logging.Logger
	apiClient *apiClient
}

func New() (*apiPlatform, error) {
	return &apiPlatform{log: logging.New("platform"), apiClient: newAPIClient()}, nil
}

type statusResponse struct {
	osVersion *semver.Version
}

func (sr *statusResponse) OK() bool {
	// Bottlerocket OS version needs to be at least a certain version to support the Update API
	constraint, err := semver.NewConstraint(">= " + minimumRequiredOSVer)
	if err != nil {
		return false
	}
	return constraint.Check(sr.osVersion)
}

func (p apiPlatform) Status() (platform.Status, error) {
	// Try to determine if the update API is supported in the Bottlerocket host
	osInfo, err := p.apiClient.GetOSInfo()
	if err != nil {
		return nil, err
	}

	osVersion, err := semver.NewVersion(osInfo.VersionID)
	p.log.Info("current running OS version: ", osInfo.VersionID)
	if err != nil {
		return nil, errors.Wrap(err, "failed to parse 'version_id' field as semver")
	}
	return &statusResponse{osVersion: osVersion}, nil
}

type listAvailableResponse struct {
	chosenUpdate *updateImage
}

func (lar *listAvailableResponse) Updates() []platform.Update {
	return []platform.Update{lar.chosenUpdate}
}

func (p apiPlatform) ListAvailable() (platform.Available, error) {
	p.log.Debug("fetching list of available updates")

	// Refresh list of updates and check if there are any available
	err := p.apiClient.RefreshUpdates()
	if err != nil {
		return nil, err
	}

	updateStatus, err := p.apiClient.GetUpdateStatus()
	if err != nil {
		return nil, err
	}
	if updateStatus.MostRecentCommand.CmdType != commandRefresh && updateStatus.MostRecentCommand.CmdStatus != statusSuccess {
		return nil, errors.New("failed to refresh updates or update action performed out of band")

	}
	return &listAvailableResponse{chosenUpdate: updateStatus.ChosenUpdate}, nil
}

func (p apiPlatform) Prepare(target platform.Update) error {
	updateStatus, err := p.apiClient.GetUpdateStatus()
	if err != nil {
		return err
	}
	if updateStatus.UpdateState != stateAvailable && updateStatus.UpdateState != stateStaged {
		return errors.Errorf("unexpected update state: %s, expecting state to be 'Available' or 'Staged'. update action performed out of band?", updateStatus.UpdateState)
	}

	// Download the update and apply it to the inactive partition
	err = p.apiClient.PrepareUpdate()
	if err != nil {
		return err
	}

	commandResult, err := p.apiClient.GetMostRecentCommand()
	if err != nil {
		return err
	}
	if commandResult.CmdType != commandPrepare || commandResult.CmdStatus != statusSuccess {
		return errors.New("failed to prepare update or update action performed out of band")
	}
	return nil
}

func (p apiPlatform) Update(target platform.Update) error {
	updateStatus, err := p.apiClient.GetUpdateStatus()
	if err != nil {
		return err
	}
	if updateStatus.UpdateState != stateStaged {
		return errors.Errorf("unexpected update state: %s, expecting state to be 'Staged'. update action performed out of band?", updateStatus.UpdateState)
	}

	// Activate the prepared update

	err = p.apiClient.ActivateUpdate()
	if err != nil {
		return err
	}

	commandResult, err := p.apiClient.GetMostRecentCommand()
	if err != nil {
		return err
	}
	if commandResult.CmdType != commandActivate || commandResult.CmdStatus != statusSuccess {
		return errors.New("failed to activate update or update action performed out of band")
	}
	return nil
}

func (p apiPlatform) BootUpdate(target platform.Update, rebootNow bool) error {
	updateStatus, err := p.apiClient.GetUpdateStatus()
	if err != nil {
		return err
	}
	if updateStatus.UpdateState != stateReady {
		return errors.Errorf("unexpected update state: %s, expecting state to be 'Ready'. update action performed out of band?", updateStatus.UpdateState)
	}

	// Reboot the host into the activated update
	err = p.apiClient.Reboot()
	if err != nil {
		return err
	}
	return nil
}
