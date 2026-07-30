// Package ipc — pont HTTP entre les services Go et le lecteur Rust.
// Le lecteur Rust appelle ces endpoints via reqwest.
package ipc

import (
	"encoding/json"
	"fmt"
	"net/http"
	"os"
	"path/filepath"
	"strings"

	"forgeplayer/pkg/metadata"
	"forgeplayer/pkg/subtitles"
)

// Bridge regroupe les handlers HTTP exposés au lecteur Rust.
type Bridge struct {
	subtitleClient *subtitles.Client
	tmdbClient     *metadata.Client
	mux            *http.ServeMux
}

// NewBridge crée un Bridge avec les clés API configurées.
func NewBridge(subtitleAPIKey, tmdbAPIKey, lang string) *Bridge {
	b := &Bridge{
		subtitleClient: subtitles.NewClient(subtitleAPIKey),
		tmdbClient:     metadata.NewTMDBClient(tmdbAPIKey, lang),
		mux:            http.NewServeMux(),
	}
	b.registerRoutes()
	return b
}

func (b *Bridge) registerRoutes() {
	b.mux.HandleFunc("GET /health",              b.handleHealth)
	b.mux.HandleFunc("GET /subtitles/search",    b.handleSubtitleSearch)
	b.mux.HandleFunc("POST /subtitles/download", b.handleSubtitleDownload)
	b.mux.HandleFunc("GET /metadata/movie",      b.handleMovieSearch)
	b.mux.HandleFunc("GET /metadata/tv",         b.handleTVSearch)
}

// ListenAndServe démarre le serveur HTTP sur addr (ex: "127.0.0.1:18080").
func (b *Bridge) ListenAndServe(addr string) error {
	fmt.Printf("[ipc] bridge Go écoute sur http://%s\n", addr)
	return http.ListenAndServe(addr, b.mux)
}

// ── Handlers ──────────────────────────────────────────────────────────────────

func (b *Bridge) handleHealth(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, 200, map[string]string{"status": "ok"})
}

// GET /subtitles/search?filename=Movie.Name.mkv&lang=fr
func (b *Bridge) handleSubtitleSearch(w http.ResponseWriter, r *http.Request) {
	filename := r.URL.Query().Get("filename")
	lang     := r.URL.Query().Get("lang")
	if filename == "" || lang == "" {
		writeJSON(w, 400, map[string]string{"error": "filename et lang requis"})
		return
	}

	results, err := b.subtitleClient.Search(filename, lang)
	if err != nil {
		writeJSON(w, 500, map[string]string{"error": err.Error()})
		return
	}
	writeJSON(w, 200, results)
}

// POST /subtitles/download  body: {"file_id":123456,"dest_dir":"/tmp"}
func (b *Bridge) handleSubtitleDownload(w http.ResponseWriter, r *http.Request) {
	// Guard against oversized request bodies (max 4 KB for this small JSON payload).
	r.Body = http.MaxBytesReader(w, r.Body, 4096)
	var req struct {
		FileID  int    `json:"file_id"`
		DestDir string `json:"dest_dir"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		writeJSON(w, 400, map[string]string{"error": err.Error()})
		return
	}

	// H8 — valider dest_dir pour prévenir la traversée de répertoires
	safeDir, err := validateDestDir(req.DestDir)
	if err != nil {
		writeJSON(w, 400, map[string]string{"error": err.Error()})
		return
	}

	path, err := b.subtitleClient.Download(req.FileID, safeDir)
	if err != nil {
		writeJSON(w, 500, map[string]string{"error": err.Error()})
		return
	}
	writeJSON(w, 200, map[string]string{"path": path})
}

// GET /metadata/movie?title=Inception&lang=fr
func (b *Bridge) handleMovieSearch(w http.ResponseWriter, r *http.Request) {
	title := r.URL.Query().Get("title")
	if title == "" {
		writeJSON(w, 400, map[string]string{"error": "title requis"})
		return
	}
	results, err := b.tmdbClient.SearchMovie(title)
	if err != nil {
		writeJSON(w, 500, map[string]string{"error": err.Error()})
		return
	}
	writeJSON(w, 200, results)
}

// GET /metadata/tv?title=Breaking+Bad
func (b *Bridge) handleTVSearch(w http.ResponseWriter, r *http.Request) {
	title := r.URL.Query().Get("title")
	if title == "" {
		writeJSON(w, 400, map[string]string{"error": "title requis"})
		return
	}
	results, err := b.tmdbClient.SearchTV(title)
	if err != nil {
		writeJSON(w, 500, map[string]string{"error": err.Error()})
		return
	}
	writeJSON(w, 200, results)
}

// ── Utils ─────────────────────────────────────────────────────────────────────

// validateDestDir résout le chemin absolu et s'assure qu'il reste dans le home de l'utilisateur.
// Empêche les traversées de répertoires via ".." ou chemins symboliques.
func validateDestDir(destDir string) (string, error) {
	abs, err := filepath.Abs(destDir)
	if err != nil {
		return "", fmt.Errorf("chemin invalide: %w", err)
	}

	home, err := os.UserHomeDir()
	if err != nil {
		return "", fmt.Errorf("impossible de déterminer le home: %w", err)
	}

	// S'assurer que le séparateur final est présent pour éviter les faux positifs
	// (ex: home=/home/user, abs=/home/username ne doit pas matcher)
	if !strings.HasPrefix(abs, home+string(filepath.Separator)) && abs != home {
		return "", fmt.Errorf("dossier de destination non autorisé: %s", abs)
	}

	return abs, nil
}

func writeJSON(w http.ResponseWriter, code int, body any) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(code)
	_ = json.NewEncoder(w).Encode(body)
}
