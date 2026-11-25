package main

import (
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"sync"
	"time"

	"golang.org/x/sys/unix"
)

const schemaVersion = 1

var validDevice = regexp.MustCompile(`^[a-f0-9]{32}$|^[a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}$`)

type peer struct {
	Node         string     `json:"node"`
	Device       string     `json:"device"`
	Name         string     `json:"name"`
	Account      string     `json:"account,omitempty"`
	Build        int        `json:"build,omitempty"`
	Compatible   bool       `json:"compatible"`
	Allowed      bool       `json:"allowed"`
	Online       bool       `json:"online"`
	Update       bool       `json:"update,omitempty"`
	Control      bool       `json:"control,omitempty"`
	Capabilities []string   `json:"capabilities,omitempty"`
	FileProject  string     `json:"file_project,omitempty"`
	Terminal     bool       `json:"terminal,omitempty"`
	Transport    string     `json:"transport,omitempty"`
	Platform     string     `json:"platform,omitempty"`
	FormFactor   string     `json:"form_factor,omitempty"`
	MutualUntil  *time.Time `json:"mutual_until,omitempty"`
}

type pairingRequest struct {
	Peer              peer      `json:"peer"`
	Expires           time.Time `json:"expires"`
	ControlRequested  bool      `json:"control_requested,omitempty"`
	ProjectRequested  string    `json:"project_requested,omitempty"`
	TerminalRequested bool      `json:"terminal_requested,omitempty"`
}

type identity struct {
	SystemTailnet bool                      `json:"system_tailnet,omitempty"`
	Schema        int                       `json:"schema"`
	Device        string                    `json:"device"`
	Node          string                    `json:"node,omitempty"`
	Enabled       bool                      `json:"enabled"`
	Trusted       map[string]peer           `json:"trusted"`
	Outgoing      map[string]peer           `json:"outgoing"`
	Requests      map[string]pairingRequest `json:"requests"`
	Codes         map[string]time.Time      `json:"codes,omitempty"`
}

type state struct {
	mu       sync.Mutex
	filename string
	identity
}

func saveJSON(path string, value any) error {
	data, err := json.Marshal(value)
	if err != nil {
		return err
	}
	file, err := os.CreateTemp(filepath.Dir(path), ".next-*")
	if err != nil {
		return err
	}
	defer os.Remove(file.Name())
	_, err = file.Write(data)
	if err == nil {
		err = file.Sync()
	}
	if closeErr := file.Close(); err == nil {
		err = closeErr
	}
	if err != nil {
		return err
	}
	if err := os.Rename(file.Name(), path); err != nil {
		return err
	}
	// The rename is the commit point. Also flush its directory where supported.
	if dir, err := os.Open(filepath.Dir(path)); err == nil {
		_ = dir.Sync()
		_ = dir.Close()
	}
	return nil
}

func openState(root, legacyID string) (*state, error) {
	s := &state{filename: filepath.Join(root, "identity.json")}
	data, err := os.ReadFile(s.filename)
	if errors.Is(err, os.ErrNotExist) {
		id := legacyID
		if !validDevice.MatchString(id) {
			var random [16]byte
			if _, err := rand.Read(random[:]); err != nil {
				return nil, err
			}
			id = hex.EncodeToString(random[:])
		}
		s.identity = identity{Schema: schemaVersion, Device: id,
			Trusted: map[string]peer{}, Outgoing: map[string]peer{}, Requests: map[string]pairingRequest{}}
		return s, saveJSON(s.filename, s.identity)
	}
	if err != nil {
		return nil, err
	}
	if err := json.Unmarshal(data, &s.identity); err != nil {
		return nil, fmt.Errorf("read identity: %w", err)
	}
	if s.Schema != schemaVersion {
		return nil, fmt.Errorf("Kai server state schema %d requires a compatible Kai build", s.Schema)
	}
	if !validDevice.MatchString(s.Device) || s.Trusted == nil || s.Outgoing == nil || s.Requests == nil {
		return nil, errors.New("Kai server identity is incomplete. Restore it from backup")
	}
	return s, nil
}

func (s *state) snapshot() identity {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.copy()
}

func (s *state) copy() identity {
	value := s.identity
	value.Trusted = make(map[string]peer, len(s.Trusted))
	for k, p := range s.Trusted {
		value.Trusted[k] = p
	}
	value.Outgoing = make(map[string]peer, len(s.Outgoing))
	for k, p := range s.Outgoing {
		value.Outgoing[k] = p
	}
	value.Requests = make(map[string]pairingRequest, len(s.Requests))
	for k, p := range s.Requests {
		value.Requests[k] = p
	}
	value.Codes = make(map[string]time.Time, len(s.Codes))
	for k, p := range s.Codes {
		value.Codes[k] = p
	}
	return value
}

func (s *state) change(update func(*identity) error) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	next := s.copy()
	if err := update(&next); err != nil {
		return err
	}
	if err := saveJSON(s.filename, next); err != nil {
		return err
	}
	s.identity = next
	return nil
}

func (s *state) isAllowed(node, device string) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	p, exists := s.Trusted[node]
	return exists && p.Device == device
}

func lockFile(path string) (*os.File, error) {
	file, err := os.OpenFile(path, os.O_CREATE|os.O_RDWR, 0600)
	if err != nil {
		return nil, err
	}
	if err = unix.Flock(int(file.Fd()), unix.LOCK_EX|unix.LOCK_NB); err != nil {
		file.Close()
		return nil, err
	}
	return file, nil
}

func privateDir(path string) error {
	if err := os.MkdirAll(path, 0700); err != nil {
		return err
	}
	info, err := os.Lstat(path)
	if err != nil {
		return err
	}
	var owner unix.Stat_t
	if err := unix.Lstat(path, &owner); err != nil {
		return err
	}
	if !info.IsDir() || info.Mode().Perm() != 0700 || int(owner.Uid) != os.Getuid() {
		return fmt.Errorf("expected a private directory owned by Kai's current user: %s", path)
	}
	return nil
}

func (s *state) terminalAllowed(node, device, project string) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	p, ok := s.Trusted[node]
	return ok && p.Device == device && p.Control && project != ""
}

func (s *state) fileAllowed(node, device, project string) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	p, ok := s.Trusted[node]
	return ok && p.Device == device && project != "" && (p.Control || p.FileProject == project)
}
