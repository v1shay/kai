package main

import (
	"context"
	"errors"
	"fmt"
	"io"
	"os"
	"os/signal"
	"path/filepath"
	"runtime"
	"syscall"
	"time"

	"tailscale.com/envknob"
)

const serviceLabel = "com.kai.unified.server"

func bundledApp() (string, error) {
	executable, err := os.Executable()
	if err != nil {
		return "", err
	}
	app := filepath.Dir(filepath.Dir(filepath.Dir(executable)))
	if filepath.Ext(app) != ".app" {
		return "", errors.New("Kai Server must be started from a packaged Kai app")
	}
	return filepath.EvalSymlinks(app)
}

func locations() (string, string, error) {
	home, err := os.UserHomeDir()
	if err != nil {
		return "", "", err
	}
	data := filepath.Join(home, "Library", "Application Support", "Kai Unified")
	return filepath.Join(data, "server"), data, nil
}

func run() error {
	if len(os.Args) < 2 {
		return errors.New("Usage: kai-server bootstrap | call | serve ROOT DATA APP | supervise ROOT DATA APP")
	}
	cmd := os.Args[1]
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	if cmd == "self-test" {
		fmt.Println(appBuild)
		return nil
	}
	if cmd == "pty-owner" {
		if len(os.Args) != 4 {
			return errors.New("Missing terminal owner paths")
		}
		return serveTerminal(ctx, os.Args[2], os.Args[3])
	}
	if cmd == "serve" || cmd == "supervise" {
		if len(os.Args) != 5 {
			return errors.New("Missing Kai service paths")
		}
		root, data, app := os.Args[2], os.Args[3], os.Args[4]
		if cmd == "serve" {
			return serve(ctx, root, data, app)
		}
		return supervise(ctx, root, data, app)
	}
	root, data, err := locations()
	if err != nil {
		return err
	}
	switch cmd {
	case "bootstrap":
		app, err := bundledApp()
		if err != nil {
			return err
		}
		return bootstrap(ctx, root, data, app)
	case "terminal":
		if len(os.Args) != 5 {
			return errors.New("Expected the remote node, device, and terminal IDs")
		}
		return runTerminalClient(ctx, root, os.Args[2], os.Args[3], os.Args[4])
	case "stream":
		b, err := io.ReadAll(io.LimitReader(os.Stdin, 8193))
		if err != nil {
			return err
		}
		if len(b) > 8192 {
			return errors.New("Kai stream request is too large")
		}
		return streamLocal(ctx, root, b, os.Stdout)
	case "call":
		ctx, cancel := context.WithTimeout(ctx, 60*time.Second)
		defer cancel()
		b, err := io.ReadAll(io.LimitReader(os.Stdin, maxLocalRequest+1))
		if err != nil {
			return err
		}
		if len(b) > maxLocalRequest {
			return errors.New("Kai request is too large")
		}
		b, err = callLocal(ctx, root, "server.sock", b)
		if err != nil {
			return err
		}
		_, err = os.Stdout.Write(b)
		return err
	default:
		return errors.New("Unknown Kai Server operation")
	}
}

func main() {
	if runtime.GOOS == "darwin" && len(os.Args) > 1 && os.Args[1] == "serve" {
		const controlHTTPS = "TS_FORCE_NOISE_443"
		if _, configured := os.LookupEnv(controlHTTPS); !configured {
			// macOS web filtering can drop control traffic after upgrading an HTTP connection.
			envknob.Setenv(controlHTTPS, "true")
		}
	}
	if err := run(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
