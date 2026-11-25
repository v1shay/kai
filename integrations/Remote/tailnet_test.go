package main

import (
	"context"
	"errors"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"
	"time"

	"tailscale.com/net/netns"
	"tailscale.com/tailcfg"
	"tailscale.com/tsnet"
	"tailscale.com/tstest/integration"
	"tailscale.com/tstest/integration/testcontrol"
	"tailscale.com/types/logger"
)

func TestEmbeddedPeersPairAcrossBuildsAndReuseEnrollment(t *testing.T) {
	if testing.Short() {
		t.Skip("runs a loopback Tailscale test control and relay")
	}
	t.Setenv("TS_AUTHKEY", "")
	netns.SetEnabled(false)
	t.Cleanup(func() { netns.SetEnabled(true) })
	ctx, cancel := context.WithTimeout(t.Context(), 35*time.Second)
	defer cancel()
	control := &testcontrol.Server{
		DERPMap:        integration.RunDERPAndSTUN(t, logger.Discard, "127.0.0.1"),
		DNSConfig:      &tailcfg.DNSConfig{Proxied: true},
		MagicDNSDomain: "kai-test.ts.net",
	}
	control.HTTPTestServer = httptest.NewServer(control)
	t.Cleanup(control.HTTPTestServer.Close)
	makeNode := func(root, name string, build int) (*service, *tsnet.Server) {
		t.Helper()
		data := filepath.Join(root, "workspace")
		if err := os.MkdirAll(data, 0700); err != nil {
			t.Fatal(err)
		}
		s, err := newService(ctx, root, data, build)
		if err != nil {
			t.Fatal(err)
		}
		if err := s.state.change(func(i *identity) error { i.Enabled = true; return nil }); err != nil {
			t.Fatal(err)
		}
		tail := &tsnet.Server{Dir: filepath.Join(root, "tsnet"), Hostname: name, ControlURL: control.HTTPTestServer.URL,
			Logf: logger.Discard, UserLogf: logger.Discard}
		if err := s.attachTail(tail); err != nil {
			t.Fatal(err)
		}
		t.Cleanup(func() { _ = tail.Close() })
		status, err := tail.Up(ctx)
		if err != nil {
			t.Fatal(err)
		}
		if err := s.recordNode(string(status.Self.ID)); err != nil {
			t.Fatal(err)
		}
		return s, tail
	}
	aRoot, bRoot := t.TempDir(), t.TempDir()
	a, _ := makeNode(aRoot, "fixture-a-kai", 190)
	b, bTail := makeNode(bRoot, "fixture-b-kai", 220)
	aID, bID := a.state.snapshot(), b.state.snapshot()
	var target tailPeer
	for target.Node == "" {
		peers, err := a.tailPeers(ctx)
		if err != nil {
			t.Fatal(err)
		}
		for _, p := range peers {
			if p.Node == bID.Node {
				target = p
				break
			}
		}
		if target.Node != "" {
			break
		}
		select {
		case <-ctx.Done():
			t.Fatal("embedded peer did not appear")
		case <-time.After(100 * time.Millisecond):
		}
	}
	var discovered peer
	for discovered.Device == "" {
		p, err := a.probe(ctx, target)
		if err == nil {
			discovered = p
			break
		}
		select {
		case <-ctx.Done():
			t.Fatal("embedded Kai handshake did not succeed")
		case <-time.After(100 * time.Millisecond):
		}
	}
	if discovered.Device != bID.Device || discovered.Build != 220 || !discovered.Compatible || discovered.Allowed {
		t.Fatalf("unexpected discovery result: %+v", discovered)
	}
	var result map[string]bool
	if err := a.send(ctx, target, http.MethodPost, "pair", &result); err != nil {
		t.Fatal(err)
	}
	if err := b.decide(aID.Node, aID.Device, "allow", false, false, time.Now()); err != nil {
		t.Fatal(err)
	}
	var workspace metadata
	if err := a.send(ctx, target, http.MethodGet, "workspace", &workspace); err != nil {
		t.Fatal(err)
	}
	credentials := filepath.Join(bRoot, "tsnet", "tailscaled.state")
	if err := verifyTailnetState(credentials, bID.Node); err != nil {
		t.Fatalf("recovery rejected valid saved credentials: %v", err)
	}
	if err := verifyTailnetState(credentials, "different-node"); !errors.Is(err, errNodeChanged) {
		t.Fatalf("opening another node's credentials should fail: %v", err)
	}
	invalid := filepath.Join(t.TempDir(), "tailscaled.state")
	if err := os.WriteFile(invalid, []byte("{}"), 0600); err != nil {
		t.Fatal(err)
	}
	if err := verifyTailnetState(invalid, bID.Node); !errors.Is(err, errNetworkState) {
		t.Fatalf("an empty store must not enroll another node: %v", err)
	}

	host := b
	recovery, stopRecovery := context.WithCancel(ctx)
	done := make(chan struct{})
	go func() {
		defer close(done)
		host.runNetwork(recovery, func() error {
			if err := verifyTailnetState(credentials, bID.Node); err != nil {
				return err
			}
			return host.attachTail(&tsnet.Server{Dir: filepath.Dir(credentials), Hostname: "fixture-b-kai",
				ControlURL: control.HTTPTestServer.URL, Logf: logger.Discard, UserLogf: logger.Discard})
		}, 100*time.Millisecond, 3*time.Second)
	}()
	t.Cleanup(func() { stopRecovery(); <-done })
	if err := b.remoteServer.Close(); err != nil {
		t.Fatal(err)
	}
	reconnect, cancelReconnect := context.WithTimeout(ctx, 12*time.Second)
	defer cancelReconnect()
	for {
		host.mu.Lock()
		replacement := host.tail
		host.mu.Unlock()
		if replacement != nil && replacement != bTail {
			request, cancel := context.WithTimeout(reconnect, time.Second)
			_, err := a.remoteCall(request, localRequest{Action: "workspace", Node: bID.Node, Device: bID.Device})
			cancel()
			if err == nil {
				break
			}
		}
		select {
		case <-reconnect.Done():
			t.Fatal("paired workspace did not reconnect after the listener was closed")
		case <-time.After(100 * time.Millisecond):
		}
	}
	if recovered := host.state.snapshot(); recovered.Node != bID.Node || recovered.Device != bID.Device ||
		!host.state.isAllowed(aID.Node, aID.Device) {
		t.Fatal("recovery changed the host identity or forgot pairing")
	}
	if count := control.NumNodes(); count != 2 {
		t.Fatalf("recovery changed the number of registered nodes to %d", count)
	}
	stopRecovery()
	<-done
	b, _ = makeNode(bRoot, "fixture-b-kai", 221)
	newID := b.state.snapshot()
	if newID.Device != bID.Device || newID.Node != bID.Node {
		t.Fatal("update generated another device or tailnet node")
	}
	if !b.state.isAllowed(aID.Node, aID.Device) {
		t.Fatal("host forgot pairing across update")
	}
	if err := b.revoke(aID.Node, aID.Device); err != nil {
		t.Fatal(err)
	}
	if b.state.isAllowed(aID.Node, aID.Device) {
		t.Fatal("revoked peer still authorized")
	}
}
