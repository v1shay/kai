package main

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/coder/websocket"
	"github.com/coder/websocket/wsjson"
)

func TestProjectFilesGrantBoundariesAndRevisionCheckedEdit(t *testing.T) {
	root := t.TempDir()
	project, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	if err := saveJSON(filepath.Join(root, "projects.json"), projectRegistry{Custom: []string{project}}); err != nil {
		t.Fatal(err)
	}
	s, err := newService(t.Context(), root, root, 192)
	if err != nil {
		t.Fatal(err)
	}
	name := filepath.Join(project, "readme.txt")
	if err := os.WriteFile(name, []byte("before"), 0600); err != nil {
		t.Fatal(err)
	}
	outside := filepath.Join(t.TempDir(), "outside")
	if err := os.Mkdir(outside, 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(outside, "secret"), []byte("private"), 0600); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(outside, filepath.Join(project, "escape")); err != nil {
		t.Fatal(err)
	}
	call := func(method, endpoint, path, body, revision string, grant peer) *httptest.ResponseRecorder {
		t.Helper()
		r := httptest.NewRequest(method, "http://kai.local/"+endpoint, strings.NewReader(body))
		q := r.URL.Query()
		q.Set("project", project)
		q.Set("path", path)
		r.URL.RawQuery = q.Encode()
		if revision != "" {
			r.Header.Set("If-Match", revision)
		}
		w := httptest.NewRecorder()
		s.files(w, r, grant, endpoint)
		return w
	}
	read := peer{FileProject: project}
	write := peer{Control: true}
	want := func(w *httptest.ResponseRecorder, code int) {
		t.Helper()
		if w.Code != code {
			t.Fatalf("HTTP %d, wanted %d: %s", w.Code, code, w.Body.String())
		}
	}
	want(call("GET", "files", ".", "", "", peer{}), 403)
	w := call("GET", "files", ".", "", "", read)
	want(w, 200)
	if !strings.Contains(w.Body.String(), "readme.txt") || strings.Contains(w.Body.String(), "escape") {
		t.Fatalf("wrong directory listing %s", w.Body.String())
	}
	want(call("GET", "file", "../outside/secret", "", "", read), 400)
	want(call("GET", "file", "escape/secret", "", "", read), 404)
	w = call("GET", "file", "readme.txt", "", "", read)
	want(w, 200)
	revision := w.Header().Get("ETag")
	if !validETag(revision) {
		t.Fatal(revision)
	}
	want(call("PUT", "file", "readme.txt", "denied", revision, read), 404)
	want(call("PUT", "file", "readme.txt", "after", revision, write), 200)
	want(call("PUT", "file", "readme.txt", "stale", revision, write), 412)
	b, err := os.ReadFile(name)
	if err != nil || string(b) != "after" {
		t.Fatalf("file changed incorrectly: %q %v", b, err)
	}
}

type fixtureTransport func(*http.Request) (*http.Response, error)

func (f fixtureTransport) RoundTrip(r *http.Request) (*http.Response, error) { return f(r) }

