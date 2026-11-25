package main

import (
	"context"
	"crypto/sha256"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"slices"
	"strings"
	"sync"
	"syscall"
	"time"

	"github.com/coder/websocket"
	"github.com/creack/pty"
)

var terminalID = regexp.MustCompile(`^[a-f0-9]{32}$`)
var cliSessionID = regexp.MustCompile(`^[a-zA-Z0-9_.:][a-zA-Z0-9_.:-]{0,255}$`)

var cliResumeFlags = map[string]string{
	"codex": "resume", "claude": "--resume", "gemini": "--resume",
	"opencode": "--session", "agy": "--conversation", "cursor-agent": "--resume", "grok": "--resume",
}

const terminalRetain = 1 << 20
const maxLiveTerminals = 128

type terminalSession struct {
	ID           string    `json:"id"`
	Project      string    `json:"project"`
	Program      string    `json:"program"`
	Started      time.Time `json:"started"`
	Resume       string    `json:"resume,omitempty"`
	ResumeSocket string    `json:"resume_socket,omitempty"`
}

type terminalIO struct {
	Op    string `json:"op"`
	Input []byte `json:"input,omitempty"`
	After int64  `json:"after,omitempty"`
	Rows  uint16 `json:"rows,omitempty"`
	Cols  uint16 `json:"cols,omitempty"`
}

type terminalOutput struct {
	Output []byte `json:"output,omitempty"`
	Next   int64  `json:"next"`
	Done   bool   `json:"done"`
}

func terminalSocket(id string) string { return "pty-" + id + ".sock" }

func terminalRecord(root, id string) (terminalSession, error) {
	var value terminalSession
	if !terminalID.MatchString(id) {
		return value, errors.New("Invalid terminal identifier")
	}
	b, err := os.ReadFile(filepath.Join(root, "terminals", id, "terminal.json"))
	if err != nil {
		return value, err
	}
	err = json.Unmarshal(b, &value)
	return value, err
}

func (s *service) beginTerminal(p peer, requestID, program string) (terminalSession, error) {
	return s.beginTerminalSession(p, requestID, program, "", "")
}

func (s *service) beginTerminalSession(p peer, requestID, program, resume, socket string) (terminalSession, error) {
	var record terminalSession
	if !p.Control || p.FileProject == "" {
		return record, errors.New("Choose a project from a paired device with control access")
	}
	_, cli := cliResumeFlags[program]
	if program != "shell" && !cli && !(program == "tmux" && terminalID.MatchString(resume) && socket != "") {
		return record, errors.New("This terminal program is unavailable")
	}
	if cli && resume != "" && !cliSessionID.MatchString(resume) {
		return record, errors.New("The chat's native session ID is invalid")
	}
	if len(requestID) < 1 || len(requestID) > 128 {
		return record, errors.New("A stable terminal request ID is required")
	}
	sum := sha256.Sum256([]byte(p.Node + "\x00" + p.Device + "\x00" + requestID))
	id := fmt.Sprintf("%x", sum[:16])
	dir := filepath.Join(s.root, "terminals", id)
	s.terminalLaunch.Lock()
	defer s.terminalLaunch.Unlock()
	if cli && resume != "" {
		for _, existing := range listTerminals(s.root, p.FileProject, false) {
			if existing.Program != program || existing.Resume != resume {
				continue
			}
			existingDir := filepath.Join(s.root, "terminals", existing.ID)
			_, finished := os.Stat(filepath.Join(existingDir, "scrollback"))
			_, failed := os.Stat(filepath.Join(existingDir, "exit.json"))
			socket, err := os.Stat(filepath.Join(socketDir(s.root), terminalSocket(existing.ID)))
			if errors.Is(finished, os.ErrNotExist) && errors.Is(failed, os.ErrNotExist) &&
				(time.Since(existing.Started) < 7*time.Second || err == nil && socket.Mode()&os.ModeSocket != 0) {
				return existing, nil
			}
		}
	}
	record, err := terminalRecord(s.root, id)
	if err == nil {
		if record.Program != program || record.Project != p.FileProject || record.Resume != resume {
			return terminalSession{}, errors.New("This terminal request ID was already used")
		}
		return record, nil
	}
	if !errors.Is(err, os.ErrNotExist) {
		return record, err
	}
	if liveTerminalCount(s.root, time.Now()) >= maxLiveTerminals {
		return record, errors.New("Too many active Kai terminals")
	}
	executable := helper(s.app)
	if _, err := os.Stat(executable); err != nil {
		return record, errors.New("Kai terminal owner executable is unavailable")
	}
	if err := privateDir(dir); err != nil {
		return record, err
	}
	record = terminalSession{ID: id, Project: p.FileProject, Program: program, Started: time.Now(), Resume: resume, ResumeSocket: socket}
	if err := saveJSON(filepath.Join(dir, "terminal.json"), record); err != nil {
		return record, err
	}
	cmd := exec.Command(executable, "pty-owner", s.root, id)
	cmd.SysProcAttr = &syscall.SysProcAttr{Setsid: true}
	if err := cmd.Start(); err != nil {
		_ = saveJSON(filepath.Join(dir, "exit.json"), map[string]string{"error": err.Error()})
		return terminalSession{}, err
	}
	go func() { _ = cmd.Wait() }()
	return record, nil
}

