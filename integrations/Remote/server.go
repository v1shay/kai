package main

import (
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"runtime"
	"slices"
	"strconv"
	"strings"
	"sync"
	"time"

	"tailscale.com/client/local"
	"tailscale.com/ipn/ipnstate"
	"tailscale.com/tsnet"
)

const protocol = 1
const minimumBuild = 190
const port = "47832"
const prefix = "/kai-remote/"
const maxResponse = 8 << 20

var appBuild = "190"
var nodeName = regexp.MustCompile(`[^a-z0-9-]+`)

var hostFormFactor = sync.OnceValue(func() string {
	if runtime.GOOS != "darwin" {
		return ""
	}
	ctx, cancel := context.WithTimeout(context.Background(), time.Second)
	defer cancel()
	model, _ := exec.CommandContext(ctx, "/usr/sbin/sysctl", "-n", "hw.model").Output()
	if strings.Contains(string(model), "MacBook") {
		return "laptop"
	}
	battery, err := exec.CommandContext(ctx, "/usr/sbin/ioreg", "-r", "-c", "AppleSmartBattery", "-d", "1").Output()
	if err == nil && strings.Contains(string(battery), "AppleSmartBattery") {
		return "laptop"
	}
	return "desktop"
})

type hello struct {
	Device       string   `json:"device"`
	Build        int      `json:"build"`
	Protocols    []int    `json:"protocols"`
	Capabilities []string `json:"capabilities"`
	Allowed      bool     `json:"allowed"`
	Update       bool     `json:"update,omitempty"`
	Control      bool     `json:"control,omitempty"`
	FileProject  string   `json:"file_project,omitempty"`
	Terminal     bool     `json:"terminal,omitempty"`
	Platform     string   `json:"platform,omitempty"`
	FormFactor   string   `json:"form_factor,omitempty"`
}

type service struct {
	root    string
	app     string
	state   *state
	history workspaceHistory
	build   int
	life    context.Context

	mu              sync.Mutex
	tail            *tsnet.Server
	client          *local.Client
	httpClient      *http.Client
	remoteServer    *http.Server
	remoteDone      <-chan struct{}
	networkWake     chan struct{}
	networkError    string
	lastPeerTraffic time.Time
	workspaceLaunch sync.Mutex
	terminalLaunch  sync.Mutex
	systemTailnet   systemTailnet
	fileWrite       sync.Mutex

	who          func(*http.Request) (peer, error)
	updater      *updater
	screenshots  screenshots
	changes      *fileChanges
	streamSlots  chan struct{}
	ptySlots     chan struct{}
	historySlots chan struct{}
	inference    inferenceWorker
}

func newService(ctx context.Context, root, data string, build int) (*service, error) {
	if build < minimumBuild {
		return nil, errors.New("update Kai to build 190 or newer")
	}
	if _, err := os.Stat(filepath.Join(root, "identity.json")); errors.Is(err, os.ErrNotExist) {
		if _, err := os.Stat(filepath.Join(root, "tsnet", "tailscaled.state")); err == nil {
			return nil, errors.New("Unpaired Tailscale state found. Restore Kai’s identity and tailnet state together.")
		}
	}
	legacy, _ := os.ReadFile(filepath.Join(data, "remote", "host-id"))
	state, err := openState(root, strings.TrimSpace(string(legacy)))
	if err != nil {
		return nil, err
	}
	s := &service{root: root, state: state, history: workspaceHistory{data}, build: build, life: ctx,
		networkWake: make(chan struct{}, 1), historySlots: make(chan struct{}, 2)}
	s.who = s.identify
	return s, nil
}

