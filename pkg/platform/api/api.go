package api

import (
	"context"
	"encoding/json"
	"io/ioutil"
	"net"
	"net/http"
	"time"

	"github.com/pkg/errors"

	"github.com/bottlerocket-os/bottlerocket-update-operator/pkg/logging"
)

const (
	bottlerocketAPISock = "/run/api.sock"
	// The minimum required host Bottlerocket OS version is v0.4.1 because that's when the Update API
	// was first added. https://github.com/bottlerocket-os/bottlerocket/releases/tag/v0.4.1
	minimumRequiredOSVer = "0.4.1"
)

type updateState string

const (
	stateIdle      updateState = "Idle"
	stateAvailable updateState = "Available"
	stateStaged    updateState = "Staged"
	stateReady     updateState = "Ready"
)

type updateImage struct {
	Arch    string `json:"arch"`
	Version string `json:"version"`
	Variant string `json:"variant"`
}

func (ui *updateImage) Identifier() interface{} {
	return ui.Version
}

type stagedImage struct {
	Image      updateImage `json:"image"`
	NextToBoot bool        `json:"next_to_boot"`
}

type updateCommand string

const (
	commandRefresh  updateCommand = "refresh"
	commandPrepare  updateCommand = "prepare"
	commandActivate updateCommand = "activate"
)

type commandStatus string

const (
	statusSuccess commandStatus = "Success"
	Failed        commandStatus = "Failed"
	Unknown       commandStatus = "Unknown"
)

type commandResult struct {
	CmdType    updateCommand `json:"cmd_type"`
	CmdStatus  commandStatus `json:"cmd_status"`
	Timestamp  string        `json:"timestamp"`
	ExitStatus *int32        `json:"exit_status"`
	Stderr     *string       `json:"stderr"`
}

type updateStatus struct {
	UpdateState       updateState    `json:"update_state"`
	AvailableUpdates  []string       `json:"available_updates"`
	ChosenUpdate      *updateImage   `json:"chosen_update"`
	ActivePartition   *stagedImage   `json:"active_partition"`
	StagingPartition  *stagedImage   `json:"staging_partition"`
	MostRecentCommand *commandResult `json:"most_recent_command"`
}

type apiClient struct {
	log        logging.Logger
	httpClient *http.Client
}

func newAPIClient() *apiClient {
	return &apiClient{log: logging.New("update-api"), httpClient: &http.Client{
		Transport: &http.Transport{
			DialContext: func(ctx context.Context, _, _ string) (net.Conn, error) {
				dialer := net.Dialer{}
				return dialer.DialContext(ctx, "unix", bottlerocketAPISock)
			},
		},
		// By default, Timeout is set to 0 which would mean no timeout.
		// Set a 10 second timeout for all requests so we don't wait forever if the API fails to return a response.
		// The Bottlerocket API should always immediately return a response regardless of the request.
		// The 10 second value picked here is arbitrary and should be changed if it proves insufficient.
		Timeout: 10 * time.Second,
	},
	}
}

func (c *apiClient) do(req *http.Request) (*http.Response, error) {
	var response *http.Response
	const maxAttempts = 5
	attempts := 0
	// Retry up to 5 times in case the Update API is busy; Waiting 10 seconds between each attempt.
	for ; attempts < maxAttempts; attempts++ {
		var err error
		response, err = c.httpClient.Do(req)
		if err != nil {
			return nil, errors.Wrapf(err, "update API request error")
		}
		if response.StatusCode >= 200 && response.StatusCode < 300 {
			// Response OK
			break
		} else if response.StatusCode == 423 {
			if attempts < maxAttempts-1 {
				c.log.Info("API server busy, retrying in 10 seconds ...")
				// Retry after ten seconds if we get a 423 Locked response (update API busy)
				time.Sleep(10 * time.Second)
				continue
			}
		}
		// API response was a non-transient error, bail out.
		return response, errors.Errorf("bad http response, status code: %d", response.StatusCode)
	}
	if attempts == 5 {
		return nil, errors.New("update API unavailable: retries exhausted")
	}
	return response, nil
}

func (c *apiClient) Get(path string) (*http.Response, error) {
	req, err := http.NewRequest(http.MethodGet, "http://unix"+path, nil)
	if err != nil {
		return nil, err
	}
	c.log.WithField("path", path).WithField("method", http.MethodGet).Debugf("update API request")
	return c.do(req)
}

func (c *apiClient) Post(path string) (*http.Response, error) {
	req, err := http.NewRequest(http.MethodPost, "http://unix"+path, http.NoBody)
	if err != nil {
		return nil, err
	}
	c.log.WithField("path", path).WithField("method", http.MethodPost).Debugf("update API request")
	return c.do(req)
}

// GetUpdateStatus returns the update status from the update API
func (c *apiClient) GetUpdateStatus() (*updateStatus, error) {
	response, err := c.Get("/updates/status")
	if err != nil {
		return nil, err
	}

	var updateStatus updateStatus
	body, err := ioutil.ReadAll(response.Body)
	if err != nil {
		return nil, err
	}
	err = json.Unmarshal(body, &updateStatus)
	if err != nil {
		return nil, err
	}
	return &updateStatus, nil
}

func (c *apiClient) GetMostRecentCommand() (*commandResult, error) {
	updateStatus, err := c.GetUpdateStatus()
	if err != nil {
		return nil, err
	}
	return updateStatus.MostRecentCommand, nil
}

type osInfo struct {
	VersionID string `json:"version_id"`
}

func (c *apiClient) GetOSInfo() (*osInfo, error) {
	response, err := c.Get("/os")
	if err != nil {
		return nil, err
	}

	var osInfo osInfo
	body, err := ioutil.ReadAll(response.Body)
	if err != nil {
		return nil, err
	}
	err = json.Unmarshal(body, &osInfo)
	if err != nil {
		return nil, err
	}
	return &osInfo, nil
}

func (c *apiClient) RefreshUpdates() error {
	_, err := c.Post("/actions/refresh-updates")
	return err
}

func (c *apiClient) PrepareUpdate() error {
	_, err := c.Post("/actions/prepare-update")
	return err
}

func (c *apiClient) ActivateUpdate() error {
	_, err := c.Post("/actions/activate-update")
	return err
}

func (c *apiClient) Reboot() error {
	_, err := c.Post("/actions/reboot")
	return err
}
