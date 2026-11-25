package main

import (
	"context"
	"crypto/sha256"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"time"
)

func (s *service) dispatchCommand(w http.ResponseWriter, r *http.Request, p peer) {
	var request json.RawMessage
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, 416<<10)).Decode(&request); err != nil {
		reject(w, 400, "Invalid workspace command")
		return
	}
	var command struct {
		Action  string `json:"action"`
		Chat    string `json:"chat_id"`
		Project string `json:"project"`
	}
	if err := json.Unmarshal(request, &command); err != nil ||
		!slices.Contains([]string{"create_chat", "prompt_chat", "create_project", "stop", "steer", "answer", "task_status", "list_artifacts", "read_artifact", "list_models", "composer_options", "voice_start", "voice_status", "voice_end", "voice_interrupt"}, command.Action) {
		reject(w, 400, "Unsupported workspace command")
		return
	}
	if command.Action != "voice_start" && len(request) > 16384 {
		var fields map[string]json.RawMessage
		var prompt string
		promptAction := command.Action == "create_chat" || command.Action == "prompt_chat"
		if !promptAction || json.Unmarshal(request, &fields) != nil ||
			json.Unmarshal(fields["prompt"], &prompt) != nil || len(prompt) > 64<<10 ||
			len(request)-len(fields["prompt"]) > 16384 {
			reject(w, 400, "Workspace command is too large")
			return
		}
	}
	if command.Action == "answer" {
		var envelope map[string]json.RawMessage
		var answer map[string]json.RawMessage
		_ = json.Unmarshal(request, &envelope)
		if err := json.Unmarshal(envelope["answer"], &answer); err != nil || answer == nil {
			reject(w, 400, "Missing approval answer")
			return
		}
		if alias := answer["approval_request_id"]; len(alias) > 0 {
			if _, exists := answer["approvalRequestId"]; exists {
				reject(w, 400, "Multiple approval request IDs")
				return
			}
			answer["approvalRequestId"] = alias
			delete(answer, "approval_request_id")
			envelope["answer"], _ = json.Marshal(answer)
			request, _ = json.Marshal(envelope)
		}
	}
	reg, err := s.history.registry()
	if err != nil {
		reject(w, 503, "Workspace configuration is unavailable")
		return
	}
	if contains(reg.Hidden, command.Project) || contains(reg.HiddenSessions, command.Chat) {
		reject(w, 404, "Workspace or chat unavailable")
		return
	}
	if command.Chat != "" {
		if !s.history.hasChat(r.Context(), command.Chat) {
			reject(w, 404, "Chat unavailable")
			return
		}
	}
	client := fmt.Sprintf("%x", sha256.Sum256([]byte(p.Node+"\x00"+p.Device)))
	input, err := json.Marshal(map[string]any{"client": client, "command": request})
	if err != nil {
		reject(w, 400, "Invalid workspace command")
		return
	}
	if err := s.ensureWorkspace(r.Context(), command.Project); err != nil {
		reject(w, 503, "Kai could not start its workspace on the host. Check that the host user is logged in, then retry with the same request ID.")
		return
	}
	trusted := s.state.snapshot().Trusted[p.Node]
	if trusted.Device != p.Device || !trusted.Control {
		reject(w, 403, "Kai control access was revoked")
		return
	}
	output, err := callLocal(r.Context(), s.root, "bridge.sock", input)
	if err != nil {
		reject(w, 503, "The workspace connection was interrupted. Retry with the same request ID to recover its receipt.")
		return
	}
	var response json.RawMessage
	if json.Unmarshal(output, &response) != nil {
		reject(w, 502, "Invalid workspace receipt")
		return
	}
	respond(w, 200, response)
}

