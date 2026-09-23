use anyhow::Result;
use once_cell::sync::Lazy;
use regex::Regex;
use std::time::Duration;

static RE_HTML: Lazy<Regex> = Lazy::new(|| Regex::new(r"<[^>]+>").unwrap());
static RE_ASS:  Lazy<Regex> = Lazy::new(|| Regex::new(r"\{[^}]*\}").unwrap());

/// Une image de sous-titre décodée (PGS/HDMV, VOBSUB, DVB), convertie en RGBA.
///
/// Les formats bitmap ne portent pas de texte : le décodeur rend des rectangles
/// d'index de palette qu'il faut afficher tels quels par-dessus l'image. Les
/// coordonnées sont celles de la résolution d'origine du sous-titre, que
/// l'affichage ramène à la taille réelle de la vidéo.
#[derive(Debug, Clone)]
pub struct SubtitleBitmap {
    pub x:      u32,
    pub y:      u32,
    pub width:  u32,
    pub height: u32,
    /// Pixels RGBA non prémultipliés, `width * height * 4` octets.
    pub rgba:   Vec<u8>,
}

/// Événement subtitle avec texte formaté (ASS/SRT normalisé).
#[derive(Debug, Clone)]
pub struct SubtitleEvent {
    pub start:   Duration,
    pub end:     Duration,
    pub text:    String,   // texte brut sans balises
    pub ass_line: Option<String>, // ligne ASS originale pour rendu avancé
}

/// Parser léger SRT/ASS/VTT.
pub struct SubtitleTrack {
    pub events: Vec<SubtitleEvent>,
}

impl SubtitleTrack {
    pub fn from_srt(data: &str) -> Result<Self> {
        let mut events = Vec::new();
        let blocks: Vec<&str> = data.split("\n\n").collect();

        for block in blocks {
            let lines: Vec<&str> = block.trim().lines().collect();
            if lines.len() < 3 { continue; }

            // Ligne 1: index (ignoré)
            // Ligne 2: timecode  HH:MM:SS,ms --> HH:MM:SS,ms
            if let Some((start, end)) = parse_srt_timecodes(lines[1]) {
                let text = lines[2..].join("\n");
                let text = strip_html_tags(&text);
                events.push(SubtitleEvent { start, end, text, ass_line: None });
            }
        }

        Ok(Self { events })
    }

    pub fn from_ass(data: &str) -> Result<Self> {
        let mut events = Vec::new();

        for line in data.lines() {
            if !line.starts_with("Dialogue:") { continue; }
            let parts: Vec<&str> = line.splitn(10, ',').collect();
            if parts.len() < 10 { continue; }

            if let (Some(start), Some(end)) = (
                parse_ass_timecode(parts[1].trim()),
                parse_ass_timecode(parts[2].trim()),
            ) {
                let raw_text = parts[9].trim();
                let text = strip_ass_tags(raw_text);
                events.push(SubtitleEvent {
                    start,
                    end,
                    text,
                    ass_line: Some(line.to_string()),
                });
            }
        }

        Ok(Self { events })
    }

    /// Retourne le(s) event(s) actif(s) à un instant donné.
    pub fn events_at(&self, pos: Duration) -> impl Iterator<Item = &SubtitleEvent> {
        self.events
            .iter()
            .filter(move |e| e.start <= pos && pos <= e.end)
    }
}

fn parse_srt_timecodes(s: &str) -> Option<(Duration, Duration)> {
    let parts: Vec<&str> = s.split(" --> ").collect();
    if parts.len() != 2 { return None; }
    Some((parse_srt_ts(parts[0])?, parse_srt_ts(parts[1])?))
}

fn parse_srt_ts(s: &str) -> Option<Duration> {
    // HH:MM:SS,mmm
    let s = s.trim().replace(',', ".");
    let parts: Vec<&str> = s.splitn(3, ':').collect();
    if parts.len() != 3 { return None; }
    let h: u64 = parts[0].parse().ok()?;
    let m: u64 = parts[1].parse().ok()?;
    let sec: f64 = parts[2].parse().ok()?;
    let total_ms = h * 3_600_000 + m * 60_000 + (sec * 1000.0) as u64;
    Some(Duration::from_millis(total_ms))
}

fn parse_ass_timecode(s: &str) -> Option<Duration> {
    // H:MM:SS.cc
    let parts: Vec<&str> = s.splitn(3, ':').collect();
    if parts.len() != 3 { return None; }
    let h: u64 = parts[0].parse().ok()?;
    let m: u64 = parts[1].parse().ok()?;
    let sec: f64 = parts[2].parse().ok()?;
    let total_ms = h * 3_600_000 + m * 60_000 + (sec * 1000.0) as u64;
    Some(Duration::from_millis(total_ms))
}

fn strip_html_tags(s: &str) -> String {
    RE_HTML.replace_all(s, "").to_string()
}

fn strip_ass_tags(s: &str) -> String {
    RE_ASS.replace_all(s, "").to_string()
}