func liveTerminalCount(root string, now time.Time) int {
	toplevel := filepath.Join(root, "terminals")
	children, _ := os.ReadDir(toplevel)
	count := 0
	for _, child := range children {
		if !child.IsDir() {
			continue
		}
		session, err := terminalRecord(root, child.Name())
		if err != nil {
			continue
		}
		dir := filepath.Join(toplevel, child.Name())
		if _, err := os.Stat(filepath.Join(dir, "scrollback")); err == nil {
			continue
		}
		if _, err := os.Stat(filepath.Join(dir, "exit.json")); err == nil {
			continue
		}
		owner := filepath.Join(dir, "owner.lock")
		if _, err := os.Stat(owner); err == nil {
			lock, err := lockFile(owner)
			if errors.Is(err, syscall.EWOULDBLOCK) {
				count++
				continue
			}
			if err == nil {
				_ = lock.Close()
			}
		}
		// The durable record reserves a slot before the child has taken its lock.
		if now.Sub(session.Started) < 10*time.Second {
			count++
		}
	}
	return count
}

func listTerminals(root, project string, activeOnly bool) []terminalSession {
	list := []terminalSession{}
	children, _ := os.ReadDir(filepath.Join(root, "terminals"))
	for _, dir := range children {
		record, err := terminalRecord(root, dir.Name())
		if err != nil || record.Project != project {
			continue
		}
		if activeOnly {
			if _, err := os.Stat(filepath.Join(root, "terminals", record.ID, "scrollback")); err == nil {
				continue
			}
			socket, err := os.Stat(filepath.Join(socketDir(root), terminalSocket(record.ID)))
			if err != nil || socket.Mode()&os.ModeSocket == 0 {
				continue
			}
		}
		list = append(list, record)
	}
	slices.SortFunc(list, func(a, b terminalSession) int { return b.Started.Compare(a.Started) })
	return list
}

type terminalBuffer struct {
	mu      sync.Mutex
	data    []byte
	cursor  int64
	done    bool
	changed chan struct{}
}

func (b *terminalBuffer) append(data []byte, done bool) {
	b.mu.Lock()
	defer b.mu.Unlock()
	b.data = append(b.data, data...)
	if over := len(b.data) - terminalRetain; over > 0 {
		copy(b.data, b.data[over:])
		b.data = b.data[:terminalRetain]
		b.cursor += int64(over)
	}
	b.done = done
	close(b.changed)
	b.changed = make(chan struct{})
}
func (b *terminalBuffer) read(ctx context.Context, after int64) terminalOutput {
	timer := time.NewTimer(time.Second)
	defer timer.Stop()
	for {
		b.mu.Lock()
		if after < b.cursor {
			after = b.cursor
		}
		i := min(after-b.cursor, int64(len(b.data)))
		if i < int64(len(b.data)) || b.done {
			end := min(i+48_000, int64(len(b.data)))
			result := terminalOutput{Output: append([]byte(nil), b.data[i:end]...), Next: b.cursor + end, Done: b.done && end == int64(len(b.data))}
			b.mu.Unlock()
			return result
		}
		wait := b.changed
		b.mu.Unlock()
		select {
		case <-ctx.Done():
			return terminalOutput{Next: after}
		case <-timer.C:
			return terminalOutput{Next: after}
		case <-wait:
		}
	}
}

func terminalProgramDirs() []string {
	home, _ := os.UserHomeDir()
	return []string{filepath.Join(home, ".local", "bin"), "/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin", "/usr/sbin", "/sbin"}
}

func terminalEnvironment(program string) []string {
	env := os.Environ()
	if _, cli := cliResumeFlags[program]; cli {
		env = slices.DeleteFunc(env, func(entry string) bool { return strings.HasPrefix(entry, "PATH=") })
		env = append(env, "PATH="+strings.Join(terminalProgramDirs(), string(os.PathListSeparator)))
	}
	return append(env, "TERM=xterm-256color", "COLORTERM=truecolor")
}