func (s *service) openNetwork() error {
	dir := filepath.Join(s.root, "tsnet")
	if err := privateDir(dir); err != nil {
		return err
	}
	if expected := s.state.snapshot().Node; expected != "" {
		if err := verifyTailnetState(filepath.Join(dir, "tailscaled.state"), expected); err != nil {
			return err
		}
	}
	hostname, _ := os.Hostname()
	name := strings.Trim(nodeName.ReplaceAllString(strings.ToLower(strings.Split(hostname, ".")[0]), "-"), "-")
	if name == "" {
		name = "mac"
	}
	if len(name) > 59 {
		name = strings.TrimRight(name[:59], "-")
	}
	tail := &tsnet.Server{Dir: dir, Hostname: name + "-kai", UserLogf: func(string, ...any) {}}
	if s.attachTail(tail) != nil {
		return errNetworkStart
	}
	return nil
}

func (s *service) attachTail(tail *tsnet.Server) error {
	listener, err := tail.Listen("tcp", ":"+port)
	if err != nil {
		tail.Close()
		return err
	}
	client, err := tail.LocalClient()
	if err != nil {
		listener.Close()
		tail.Close()
		return err
	}
	transport := &http.Transport{DialContext: tail.Dial, MaxIdleConns: 16, MaxIdleConnsPerHost: 2,
		IdleConnTimeout: 30 * time.Second, ResponseHeaderTimeout: 52 * time.Second, MaxResponseHeaderBytes: 16384}
	httpClient := &http.Client{Transport: transport, Timeout: 55 * time.Second, CheckRedirect: func(*http.Request, []*http.Request) error {
		return errors.New("Kai peers cannot redirect requests")
	}}
	server := httpServer(http.HandlerFunc(s.remote))
	server.BaseContext = func(net.Listener) context.Context { return s.life }
	done := make(chan struct{})
	s.mu.Lock()
	s.tail, s.client, s.httpClient = tail, client, httpClient
	s.remoteServer, s.remoteDone = server, done
	s.mu.Unlock()
	go func() {
		_ = server.Serve(listener)
		_ = server.Close()
		close(done)
	}()
	return nil
}

func (s *service) stopNetwork() {
	s.mu.Lock()
	tail, client, server, done := s.tail, s.httpClient, s.remoteServer, s.remoteDone
	s.tail, s.client, s.httpClient = nil, nil, nil
	s.remoteServer, s.remoteDone = nil, nil
	s.lastPeerTraffic = time.Time{}
	s.mu.Unlock()
	if client != nil {
		client.CloseIdleConnections()
	}
	if server != nil {
		_ = server.Close()
	}
	if tail != nil {
		_ = tail.Close()
	}
	if done != nil {
		<-done
	}
}

func (s *service) tailscale() (*local.Client, *http.Client) {
	s.mu.Lock()
	defer s.mu.Unlock()
	return s.client, s.httpClient
}

func (s *service) recordNode(node string) error {
	current := s.state.snapshot().Node
	if current == node && node != "" {
		return nil
	}
	if current != "" || node == "" {
		return errNodeChanged
	}
	return s.state.change(func(i *identity) error {
		if i.Node != "" && i.Node != node {
			return errNodeChanged
		}
		i.Node = node
		return nil
	})
}

func (s *service) identify(r *http.Request) (peer, error) {
	if !s.state.snapshot().Enabled {
		return peer{}, errTailnetOffline
	}
	client, _ := s.tailscale()
	if client == nil {
		return peer{}, errors.New("Tailscale offline")
	}
	status, err := client.StatusWithoutPeers(r.Context())
	if err != nil || status.BackendState != "Running" || status.Self == nil {
		return peer{}, errors.New("Tailscale offline")
	}
	if err := s.recordNode(string(status.Self.ID)); err != nil {
		return peer{}, err
	}
	result, err := client.WhoIs(r.Context(), r.RemoteAddr)
	if err != nil || result.Node == nil || result.Node.StableID == "" {
		return peer{}, errors.New("unauthenticated Tailscale peer")
	}
	p := peer{Node: string(result.Node.StableID), Name: strings.TrimSuffix(result.Node.ComputedName, "."),
		Device: r.Header.Get("Kai-Device"), Compatible: true, Online: true}
	if result.UserProfile != nil {
		p.Account = result.UserProfile.LoginName
	}
	s.notePeerTraffic()
	return p, nil
}

