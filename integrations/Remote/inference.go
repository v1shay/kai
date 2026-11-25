package main

import (
	"context"
	"io"
	"net"
	"net/http"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"time"
)

const inferencePath = "inference/"
const inferenceCapability = "ninja.inference"
const inferenceLimit = 32 << 20

var inferenceNode = regexp.MustCompile("^[0-9A-Za-z_-]{1,128}$")

var inferenceClient = &http.Client{
	Transport: &http.Transport{
		DialContext: (&net.Dialer{Timeout: 3 * time.Second}).DialContext,
	},
	CheckRedirect: func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse },
}

func inferenceEngine(method, path string) (string, bool) {
	engine, endpoint, found := strings.Cut(path, "/")
	if !found || (engine != "ollama" && engine != "llama_cpp") {
		return "", false
	}
	switch {
	case method == http.MethodGet && endpoint == "engine":
		if engine == "llama_cpp" {
			return "engine/llama_cpp", true
		}
		return "api/version", true
	case method == http.MethodGet && (endpoint == "api/tags" || engine == "ollama" && endpoint == "api/version"):
		return endpoint, true
	case method == http.MethodGet && engine == "llama_cpp" && endpoint == "v1/models":
		return "api/tags", true
	case method == http.MethodPost && slices.Contains([]string{"api/show", "api/pull", "model_info"}, endpoint):
		return endpoint, true
	case method == http.MethodPost && engine == "ollama" && (endpoint == "api/chat" || endpoint == "infer/ollama/api/chat"):
		return "infer/ollama/api/chat", true
	case method == http.MethodPost && engine == "llama_cpp" && (endpoint == "v1/chat/completions" || endpoint == "infer/llama_cpp/v1/chat/completions"):
		return "infer/llama_cpp/v1/chat/completions", true
	}
	return "", false
}

func forwardInference(w http.ResponseWriter, r *http.Request, client *http.Client, url, device string) {
	// The ordinary workspace deadline is short. A streamed generation or
	// model download has its own deadline and ends when its caller disconnects.
	controller := http.NewResponseController(w)
	_ = controller.SetWriteDeadline(time.Time{})
	_ = controller.SetReadDeadline(time.Now().Add(2 * time.Minute))
	ctx, cancel := context.WithTimeout(r.Context(), time.Hour)
	defer cancel()
	var body io.Reader
	if r.Method == http.MethodPost {
		body = http.MaxBytesReader(w, r.Body, inferenceLimit)
	}
	request, err := http.NewRequestWithContext(ctx, r.Method, url, body)
	if err != nil {
		reject(w, http.StatusBadRequest, "Invalid inference request")
		return
	}
	if r.ContentLength > inferenceLimit {
		reject(w, http.StatusRequestEntityTooLarge, "Inference request exceeds 32 MiB")
		return
	}
	request.ContentLength = r.ContentLength
	request.Header.Set("Content-Type", "application/json")
	if device != "" {
		request.Header.Set("Kai-Device", device)
		request.Header.Set("Kai-Protocol", strconv.Itoa(protocol))
	}
	response, err := client.Do(request)
	if err != nil {
		reject(w, http.StatusBadGateway, "The host inference engine is unavailable")
		return
	}
	defer response.Body.Close()
	if response.StatusCode >= 300 && response.StatusCode < 400 {
		reject(w, http.StatusBadGateway, "Unexpected inference redirect")
		return
	}
	w.Header().Set("Content-Type", response.Header.Get("Content-Type"))
	w.Header().Set("Cache-Control", "no-store")
	w.WriteHeader(response.StatusCode)
	_ = controller.Flush()
	buf := make([]byte, 16<<10)
	for {
		n, err := response.Body.Read(buf)
		if n > 0 {
			if _, writeErr := w.Write(buf[:n]); writeErr != nil {
				return
			}
			if flushErr := controller.Flush(); flushErr != nil {
				return
			}
		}
		if err != nil {
			return
		}
	}
}

func (s *service) hostInference(w http.ResponseWriter, r *http.Request, grant peer, path string) {
	if !grant.Control {
		reject(w, http.StatusForbidden, "Request agent control on the inference Mac")
		return
	}
	endpoint, ok := inferenceEngine(r.Method, path)
	if !ok || r.URL.RawQuery != "" {
		http.NotFound(w, r)
		return
	}
	client, err := s.inference.connection(s.app, s.root)
	if err != nil {
		reject(w, http.StatusBadGateway, "Kai could not start the inference worker")
		return
	}
	forwardInference(w, r, client, "http://kai.local/"+endpoint, "")
}

// This handler is only registered on the owner's mode-0600 Unix socket.
func (s *service) localInference(w http.ResponseWriter, r *http.Request) {
	path := strings.TrimPrefix(r.URL.Path, "/"+inferencePath)
	parts := strings.SplitN(path, "/", 3)
	if len(parts) != 3 || r.URL.RawQuery != "" {
		http.NotFound(w, r)
		return
	}
	if _, ok := inferenceEngine(r.Method, parts[2]); !ok {
		http.NotFound(w, r)
		return
	}
	if parts[0] == "self" && parts[1] == "self" {
		s.hostInference(w, r, peer{Control: true}, parts[2])
		return
	}
	if !inferenceNode.MatchString(parts[0]) || !validDevice.MatchString(parts[1]) {
		http.NotFound(w, r)
		return
	}
	peer, err := s.resolvePeer(r.Context(), parts[0], parts[1])
	if err != nil || !peer.Allowed || !peer.Control || !slices.Contains(peer.Capabilities, inferenceCapability) {
		reject(w, http.StatusBadGateway, "Paired inference Mac is unavailable or needs an update")
		return
	}
	client := s.peerClient(peer)
	if client == nil {
		reject(w, http.StatusBadGateway, "Kai network is offline")
		return
	}
	// Keep the authenticated transport while using the relay's streaming
	// deadline in place of the request timeout used for workspace metadata.
	streaming := *client
	streaming.Timeout = 0
	streaming.CheckRedirect = inferenceClient.CheckRedirect
	forwardInference(w, r, &streaming, "http://"+peer.address+prefix+inferencePath+parts[2], s.state.snapshot().Device)
}
