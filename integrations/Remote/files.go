package main

import (
	"bytes"
	"context"
	"crypto/sha256"
	"database/sql"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"mime"
	"net/http"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"time"
	"unicode/utf8"

	"golang.org/x/sys/unix"
)

const maxTransfer = 512 << 20
const maxPreview = 256 << 10
const maxFileEdit = 64 << 10

type fileEntry struct {
	Name     string    `json:"name"`
	Kind     string    `json:"kind"`
	Bytes    int64     `json:"bytes"`
	Modified time.Time `json:"modified"`
}

type fileDirectory struct {
	Entries  []fileEntry `json:"entries"`
	Next     string      `json:"next,omitempty"`
	Revision string      `json:"revision"`
}

type filePreview struct {
	Text        string `json:"text,omitempty"`
	Revision    string `json:"revision"`
	Size        int64  `json:"size"`
	ContentType string `json:"content_type"`
	Editable    bool   `json:"editable"`
}

func (w workspaceHistory) allowsProject(ctx context.Context, dir string) bool {
	if dir == "/" || !filepath.IsAbs(dir) || filepath.Clean(dir) != dir {
		return false
	}
	reg, err := w.registry()
	if err != nil || contains(reg.Hidden, dir) {
		return false
	}
	if contains(reg.Custom, dir) {
		return true
	}
	db, err := w.database()
	if err != nil {
		return false
	}
	defer db.Close()
	var exists bool
	return db.QueryRowContext(ctx, "SELECT EXISTS(SELECT 1 FROM sessions WHERE project_path=?)", dir).Scan(&exists) == nil && exists
}

func relativeFile(name string) bool {
	return len(name) <= 4096 && filepath.IsLocal(name) && filepath.Clean(name) == name
}

func directoryEntries(root *os.Root, name, after string) (fileDirectory, error) {
	result := fileDirectory{Entries: []fileEntry{}}
	f, err := root.Open(name)
	if err != nil {
		return result, err
	}
	defer f.Close()
	info, err := f.Stat()
	if err != nil {
		return result, err
	}
	if !info.IsDir() {
		return result, errors.New("Path is not a directory")
	}
	items, err := f.ReadDir(-1)
	if err != nil {
		return result, err
	}
	slices.SortFunc(items, func(a, b os.DirEntry) int { return strings.Compare(a.Name(), b.Name()) })
	revision := sha256.New()
	for _, item := range items {
		info, err := item.Info()
		if err != nil {
			continue
		}
		kind := "file"
		switch {
		case info.IsDir():
			kind = "folder"
		case !info.Mode().IsRegular():
			continue
		}
		fmt.Fprintf(revision, "%s\x00%d\x00%d\x00", item.Name(), info.Size(), info.ModTime().UnixNano())
		if item.Name() <= after {
			continue
		}
		if len(result.Entries) >= 200 {
			result.Next = result.Entries[len(result.Entries)-1].Name
			continue
		}
		result.Entries = append(result.Entries, fileEntry{item.Name(), kind, info.Size(), info.ModTime()})
	}
	result.Revision = hex.EncodeToString(revision.Sum(nil))
	return result, nil
}

func openProjectFile(root *os.Root, name string) (*os.File, os.FileInfo, error) {
	info, err := root.Lstat(name)
	if err != nil {
		return nil, nil, err
	}
	if !info.Mode().IsRegular() || info.Size() > maxTransfer {
		return nil, nil, errors.New("File is unavailable or too large")
	}
	f, err := root.Open(name)
	if err != nil {
		return nil, nil, err
	}
	opened, err := f.Stat()
	if err != nil || !os.SameFile(info, opened) {
		f.Close()
		return nil, nil, errors.New("File changed before it could be opened")
	}
	return f, info, nil
}

func revisionOf(f *os.File) (string, error) {
	hash := sha256.New()
	if _, err := io.Copy(hash, io.LimitReader(f, maxTransfer+1)); err != nil {
		return "", err
	}
	if _, err := f.Seek(0, io.SeekStart); err != nil {
		return "", err
	}
	return hex.EncodeToString(hash.Sum(nil)), nil
}