func respond(w http.ResponseWriter, status int, value any) {
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Cache-Control", "no-store")
	w.Header().Set("X-Content-Type-Options", "nosniff")
	w.WriteHeader(status)
	_ = json.NewEncoder(w).Encode(value)
}

func reject(w http.ResponseWriter, status int, detail string) {
	respond(w, status, map[string]string{"error": detail})
}

func httpServer(h http.Handler) *http.Server {
	return &http.Server{Handler: h, ReadHeaderTimeout: 3 * time.Second, ReadTimeout: 10 * time.Second,
		WriteTimeout: 58 * time.Second, IdleTimeout: 30 * time.Second, MaxHeaderBytes: 16384}
}

func (s *service) remote(w http.ResponseWriter, r *http.Request) {
	muxPath := strings.TrimPrefix(r.URL.Path, prefix)
	if muxPath == r.URL.Path {
		http.NotFound(w, r)
		return
	}
	p, err := s.who(r)
	if err != nil {
		reject(w, 401, "Tailscale authentication required")
		return
	}
	if !validDevice.MatchString(p.Device) {
		reject(w, 400, "Missing or invalid Kai device ID")
		return
	}
	known := s.state.snapshot()
	grant, paired := known.Trusted[p.Node]
	granted := paired && grant.Device == p.Device
	if r.Method == http.MethodGet && muxPath == "hello" {
		capabilities := []string{"workspace.history", "workspace.activity", "workspace.command", "workspace.screenshot", "workspace.files", "workspace.terminal", "workspace.chat-cli", "workspace.paired-access", "pairing.code", "pairing.control", "pairing.project-access", "workspace.composer", "workspace.long-prompts", "workspace.realtime"}
		capabilities = append(capabilities, "workspace-overview", inferenceCapability)
		capabilities = append(capabilities, "pairing.mutual")
		if s.app != "" {
			capabilities = append(capabilities, "workspace.canonical-history")
		}
		if s.changes != nil {
			capabilities = append(capabilities, "workspace.stream")
		}
		if s.updater != nil {
			capabilities = append(capabilities, "update.relaunch")
		}
		respond(w, 200, hello{Device: known.Device, Build: s.build, Protocols: []int{protocol},
			Capabilities: capabilities, Allowed: granted, Update: granted && grant.Update, Control: granted && grant.Control,
			FileProject: fileGrant(granted, grant), Terminal: granted && grant.Terminal,
			Platform: runtime.GOOS, FormFactor: hostFormFactor()})
		return
	}
	if r.Header.Get("Kai-Protocol") != strconv.Itoa(protocol) {
		reject(w, http.StatusUpgradeRequired, "Update Kai on one or both Macs. Their remote protocols do not overlap.")
		return
	}
	if r.Method == http.MethodPost && muxPath == "pair" {
		if granted {
			respond(w, 200, map[string]bool{"allowed": true})
			return
		}
		if err := s.requestPairing(p, time.Now()); err != nil {
			reject(w, 409, err.Error())
			return
		}
		respond(w, 202, map[string]bool{"allowed": false})
		return
	}
	if r.Method == http.MethodPost && muxPath == "pair-code" {
		s.pairByCode(w, r, p)
		return
	}
	if !granted {
		reject(w, 403, "Pair this Kai device on the host Mac to continue")
		return
	}
	switch {
	case strings.HasPrefix(muxPath, inferencePath):
		s.hostInference(w, r, grant, strings.TrimPrefix(muxPath, inferencePath))
	case r.Method == http.MethodPost && muxPath == "request-control":
		if !grant.Control {
			if err := s.requestAccess(p, "", false, time.Now()); err != nil {
				reject(w, 409, err.Error())
				return
			}
		}
		respond(w, 202, map[string]bool{"control": grant.Control})
	case r.Method == http.MethodPost && muxPath == "request-project":
		var access struct {
			Project  string `json:"project"`
			Terminal bool   `json:"terminal"`
		}
		if json.NewDecoder(http.MaxBytesReader(w, r.Body, 8192)).Decode(&access) != nil ||
			!s.history.allowsProject(r.Context(), access.Project) {
			reject(w, 400, "Choose a registered Kai project")
			return
		}
		if grant.FileProject != access.Project || (access.Terminal && !grant.Terminal) {
			if err := s.requestAccess(p, access.Project, access.Terminal, time.Now()); err != nil {
				reject(w, 409, err.Error())
				return
			}
		}
		respond(w, 202, map[string]bool{"pending": true})
	case r.Method == http.MethodGet && muxPath == "stream" && s.changes != nil:
		s.stream(w, r, p)
	case r.Method == http.MethodGet && muxPath == "workspace":
		offset, _ := strconv.Atoi(r.URL.Query().Get("offset"))
		if offset < 0 || offset > 100000 {
			reject(w, 400, "Invalid workspace cursor")
			return
		}
		result, err := s.history.browse(r.Context(), r.URL.Query().Get("project"), offset)
		if err != nil {
			reject(w, 503, "Kai could not read the workspace history")
			return
		}
		respond(w, 200, result)
	case r.Method == http.MethodGet && muxPath == "messages":
		id := r.URL.Query().Get("chat")
		if id == "" || len(id) > 256 {
			reject(w, 400, "Invalid chat ID")
			return
		}
		before, _ := strconv.ParseInt(r.URL.Query().Get("before"), 10, 64)
		if r.URL.Query().Get("canonical") == "1" {
			if !s.history.hasChat(r.Context(), id) {
				reject(w, 404, "Chat unavailable")
				return
			}
			raw, err := s.canonicalHistory(r.Context(), id, before)
			if err != nil {
				reject(w, 503, "Kai couldn’t read the original transcript on the host Mac")
				return
			}
			respond(w, 200, raw)
			return
		}
		result, err := s.history.messages(r.Context(), id, before)
		if errors.Is(err, sql.ErrNoRows) || errors.Is(err, os.ErrNotExist) {
			reject(w, 404, "Chat unavailable")
			return
		}
		if err != nil {
			reject(w, 503, "Kai could not read this chat")
			return
		}
		respond(w, 200, result)
	case r.Method == http.MethodGet && muxPath == "workspace-overview":
		var live []liveOutput
		probe, cancel := context.WithTimeout(r.Context(), 300*time.Millisecond)
		if raw, err := callLocal(probe, s.root, "bridge.sock", []byte(`{"operation":"overview_activity"}`)); err == nil {
			_ = json.Unmarshal(raw, &live)
		}
		cancel()
		result, err := s.history.overview(r.Context(), live)
		if err != nil {
			reject(w, 503, "Kai could not read the host's open tabs")
			return
		}
		respond(w, 200, result)
	case r.Method == http.MethodGet && muxPath == "activity":
		activity, err := s.history.activity(r.URL.Query().Get("chat"))
		if err != nil {
			reject(w, 503, "Kai could not read workspace activity")
			return
		}
		respond(w, 200, activity)
	case r.Method == http.MethodGet && muxPath == "run-controls" && grant.Control:
		s.runControls(w, r)
	case r.Method == http.MethodGet && muxPath == "artifacts":
		s.artifacts(w, r, grant)
	case muxPath == "terminals" || muxPath == "terminal":
		s.terminalEndpoint(w, r, grant, muxPath)
	case muxPath == "files" || muxPath == "file":
		s.files(w, r, grant, muxPath)
	case r.Method == http.MethodPost && muxPath == "command" && grant.Control:
		s.dispatchCommand(w, r, p)
	case r.Method == http.MethodPost && muxPath == "screenshot":
		request, err := s.screenshots.ask(p)
		if err != nil {
			reject(w, 409, err.Error())
			return
		}
		respond(w, 202, request)
	case r.Method == http.MethodGet && muxPath == "screenshot":
		s.screenshots.fetch(w, r.URL.Query().Get("id"), p)
	case r.Method == http.MethodPost && muxPath == "unpair":
		if err := s.revoke(p.Node, p.Device); err != nil {
			reject(w, 409, err.Error())
			return
		}
		respond(w, 200, map[string]bool{"ok": true})
	case muxPath == "update" && grant.Update && s.updater != nil:
		s.updater.route(w, r)
	case r.Method == http.MethodPost && muxPath == "update/check" && grant.Update && s.updater != nil:
		if err := s.updater.scheduleCheck(""); err != nil {
			reject(w, 409, err.Error())
			return
		}
		respond(w, 202, map[string]bool{"accepted": true})
	default:
		http.NotFound(w, r)
	}
}

