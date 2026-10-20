use std::fs;
use std::io::{self, ErrorKind};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::Path;

const POWER_SUPPLIES: &str = "/sys/class/power_supply";

pub struct PowerState {
    pub battery_percent: u8,
    pub plugged: bool,
}

pub fn read_power_state() -> io::Result<PowerState> {
    let mut battery_percent = None;
    let mut plugged = false;

    for supply in fs::read_dir(POWER_SUPPLIES)? {
        let supply = supply?.path();
        match read_attribute(&supply, "type").as_deref() {
            // Mice and headsets also show up as batteries, marked with scope "Device".
            Some("Battery") if read_attribute(&supply, "scope").as_deref() != Some("Device") => {
                battery_percent = read_attribute(&supply, "capacity").and_then(|capacity| capacity.parse().ok());
            }
            Some("Mains" | "USB") => plugged |= read_attribute(&supply, "online").as_deref() == Some("1"),
            _ => {}
        }
    }

    let battery_percent = battery_percent.ok_or_else(|| io::Error::new(ErrorKind::NotFound, "no system battery found"))?;
    Ok(PowerState { battery_percent, plugged })
}

fn read_attribute(supply: &Path, attribute: &str) -> Option<String> {
    fs::read_to_string(supply.join(attribute)).ok().map(|value| value.trim().to_owned())
}

/// Kernel notifications about power supplies, so a charger being plugged in or
/// pulled out is noticed right away instead of at the next sample.
pub struct PowerEvents {
    socket: OwnedFd,
}

impl PowerEvents {
    pub fn listen() -> io::Result<Self> {
        const KERNEL_UEVENT_GROUP: u32 = 1;

        // SAFETY: plain socket/bind calls; the fd is owned from the moment it is checked.
        unsafe {
            let fd = libc::socket(
                libc::AF_NETLINK,
                libc::SOCK_DGRAM | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
                libc::NETLINK_KOBJECT_UEVENT,
            );
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            let socket = OwnedFd::from_raw_fd(fd);

            let mut address: libc::sockaddr_nl = std::mem::zeroed();
            address.nl_family = libc::AF_NETLINK as libc::sa_family_t;
            address.nl_groups = KERNEL_UEVENT_GROUP;
            let bound = libc::bind(
                socket.as_raw_fd(),
                (&raw const address).cast(),
                size_of::<libc::sockaddr_nl>() as libc::socklen_t,
            );
            if bound < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Self { socket })
        }
    }

    pub fn fd(&self) -> RawFd {
        self.socket.as_raw_fd()
    }

    /// Consumes every pending notification and reports whether any concerned a power supply.
    pub fn power_supply_changed(&self) -> bool {
        const MARKER: &[u8] = b"SUBSYSTEM=power_supply";

        let mut changed = false;
        let mut message = [0u8; 8192];
        loop {
            // SAFETY: recv writes at most `message.len()` bytes into `message`.
            let received = unsafe { libc::recv(self.socket.as_raw_fd(), message.as_mut_ptr().cast(), message.len(), 0) };
            if received <= 0 {
                return changed;
            }
            changed |= message[..received as usize].windows(MARKER.len()).any(|window| window == MARKER);
        }
    }
}
