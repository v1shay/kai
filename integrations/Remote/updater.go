package main

import (
	"archive/zip"
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"runtime"
	"strconv"
	"strings"
	"sync"
	"time"
)

const releaseEndpoint = "https://api.github.com/repos/v1shay/kai/releases/latest"
const bundleID = "com.kai.unified.experimental"
const maxArchive = 512 << 20

type updateStatus struct {
	State   string `json:"state"`
	Build   int    `json:"build,omitempty"`
	Message string `json:"message,omitempty"`
	Remote  bool   `json:"remote,omitempty"`
}

type releaseAsset struct {
	Name   string `json:"name"`
	URL    string `json:"browser_download_url"`
	Digest string `json:"digest"`
	Size   int64  `json:"size"`
}

type updater struct {
	root, app string
	build     int
	mu        sync.Mutex
	value     updateStatus
	check     chan string
}

func makeUpdater(root, app string, build int) *updater {
	if filepath.Ext(app) != ".app" {
		return nil
	}
	return &updater{root: root, app: app, build: build, value: updateStatus{State: "idle"}, check: make(chan string, 1)}
}

func (u *updater) status() updateStatus {
	u.mu.Lock()
	defer u.mu.Unlock()
	update := u.value
	if b, err := os.ReadFile(filepath.Join(u.root, "update-result.json")); err == nil {
		var result updateStatus
		if json.Unmarshal(b, &result) == nil {
			if result.State == "installed" && result.Build == u.build && update.State == "idle" {
				return result
			}
			if result.State == "error" && (update.State == "idle" || update.State == "ready") {
				return result
			}
		}
	}
	return update
}

func (u *updater) set(value updateStatus) {
	u.mu.Lock()
	defer u.mu.Unlock()
	u.value = value
}

func (u *updater) scheduleCheck(archive string) error {
	if archive != "" {
		if !filepath.IsAbs(archive) || !strings.EqualFold(filepath.Ext(archive), ".zip") {
			return errors.New("Select a Kai update ZIP")
		}
		switch u.status().State {
		case "ready", "checking":
			return errors.New("Another Kai update is already being prepared or is ready")
		}
	}
	select {
	case u.check <- archive:
		return nil
	default:
		return errors.New("An update check is already queued")
	}
}

func (u *updater) run(ctx context.Context) {
	if prepared, err := readPrepared(u.root); err == nil && prepared.Build > u.build {
		u.set(updateStatus{State: "ready", Build: prepared.Build})
	}
	initial := time.NewTimer(3 * time.Minute)
	defer initial.Stop()
	ticker := time.NewTicker(6 * time.Hour)
	defer ticker.Stop()
	for {
		var archive string
		select {
		case <-ctx.Done():
			return
		case <-initial.C:
		case <-ticker.C:
		case archive = <-u.check:
			_ = os.Remove(filepath.Join(u.root, "update-result.json"))
		}
		if u.status().State == "ready" {
			continue
		}
		if _, err := os.Stat(filepath.Join(u.root, "activation.json")); err == nil {
			continue
		}
		u.set(updateStatus{State: "checking"})
		checkCtx, cancel := context.WithTimeout(ctx, 20*time.Minute)
		var value updateStatus
		var err error
		if archive == "" {
			value, err = u.prepare(checkCtx)
		} else {
			value, err = u.importArchive(checkCtx, archive)
		}
		cancel()
		if err != nil {
			u.set(updateStatus{State: "error", Message: err.Error()})
		} else {
			u.set(value)
		}
	}
}

type preparedUpdate struct {
	App   string `json:"app"`
	Build int    `json:"build"`
}

func readPrepared(root string) (preparedUpdate, error) {
	var update preparedUpdate
	b, err := os.ReadFile(filepath.Join(root, "prepared.json"))
	if err != nil {
		return update, err
	}
	err = json.Unmarshal(b, &update)
	return update, err
}