func (s *service) requestPairing(p peer, now time.Time) error {
	known := s.state.snapshot()
	if req, exists := known.Requests[p.Node]; exists && req.Peer.Device == p.Device && req.Expires.After(now) {
		return nil
	}
	return s.state.change(func(i *identity) error {
		for node, req := range i.Requests {
			if now.After(req.Expires) {
				delete(i.Requests, node)
			}
		}
		if current, exists := i.Trusted[p.Node]; exists {
			if current.Device == p.Device {
				return nil
			}
			return errors.New("A different Kai identity is paired to this node. Revoke it on the host first.")
		}
		if len(i.Requests) >= 32 || len(i.Trusted) >= 64 {
			return errors.New("Kai’s pairing list is full")
		}
		i.Requests[p.Node] = pairingRequest{Peer: p, Expires: now.Add(5 * time.Minute)}
		return nil
	})
}

func (s *service) decide(node, device, decision string, update, control bool, now time.Time) error {
	return s.decideFiles(node, device, decision, update, control, false, "", now)
}

func (s *service) requestControl(p peer, now time.Time) error {
	return s.requestAccess(p, "", false, now)
}

func (s *service) requestAccess(p peer, project string, terminal bool, now time.Time) error {
	return s.state.change(func(i *identity) error {
		trusted, ok := i.Trusted[p.Node]
		if !ok || trusted.Device != p.Device {
			return errors.New("This Kai pairing changed")
		}
		if _, exists := i.Requests[p.Node]; !exists && len(i.Requests) >= 32 {
			return errors.New("Kai’s pairing list is full")
		}
		i.Requests[p.Node] = pairingRequest{
			Peer: p, Expires: now.Add(5 * time.Minute),
			ControlRequested: project == "", ProjectRequested: project, TerminalRequested: terminal,
		}
		return nil
	})
}

