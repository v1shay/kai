package main

import (
	"context"
	"database/sql"
	"encoding/json"
	"errors"
	"io"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"time"

	_ "github.com/mattn/go-sqlite3"
)

type workspaceHistory struct {
	root string
}

type chatSummary struct {
	ID       string `json:"id"`
	Title    string `json:"title"`
	Agent    string `json:"agent"`
	Project  string `json:"project"`
	Updated  int64  `json:"updated_at"`
	Terminal bool   `json:"terminal,omitempty"`
}

type message struct {
	ID        int64  `json:"id"`
	Role      string `json:"role"`
	Text      string `json:"text"`
	Truncated bool   `json:"truncated"`
}

func (s *service) canonicalHistory(ctx context.Context, id string, page int64) (json.RawMessage, error) {
	if s.app == "" || page < 0 || page > 2000 {
		return nil, errors.New("unsupported transcript request")
	}
	select {
	case s.historySlots <- struct{}{}:
		defer func() { <-s.historySlots }()
	case <-ctx.Done():
		return nil, ctx.Err()
	}
	ctx, cancel := context.WithTimeout(ctx, 8*time.Second)
	defer cancel()
	executable := filepath.Join(s.app, "Contents", "MacOS", "kai-unified")
	args := []string{"--kai-remote-history", s.history.root, id, strconv.FormatInt(page, 10)}
	if activity, err := s.history.activity(id); err == nil && slices.ContainsFunc(activity, func(item liveOutput) bool {
		return item.Status == "Running" || item.Status == "Awaiting approval"
	}) {
		args = append(args, "streaming")
	}
	output, err := exec.CommandContext(ctx, executable, args...).Output()
	if err != nil {
		return nil, err
	}
	if len(output) > maxResponse || !json.Valid(output) {
		return nil, errors.New("invalid or oversized transcript page")
	}
	return json.RawMessage(output), nil
}

type projectSummary struct {
	Path string `json:"path"`
	Name string `json:"name"`
}

type projectRegistry struct {
	Custom         []string `json:"custom"`
	Hidden         []string `json:"hidden"`
	HiddenSessions []string `json:"hidden_sessions"`
	names          map[string]string
}

func (p *projectRegistry) UnmarshalJSON(data []byte) error {
	var raw struct {
		Custom         json.RawMessage `json:"custom"`
		Hidden         []string        `json:"hidden"`
		HiddenSessions []string        `json:"hidden_sessions"`
		Lifecycle      struct {
			ProjectNames map[string]string `json:"project_names"`
		} `json:"lifecycle"`
	}
	if err := json.Unmarshal(data, &raw); err != nil {
		return err
	}
	p.Hidden, p.HiddenSessions = raw.Hidden, raw.HiddenSessions
	p.names = make(map[string]string)
	if len(raw.Custom) > 0 {
		var entries []projectSummary
		if err := json.Unmarshal(raw.Custom, &entries); err != nil {
			if err := json.Unmarshal(raw.Custom, &p.Custom); err != nil {
				return err
			}
		} else {
			for _, entry := range entries {
				p.Custom = append(p.Custom, entry.Path)
				p.names[entry.Path] = entry.Name
			}
		}
	}
	for path, name := range raw.Lifecycle.ProjectNames {
		p.names[path] = name
	}
	return nil
}

type metadata struct {
	Projects []projectSummary `json:"projects"`
	Chats    []chatSummary    `json:"chats"`
	Next     int              `json:"next_offset,omitempty"`
}

type workspaceOverview struct {
	Tabs     []chatSummary   `json:"tabs"`
	Active   string          `json:"active"`
	Dock     json.RawMessage `json:"dock,omitempty"`
	Activity []liveOutput    `json:"activity"`
	Running  []chatSummary   `json:"running"`
}

type savedHostTerminalDeck struct {
	Owner    struct{ Chat string }          `json:"owner"`
	Active   string                         `json:"active"`
	Sessions []struct{ ID, Project string } `json:"sessions"`
}

func (deck savedHostTerminalDeck) resume(chat, project string) string {
	if deck.Owner.Chat != chat || chat == "" {
		return ""
	}
	for _, session := range deck.Sessions {
		if session.ID == deck.Active && session.Project == project {
			id := strings.ReplaceAll(session.ID, "-", "")
			if terminalID.MatchString(id) {
				return id
			}
		}
	}
	return ""
}

