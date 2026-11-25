package main

import (
	"context"
	"encoding/json"
	"errors"
	"net"
	"net/http"
	"net/netip"
	"os"
	"os/exec"
	"sync"
	"time"

	"tailscale.com/ipn/ipnstate"
	"tailscale.com/net/tsaddr"
)

const (
	systemTailscaleCLI       = "/Applications/Tailscale.app/Contents/MacOS/tailscale"
	systemTailscaleSignature = `anchor apple generic and identifier "io.tailscale.ipn.macsys" and certificate leaf[subject.OU] = "W5364U7YZB"`
)

// Only the already connected, signed macOS standalone client provides this
// transport. The Studio still authenticates its source node and Kai pairing.
type systemTailnet struct {
	mu       sync.Mutex
	until    time.Time
	snapshot *ipnstate.Status
	err      error
	life     context.Context
	stop     context.CancelFunc
}

func (n *systemTailnet) peers(ctx context.Context) ([]tailPeer, error) {
	n.mu.Lock()
	defer n.mu.Unlock()
	if !time.Now().Before(n.until) {
		probe, cancel := context.WithTimeout(ctx, 3*time.Second)
		n.snapshot, n.err = readSystemTailnet(probe)
		if probe.Err() == nil {
			n.until = time.Now().Add(2 * time.Second)
		}
		cancel()
	}
	if n.err != nil {
		return nil, n.err
	}
	if n.life == nil {
		n.life, n.stop = context.WithCancel(context.Background())
	}
	return systemTailPeers(n.life, n.snapshot), nil
}

func (n *systemTailnet) reset() {
	n.mu.Lock()
	defer n.mu.Unlock()
	if n.stop != nil {
		n.stop()
	}
	n.life, n.stop, n.snapshot = nil, nil, nil
	n.until = time.Time{}
}

func readSystemTailnet(ctx context.Context) (*ipnstate.Status, error) {
	if err := exec.CommandContext(ctx, "/usr/bin/codesign", "--verify", "-R="+systemTailscaleSignature, systemTailscaleCLI).Run(); err != nil {
		return nil, errors.New("Install the signed, standalone Tailscale app in Applications.")
	}
	cmd := exec.CommandContext(ctx, systemTailscaleCLI, "status", "--json")
	cmd.Env = append(os.Environ(), "TAILSCALE_BE_CLI=1")
	raw, err := cmd.Output()
	if err != nil {
		return nil, errors.New("Could not read the Mac’s Tailscale connection.")
	}
	var status ipnstate.Status
	if len(raw) > maxResponse || json.Unmarshal(raw, &status) != nil ||
		status.BackendState != "Running" || status.Self == nil || status.Self.ID == "" {
		return nil, errors.New("Connect the Mac’s Tailscale app to use it as a backup.")
	}
	return &status, nil
}

func systemTailPeers(life context.Context, status *ipnstate.Status) []tailPeer {
	peers := kaiPeersOnTailnet(status)
	transports := make(map[bool]*http.Client, 2)
	for _, source := range status.TailscaleIPs {
		if tsaddr.IsTailscaleIP(source) && transports[source.Is4()] == nil {
			transports[source.Is4()] = systemPeerClient(life, source)
		}
	}
	selected := peers[:0]
	for _, peer := range peers {
		if addr, err := netip.ParseAddrPort(peer.address); err == nil && tsaddr.IsTailscaleIP(addr.Addr()) {
			if transport := transports[addr.Addr().Is4()]; transport != nil {
				peer.Transport = "system"
				peer.transport = transport
				selected = append(selected, peer)
			}
		}
	}
	return selected
}

type systemPeerConn struct {
	net.Conn
	stop func() bool
}

func (c systemPeerConn) Close() error {
	c.stop()
	return c.Conn.Close()
}

func systemPeerClient(life context.Context, source netip.Addr) *http.Client {
	dialer := net.Dialer{LocalAddr: &net.TCPAddr{IP: source.AsSlice()}, Timeout: 5 * time.Second}
	transport := &http.Transport{
		DialContext: func(ctx context.Context, network, address string) (net.Conn, error) {
			if err := life.Err(); err != nil {
				return nil, err
			}
			target, err := netip.ParseAddrPort(address)
			if err != nil || target.Port() != 47832 || !tsaddr.IsTailscaleIP(target.Addr()) || target.Addr().Is4() != source.Is4() {
				return nil, errors.New("The Mac’s Tailscale backup requires a Kai peer’s tailnet address.")
			}
			conn, err := dialer.DialContext(ctx, network, address)
			if err != nil {
				return nil, err
			}
			if conn.RemoteAddr().String() != address {
				_ = conn.Close()
				return nil, errors.New("The connection was redirected outside the Kai peer’s tailnet address.")
			}
			stop := context.AfterFunc(life, func() { _ = conn.Close() })
			return systemPeerConn{Conn: conn, stop: stop}, nil
		},
		DisableKeepAlives: true, ResponseHeaderTimeout: 52 * time.Second, MaxResponseHeaderBytes: 16384,
	}
	return &http.Client{Transport: transport, Timeout: 55 * time.Second,
		CheckRedirect: func(*http.Request, []*http.Request) error {
			return errors.New("Kai peers cannot redirect requests")
		},
	}
}