func fileGrant(granted bool, p peer) string {
	if granted {
		return p.FileProject
	}
	return ""
}

func (s *service) decideFiles(node, device, decision string, update, control, terminal bool, project string, now time.Time) error {
	return s.state.change(func(i *identity) error {
		request, ok := i.Requests[node]
		if !ok || request.Peer.Device != device || !now.Before(request.Expires) {
			return errors.New("Pairing request expired or changed. Ask the other Mac to connect again.")
		}
		if decision == "allow" {
			p := request.Peer
			if request.ControlRequested || request.ProjectRequested != "" {
				p, ok := i.Trusted[node]
				if !ok || p.Device != device {
					return errors.New("This Kai pairing changed")
				}
				if request.ProjectRequested != "" && (project != request.ProjectRequested || (terminal && !request.TerminalRequested)) {
					return errors.New("The project's permission request changed. Review the current request.")
				}
				if request.ProjectRequested == "" && project != "" {
					return errors.New("Ask the paired Mac to request project access")
				}
				p.Control = p.Control || control
				if project != "" {
					if p.FileProject != project {
						p.Terminal = false
					}
					p.FileProject = project
					p.Terminal = p.Terminal || (terminal && p.Control)
				}
				i.Trusted[node] = p
				delete(i.Requests, node)
				return nil
			}
			p.Allowed, p.Update, p.Control = true, update, control
			p.FileProject = project
			p.Terminal = terminal && control && project != ""
			i.Trusted[node] = p
		}
		delete(i.Requests, node)
		return nil
	})
}

