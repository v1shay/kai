package main

import (
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"net/http"
	"net/url"
	"time"
)

type pairingCode struct {
	Code    string    `json:"code"`
	Link    string    `json:"link"`
	Expires time.Time `json:"expires"`
}

func codeKey(code string) string { return fmt.Sprintf("%x", sha256.Sum256([]byte(code))) }

func (s *service) createCode() (pairingCode, error) {
	before := s.state.snapshot()
	if before.Node == "" || !before.Enabled {
		return pairingCode{}, errors.New("Enroll this Mac in Tailscale first")
	}
	var random [18]byte
	if _, err := rand.Read(random[:]); err != nil {
		return pairingCode{}, err
	}
	text := base64.RawURLEncoding.EncodeToString(random[:])
	expires := time.Now().Add(5 * time.Minute)
	err := s.state.change(func(i *identity) error {
		for key, expiry := range i.Codes {
			if !time.Now().Before(expiry) {
				delete(i.Codes, key)
			}
		}
		if len(i.Codes) >= 8 {
			return errors.New("Too many outstanding codes. Wait for an existing code to expire.")
		}
		i.Codes[codeKey(text)] = expires
		return nil
	})
	if err != nil {
		return pairingCode{}, err
	}
	q := url.Values{"node": {before.Node}, "device": {before.Device}, "code": {text}}
	return pairingCode{Code: text, Link: "kai://pair?" + q.Encode(), Expires: expires}, nil
}

func (s *service) consumeCode(p peer, text string, now time.Time) error {
	if len(text) != 24 {
		return errors.New("This pairing code is invalid or expired")
	}
	return s.state.change(func(i *identity) error {
		key := codeKey(text)
		if until, ok := i.Codes[key]; !ok || !now.Before(until) {
			return errors.New("This pairing code is invalid or expired")
		}
		if prior, exists := i.Trusted[p.Node]; exists {
			if prior.Device != p.Device {
				return errors.New("A different Kai identity is paired to this node")
			}
		} else {
			if len(i.Trusted) >= 64 {
				return errors.New("Kai’s pairing list is full")
			}
			p.Allowed = true
			i.Trusted[p.Node] = p
		}
		delete(i.Codes, key)
		delete(i.Requests, p.Node)
		return nil
	})
}

func (s *service) pairByCode(w http.ResponseWriter, r *http.Request, p peer) {
	var body struct {
		Code string `json:"code"`
	}
	if err := json.NewDecoder(http.MaxBytesReader(w, r.Body, 4096)).Decode(&body); err != nil {
		reject(w, 400, "Invalid pairing request")
		return
	}
	if err := s.consumeCode(p, body.Code, time.Now()); err != nil {
		reject(w, 403, err.Error())
		return
	}
	respond(w, 200, map[string]bool{"allowed": true})
}
