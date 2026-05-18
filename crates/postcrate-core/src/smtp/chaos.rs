//! Chaos injection. Drives all controlled failures from a single
//! deterministic PRNG seeded per session (so a `seed` in the config
//! yields reproducible test runs).

use std::time::Duration;

use parking_lot::Mutex;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

use crate::db::chaos_configs::ChaosConfig;
use crate::smtp::response::SmtpReply;

#[derive(Clone)]
pub struct ChaosInjector {
    cfg: ChaosConfig,
    rng: std::sync::Arc<Mutex<ChaCha8Rng>>,
}

impl std::fmt::Debug for ChaosInjector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChaosInjector").field("cfg", &self.cfg).finish()
    }
}

impl ChaosInjector {
    pub fn new(cfg: ChaosConfig, fallback_seed: u64) -> Self {
        let seed = cfg.seed.unwrap_or(fallback_seed);
        Self {
            cfg,
            rng: std::sync::Arc::new(Mutex::new(ChaCha8Rng::seed_from_u64(seed))),
        }
    }

    pub fn enabled(&self) -> bool {
        self.cfg.enabled
    }

    pub fn delay(&self) -> Option<Duration> {
        if !self.cfg.enabled {
            return None;
        }
        if self.cfg.delay_ms_max == 0 {
            return None;
        }
        let mut rng = self.rng.lock();
        let lo = self.cfg.delay_ms_min;
        let hi = self.cfg.delay_ms_max.max(lo);
        if hi == 0 {
            return None;
        }
        let pick = rng.gen_range(lo..=hi);
        Some(Duration::from_millis(u64::from(pick)))
    }

    /// Roll for an unconditional rejection (regardless of command).
    pub fn maybe_reject(&self) -> Option<SmtpReply> {
        if !self.cfg.enabled {
            return None;
        }
        let mut rng = self.rng.lock();
        if rng.gen::<f32>() < self.cfg.reject_5xx_prob {
            return Some(SmtpReply::custom(550, "Chaos: rejected"));
        }
        if rng.gen::<f32>() < self.cfg.reject_4xx_prob {
            return Some(SmtpReply::custom(451, "Chaos: try again later"));
        }
        None
    }

    /// Roll for a malformed response (byte-level garbage instead of the
    /// real reply). Returns Some(bytes) when the chaos PRNG fires.
    pub fn maybe_malformed_bytes(&self) -> Option<Vec<u8>> {
        if !self.cfg.enabled || self.cfg.malformed_resp_prob == 0.0 {
            return None;
        }
        let mut rng = self.rng.lock();
        if rng.gen::<f32>() < self.cfg.malformed_resp_prob {
            // A code-less, partial line — well-behaved clients abort.
            return Some(b"x99 garbled response\r\n".to_vec());
        }
        None
    }

    /// Roll for a mid-DATA drop. Caller closes the socket on `true`.
    pub fn should_drop_during_data(&self) -> bool {
        if !self.cfg.enabled || self.cfg.drop_during_data_prob == 0.0 {
            return false;
        }
        let mut rng = self.rng.lock();
        rng.gen::<f32>() < self.cfg.drop_during_data_prob
    }
}