func (s *service) revoke(node, device string) error {
	err := s.state.change(func(i *identity) error {
		p, ok := i.Trusted[node]
		if !ok || p.Device != device {
			return errors.New("The paired device changed")
		}
		delete(i.Trusted, node)
		delete(i.Requests, node)
		if outgoing, ok := i.Outgoing[node]; ok {
			outgoing.MutualUntil = nil
			i.Outgoing[node] = outgoing
		}
		return nil
	})
	if err == nil && s.changes != nil {
		s.changes.notify()
	}
	return err
}

type tailPeer struct {
	peer
	address   string
	transport *http.Client
}

func (s *service) tailPeers(ctx context.Context) ([]tailPeer, error) {
	identity := s.state.snapshot()
	if !identity.Enabled {
		return nil, nil
	}
	lc, _ := s.tailscale()
	var embedded *ipnstate.Status
	var embeddedErr error
	if lc != nil {
		probe, cancel := context.WithTimeout(ctx, 3*time.Second)
		status, err := lc.Status(probe)
		cancel()
		embeddedErr = err
		if err == nil && status.BackendState == "Running" && status.Self != nil && status.Self.ID != "" {
			if err := s.recordNode(string(status.Self.ID)); err != nil {
				return nil, err
			}
			embedded = status
			if status.Self.Online {
				return kaiPeersOnTailnet(status), nil
			}
		}
	}
	if identity.SystemTailnet && runtime.GOOS == "darwin" {
		if system, err := s.systemTailnet.peers(ctx); err == nil {
			return system, nil
		}
	}
	if embedded != nil {
		return kaiPeersOnTailnet(embedded), nil
	}
	return nil, embeddedErr
}

func kaiPeersOnTailnet(st *ipnstate.Status) []tailPeer {
	result := []tailPeer{}
	for _, p := range st.Peer {
		if p.ID == "" || len(p.TailscaleIPs) == 0 {
			continue
		}
		suffix := strings.HasSuffix(p.HostName, "-kai")
		if !suffix {
			continue
		}
		name := strings.Split(strings.TrimSuffix(p.DNSName, "."), ".")[0]
		if name == "" {
			name = p.HostName
		}
		result = append(result, tailPeer{peer: peer{Node: string(p.ID), Name: name, Online: p.Online, Platform: p.OS},
			address: net.JoinHostPort(p.TailscaleIPs[0].String(), port)})
	}
	slices.SortFunc(result, func(a, b tailPeer) int {
		if a.Online != b.Online {
			if a.Online {
				return -1
			}
			return 1
		}
		return strings.Compare(a.Name, b.Name)
	})
	return result
}

func (s *service) peerClient(p tailPeer) *http.Client {
	if p.transport != nil {
		identity := s.state.snapshot()
		if identity.Enabled && identity.SystemTailnet {
			return p.transport
		}
		return nil
	}
	_, client := s.tailscale()
	return client
}

func (s *service) send(ctx context.Context, p tailPeer, method, endpoint string, out any) error {
	return s.sendRequest(ctx, p, method, endpoint, nil, out)
}

func (s *service) sendRequest(ctx context.Context, p tailPeer, method, endpoint string, body io.Reader, out any) error {
	client := s.peerClient(p)
	if client == nil {
		return errTailnetOffline
	}
	request, err := http.NewRequestWithContext(ctx, method, "http://"+p.address+prefix+endpoint, body)
	if err != nil {
		return err
	}
	request.Header.Set("Kai-Device", s.state.snapshot().Device)
	request.Header.Set("Kai-Protocol", strconv.Itoa(protocol))
	response, err := client.Do(request)
	if err != nil {
		return errors.New("Could not reach this Kai server. Check that the Mac is awake and tailnet access is allowed.")
	}
	defer response.Body.Close()
	if p.transport == nil {
		s.notePeerTraffic()
	}
	decoder := json.NewDecoder(io.LimitReader(response.Body, maxResponse))
	if response.StatusCode >= 300 {
		var issue struct {
			Error string `json:"error"`
		}
		if decoder.Decode(&issue) == nil && issue.Error != "" {
			return errors.New(issue.Error)
		}
		return fmt.Errorf("Kai peer returned HTTP %d", response.StatusCode)
	}
	return decoder.Decode(out)
}

