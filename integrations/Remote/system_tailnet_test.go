package main

import (
	"context"
	"net/http"
	"net/http/httptest"
	"net/netip"
	"testing"

	"tailscale.com/ipn/ipnstate"
	"tailscale.com/types/key"
)

func TestSystemTailnetOnlySelectsKaiTailnetAddresses(t *testing.T) {
	status := &ipnstate.Status{
		TailscaleIPs: []netip.Addr{netip.MustParseAddr("100.64.0.1")},
		Peer: map[key.NodePublic]*ipnstate.PeerStatus{
			key.NewNode().Public(): {ID: "studio", HostName: "studio-kai", TailscaleIPs: []netip.Addr{netip.MustParseAddr("100.64.0.2")}},
			key.NewNode().Public(): {ID: "public", HostName: "public-kai", TailscaleIPs: []netip.Addr{netip.MustParseAddr("192.0.2.1")}},
			key.NewNode().Public(): {ID: "other", HostName: "other", TailscaleIPs: []netip.Addr{netip.MustParseAddr("100.64.0.3")}},
		},
	}
	peers := systemTailPeers(t.Context(), status)
	if len(peers) != 1 || peers[0].Node != "studio" || peers[0].Transport != "system" {
		t.Fatalf("unexpected system peers: %v", peers)
	}
	transport := peers[0].transport.Transport.(*http.Transport)
	if conn, err := transport.DialContext(t.Context(), "tcp", "127.0.0.1:47832"); err == nil {
		conn.Close()
		t.Fatal("system transport accepted an address outside Tailscale")
	}
	if conn, err := transport.DialContext(t.Context(), "tcp", "100.64.0.2:80"); err == nil {
		conn.Close()
		t.Fatal("system transport accepted a port outside Kai")
	}
}

func TestSystemNodeNeedsItsOwnPairing(t *testing.T) {
	const device = "499c1245-a0f0-48fb-b5f7-6e20610501ea"
	host, err := newService(t.Context(), t.TempDir(), t.TempDir(), 195)
	if err != nil {
		t.Fatal(err)
	}
	if err := host.state.change(func(i *identity) error {
		i.Trusted["embedded"] = peer{Device: device, Control: true}
		return nil
	}); err != nil {
		t.Fatal(err)
	}
	host.who = func(*http.Request) (peer, error) {
		return peer{Node: "system", Device: device}, nil
	}
	req := httptest.NewRequest(http.MethodGet, prefix+"workspace", nil)
	req.Header.Set("Kai-Protocol", "1")
	w := httptest.NewRecorder()
	host.remote(w, req)
	if w.Code != http.StatusForbidden {
		t.Fatalf("system node inherited embedded access, status %d", w.Code)
	}

	life, cancel := context.WithCancel(t.Context())
	cancel()
	client := systemPeerClient(life, netip.MustParseAddr("100.64.0.1"))
	transport := client.Transport.(*http.Transport)
	if conn, err := transport.DialContext(t.Context(), "tcp", "100.64.0.2:47832"); err == nil {
		conn.Close()
		t.Fatal("disabled system transport accepted a new connection")
	}
}
