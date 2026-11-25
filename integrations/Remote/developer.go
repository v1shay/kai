package main

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
)

const sourceRepository = "https://github.com/v1shay/kai.git"

var sourceBranch = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9_/-]{0,100}$`)
var commitHash = regexp.MustCompile(`^[a-f0-9]{40,64}$`)

type updateConfig struct {
	Channel         string `json:"channel"`
	Branch          string `json:"branch,omitempty"`
	SigningIdentity string `json:"signing_identity,omitempty"`
}

func (u *updater) configuration() (updateConfig, error) {
	var c updateConfig
	b, err := os.ReadFile(filepath.Join(u.root, "updates.json"))
	if errors.Is(err, os.ErrNotExist) {
		return updateConfig{Channel: "release"}, nil
	}
	if err != nil {
		return c, err
	}
	if err := json.Unmarshal(b, &c); err != nil {
		return c, err
	}
	switch c.Channel {
	case "release", "off":
		return c, nil
	case "developer":
		if !sourceBranch.MatchString(c.Branch) {
			return c, errors.New("Developer updates require an explicitly configured GitHub branch")
		}
		if c.SigningIdentity != "" && !regexp.MustCompile(`^[0-9A-Fa-f]{40}$`).MatchString(c.SigningIdentity) {
			return c, errors.New("Developer build signing_identity must be a 40-character certificate SHA-1")
		}
		return c, nil
	default:
		return c, errors.New("Unknown Kai update channel")
	}
}

func (u *updater) prepareDeveloper(ctx context.Context, c updateConfig) (updateStatus, error) {
	signers := filepath.Join(u.root, "developer-allowed-signers")
	info, err := os.Lstat(signers)
	if err != nil || !info.Mode().IsRegular() || info.Mode().Perm()&0022 != 0 {
		return updateStatus{}, errors.New("Developer updates need a locally trusted SSH allowed-signers file")
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
	repo := filepath.Join(stage, "source")
	if err := os.Mkdir(repo, 0700); err != nil {
		return updateStatus{}, err
	}
	buildHome, err := os.UserHomeDir()
	if err != nil {
		return updateStatus{}, err
	}
	path := strings.Join([]string{filepath.Join(buildHome, ".cargo/bin"), "/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin", "/usr/sbin", "/sbin"}, ":")
	env := []string{"HOME=" + buildHome, "PATH=" + path, "GIT_CONFIG_GLOBAL=/dev/null", "GIT_CONFIG_NOSYSTEM=1",
		"GIT_TERMINAL_PROMPT=0", "TMPDIR=" + os.TempDir()}
	git := func(args ...string) (string, error) {
		args = append([]string{"-c", "core.hooksPath=/dev/null"}, args...)
		cmd := exec.CommandContext(ctx, "/usr/bin/git", args...)
		cmd.Dir = repo
		cmd.Env = env
		b, err := cmd.Output()
		return strings.TrimSpace(string(b)), err
	}
	if _, err := git("init", "--quiet"); err != nil {
		return updateStatus{}, errors.New("Could not initialize the isolated source build")
	}
	if _, err := git("fetch", "--quiet", "--depth=1", "--no-tags", sourceRepository, "refs/heads/"+c.Branch); err != nil {
		return updateStatus{}, errors.New("Could not fetch the configured Kai GitHub branch")
	}
	commit, err := git("rev-parse", "--verify", "FETCH_HEAD^{commit}")
	if err != nil || !commitHash.MatchString(commit) {
		return updateStatus{}, errors.New("GitHub did not return a valid Kai commit")
	}
	old, _ := plistValue(ctx, u.app, "KaiSourceCommit")
	if old == commit {
		return updateStatus{State: "idle", Message: "Kai is already on the latest developer commit."}, nil
	}
	if _, err := git("-c", "gpg.format=ssh", "-c", "gpg.ssh.allowedSignersFile="+signers, "verify-commit", commit); err != nil {
		return updateStatus{}, errors.New("GitHub commit is unsigned or signed by a source signer this Mac does not trust")
	}
	if _, err := git("checkout", "--quiet", "--detach", commit); err != nil {
		return updateStatus{}, errors.New("Could not check out the verified Kai commit")
	}
	build := u.build + 1
	cmd := exec.CommandContext(ctx, "/bin/zsh", filepath.Join(repo, "scripts", "build-unified-app.sh"))
	cmd.Dir = repo
	cmd.Env = append(env, "KAI_BUILD_NUMBER="+strconv.Itoa(build), "KAI_DISPLAY_VERSION=0.12."+strconv.Itoa(build),
		"KAI_SOURCE_COMMIT="+commit, "CARGO_BUILD_JOBS=3")
	if c.SigningIdentity != "" {
		cmd.Env = append(cmd.Env, "KAI_SIGNING_IDENTITY="+c.SigningIdentity)
	}
	cmd.Stdout = io.Discard
	cmd.Stderr = io.Discard
	if err := cmd.Run(); err != nil {
		return updateStatus{}, fmt.Errorf("Verified source %s did not build. Check Go, Rust, Xcode, cached dependencies, and signing setup.", commit[:12])
	}
	candidate := filepath.Join(stage, "Kai Unified.app")
	if err := os.Rename(filepath.Join(repo, ".build", "unified", "Kai Unified.app"), candidate); err != nil {
		return updateStatus{}, err
	}
	if err := verifyReplacement(ctx, u.app, candidate, build); err != nil {
		return updateStatus{}, err
	}
	builtCommit, _ := plistValue(ctx, candidate, "KaiSourceCommit")
	if builtCommit != commit {
		return updateStatus{}, errors.New("The built Kai bundle does not record its verified GitHub commit")
	}
	if err := os.RemoveAll(repo); err != nil {
		return updateStatus{}, err
	}
	if err := saveJSON(filepath.Join(u.root, "prepared.json"), preparedUpdate{App: candidate, Build: build}); err != nil {
		return updateStatus{}, err
	}
	keep = true
	return updateStatus{State: "ready", Build: build}, nil
}