func (s *service) probe(ctx context.Context, p tailPeer) (peer, error) {
	var h hello
	if err := s.send(ctx, p, http.MethodGet, "hello", &h); err != nil {
		return peer{}, err
	}
	if !validDevice.MatchString(h.Device) {
		return peer{}, errors.New("Invalid Kai peer identity")
	}
	p.Device, p.Build, p.Compatible, p.Online, p.Allowed, p.Update, p.Control =
		h.Device, h.Build, slices.Contains(h.Protocols, protocol) && h.Build >= minimumBuild, true, h.Allowed, h.Update, h.Control
	p.Capabilities = h.Capabilities
	p.FileProject = h.FileProject
	p.Terminal = h.Terminal
	if h.Platform != "" {
		p.Platform = h.Platform
	}
	p.FormFactor = h.FormFactor
	return p.peer, nil
}

func (s *service) discover(ctx context.Context) ([]peer, error) {
	nodes, err := s.tailPeers(ctx)
	if err != nil {
		return nil, err
	}
	known := s.state.snapshot()
	result := make([]peer, len(nodes))
	var wg sync.WaitGroup
	slots := make(chan struct{}, 12)
	for index, p := range nodes {
		if saved, ok := known.Outgoing[p.Node]; ok {
			saved.Online, saved.Allowed = false, false
			result[index] = saved
		}
		if index >= 96 {
			continue
		}
		wg.Go(func() {
			select {
			case slots <- struct{}{}:
			case <-ctx.Done():
				return
			}
			defer func() { <-slots }()
			probeCtx, cancel := context.WithTimeout(ctx, 2*time.Second)
			defer cancel()
			if v, err := s.probe(probeCtx, p); err == nil {
				result[index] = v
			}
		})
	}
	wg.Wait()
	if err := s.finishMutualPairings(result, time.Now()); err != nil {
		return nil, err
	}
	for i := len(result) - 1; i >= 0; i-- {
		if result[i].Device == "" {
			result = slices.Delete(result, i, i+1)
		}
	}
	for node, p := range known.Outgoing {
		if !slices.ContainsFunc(result, func(v peer) bool { return v.Node == node }) {
			p.Online, p.Allowed = false, false
			result = append(result, p)
		}
	}
	return result, nil
}

func (s *service) finishMutualPairings(observed []peer, now time.Time) error {
	known := s.state.snapshot()
	if !slices.ContainsFunc(observed, func(p peer) bool {
		intent := known.Outgoing[p.Node]
		return p.Online && p.Allowed && intent.Device == p.Device && intent.MutualUntil != nil && now.Before(*intent.MutualUntil)
	}) {
		return nil
	}
	return s.state.change(func(i *identity) error {
		for _, p := range observed {
			intent := i.Outgoing[p.Node]
			if !p.Online || !p.Allowed || intent.Device != p.Device || intent.MutualUntil == nil || !now.Before(*intent.MutualUntil) {
				continue
			}
			if _, exists := i.Trusted[p.Node]; !exists {
				i.Trusted[p.Node] = peer{Node: p.Node, Device: p.Device, Name: p.Name, Online: true, Allowed: true, Control: p.Control}
				delete(i.Requests, p.Node)
			}
			intent.MutualUntil = nil
			i.Outgoing[p.Node] = intent
		}
		return nil
	})
}

func (s *service) resolvePeer(ctx context.Context, node, device string) (tailPeer, error) {
	peers, err := s.tailPeers(ctx)
	if err != nil {
		return tailPeer{}, err
	}
	i := slices.IndexFunc(peers, func(p tailPeer) bool { return p.Node == node })
	if i < 0 {
		return tailPeer{}, errors.New("Kai device is offline")
	}
	target := peers[i]
	remote, err := s.probe(ctx, target)
	if err != nil {
		return tailPeer{}, err
	}
	if remote.Device != device {
		return tailPeer{}, errors.New("The Kai identity on this node changed. Review it before connecting.")
	}
	if !remote.Compatible {
		return tailPeer{}, errors.New("Update Kai on one or both Macs. Their remote protocols do not overlap.")
	}
	target.peer = remote
	return target, nil
}

