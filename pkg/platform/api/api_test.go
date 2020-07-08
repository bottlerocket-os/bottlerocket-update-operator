package api

import (
	"encoding/json"
	"testing"

	"github.com/stretchr/testify/assert"
)

func TestUnmarshallUpdateStatus(t *testing.T) {
	update_string := "Starting update to 0.4.0\n"
	cases := []struct {
		Name             string
		UpdateStatusJson []byte
		Expected         updateStatus
	}{
		{
			Name:             "No update available after refresh",
			UpdateStatusJson: []byte(`{"update_state":"Idle","available_updates":["0.4.0","0.3.4","0.3.3","0.3.2","0.3.1","0.3.0"],"chosen_update":null,"active_partition":{"image":{"arch":"x86_64","version":"0.4.0","variant":"aws-k8s-1.15"},"next_to_boot":true},"staging_partition":null,"most_recent_command":{"cmd_type":"refresh","cmd_status":"Success","timestamp":"2020-07-08T21:32:35.802253160Z","exit_status":0,"stderr":""}}`),
			Expected: updateStatus{
				UpdateState:      stateIdle,
				AvailableUpdates: []string{"0.4.0", "0.3.4", "0.3.3", "0.3.2", "0.3.1", "0.3.0"},
				ChosenUpdate:     nil,
				ActivePartition: &stagedImage{
					Image: updateImage{
						Arch:    "x86_64",
						Version: "0.4.0",
						Variant: "aws-k8s-1.15",
					},
					NextToBoot: true,
				},
				StagingPartition: nil,
				MostRecentCommand: &commandResult{
					CmdType:    commandRefresh,
					CmdStatus:  statusSuccess,
					Timestamp:  "2020-07-08T21:32:35.802253160Z",
					ExitStatus: new(int32),
					Stderr:     new(string),
				},
			},
		},
		{
			Name:             "Update available after refresh",
			UpdateStatusJson: []byte(`{"update_state":"Available","available_updates":["0.4.0","0.3.4","0.3.3","0.3.2","0.3.1","0.3.0"],"chosen_update":{"arch":"x86_64","version":"0.4.0","variant":"aws-k8s-1.15"},"active_partition":{"image":{"arch":"x86_64","version":"0.3.2","variant":"aws-k8s-1.15"},"next_to_boot":true},"staging_partition":null,"most_recent_command":{"cmd_type":"refresh","cmd_status":"Success","timestamp":"2020-06-18T17:57:43.141433622Z","exit_status":0,"stderr":""}}`),
			Expected: updateStatus{
				UpdateState:      stateAvailable,
				AvailableUpdates: []string{"0.4.0", "0.3.4", "0.3.3", "0.3.2", "0.3.1", "0.3.0"},
				ChosenUpdate: &updateImage{
					Arch:    "x86_64",
					Version: "0.4.0",
					Variant: "aws-k8s-1.15",
				},
				ActivePartition: &stagedImage{
					Image: updateImage{
						Arch:    "x86_64",
						Version: "0.3.2",
						Variant: "aws-k8s-1.15",
					},
					NextToBoot: true,
				},
				StagingPartition: nil,
				MostRecentCommand: &commandResult{
					CmdType:    commandRefresh,
					CmdStatus:  statusSuccess,
					Timestamp:  "2020-06-18T17:57:43.141433622Z",
					ExitStatus: new(int32),
					Stderr:     new(string),
				},
			},
		},
		{
			Name:             "Update staged",
			UpdateStatusJson: []byte(`{"update_state":"Staged","available_updates":["0.4.0","0.3.4","0.3.3","0.3.2","0.3.1","0.3.0"],"chosen_update":{"arch":"x86_64","version":"0.4.0","variant":"aws-k8s-1.15"},"active_partition":{"image":{"arch":"x86_64","version":"0.3.4","variant":"aws-k8s-1.15"},"next_to_boot":true},"staging_partition":{"image":{"arch":"x86_64","version":"0.4.0","variant":"aws-k8s-1.15"},"next_to_boot":false},"most_recent_command":{"cmd_type":"prepare","cmd_status":"Success","timestamp":"2020-07-10T06:44:58.766493367Z","exit_status":0,"stderr":"Starting update to 0.4.0\n"}}`),
			Expected: updateStatus{
				UpdateState:      stateStaged,
				AvailableUpdates: []string{"0.4.0", "0.3.4", "0.3.3", "0.3.2", "0.3.1", "0.3.0"},
				ChosenUpdate: &updateImage{
					Arch:    "x86_64",
					Version: "0.4.0",
					Variant: "aws-k8s-1.15",
				},
				ActivePartition: &stagedImage{
					Image: updateImage{
						Arch:    "x86_64",
						Version: "0.3.4",
						Variant: "aws-k8s-1.15",
					},
					NextToBoot: true,
				},
				StagingPartition: &stagedImage{
					Image: updateImage{
						Arch:    "x86_64",
						Version: "0.4.0",
						Variant: "aws-k8s-1.15",
					},
					NextToBoot: false,
				},
				MostRecentCommand: &commandResult{
					CmdType:    commandPrepare,
					CmdStatus:  statusSuccess,
					Timestamp:  "2020-07-10T06:44:58.766493367Z",
					ExitStatus: new(int32),
					Stderr:     &update_string,
				},
			},
		},
		{
			Name:             "Update ready",
			UpdateStatusJson: []byte(`{"update_state":"Ready","available_updates":["0.4.0","0.3.4","0.3.3","0.3.2","0.3.1","0.3.0"],"chosen_update":{"arch":"x86_64","version":"0.4.0","variant":"aws-k8s-1.15"},"active_partition":{"image":{"arch":"x86_64","version":"0.3.4","variant":"aws-k8s-1.15"},"next_to_boot":false},"staging_partition":{"image":{"arch":"x86_64","version":"0.4.0","variant":"aws-k8s-1.15"},"next_to_boot":true},"most_recent_command":{"cmd_type":"activate","cmd_status":"Success","timestamp":"2020-07-10T06:47:19.903337270Z","exit_status":0,"stderr":""}}`),
			Expected: updateStatus{
				UpdateState:      stateReady,
				AvailableUpdates: []string{"0.4.0", "0.3.4", "0.3.3", "0.3.2", "0.3.1", "0.3.0"},
				ChosenUpdate: &updateImage{
					Arch:    "x86_64",
					Version: "0.4.0",
					Variant: "aws-k8s-1.15",
				},
				ActivePartition: &stagedImage{
					Image: updateImage{
						Arch:    "x86_64",
						Version: "0.3.4",
						Variant: "aws-k8s-1.15",
					},
					NextToBoot: false,
				},
				StagingPartition: &stagedImage{
					Image: updateImage{
						Arch:    "x86_64",
						Version: "0.4.0",
						Variant: "aws-k8s-1.15",
					},
					NextToBoot: true,
				},
				MostRecentCommand: &commandResult{
					CmdType:    commandActivate,
					CmdStatus:  statusSuccess,
					Timestamp:  "2020-07-10T06:47:19.903337270Z",
					ExitStatus: new(int32),
					Stderr:     new(string),
				},
			},
		},
	}
	for _, tc := range cases {
		t.Run(tc.Name, func(t *testing.T) {
			var unmarshaledStatus updateStatus
			err := json.Unmarshal(tc.UpdateStatusJson, &unmarshaledStatus)
			assert.NoError(t, err, "failed to unmarshal into update status")
			assert.Equal(t, tc.Expected, unmarshaledStatus)
		})
	}
}
