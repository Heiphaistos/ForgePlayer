// Package subtitles — client OpenSubtitles REST v1.
// Recherche, télécharge et cache les sous-titres localement.
package subtitles

import (
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"time"
)

const (
	baseURL        = "https://api.opensubtitles.com/api/v1"
	userAgent      = "ForgePlayer v1.5"
	maxDownloadSize = 10 * 1024 * 1024 // 10 MB — subtitle files are never larger
)

// Client OpenSubtitles.
type Client struct {
	apiKey  string
	token   string
	http    *http.Client
	cacheDir string
}

// NewClient crée un client avec clé API.
// Clé gratuite disponible sur opensubtitles.com.
func NewClient(apiKey string) *Client {
	cacheDir := filepath.Join(os.TempDir(), "forgeplayer", "subtitles")
	_ = os.MkdirAll(cacheDir, 0755)

	return &Client{
		apiKey:   apiKey,
		http:     &http.Client{Timeout: 15 * time.Second},
		cacheDir: cacheDir,
	}
}

// SubtitleResult représente un résultat de recherche.
type SubtitleResult struct {
	ID         string  `json:"id"`
	Language   string  `json:"language"`
	FileName   string  `json:"file_name"`
	Rating     float64 `json:"ratings"`
	DownloadCount int   `json:"download_count"`
	FileID     int     `json:"file_id"`
}

type searchResponse struct {
	Data []struct {
		ID         string `json:"id"`
		Attributes struct {
			Language  string  `json:"language"`
			Files     []struct {
				FileID   int    `json:"file_id"`
				FileName string `json:"file_name"`
			} `json:"files"`
			Ratings       float64 `json:"ratings"`
			DownloadCount int     `json:"download_count"`
		} `json:"attributes"`
	} `json:"data"`
}

// Search cherche des sous-titres par nom de fichier + langue.
func (c *Client) Search(filename, lang string) ([]SubtitleResult, error) {
	q := url.Values{}
	q.Set("query", cleanFilename(filename))
	q.Set("languages", lang)
	q.Set("order_by", "download_count")

	req, err := http.NewRequest("GET", baseURL+"/subtitles?"+q.Encode(), nil)
	if err != nil {
		return nil, fmt.Errorf("search request: %w", err)
	}
	c.setHeaders(req)

	resp, err := c.http.Do(req)
	if err != nil {
		return nil, fmt.Errorf("search http: %w", err)
	}
	defer resp.Body.Close()

	if resp.StatusCode != 200 {
		return nil, fmt.Errorf("opensubtitles HTTP %d", resp.StatusCode)
	}

	var sr searchResponse
	if err := json.NewDecoder(resp.Body).Decode(&sr); err != nil {
		return nil, fmt.Errorf("decode: %w", err)
	}

	var results []SubtitleResult
	for _, d := range sr.Data {
		if len(d.Attributes.Files) == 0 {
			continue
		}
		results = append(results, SubtitleResult{
			ID:            d.ID,
			Language:      d.Attributes.Language,
			FileName:      d.Attributes.Files[0].FileName,
			Rating:        d.Attributes.Ratings,
			DownloadCount: d.Attributes.DownloadCount,
			FileID:        d.Attributes.Files[0].FileID,
		})
	}
	return results, nil
}

type downloadLinkResp struct {
	Link     string `json:"link"`
	FileName string `json:"file_name"`
}

// Download télécharge un sous-titre par file_id et retourne le chemin local.
func (c *Client) Download(fileID int, destDir string) (string, error) {
	// Vérifie le cache
	cachePath := filepath.Join(c.cacheDir, fmt.Sprintf("%d.srt", fileID))
	if _, err := os.Stat(cachePath); err == nil {
		return cachePath, nil
	}

	// Demande le lien de téléchargement
	payload, err := json.Marshal(map[string]int{"file_id": fileID})
	if err != nil {
		return "", fmt.Errorf("marshal download request: %w", err)
	}
	req, err := http.NewRequest("POST", baseURL+"/download", strings.NewReader(string(payload)))
	if err != nil {
		return "", err
	}
	c.setHeaders(req)
	req.Header.Set("Content-Type", "application/json")
	if c.token != "" {
		req.Header.Set("Authorization", "Bearer "+c.token)
	}

	resp, err := c.http.Do(req)
	if err != nil {
		return "", fmt.Errorf("download link: %w", err)
	}
	defer resp.Body.Close()

	var dlResp downloadLinkResp
	if err := json.NewDecoder(resp.Body).Decode(&dlResp); err != nil {
		return "", fmt.Errorf("decode download link: %w", err)
	}

	// Télécharge le fichier
	fileResp, err := c.http.Get(dlResp.Link)
	if err != nil {
		return "", fmt.Errorf("download file: %w", err)
	}
	defer fileResp.Body.Close()

	out, err := os.Create(cachePath)
	if err != nil {
		return "", err
	}
	defer out.Close()

	// Limit read to maxDownloadSize to prevent unbounded memory/disk consumption.
	if _, err := io.Copy(out, io.LimitReader(fileResp.Body, maxDownloadSize)); err != nil {
		return "", err
	}

	return cachePath, nil
}

func (c *Client) setHeaders(r *http.Request) {
	r.Header.Set("Api-Key", c.apiKey)
	r.Header.Set("User-Agent", userAgent)
	r.Header.Set("Accept", "application/json")
}

// cleanFilename normalise le nom de fichier pour la recherche.
func cleanFilename(name string) string {
	name = strings.TrimSuffix(name, filepath.Ext(name))
	// Supprime les patterns courants: [BluRay], (2023), etc.
	replacer := strings.NewReplacer(
		".", " ", "_", " ", "-", " ",
		"[BluRay]", "", "[1080p]", "", "[4K]", "", "[HDR]", "",
		"(", " ", ")", " ",
	)
	return strings.TrimSpace(replacer.Replace(name))
}