func (s *service) remoteCall(ctx context.Context, req localRequest) (any, error) {
	node, device, action, project, chat, cursor := req.Node, req.Device, req.Action, req.Project, req.Chat, req.Cursor
	target, err := s.resolvePeer(ctx, node, device)
	if err != nil {
		return nil, err
	}
	remote := target.peer
	method, endpoint := http.MethodGet, ""
	var body io.Reader
	switch action {
	case "pair":
		method, endpoint = http.MethodPost, "pair"
	case "request-control":
		if !slices.Contains(remote.Capabilities, "pairing.control") {
			return nil, errors.New("Update Kai on the host Mac to request agent control without pairing again")
		}
		method, endpoint = http.MethodPost, "request-control"
	case "request-project":
		if !slices.Contains(remote.Capabilities, "pairing.project-access") {
			return nil, errors.New("Update Kai on the host Mac to share a project with this device")
		}
		method, endpoint = http.MethodPost, "request-project"
		encoded, _ := json.Marshal(map[string]any{"project": project, "terminal": req.Terminal})
		body = strings.NewReader(string(encoded))
	case "redeem-code":
		if !slices.Contains(remote.Capabilities, "pairing.code") {
			return nil, errors.New("Update the host Kai app to use pairing codes")
		}
		method, endpoint = http.MethodPost, "pair-code"
		code, _ := json.Marshal(map[string]string{"code": req.Code})
		body = strings.NewReader(string(code))
	case "disconnect":
		method, endpoint = http.MethodPost, "unpair"
	case "workspace":
		endpoint = "workspace?" + url.Values{"project": {project}, "offset": {cursor}}.Encode()
	case "workspace-overview":
		endpoint = "workspace-overview"
	case "messages":
		values := url.Values{"chat": {chat}, "before": {cursor}}
		if req.Canonical && slices.Contains(remote.Capabilities, "workspace.canonical-history") {
			values.Set("canonical", "1")
		}
		endpoint = "messages?" + values.Encode()
	case "activity":
		endpoint = "activity?" + url.Values{"chat": {chat}}.Encode()
	case "command":
		method, endpoint = http.MethodPost, "command"
		body = strings.NewReader(string(req.Command))
	case "artifacts":
		endpoint = "artifacts?" + url.Values{"project": {project}, "id": {req.File}, "offset": {cursor}}.Encode()
	case "run-controls":
		endpoint = "run-controls?" + url.Values{"chat": {chat}}.Encode()
	case "terminals":
		endpoint = "terminals?" + url.Values{"active": {"true"}, "project": {project}}.Encode()
	case "terminal-start":
		method, endpoint = http.MethodPost, "terminals?"+url.Values{"project": {project}}.Encode()
		body = strings.NewReader(string(req.Command))
	case "request-screenshot":
		if !slices.Contains(remote.Capabilities, "workspace.screenshot") {
			return nil, errors.New("Update the host Kai app to request screenshots")
		}
		method, endpoint = http.MethodPost, "screenshot"
	case "remote-check-update":
		method, endpoint = http.MethodPost, "update/check"
	case "remote-update-status":
		endpoint = "update"
	case "remote-update":
		method, endpoint = http.MethodPost, "update"
	default:
		return nil, errors.New("Unknown Kai peer action")
	}
	var out any
	if err := s.sendRequest(ctx, target, method, endpoint, body, &out); err != nil {
		return nil, err
	}
	if action == "pair" || action == "redeem-code" || action == "disconnect" {
		err = s.state.change(func(i *identity) error {
			if action == "disconnect" {
				delete(i.Outgoing, node)
			} else {
				if action == "pair" && slices.Contains(remote.Capabilities, "pairing.mutual") {
					until := time.Now().Add(5 * time.Minute)
					remote.MutualUntil = &until
				}
				i.Outgoing[node] = remote
			}
			return nil
		})
	}
	return out, err
}