func TestInterruptedDownloadResumesAndVerifiesFinalHash(t *testing.T) {
	tail := tailPeer{peer: peer{Node: "test", Device: "11111111111111111111111111111111"}, address: "127.0.0.1"}
	payload := "the file starts here, and these bytes complete it"
	etag := fmt.Sprintf("\"%x\"", sha256.Sum256([]byte(payload)))
	calls := 0
	root := t.TempDir()
	s, err := newService(t.Context(), root, root, 192)
	if err != nil {
		t.Fatal(err)
	}
	s.httpClient = &http.Client{Transport: fixtureTransport(func(r *http.Request) (*http.Response, error) {
		calls++
		h := make(http.Header)
		h.Set("ETag", etag)
		if calls == 1 {
			return &http.Response{StatusCode: 200, Header: h, ContentLength: int64(len(payload)), Body: io.NopCloser(strings.NewReader(payload[:10]))}, nil
		}
		if r.Header.Get("Range") != "bytes=10-" || r.Header.Get("If-Range") != etag {
			t.Fatalf("resume headers: %v", r.Header)
		}
		h.Set("Content-Range", fmt.Sprintf("bytes 10-%d/%d", len(payload)-1, len(payload)))
		return &http.Response{StatusCode: 206, Header: h, ContentLength: int64(len(payload) - 10), Body: io.NopCloser(strings.NewReader(payload[10:]))}, nil
	})}
	req := localRequest{Project: "/fixture", File: "file", Path: filepath.Join(root, "complete")}
	if _, err := s.downloadFile(t.Context(), tail, req); err == nil {
		t.Fatal("accepted interrupted transfer")
	}
	if _, err := os.Stat(req.Path); !os.IsNotExist(err) {
		t.Fatal("partial bytes published at final destination")
	}
	if _, err := s.downloadFile(t.Context(), tail, req); err != nil {
		t.Fatal(err)
	}
	b, err := os.ReadFile(req.Path)
	if err != nil || string(b) != payload {
		t.Fatalf("final file: %q %v", b, err)
	}
}

func TestGitHubDownloadRejectsCorruptAndTruncatedAssets(t *testing.T) {
	wanted := "correct fixture payload"
	asset := releaseAsset{URL: "https://github.com/v1shay/kai/releases/download/test/kai.zip", Size: int64(len(wanted))}
	hash := fmt.Sprintf("%x", sha256.Sum256([]byte(wanted)))
	for _, test := range []struct {
		body  string
		valid bool
	}{
		{wanted, true}, {wanted[:len(wanted)-1], false}, {strings.Repeat("x", len(wanted)), false},
	} {
		client := &http.Client{Transport: fixtureTransport(func(r *http.Request) (*http.Response, error) {
			return &http.Response{StatusCode: 200, Body: io.NopCloser(strings.NewReader(test.body))}, nil
		})}
		err := downloadRelease(t.Context(), client, asset, filepath.Join(t.TempDir(), "download"), hash)
		if (err == nil) != test.valid {
			t.Fatalf("asset %q valid=%t err=%v", test.body, test.valid, err)
		}
	}
}

func TestWebSocketPushesAndClosesUponRevocation(t *testing.T) {
	root := t.TempDir()
	s, err := newService(t.Context(), root, root, 192)
	if err != nil {
		t.Fatal(err)
	}
	changes, err := newFileChanges(t.Context())
	if err != nil {
		t.Fatal(err)
	}
	s.changes = changes
	s.streamSlots = make(chan struct{}, 1)
	stop := changes.watch(root)
	defer stop()
	remote := peer{Node: "fixture-node", Device: "11111111111111111111111111111111"}
	if err := s.state.change(func(i *identity) error { i.Trusted[remote.Node] = remote; return nil }); err != nil {
		t.Fatal(err)
	}
	s.who = func(r *http.Request) (peer, error) { return remote, nil }
	ctx, cancel := context.WithTimeout(t.Context(), 8*time.Second)
	defer cancel()
	httpServer := httptest.NewServer(http.HandlerFunc(s.remote))
	defer httpServer.Close()
	headers := http.Header{"Kai-Device": {remote.Device}, "Kai-Protocol": {"1"}}
	ws, _, err := websocket.Dial(ctx, "ws"+strings.TrimPrefix(httpServer.URL, "http")+prefix+"stream", &websocket.DialOptions{HTTPHeader: headers})
	if err != nil {
		t.Fatal(err)
	}
	defer ws.CloseNow()
	if err := wsjson.Write(ctx, ws, streamSubscribe{}); err != nil {
		t.Fatal(err)
	}
	read := func(kind string) streamFrame {
		t.Helper()
		for {
			var f streamFrame
			if err := wsjson.Read(ctx, ws, &f); err != nil {
				t.Fatal(err)
			}
			if f.Type == kind {
				return f
			}
		}
	}
	first := read("metadata")
	if first.Workspace == nil || len(first.Workspace.Projects) != 0 {
		t.Fatalf("unexpected first snapshot %+v", first)
	}
	if err := saveJSON(filepath.Join(root, "projects.json"), projectRegistry{Custom: []string{"/generated/project"}}); err != nil {
		t.Fatal(err)
	}
	next := read("metadata_delta")
	if next.Delta == nil || len(next.Delta.Projects) != 1 {
		t.Fatalf("updated snapshot %+v", next)
	}
	if err := s.revoke(remote.Node, remote.Device); err != nil {
		t.Fatal(err)
	}
	_, _, err = ws.Read(ctx)
	if websocket.CloseStatus(err) != websocket.StatusPolicyViolation {
		t.Fatalf("revoked stream stayed open: %v", err)
	}
}