func (s *service) ensureWorkspace(ctx context.Context, project string) error {
	payload, _ := json.Marshal(map[string]string{"operation": "health", "project": project})
	healthy := func() bool {
		probe, cancel := context.WithTimeout(ctx, 400*time.Millisecond)
		defer cancel()
		_, err := callLocal(probe, s.root, "bridge.sock", payload)
		return err == nil
	}
	if healthy() {
		return nil
	}
	if filepath.Ext(s.app) != ".app" {
		return errors.New("Kai app bundle unavailable")
	}
	s.workspaceLaunch.Lock()
	defer s.workspaceLaunch.Unlock()
	if healthy() {
		return nil
	}
	if err := exec.CommandContext(ctx, "/usr/bin/open", "-g", "-j", "-a", s.app, "--args", "--kai-remote-background").Run(); err != nil {
		return err
	}
	timer := time.NewTimer(26 * time.Second)
	defer timer.Stop()
	tick := time.NewTicker(150 * time.Millisecond)
	defer tick.Stop()
	for {
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-timer.C:
			return errors.New("Kai workspace did not become ready")
		case <-tick.C:
			if healthy() {
				return nil
			}
		}
	}
}

// The provider owner runs independently of the workspace. A hidden workspace
// resumes accepted prompts, answers delegation callbacks, and files results.
type workspaceWork struct {
	Teams []struct {
		Remote  bool `json:"remote_prompt"`
		Members []struct {
			Status    string `json:"status"`
			Attempted bool   `json:"attempted"`
		} `json:"members"`
	} `json:"teams"`
	Continuations []struct {
		Durable      string `json:"durable"`
		Orchestrator bool   `json:"orchestrator"`
	} `json:"continuations"`
}

func readWorkspaceWork(reader io.Reader) workspaceWork {
	var work workspaceWork
	if json.NewDecoder(io.LimitReader(reader, 8<<20)).Decode(&work) != nil {
		return workspaceWork{}
	}
	return work
}

func (work workspaceWork) needsWorkspace(owners string) bool {
	for _, team := range work.Teams {
		if team.Remote && len(team.Members) == 1 && team.Members[0].Status == "Queued" && !team.Members[0].Attempted {
			return true
		}
	}
	if len(work.Continuations) == 0 {
		return false
	}
	root, err := os.OpenRoot(owners)
	if err != nil {
		return false
	}
	defer root.Close()
	for _, owner := range work.Continuations {
		id := filepath.Base(owner.Durable)
		if filepath.Dir(owner.Durable) != owners || !validDevice.MatchString(id) {
			continue
		}
		f, err := root.Open(filepath.Join(id, "snapshot.json"))
		if err != nil {
			continue
		}
		var snapshot struct {
			Done bool `json:"done"`
		}
		err = json.NewDecoder(io.LimitReader(f, 4<<20)).Decode(&snapshot)
		_ = f.Close()
		if err == nil && (snapshot.Done || owner.Orchestrator) {
			return true
		}
	}
	return false
}

func (s *service) resumeRemotePrompts(ctx context.Context) {
	ticker := time.NewTicker(12 * time.Second)
	defer ticker.Stop()
	workspacePath := filepath.Join(s.history.root, "workspace.json")
	owners := filepath.Join(s.history.root, "session-owners")
	var lastFile os.FileInfo
	var work workspaceWork
	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
		}
		stat, err := os.Stat(workspacePath)
		if err != nil {
			lastFile, work = nil, workspaceWork{}
			continue
		}
		if lastFile == nil || !os.SameFile(lastFile, stat) || !lastFile.ModTime().Equal(stat.ModTime()) {
			file, err := os.Open(workspacePath)
			if err != nil {
				continue
			}
			work = readWorkspaceWork(file)
			_ = file.Close()
			lastFile = stat
		}
		if work.needsWorkspace(owners) {
			attempt, cancel := context.WithTimeout(ctx, 30*time.Second)
			_ = s.ensureWorkspace(attempt, "")
			cancel()
		}
	}
}

func (s *service) runControls(w http.ResponseWriter, r *http.Request) {
	chat := r.URL.Query().Get("chat")
	if !s.history.hasChat(r.Context(), chat) {
		reject(w, 404, "Chat unavailable")
		return
	}
	ctx, stop := context.WithTimeout(r.Context(), 2*time.Second)
	defer stop()
	payload, _ := json.Marshal(map[string]string{"operation": "controls", "chat": chat})
	if raw, err := callLocal(ctx, s.root, "bridge.sock", payload); err == nil && json.Valid(raw) {
		respond(w, 200, json.RawMessage(raw))
	} else {
		respond(w, 200, map[string]any{"tasks": []any{}, "approvals": []any{}})
	}
}
