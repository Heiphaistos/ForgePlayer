// Package metadata — client TMDB (The Movie Database) pour les métadonnées.
// Récupère titre, synopsis, affiche, année, note, casting.
package metadata

import (
	"encoding/json"
	"fmt"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"regexp"
	"time"
)

const tmdbBase = "https://api.themoviedb.org/3"
const imageBase = "https://image.tmdb.org/t/p/w500"

// titleRegex valide les titres de films/séries pour prévenir les injections et abus SSRF.
// Autorise les caractères alphanumériques, accents courants et ponctuation de titre.
var titleRegex = regexp.MustCompile(`^[a-zA-ZÀ-ÿ0-9\s\-\.',:!?()\[\]]{1,200}$`)

// validateTitle retourne une erreur si le titre ne correspond pas au format attendu.
func validateTitle(title string) error {
	if !titleRegex.MatchString(title) {
		return fmt.Errorf("titre invalide: %q", title)
	}
	return nil
}

// Client TMDB.
type Client struct {
	apiKey  string
	lang    string
	http    *http.Client
	cacheDir string
}

func NewTMDBClient(apiKey, lang string) *Client {
	cacheDir := filepath.Join(os.TempDir(), "forgeplayer", "metadata")
	_ = os.MkdirAll(cacheDir, 0755)
	return &Client{
		apiKey:   apiKey,
		lang:     lang,
		http:     &http.Client{Timeout: 10 * time.Second},
		cacheDir: cacheDir,
	}
}

// MovieInfo données d'un film.
type MovieInfo struct {
	ID          int     `json:"id"`
	Title       string  `json:"title"`
	OriginalTitle string `json:"original_title"`
	Overview    string  `json:"overview"`
	ReleaseDate string  `json:"release_date"`
	PosterPath  string  `json:"poster_path"`
	BackdropPath string  `json:"backdrop_path"`
	VoteAverage float64 `json:"vote_average"`
	Runtime     int     `json:"runtime"`
	Genres      []struct {
		Name string `json:"name"`
	} `json:"genres"`
}

// TVInfo données d'une série TV.
type TVInfo struct {
	ID           int     `json:"id"`
	Name         string  `json:"name"`
	Overview     string  `json:"overview"`
	FirstAirDate string  `json:"first_air_date"`
	PosterPath   string  `json:"poster_path"`
	VoteAverage  float64 `json:"vote_average"`
	NumberOfSeasons int  `json:"number_of_seasons"`
}

type searchMovieResp struct {
	Results []MovieInfo `json:"results"`
}

type searchTVResp struct {
	Results []TVInfo `json:"results"`
}

// SearchMovie cherche un film par titre.
func (c *Client) SearchMovie(title string) ([]MovieInfo, error) {
	// H9 — valider le titre avant de l'injecter dans la requête TMDB
	if err := validateTitle(title); err != nil {
		return nil, err
	}
	q := url.Values{
		"api_key":       {c.apiKey},
		"language":      {c.lang},
		"query":         {url.QueryEscape(title)},
		"include_adult": {"false"},
	}
	var resp searchMovieResp
	if err := c.get("/search/movie", q, &resp); err != nil {
		return nil, err
	}
	return resp.Results, nil
}

// SearchTV cherche une série TV par titre.
func (c *Client) SearchTV(title string) ([]TVInfo, error) {
	// H9 — valider le titre avant de l'injecter dans la requête TMDB
	if err := validateTitle(title); err != nil {
		return nil, err
	}
	q := url.Values{
		"api_key":  {c.apiKey},
		"language": {c.lang},
		"query":    {url.QueryEscape(title)},
	}
	var resp searchTVResp
	if err := c.get("/search/tv", q, &resp); err != nil {
		return nil, err
	}
	return resp.Results, nil
}

// GetMovie retourne les détails complets d'un film par ID.
func (c *Client) GetMovie(id int) (*MovieInfo, error) {
	// Cache disque
	cachePath := filepath.Join(c.cacheDir, fmt.Sprintf("movie_%d.json", id))
	if data, err := os.ReadFile(cachePath); err == nil {
		var info MovieInfo
		if json.Unmarshal(data, &info) == nil {
			return &info, nil
		}
	}

	q := url.Values{"api_key": {c.apiKey}, "language": {c.lang}}
	var info MovieInfo
	if err := c.get(fmt.Sprintf("/movie/%d", id), q, &info); err != nil {
		return nil, err
	}

	// Sauvegarde dans le cache
	if data, err := json.Marshal(info); err == nil {
		_ = os.WriteFile(cachePath, data, 0644)
	}

	return &info, nil
}

// PosterURL retourne l'URL complète de l'affiche.
func PosterURL(path string) string {
	if path == "" { return "" }
	return imageBase + path
}

func (c *Client) get(endpoint string, params url.Values, dest any) error {
	rawURL := tmdbBase + endpoint + "?" + params.Encode()
	resp, err := c.http.Get(rawURL)
	if err != nil {
		return fmt.Errorf("tmdb GET %s: %w", endpoint, err)
	}
	defer resp.Body.Close()
	if resp.StatusCode != 200 {
		return fmt.Errorf("tmdb HTTP %d pour %s", resp.StatusCode, endpoint)
	}
	return json.NewDecoder(resp.Body).Decode(dest)
}