func TestJournalCursorDoesNotAdvancePastPartialEvents(t *testing.T) {
	root := t.TempDir()
	s, err := newService(t.Context(), root, root, 192)
	if err != nil {
		t.Fatal(err)
	}
	owner := filepath.Join(root, "session-owners", "33333333333333333333333333333333")
	if err := os.MkdirAll(owner, 0700); err != nil {
		t.Fatal(err)
	}
	journal := filepath.Join(owner, "events.jsonl")
	if err := os.WriteFile(journal, []byte("{\"Text\":\"first\"}\n{\"Text\":\"sec"), 0600); err != nil {
		t.Fatal(err)
	}
	var got []string
	req := streamSubscribe{Chat: "test"}
	write := func(f streamFrame) error {
		for _, ev := range f.Events {
			var event map[string]string
			_ = json.Unmarshal(ev, &event)
			got = append(got, event["Text"])
		}
		return nil
	}
	if _, err := s.pushJournal(t.Context(), owner, &req, write); err != nil {
		t.Fatal(err)
	}
	if len(got) != 1 || got[0] != "first" {
		t.Fatalf("initial events %v", got)
	}
	f, err := os.OpenFile(journal, os.O_APPEND|os.O_WRONLY, 0600)
	if err != nil {
		t.Fatal(err)
	}
	_, err = f.WriteString("ond\"}\n")
	f.Close()
	if err != nil {
		t.Fatal(err)
	}
	if _, err := s.pushJournal(t.Context(), owner, &req, write); err != nil {
		t.Fatal(err)
	}
	if len(got) != 2 || got[1] != "second" {
		t.Fatalf("resume duplicated or skipped events: %v", got)
	}
}

func TestPairingCodeIsSingleUseAndReadOnly(t *testing.T) {
	root := t.TempDir()
	s, err := newService(t.Context(), root, root, 194)
	if err != nil {
		t.Fatal(err)
	}
	if err := s.state.change(func(i *identity) error { i.Enabled = true; i.Node = "host-node"; return nil }); err != nil {
		t.Fatal(err)
	}
	first, err := s.createCode()
	if err != nil {
		t.Fatal(err)
	}
	onDisk, err := os.ReadFile(s.state.filename)
	if err != nil || strings.Contains(string(onDisk), first.Code) {
		t.Fatal("pairing code was stored in plaintext")
	}
	p := peer{Node: "identity-from-whois", Device: "22222222222222222222222222222222"}
	if err := s.consumeCode(p, first.Code, time.Now().Add(6*time.Minute)); err == nil {
		t.Fatal("expired code accepted")
	}
	if err := s.consumeCode(p, first.Code, time.Now()); err != nil {
		t.Fatal(err)
	}
	if err := s.consumeCode(peer{Node: "other", Device: p.Device}, first.Code, time.Now()); err == nil {
		t.Fatal("pairing code replay accepted")
	}
	grant := s.state.snapshot().Trusted[p.Node]
	if grant.Device != p.Device || !grant.Allowed || grant.Update || grant.Control || grant.Terminal || grant.FileProject != "" {
		t.Fatalf("wrong grant after code redemption: %+v", grant)
	}
}

