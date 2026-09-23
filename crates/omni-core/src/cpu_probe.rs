//! Mesure du temps PROCESSEUR réellement consommé par un thread.
//!
//! Une mesure en temps écoulé (`Instant`) compte aussi les attentes : un
//! rapatriement GPU qui bloque 15 ms sur une synchronisation n'a coûté que
//! quelques millisecondes de processeur. Pour savoir où part le processeur, il
//! faut interroger l'horloge du thread lui-même.

use std::time::Instant;

/// Temps processeur (noyau + utilisateur) consommé par le thread courant.
#[cfg(windows)]
pub fn thread_cpu_secs() -> f64 {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetCurrentThread, GetThreadTimes};

    let (mut creation, mut exit, mut kernel, mut user) =
        (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    let ok = unsafe {
        GetThreadTimes(GetCurrentThread(), &mut creation, &mut exit, &mut kernel, &mut user)
    };
    if ok.is_err() { return 0.0; }
    let to_secs = |f: FILETIME| {
        (((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64) as f64 / 1e7
    };
    to_secs(kernel) + to_secs(user)
}

#[cfg(not(windows))]
pub fn thread_cpu_secs() -> f64 { 0.0 }

/// Sonde périodique : journalise le pourcentage d'un cœur consommé par le
/// thread courant. Silencieuse hors `RUST_LOG=debug`.
pub struct CpuProbe {
    label:    &'static str,
    last_at:  Instant,
    last_cpu: f64,
}

impl CpuProbe {
    pub fn new(label: &'static str) -> Self {
        Self { label, last_at: Instant::now(), last_cpu: thread_cpu_secs() }
    }

    /// À appeler dans la boucle du thread : ne fait rien tant que la période
    /// n'est pas écoulée, donc peut être appelée très souvent.
    pub fn tick(&mut self) {
        let wall = self.last_at.elapsed().as_secs_f64();
        if wall < 5.0 { return; }
        let cpu = thread_cpu_secs();
        log::debug!(
            "DBGCPU {}: {:.1} % d'un coeur ({:.2} s de processeur en {:.1} s)",
            self.label, (cpu - self.last_cpu) / wall * 100.0, cpu - self.last_cpu, wall
        );
        self.last_at  = Instant::now();
        self.last_cpu = cpu;
    }
}
