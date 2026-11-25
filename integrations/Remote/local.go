package main

import (
	"context"
	"crypto/sha256"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"time"
)

type localStatus struct {
	SystemTailnet bool                `json:"system_tailnet,omitempty"`
	Device        string              `json:"device"`
	Node          string              `json:"node,omitempty"`
	Build         int                 `json:"build"`
	Enabled       bool                `json:"enabled"`
	Network       string              `json:"network"`
	AuthURL       string              `json:"auth_url,omitempty"`
	Error         string              `json:"error,omitempty"`
	Requests      []pairingRequest    `json:"requests"`
	Trusted       []peer              `json:"trusted"`
	Update        updateStatus        `json:"update"`
	Screenshots   []screenshotRequest `json:"screenshots"`
}

const maxLocalRequest = (416 << 10) + 16384

type localRequest struct {
	Action       string          `json:"action"`
	Node         string          `json:"node,omitempty"`
	Device       string          `json:"device,omitempty"`
	Update       bool            `json:"update,omitempty"`
	Control      bool            `json:"control,omitempty"`
	Terminal     bool            `json:"terminal,omitempty"`
	Command      json.RawMessage `json:"command,omitempty"`
	Project      string          `json:"project,omitempty"`
	Chat         string          `json:"chat,omitempty"`
	Cursor       string          `json:"cursor,omitempty"`
	Canonical    bool            `json:"canonical,omitempty"`
	Screenshot   string          `json:"screenshot,omitempty"`
	Path         string          `json:"path,omitempty"`
	File         string          `json:"file,omitempty"`
	Text         string          `json:"text,omitempty"`
	Revision     string          `json:"revision,omitempty"`
	Code         string          `json:"code,omitempty"`
	StreamCursor streamCursor    `json:"stream_cursor"`
}

func socketDir(root string) string {
	key := sha256.Sum256([]byte(root))
	return fmt.Sprintf("/tmp/kai-%d-%x", os.Getuid(), key[:9])
}

func listenLocal(root, name string) (net.Listener, error) {
	dir := socketDir(root)
	if err := privateDir(dir); err != nil {
		return nil, err
	}
	path := filepath.Join(dir, name)
	if err := os.Remove(path); err != nil && !errors.Is(err, os.ErrNotExist) {
		return nil, err
	}
	listener, err := net.Listen("unix", path)
	if err != nil {
		return nil, err
	}
	if err := os.Chmod(path, 0600); err != nil {
		listener.Close()
		return nil, err
	}
	return listener, nil
}

func callLocal(ctx context.Context, root, name string, raw []byte) ([]byte, error) {
	dir := socketDir(root)
	if err := privateDir(dir); err != nil {
		return nil, err
	}
	dialer := net.Dialer{}
	transport := &http.Transport{DialContext: func(ctx context.Context, _, _ string) (net.Conn, error) {
		return dialer.DialContext(ctx, "unix", filepath.Join(dir, name))
	}, DisableKeepAlives: true}
	client := http.Client{Transport: transport}
	request, err := http.NewRequestWithContext(ctx, http.MethodPost, "http://kai.local/", strings.NewReader(string(raw)))
	if err != nil {
		return nil, err
	}
	response, err := client.Do(request)
	if err != nil {
		return nil, err
	}
	defer response.Body.Close()
	out, err := io.ReadAll(io.LimitReader(response.Body, maxResponse+1))
	if err != nil {
		return nil, err
	}
	if len(out) > maxResponse {
		return nil, errors.New("Kai server response exceeded its limit")
	}
	if response.StatusCode != 200 {
		var issue struct {
			Error string `json:"error"`
		}
		if json.Unmarshal(out, &issue) == nil && issue.Error != "" {
			return nil, errors.New(issue.Error)
		}
		return nil, fmt.Errorf("Kai server returned HTTP %d", response.StatusCode)
	}
	return out, nil
}