func (s *service) files(w http.ResponseWriter, r *http.Request, grant peer, endpoint string) {
	project := r.URL.Query().Get("project")
	name := r.URL.Query().Get("path")
	if name == "" {
		name = "."
	}
	if (!grant.Control && grant.FileProject != project) || !s.history.allowsProject(r.Context(), project) {
		reject(w, 403, "This project has no file access grant")
		return
	}
	if !relativeFile(name) {
		reject(w, 400, "Invalid project-relative path")
		return
	}
	canonical, err := filepath.EvalSymlinks(project)
	if err != nil || canonical != project {
		reject(w, 403, "The granted project directory changed. Reapprove file access on the host.")
		return
	}
	root, err := os.OpenRoot(project)
	if err != nil {
		reject(w, 404, "Project directory unavailable")
		return
	}
	defer root.Close()
	if r.Method == http.MethodGet && endpoint == "files" {
		if query := r.URL.Query().Get("search"); query != "" {
			results, err := findProjectFiles(r.Context(), root, name, query)
			if err != nil {
				reject(w, 400, "Could not search the project")
				return
			}
			respond(w, 200, results)
			return
		}
		result, err := directoryEntries(root, name, r.URL.Query().Get("after"))
		if err != nil {
			reject(w, 404, "Directory unavailable")
			return
		}
		respond(w, 200, result)
		return
	}
	if r.Method == http.MethodPut && endpoint == "file" && grant.Control {
		if r.Header.Get("If-None-Match") == "*" {
			if r.URL.Query().Get("composer_upload") == "true" {
				if filepath.Dir(name) != ".kai-attachments" || filepath.Base(name) == "." {
					reject(w, 400, "Invalid composer attachment path")
					return
				}
				if err := root.Mkdir(".kai-attachments", 0700); err != nil && !errors.Is(err, os.ErrExist) {
					reject(w, 503, "Could not prepare the host attachment folder")
					return
				}
				info, err := root.Lstat(".kai-attachments")
				if err != nil || !info.IsDir() {
					reject(w, 409, "The host attachment folder is unavailable")
					return
				}
			}
			s.uploadFile(w, r, root, name)
			return
		}
		s.updateFile(w, r, root, name)
		return
	}
	if r.Method != http.MethodGet || endpoint != "file" {
		http.NotFound(w, r)
		return
	}
	if r.URL.Query().Get("diff") == "true" {
		value, err := projectDiff(r.Context(), project, name)
		if err != nil {
			reject(w, 404, "No Git diff is available for this path")
			return
		}
		respond(w, 200, map[string]string{"diff": value})
		return
	}
	f, info, err := openProjectFile(root, name)
	if err != nil {
		reject(w, 404, "File unavailable")
		return
	}
	defer f.Close()
	contentType := mime.TypeByExtension(filepath.Ext(name))
	if contentType == "" {
		contentType = "application/octet-stream"
	}
	revision, err := revisionOf(f)
	if err != nil {
		reject(w, 503, "Could not verify file revision")
		return
	}
	if r.URL.Query().Get("preview") == "true" {
		preview := filePreview{Revision: revision, Size: info.Size(), ContentType: contentType}
		if info.Size() <= maxPreview {
			body, err := io.ReadAll(io.LimitReader(f, maxPreview+1))
			if err == nil && utf8.Valid(body) && !bytes.ContainsRune(body, 0) {
				preview.Text, preview.Editable = string(body), grant.Control && len(body) <= maxFileEdit
			}
		}
		respond(w, 200, preview)
		return
	}
	w.Header().Set("ETag", `"`+revision+`"`)
	w.Header().Set("Content-Type", contentType)
	w.Header().Set("Content-Disposition", "attachment")
	w.Header().Set("Cache-Control", "no-store")
	w.Header().Set("X-Content-Type-Options", "nosniff")
	var content io.ReadSeeker = f
	if grant.Node != "" {
		content = ioWithGrant{ReadSeeker: f, allowed: func() bool {
			return s.state.fileAllowed(grant.Node, grant.Device, project)
		}}
	}
	http.ServeContent(w, r, filepath.Base(name), info.ModTime(), content)
}

type ioWithGrant struct {
	io.ReadSeeker
	allowed func() bool
}

func (g ioWithGrant) Read(b []byte) (int, error) {
	if !g.allowed() {
		return 0, errors.New("Kai file access was revoked")
	}
	return g.ReadSeeker.Read(b)
}

