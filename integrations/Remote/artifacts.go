package main

import (
	"database/sql"
	"errors"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"strconv"
)

type artifactItem struct {
	ID       string  `json:"id"`
	Chat     string  `json:"chat_id"`
	Title    string  `json:"title"`
	Kind     string  `json:"kind"`
	Revision int64   `json:"revision"`
	Updated  int64   `json:"updated_at"`
	Bytes    int64   `json:"bytes"`
	Content  *string `json:"content,omitempty"`
}

func (s *service) artifacts(w http.ResponseWriter, r *http.Request, grant peer) {
	project := r.URL.Query().Get("project")
	if grant.FileProject != project || !s.history.allowsProject(r.Context(), project) {
		reject(w, 403, "The host has not shared this project’s artifacts")
		return
	}
	path := filepath.Join(s.history.root, "artifacts", "artifacts.sqlite")
	if _, err := os.Stat(path); errors.Is(err, os.ErrNotExist) {
		respond(w, 200, map[string]any{"artifacts": []artifactItem{}})
		return
	}
	uri := url.URL{Scheme: "file", Path: path, RawQuery: "mode=ro&_busy_timeout=2000&_query_only=1"}
	db, err := sql.Open("sqlite3", uri.String())
	if err != nil {
		reject(w, 503, "Artifacts are unavailable")
		return
	}
	defer db.Close()
	reg, err := s.history.registry()
	if err != nil {
		reject(w, 503, "Artifacts are unavailable")
		return
	}
	id := r.URL.Query().Get("id")
	if id != "" {
		var a artifactItem
		err := db.QueryRowContext(r.Context(), `SELECT id,chat,title,kind,revision,updated,length(cast(body as blob)),body 
			FROM artifacts WHERE project=? AND id=?`, project, id).Scan(&a.ID, &a.Chat, &a.Title, &a.Kind, &a.Revision, &a.Updated, &a.Bytes, &a.Content)
		if err != nil || contains(reg.HiddenSessions, a.Chat) {
			reject(w, 404, "Artifact is unavailable")
			return
		}
		respond(w, 200, a)
		return
	}
	offset, _ := strconv.Atoi(r.URL.Query().Get("offset"))
	if offset < 0 || offset > 100000 {
		reject(w, 400, "Invalid artifact cursor")
		return
	}
	type list struct {
		Artifacts []artifactItem `json:"artifacts"`
		Next      int            `json:"next_offset,omitempty"`
	}
	result := list{Artifacts: []artifactItem{}}
	rows, err := db.QueryContext(r.Context(), `SELECT id,chat,title,kind,revision,updated,length(cast(body as blob)) 
		FROM artifacts WHERE project=? ORDER BY updated DESC,id LIMIT 51 OFFSET ?`, project, offset)
	if err != nil {
		reject(w, 503, "Artifacts are unavailable")
		return
	}
	defer rows.Close()
	index := 0
	for rows.Next() {
		index++
		if index > 50 {
			result.Next = offset + 50
			break
		}
		var a artifactItem
		if err := rows.Scan(&a.ID, &a.Chat, &a.Title, &a.Kind, &a.Revision, &a.Updated, &a.Bytes); err != nil {
			reject(w, 503, "Artifacts are unavailable")
			return
		}
		if !contains(reg.HiddenSessions, a.Chat) {
			result.Artifacts = append(result.Artifacts, a)
		}
	}
	if rows.Err() != nil {
		reject(w, 503, "Artifacts are unavailable")
		return
	}
	respond(w, 200, result)
}
