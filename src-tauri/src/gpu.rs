//! Notices when Windows resets the graphics driver: a driver crash or a GPU timeout ("TDR"). Every Direct3D device on
//! the GPU is lost then, the recorder's included, so its capture stops (ffmpeg can hang, still "recording") or ffmpeg
//! exits. Backtrack keeps one small device of its own, with no work on it, and asks it every couple of seconds whether
//! it's been removed.

use std::time::Duration;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{D3D11CreateDevice, ID3D11Device, D3D11_CREATE_DEVICE_FLAG, D3D11_SDK_VERSION};

/// How often the probe is asked (a cheap call: nothing is drawn or waited for).
const EVERY: Duration = Duration::from_secs(2);

struct Probe(ID3D11Device);

impl Probe {
    /// A device on the default GPU (the one driving the main display), or None while the driver isn't back yet.
    fn new() -> Option<Probe> {
        let mut device = None;
        unsafe {
            D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(), D3D11_CREATE_DEVICE_FLAG(0), None, D3D11_SDK_VERSION, Some(&mut device), None, None)
        }
        .ok()?;
        device.map(Probe)
    }

    /// The driver has been reset (or the GPU removed) since this probe was made.
    fn lost(&self) -> bool {
        unsafe { self.0.GetDeviceRemovedReason() }.is_err()
    }
}

/// Watch for driver resets on a thread of its own. `on_reset` runs on that thread after each one.
pub fn watch(on_reset: impl Fn() + Send + 'static) {
    let _ = std::thread::Builder::new().name("gpu-watch".into()).spawn(move || {
        // The probe stays on this thread: it's made, asked and dropped here.
        let mut probe: Option<Probe> = None;
        loop {
            std::thread::sleep(EVERY);
            match &probe {
                // At start, and after a reset once the driver is back.
                None => probe = Probe::new(),
                Some(p) if p.lost() => {
                    probe = None;
                    on_reset();
                }
                Some(_) => {}
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "needs a GPU"]
    fn a_fresh_probe_is_not_lost() {
        let p = Probe::new().expect("a Direct3D 11 device");
        assert!(!p.lost());
    }
}
