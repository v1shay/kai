package main

import (
	"bytes"
	"context"
	"crypto/rand"
	"encoding/hex"
	"errors"
	"image"
	_ "image/png"
	"io"
	"net/http"
	"os"
	"strconv"
	"sync"
	"time"
)

const maxScreenshot = 4 << 20

type screenshotRequest struct {
	ID      string    `json:"id"`
	Peer    peer      `json:"peer"`
	Expires time.Time `json:"expires"`
	png     []byte
}

type screenshots struct {
	mu       sync.Mutex
	requests map[string]screenshotRequest
}

func (s *screenshots) prune() {
	for id, request := range s.requests {
		if !time.Now().Before(request.Expires) {
			delete(s.requests, id)
		}
	}
}

func (s *screenshots) pending() []screenshotRequest {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.prune()
	result := []screenshotRequest{}
	for _, request := range s.requests {
		if request.png == nil {
			result = append(result, request)
		}
	}
	return result
}

func (s *screenshots) ask(p peer) (screenshotRequest, error) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.requests == nil {
		s.requests = make(map[string]screenshotRequest)
	}
	s.prune()
	for _, request := range s.requests {
		if request.Peer.Node == p.Node && request.Peer.Device == p.Device && request.png == nil {
			return request, nil
		}
	}
	if len(s.requests) >= 8 {
		return screenshotRequest{}, errors.New("Screenshot queue is full")
	}
	var token [16]byte
	if _, err := rand.Read(token[:]); err != nil {
		return screenshotRequest{}, err
	}
	request := screenshotRequest{ID: hex.EncodeToString(token[:]), Peer: p, Expires: time.Now().Add(3 * time.Minute)}
	s.requests[request.ID] = request
	return request, nil
}

func (s *screenshots) resolve(id, path string, allowed func(peer) bool) error {
	var png []byte
	if path != "" {
		file, err := os.Open(path)
		if err != nil {
			return err
		}
		png, err = io.ReadAll(io.LimitReader(file, maxScreenshot+1))
		file.Close()
		if err != nil {
			return err
		}
		if len(png) > maxScreenshot {
			return errors.New("Screenshot is larger than 4 MiB")
		}
		config, format, err := image.DecodeConfig(bytes.NewReader(png))
		if err != nil || format != "png" || config.Width < 1 || config.Height < 1 || config.Width > 1920 || config.Height > 1920 {
			return errors.New("Invalid screenshot PNG")
		}
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	s.prune()
	item, ok := s.requests[id]
	if !ok || item.png != nil || !allowed(item.Peer) {
		return errors.New("Screenshot request has expired or access was revoked")
	}
	if png == nil {
		delete(s.requests, id)
		return nil
	}
	item.png = png
	item.Expires = time.Now().Add(3 * time.Minute)
	s.requests[id] = item
	return nil
}

func (s *screenshots) fetch(w http.ResponseWriter, id string, p peer) {
	s.mu.Lock()
	s.prune()
	item, ok := s.requests[id]
	s.mu.Unlock()
	if !ok || item.Peer.Node != p.Node || item.Peer.Device != p.Device {
		reject(w, 404, "Screenshot declined or expired")
		return
	}
	if item.png == nil {
		reject(w, 409, "Waiting for approval on the host Mac")
		return
	}
	writePNG(w, item.png)
}

func writePNG(w http.ResponseWriter, png []byte) {
	w.Header().Set("Content-Type", "image/png")
	w.Header().Set("Cache-Control", "no-store")
	w.Header().Set("X-Content-Type-Options", "nosniff")
	_, _ = w.Write(png)
}

func (s *service) pullScreenshot(ctx context.Context, node, device, id string) ([]byte, error) {
	if !validDevice.MatchString(id) {
		return nil, errors.New("Invalid screenshot request")
	}
	target, err := s.resolvePeer(ctx, node, device)
	if err != nil {
		return nil, err
	}
	client := s.peerClient(target)
	if client == nil {
		return nil, errTailnetOffline
	}
	r, err := http.NewRequestWithContext(ctx, http.MethodGet, "http://"+target.address+prefix+"screenshot?id="+id, nil)
	if err != nil {
		return nil, err
	}
	r.Header.Set("Kai-Device", s.state.snapshot().Device)
	r.Header.Set("Kai-Protocol", strconv.Itoa(protocol))
	res, err := client.Do(r)
	if err != nil {
		return nil, errors.New("Could not reach the screenshot host")
	}
	defer res.Body.Close()
	if res.Header.Get("Content-Type") != "image/png" || res.StatusCode != 200 {
		if res.StatusCode == 409 {
			return nil, errors.New("Waiting for approval on the host Mac")
		}
		return nil, errors.New("Screenshot declined, expired, or unavailable")
	}
	data, err := io.ReadAll(io.LimitReader(res.Body, maxScreenshot+1))
	if err != nil {
		return nil, err
	}
	if len(data) > maxScreenshot {
		return nil, errors.New("Screenshot exceeds its transfer limit")
	}
	config, format, err := image.DecodeConfig(bytes.NewReader(data))
	if err != nil || format != "png" || config.Width < 1 || config.Height < 1 || config.Width > 1920 || config.Height > 1920 {
		return nil, errors.New("Invalid screenshot PNG")
	}
	return data, nil
}
