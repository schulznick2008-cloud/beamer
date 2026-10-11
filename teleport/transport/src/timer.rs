//! Unter Windows laufen Socket-Timeouts und Sleeps standardmaessig in ~15,6-ms-Schritten. Die Fristen des
//! Transports (NACK nach wenigen ms, Frames nach 30 ms verwerfen) brauchen 1 ms. `timeBeginPeriod(1)` aus
//! winmm.dll (Systembibliothek) stellt das fuer den Prozess ein, solange eine Session offen ist.
//! Anderswo passiert nichts.

#[cfg(windows)]
mod imp {
    #[link(name = "winmm")]
    extern "system" {
        fn timeBeginPeriod(period: u32) -> u32;
        fn timeEndPeriod(period: u32) -> u32;
    }
    pub fn begin() {
        // SAFETY: reine Systemfunktion ohne Zeiger; Aufrufe sind paarweise (begin/end je Session).
        unsafe {
            timeBeginPeriod(1);
        }
    }
    pub fn end() {
        // SAFETY: siehe begin().
        unsafe {
            timeEndPeriod(1);
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn begin() {}
    pub fn end() {}
}

pub fn begin_fine_timer() {
    imp::begin();
}

pub fn end_fine_timer() {
    imp::end();
}