func TestUploadNeverOverwritesExistingProjectFile(t *testing.T) {
	rootPath := t.TempDir()
	root, err := os.OpenRoot(rootPath)
	if err != nil {
		t.Fatal(err)
	}
	defer root.Close()
	s := &service{}
	do := func(body string) *httptest.ResponseRecorder {
		r := httptest.NewRequest(http.MethodPut, "http://kai.local/file", strings.NewReader(body))
		out := httptest.NewRecorder()
		s.uploadFile(out, r, root, "new.bin")
		return out
	}
	if w := do("original"); w.Code != 200 {
		t.Fatalf("initial upload: %d %s", w.Code, w.Body.String())
	}
	if w := do("replacement"); w.Code != 409 {
		t.Fatalf("accepted overwrite: %d %s", w.Code, w.Body.String())
	}
	if b, err := os.ReadFile(filepath.Join(rootPath, "new.bin")); err != nil || string(b) != "original" {
		t.Fatalf("unexpected uploaded content %q %v", b, err)
	}
	files, err := findProjectFiles(t.Context(), root, ".", "NEW")
	if err != nil || len(files) != 1 || files[0].Name != "new.bin" {
		t.Fatalf("scoped discovery: %+v %v", files, err)
	}
}

func TestProjectApprovalPreservesPairingAndScopesTerminal(t *testing.T) {
	root := t.TempDir()
	s, err := newService(t.Context(), root, root, 206)
	if err != nil {
		t.Fatal(err)
	}
	p := peer{Node: "test-node", Device: "test-device"}
	if err := s.state.change(func(i *identity) error {
		i.Trusted[p.Node] = peer{Node: p.Node, Device: p.Device, Allowed: true, Update: true}
		return nil
	}); err != nil {
		t.Fatal(err)
	}
	project := filepath.Join(root, "project")
	now := time.Now()
	if err := s.requestAccess(p, project, true, now); err != nil {
		t.Fatal(err)
	}
	if s.state.terminalAllowed(p.Node, p.Device, project) {
		t.Fatal("request must await host approval")
	}
	if err := s.decideFiles(p.Node, p.Device, "allow", false, true, true, root, now); err == nil {
		t.Fatal("approval accepted a different project")
	}
	if err := s.decideFiles(p.Node, p.Device, "allow", false, true, true, project, now); err != nil {
		t.Fatal(err)
	}
	got := s.state.snapshot().Trusted[p.Node]
	if !got.Allowed || !got.Update || !s.state.terminalAllowed(p.Node, p.Device, project) {
		t.Fatalf("approval did not retain the existing pairing: %+v", got)
	}
	if err := s.requestAccess(p, root, false, now); err != nil {
		t.Fatal(err)
	}
	if err := s.decideFiles(p.Node, p.Device, "allow", false, false, false, root, now); err != nil {
		t.Fatal(err)
	}
	if err := s.revoke(p.Node, p.Device); err != nil {
		t.Fatal(err)
	}
	if s.state.terminalAllowed(p.Node, p.Device, project) || s.state.fileAllowed(p.Node, p.Device, project) {
		t.Fatal("revoked pairing still authorizes project access")
	}
}

