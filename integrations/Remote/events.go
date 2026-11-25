package main

import (
	"bufio"
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/coder/websocket"
	"github.com/coder/websocket/wsjson"
	"github.com/fsnotify/fsnotify"
)

type fileChanges struct {
	mu      sync.Mutex
	change  chan struct{}
	watcher *fsnotify.Watcher
	refs    map[string]int
}

func newFileChanges(ctx context.Context) (*fileChanges, error) {
	w, err := fsnotify.NewWatcher()
	if err != nil {
		return nil, err
	}
	f := &fileChanges{watcher: w, change: make(chan struct{}), refs: make(map[string]int)}
	go func() {
		defer w.Close()
		for {
			select {
			case <-ctx.Done():
				return
			case _, ok := <-w.Events:
				if !ok {
					return
				}
				f.notify()
			case _, ok := <-w.Errors:
				if !ok {
					return
				}
				f.notify()
			}
		}
	}()
	return f, nil
}

func (f *fileChanges) notify() {
	f.mu.Lock()
	defer f.mu.Unlock()
	close(f.change)
	f.change = make(chan struct{})
}

func (f *fileChanges) next() <-chan struct{} {
	f.mu.Lock()
	defer f.mu.Unlock()
	return f.change
}

func (f *fileChanges) watch(path string) func() {
	f.mu.Lock()
	if f.refs[path] == 0 {
		if f.watcher.Add(path) != nil {
			f.mu.Unlock()
			return func() {}
		}
	}
	f.refs[path]++
	f.mu.Unlock()
	return func() {
		f.mu.Lock()
		defer f.mu.Unlock()
		f.refs[path]--
		if f.refs[path] == 0 {
			delete(f.refs, path)
			_ = f.watcher.Remove(path)
		}
	}
}

type streamCursor struct {
	Owner  string `json:"owner"`
	Offset int64  `json:"offset"`
}

type streamSubscribe struct {
	Chat      string       `json:"chat,omitempty"`
	Cursor    streamCursor `json:"cursor"`
	Directory string       `json:"directory,omitempty"`
	Project   string       `json:"project,omitempty"`
}

type metadataDelta struct {
	Projects        []projectSummary `json:"projects"`
	Chats           []chatSummary    `json:"chats"`
	RemovedProjects []string         `json:"removed_projects"`
	RemovedChats    []string         `json:"removed_chats"`
	Next            int              `json:"next_offset,omitempty"`
}

func diffMetadata(old, next metadata) metadataDelta {
	delta := metadataDelta{Projects: []projectSummary{}, Chats: []chatSummary{}, RemovedProjects: []string{}, RemovedChats: []string{}, Next: next.Next}
	projects := make(map[string]projectSummary, len(old.Projects))
	for _, p := range old.Projects {
		projects[p.Path] = p
	}
	for _, p := range next.Projects {
		if projects[p.Path] != p {
			delta.Projects = append(delta.Projects, p)
		}
		delete(projects, p.Path)
	}
	for path := range projects {
		delta.RemovedProjects = append(delta.RemovedProjects, path)
	}
	chats := make(map[string]chatSummary, len(old.Chats))
	for _, c := range old.Chats {
		chats[c.ID] = c
	}
	for _, c := range next.Chats {
		if chats[c.ID] != c {
			delta.Chats = append(delta.Chats, c)
		}
		delete(chats, c.ID)
	}
	for id := range chats {
		delta.RemovedChats = append(delta.RemovedChats, id)
	}
	return delta
}

type streamFrame struct {
	Type      string            `json:"type"`
	Workspace *metadata         `json:"workspace,omitempty"`
	Delta     *metadataDelta    `json:"delta,omitempty"`
	Directory *fileDirectory    `json:"directory,omitempty"`
	Path      string            `json:"path,omitempty"`
	Activity  []liveOutput      `json:"activity,omitempty"`
	Chat      string            `json:"chat,omitempty"`
	Events    []json.RawMessage `json:"events,omitempty"`
	Cursor    *streamCursor     `json:"cursor,omitempty"`
}