func trustedDownload(raw string) bool {
	u, err := url.Parse(raw)
	if err != nil || u.Scheme != "https" || u.User != nil || u.Port() != "" {
		return false
	}
	host := u.Hostname()
	switch host {
	case "api.github.com", "release-assets.githubusercontent.com", "objects.githubusercontent.com":
		return true
	case "github.com":
		return strings.HasPrefix(u.Path, "/v1shay/kai/releases/download/")
	default:
		return false
	}
}

func githubRequest(ctx context.Context, client *http.Client, raw string) (*http.Response, error) {
	if !trustedDownload(raw) {
		return nil, errors.New("GitHub update URL is outside the trusted release endpoints")
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, raw, nil)
	if err != nil {
		return nil, err
	}
	req.Header.Set("User-Agent", "Kai-Unified-Updater")
	if raw == releaseEndpoint {
		req.Header.Set("Accept", "application/vnd.github+json")
	}
	return client.Do(req)
}

var assetName = regexp.MustCompile(`^Kai-Unified-([0-9]+)-macos-(arm64|amd64|universal)\.zip$`)

func (u *updater) prepare(ctx context.Context) (updateStatus, error) {
	config, err := u.configuration()
	if err != nil {
		return updateStatus{}, err
	}
	switch config.Channel {
	case "off":
		return updateStatus{State: "idle", Message: "Automatic updates are disabled on this Mac."}, nil
	case "developer":
		return u.prepareDeveloper(ctx, config)
	}

	client := &http.Client{Timeout: 15 * time.Minute, CheckRedirect: func(r *http.Request, via []*http.Request) error {
		if len(via) > 4 || !trustedDownload(r.URL.String()) {
			return errors.New("Rejected unexpected update redirect")
		}
		return nil
	}}
	return u.prepareGitHub(ctx, client)
}

func (u *updater) prepareGitHub(ctx context.Context, client *http.Client) (updateStatus, error) {
	response, err := githubRequest(ctx, client, releaseEndpoint)
	if err != nil {
		return updateStatus{}, err
	}
	defer response.Body.Close()
	if response.StatusCode == http.StatusNotFound {
		return updateStatus{State: "idle", Message: "No accessible GitHub release is available."}, nil
	}
	if response.StatusCode != http.StatusOK {
		return updateStatus{}, fmt.Errorf("GitHub release request returned HTTP %d", response.StatusCode)
	}
	var release struct {
		Draft      bool           `json:"draft"`
		Prerelease bool           `json:"prerelease"`
		Assets     []releaseAsset `json:"assets"`
	}
	if err := json.NewDecoder(io.LimitReader(response.Body, 1<<20)).Decode(&release); err != nil {
		return updateStatus{}, err
	}
	response.Body.Close()
	var selected releaseAsset
	build := u.build
	if !release.Draft && !release.Prerelease {
		for _, asset := range release.Assets {
			match := assetName.FindStringSubmatch(asset.Name)
			if match == nil || match[2] != runtime.GOARCH && match[2] != "universal" {
				continue
			}
			version, _ := strconv.Atoi(match[1])
			if version > build {
				selected, build = asset, version
			}
		}
	}
	if build == u.build {
		return updateStatus{State: "idle", Message: "No newer Kai release is available."}, nil
	}
	digest, err := hex.DecodeString(strings.TrimPrefix(selected.Digest, "sha256:"))
	if err != nil || len(digest) != sha256.Size || !strings.HasPrefix(selected.Digest, "sha256:") || selected.Size <= 0 || selected.Size > maxArchive {
		return updateStatus{}, errors.New("GitHub release is missing a valid size or SHA-256 digest")
	}
	stage, err := os.MkdirTemp(filepath.Dir(u.app), ".kai-update-*")
	if err != nil {
		return updateStatus{}, err
	}
	keep := false
	defer func() {
		if !keep {
			_ = os.RemoveAll(stage)
		}
	}()
	archive := filepath.Join(stage, "release.zip")
	if err := downloadRelease(ctx, client, selected, archive, hex.EncodeToString(digest)); err != nil {
		return updateStatus{}, err
	}
	value, err := u.stageArchive(ctx, archive, stage, build)
	if err != nil {
		return updateStatus{}, err
	}
	_ = os.Remove(archive)
	keep = true
	return value, nil
}