func (w workspaceHistory) overview(ctx context.Context, live []liveOutput) (workspaceOverview, error) {
	result := workspaceOverview{Tabs: []chatSummary{}, Activity: []liveOutput{}}
	f, err := os.Open(filepath.Join(w.root, "workspace.json"))
	if errors.Is(err, os.ErrNotExist) {
		return result, nil
	}
	if err != nil {
		return result, err
	}
	defer f.Close()
	var saved struct {
		Tabs   []string                `json:"open_tabs"`
		Active string                  `json:"active_chat"`
		Decks  []savedHostTerminalDeck `json:"terminal_decks"`
		Dock   struct {
			Side          string
			Width, Height float64
		} `json:"terminal_dock"`
	}
	if err := json.NewDecoder(io.LimitReader(f, 8<<20)).Decode(&saved); err != nil {
		return result, err
	}
	result.Active = saved.Active
	if saved.Dock.Side != "" {
		result.Dock, _ = json.Marshal(map[string]any{"side": saved.Dock.Side, "width": saved.Dock.Width, "height": saved.Dock.Height})
	}
	if activity, err := w.activity(""); err == nil {
		result.Activity = activity
	}
	for _, item := range live {
		index := slices.IndexFunc(result.Activity, func(existing liveOutput) bool { return existing.Chat == item.Chat })
		if index < 0 {
			result.Activity = append(result.Activity, item)
		} else if item.Status != "Queued" {
			result.Activity[index].Status = item.Status
		}
	}
	reg, err := w.registry()
	if err != nil {
		return result, err
	}
	db, err := w.database()
	if errors.Is(err, os.ErrNotExist) {
		return result, nil
	}
	if err != nil {
		return result, err
	}
	defer db.Close()
	tabs := saved.Tabs[:min(len(saved.Tabs), 32)]
	wanted := slices.Clone(tabs)
	for _, live := range result.Activity[:min(len(result.Activity), 32)] {
		if !slices.Contains(wanted, live.Chat) {
			wanted = append(wanted, live.Chat)
		}
	}
	ids, _ := json.Marshal(wanted)
	rows, err := db.QueryContext(ctx, `SELECT sessions.id,title,agent,project_path,updated_at
		FROM (SELECT key AS tab_order,value AS tab_id FROM json_each(?)) AS tabs
		JOIN sessions ON sessions.id=tabs.tab_id WHERE `+visibleSession+` ORDER BY tab_order`, string(ids))
	if err != nil {
		return result, err
	}
	defer rows.Close()
	for rows.Next() {
		var chat chatSummary
		if err := rows.Scan(&chat.ID, &chat.Title, &chat.Agent, &chat.Project, &chat.Updated); err != nil {
			return result, err
		}
		if !contains(reg.Hidden, chat.Project) && !contains(reg.HiddenSessions, chat.ID) {
			if slices.Contains(tabs, chat.ID) {
				chat.Terminal = slices.ContainsFunc(saved.Decks, func(deck savedHostTerminalDeck) bool {
					return deck.resume(chat.ID, chat.Project) != ""
				})
				result.Tabs = append(result.Tabs, chat)
			}
			if slices.ContainsFunc(result.Activity, func(live liveOutput) bool { return live.Chat == chat.ID }) {
				result.Running = append(result.Running, chat)
			}
		}
	}
	if !slices.ContainsFunc(result.Tabs, func(chat chatSummary) bool { return chat.ID == result.Active }) {
		result.Active = ""
	}
	result.Activity = slices.DeleteFunc(result.Activity, func(item liveOutput) bool {
		return !slices.ContainsFunc(result.Running, func(chat chatSummary) bool { return chat.ID == item.Chat })
	})
	return result, rows.Err()
}

const visibleSession = `NOT (length(id)=36 AND id LIKE '________-____-____-____-____________'
	AND external_id IS NULL AND transcript_path IS NULL
	AND NOT EXISTS (SELECT 1 FROM messages WHERE session_id=sessions.id))`

func (w workspaceHistory) registry() (projectRegistry, error) {
	var registry projectRegistry
	b, err := os.ReadFile(filepath.Join(w.root, "projects.json"))
	if errors.Is(err, os.ErrNotExist) {
		return registry, nil
	}
	if err != nil {
		return registry, err
	}
	return registry, json.Unmarshal(b, &registry)
}

