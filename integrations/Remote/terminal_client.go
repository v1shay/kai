package main

import (
	"context"
	"errors"
	"fmt"
	"net"
	"net/http"
	"net/url"
	"os"
	"os/signal"
	"path/filepath"
	"slices"
	"strconv"
	"syscall"
	"time"

	"github.com/coder/websocket"
	"github.com/coder/websocket/wsjson"
	"golang.org/x/term"
)

func (s *service) proxyTerminal(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodGet || r.Host != "kai.local" {
		http.NotFound(w, r)
		return
	}
	params := r.URL.Query()
	id := params.Get("id")
	if !terminalID.MatchString(id) {
		reject(w, 400, "Invalid terminal ID")
		return
	}
	p, err := s.resolvePeer(r.Context(), params.Get("node"), params.Get("device"))
	if err != nil || !p.Allowed || !p.Control ||
		(!slices.Contains(p.Capabilities, "workspace.paired-access") && (!p.Terminal || p.FileProject == "")) {
		reject(w, 403, "Terminal control is unavailable on this Mac")
		return
	}
	client := s.peerClient(p)
	if client == nil {
		reject(w, http.StatusServiceUnavailable, errTailnetOffline.Error())
		return
	}
	header := http.Header{
		"Kai-Device":   {s.state.snapshot().Device},
		"Kai-Protocol": {strconv.Itoa(protocol)},
	}
	up, _, err := websocket.Dial(r.Context(), "ws://"+p.address+prefix+"terminal?id="+id,
		&websocket.DialOptions{HTTPClient: client, HTTPHeader: header})
	if err != nil {
		reject(w, 503, "Could not attach to the host terminal")
		return
	}
	defer up.CloseNow()
	down, err := websocket.Accept(w, r, nil)
	if err != nil {
		return
	}
	defer down.CloseNow()
	down.SetReadLimit(16_000)
	up.SetReadLimit(64_000)
	ctx, cancel := context.WithCancel(r.Context())
	defer cancel()
	go func() { defer cancel(); _ = copyTerminalMessages(ctx, down, up) }()
	err = copyTerminalMessages(ctx, up, down)
	if code := websocket.CloseStatus(err); code == websocket.StatusNormalClosure || code == websocket.StatusPolicyViolation {
		_ = down.Close(code, "Remote terminal ended")
	}
}

func copyTerminalMessages(ctx context.Context, from, to *websocket.Conn) error {
	for {
		kind, data, err := from.Read(ctx)
		if err != nil {
			return err
		}
		write, cancel := context.WithTimeout(ctx, 5*time.Second)
		err = to.Write(write, kind, data)
		cancel()
		if err != nil {
			return err
		}
	}
}

func runTerminalClient(ctx context.Context, root, node, device, id string) error {
	if !terminalID.MatchString(id) || !validDevice.MatchString(device) || node == "" {
		return errors.New("Invalid remote terminal target")
	}
	dialer := net.Dialer{}
	transport := &http.Transport{DialContext: func(ctx context.Context, _, _ string) (net.Conn, error) {
		return dialer.DialContext(ctx, "unix", filepath.Join(socketDir(root), "server.sock"))
	}}
	client := &http.Client{Transport: transport, Timeout: 20 * time.Second}
	values := url.Values{"node": {node}, "device": {device}, "id": {id}}
	conn, res, err := websocket.Dial(ctx, "ws://kai.local/terminal?"+values.Encode(), &websocket.DialOptions{HTTPClient: client})
	if err != nil {
		if res != nil && res.Body != nil {
			defer res.Body.Close()
			return fileResponseError(res)
		}
		return err
	}
	defer conn.CloseNow()
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	conn.SetReadLimit(64_000)
	fd := int(os.Stdin.Fd())
	if term.IsTerminal(fd) {
		prior, err := term.MakeRaw(fd)
		if err != nil {
			return err
		}
		defer term.Restore(fd, prior)
	}
	windowChange := make(chan os.Signal, 1)
	signal.Notify(windowChange, syscall.SIGWINCH)
	defer signal.Stop(windowChange)
	windowChange <- syscall.SIGWINCH
	go func() {
		for {
			select {
			case <-ctx.Done():
				return
			case <-windowChange:
				w, h, err := term.GetSize(fd)
				if err == nil && w > 0 && h > 0 && w <= 1000 && h <= 1000 {
					up, stop := context.WithTimeout(ctx, 5*time.Second)
					_ = wsjson.Write(up, conn, terminalIO{Op: "resize", Cols: uint16(w), Rows: uint16(h)})
					stop()
				}
			}
		}
	}()
	go func() {
		buffer := make([]byte, 4096)
		for {
			n, err := os.Stdin.Read(buffer)
			if n > 0 {
				write, stop := context.WithTimeout(ctx, 5*time.Second)
				sendErr := conn.Write(write, websocket.MessageBinary, buffer[:n])
				stop()
				if sendErr != nil {
					return
				}
			}
			if err != nil {
				return
			}
		}
	}()
	for {
		kind, text, err := conn.Read(ctx)
		if websocket.CloseStatus(err) == websocket.StatusNormalClosure {
			return nil
		}
		if err != nil {
			return fmt.Errorf("Remote terminal disconnected: %w", err)
		}
		if kind == websocket.MessageBinary {
			if _, err := os.Stdout.Write(text); err != nil {
				return err
			}
		}
	}
}
