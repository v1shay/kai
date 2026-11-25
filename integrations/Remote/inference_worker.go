package main

import (
	"context"
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"sync"
	"time"
)

type inferenceWorker struct {
	mu        sync.Mutex
	pipe      io.WriteCloser
	client    *http.Client
	exited    chan struct{}
	directory string
}

func (worker *inferenceWorker) connection(app, root string) (*http.Client, error) {
	worker.mu.Lock()
	defer worker.mu.Unlock()
	if worker.exited != nil {
		select {
		case <-worker.exited:
			worker.remove()
		default:
			return worker.client, nil
		}
	}
	if app == "" {
		return nil, fmt.Errorf("Kai inference needs an installed app bundle")
	}
	parent := socketDir(root)
	if err := privateDir(parent); err != nil {
		return nil, err
	}
	directory, err := os.MkdirTemp(parent, "infer-")
	if err != nil {
		return nil, err
	}
	if err := worker.launch(app, directory); err != nil {
		_ = os.RemoveAll(directory)
		return nil, err
	}
	return worker.client, nil
}

func (worker *inferenceWorker) launch(app, directory string) error {
	path := filepath.Join(directory, "engine.sock")
	listener, err := net.ListenUnix("unix", &net.UnixAddr{Name: path, Net: "unix"})
	if err != nil {
		return err
	}
	listener.SetUnlinkOnClose(false)
	defer listener.Close()
	if err := os.Chmod(path, 0600); err != nil {
		return err
	}
	file, err := listener.File()
	if err != nil {
		return err
	}
	defer file.Close()
	script := filepath.Join(app, "Contents", "Resources", "kai_ninja_host.py")
	command := exec.Command("/usr/bin/python3", script, "--listener-fd", "3")
	command.ExtraFiles = []*os.File{file}
	command.Stderr = os.Stderr
	pipe, err := command.StdinPipe()
	if err != nil {
		return err
	}
	if err := command.Start(); err != nil {
		_ = pipe.Close()
		return err
	}
	exited := make(chan struct{})
	go func() {
		_ = command.Wait()
		_ = os.RemoveAll(directory)
		close(exited)
	}()
	client := &http.Client{
		Transport: &http.Transport{
			DisableKeepAlives: true,
			DialContext: func(ctx context.Context, _, _ string) (net.Conn, error) {
				return (&net.Dialer{Timeout: 3 * time.Second}).DialContext(ctx, "unix", path)
			},
		},
		CheckRedirect: inferenceClient.CheckRedirect,
	}
	worker.pipe, worker.exited, worker.client, worker.directory = pipe, exited, client, directory
	return nil
}

func (worker *inferenceWorker) remove() {
	_ = worker.pipe.Close()
	worker.client.CloseIdleConnections()
	_ = os.RemoveAll(worker.directory)
	worker.pipe, worker.exited, worker.client, worker.directory = nil, nil, nil, ""
}

func (worker *inferenceWorker) stop() {
	worker.mu.Lock()
	defer worker.mu.Unlock()
	if worker.exited == nil {
		return
	}
	_ = worker.pipe.Close()
	select {
	case <-worker.exited:
		worker.remove()
	case <-time.After(3 * time.Second):
		// The inherited socket directory is removed when the child finishes.
	}
}