func (s *service) updateFile(w http.ResponseWriter, r *http.Request, root *os.Root, name string) {
	want := strings.Trim(r.Header.Get("If-Match"), "\"")
	if len(want) != 64 {
		reject(w, 428, "A saved file revision is required")
		return
	}
	body, err := io.ReadAll(http.MaxBytesReader(w, r.Body, maxFileEdit))
	if err != nil || !utf8.Valid(body) || bytes.ContainsRune(body, 0) {
		reject(w, 400, "Invalid UTF-8 text edit")
		return
	}
	s.fileWrite.Lock()
	defer s.fileWrite.Unlock()
	current, info, err := openProjectFile(root, name)
	if err != nil {
		reject(w, 404, "File unavailable")
		return
	}
	revision, revErr := revisionOf(current)
	current.Close()
	if revErr != nil || want != revision {
		reject(w, 412, "The host file changed. Reload its latest revision before editing.")
		return
	}
	var tmp string
	for i := 0; i < 4; i++ {
		tmp = filepath.Join(filepath.Dir(name), ".kai-edit-"+strconv.FormatInt(time.Now().UnixNano(), 36))
		temp, err := root.OpenFile(tmp, os.O_WRONLY|os.O_CREATE|os.O_EXCL, info.Mode().Perm())
		if errors.Is(err, os.ErrExist) {
			continue
		}
		if err != nil {
			reject(w, 503, "Could not prepare the edit")
			return
		}
		defer root.Remove(tmp)
		_, err = temp.Write(body)
		if err == nil {
			err = temp.Sync()
		}
		if closeErr := temp.Close(); err == nil {
			err = closeErr
		}
		if err != nil {
			reject(w, 503, "Could not save the edit")
			return
		}
		verify, _, err := openProjectFile(root, name)
		if err != nil {
			reject(w, 412, "The host file changed")
			return
		}
		actual, err := revisionOf(verify)
		verify.Close()
		if err != nil || actual != want {
			reject(w, 412, "The host file changed")
			return
		}
		if err := root.Rename(tmp, name); err != nil {
			reject(w, 503, "Could not activate the edit")
			return
		}
		respond(w, 200, map[string]string{"revision": fmt.Sprintf("%x", sha256.Sum256(body))})
		return
	}
	reject(w, 503, "Could not create a temporary file")
}

func (s *service) fileRequest(ctx context.Context, p tailPeer, req localRequest) (*http.Response, error) {
	client := s.peerClient(p)
	if client == nil {
		return nil, errTailnetOffline
	}
	endpoint := "file"
	method := http.MethodGet
	values := url.Values{"project": {req.Project}, "path": {req.File}}
	var body io.Reader
	var upload *os.File
	switch req.Action {
	case "files":
		endpoint = "files"
		if req.Cursor != "" {
			values.Set("after", req.Cursor)
		}
	case "file-search":
		endpoint = "files"
		values.Set("search", req.Text)
	case "file-diff":
		values.Set("diff", "true")
	case "file-upload", "composer-upload":
		if req.Action == "composer-upload" {
			values.Set("composer_upload", "true")
		}
		method = http.MethodPut
		if !filepath.IsAbs(req.Path) {
			return nil, errors.New("Choose a local file to upload")
		}
		var err error
		upload, err = os.Open(req.Path)
		if err != nil {
			return nil, err
		}
		info, err := upload.Stat()
		if err != nil || !info.Mode().IsRegular() || info.Size() > maxUpload {
			upload.Close()
			return nil, errors.New("The upload must be a regular file up to 128 MiB")
		}
		body = upload
	case "file-preview":
		values.Set("preview", "true")
	case "file-save":
		method = http.MethodPut
		body = strings.NewReader(req.Text)
	}
	r, err := http.NewRequestWithContext(ctx, method, "http://"+p.address+prefix+endpoint+"?"+values.Encode(), body)
	if err != nil {
		if upload != nil {
			upload.Close()
		}
		return nil, err
	}
	r.Header.Set("Kai-Device", s.state.snapshot().Device)
	r.Header.Set("Kai-Protocol", strconv.Itoa(protocol))
	if method == http.MethodPut {
		if req.Action == "file-upload" || req.Action == "composer-upload" {
			r.Header.Set("If-None-Match", "*")
		} else {
			r.Header.Set("If-Match", `"`+req.Revision+`"`)
		}
	}
	return client.Do(r)
}

const maxUpload = 128 << 20

