//go:build darwin

package main

import (
	"context"
	"encoding/json"
	"encoding/xml"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"sync/atomic"
	"time"

	"golang.org/x/sys/unix"
)

type installation struct {
	App             string `json:"app"`
	Requirement     string `json:"requirement"`
	SupervisorBuild int    `json:"supervisor_build,omitempty"`
}

type activationRequest struct {
	Build  int  `json:"build"`
	Remote bool `json:"remote,omitempty"`
	Reload bool `json:"reload,omitempty"`
}

type activationJournal struct {
	Build         int    `json:"build"`
	PreviousBuild int    `json:"previous_build"`
	StagedApp     string `json:"staged_app"`
	Remote        bool   `json:"remote"`
	Committed     bool   `json:"committed,omitempty"`
}

func helper(app string) string { return filepath.Join(app, "Contents", "MacOS", "kai-server") }

func readInstallation(root string) (installation, error) {
	var i installation
	data, err := os.ReadFile(filepath.Join(root, "installation.json"))
	if err != nil {
		return i, err
	}
	err = json.Unmarshal(data, &i)
	return i, err
}

func bootstrap(ctx context.Context, root, data, app string) error {
	if err := privateDir(root); err != nil {
		return err
	}
	lock, err := lockFile(filepath.Join(root, "bootstrap.lock"))
	if err != nil {
		return errors.New("Kai Server setup is already running")
	}
	defer lock.Close()
	i, err := readInstallation(root)
	if errors.Is(err, os.ErrNotExist) {
		req, newErr := designatedRequirement(ctx, app)
		if newErr != nil {
			return newErr
		}
		if err := verifySignature(ctx, app, req); err != nil {
			return err
		}
		i = installation{App: app, Requirement: req}
		if err := saveJSON(filepath.Join(root, "installation.json"), i); err != nil {
			return err
		}
	} else if err != nil {
		return err
	}
	if app != i.App {
		return fmt.Errorf("Kai Server is already installed for %s. Open that copy to manage this Mac.", i.App)
	}
	build, err := bundleBuild(ctx, app)
	if err != nil {
		return err
	}
	launchRoot := filepath.Join(filepath.Dir(filepath.Dir(data)), "LaunchAgents")
	if err := os.MkdirAll(launchRoot, 0755); err != nil {
		return err
	}
	supervisorPath := filepath.Join(root, "kai-supervisor")
	_, supervisorErr := os.Stat(supervisorPath)
	if supervisorErr != nil && !errors.Is(supervisorErr, os.ErrNotExist) {
		return supervisorErr
	}
	if supervisorErr != nil || i.SupervisorBuild != build {
		if err := verifySignature(ctx, app, i.Requirement); err != nil {
			return err
		}
		if err := copyHelper(helper(app), supervisorPath); err != nil {
			return err
		}
		i.SupervisorBuild = build
		if err := saveJSON(filepath.Join(root, "installation.json"), i); err != nil {
			return err
		}
	}
	plist := filepath.Join(launchRoot, serviceLabel+".plist")
	domain := "gui/" + strconv.Itoa(os.Getuid())
	target := domain + "/" + serviceLabel
	if exec.CommandContext(ctx, "/bin/launchctl", "print", target).Run() != nil {
		if err := writeLaunchAgent(plist, supervisorPath, root, data, app); err != nil {
			return err
		}
		if out, err := exec.CommandContext(ctx, "/bin/launchctl", "bootstrap", domain, plist).CombinedOutput(); err != nil {
			return fmt.Errorf("Could not start Kai Server through launchd: %s", strings.TrimSpace(string(out)))
		}
	}
	health, err := waitHealthFor(ctx, root, 0, 90*time.Second)
	if err != nil {
		return err
	}
	if health != build {
		raw, _ := json.Marshal(activationRequest{Build: build, Reload: true})
		if _, err := callLocal(ctx, root, "supervisor.sock", raw); err != nil {
			return err
		}
		_, err = waitHealth(ctx, root, build)
	}
	return err
}

func copyHelper(source, dest string) error {
	input, err := os.Open(source)
	if err != nil {
		return err
	}
	defer input.Close()
	f, err := os.CreateTemp(filepath.Dir(dest), ".supervisor-*")
	if err != nil {
		return err
	}
	defer os.Remove(f.Name())
	_, err = io.Copy(f, input)
	if err == nil {
		err = f.Chmod(0700)
	}
	if err == nil {
		err = f.Sync()
	}
	if closeErr := f.Close(); err == nil {
		err = closeErr
	}
	if err != nil {
		return err
	}
	return os.Rename(f.Name(), dest)
}