func (u *updater) importArchive(ctx context.Context, archive string) (updateStatus, error) {
	info, err := os.Stat(archive)
	if err != nil || !info.Mode().IsRegular() || info.Size() <= 0 || info.Size() > maxArchive {
		return updateStatus{}, errors.New("The Kai ZIP is missing, empty, or too large")
	}
	stage, err := os.MkdirTemp(filepath.Dir(u.app), ".kai-update-*")
	if err != nil {
		return updateStatus{}, err
	}
	value, err := u.stageArchive(ctx, archive, stage, 0)
	if err != nil {
		_ = os.RemoveAll(stage)
	}
	return value, err
}

func (u *updater) stageArchive(ctx context.Context, archive, stage string, build int) (updateStatus, error) {
	if err := unpackRelease(archive, stage); err != nil {
		return updateStatus{}, err
	}
	app := filepath.Join(stage, "Kai Unified.app")
	if build == 0 {
		var err error
		build, err = bundleBuild(ctx, app)
		if err != nil {
			return updateStatus{}, errors.New("The ZIP does not contain a valid Kai app")
		}
	}
	if build <= u.build {
		return updateStatus{}, errors.New("The update must be newer than this Kai installation")
	}
	if err := verifyReplacement(ctx, u.app, app, build); err != nil {
		return updateStatus{}, err
	}
	if err := saveJSON(filepath.Join(u.root, "prepared.json"), preparedUpdate{App: app, Build: build}); err != nil {
		return updateStatus{}, err
	}
	return updateStatus{State: "ready", Build: build}, nil
}

func downloadRelease(ctx context.Context, client *http.Client, asset releaseAsset, path, want string) error {
	res, err := githubRequest(ctx, client, asset.URL)
	if err != nil {
		return err
	}
	defer res.Body.Close()
	if res.StatusCode != http.StatusOK {
		return fmt.Errorf("GitHub asset request returned HTTP %d", res.StatusCode)
	}
	f, err := os.OpenFile(path, os.O_CREATE|os.O_EXCL|os.O_WRONLY, 0600)
	if err != nil {
		return err
	}
	hash := sha256.New()
	n, copyErr := io.Copy(io.MultiWriter(f, hash), io.LimitReader(res.Body, asset.Size+1))
	closeErr := f.Close()
	if copyErr != nil {
		return copyErr
	}
	if closeErr != nil {
		return closeErr
	}
	if n != asset.Size || hex.EncodeToString(hash.Sum(nil)) != want {
		return errors.New("GitHub update failed size or SHA-256 verification")
	}
	return nil
}

func unpackRelease(path, destination string) error {
	z, err := zip.OpenReader(path)
	if err != nil {
		return err
	}
	defer z.Close()
	var expanded uint64
	for _, file := range z.File {
		name := strings.TrimSuffix(file.Name, "/")
		mode := file.Mode()
		if !filepath.IsLocal(name) || filepath.Clean(name) != name ||
			(name != "Kai Unified.app" && !strings.HasPrefix(name, "Kai Unified.app/")) ||
			(!mode.IsDir() && !mode.IsRegular()) {
			return errors.New("Release contains an invalid bundle path or archive entry")
		}
		if file.UncompressedSize64 > 2*maxArchive-expanded {
			return errors.New("Unpacked release exceeds the size limit")
		}
		expanded += file.UncompressedSize64
		target := filepath.Join(destination, name)
		if mode.IsDir() {
			if err := os.MkdirAll(target, 0755); err != nil {
				return err
			}
			continue
		}
		if err := os.MkdirAll(filepath.Dir(target), 0755); err != nil {
			return err
		}
		if err := unpackFile(file, target); err != nil {
			return err
		}
	}
	return nil
}

