//! Android integration for RKB RUST.
//!
//! Exposes a small JNI surface consumed by `RkbNative.kt`. The Rust side owns
//! the runtime and renders into a Kotlin-provided direct `ByteBuffer`.
//!
//! # Threading
//!
//! Native state is not internally synchronized. Calls must be serialized by the
//! host: the Kotlin wrapper queues touch input for its single render thread and
//! stops/joins that thread before pause, resume, or destroy. Handles are
//! heap-allocated [`AndroidEngine`] pointers and must be destroyed exactly once.

#![allow(clippy::missing_safety_doc)]

mod bridge;

pub use bridge::{AndroidEngine, DirectBufferSurface};

use std::path::PathBuf;
use std::time::Duration;

use ::jni::objects::{JByteBuffer, JClass, JString};
use ::jni::sys::{jboolean, jint, jlong, jstring, JNI_FALSE, JNI_TRUE};
use ::jni::JNIEnv;

use bridge::{frame_bytes, touch_phase, EngineFrame};

const LOG_ERROR: i32 = 6;
const LOG_INFO: i32 = 4;

fn log(priority: i32, message: &str) {
    #[cfg(target_os = "android")]
    {
        use std::ffi::c_char;

        unsafe extern "C" {
            fn __android_log_write(prio: i32, tag: *const c_char, text: *const c_char) -> i32;
        }

        let mut text = message.as_bytes().to_vec();
        text.push(0);
        // SAFETY: `text` and the static tag are NUL-terminated and live for the call.
        unsafe {
            __android_log_write(priority, c"RKB-RUST".as_ptr(), text.as_ptr().cast());
        }
    }

    #[cfg(not(target_os = "android"))]
    {
        let level = if priority >= LOG_ERROR {
            "error"
        } else {
            "info"
        };
        eprintln!("[RKB-RUST/{level}] {message}");
    }
}

/// # Safety
///
/// `handle` must be zero or a pointer previously returned by `nativeCreate`
/// that has not been destroyed.
unsafe fn engine_mut<'a>(handle: jlong) -> Option<&'a mut AndroidEngine> {
    if handle == 0 {
        return None;
    }
    let pointer = handle as *mut AndroidEngine;
    if pointer.is_null() {
        return None;
    }
    // SAFETY: upheld by the caller contract; JNI is single-threaded here.
    Some(unsafe { &mut *pointer })
}

#[no_mangle]
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub extern "system" fn Java_com_rkb_rust_RkbNative_nativeCreate(
    mut env: JNIEnv,
    _class: JClass,
    width: jint,
    height: jint,
    world_path: jstring,
) -> jlong {
    let width = u32::try_from(width.max(1)).unwrap_or(1);
    let height = u32::try_from(height.max(1)).unwrap_or(1);
    let world = if world_path.is_null() {
        None
    } else {
        // SAFETY: JNI guarantees a valid local reference for the duration of
        // this call; we never retain the `JString`.
        let path = unsafe { JString::from_raw(world_path) };
        let converted = match env.get_string(&path) {
            Ok(text) => {
                let text: String = text.into();
                if text.trim().is_empty() {
                    None
                } else {
                    Some(PathBuf::from(text))
                }
            }
            Err(error) => {
                log(
                    LOG_ERROR,
                    &format!("world path is not valid UTF-8: {error}"),
                );
                None
            }
        };
        converted
    };
    let engine = AndroidEngine::new(width, height, world);
    log(
        LOG_INFO,
        &format!("native engine created ({width}x{height})"),
    );
    log(
        LOG_INFO,
        &format!("minecraft client: {}", engine.status_text()),
    );
    Box::into_raw(Box::new(engine)) as jlong
}

fn string_arg(env: &mut JNIEnv, value: jstring) -> Option<String> {
    if value.is_null() {
        return None;
    }
    // SAFETY: JNI guarantees a valid local reference for the duration of this
    // call; the string is converted immediately and never retained.
    let text = unsafe { JString::from_raw(value) };
    env.get_string(&text).ok().map(Into::into)
}

/// Starts an offline-mode server join without blocking the render thread.
///
/// `registry_path` must point at a `blocks.json` report the user generated from
/// their own 1.21.1 installation; RKB bundles no Mojang data. `resource_pack`
/// is an optional path to a user-supplied pack directory or `.zip`; an empty
/// or null value keeps the built-in procedural atlas.
#[no_mangle]
#[allow(clippy::not_unsafe_ptr_arg_deref)]
pub extern "system" fn Java_com_rkb_rust_RkbNative_nativeJoinServer(
    mut env: JNIEnv,
    _class: JClass,
    handle: jlong,
    server: jstring,
    name: jstring,
    registry_path: jstring,
    resource_pack: jstring,
) -> jboolean {
    let Some(engine) = (unsafe { engine_mut(handle) }) else {
        log(LOG_ERROR, "nativeJoinServer: invalid engine handle");
        return JNI_FALSE;
    };
    let Some(server) = string_arg(&mut env, server) else {
        log(LOG_ERROR, "nativeJoinServer: server address is missing");
        return JNI_FALSE;
    };
    let Some(name) = string_arg(&mut env, name) else {
        log(LOG_ERROR, "nativeJoinServer: player name is missing");
        return JNI_FALSE;
    };
    let Some(registry) = string_arg(&mut env, registry_path) else {
        log(LOG_ERROR, "nativeJoinServer: block report path is missing");
        return JNI_FALSE;
    };
    let pack = string_arg(&mut env, resource_pack)
        .map(|path| path.trim().to_owned())
        .filter(|path| !path.is_empty())
        .map(PathBuf::from);
    if let Some(path) = &pack {
        log(
            LOG_INFO,
            &format!("nativeJoinServer: resource pack {}", path.display()),
        );
    }
    match engine.begin_join(server, name, PathBuf::from(registry), pack) {
        Ok(()) => {
            log(LOG_INFO, "nativeJoinServer: connection started");
            JNI_TRUE
        }
        Err(error) => {
            log(LOG_ERROR, &format!("nativeJoinServer rejected: {error}"));
            JNI_FALSE
        }
    }
}