func writeLaunchAgent(path, executable, root, data, app string) error {
	var args strings.Builder
	for _, arg := range []string{executable, "supervise", root, data, app} {
		args.WriteString("<string>")
		_ = xml.EscapeText(&args, []byte(arg))
		args.WriteString("</string>")
	}
	var logPath strings.Builder
	_ = xml.EscapeText(&logPath, []byte(filepath.Join(root, "supervisor.log")))
	plist := `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>Label</key><string>` + serviceLabel + `</string>
<key>ProgramArguments</key><array>` + args.String() + `</array>
<key>RunAtLoad</key><true/><key>KeepAlive</key><true/>
<key>ThrottleInterval</key><integer>10</integer>
<key>LimitLoadToSessionType</key><string>Aqua</string>
<key>StandardErrorPath</key><string>` + logPath.String() + `</string></dict></plist>`
	f, err := os.CreateTemp(filepath.Dir(path), ".kai-launch-*")
	if err != nil {
		return err
	}
	defer os.Remove(f.Name())
	_, err = f.WriteString(plist)
	if closeErr := f.Close(); err == nil {
		err = closeErr
	}
	if err != nil {
		return err
	}
	return os.Rename(f.Name(), path)
}

type serverProcess struct {
	cmd  *exec.Cmd
	done chan error
}

func launchServer(root, data, app string) (*serverProcess, error) {
	cmd := exec.Command(helper(app), "serve", root, data, app)
	cmd.Stderr = os.Stderr
	if err := cmd.Start(); err != nil {
		return nil, err
	}
	p := &serverProcess{cmd: cmd, done: make(chan error, 1)}
	go func() { p.done <- cmd.Wait(); close(p.done) }()
	return p, nil
}

func (p *serverProcess) stop() {
	_ = p.cmd.Process.Signal(unix.SIGTERM)
	select {
	case <-p.done:
	case <-time.After(4 * time.Second):
		_ = p.cmd.Process.Kill()
		<-p.done
	}
}

func waitHealth(ctx context.Context, root string, wantBuild int) (int, error) {
	return waitHealthFor(ctx, root, wantBuild, 15*time.Second)
}

func waitHealthFor(ctx context.Context, root string, wantBuild int, timeout time.Duration) (int, error) {
	ctx, cancel := context.WithTimeout(ctx, timeout)
	defer cancel()
	tick := time.NewTicker(100 * time.Millisecond)
	defer tick.Stop()
	for {
		probe, cancel := context.WithTimeout(ctx, 700*time.Millisecond)
		b, err := callLocal(probe, root, "server.sock", []byte(`{"action":"health"}`))
		cancel()
		var status struct {
			Build int `json:"build"`
		}
		if err == nil && json.Unmarshal(b, &status) == nil && status.Build >= minimumBuild && (wantBuild == 0 || status.Build == wantBuild) {
			return status.Build, nil
		}
		select {
		case <-ctx.Done():
			return 0, errors.New("Kai Server did not become healthy")
		case <-tick.C:
		}
	}
}

func supervise(ctx context.Context, root, data, app string) error {
	if err := privateDir(root); err != nil {
		return err
	}
	lock, err := lockFile(filepath.Join(root, "supervisor.lock"))
	if err != nil {
		return err
	}
	defer lock.Close()
	i, err := readInstallation(root)
	if err != nil {
		return err
	}
	if i.App != app {
		return errors.New("Kai installation path does not match")
	}
	if err := recoverActivation(ctx, root, app); err != nil {
		return err
	}
	if err := verifySignature(ctx, app, i.Requirement); err != nil {
		return err
	}
	listener, err := listenLocal(root, "supervisor.sock")
	if err != nil {
		return err
	}
	activation := make(chan activationRequest, 1)
	var activating atomic.Bool
	api := httpServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost || r.URL.Path != "/" || r.Host != "kai.local" {
			http.NotFound(w, r)
			return
		}
		var req activationRequest
		if json.NewDecoder(http.MaxBytesReader(w, r.Body, 4096)).Decode(&req) != nil || req.Build < minimumBuild {
			reject(w, 400, "Invalid activation request")
			return
		}
		if !activating.CompareAndSwap(false, true) {
			reject(w, 409, "A Kai update is already queued")
			return
		}
		respond(w, 200, map[string]bool{"accepted": true})
		if f, ok := w.(http.Flusher); ok {
			f.Flush()
		}
		activation <- req
	}))
	defer api.Close()
	go func() { _ = api.Serve(listener) }()
	p, err := launchServer(root, data, app)
	if err != nil {
		return err
	}
	defer func() {
		if p != nil {
			p.stop()
		}
	}()
	heartbeat := time.NewTicker(20 * time.Second)
	defer heartbeat.Stop()
	missed := 0
	for {
		select {
		case <-ctx.Done():
			return nil
		case err := <-p.done:
			return fmt.Errorf("Kai Server exited: %v", err)
		case <-heartbeat.C:
			if activating.Load() {
				continue
			}
			if _, err := waitHealthFor(ctx, root, 0, time.Second); err == nil {
				missed = 0
			} else {
				missed++
				if missed == 3 {
					return errors.New("Kai Server stopped responding. Launchd will restart it.")
				}
			}
		case request := <-activation:
			missed = 0
			if request.Reload {
				installed, err := bundleBuild(ctx, app)
				if err != nil || installed != request.Build || verifySignature(ctx, app, i.Requirement) != nil {
					activating.Store(false)
					continue
				}
				p.stop()
				next, err := launchServer(root, data, app)
				if err != nil {
					return err
				}
				p = next
				activating.Store(false)
				continue
			}
			p, err = activateReplacement(ctx, root, data, app, p, request)
			activating.Store(false)
			if err != nil {
				_ = saveJSON(filepath.Join(root, "update-result.json"), updateStatus{State: "error", Message: err.Error()})
				fmt.Fprintln(os.Stderr, err)
				if p == nil {
					return err
				}
			}
		}
	}
}