func unpackFile(entry *zip.File, target string) error {
	rc, err := entry.Open()
	if err != nil {
		return err
	}
	defer rc.Close()
	f, err := os.OpenFile(target, os.O_CREATE|os.O_EXCL|os.O_WRONLY, 0644|entry.Mode().Perm()&0111)
	if err != nil {
		return err
	}
	n, err := io.Copy(f, io.LimitReader(rc, int64(entry.UncompressedSize64)+1))
	closeErr := f.Close()
	if err != nil {
		return err
	}
	if closeErr != nil {
		return closeErr
	}
	if n != int64(entry.UncompressedSize64) {
		return errors.New("Unexpected ZIP entry size")
	}
	return nil
}

func plistValue(ctx context.Context, app, key string) (string, error) {
	b, err := exec.CommandContext(ctx, "/usr/libexec/PlistBuddy", "-c", "Print :"+key, filepath.Join(app, "Contents", "Info.plist")).Output()
	return strings.TrimSpace(string(b)), err
}

func bundleBuild(ctx context.Context, app string) (int, error) {
	b, err := plistValue(ctx, app, "CFBundleVersion")
	if err != nil {
		return 0, err
	}
	return strconv.Atoi(b)
}

func verifyReplacement(ctx context.Context, current, candidate string, build int) error {
	id, err := plistValue(ctx, candidate, "CFBundleIdentifier")
	if err != nil || id != bundleID {
		return errors.New("Update has the wrong Kai bundle identifier")
	}
	version, err := bundleBuild(ctx, candidate)
	if err != nil || version != build || version < minimumBuild {
		return errors.New("Update bundle build does not match its release")
	}
	requirement, err := designatedRequirement(ctx, current)
	if err != nil {
		return err
	}
	if err := verifySignature(ctx, candidate, requirement); err != nil {
		return err
	}
	b, err := exec.CommandContext(ctx, helper(candidate), "self-test").Output()
	if err != nil || strings.TrimSpace(string(b)) != strconv.Itoa(build) {
		return errors.New("Update does not contain a runnable, matching Kai server")
	}
	return nil
}

func designatedRequirement(ctx context.Context, app string) (string, error) {
	out, err := exec.CommandContext(ctx, "/usr/bin/codesign", "-d", "-r-", app).CombinedOutput()
	if err != nil {
		return "", errors.New("Could not read the installed Kai signing requirement")
	}
	for _, line := range strings.Split(string(out), "\n") {
		if req, ok := strings.CutPrefix(line, "designated => "); ok && (strings.Contains(req, "certificate") || strings.Contains(req, "anchor apple")) {
			return req, nil
		}
	}
	return "", errors.New("The installed Kai app needs a certificate-backed signing identity")
}

func verifySignature(ctx context.Context, candidate, requirement string) error {
	if out, err := exec.CommandContext(ctx, "/usr/bin/codesign", "--verify", "--deep", "--strict", "-R", "="+requirement, candidate).CombinedOutput(); err != nil {
		return fmt.Errorf("Kai bundle signature or publisher verification failed: %s", strings.TrimSpace(string(out)))
	}
	return nil
}

func (u *updater) activate(ctx context.Context, remote bool) error {
	prepared, err := readPrepared(u.root)
	if err != nil || prepared.Build <= u.build {
		return errors.New("No verified Kai update is ready")
	}
	data, _ := json.Marshal(activationRequest{Build: prepared.Build, Remote: remote})
	_, err = callLocal(ctx, u.root, "supervisor.sock", data)
	return err
}

func (u *updater) route(w http.ResponseWriter, r *http.Request) {
	switch r.Method {
	case http.MethodGet:
		respond(w, 200, u.status())
	case http.MethodPost:
		if err := u.activate(r.Context(), true); err != nil {
			reject(w, 409, err.Error())
			return
		}
		respond(w, 202, map[string]bool{"accepted": true})
	default:
		http.NotFound(w, r)
	}
}