func ownerProgram(program string) (string, error) {
	if program == "shell" {
		return "/bin/zsh", nil
	}
	if program == "codex" {
		path := "/Applications/Codex.app/Contents/Resources/codex"
		if info, err := os.Stat(path); err == nil && info.Mode().IsRegular() && info.Mode().Perm()&0111 != 0 {
			return path, nil
		}
	}
	for _, dir := range terminalProgramDirs() {
		path := filepath.Join(dir, program)
		if info, err := os.Stat(path); err == nil && info.Mode().IsRegular() && info.Mode().Perm()&0111 != 0 {
			return path, nil
		}
	}
	return "", fmt.Errorf("%s CLI is not installed on the host", program)
}

func serveTerminal(ctx context.Context, root, id string) (failure error) {
	record, err := terminalRecord(root, id)
	if err != nil {
		return err
	}
	dir := filepath.Join(root, "terminals", id)
	lock, err := lockFile(filepath.Join(dir, "owner.lock"))
	if err != nil {
		return err
	}
	defer lock.Close()
	defer func() {
		if failure != nil {
			_ = saveJSON(filepath.Join(dir, "exit.json"), map[string]string{"error": failure.Error()})
		}
	}()
	buf := terminalBuffer{changed: make(chan struct{})}
	path, err := ownerProgram(record.Program)
	if err != nil {
		buf.append([]byte(err.Error()+"\r\n"), true)
	}
	canonical, realErr := filepath.EvalSymlinks(record.Project)
	if realErr != nil || canonical != record.Project {
		return errors.New("Terminal project directory changed")
	}
	var terminal *os.File
	if err == nil {
		cmd := exec.Command(path)
		if record.Program == "shell" {
			cmd.Args = append(cmd.Args, "-l")
		}
		if record.Program == "tmux" {
			if !terminalID.MatchString(record.Resume) || !regexp.MustCompile(`^kai-[a-f0-9]{24}$`).MatchString(record.ResumeSocket) {
				return errors.New("Invalid saved Kai terminal")
			}
			cmd.Args = append(cmd.Args, "-L", record.ResumeSocket, "-f", "/dev/null", "attach-session", "-t", "="+record.Resume)
		}
		if flag, ok := cliResumeFlags[record.Program]; ok && record.Resume != "" {
			if !cliSessionID.MatchString(record.Resume) {
				return errors.New("Invalid saved CLI session")
			}
			cmd.Args = append(cmd.Args, flag, record.Resume)
		}
		cmd.Dir = record.Project
		cmd.Env = terminalEnvironment(record.Program)
		terminal, err = pty.StartWithSize(cmd, &pty.Winsize{Rows: 24, Cols: 100})
		if err != nil {
			buf.append([]byte(err.Error()+"\r\n"), true)
		}
		if terminal != nil {
			defer func() {
				_ = terminal.Close()
				_ = cmd.Process.Kill()
				_ = cmd.Wait()
			}()
			go func() {
				chunk := make([]byte, 32768)
				for {
					n, err := terminal.Read(chunk)
					if n > 0 {
						buf.append(chunk[:n], false)
					}
					if err != nil {
						break
					}
				}
				buf.append(nil, true)
				buf.mu.Lock()
				data := append([]byte(nil), buf.data...)
				buf.mu.Unlock()
				_ = os.WriteFile(filepath.Join(dir, "scrollback"), data, 0600)
			}()
		}
	}
	l, err := listenLocal(root, terminalSocket(id))
	if err != nil {
		return err
	}
	server := httpServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var value terminalIO
		if r.Method != http.MethodPost || r.Host != "kai.local" || json.NewDecoder(http.MaxBytesReader(w, r.Body, 48_000)).Decode(&value) != nil {
			reject(w, 400, "Invalid terminal operation")
			return
		}
		switch value.Op {
		case "read":
			respond(w, 200, buf.read(r.Context(), max(value.After, 0)))
		case "input":
			if terminal == nil || len(value.Input) > 16000 {
				reject(w, 409, "Remote terminal is closed")
				return
			}
			if _, err := terminal.Write(value.Input); err != nil {
				reject(w, 409, "Remote terminal is closed")
				return
			}
			respond(w, 200, map[string]bool{"ok": true})
		case "resize":
			if terminal == nil || value.Rows == 0 || value.Rows > 1000 || value.Cols == 0 || value.Cols > 1000 {
				reject(w, 400, "Invalid terminal size")
				return
			}
			if err := pty.Setsize(terminal, &pty.Winsize{Rows: value.Rows, Cols: value.Cols}); err != nil {
				reject(w, 409, "Host terminal resize failed")
				return
			}
			respond(w, 200, map[string]bool{"ok": true})
		default:
			reject(w, 400, "Unknown terminal operation")
		}
	}))
	go func() {
		for {
			buf.mu.Lock()
			done, changed := buf.done, buf.changed
			buf.mu.Unlock()
			if done {
				select {
				case <-ctx.Done():
				case <-time.After(5 * time.Second):
				}
				_ = server.Close()
				return
			}
			select {
			case <-ctx.Done():
				_ = server.Close()
				return
			case <-changed:
			}
		}
	}()

	err = server.Serve(l)
	if errors.Is(err, http.ErrServerClosed) {
		return nil
	}
	return err
}