func (s *service) stream(w http.ResponseWriter, r *http.Request, p peer) {
	select {
	case s.streamSlots <- struct{}{}:
		defer func() { <-s.streamSlots }()
	default:
		reject(w, 503, "Kai stream capacity is temporarily full")
		return
	}
	c, err := websocket.Accept(w, r, &websocket.AcceptOptions{CompressionMode: websocket.CompressionDisabled})
	if err != nil {
		return
	}
	defer c.CloseNow()
	c.SetReadLimit(4096)
	ctx, cancel := context.WithCancel(r.Context())
	defer cancel()
	subs := make(chan streamSubscribe, 1)
	go func() {
		defer cancel()
		for {
			var req streamSubscribe
			if wsjson.Read(ctx, c, &req) != nil {
				return
			}
			if len(req.Chat) > 256 || req.Cursor.Offset < 0 || (req.Cursor.Owner != "" && !validDevice.MatchString(req.Cursor.Owner)) {
				_ = c.Close(websocket.StatusPolicyViolation, "Invalid stream subscription")
				return
			}
			select {
			case subs <- req:
			case <-ctx.Done():
				return
			}
		}
	}()
	var req streamSubscribe
	select {
	case req = <-subs:
	case <-ctx.Done():
		return
	case <-time.After(5 * time.Second):
		return
	}
	var lastMetadata, lastActivity []byte
	var lastHistory string
	var previous metadata
	var relative, lastRevision, directoryWatch string
	watched := map[string]func(){}
	defer func() {
		for _, unwatch := range watched {
			unwatch()
		}
	}()
	ticker := time.NewTicker(12 * time.Second)
	defer ticker.Stop()
	writer := func(frame streamFrame) error {
		writeCtx, cancel := context.WithTimeout(ctx, 4*time.Second)
		defer cancel()
		return wsjson.Write(writeCtx, c, frame)
	}
	for {
		next := s.changes.next()
		if !s.state.isAllowed(p.Node, p.Device) {
			_ = c.Close(websocket.StatusPolicyViolation, "Kai access revoked")
			return
		}
		metadataSent := false
		if value, err := s.history.browse(ctx, "", 0); err == nil {
			b, _ := json.Marshal(value)
			if !bytes.Equal(lastMetadata, b) {
				frame := streamFrame{Type: "metadata", Workspace: &value}
				if lastMetadata != nil {
					delta := diffMetadata(previous, value)
					frame = streamFrame{Type: "metadata_delta", Delta: &delta}
				}
				if writer(frame) != nil {
					return
				}
				previous = value
				lastMetadata = b
				metadataSent = true
			}
		}
		if req.Chat != "" {
			stamp := req.Chat + ":" + s.history.revision()
			if lastHistory != "" && stamp != lastHistory && !metadataSent {
				// Existing clients treat this frame as an invalidation and request
				// the bounded canonical page for their selected chat.
				if writer(streamFrame{Type: "metadata_delta"}) != nil {
					return
				}
			}
			lastHistory = stamp
		}
		watchSet := map[string]bool{}
		if active, err := s.history.activity(""); err == nil {
			for _, item := range active {
				if item.Owner != "" {
					watchSet[item.Owner] = true
				}
			}
			b, _ := json.Marshal(active)
			if !bytes.Equal(lastActivity, b) {
				if writer(streamFrame{Type: "activity", Activity: active}) != nil {
					return
				}
				lastActivity = b
			}
		}
		owner := s.journalOwner(ctx, req.Chat)
		if owner != "" {
			watchSet[owner] = true
		}
		if relative != req.Directory {
			relative, lastRevision = req.Directory, ""
			directoryWatch = ""
		}
		if req.Directory != "" && relativeFile(req.Directory) {
			directoryWatch = ""
			grant := s.state.snapshot().Trusted[p.Node]
			project := req.Project
			if project == "" {
				project = grant.FileProject
			}
			if s.state.fileAllowed(p.Node, p.Device, project) && s.history.allowsProject(ctx, project) {
				if canonical, err := filepath.EvalSymlinks(filepath.Join(project, req.Directory)); err == nil &&
					(canonical == project || filepath.Dir(canonical) != "." && filepath.IsAbs(canonical) &&
						strings.HasPrefix(canonical, project+string(filepath.Separator))) {
					directoryWatch = canonical
					root, err := os.OpenRoot(project)
					if err == nil {
						value, err := directoryEntries(root, req.Directory, "")
						root.Close()
						if err == nil && value.Revision != lastRevision {
							if writer(streamFrame{Type: "directory", Directory: &value, Path: req.Directory}) != nil {
								return
							}
							lastRevision = value.Revision
						}
					}
				}
			}
		}
		if directoryWatch != "" {
			watchSet[directoryWatch] = true
		}
		for path, unwatch := range watched {
			if !watchSet[path] {
				unwatch()
				delete(watched, path)
			}
		}
		for path := range watchSet {
			if watched[path] == nil {
				watched[path] = s.changes.watch(path)
			}
		}
		if owner != "" {
			caughtUp, err := s.pushJournal(ctx, owner, &req, writer)
			if err != nil {
				return
			}
			if !caughtUp {
				continue
			}
		}
		select {
		case <-ctx.Done():
			return
		case <-next:
			// Coalesce bursts of writes without dropping journal entries.
			select {
			case <-ctx.Done():
				return
			case <-time.After(45 * time.Millisecond):
			}
		case req = <-subs:
		case <-ticker.C:
			pingCtx, stop := context.WithTimeout(ctx, 3*time.Second)
			err := c.Ping(pingCtx)
			stop()
			if err != nil {
				return
			}
		}
	}
}