func (s *service) status(ctx context.Context) localStatus {
	value := s.state.snapshot()
	result := localStatus{Device: value.Device, Node: value.Node, Build: s.build, Enabled: value.Enabled, Network: "Disabled",
		Requests: []pairingRequest{}, Trusted: []peer{}}
	result.SystemTailnet = value.SystemTailnet
	result.Screenshots = s.screenshots.pending()
	if s.updater != nil {
		result.Update = s.updater.status()
	}
	for _, p := range value.Trusted {
		result.Trusted = append(result.Trusted, p)
	}
	for _, r := range value.Requests {
		if time.Now().Before(r.Expires) {
			result.Requests = append(result.Requests, r)
		}
	}
	slices.SortFunc(result.Trusted, func(a, b peer) int { return strings.Compare(a.Name, b.Name) })
	slices.SortFunc(result.Requests, func(a, b pairingRequest) int { return a.Expires.Compare(b.Expires) })
	if !value.Enabled {
		return result
	}
	s.mu.Lock()
	lc, issue := s.client, s.networkError
	s.mu.Unlock()
	result.Network = "Starting"
	if issue != "" {
		result.Error = issue
		result.Network = "Reconnecting"
		if issue == errNetworkState.Error() || issue == errNodeChanged.Error() {
			result.Network = "Unavailable"
		}
	}
	if lc == nil {
		return result
	}
	probe, cancel := context.WithTimeout(ctx, 3*time.Second)
	defer cancel()
	status, err := lc.StatusWithoutPeers(probe)
	if err != nil {
		result.Error = "Could not read tailnet status"
		return result
	}
	result.Network = status.BackendState
	if issue := tailnetIssue(status, value.Node != ""); issue != "" {
		result.Error = issue
	}
	if value.Node != "" && (status.BackendState == "NoState" || status.BackendState == "Starting" ||
		status.BackendState == "Stopped" || status.BackendState == "Running" && status.Self != nil && !status.Self.Online) {
		result.Network = "Reconnecting"
	}
	if status.AuthURL != "" && value.Node == "" {
		if parsed, err := url.Parse(status.AuthURL); err == nil && parsed.Scheme == "https" && parsed.Hostname() == "login.tailscale.com" && parsed.User == nil {
			result.AuthURL = parsed.String()
		}
	}
	if status.BackendState == "Running" && status.Self != nil {
		if err := s.recordNode(string(status.Self.ID)); err != nil {
			result.Network = "Unavailable"
			result.Error = err.Error()
		} else {
			result.Node = string(status.Self.ID)
		}
	}
	return result
}