func (s *service) terminalEndpoint(w http.ResponseWriter, r *http.Request, p peer, endpoint string) {
	project := r.URL.Query().Get("project")
	if endpoint == "terminal" {
		record, err := terminalRecord(s.root, r.URL.Query().Get("id"))
		if err != nil {
			reject(w, 404, "Remote terminal unavailable")
			return
		}
		project = record.Project
	} else if project == "" {
		project = p.FileProject
	}
	if !s.state.terminalAllowed(p.Node, p.Device, project) || !s.history.allowsProject(r.Context(), project) {
		reject(w, 403, "Terminal access requires a registered project and a paired device with control")
		return
	}
	p.FileProject = project
	switch {
	case r.Method == http.MethodGet && endpoint == "terminals":
		respond(w, 200, listTerminals(s.root, p.FileProject, r.URL.Query().Get("active") == "true"))
	case r.Method == http.MethodPost && endpoint == "terminals":
		var request struct {
			Request     string `json:"request_id"`
			Program     string `json:"program"`
			RestoreChat string `json:"restore_chat"`
			CLIChat     string `json:"cli_chat"`
		}
		if json.NewDecoder(http.MaxBytesReader(w, r.Body, 4096)).Decode(&request) != nil {
			reject(w, 400, "Invalid terminal request")
			return
		}
		var record terminalSession
		var err error
		switch {
		case request.CLIChat != "":
			record, err = s.beginChatCLI(r.Context(), p, request.CLIChat)
		case request.RestoreChat == "":
			record, err = s.beginTerminal(p, request.Request, request.Program)
		default:
			var resume string
			resume, err = s.savedTerminal(r.Context(), request.RestoreChat, project)
			if err == nil {
				digest := sha256.Sum256([]byte(s.history.root))
				socket := fmt.Sprintf("kai-%x", digest[:12])
				record, err = s.beginTerminalSession(p, "restore-"+resume, "tmux", resume, socket)
			}
		}
		if err != nil {
			reject(w, 409, err.Error())
			return
		}
		respond(w, 200, record)
	case r.Method == http.MethodGet && endpoint == "terminal":
		id := r.URL.Query().Get("id")
		s.serveTerminalStream(w, r, p, id)
	default:
		http.NotFound(w, r)
	}
}

func (s *service) beginChatCLI(ctx context.Context, p peer, chat string) (terminalSession, error) {
	if len(chat) > 256 || !s.history.hasChat(ctx, chat) {
		return terminalSession{}, errors.New("This chat is unavailable")
	}
	db, err := s.history.database()
	if err != nil {
		return terminalSession{}, err
	}
	defer db.Close()
	var program, resume string
	err = db.QueryRowContext(ctx, `SELECT agent, COALESCE(external_id,'') FROM sessions
		WHERE id=? AND project_path=? AND `+visibleSession, chat, p.FileProject).Scan(&program, &resume)
	if err != nil || !cliSessionID.MatchString(resume) {
		return terminalSession{}, errors.New("This chat has no saved native session in the shared project")
	}
	if _, supported := cliResumeFlags[program]; !supported {
		return terminalSession{}, errors.New("This agent does not provide a remote CLI")
	}
	if s.journalOwner(ctx, chat) != "" {
		active, _ := s.history.activity(chat)
		if slices.ContainsFunc(active, func(item liveOutput) bool {
			return item.Status == "Running" || item.Status == "Awaiting approval"
		}) {
			return terminalSession{}, errors.New("Wait for the current turn to finish before opening this chat in the CLI")
		}
	}
	requestID := fmt.Sprintf("cli-%x", sha256.Sum256([]byte(chat+"\x00"+program+"\x00"+resume+"\x00"+time.Now().UTC().Format(time.RFC3339Nano))))
	return s.beginTerminalSession(p, requestID, program, resume, "")
}

