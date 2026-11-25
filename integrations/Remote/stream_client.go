package main

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"net"
	"net/http"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"github.com/coder/websocket"
	"github.com/coder/websocket/wsjson"
)

func streamLocal(ctx context.Context, root string, input []byte, output io.Writer) error {
	dir := socketDir(root)
	if err := privateDir(dir); err != nil {
		return err
	}
	dialer := net.Dialer{}
	client := &http.Client{Transport: &http.Transport{
		DialContext: func(ctx context.Context, _, _ string) (net.Conn, error) {
			return dialer.DialContext(ctx, "unix", filepath.Join(dir, "server.sock"))
		},
		DisableKeepAlives: true,
	}}
	request, err := http.NewRequestWithContext(ctx, http.MethodPost, "http://kai.local/stream", strings.NewReader(string(input)))
	if err != nil {
		return err
	}
	res, err := client.Do(request)
	if err != nil {
		return err
	}
	defer res.Body.Close()
	if res.StatusCode != 200 {
		return fileResponseError(res)
	}
	_, err = io.Copy(output, res.Body)
	return err
}

func (s *service) proxyEvents(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodPost || r.Host != "kai.local" {
		http.NotFound(w, r)
		return
	}
	var req localRequest
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, 8192)).Decode(&req); err != nil {
		reject(w, 400, "Invalid Kai stream request")
		return
	}
	target, err := s.resolvePeer(r.Context(), req.Node, req.Device)
	if err != nil {
		reject(w, 503, err.Error())
		return
	}
	if !target.Allowed {
		reject(w, 403, "The host has not approved this Kai device")
		return
	}
	client := s.peerClient(target)
	if client == nil {
		reject(w, http.StatusServiceUnavailable, errTailnetOffline.Error())
		return
	}
	dialClient := &http.Client{Transport: client.Transport, Timeout: 8 * time.Second, CheckRedirect: client.CheckRedirect}
	header := http.Header{}
	header.Set("Kai-Device", s.state.snapshot().Device)
	header.Set("Kai-Protocol", strconv.Itoa(protocol))
	conn, _, err := websocket.Dial(r.Context(), "ws://"+target.address+prefix+"stream",
		&websocket.DialOptions{HTTPClient: dialClient, HTTPHeader: header, CompressionMode: websocket.CompressionDisabled})
	if err != nil {
		reject(w, 503, "Could not open a stream to the Kai host")
		return
	}
	defer conn.CloseNow()
	conn.SetReadLimit(maxResponse)
	if err := wsjson.Write(r.Context(), conn, streamSubscribe{Chat: req.Chat, Cursor: req.StreamCursor, Directory: req.File, Project: req.Project}); err != nil {
		reject(w, 503, "Could not subscribe to the Kai workspace")
		return
	}
	controller := http.NewResponseController(w)
	// The closing app connection or an explicit write deadline ends a slow subscriber.
	_ = controller.SetWriteDeadline(time.Time{})
	w.Header().Set("Content-Type", "application/x-ndjson")
	w.Header().Set("Cache-Control", "no-store")
	w.WriteHeader(200)
	_ = controller.Flush()
	for {
		messageType, body, err := conn.Read(r.Context())
		if err != nil {
			if websocket.CloseStatus(err) == websocket.StatusPolicyViolation {
				_, _ = io.WriteString(w, `{"type":"revoked"}`+"\n")
				_ = controller.Flush()
			}
			return
		}
		if messageType != websocket.MessageText || !json.Valid(body) {
			continue
		}
		_ = controller.SetWriteDeadline(time.Now().Add(5 * time.Second))
		if _, err = w.Write(append(body, '\n')); err != nil {
			return
		}
		if err = controller.Flush(); err != nil && !errors.Is(err, http.ErrNotSupported) {
			return
		}
	}
}