func (s *service) local(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost || r.URL.Path != "/" || r.Host != "kai.local" {
		http.NotFound(w, r)
		return
	}
	var req localRequest
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, maxLocalRequest)).Decode(&req); err != nil {
		reject(w, 400, "Invalid Kai request")
		return
	}
	ctx := r.Context()
	var result any = map[string]bool{"ok": true}
	var err error
	switch req.Action {
	case "health":
		result = map[string]any{"build": s.build, "device": s.state.snapshot().Device}
	case "status":
		result = s.status(ctx)
	case "enable":
		if !s.state.snapshot().Enabled {
			err = s.state.change(func(i *identity) error { i.Enabled = true; return nil })
			if err == nil {
				s.wakeNetwork()
			}
		}
	case "disable":
		err = s.state.change(func(i *identity) error { i.Enabled = false; return nil })
		if err == nil {
			s.systemTailnet.reset()
			s.wakeNetwork()
		}
	case "enable-system-tailnet", "disable-system-tailnet":
		if !s.state.snapshot().Enabled {
			err = errors.New("Enable Remote Access before configuring this Mac’s Tailscale connection.")
		} else {
			err = s.state.change(func(i *identity) error {
				i.SystemTailnet = req.Action == "enable-system-tailnet"
				return nil
			})
			if err == nil && req.Action == "disable-system-tailnet" {
				s.systemTailnet.reset()
			}
		}
	case "peers":
		result, err = s.discover(ctx)
	case "local-projects":
		var data metadata
		data, err = s.history.browse(ctx, "", 0)
		result = data.Projects
	case "allow-workspace":
		err = s.decide(req.Node, req.Device, "allow", false, true, time.Now())
	case "allow", "deny":
		if req.Project != "" && !s.history.allowsProject(ctx, req.Project) {
			err = errors.New("Choose a registered Kai project to share")
		} else {
			err = s.decideFiles(req.Node, req.Device, req.Action, req.Update, req.Control, req.Terminal, req.Project, time.Now())
		}
	case "revoke":
		err = s.revoke(req.Node, req.Device)
	case "generate-code":
		result, err = s.createCode()
	case "pair", "request-control", "request-project", "redeem-code", "disconnect", "workspace", "workspace-overview", "messages", "activity", "command", "run-controls", "artifacts", "terminals", "terminal-start", "request-screenshot", "remote-update-status", "remote-check-update", "remote-update":
		result, err = s.remoteCall(ctx, req)
	case "files", "file-search", "file-diff", "file-upload", "composer-upload", "file-preview", "file-save", "file-download":
		result, err = s.clientFile(ctx, req)
	case "resolve-screenshot":
		err = s.screenshots.resolve(req.Screenshot, req.Path, func(p peer) bool { return s.state.isAllowed(p.Node, p.Device) })
	case "fetch-screenshot":
		var image []byte
		image, err = s.pullScreenshot(ctx, req.Node, req.Device, req.Screenshot)
		if err == nil {
			writePNG(w, image)
			return
		}
	case "check-update", "import-update":
		if s.updater == nil {
			err = errors.New("Updater unavailable")
		} else {
			if req.Action == "import-update" && req.Path == "" {
				err = errors.New("Select a Kai update ZIP")
			} else {
				err = s.updater.scheduleCheck(req.Path)
			}
		}
	case "activate-update":
		if s.updater == nil {
			err = errors.New("Updater unavailable")
		} else {
			err = s.updater.activate(ctx, false)
		}
	default:
		reject(w, 400, "Unknown Kai action")
		return
	}
	if err != nil {
		reject(w, 409, err.Error())
		return
	}
	respond(w, 200, result)
}

func serve(ctx context.Context, root, data, app string) error {
	if err := privateDir(root); err != nil {
		return err
	}
	lock, err := lockFile(filepath.Join(root, "server.lock"))
	if err != nil {
		return fmt.Errorf("Kai server already owns this installation: %w", err)
	}
	defer lock.Close()
	build, err := strconv.Atoi(appBuild)
	if err != nil {
		return err
	}
	s, err := newService(ctx, root, data, build)
	if err != nil {
		return err
	}
	s.app = app
	defer s.inference.stop()
	go s.resumeRemotePrompts(ctx)
	s.ptySlots = make(chan struct{}, 12)
	if changes, err := newFileChanges(ctx); err == nil {
		s.changes, s.streamSlots = changes, make(chan struct{}, 16)
		unwatch := changes.watch(data)
		defer unwatch()
	}
	s.updater = makeUpdater(root, app, build)
	if s.updater != nil {
		go s.updater.run(ctx)
	}
	listener, err := listenLocal(root, "server.sock")
	if err != nil {
		return err
	}
	networkCtx, stopNetwork := context.WithCancel(ctx)
	networkDone := make(chan struct{})
	go func() {
		defer close(networkDone)
		s.runNetwork(networkCtx, s.openNetwork, 5*time.Second, 45*time.Second)
	}()
	defer func() {
		stopNetwork()
		<-networkDone
	}()
	httpMux := http.NewServeMux()
	httpMux.HandleFunc("/stream", s.proxyEvents)
	httpMux.HandleFunc("/terminal", s.proxyTerminal)
	httpMux.HandleFunc("/"+inferencePath, s.localInference)
	httpMux.HandleFunc("/", s.local)
	server := httpServer(httpMux)
	go func() { <-ctx.Done(); _ = server.Close() }()
	err = server.Serve(listener)
	if errors.Is(err, http.ErrServerClosed) {
		return nil
	}
	return err
}
