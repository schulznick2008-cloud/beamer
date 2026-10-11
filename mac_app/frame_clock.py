"""Frame callbacks on the display refresh, with a common-mode timer fallback."""

import AppKit
import Quartz

# The Design page's tiles are thumbnails: at a ProMotion panel's 120Hz their hover doubled the
# page's work for nothing anyone can see, so they play at 60 at most.
PREVIEW_MAX_RATE = 60


class _Fire(AppKit.NSObject):
    def fire_(self, _sender):
        self.callback()


class FrameClock:
    """Calls `callback()` on the display refresh where macOS provides a display link."""

    def __init__(self, view, callback, max_rate=None):
        self.view = view
        self.callback = callback
        self.max_rate = max_rate
        self.source = None

    def start(self):
        if self.source is not None:
            return
        target = _Fire.alloc().init()
        target.callback = self.callback
        runloop = AppKit.NSRunLoop.mainRunLoop()
        window = None if self.view is None else self.view.window()
        # An overlay with no view yet plays on whichever display the crossing is at: paced to the
        # fastest, a ProMotion panel is never held to an external display's 60Hz.
        screen = window.screen() if window is not None else max(
            AppKit.NSScreen.screens() or [None], key=lambda each: getattr(each, "maximumFramesPerSecond", lambda: 0)() if each else 0)
        link_owner = screen if self.view is None else self.view
        if (screen is not None and (self.view is None or window is not None)
                and hasattr(link_owner, "displayLinkWithTarget_selector_")
                and hasattr(screen, "maximumFramesPerSecond")):
            link = link_owner.displayLinkWithTarget_selector_(target, "fire:")
            rate = max(1, int(screen.maximumFramesPerSecond()))
            if self.max_rate:
                rate = min(rate, self.max_rate)
            link.setPreferredFrameRateRange_(Quartz.CAFrameRateRangeMake(min(30, rate), rate, rate))
            link.addToRunLoop_forMode_(runloop, AppKit.NSRunLoopCommonModes)
            self.source = link
        else:
            timer = AppKit.NSTimer.timerWithTimeInterval_target_selector_userInfo_repeats_(
                1.0 / 60.0, target, "fire:", None, True
            )
            runloop.addTimer_forMode_(timer, AppKit.NSRunLoopCommonModes)
            self.source = timer

    def stop(self):
        source, self.source = self.source, None
        if source is not None:
            source.invalidate()

    def is_alive(self):
        return self.source is not None