#[no_mangle]
pub extern "system" fn Java_com_rkb_rust_RkbNative_nativeDestroy(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) {
    if handle == 0 {
        return;
    }
    // SAFETY: handle originates from nativeCreate and Kotlin destroys once.
    let mut engine = unsafe { Box::from_raw(handle as *mut AndroidEngine) };
    engine.destroy();
    log(LOG_INFO, "native engine destroyed");
}

#[no_mangle]
pub extern "system" fn Java_com_rkb_rust_RkbNative_nativeResize(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
    width: jint,
    height: jint,
) -> jboolean {
    let Some(engine) = (unsafe { engine_mut(handle) }) else {
        return JNI_FALSE;
    };
    engine.resize(
        u32::try_from(width.max(1)).unwrap_or(1),
        u32::try_from(height.max(1)).unwrap_or(1),
    );
    JNI_TRUE
}

#[no_mangle]
pub extern "system" fn Java_com_rkb_rust_RkbNative_nativeRenderFrame(
    env: JNIEnv,
    _class: JClass,
    handle: jlong,
    buffer: JByteBuffer,
    width: jint,
    height: jint,
    delta_nanos: jlong,
) -> jboolean {
    let Some(engine) = (unsafe { engine_mut(handle) }) else {
        return JNI_FALSE;
    };
    let width = u32::try_from(width.max(1)).unwrap_or(1);
    let height = u32::try_from(height.max(1)).unwrap_or(1);
    let Some(expected) = frame_bytes(width, height) else {
        log(LOG_ERROR, "frame dimensions overflow");
        return JNI_FALSE;
    };

    let address = match env.get_direct_buffer_address(&buffer) {
        Ok(address) if !address.is_null() => address,
        _ => {
            log(LOG_ERROR, "frame buffer is not a direct ByteBuffer");
            return JNI_FALSE;
        }
    };
    let capacity = env.get_direct_buffer_capacity(&buffer).unwrap_or(0);
    if capacity < expected {
        log(
            LOG_ERROR,
            &format!("frame buffer too small: {capacity} < {expected}"),
        );
        return JNI_FALSE;
    }

    // SAFETY: JNI guarantees a direct buffer of `capacity` writable bytes for
    // the duration of this call, and capacity >= expected.
    let pixels = unsafe { std::slice::from_raw_parts_mut(address, expected) };
    let delta = Duration::from_nanos(u64::try_from(delta_nanos.max(0)).unwrap_or(0));
    match engine.frame(pixels, width, height, delta) {
        EngineFrame::Presented(_) => JNI_TRUE,
        // Nothing new was rendered (paused or not started). Report false so
        // the host does not present a stale buffer.
        EngineFrame::Skipped => JNI_FALSE,
        EngineFrame::Failed(error) => {
            log(LOG_ERROR, &error);
            JNI_FALSE
        }
    }
}

#[no_mangle]
pub extern "system" fn Java_com_rkb_rust_RkbNative_nativeTouch(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
    action: jint,
    pointer_id: jlong,
    x: f32,
    y: f32,
    time_ms: jlong,
) -> jboolean {
    let Some(engine) = (unsafe { engine_mut(handle) }) else {
        return JNI_FALSE;
    };
    let timestamp = Duration::from_millis(u64::try_from(time_ms.max(0)).unwrap_or(0));
    match touch_phase(action) {
        Some(phase) => {
            engine.touch(phase, pointer_id as u64, x, y, timestamp);
            JNI_TRUE
        }
        None => JNI_FALSE,
    }
}

#[no_mangle]
pub extern "system" fn Java_com_rkb_rust_RkbNative_nativePause(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jboolean {
    let Some(engine) = (unsafe { engine_mut(handle) }) else {
        return JNI_FALSE;
    };
    if engine.pause().is_ok() {
        JNI_TRUE
    } else {
        JNI_FALSE
    }
}

#[no_mangle]
pub extern "system" fn Java_com_rkb_rust_RkbNative_nativeResume(
    _env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jboolean {
    let Some(engine) = (unsafe { engine_mut(handle) }) else {
        return JNI_FALSE;
    };
    if engine.resume().is_ok() {
        JNI_TRUE
    } else {
        JNI_FALSE
    }
}

#[no_mangle]
pub extern "system" fn Java_com_rkb_rust_RkbNative_nativeStats(
    env: JNIEnv,
    _class: JClass,
    handle: jlong,
) -> jstring {
    let summary = (unsafe { engine_mut(handle) })
        .map(|engine| engine.stats_text())
        .unwrap_or_else(|| "no-engine".to_owned());
    env.new_string(summary)
        .map_or(std::ptr::null_mut(), ::jni::objects::JString::into_raw)
}

#[no_mangle]
pub extern "system" fn Java_com_rkb_rust_RkbNative_nativeVersion(
    env: JNIEnv,
    _class: JClass,
) -> jstring {
    env.new_string(rkb_core::VERSION)
        .map_or(std::ptr::null_mut(), ::jni::objects::JString::into_raw)
}
