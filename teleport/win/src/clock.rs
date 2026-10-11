//! Monotone Uhr in µs auf Basis von QueryPerformanceCounter. Alle Prozesse eines Rechners sehen
//! denselben Zaehler; so kann der Selbsttest das Alter eines Bildes ueber Prozessgrenzen messen.

use std::sync::OnceLock;

use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

fn freq() -> i64 {
    static F: OnceLock<i64> = OnceLock::new();
    *F.get_or_init(|| {
        let mut f = 0i64;
        unsafe {
            let _ = QueryPerformanceFrequency(&mut f);
        }
        f.max(1)
    })
}

pub fn now_us() -> u64 {
    let mut c = 0i64;
    unsafe {
        let _ = QueryPerformanceCounter(&mut c);
    }
    (c as i128 * 1_000_000 / freq() as i128) as u64
}

#[cfg(test)]
mod tests {
    #[test]
    fn monotonic() {
        let a = super::now_us();
        std::thread::sleep(std::time::Duration::from_millis(5));
        let b = super::now_us();
        assert!(b >= a + 4000, "{a} {b}");
    }
}
