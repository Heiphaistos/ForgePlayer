// Service d'indexation de la bibliothèque médias.
// Parcourt récursivement les dossiers configurés et expose les résultats via HTTP.
package main

import (
	"encoding/json"
	"flag"
	"fmt"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"forgeplayer/pkg/metadata"
)

// MediaEntry représente un fichier média indexé.
type MediaEntry struct {
	Path         string    `json:"path"`
	Name         string    `json:"name"`
	Ext          string    `json:"ext"`
	SizeBytes    int64     `json:"size_bytes"`
	ModifiedAt   time.Time `json:"modified_at"`
}

var (
	mu      sync.RWMutex
	library []MediaEntry
)

// Extensions vidéo supportées (doit correspondre à SUPPORTED_EXTENSIONS côté Rust)
var mediaExts = map[string]bool{
	".mp4": true, ".mkv": true, ".avi": true, ".mov": true,
	".wmv": true, ".flv": true, ".webm": true, ".ts": true, ".m2ts": true,
	".mpg": true, ".mpeg": true, ".m4v": true, ".3gp": true, ".ogv": true,
	".rm": true, ".rmvb": true, ".divx": true, ".vob": true, ".f4v": true,
	".mxf": true, ".dv": true,
}

func main() {
	port     := flag.Int("port", 18081, "Port d'écoute de l'indexeur")
	dirs     := flag.String("dirs", "", "Dossiers à indexer, séparés par ';'")
	tmdbKey  := flag.String("tmdb-key", os.Getenv("TMDB_API_KEY"), "Clé API TMDB")
	lang     := flag.String("lang", "fr", "Langue métadonnées")
	flag.Parse()

	_ = tmdbKey // sera utilisé pour enrichir les métadonnées
	_ = lang

	// Indexation initiale
	if *dirs != "" {
		go indexDirs(strings.Split(*dirs, ";"))
	}

	// Routes HTTP
	mux := http.NewServeMux()
	mux.HandleFunc("GET /library",      handleLibrary)
	mux.HandleFunc("POST /index",        handleReindex)
	mux.HandleFunc("GET /search",        handleSearch)

	addr := fmt.Sprintf("127.0.0.1:%d", *port)
	fmt.Printf("[indexer] écoute sur http://%s\n", addr)
	if err := http.ListenAndServe(addr, mux); err != nil {
		fmt.Fprintf(os.Stderr, "[fatal] indexer: %v\n", err)
		os.Exit(1)
	}
}

// GET /library — retourne toute la bibliothèque indexée.
func handleLibrary(w http.ResponseWriter, r *http.Request) {
	mu.RLock()
	defer mu.RUnlock()
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(library)
}

// POST /index  body: {"dirs":["C:\\Films","D:\\Séries"]}
func handleReindex(w http.ResponseWriter, r *http.Request) {
	// Guard against oversized request bodies (max 64 KB for a path list).
	r.Body = http.MaxBytesReader(w, r.Body, 65536)
	var req struct {
		Dirs []string `json:"dirs"`
	}
	if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
		http.Error(w, err.Error(), 400)
		return
	}
	go indexDirs(req.Dirs)
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(map[string]string{"status": "indexing"})
}

// GET /search?q=inception — recherche dans la bibliothèque.
func handleSearch(w http.ResponseWriter, r *http.Request) {
	q := strings.ToLower(r.URL.Query().Get("q"))
	mu.RLock()
	defer mu.RUnlock()

	var results []MediaEntry
	for _, e := range library {
		if strings.Contains(strings.ToLower(e.Name), q) {
			results = append(results, e)
			if len(results) >= 50 { break }
		}
	}
	w.Header().Set("Content-Type", "application/json")
	_ = json.NewEncoder(w).Encode(results)
}

// indexDirs parcourt récursivement les dossiers et remplit la bibliothèque.
func indexDirs(dirs []string) {
	var entries []MediaEntry

	for _, dir := range dirs {
		dir = strings.TrimSpace(dir)
		if dir == "" { continue }

		fmt.Printf("[indexer] scan: %s\n", dir)
		_ = filepath.Walk(dir, func(path string, info os.FileInfo, err error) error {
			if err != nil || info.IsDir() { return nil }
			ext := strings.ToLower(filepath.Ext(path))
			if !mediaExts[ext] { return nil }

			entries = append(entries, MediaEntry{
				Path:       path,
				Name:       strings.TrimSuffix(info.Name(), filepath.Ext(info.Name())),
				Ext:        ext,
				SizeBytes:  info.Size(),
				ModifiedAt: info.ModTime(),
			})
			return nil
		})
	}

	mu.Lock()
	library = entries
	mu.Unlock()
	fmt.Printf("[indexer] %d fichiers indexés\n", len(entries))
}

// Référence pour éviter import inutilisé
var _ = metadata.PosterURL