func stagePathOK(app, staged string) bool {
	dir := filepath.Dir(staged)
	return filepath.Base(staged) == "Kai Unified.app" && filepath.Dir(dir) == filepath.Dir(app) &&
		strings.HasPrefix(filepath.Base(dir), ".kai-update-") && privateDir(dir) == nil
}

func recoverActivation(ctx context.Context, root, app string) error {
	journal := filepath.Join(root, "activation.json")
	b, err := os.ReadFile(journal)
	if errors.Is(err, os.ErrNotExist) {
		return nil
	}
	if err != nil {
		return err
	}
	var j activationJournal
	if err := json.Unmarshal(b, &j); err != nil {
		return err
	}
	if !stagePathOK(app, j.StagedApp) {
		return errors.New("Unsafe Kai rollback path")
	}
	current, err := bundleBuild(ctx, app)
	if err != nil {
		return err
	}
	if current == j.Build && j.Committed {
		return finishActivation(root, j)
	}
	if current == j.Build {
		previous, err := bundleBuild(ctx, j.StagedApp)
		if err != nil || previous != j.PreviousBuild {
			return errors.New("The previous Kai build is missing. Restore the original bundle from backup.")
		}
		if err := unix.RenameatxNp(unix.AT_FDCWD, app, unix.AT_FDCWD, j.StagedApp, unix.RENAME_SWAP); err != nil {
			return err
		}
	} else if current != j.PreviousBuild {
		return errors.New("The installed Kai build changed during update recovery")
	}
	if err := os.Remove(journal); err != nil {
		return err
	}
	_ = os.Remove(filepath.Join(root, "prepared.json"))
	return saveJSON(filepath.Join(root, "update-result.json"), updateStatus{State: "error", Message: "An interrupted update was rolled back."})
}

func activateReplacement(ctx context.Context, root, data, app string, p *serverProcess, req activationRequest) (*serverProcess, error) {
	stage, err := readPrepared(root)
	if err != nil || stage.Build != req.Build || !stagePathOK(app, stage.App) {
		return p, errors.New("The staged Kai update changed or is missing")
	}
	current, err := bundleBuild(ctx, app)
	if err != nil || stage.Build <= current {
		return p, errors.New("An update must advance the installed Kai build")
	}
	if err := verifyReplacement(ctx, app, stage.App, stage.Build); err != nil {
		return p, err
	}
	j := activationJournal{Build: stage.Build, PreviousBuild: current, StagedApp: stage.App, Remote: req.Remote}
	journal := filepath.Join(root, "activation.json")
	if err := saveJSON(journal, j); err != nil {
		return p, err
	}
	p.stop()
	err = unix.RenameatxNp(unix.AT_FDCWD, app, unix.AT_FDCWD, stage.App, unix.RENAME_SWAP)
	var next *serverProcess
	if err == nil {
		next, err = launchServer(root, data, app)
		if err == nil {
			_, err = waitHealth(ctx, root, stage.Build)
		}
	}
	if err == nil {
		select {
		case crash := <-next.done:
			err = fmt.Errorf("Updated Kai Server exited: %v", crash)
		case <-time.After(2 * time.Second):
		case <-ctx.Done():
			err = ctx.Err()
		}
	}
	if err != nil {
		if next != nil {
			next.stop()
		}
		if rollbackErr := recoverActivation(context.Background(), root, app); rollbackErr != nil {
			return nil, fmt.Errorf("Update failed: %v; rollback: %w", err, rollbackErr)
		}
		old, launchErr := launchServer(root, data, app)
		if launchErr != nil {
			return nil, launchErr
		}
		return old, fmt.Errorf("Kai update rolled back: %w", err)
	}
	j.Committed = true
	if err := saveJSON(journal, j); err != nil {
		next.stop()
		return nil, err
	}
	return next, finishActivation(root, j)
}

func finishActivation(root string, j activationJournal) error {
	result := updateStatus{State: "installed", Build: j.Build, Remote: j.Remote}
	if err := saveJSON(filepath.Join(root, "update-result.json"), result); err != nil {
		return err
	}
	_ = os.Remove(filepath.Join(root, "prepared.json"))
	return os.Remove(filepath.Join(root, "activation.json"))
}