func TestTerminalCapacityCountsOwnersAndStartsWithoutDiscardingArchives(t *testing.T) {
	root := t.TempDir()
	now := time.Now()
	record := func(i int, age time.Duration) string {
		t.Helper()
		id := fmt.Sprintf("%032x", i)
		dir := filepath.Join(root, "terminals", id)
		if err := os.MkdirAll(dir, 0700); err != nil {
			t.Fatal(err)
		}
		if err := saveJSON(filepath.Join(dir, "terminal.json"), terminalSession{ID: id, Started: now.Add(-age)}); err != nil {
			t.Fatal(err)
		}
		return dir
	}
	for i := 0; i < maxLiveTerminals+1; i++ {
		dir := record(i, time.Hour)
		if err := os.WriteFile(filepath.Join(dir, "scrollback"), []byte("preserved"), 0600); err != nil {
			t.Fatal(err)
		}
	}
	record(maxLiveTerminals+1, time.Second)
	record(maxLiveTerminals+2, time.Hour)
	active := record(maxLiveTerminals+3, time.Hour)
	owner, err := lockFile(filepath.Join(active, "owner.lock"))
	if err != nil {
		t.Fatal(err)
	}
	defer owner.Close()
	if got := liveTerminalCount(root, now); got != 2 {
		t.Fatalf("expected one starting and one running terminal, got %d", got)
	}
	archives, err := os.ReadDir(filepath.Join(root, "terminals"))
	if err != nil || len(archives) != maxLiveTerminals+4 {
		t.Fatalf("terminal records were changed: %d, %v", len(archives), err)
	}
}

func TestPTYStreamsInputOutputAndResizeAfterReconnect(t *testing.T) {
	root, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(t.Context(), 12*time.Second)
	defer cancel()
	s, err := newService(ctx, root, root, 206)
	if err != nil {
		t.Fatal(err)
	}
	s.ptySlots = make(chan struct{}, 2)
	p := peer{Node: "test-node", Device: "test-device", Allowed: true, Control: true, Terminal: true, FileProject: root}
	if err := s.state.change(func(i *identity) error {
		i.Trusted[p.Node] = p
		return nil
	}); err != nil {
		t.Fatal(err)
	}
	id := "0123456789abcdef0123456789abcdef"
	dir := filepath.Join(root, "terminals", id)
	if err := os.MkdirAll(dir, 0700); err != nil {
		t.Fatal(err)
	}
	if err := saveJSON(filepath.Join(dir, "terminal.json"), terminalSession{ID: id, Project: root, Program: "shell"}); err != nil {
		t.Fatal(err)
	}
	owner := make(chan error, 1)
	go func() { owner <- serveTerminal(ctx, root, id) }()
	t.Cleanup(func() {
		cancel()
		if err := <-owner; err != nil {
			t.Errorf("terminal owner: %v", err)
		}
	})
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		s.serveTerminalStream(w, r, p, id)
	}))
	defer server.Close()
	dial := func() *websocket.Conn {
		conn, _, err := websocket.Dial(ctx, "ws"+strings.TrimPrefix(server.URL, "http"), nil)
		if err != nil {
			t.Fatal(err)
		}
		return conn
	}
	waitOutput := func(conn *websocket.Conn, marker string) {
		var output strings.Builder
		for !strings.Contains(output.String(), marker) {
			kind, bytes, err := conn.Read(ctx)
			if err != nil {
				t.Fatalf("waiting for %q: %v", marker, err)
			}
			if kind == websocket.MessageBinary {
				output.Write(bytes)
			}
		}
	}
	first := dial()
	defer first.CloseNow()
	if err := wsjson.Write(ctx, first, terminalIO{Op: "resize", Rows: 39, Cols: 121}); err != nil {
		t.Fatal(err)
	}
	if err := first.Write(ctx, websocket.MessageBinary, []byte("printf 'KAI_%s\\n' \"$(stty size)\"\n")); err != nil {
		t.Fatal(err)
	}
	waitOutput(first, "KAI_39 121")
	if sessions := listTerminals(root, root, true); len(sessions) != 1 || sessions[0].ID != id {
		t.Fatalf("expected one attachable terminal, got %+v", sessions)
	}
	_ = first.CloseNow()
	second := dial()
	defer second.CloseNow()
	waitOutput(second, "KAI_39 121")
	if err := second.Write(ctx, websocket.MessageBinary, []byte("printf 'KAI_reattached_%s\\n' ok\n")); err != nil {
		t.Fatal(err)
	}
	waitOutput(second, "KAI_reattached_ok")
}

