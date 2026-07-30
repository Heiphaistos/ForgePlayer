// Service sous-titres — exposé localement pour le lecteur Rust.
// Lance le bridge HTTP avec OpenSubtitles + TMDB.
package main

import (
	"flag"
	"fmt"
	"os"

	"forgeplayer/pkg/ipc"
)

func main() {
	port           := flag.Int("port", 18080, "Port d'écoute du service")
	subAPIKey      := flag.String("sub-key", os.Getenv("OPENSUBTITLES_API_KEY"), "Clé API OpenSubtitles")
	tmdbAPIKey     := flag.String("tmdb-key", os.Getenv("TMDB_API_KEY"), "Clé API TMDB")
	lang           := flag.String("lang", "fr", "Langue sous-titres (fr, en, ...)")
	flag.Parse()

	if *subAPIKey == "" {
		fmt.Fprintln(os.Stderr,
			"[warn] OPENSUBTITLES_API_KEY non définie — recherche sous-titres désactivée")
	}
	if *tmdbAPIKey == "" {
		fmt.Fprintln(os.Stderr,
			"[warn] TMDB_API_KEY non définie — métadonnées films désactivées")
	}

	addr   := fmt.Sprintf("127.0.0.1:%d", *port)
	bridge := ipc.NewBridge(*subAPIKey, *tmdbAPIKey, *lang)

	if err := bridge.ListenAndServe(addr); err != nil {
		fmt.Fprintf(os.Stderr, "[fatal] bridge: %v\n", err)
		os.Exit(1)
	}
}
