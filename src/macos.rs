use cocoa::appkit::{NSApp, NSApplication, NSEvent, NSEventMask};
use cocoa::foundation::{NSAutoreleasePool, NSDate, NSDefaultRunLoopMode};

pub fn poll_macos_events() -> Option<String> {
    unsafe {
        let app = NSApp();

        let pool = NSAutoreleasePool::new(cocoa::base::nil);

        let ns_event = app.nextEventMatchingMask_untilDate_inMode_dequeue_(
            NSEventMask::NSAnyEventMask.bits() | NSEventMask::NSEventMaskPressure.bits(),
            NSDate::distantPast(cocoa::base::nil),
            NSDefaultRunLoopMode,
            cocoa::base::YES);

        msg_send![pool, release];

        if ns_event == cocoa::base::nil {
            return None;
        }

        let data = ns_event.data1();
        let keycode = (data & 0xffff0000) >> 16;
        let keyflags = data & 0x0000ffff;
        let keystate = (keyflags & 0xff00) >> 8;

        if keystate == 0xa {
            if keycode == 16 {
                Some("playpause".into())
            } else if keycode == 17 || keycode == 19 {
                Some("next".into())
            } else if keycode == 18 || keycode == 20 {
                Some("previous".into())
            } else {
                None
            }
        } else {
            None
        }
    }
}
