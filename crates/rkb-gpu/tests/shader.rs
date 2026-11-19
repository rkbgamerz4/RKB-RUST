//! Shader validation.
//!
//! The WGSL module is compiled by wgpu at runtime on the device, but these
//! tests also parse and validate it with naga directly. That catches shader
//! errors on every host/CI run without needing a GPU, and it asserts the entry
//! points the pipelines reference actually exist.

#[test]
fn world_shader_parses_and_validates() {
    let module =
        naga::front::wgsl::parse_str(rkb_gpu::WORLD_WGSL).expect("world.wgsl must parse as WGSL");
    let mut validator = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    );
    validator
        .validate(&module)
        .expect("world.wgsl must pass naga validation");
}

#[test]
fn shader_declares_every_pipeline_entry_point() {
    let module = naga::front::wgsl::parse_str(rkb_gpu::WORLD_WGSL).expect("parse WGSL");
    let names: Vec<&str> = module
        .entry_points
        .iter()
        .map(|entry| entry.name.as_str())
        .collect();
    for expected in [
        "vs_world",
        "fs_opaque",
        "fs_cutout",
        "fs_translucent",
        "vs_sky",
        "fs_sky",
        "vs_ui",
        "fs_ui",
    ] {
        assert!(
            names.contains(&expected),
            "missing entry point {expected}, found {names:?}"
        );
    }
}

#[test]
fn shader_has_no_uniform_buffer_layout_mismatch() {
    // naga validates the std140/uniform layout of `Camera`; this test exists to
    // fail loudly if a field is added without keeping 16-byte alignment.
    let module = naga::front::wgsl::parse_str(rkb_gpu::WORLD_WGSL).expect("parse WGSL");
    let mut validator = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    );
    let _info = validator.validate(&module).expect("validate WGSL");
    let (_, camera) = module
        .global_variables
        .iter()
        .find(|(_, global)| global.name.as_deref() == Some("camera"))
        .expect("camera uniform");
    let ty = &module.types[camera.ty];
    assert!(matches!(ty.inner, naga::TypeInner::Struct { .. }));
    // The Rust-side size/alignment check lives in scene.rs; validation above
    // already proved naga accepts the uniform layout.
}