func (s *service) uploadFile(w http.ResponseWriter, r *http.Request, root *os.Root, name string) {
	if name == "." {
		reject(w, 400, "Choose a new file name")
		return
	}
	s.fileWrite.Lock()
	defer s.fileWrite.Unlock()
	if _, err := root.Lstat(name); !errors.Is(err, os.ErrNotExist) {
		reject(w, 409, "A file with that name already exists")
		return
	}
	tmp := filepath.Join(filepath.Dir(name), ".kai-upload-"+strconv.FormatInt(time.Now().UnixNano(), 36))
	file, err := root.OpenFile(tmp, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0600)
	if err != nil {
		reject(w, 503, "Could not prepare upload")
		return
	}
	defer root.Remove(tmp)
	hash := sha256.New()
	_, err = io.Copy(io.MultiWriter(file, hash), http.MaxBytesReader(w, r.Body, maxUpload))
	if err == nil {
		err = file.Sync()
	}
	if closeErr := file.Close(); err == nil {
		err = closeErr
	}
	if err != nil {
		reject(w, 400, "The upload was interrupted or exceeded 128 MiB")
		return
	}
	// Link publishes the completed file without allowing a concurrent creator to be overwritten.
	if err := root.Link(tmp, name); err != nil {
		reject(w, 409, "The upload target is unavailable or already exists")
		return
	}
	respond(w, 200, map[string]string{"revision": hex.EncodeToString(hash.Sum(nil))})
}

func findProjectFiles(ctx context.Context, root *os.Root, start, query string) ([]fileEntry, error) {
	if len(query) > 256 {
		return nil, errors.New("Search text is too long")
	}
	query = strings.ToLower(query)
	matches := []fileEntry{}
	visited := 0
	err := fs.WalkDir(root.FS(), start, func(path string, item fs.DirEntry, err error) error {
		if err != nil {
			return nil
		}
		if ctx.Err() != nil {
			return ctx.Err()
		}
		visited++
		if visited > 20_000 || len(matches) >= 200 {
			return fs.SkipAll
		}
		if item.IsDir() {
			if path != start && slices.Contains([]string{".git", "node_modules", ".build"}, item.Name()) {
				return fs.SkipDir
			}
			return nil
		}
		if strings.Contains(strings.ToLower(path), query) && item.Type().IsRegular() {
			info, err := item.Info()
			if err == nil {
				matches = append(matches, fileEntry{Name: path, Kind: "file", Bytes: info.Size(), Modified: info.ModTime()})
			}
		}
		return nil
	})
	return matches, err
}

func projectDiff(ctx context.Context, project, path string) (string, error) {
	ctx, cancel := context.WithTimeout(ctx, 4*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, "/usr/bin/git", "--no-pager", "--literal-pathspecs", "-C", project, "diff", "--no-ext-diff", "--no-textconv", "HEAD", "--", path)
	out, err := cmd.StdoutPipe()
	if err != nil {
		return "", err
	}
	if err = cmd.Start(); err != nil {
		return "", err
	}
	data, err := io.ReadAll(io.LimitReader(out, maxPreview+1))
	if len(data) > maxPreview {
		_ = cmd.Process.Kill()
		_ = cmd.Wait()
		return string(data[:maxPreview]), nil
	}
	if err = errors.Join(err, cmd.Wait()); err != nil {
		return "", err
	}
	return strings.ToValidUTF8(string(data), "\uFFFD"), nil
}

func (w workspaceHistory) hasChat(ctx context.Context, id string) bool {
	reg, err := w.registry()
	if err != nil || contains(reg.HiddenSessions, id) {
		return false
	}
	db, err := w.database()
	if err != nil {
		return false
	}
	defer db.Close()
	var project string
	err = db.QueryRowContext(ctx, "SELECT project_path FROM sessions WHERE id=? AND "+visibleSession, id).Scan(&project)
	return !errors.Is(err, sql.ErrNoRows) && err == nil && !contains(reg.Hidden, project)
}

func (s *service) clientFile(ctx context.Context, req localRequest) (any, error) {
	target, err := s.resolvePeer(ctx, req.Node, req.Device)
	if err != nil {
		return nil, err
	}
	if req.Project == "" || (target.FileProject != req.Project &&
		!(target.Control && slices.Contains(target.Capabilities, "workspace.paired-access"))) {
		return nil, errors.New("The host has not shared this project’s files")
	}
	if req.Action == "file-download" {
		return s.downloadFile(ctx, target, req)
	}
	res, err := s.fileRequest(ctx, target, req)
	if err != nil {
		return nil, err
	}
	defer res.Body.Close()
	if res.StatusCode != 200 {
		return nil, fileResponseError(res)
	}
	var out json.RawMessage
	if err := json.NewDecoder(io.LimitReader(res.Body, maxResponse)).Decode(&out); err != nil {
		return nil, err
	}
	return out, nil
}

func fileResponseError(res *http.Response) error {
	var value struct {
		Error string `json:"error"`
	}
	if err := json.NewDecoder(io.LimitReader(res.Body, 16384)).Decode(&value); err == nil && value.Error != "" {
		return errors.New(value.Error)
	}
	return fmt.Errorf("Kai file request returned HTTP %d", res.StatusCode)
}

