//! Replay-Fenster: 4096 seq-Werte (SCHNITTSTELLEN.md 4.1).

pub const WINDOW: u64 = 4096;
const WORDS: usize = (WINDOW / 64) as usize;

pub struct ReplayWindow {
    top: u64,
    bits: [u64; WORDS],
}

impl Default for ReplayWindow {
    fn default() -> Self {
        Self::new()
    }
}

impl ReplayWindow {
    pub fn new() -> Self {
        ReplayWindow {
            top: 0,
            bits: [0; WORDS],
        }
    }

    /// Hoechste bisher angenommene seq (0 = noch keine).
    pub fn top(&self) -> u64 {
        self.top
    }

    fn slot(seq: u64) -> (usize, u64) {
        let i = seq % WINDOW;
        ((i / 64) as usize, 1u64 << (i % 64))
    }

    /// Wuerde `seq` angenommen? Aendert nichts.
    pub fn check(&self, seq: u64) -> bool {
        if seq == 0 {
            return false;
        }
        if seq > self.top {
            return true;
        }
        if self.top - seq >= WINDOW {
            return false;
        }
        let (w, m) = Self::slot(seq);
        self.bits[w] & m == 0
    }

    /// Erst nach erfolgreicher Authentifizierung aufrufen.
    pub fn commit(&mut self, seq: u64) {
        if seq > self.top {
            let delta = seq - self.top;
            if delta >= WINDOW {
                self.bits = [0; WORDS];
            } else {
                for s in self.top + 1..=seq {
                    let (w, m) = Self::slot(s);
                    self.bits[w] &= !m;
                }
            }
            self.top = seq;
        }
        let (w, m) = Self::slot(seq);
        self.bits[w] |= m;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicates_and_old() {
        let mut r = ReplayWindow::new();
        assert!(!r.check(0));
        assert!(r.check(1));
        r.commit(1);
        assert!(!r.check(1));
        r.commit(10);
        assert!(r.check(5));
        r.commit(5);
        assert!(!r.check(5));
        r.commit(10 + WINDOW);
        assert!(!r.check(10), "zu alt");
        assert!(r.check(10 + WINDOW - 1));
        assert!(!r.check(10 + WINDOW));
    }

    #[test]
    fn big_jump_clears() {
        let mut r = ReplayWindow::new();
        for s in 1..100 {
            r.commit(s);
        }
        r.commit(100_000);
        assert!(!r.check(100_000));
        assert!(r.check(99_999));
        assert!(!r.check(99));
    }
}