func (w workspaceHistory) revision() string {
	var stamp strings.Builder
	for _, name := range []string{"history.sqlite", "history.sqlite-wal"} {
		if info, err := os.Stat(filepath.Join(w.root, name)); err == nil {
			fmt.Fprintf(&stamp, "%s:%d:%d;", name, info.ModTime().UnixNano(), info.Size())
		}
	}
	return stamp.String()
}

func (s *service) pushJournal(ctx context.Context, owner string, req *streamSubscribe, write func(streamFrame) error) (bool, error) {
	root, err := os.OpenRoot(filepath.Join(s.history.root, "session-owners"))
	if err != nil {
		return true, nil
	}
	defer root.Close()
	id := filepath.Base(owner)
	f, err := root.Open(filepath.Join(id, "events.jsonl"))
	if err != nil {
		return true, nil
	}
	defer f.Close()
	info, err := f.Stat()
	if err != nil || !info.Mode().IsRegular() {
		return true, nil
	}
	if req.Cursor.Owner != id || req.Cursor.Offset > info.Size() {
		req.Cursor = streamCursor{Owner: id}
		if err := write(streamFrame{Type: "reset", Chat: req.Chat, Cursor: &req.Cursor}); err != nil {
			return true, err
		}
	}
	if _, err := f.Seek(req.Cursor.Offset, io.SeekStart); err != nil {
		return true, nil
	}
	reader := bufio.NewReaderSize(f, 256<<10)
	var batch []json.RawMessage
	remaining := 512 << 10
	caughtUp := true
	for i := 0; i < 64 && remaining > 0; i++ {
		if ctx.Err() != nil {
			return true, ctx.Err()
		}
		data, consumed, err := journalLine(reader)
		if err != nil {
			break
		}
		req.Cursor.Offset += consumed
		remaining -= int(consumed)
		if len(data) > 0 && json.Valid(data) {
			batch = append(batch, json.RawMessage(data))
		}
		caughtUp = req.Cursor.Offset >= info.Size()
		if caughtUp {
			break
		}
	}
	if len(batch) > 0 {
		if err := write(streamFrame{Type: "events", Chat: req.Chat, Events: batch, Cursor: &req.Cursor}); err != nil {
			return true, err
		}
	}
	return caughtUp, nil
}

func journalLine(r *bufio.Reader) ([]byte, int64, error) {
	b, err := r.ReadSlice('\n')
	n := int64(len(b))
	if !errors.Is(err, bufio.ErrBufferFull) {
		return bytes.Clone(b), n, err
	}
	for errors.Is(err, bufio.ErrBufferFull) {
		b, err = r.ReadSlice('\n')
		n += int64(len(b))
	}
	return nil, n, err
}

// Record only links established by the authoritative workspace. A client
// cursor can never use a guessed owner ID to read another conversation.
func (s *service) journalOwner(ctx context.Context, chat string) string {
	if chat == "" || !s.history.hasChat(ctx, chat) {
		return ""
	}
	owner := s.history.ownerForChat(chat)
	directory := filepath.Join(s.root, "journal-links")
	name := filepath.Join(directory, fmt.Sprintf("%x.json", sha256.Sum256([]byte(chat))))
	var recorded string
	if b, err := os.ReadFile(name); err == nil && len(b) < 1024 {
		_ = json.Unmarshal(b, &recorded)
	}
	if owner != "" {
		if owner != recorded && privateDir(directory) == nil {
			_ = saveJSON(name, owner)
		}
		return owner
	}
	owners := filepath.Join(s.history.root, "session-owners")
	if filepath.Dir(recorded) == owners && validDevice.MatchString(filepath.Base(recorded)) {
		return recorded
	}
	return ""
}

func (w workspaceHistory) ownerForChat(chat string) string {
	f, err := os.Open(filepath.Join(w.root, "workspace.json"))
	if err != nil {
		return ""
	}
	defer f.Close()
	var value struct {
		Continuations []struct {
			Session struct {
				ID string `json:"id"`
			} `json:"session"`
			Durable string `json:"durable"`
		} `json:"continuations"`
	}
	if json.NewDecoder(io.LimitReader(f, 8<<20)).Decode(&value) != nil {
		return ""
	}
	owners := filepath.Join(w.root, "session-owners")
	for _, entry := range value.Continuations {
		if entry.Session.ID == chat && filepath.Dir(entry.Durable) == owners && validDevice.MatchString(filepath.Base(entry.Durable)) {
			return entry.Durable
		}
	}
	return ""
}
