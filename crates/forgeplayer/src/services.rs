#![allow(dead_code)]
use std::sync::{Arc, Mutex};
use std::thread;

/// Résultat d'une recherche de sous-titre.
#[derive(Debug, Clone, serde::Deserialize, Default)]
pub struct SubtitleResult {
    pub file_id:        u64,
    pub file_name:      String,
    pub language:       String,
    pub download_count: u64,
    pub ratings:        f32,
}

/// Entrée de la bibliothèque indexée par le service Go.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct MediaEntry {
    pub path:         String,
    pub name:         String,
    pub ext:          String,
    #[serde(rename = "sizeBytes")]
    pub size_bytes:   u64,
}

pub type AsyncResult<T> = Arc<Mutex<Option<Result<T, String>>>>;

/// Client léger pour les services Go (sous-titres + indexeur).
pub struct ServicesClient {
    subtitle_base: String,
    indexer_base:  String,
    client:        reqwest::blocking::Client,
}

impl ServicesClient {
    pub fn new(subtitle_port: u16, indexer_port: u16) -> Self {
        Self {
            subtitle_base: format!("http://127.0.0.1:{subtitle_port}"),
            indexer_base:  format!("http://127.0.0.1:{indexer_port}"),
            client: reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(8))
                .build()
                .unwrap_or_else(|_| reqwest::blocking::Client::new()),
        }
    }

    /// Vérifie que le service est disponible (health check rapide).
    pub fn is_subtitle_service_up(&self) -> bool {
        self.client.get(format!("{}/health", self.subtitle_base))
            .send().map(|r| r.status().is_success()).unwrap_or(false)
    }

    /// Recherche async de sous-titres — retourne un handle observable.
    pub fn search_subtitles_async(
        &self,
        filename: &str,
        lang:     &str,
    ) -> AsyncResult<Vec<SubtitleResult>> {
        let result: AsyncResult<Vec<SubtitleResult>> = Arc::new(Mutex::new(None));
        let result_clone = Arc::clone(&result);
        let url  = format!("{}/subtitles/search", self.subtitle_base);
        let client = self.client.clone();
        let filename = sanitize_filename(filename);
        let lang = lang.to_string();

        thread::spawn(move || {
            let resp = client.get(&url)
                .query(&[("filename", &filename), ("lang", &lang)])
                .send()
                .and_then(|r| r.json::<serde_json::Value>())
                .map_err(|e| e.to_string());

            let parsed = resp.and_then(|v| {
                v.get("data")
                    .and_then(|d| serde_json::from_value::<Vec<SubtitleResult>>(d.clone()).ok())
                    .ok_or_else(|| "format inattendu".into())
            });

            *result_clone.lock().unwrap() = Some(parsed);
        });

        result
    }

    /// Télécharge un sous-titre en arrière-plan.
    pub fn download_subtitle_async(
        &self,
        file_id:  u64,
        dest_dir: String,
    ) -> AsyncResult<String> {
        let result: AsyncResult<String> = Arc::new(Mutex::new(None));
        let result_clone = Arc::clone(&result);
        let url    = format!("{}/subtitles/download", self.subtitle_base);
        let client = self.client.clone();

        thread::spawn(move || {
            let body = serde_json::json!({"file_id": file_id, "dest_dir": dest_dir});
            let resp = client.post(&url)
                .json(&body)
                .send()
                .and_then(|r| r.json::<serde_json::Value>())
                .map(|v| v.get("path").and_then(|p| p.as_str()).unwrap_or("").to_string())
                .map_err(|e| e.to_string());
            *result_clone.lock().unwrap() = Some(resp);
        });

        result
    }

    /// Récupère la bibliothèque de médias indexée.
    pub fn get_library(&self) -> Vec<MediaEntry> {
        self.client.get(format!("{}/library", self.indexer_base))
            .send()
            .and_then(|r| r.json::<Vec<MediaEntry>>())
            .unwrap_or_default()
    }
}

fn sanitize_filename(name: &str) -> String {
    std::path::Path::new(name)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| name.to_string())
}
