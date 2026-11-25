package main

import (
	"crypto/sha256"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"sync"
	"time"
)

type liveOutput struct {
	Sequence int64  `json:"sequence"`
	Tail     string `json:"tail,omitempty"`
	Chat     string `json:"chat"`
	Status   string `json:"status"`
	Owner    string `json:"-"`
}

var ownerTempDirectories = sync.OnceValue(func() []string {
	dirs := []string{os.TempDir(), "/tmp"}
	if runtime.GOOS == "darwin" {
		if result, err := exec.Command("/usr/bin/getconf", "DARWIN_USER_TEMP_DIR").Output(); err == nil {
			dirs = append(dirs, strings.TrimSpace(string(result)))
		}
	}
	return dirs
})

func ownerSocketName(owner string) string {
	digest := sha256.Sum256([]byte(owner))
	return fmt.Sprintf("kai-run-%x.sock", digest[:12])
}

func ownerReachable(owner string) bool {
	name := ownerSocketName(owner)
	for _, directory := range ownerTempDirectories() {
		if !filepath.IsAbs(directory) {
			continue
		}
		path := filepath.Join(directory, name)
		info, err := os.Lstat(path)
		if err != nil || info.Mode()&os.ModeSocket == 0 {
			continue
		}
		connection, err := net.DialTimeout("unix", path, 30*time.Millisecond)
		if err == nil {
			_ = connection.Close()
			return true
		}
	}
	return false
}

func (w workspaceHistory) activity(activeChat string) ([]liveOutput, error) {
	result := []liveOutput{}
	file, err := os.Open(filepath.Join(w.root, "workspace.json"))
	if errors.Is(err, os.ErrNotExist) {
		return result, nil
	}
	if err != nil {
		return nil, err
	}
	defer file.Close()
	var workspace struct {
		Continuations []struct {
			Session struct {
				ID      string `json:"id"`
				Project string `json:"project_path"`
			} `json:"session"`
			Durable string `json:"durable"`
		} `json:"continuations"`
	}
	if err := json.NewDecoder(io.LimitReader(file, 8<<20)).Decode(&workspace); err != nil {
		return nil, err
	}
	reg, err := w.registry()
	if err != nil {
		return nil, err
	}
	path := filepath.Join(w.root, "session-owners")
	root, err := os.OpenRoot(path)
	if errors.Is(err, os.ErrNotExist) {
		return result, nil
	}
	if err != nil {
		return nil, err
	}
	defer root.Close()
	for _, entry := range workspace.Continuations {
		id := entry.Session.ID
		if id == "" || activeChat != "" && activeChat != id || contains(reg.Hidden, entry.Session.Project) || contains(reg.HiddenSessions, id) {
			continue
		}
		dir, err := filepath.Rel(path, entry.Durable)
		if err != nil || strings.Contains(dir, string(filepath.Separator)) || !validDevice.MatchString(dir) {
			continue
		}
		f, err := root.Open(filepath.Join(dir, "snapshot.json"))
		if err != nil {
			continue
		}
		var snapshot struct {
			Sequence int64             `json:"sequence"`
			Done     bool              `json:"done"`
			Error    string            `json:"error"`
			Tail     string            `json:"tail"`
			Events   []json.RawMessage `json:"events"`
		}
		err = json.NewDecoder(io.LimitReader(f, 4<<20)).Decode(&snapshot)
		f.Close()
		if err != nil {
			continue
		}
		if snapshot.Done && activeChat == "" || !snapshot.Done && !ownerReachable(filepath.Join(path, dir)) {
			continue
		}
		status := "Running"
		if snapshot.Done {
			status = "Finished"
		}
		if snapshot.Error != "" {
			status = "Failed"
		}
		for _, event := range snapshot.Events {
			if strings.HasPrefix(string(event), `{"Request":`) && !snapshot.Done {
				status = "Awaiting approval"
				break
			}
		}
		tail := ""
		if activeChat != "" {
			tail = snapshot.Tail
		}
		result = append(result, liveOutput{Chat: id, Sequence: snapshot.Sequence, Status: status, Tail: tail, Owner: filepath.Join(path, dir)})
	}
	return result, nil
}

func contains(values []string, want string) bool {
	for _, value := range values {
		if value == want {
			return true
		}
	}
	return false
}