func (s *service) downloadFile(ctx context.Context, p tailPeer, req localRequest) (any, error) {
	if !filepath.IsAbs(req.Path) || filepath.Clean(req.Path) != req.Path {
		return nil, errors.New("Choose a local file destination")
	}
	sum := sha256.Sum256([]byte(p.Node + "\x00" + p.Device + "\x00" + req.Project + "\x00" + req.File + "\x00" + req.Path))
	tmp := filepath.Join(filepath.Dir(req.Path), fmt.Sprintf(".kai-download-%x", sum[:12]))
	lock, err := lockFile(tmp + ".lock")
	if err != nil {
		return nil, errors.New("A download to this destination is already running")
	}
	defer lock.Close()
	resume, _ := os.ReadFile(tmp + ".etag")
	etag := string(resume)
	var offset int64
	if info, err := os.Lstat(tmp); err == nil && info.Mode().IsRegular() && validETag(etag) && info.Size() <= maxTransfer {
		offset = info.Size()
	}
	res, err := s.openDownload(ctx, p, req, offset, etag)
	if err != nil {
		return nil, err
	}
	defer res.Body.Close()
	if res.StatusCode == http.StatusRequestedRangeNotSatisfiable && offset > 0 {
		res.Body.Close()
		offset = 0
		res, err = s.openDownload(ctx, p, req, 0, "")
		if err != nil {
			return nil, err
		}
		defer res.Body.Close()
	}
	switch res.StatusCode {
	case http.StatusOK:
		offset = 0
	case http.StatusPartialContent:
		if offset == 0 || etag != res.Header.Get("ETag") || !strings.HasPrefix(res.Header.Get("Content-Range"), fmt.Sprintf("bytes %d-", offset)) {
			return nil, errors.New("The host returned an invalid download range")
		}
	default:
		return nil, fileResponseError(res)
	}
	etag = res.Header.Get("ETag")
	if !validETag(etag) || res.ContentLength < 0 || offset+res.ContentLength > maxTransfer {
		return nil, errors.New("The host did not supply a valid file integrity hash or size")
	}
	flags := os.O_WRONLY | os.O_CREATE | unix.O_NOFOLLOW
	if offset == 0 {
		flags |= os.O_TRUNC
	}
	f, err := os.OpenFile(tmp, flags, 0600)
	if err != nil {
		return nil, err
	}
	if err := os.WriteFile(tmp+".etag", []byte(etag), 0600); err != nil {
		f.Close()
		return nil, err
	}
	if _, err := f.Seek(offset, io.SeekStart); err != nil {
		f.Close()
		return nil, err
	}
	n, err := io.Copy(f, io.LimitReader(res.Body, res.ContentLength+1))
	if err == nil {
		err = f.Sync()
	}
	if closeErr := f.Close(); err == nil {
		err = closeErr
	}
	if err != nil || n != res.ContentLength {
		return nil, errors.New("The transfer was interrupted. Download to the same destination to resume.")
	}
	f, err = os.Open(tmp)
	if err != nil {
		return nil, err
	}
	actual, err := revisionOf(f)
	f.Close()
	if err != nil || strings.Trim(etag, "\"") != actual {
		_ = os.Remove(tmp)
		_ = os.Remove(tmp + ".etag")
		return nil, errors.New("The downloaded file failed integrity verification")
	}
	if err := os.Rename(tmp, req.Path); err != nil {
		return nil, err
	}
	_ = os.Remove(tmp + ".etag")
	return map[string]string{"path": req.Path, "revision": actual}, nil
}

func (s *service) openDownload(ctx context.Context, p tailPeer, req localRequest, offset int64, etag string) (*http.Response, error) {
	client := s.peerClient(p)
	if client == nil {
		return nil, errTailnetOffline
	}
	path := url.Values{"project": {req.Project}, "path": {req.File}}.Encode()
	request, err := http.NewRequestWithContext(ctx, http.MethodGet, "http://"+p.address+prefix+"file?"+path, nil)
	if err != nil {
		return nil, err
	}
	request.Header.Set("Kai-Device", s.state.snapshot().Device)
	request.Header.Set("Kai-Protocol", strconv.Itoa(protocol))
	if offset > 0 {
		request.Header.Set("Range", fmt.Sprintf("bytes=%d-", offset))
		request.Header.Set("If-Range", etag)
	}
	return client.Do(request)
}

func validETag(value string) bool {
	if len(value) != 66 || value[0] != '"' || value[65] != '"' {
		return false
	}
	_, err := hex.DecodeString(value[1:65])
	return err == nil
}
