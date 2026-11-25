package main

import (
	"context"
	"encoding/json"
	"errors"
	"math/rand/v2"
	"os"
	"strings"
	"time"

	"tailscale.com/ipn"
	"tailscale.com/ipn/ipnstate"
	"tailscale.com/types/key"
)

var (
	errNetworkStart   = errors.New("Kai could not start Tailscale. Retrying automatically.")
	errNetworkState   = errors.New("Kai’s saved Tailscale credentials are missing or invalid. Restore the original server state.")
	errNodeChanged    = errors.New("The enrolled Tailscale identity changed. Restore the original server state.")
	errTailnetOffline = errors.New("Tailscale is offline. Check the connection in Remote Devices.")
)

const screenTimeIssue = "macOS Screen Time is blocking Kai’s Tailscale connection. Contact whoever manages Screen Time."

func verifyTailnetState(path, node string) error {
	payload, err := os.ReadFile(path)
	if err != nil {
		return errNetworkState
	}
	var saved map[ipn.StateKey][]byte
	var machine key.MachinePrivate
	if json.Unmarshal(payload, &saved) != nil ||
		machine.UnmarshalText(saved[ipn.MachineKeyStateKey]) != nil || machine.IsZero() {
		return errNetworkState
	}
	var profiles map[ipn.ProfileID]ipn.LoginProfile
	profileKey := ipn.StateKey(saved[ipn.CurrentProfileStateKey])
	if profileKey == "" || json.Unmarshal(saved[ipn.KnownProfilesStateKey], &profiles) != nil {
		return errNetworkState
	}
	for _, profile := range profiles {
		if profile.Key != profileKey {
			continue
		}
		if profile.NodeID != "" && string(profile.NodeID) != node {
			return errNodeChanged
		}
		prefs := ipn.NewPrefs()
		if data := saved[profileKey]; len(data) != 0 && ipn.PrefsFromBytes(data, prefs) == nil &&
			!prefs.LoggedOut && prefs.Persist != nil && !prefs.Persist.PrivateNodeKey.IsZero() {
			return nil
		}
		break
	}
	return errNetworkState
}

func tailnetIssue(status *ipnstate.Status, enrolled bool) string {
	for _, warning := range status.Health {
		if strings.Contains(warning, "macOS Screen Time") {
			return screenTimeIssue
		}
	}
	switch status.BackendState {
	case ipn.NeedsMachineAuth.String():
		return "Approve this Kai device in the Tailscale admin console."
	case ipn.NeedsLogin.String():
		if enrolled {
			return "Tailscale needs to reauthenticate this Kai node. Keep the saved server state and review the node in the Tailscale admin console."
		}
	}
	if status.Self != nil && status.Self.Expired {
		return "The Tailscale node key expired. Reauthenticate the existing Kai node."
	}
	return ""
}

func (s *service) wakeNetwork() {
	select {
	case s.networkWake <- struct{}{}:
	default:
	}
}

func (s *service) waitNetwork(ctx context.Context, delay time.Duration) {
	if delay == 0 {
		select {
		case <-ctx.Done():
		case <-s.networkWake:
		}
		return
	}
	timer := time.NewTimer(delay)
	defer timer.Stop()
	select {
	case <-ctx.Done():
	case <-s.networkWake:
	case <-timer.C:
	}
}

func (s *service) setNetworkError(message string) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.networkError = message
}

func (s *service) notePeerTraffic() {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.lastPeerTraffic = time.Now()
}

// One worker owns both opening and closing tsnet. Only a sustained local
// failure can recycle it; a single unreachable peer cannot trigger a restart.
func (s *service) runNetwork(ctx context.Context, open func() error, interval, grace time.Duration) {
	defer s.stopNetwork()
	var failingSince, healthySince time.Time
	retry := interval
	backoff := func() {
		s.waitNetwork(ctx, retry+rand.N(retry/4+1))
		retry = min(2*retry, 5*time.Minute)
	}
	for ctx.Err() == nil {
		identity := s.state.snapshot()
		if !identity.Enabled {
			s.stopNetwork()
			s.setNetworkError("")
			failingSince, healthySince, retry = time.Time{}, time.Time{}, interval
			s.waitNetwork(ctx, 0)
			continue
		}
		client, _ := s.tailscale()
		if client == nil {
			if err := open(); err != nil {
				s.setNetworkError(err.Error())
				backoff()
				continue
			}
			failingSince, healthySince = time.Time{}, time.Time{}
			s.setNetworkError("")
			s.waitNetwork(ctx, interval)
			continue
		}

		s.mu.Lock()
		listener, peerTraffic := s.remoteDone, s.lastPeerTraffic
		s.mu.Unlock()
		probe, cancel := context.WithTimeout(ctx, interval)
		status, err := client.StatusWithoutPeers(probe)
		cancel()
		now := time.Now()
		failed, issue := true, ""
		if err == nil {
			issue = tailnetIssue(status, identity.Node != "")
			switch status.BackendState {
			case ipn.NeedsLogin.String(), ipn.NeedsMachineAuth.String(), ipn.InUseOtherUser.String():
				// Leave an outstanding approval or sign-in alive.
				failed = false
			case ipn.Running.String():
				if status.Self != nil {
					if err := s.recordNode(string(status.Self.ID)); err != nil {
						s.setNetworkError(err.Error())
						s.stopNetwork()
						backoff()
						continue
					}
					failed = !status.Self.Online && now.Sub(peerTraffic) >= grace
				}
			}
		}
		select {
		case <-listener:
			// A terminated listener cannot recover by waiting on its backend.
			failed, failingSince = true, now.Add(-grace)
		default:
		}
		if failed {
			healthySince = time.Time{}
			if failingSince.IsZero() {
				failingSince = now
			}
			if issue == "" {
				issue = "Kai is reconnecting to Tailscale automatically."
			}
		} else {
			failingSince = time.Time{}
			if healthySince.IsZero() {
				healthySince = now
			}
			if now.Sub(healthySince) >= grace {
				retry = interval
			}
		}
		s.setNetworkError(issue)
		if failed && now.Sub(failingSince) >= grace {
			s.stopNetwork()
			backoff()
			continue
		}
		s.waitNetwork(ctx, interval)
	}
}
