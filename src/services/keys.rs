//! Caps Lock / Num Lock toggle state.
//!
//! Read with `GetKeyState` five times a second. This deliberately avoids a
//! global keyboard hook or raw keyboard input: the notch never sees which
//! keys are typed, only the two lock states.

use std::time::Duration;

use tokio::runtime::Handle;
use tokio::time::MissedTickBehavior;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CAPITAL, VK_NUMLOCK};

use super::{EventBus, ServiceEvent};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LockKeys {
    pub caps: bool,
    pub num: bool,
}

fn read() -> LockKeys {
    unsafe { LockKeys { caps: GetKeyState(VK_CAPITAL.0 as i32) & 1 != 0, num: GetKeyState(VK_NUMLOCK.0 as i32) & 1 != 0 } }
}

pub fn spawn(rt: &Handle, bus: EventBus) {
    rt.spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(200));
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut last: Option<LockKeys> = None;
        loop {
            tick.tick().await;
            let state = read();
            if last != Some(state) {
                bus.send(ServiceEvent::LockKeys { state, initial: last.is_none() });
                last = Some(state);
            }
        }
    });
}
