package main

import (
	"archive/zip"
	"context"
	"database/sql"
	"encoding/json"
	"fmt"
	"golang.org/x/sys/unix"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func TestIdentitySurvives100Openings(t *testing.T) {
	root := t.TempDir()
	const legacy = "499c1245-a0f0-48fb-b5f1-6e20610501ea"
	state, err := openState(root, legacy)
	if err != nil {
		t.Fatal(err)
	}
	err = state.change(func(i *identity) error {
		i.Node = "original-tailnet-node"
		i.Trusted["stable-peer"] = peer{Device: "b651648a715940d2a7b31df8498c8627", Control: true}
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
	for n := 0; n < 100; n++ {
		opened, err := openState(root, "")
		if err != nil {
			t.Fatal(err)
		}
		if opened.Node != "original-tailnet-node" || opened.Device != legacy || !opened.isAllowed("stable-peer", "b651648a715940d2a7b31df8498c8627") {
			t.Fatalf("state changed after reopening %d times", n)
		}
	}
}

func TestIdentityFailsClosedOnDowngrade(t *testing.T) {
	root := t.TempDir()
	state, err := openState(root, "")
	if err != nil {
		t.Fatal(err)
	}
	if err := state.change(func(i *identity) error { i.Schema = schemaVersion + 1; return nil }); err != nil {
		t.Fatal(err)
	}
	before, _ := os.ReadFile(state.filename)
	if _, err := openState(root, ""); err == nil {
		t.Fatal("newer schema accepted")
	}
	after, _ := os.ReadFile(state.filename)
	if string(after) != string(before) {
		t.Fatal("incompatible state was overwritten")
	}
}

func TestServerSingleInstanceAndRestart(t *testing.T) {
	root, data := t.TempDir(), t.TempDir()
	if err := os.Chmod(root, 0700); err != nil {
		t.Fatal(err)
	}
	defer os.RemoveAll(socketDir(root))
	run := func() (context.CancelFunc, <-chan error, string) {
		ctx, stop := context.WithCancel(context.Background())
		done := make(chan error, 1)
		go func() { done <- serve(ctx, root, data, "") }()
		if _, err := waitHealth(ctx, root, minimumBuild); err != nil {
			stop()
			t.Fatal(err)
		}
		raw, err := callLocal(ctx, root, "server.sock", []byte(`{"action":"health"}`))
		if err != nil {
			stop()
			t.Fatal(err)
		}
		var health struct {
			Device string `json:"device"`
		}
		if err := json.Unmarshal(raw, &health); err != nil || !validDevice.MatchString(health.Device) {
			stop()
			t.Fatalf("invalid health: %s", raw)
		}
		return stop, done, health.Device
	}
	stop, done, device := run()
	if err := serve(context.Background(), root, data, ""); err == nil || !strings.Contains(err.Error(), "already owns") {
		t.Fatalf("duplicate server not rejected: %v", err)
	}
	stop()
	if err := <-done; err != nil {
		t.Fatal(err)
	}
	stop, done, deviceAfter := run()
	stop()
	if err := <-done; err != nil {
		t.Fatal(err)
	}
	if deviceAfter != device {
		t.Fatal("server assigned another device after restart")
	}
}

func TestPairingBindingCompatibilityAndRevocation(t *testing.T) {
	host, err := newService(t.Context(), t.TempDir(), t.TempDir(), 220)
	if err != nil {
		t.Fatal(err)
	}
	other, err := newService(t.Context(), t.TempDir(), t.TempDir(), 190)
	if err != nil {
		t.Fatal(err)
	}
	host.who = func(r *http.Request) (peer, error) {
		if r.Header.Get("X-Test-Node") == "" {
			return peer{}, fmt.Errorf("missing node")
		}
		return peer{Node: r.Header.Get("X-Test-Node"), Device: r.Header.Get("Kai-Device"), Name: "test-mac", Account: "kai-user"}, nil
	}
	device := other.state.Device
	request := func(method, path, node, id, protocol string) *httptest.ResponseRecorder {
		r := httptest.NewRequest(method, prefix+path, nil)
		r.Header.Set("X-Test-Node", node)
		r.Header.Set("Kai-Device", id)
		r.Header.Set("Kai-Protocol", protocol)
		w := httptest.NewRecorder()
		host.remote(w, r)
		return w
	}
	checkStatus := func(w *httptest.ResponseRecorder, want int) {
		t.Helper()
		if w.Code != want {
			t.Fatalf("status %d, want %d: %s", w.Code, want, w.Body.String())
		}
	}
	checkStatus(request("GET", "workspace", "", device, "1"), 401)
	checkStatus(request("GET", "workspace", "real-node", device, "1"), 403)
	checkStatus(request("POST", "pair", "real-node", device, "8"), 426)
	w := request("GET", "hello", "real-node", device, "1")
	checkStatus(w, 200)
	var offer hello
	if err := json.Unmarshal(w.Body.Bytes(), &offer); err != nil || offer.Build != 220 || len(offer.Protocols) != 1 || offer.Protocols[0] != protocol {
		t.Fatalf("unexpected protocol offer: %s", w.Body.String())
	}
	checkStatus(request("POST", "pair", "real-node", device, "1"), 202)
	if err := host.decide("real-node", device, "allow", false, false, time.Now().Add(6*time.Minute)); err == nil {
		t.Fatal("expired approval accepted")
	}
	if err := host.decide("real-node", device, "allow", false, false, time.Now()); err != nil {
		t.Fatal(err)
	}
	checkStatus(request("GET", "workspace", "real-node", device, "1"), 200)
	checkStatus(request("GET", "workspace", "impersonator", device, "1"), 403)
	checkStatus(request("GET", "workspace", "real-node", host.state.Device, "1"), 403)
	checkStatus(request("POST", "command", "real-node", device, "1"), 404)
	checkStatus(request("POST", "request-control", "impersonator", device, "1"), 403)
	checkStatus(request("POST", "request-control", "real-node", device, "1"), 202)
	if host.state.snapshot().Trusted["real-node"].Control {
		t.Fatal("a request escalated permissions without approval")
	}
	if err := host.decide("real-node", device, "allow", true, true, time.Now()); err != nil {
		t.Fatal(err)
	}
	updated := host.state.snapshot().Trusted["real-node"]
	if !updated.Control || updated.Update || updated.Device != device {
		t.Fatalf("agent control changed the peer or unrelated permissions: %+v", updated)
	}
	checkStatus(request("POST", "unpair", "real-node", device, "1"), 200)
	checkStatus(request("GET", "workspace", "real-node", device, "1"), 403)
	restarted, err := openState(host.root, "")
	if err != nil || restarted.isAllowed("real-node", device) {
		t.Fatalf("revocation changed after restart: %v", err)
	}
}

func TestMutualPairingRequiresMatchingLocalIntentAndHostApproval(t *testing.T) {
	s, err := newService(t.Context(), t.TempDir(), t.TempDir(), 214)
	if err != nil {
		t.Fatal(err)
	}
	now := time.Now()
	expiry := now.Add(time.Minute)
	host := peer{Node: "host-node", Device: "confirmed-device", Name: "Host", Online: true, Allowed: true, Control: true, Update: true}
	if err := s.finishMutualPairings([]peer{host}, now); err != nil || s.state.isAllowed(host.Node, host.Device) {
		t.Fatal("a discovered host gained a grant without local pairing intent", err)
	}
	if err := s.state.change(func(i *identity) error {
		i.Outgoing[host.Node] = peer{Node: host.Node, Device: host.Device, MutualUntil: &expiry}
		return nil
	}); err != nil {
		t.Fatal(err)
	}
	impostor := host
	impostor.Device = "other-device"
	if err := s.finishMutualPairings([]peer{impostor, host}, expiry); err != nil || s.state.isAllowed(host.Node, host.Device) {
		t.Fatal("expired or unmatched intent became a grant", err)
	}
	if err := s.finishMutualPairings([]peer{impostor}, now); err != nil || s.state.isAllowed(host.Node, impostor.Device) {
		t.Fatal("matching node accepted a different device", err)
	}
	if err := s.finishMutualPairings([]peer{host}, now); err != nil {
		t.Fatal(err)
	}
	grant := s.state.snapshot().Trusted[host.Node]
	if grant.Device != host.Device || !grant.Control || grant.Update {
		t.Fatalf("unexpected reciprocal grant %+v", grant)
	}
	if err := s.revoke(host.Node, host.Device); err != nil {
		t.Fatal(err)
	}
	if err := s.finishMutualPairings([]peer{host}, now); err != nil || s.state.isAllowed(host.Node, host.Device) {
		t.Fatal("revoked grant was recreated", err)
	}
}

func TestHistoryUsesAuthoritativeExistingStorage(t *testing.T) {
	h := workspaceHistory{t.TempDir()}
	dbPath := filepath.Join(h.root, "history.sqlite")
	if err := os.WriteFile(dbPath, nil, 0600); err != nil {
		t.Fatal(err)
	}
	db, err := sql.Open("sqlite3", dbPath)
	if err != nil {
		t.Fatal(err)
	}
	query := `CREATE TABLE sessions(id TEXT,title TEXT, agent TEXT, project_path TEXT,updated_at INT,external_id TEXT,transcript_path TEXT);
	CREATE TABLE messages(id INTEGER,session_id TEXT,role TEXT,text TEXT);
	INSERT INTO sessions VALUES
	('real','Visible chat','codex','/visible',1,NULL,NULL),
	('hidden','Hidden chat','codex','/visible',2,NULL,NULL),
	('hidden-project','Hidden project chat','codex','/secret',3,NULL,NULL),
	('499c1245-a0f0-48fb-b5f1-6e20610501ea','Draft','codex','/visible',4,NULL,NULL);
	INSERT INTO messages VALUES (100,'real','user','Hello'),(101,'real','assistant','Hi'),(102,'hidden','user','hidden'),(103,'hidden-project','user','secret');`
	if _, err := db.Exec(query); err != nil {
		t.Fatal(err)
	}
	db.Close()
	if err := saveJSON(filepath.Join(h.root, "projects.json"), projectRegistry{Custom: []string{"/custom"}, Hidden: []string{"/secret"}, HiddenSessions: []string{"hidden"}}); err != nil {
		t.Fatal(err)
	}
	s, err := h.browse(t.Context(), "", 0)
	if err != nil || len(s.Projects) != 2 || len(s.Chats) != 1 || s.Chats[0].ID != "real" {
		t.Fatalf("snapshot=%+v err=%v", s, err)
	}
	m, err := h.messages(t.Context(), "real", 101)
	if err != nil || len(m) != 1 || m[0].Text != "Hello" {
		t.Fatalf("messages=%+v err=%v", m, err)
	}
	for _, chat := range []string{"hidden", "hidden-project", "499c1245-a0f0-48fb-b5f1-6e20610501ea"} {
		if _, err := h.messages(t.Context(), chat, 0); err == nil {
			t.Fatalf("returned restricted chat %s", chat)
		}
	}
	workspace := `{"open_tabs":["hidden","real","hidden-project"],"active_chat":"real",
		"drafts":{"real":"private unsent draft"},"terminal_dock":{"side":"right","height":270,"width":390},
		"terminal_decks":[{"owner":{"Chat":"real"},"active":"b52e9011-f017-41ac-8e8a-113cfb376055",
		"sessions":[{"id":"b52e9011-f017-41ac-8e8a-113cfb376055","project":"/visible"}]}]}`
	if err := os.WriteFile(filepath.Join(h.root, "workspace.json"), []byte(workspace), 0600); err != nil {
		t.Fatal(err)
	}
	overview, err := h.overview(t.Context(), []liveOutput{{Chat: "real", Status: "Working"}, {Chat: "hidden", Status: "Working"}})
	if err != nil || overview.Active != "real" || len(overview.Tabs) != 1 || !overview.Tabs[0].Terminal ||
		len(overview.Activity) != 1 || overview.Activity[0].Chat != "real" {
		t.Fatalf("unexpected overview: %+v, err=%v", overview, err)
	}
}

func TestInterruptedActivationRecoversPreviousApp(t *testing.T) {
	root := t.TempDir()
	app := filepath.Join(root, "main.app")
	stageRoot, err := os.MkdirTemp(root, ".kai-update-")
	if err != nil {
		t.Fatal(err)
	}
	stage := filepath.Join(stageRoot, "Kai Unified.app")
	fixtureBundle(t, app, 190)
	fixtureBundle(t, stage, 220)
	j := activationJournal{Build: 220, PreviousBuild: 190, StagedApp: stage}
	if err := saveJSON(filepath.Join(root, "activation.json"), j); err != nil {
		t.Fatal(err)
	}
	if err := unix.RenameatxNp(unix.AT_FDCWD, app, unix.AT_FDCWD, stage, unix.RENAME_SWAP); err != nil {
		t.Fatal(err)
	}
	if err := recoverActivation(t.Context(), root, app); err != nil {
		t.Fatal(err)
	}
	if build, err := bundleBuild(t.Context(), app); err != nil || build != 190 {
		t.Fatalf("rollback installed build %d err=%v", build, err)
	}
	if err := recoverActivation(t.Context(), root, app); err != nil {
		t.Fatal("rollback was not idempotent")
	}
}

func TestArchiveRejectsTraversal(t *testing.T) {
	root := t.TempDir()
	path := filepath.Join(root, "release.zip")
	f, err := os.Create(path)
	if err != nil {
		t.Fatal(err)
	}
	z := zip.NewWriter(f)
	if _, err := z.Create("Kai Unified.app/../../outside"); err != nil {
		t.Fatal(err)
	}
	if err := z.Close(); err != nil {
		t.Fatal(err)
	}
	if err := f.Close(); err != nil {
		t.Fatal(err)
	}
	if err := unpackRelease(path, root); err == nil {
		t.Fatal("accepted a traversal path")
	}
}

func fixtureBundle(t *testing.T, app string, build int) {
	t.Helper()
	if err := os.MkdirAll(filepath.Join(app, "Contents"), 0700); err != nil {
		t.Fatal(err)
	}
	contents := fmt.Sprintf(`<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>CFBundleVersion</key><string>%d</string></dict></plist>`, build)
	if err := os.WriteFile(filepath.Join(app, "Contents", "Info.plist"), []byte(contents), 0600); err != nil {
		t.Fatal(err)
	}
}

func TestScreenshotRequiresApprovingExactPeer(t *testing.T) {
	owner := peer{Node: "owner", Device: "788509bed6f04fab88c965cf4f3a19ea"}
	var collection screenshots
	request, err := collection.ask(owner)
	if err != nil {
		t.Fatal(err)
	}
	if len(collection.pending()) != 1 {
		t.Fatal("missing host approval")
	}
	w := httptest.NewRecorder()
	collection.fetch(w, request.ID, owner)
	if w.Code != 409 {
		t.Fatalf("unapproved capture returned %d", w.Code)
	}
	w = httptest.NewRecorder()
	collection.fetch(w, request.ID, peer{Node: "another-node", Device: owner.Device})
	if w.Code != 404 {
		t.Fatalf("different peer gained access: %d", w.Code)
	}
	if err := collection.resolve(request.ID, "", func(peer) bool { return false }); err == nil {
		t.Fatal("revoked screenshot request stayed authorized")
	}
	if err := collection.resolve(request.ID, "", func(p peer) bool { return p.Node == owner.Node && p.Device == owner.Device }); err != nil {
		t.Fatal(err)
	}
	if len(collection.pending()) != 0 {
		t.Fatal("screenshot decline did not clear the request")
	}
}

func TestWorkspaceApprovalEnablesAgentControlByDefault(t *testing.T) {
	host, err := newService(t.Context(), t.TempDir(), t.TempDir(), 220)
	if err != nil {
		t.Fatal(err)
	}
	client := peer{Node: "fixture-mac", Device: "c67e62ae-b2ab-4db4-b9d7-b54723f28456"}
	post := func(action string) *httptest.ResponseRecorder {
		data, err := json.Marshal(localRequest{Action: action, Node: client.Node, Device: client.Device})
		if err != nil {
			t.Fatal(err)
		}
		response := httptest.NewRecorder()
		host.local(response, httptest.NewRequest(http.MethodPost, "http://kai.local/", strings.NewReader(string(data))))
		return response
	}
	if response := post("allow-workspace"); response.Code == 200 {
		t.Fatal("workspace approval was accepted without a pairing request")
	}
	if err := host.requestPairing(client, time.Now()); err != nil {
		t.Fatal(err)
	}
	if response := post("allow-workspace"); response.Code != 200 {
		t.Fatalf("workspace approval failed: %s", response.Body)
	}
	grant := host.state.snapshot().Trusted[client.Node]
	if grant.Device != client.Device || !grant.Control || grant.Update {
		t.Fatalf("unexpected grant after workspace approval: %+v", grant)
	}
}
