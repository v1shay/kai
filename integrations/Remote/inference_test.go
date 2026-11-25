package main

import (
	"bufio"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"
	"time"
)

func TestInferenceRoutesRequireControlAndMatchOnlyEngineEndpoints(t *testing.T) {
	for _, entry := range []struct {
		method, path string
		want         bool
	}{
		{"GET", "ollama/api/tags", true},
		{"POST", "ollama/api/chat", true},
		{"POST", "ollama/api/pull", true},
		{"POST", "llama_cpp/v1/chat/completions", true},
		{"POST", "llama_cpp/model_info", true},
		{"GET", "llama_cpp/engine", true},
		{"POST", "ollama/infer/llama_cpp/v1/chat/completions", false},
		{"DELETE", "ollama/api/delete", false},
		{"POST", "ollama/api/../api/chat", false},
		{"POST", "ollama/api/create", false},
		{"GET", "192.168.0.1/api/tags", false},
	} {
		_, ok := inferenceEngine(entry.method, entry.path)
		if ok != entry.want {
			t.Errorf("%s %s: accepted=%t, want %t", entry.method, entry.path, ok, entry.want)
		}
	}
	w := httptest.NewRecorder()
	(&service{}).hostInference(w, httptest.NewRequest(http.MethodGet, "/", nil), peer{}, "ollama/api/tags")
	if w.Code != http.StatusForbidden {
		t.Fatalf("Read-only peer received %d", w.Code)
	}
}

func TestInferenceRelaysFirstEventBeforeUpstreamFinishes(t *testing.T) {
	complete := make(chan struct{})
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("Kai-Device") != "test-peer" {
			t.Error("Missing authenticated device header")
		}
		w.Header().Set("Content-Type", "application/x-ndjson")
		fmt.Fprintln(w, `{"message":"first"}`)
		_ = http.NewResponseController(w).Flush()
		select {
		case <-complete:
			fmt.Fprintln(w, `{"done":true}`)
		case <-r.Context().Done():
		}
	}))
	defer upstream.Close()
	relay := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		forwardInference(w, r, upstream.Client(), upstream.URL+"/api/chat", "test-peer")
	}))
	defer relay.Close()
	client := relay.Client()
	client.Timeout = 3 * time.Second
	res, err := client.Post(relay.URL, "application/json", nil)
	if err != nil {
		t.Fatal(err)
	}
	defer res.Body.Close()
	reader := bufio.NewReader(res.Body)
	first, err := reader.ReadString('\n')
	close(complete)
	if err != nil || first != "{\"message\":\"first\"}\n" {
		t.Fatalf("Streaming first event: %q, %v", first, err)
	}
	last, err := reader.ReadString('\n')
	if err != nil || last != "{\"done\":true}\n" {
		t.Fatalf("Streaming final event: %q, %v", last, err)
	}
}

func TestInferenceWorkerRecoversAfterOwnerPipeCloses(t *testing.T) {
	if _, err := os.Stat("/usr/bin/python3"); err != nil {
		t.Skip("The packaged worker uses the macOS system Python")
	}
	bin := t.TempDir()
	if err := os.WriteFile(filepath.Join(bin, "llama-server"), []byte("#!/bin/sh\nexit 0\n"), 0700); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", bin+string(os.PathListSeparator)+os.Getenv("PATH"))
	app, root := t.TempDir(), t.TempDir()
	resources, err := filepath.Abs("../Code/native_helpers")
	if err != nil {
		t.Fatal(err)
	}
	if err := os.Mkdir(filepath.Join(app, "Contents"), 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(resources, filepath.Join(app, "Contents", "Resources")); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = os.Remove(socketDir(root)) })
	var worker inferenceWorker
	t.Cleanup(worker.stop)
	check := func() {
		t.Helper()
		client, err := worker.connection(app, root)
		if err != nil {
			t.Fatal(err)
		}
		probe := *client
		probe.Timeout = 5 * time.Second
		response, err := probe.Get("http://kai.local/engine/llama_cpp")
		if err != nil {
			t.Fatal(err)
		}
		defer response.Body.Close()
		if response.StatusCode != http.StatusOK {
			t.Fatalf("Inference worker health returned %d", response.StatusCode)
		}
		info, err := os.Stat(filepath.Join(worker.directory, "engine.sock"))
		if err != nil || info.Mode().Perm() != 0600 {
			t.Fatalf("Expected owner-only worker socket, got %v (%v)", info, err)
		}
	}
	check()
	first := worker.directory
	exited := worker.exited
	if err := worker.pipe.Close(); err != nil {
		t.Fatal(err)
	}
	select {
	case <-exited:
	case <-time.After(5 * time.Second):
		t.Fatal("The inference worker did not exit with its owning server")
	}
	check()
	if worker.directory == first {
		t.Fatal("A stale inference worker was reused")
	}
}

func TestPrivateInferenceRelayStartsLocalWorkerWithoutTailnet(t *testing.T) {
	if _, err := os.Stat("/usr/bin/python3"); err != nil {
		t.Skip("The packaged worker uses system Python")
	}
	bin, app, root := t.TempDir(), t.TempDir(), t.TempDir()
	if err := os.WriteFile(filepath.Join(bin, "llama-server"), []byte("#!/bin/sh\nexit 0\n"), 0700); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", bin+string(os.PathListSeparator)+os.Getenv("PATH"))
	resources, err := filepath.Abs("../Code/native_helpers")
	if err != nil {
		t.Fatal(err)
	}
	if err := os.Mkdir(filepath.Join(app, "Contents"), 0700); err != nil {
		t.Fatal(err)
	}
	if err := os.Symlink(resources, filepath.Join(app, "Contents", "Resources")); err != nil {
		t.Fatal(err)
	}
	service := &service{app: app, root: root}
	t.Cleanup(func() {
		service.inference.stop()
		_ = os.Remove(socketDir(root))
	})
	w := httptest.NewRecorder()
	service.localInference(w, httptest.NewRequest(http.MethodGet, "/inference/self/self/llama_cpp/engine", nil))
	var body map[string]string
	if w.Code != http.StatusOK || json.Unmarshal(w.Body.Bytes(), &body) != nil || body["engine"] != "llama_cpp" {
		t.Fatalf("Local inference failed without a peer: HTTP %d %s", w.Code, w.Body)
	}
}