func TestRemoteTerminalRejoinsHostTmuxSession(t *testing.T) {
	path, err := ownerProgram("tmux")
	if err != nil {
		t.Skip("tmux is required")
	}
	root, err := filepath.EvalSymlinks(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	digest := sha256.Sum256([]byte(root))
	socket := fmt.Sprintf("kai-%x", digest[:12])
	id := "0123456789abcdef0123456789abcdef"
	cmd := func(args ...string) *exec.Cmd {
		return exec.Command(path, append([]string{"-L", socket, "-f", "/dev/null"}, args...)...)
	}
	if out, err := cmd("new-session", "-d", "-s", id, "-c", root, "printf 'KAI_SAVED_SCROLLBACK\\n'; exec /bin/zsh -f").CombinedOutput(); err != nil {
		t.Fatalf("tmux fixture: %v %s", err, out)
	}
	t.Cleanup(func() { _ = cmd("kill-server").Run() })
	dir := filepath.Join(root, "terminals", id)
	if err := os.MkdirAll(dir, 0700); err != nil {
		t.Fatal(err)
	}
	if err := saveJSON(filepath.Join(dir, "terminal.json"), terminalSession{
		ID: id, Project: root, Program: "tmux", Resume: id, ResumeSocket: socket,
	}); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithTimeout(t.Context(), 8*time.Second)
	owner := make(chan error, 1)
	go func() { owner <- serveTerminal(ctx, root, id) }()
	defer func() { cancel(); <-owner }()
	for ctx.Err() == nil {
		data, err := callLocal(ctx, root, terminalSocket(id), []byte(`{"op":"read","after":0}`))
		var output terminalOutput
		if err == nil && json.Unmarshal(data, &output) == nil && strings.Contains(string(output.Output), "KAI_SAVED_SCROLLBACK") {
			return
		}
		time.Sleep(50 * time.Millisecond)
	}
	t.Fatal("saved scrollback did not reach the remote PTY")
}

func TestAuthenticatedComposerAndRealtimeReachNativeBridge(t *testing.T) {
	root := t.TempDir()
	s, err := newService(t.Context(), root, root, 208)
	if err != nil {
		t.Fatal(err)
	}
	p := peer{Node: "client-node", Device: "client-device", Allowed: true, Control: true}
	if err := s.state.change(func(i *identity) error { i.Trusted[p.Node] = p; return nil }); err != nil {
		t.Fatal(err)
	}
	socket, err := listenLocal(root, "bridge.sock")
	if err != nil {
		t.Fatal(err)
	}
	expectedClient := fmt.Sprintf("%x", sha256.Sum256([]byte(p.Node+"\x00"+p.Device)))
	prompt := strings.Repeat("🦀", 15<<10)
	offer := "v=0\r\n" + strings.Repeat("a=candidate:fixture\r\n", 2000)
	server := &http.Server{Handler: http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var body struct {
			Operation string `json:"operation"`
			Client    string `json:"client"`
			Command   struct {
				Action string `json:"action"`
				SDP    string `json:"sdp"`
				Prompt string `json:"prompt"`
			} `json:"command"`
		}
		if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
			reject(w, 400, "Invalid fixture body")
			return
		}
		if body.Operation == "health" {
			respond(w, 200, map[string]bool{"ready": true})
			return
		}
		if body.Client != expectedClient {
			reject(w, 403, "Wrong principal")
			return
		}
		if body.Command.Action == "voice_start" && body.Command.SDP != offer {
			reject(w, 400, "Offer changed in transit")
			return
		}
		if body.Command.Action == "prompt_chat" && body.Command.Prompt != prompt {
			reject(w, 400, "Prompt changed in transit")
			return
		}
		respond(w, 200, map[string]string{"action": body.Command.Action})
	})}
	go func() { _ = server.Serve(socket) }()
	t.Cleanup(func() { _ = server.Close() })
	call := func(principal peer, action, sdp string) *httptest.ResponseRecorder {
		b, _ := json.Marshal(map[string]string{"action": action, "sdp": sdp})
		r := httptest.NewRequest("POST", "/command", strings.NewReader(string(b)))
		w := httptest.NewRecorder()
		s.dispatchCommand(w, r, principal)
		return w
	}
	for _, action := range []string{"list_models", "composer_options", "voice_start", "voice_status", "voice_interrupt", "voice_end"} {
		sdp := ""
		if action == "voice_start" {
			sdp = offer
		}
		w := call(p, action, sdp)
		if w.Code != 200 || !strings.Contains(w.Body.String(), action) {
			t.Fatalf("%s did not reach the host: %d %s", action, w.Code, w.Body.String())
		}
	}
	if code := call(p, "prompt_chat", offer).Code; code != 400 {
		t.Fatalf("An ordinary command accepted an oversized voice payload: %d", code)
	}
	body, err := json.Marshal(map[string]string{"action": "prompt_chat", "prompt": prompt})
	if err != nil {
		t.Fatal(err)
	}
	w := httptest.NewRecorder()
	s.dispatchCommand(w, httptest.NewRequest("POST", "/command", bytes.NewReader(body)), p)
	if w.Code != 200 {
		t.Fatalf("A near-limit UTF-8 prompt did not reach the native host: %d %s", w.Code, w.Body.String())
	}
	if code := call(peer{Node: p.Node, Device: "other-device"}, "voice_status", "").Code; code != 403 {
		t.Fatalf("A different device could request signaling: %d", code)
	}
}