func (w workspaceHistory) database() (*sql.DB, error) {
	path := filepath.Join(w.root, "history.sqlite")
	if _, err := os.Stat(path); err != nil {
		return nil, err
	}
	name := url.URL{Scheme: "file", Path: path, RawQuery: "mode=ro&_busy_timeout=2000&_query_only=1"}
	db, err := sql.Open("sqlite3", name.String())
	if err == nil {
		db.SetMaxOpenConns(1)
	}
	return db, err
}

func (w workspaceHistory) browse(ctx context.Context, project string, offset int) (metadata, error) {
	result := metadata{Projects: []projectSummary{}, Chats: []chatSummary{}}
	reg, err := w.registry()
	if err != nil {
		return result, err
	}
	seen := map[string]bool{}
	for _, p := range reg.Hidden {
		seen[p] = true
	}
	add := func(p string) {
		if filepath.IsAbs(p) && !seen[p] {
			name := reg.names[p]
			if name == "" {
				name = filepath.Base(p)
			}
			result.Projects = append(result.Projects, projectSummary{p, name})
			seen[p] = true
		}
	}
	for _, p := range reg.Custom {
		add(p)
	}
	db, err := w.database()
	if errors.Is(err, os.ErrNotExist) {
		return result, nil
	}
	if err != nil {
		return result, err
	}
	defer db.Close()
	hiddenJSON, _ := json.Marshal(reg.Hidden)
	hiddenChatsJSON, _ := json.Marshal(reg.HiddenSessions)
	rows, err := db.QueryContext(ctx, `SELECT DISTINCT project_path FROM sessions WHERE `+visibleSession+`
		AND NOT EXISTS (SELECT 1 FROM json_each(?) WHERE value=sessions.project_path)
		AND NOT EXISTS (SELECT 1 FROM json_each(?) WHERE value=sessions.id)
		ORDER BY project_path LIMIT 2000`, string(hiddenJSON), string(hiddenChatsJSON))
	if err != nil {
		return result, err
	}
	for rows.Next() {
		var p string
		if err := rows.Scan(&p); err != nil {
			rows.Close()
			return result, err
		}
		add(p)
	}
	err = rows.Err()
	rows.Close()
	if err != nil {
		return result, err
	}
	rows, err = db.QueryContext(ctx, `SELECT id,title,agent,project_path,updated_at FROM sessions WHERE `+visibleSession+`
		AND NOT EXISTS (SELECT 1 FROM json_each(?) WHERE value=sessions.project_path)
		AND NOT EXISTS (SELECT 1 FROM json_each(?) WHERE value=sessions.id)
		AND (? = '' OR project_path = ?) ORDER BY updated_at DESC,id LIMIT 101 OFFSET ?`,
		string(hiddenJSON), string(hiddenChatsJSON), project, project, offset)
	if err != nil {
		return result, err
	}
	defer rows.Close()
	for rows.Next() {
		var chat chatSummary
		if err := rows.Scan(&chat.ID, &chat.Title, &chat.Agent, &chat.Project, &chat.Updated); err != nil {
			return result, err
		}
		if len(result.Chats) == 100 {
			result.Next = offset + 100
			break
		}
		result.Chats = append(result.Chats, chat)
	}
	return result, rows.Err()
}

func (w workspaceHistory) messages(ctx context.Context, id string, before int64) ([]message, error) {
	reg, err := w.registry()
	if err != nil {
		return nil, err
	}
	db, err := w.database()
	if err != nil {
		return nil, err
	}
	defer db.Close()
	var project string
	if err := db.QueryRowContext(ctx, `SELECT project_path FROM sessions WHERE id=? AND `+visibleSession, id).Scan(&project); err != nil {
		return nil, err
	}
	if contains(reg.Hidden, project) || contains(reg.HiddenSessions, id) {
		return nil, sql.ErrNoRows
	}
	rows, err := db.QueryContext(ctx, `SELECT id,role,substr(cast(text AS blob),1,16384),length(cast(text AS blob))>16384
		FROM messages WHERE session_id=? AND (?=0 OR id<?) ORDER BY id DESC LIMIT 30`, id, before, before)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	result := []message{}
	for rows.Next() {
		var m message
		if err := rows.Scan(&m.ID, &m.Role, &m.Text, &m.Truncated); err != nil {
			return nil, err
		}
		m.Text = strings.ToValidUTF8(m.Text, "\uFFFD")
		result = append(result, m)
	}
	for i, j := 0, len(result)-1; i < j; i, j = i+1, j-1 {
		result[i], result[j] = result[j], result[i]
	}
	return result, rows.Err()
}
