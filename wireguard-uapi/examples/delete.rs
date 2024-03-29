use wg::{Backend, Device};

#[cfg(target_os = "linux")]
const BACKEND: Backend = Backend::Kernel;
#[cfg(not(target_os = "linux"))]
const BACKEND: Backend = Backend::Userspace;

fn main() {
    let devices = Device::list(BACKEND).unwrap();
    for iface in devices {
        let device = Device::get(&iface, BACKEND).unwrap();
        device.delete().unwrap();
        println!("delete wireguard interface: {}", iface.to_string());
    }
}