func (s *service) savedTerminal(ctx context.Context, chat, project string) (string, error) {
	if !s.history.hasChat(ctx, chat) {
		return "", errors.New("This chat has no shared terminal")
	}
	f, err := os.Open(filepath.Join(s.history.root, "workspace.json"))
	if err != nil {
		return "", err
	}
	defer f.Close()
	var saved struct {
		Decks []savedHostTerminalDeck `json:"terminal_decks"`
	}
	if err := json.NewDecoder(io.LimitReader(f, 8<<20)).Decode(&saved); err != nil {
		return "", err
	}
	for _, deck := range saved.Decks {
		if id := deck.resume(chat, project); id != "" {
			return id, nil
		}
	}
	return "", errors.New("This chat has no saved terminal in the project")
}

func (s *service) serveTerminalStream(w http.ResponseWriter, r *http.Request, p peer, id string) {
	deadline := time.NewTimer(7 * time.Second)
	defer deadline.Stop()
	for {
		f, err := os.Stat(filepath.Join(socketDir(s.root), terminalSocket(id)))
		if err == nil && f.Mode()&os.ModeSocket != 0 {
			break
		}
		if archive, err := os.ReadFile(filepath.Join(s.root, "terminals", id, "scrollback")); err == nil {
			conn, err := websocket.Accept(w, r, nil)
			if err != nil {
				return
			}
			defer conn.CloseNow()
			for offset := 0; offset < len(archive); offset += 48_000 {
				if !s.state.terminalAllowed(p.Node, p.Device, p.FileProject) {
					return
				}
				ctx, stop := context.WithTimeout(r.Context(), 4*time.Second)
				err = conn.Write(ctx, websocket.MessageBinary, archive[offset:min(offset+48_000, len(archive))])
				stop()
				if err != nil {
					return
				}
			}
			_ = conn.Close(websocket.StatusNormalClosure, "Terminal ended")
			return
		}
		select {
		case <-r.Context().Done():
			return
		case <-deadline.C:
			reject(w, 503, "Host terminal is unavailable")
			return
		case <-time.After(150 * time.Millisecond):
		}
	}

	select {
	case s.ptySlots <- struct{}{}:
		defer func() { <-s.ptySlots }()
	default:
		reject(w, 503, "Too many open terminal viewers")
		return
	}
	conn, err := websocket.Accept(w, r, nil)
	if err != nil {
		return
	}
	defer conn.CloseNow()
	conn.SetReadLimit(16000)
	ctx, cancel := context.WithCancel(r.Context())
	defer cancel()
	grant := func() bool {
		return s.state.terminalAllowed(p.Node, p.Device, p.FileProject)
	}
	go func() {
		defer cancel()
		for ctx.Err() == nil && grant() {
			kind, data, err := conn.Read(ctx)
			if err != nil || !grant() {
				return
			}
			value := terminalIO{Op: "input", Input: data}
			if kind == websocket.MessageText {
				if json.Unmarshal(data, &value) != nil || value.Op != "resize" {
					return
				}
			}
			packet, _ := json.Marshal(value)
			if _, err := callLocal(ctx, s.root, terminalSocket(id), packet); err != nil {
				return
			}
		}
	}()
	var after int64
	until := time.Now().Add(6 * time.Second)
	for ctx.Err() == nil && grant() {
		packet, _ := json.Marshal(terminalIO{Op: "read", After: after})
		b, err := callLocal(ctx, s.root, terminalSocket(id), packet)
		if err != nil {
			if time.Now().After(until) {
				break
			}
			select {
			case <-ctx.Done():
				return
			case <-time.After(150 * time.Millisecond):
			}
			continue
		}
		until = time.Now()
		var value terminalOutput
		if json.Unmarshal(b, &value) != nil {
			return
		}
		after = value.Next
		if len(value.Output) > 0 && grant() {
			writeCtx, done := context.WithTimeout(ctx, 4*time.Second)
			err = conn.Write(writeCtx, websocket.MessageBinary, value.Output)
			done()
			if err != nil {
				return
			}
		}
		if value.Done {
			_ = conn.Close(websocket.StatusNormalClosure, "Terminal ended")
			return
		}
	}
	if !grant() {
		_ = conn.Close(websocket.StatusPolicyViolation, "Kai terminal access revoked")
	} else {
		_ = conn.Close(websocket.StatusInternalError, "Host terminal unavailable")
	}
}
