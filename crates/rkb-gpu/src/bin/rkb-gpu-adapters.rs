//! `rkb-gpu-adapters`: enumerate every wgpu adapter this device can reach.
//!
//! M9 hardware-verification tool. Run it on a phone or in Termux:
//!
//! ```text
//! rkb-gpu-adapters
//! ```
//!
//! It prints one block per adapter with the exact strings diagnostics need:
//! name, backend, device type, driver, driver info, and whether
//! [`DeviceClass`] calls it silicon or a software rasterizer. It then tries to
//! create a device on each adapter so a driver that enumerates but fails to
//! open is reported instead of silently skipped.
//!
//! Nothing here claims hardware support by itself: an adapter is only called a
//! hardware GPU when [`DeviceClass::is_hardware`] says so, which requires both
//! a non-software device type and a name that is not a known software
//! rasterizer (`llvmpipe`, `lavapipe`, `SwiftShader`, `softpipe`, ...).

use rkb_gpu::scene::{BackendInfo, DeviceClass};
use rkb_gpu::{create_instance, default_backends};

fn main() {
    let backends = default_backends();
    println!("wgpu adapter probe");
    println!("backends: {backends:?}");
    println!();

    // `VK_ICD_FILENAMES` and `VK_DRIVER_FILES` are honoured by the loader, so
    // this reports whatever the environment exposes rather than a hardcoded
    // preference. Print them so a run can be reproduced exactly.
    for key in ["VK_ICD_FILENAMES", "VK_DRIVER_FILES", "WGPU_BACKEND"] {
        match std::env::var(key) {
            Ok(value) => println!("{key}={value}"),
            Err(_) => println!("{key}=<unset>"),
        }
    }
    println!();

    let instance = create_instance(backends);
    let adapters = instance.enumerate_adapters(backends);
    if adapters.is_empty() {
        println!("no adapters reachable for {backends:?}");
        std::process::exit(2);
    }

    println!("{} adapter(s):", adapters.len());
    let mut hardware = 0usize;
    for (index, adapter) in adapters.iter().enumerate() {
        let raw = adapter.get_info();
        let info = BackendInfo {
            name: raw.name.clone(),
            backend: format!("{:?}", raw.backend),
            device_type: format!("{:?}", raw.device_type),
            driver: raw.driver.clone(),
            driver_info: raw.driver_info.clone(),
        };
        let class = info.class();
        if class.is_hardware() {
            hardware += 1;
        }
        println!("[{index}] {}", info.label());
        println!("     device_type: {}", info.device_type);
        println!("     driver:      {}", info.driver);
        println!("     driver_info: {}", info.driver_info);
        println!("     class:       {}", class.label());

        // Enumerating is not enough: prove the adapter can actually open a
        // device with the limits the renderer uses.
        match pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("rkb-gpu-adapters-probe"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_defaults(),
            },
            None,
        )) {
            Ok(_) => println!("     device:      opened"),
            Err(error) => println!("     device:      FAILED: {error}"),
        }
        println!();
    }

    println!("hardware adapters: {hardware}");
    println!("software/unknown:  {}", adapters.len() - hardware);
    if hardware == 0 {
        println!();
        println!(
            "No hardware GPU was reachable. This is expected inside Termux when \
             the vendor Vulkan ICD is blocked by the linker namespace; it is \
             NOT evidence about the device's real GPU."
        );
    } else {
        println!();
        println!(
            "At least one adapter classified as hardware. Treat this as a \
             candidate for hardware verification, not as proof until a render \
             is captured and profiled."
        );
    }
}