func TestOnlyUnattemptedRemotePromptsWakeTheWorkspace(t *testing.T) {
	for _, tc := range []struct {
		teams string
		want  bool
	}{
		{`[{"remote_prompt":true,"members":[{"status":"Queued"}]}]`, true},
		{`[{"members":[{"status":"Queued"}]}]`, false},
		{`[{"remote_prompt":true,"members":[{"status":"Queued","attempted":true}]}]`, false},
		{`[{"remote_prompt":true,"members":[{"status":"Working","attempted":true}]}]`, false},
		{`[{"remote_prompt":true,"members":[{"status":"Held"}]}]`, false},
		{`[]`, false},
	} {
		if got := readWorkspaceWork(strings.NewReader(`{"teams":` + tc.teams + `}`)).needsWorkspace(""); got != tc.want {
			t.Errorf("teams %s: got %t, want %t", tc.teams, got, tc.want)
		}
	}
}

func TestDurableCompletionAndDelegationWakeTheWorkspace(t *testing.T) {
	owners := t.TempDir()
	dir := filepath.Join(owners, "5266dcc5-479d-4efe-bbc7-fdd5054dbc1b")
	if err := os.Mkdir(dir, 0700); err != nil {
		t.Fatal(err)
	}
	for _, tc := range []struct{ done, orchestrator, want bool }{
		{false, false, false}, {true, false, true}, {false, true, true},
	} {
		snapshot, _ := json.Marshal(map[string]bool{"done": tc.done})
		if err := os.WriteFile(filepath.Join(dir, "snapshot.json"), snapshot, 0600); err != nil {
			t.Fatal(err)
		}
		work := readWorkspaceWork(strings.NewReader(fmt.Sprintf(`{"continuations":[{"durable":%q,"orchestrator":%t}]}`, dir, tc.orchestrator)))
		if got := work.needsWorkspace(owners); got != tc.want {
			t.Errorf("done=%v orchestrator=%v: got %v want %v", tc.done, tc.orchestrator, got, tc.want)
		}
		if work.needsWorkspace(t.TempDir()) {
			t.Fatal("accepted an owner outside Kai's session directory")
		}
	}
}
