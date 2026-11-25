package main

import (
	"bytes"
	"crypto/sha256"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

// Run with the older signed Kai ZIP and the exact candidate release asset.
// The fixture exercises the GitHub path without publishing a release.
func TestSignedReleaseAssetPreparation(t *testing.T) {
	installed := os.Getenv("KAI_TEST_INSTALLED_ZIP")
	release := os.Getenv("KAI_TEST_RELEASE_ZIP")
	if installed == "" || release == "" {
		t.Skip("set KAI_TEST_INSTALLED_ZIP and KAI_TEST_RELEASE_ZIP to verify a signed release asset")
	}
	dir := t.TempDir()
	base := filepath.Join(dir, "installed")
	if err := os.Mkdir(base, 0700); err != nil {
		t.Fatal(err)
	}
	if err := unpackRelease(installed, base); err != nil {
		t.Fatal(err)
	}
	app := filepath.Join(base, "Kai Unified.app")
	version, err := bundleBuild(t.Context(), app)
	if err != nil {
		t.Fatal(err)
	}
	candidate := filepath.Join(dir, "candidate")
	if err := os.Mkdir(candidate, 0700); err != nil {
		t.Fatal(err)
	}
	if err := unpackRelease(release, candidate); err != nil {
		t.Fatal(err)
	}
	nextApp := filepath.Join(candidate, "Kai Unified.app")
	nextVersion, err := bundleBuild(t.Context(), nextApp)
	if err != nil || nextVersion <= version {
		t.Fatalf("expected a newer release asset, installed=%d release=%d err=%v", version, nextVersion, err)
	}
	baseName := filepath.Base(release)
	assetURL := "https://github.com/v1shay/kai/releases/download/test/" + baseName
	prepare := func(t *testing.T, archive string) (updateStatus, error) {
		t.Helper()
		f, err := os.Open(archive)
		if err != nil {
			t.Fatal(err)
		}
		hash := sha256.New()
		size, err := io.Copy(hash, f)
		_ = f.Close()
		if err != nil {
			t.Fatal(err)
		}
		metadata, _ := json.Marshal(map[string]any{
			"assets": []releaseAsset{
				{Name: baseName, URL: assetURL, Size: size, Digest: fmt.Sprintf("sha256:%x", hash.Sum(nil))},
			},
		})
		client := &http.Client{Transport: fixtureTransport(func(r *http.Request) (*http.Response, error) {
			switch r.URL.String() {
			case releaseEndpoint:
				return &http.Response{StatusCode: 200, Body: io.NopCloser(bytes.NewReader(metadata))}, nil
			case assetURL:
				body, err := os.Open(archive)
				return &http.Response{StatusCode: 200, Body: body}, err
			default:
				return nil, fmt.Errorf("unexpected update destination %s", r.URL)
			}
		})}
		u := makeUpdater(t.TempDir(), app, version)
		result, err := u.prepareGitHub(t.Context(), client)
		if err == nil {
			staged, readErr := readPrepared(u.root)
			if readErr != nil || staged.Build != nextVersion {
				t.Fatalf("no matching release staged: %+v %v", staged, readErr)
			}
		} else if _, readErr := readPrepared(u.root); !os.IsNotExist(readErr) {
			t.Fatalf("a failed release left a staged install: %v", readErr)
		}
		return result, err
	}
	t.Run("official asset shape with matching publisher", func(t *testing.T) {
		result, err := prepare(t, release)
		if err != nil || result.State != "ready" || result.Build != nextVersion {
			t.Fatalf("release did not stage: %+v %v", result, err)
		}
	})
	t.Run("valid ZIP and digest with wrong publisher", func(t *testing.T) {
		cmd := exec.CommandContext(t.Context(), "/usr/bin/codesign", "--force", "--sign", "-", nextApp)
		if output, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("could not prepare ad hoc signature: %v %s", err, output)
		}
		wrong := filepath.Join(dir, "wrong-publisher.zip")
		cmd = exec.CommandContext(t.Context(), "/usr/bin/ditto", "-c", "-k", "--norsrc", "--keepParent", nextApp, wrong)
		if output, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("could not package wrong-publisher fixture: %v %s", err, output)
		}
		if _, err := prepare(t, wrong); err == nil || !strings.Contains(err.Error(), "verification failed") {
			t.Fatalf("incorrect publisher result: %v", err)
		}
	})
}
